//! Translate upstream OpenAI SSE chunks into Anthropic SSE events.
//!
//! Directory `anthropic_to_openai/` = client speaks Anthropic, upstream speaks OpenAI.
//! Stream direction: upstream → client, so we translate OpenAI chunks → Anthropic events.

use std::collections::HashMap;

use serde_json::{Value, json};

fn sse(event: &str, data: &Value) -> String {
    format!("event: {event}\ndata: {}\n\n", data)
}

#[allow(dead_code)]
struct ToolBlockState {
    anth_block_index: usize,
    id: String,
    name: String,
}

pub struct OpenAiToAnthropicStream {
    message_id: String,
    model: String,
    text_block_open: bool,
    tool_blocks: HashMap<usize, ToolBlockState>,
    next_anth_block_index: usize,
    cached_tokens: u64,
    input_tokens: u64,
    output_tokens: u64,
    stop_reason: Option<&'static str>,
    started: bool,
    closed: bool,
}

impl Default for OpenAiToAnthropicStream {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenAiToAnthropicStream {
    pub fn new() -> Self {
        // Generate a message ID using a simple timestamp-based approach since uuid isn't needed
        // to be truly random for tests; real usage gets uuid from the proxy layer.
        let id = format!("msg_{}", uuid_like());
        Self {
            message_id: id,
            model: String::new(),
            text_block_open: false,
            tool_blocks: HashMap::new(),
            next_anth_block_index: 0,
            cached_tokens: 0,
            input_tokens: 0,
            output_tokens: 0,
            stop_reason: None,
            started: false,
            closed: false,
        }
    }

    /// Feed one upstream OpenAI chunk; emit zero or more Anthropic SSE event strings.
    /// Each returned string includes the trailing blank line (`\n\n`).
    pub fn feed_chunk(&mut self, chunk: &Value) -> Vec<String> {
        if self.closed {
            return Vec::new();
        }

        let mut out = Vec::new();

        // Extract the single choice (index 0)
        let choice = chunk.get("choices").and_then(|c| c.get(0));
        let delta = choice.and_then(|c| c.get("delta")).unwrap_or(&Value::Null);
        let finish_reason = choice.and_then(|c| c.get("finish_reason")).and_then(|f| f.as_str());

        // First chunk ever: emit message_start
        if !self.started {
            self.started = true;

            // Capture model from this chunk (may be absent in later chunks)
            if let Some(m) = chunk.get("model").and_then(|v| v.as_str()) {
                self.model = m.to_string();
            }

            // Capture id from this chunk
            let msg_id = chunk
                .get("id")
                .and_then(|v| v.as_str())
                .map(|s| {
                    if s.starts_with("msg_") {
                        s.to_string()
                    } else {
                        format!("msg_{s}")
                    }
                })
                .unwrap_or_else(|| self.message_id.clone());
            self.message_id = msg_id.clone();

            let message_start = json!({
                "type": "message_start",
                "message": {
                    "id": msg_id,
                    "type": "message",
                    "role": "assistant",
                    "model": self.model,
                    "content": [],
                    "stop_reason": Value::Null,
                    "stop_sequence": Value::Null,
                    "usage": {
                        "input_tokens": self.input_tokens,
                        "output_tokens": 0u64
                    }
                }
            });
            out.push(sse("message_start", &message_start));
        } else {
            // Update model if present in later chunks (some providers send it)
            if let Some(m) = chunk.get("model").and_then(|v| v.as_str())
                && !m.is_empty()
            {
                self.model = m.to_string();
            }
        }

        // Capture usage if present
        if let Some(usage) = chunk.get("usage") {
            if let Some(prompt) = usage.get("prompt_tokens").and_then(|v| v.as_u64()) {
                self.input_tokens = prompt;
            }
            if let Some(completion) = usage.get("completion_tokens").and_then(|v| v.as_u64()) {
                self.output_tokens = completion;
            }
            if let Some(cached) = usage.pointer("/prompt_tokens_details/cached_tokens").and_then(|v| v.as_u64()) {
                self.cached_tokens = cached;
            }
        }

        // Stash finish_reason for later (emit on finish())
        if let Some(reason) = finish_reason {
            self.stop_reason = Some(map_finish_reason(reason));
        }

        // Process delta
        if let Some(delta_obj) = delta.as_object() {
            // Text content delta
            if let Some(content_val) = delta_obj.get("content") {
                let text = content_val.as_str().unwrap_or("");
                if !text.is_empty() {
                    if !self.text_block_open {
                        // Open text block at index 0
                        let block_start = json!({
                            "type": "content_block_start",
                            "index": 0,
                            "content_block": {"type": "text", "text": ""}
                        });
                        out.push(sse("content_block_start", &block_start));
                        self.text_block_open = true;
                        // text block always gets index 0
                        if self.next_anth_block_index == 0 {
                            self.next_anth_block_index = 1;
                        }
                    }
                    let delta_event = json!({
                        "type": "content_block_delta",
                        "index": 0,
                        "delta": {"type": "text_delta", "text": text}
                    });
                    out.push(sse("content_block_delta", &delta_event));
                }
            }

            // Tool calls delta
            if let Some(tool_calls_val) = delta_obj.get("tool_calls").and_then(|v| v.as_array()) {
                for tc in tool_calls_val {
                    let oi_index = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;

                    let is_new_tool = !self.tool_blocks.contains_key(&oi_index);

                    if is_new_tool {
                        // Close text block if open
                        if self.text_block_open {
                            let stop = json!({"type": "content_block_stop", "index": 0});
                            out.push(sse("content_block_stop", &stop));
                            self.text_block_open = false;
                        }

                        let anth_idx = self.next_anth_block_index;
                        self.next_anth_block_index += 1;

                        let id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let name = tc
                            .pointer("/function/name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();

                        let block_start = json!({
                            "type": "content_block_start",
                            "index": anth_idx,
                            "content_block": {
                                "type": "tool_use",
                                "id": id,
                                "name": name,
                                "input": {}
                            }
                        });
                        out.push(sse("content_block_start", &block_start));

                        self.tool_blocks.insert(
                            oi_index,
                            ToolBlockState {
                                anth_block_index: anth_idx,
                                id,
                                name,
                            },
                        );
                    }

                    // Emit argument delta if present
                    if let Some(args) = tc.pointer("/function/arguments").and_then(|v| v.as_str())
                        && !args.is_empty()
                        && let Some(state) = self.tool_blocks.get(&oi_index)
                    {
                        let anth_idx = state.anth_block_index;
                        let delta_event = json!({
                            "type": "content_block_delta",
                            "index": anth_idx,
                            "delta": {
                                "type": "input_json_delta",
                                "partial_json": args
                            }
                        });
                        out.push(sse("content_block_delta", &delta_event));
                    }
                }
            }
        }

        out
    }

    /// Mark stream complete — emits any pending content_block_stop + message_delta + message_stop.
    pub fn finish(&mut self) -> Vec<String> {
        if self.closed {
            return Vec::new();
        }
        self.closed = true;

        let mut out = Vec::new();

        // Ensure message_start was emitted (handles case where finish() called without any chunks)
        if !self.started {
            self.started = true;
            let message_start = json!({
                "type": "message_start",
                "message": {
                    "id": self.message_id,
                    "type": "message",
                    "role": "assistant",
                    "model": self.model,
                    "content": [],
                    "stop_reason": Value::Null,
                    "stop_sequence": Value::Null,
                    "usage": {"input_tokens": 0u64, "output_tokens": 0u64}
                }
            });
            out.push(sse("message_start", &message_start));
        }

        // Close text block if open
        if self.text_block_open {
            let stop = json!({"type": "content_block_stop", "index": 0});
            out.push(sse("content_block_stop", &stop));
            self.text_block_open = false;
        }

        // Close all tool blocks — sort by anth_block_index for deterministic ordering
        let mut tool_indices: Vec<(usize, usize)> = self
            .tool_blocks
            .values()
            .map(|s| (s.anth_block_index, s.anth_block_index))
            .collect();
        tool_indices.sort_by_key(|(idx, _)| *idx);
        for (anth_idx, _) in tool_indices {
            let stop = json!({"type": "content_block_stop", "index": anth_idx});
            out.push(sse("content_block_stop", &stop));
        }

        // message_delta
        let mut usage = json!({"output_tokens": self.output_tokens});
        if self.cached_tokens > 0 {
            usage["cache_read_input_tokens"] = json!(self.cached_tokens);
        }
        let msg_delta = json!({
            "type": "message_delta",
            "delta": {
                "stop_reason": self.stop_reason.unwrap_or("end_turn"),
                "stop_sequence": Value::Null
            },
            "usage": usage
        });
        out.push(sse("message_delta", &msg_delta));

        // message_stop
        let msg_stop = json!({"type": "message_stop"});
        out.push(sse("message_stop", &msg_stop));

        out
    }
}

fn map_finish_reason(reason: &str) -> &'static str {
    match reason {
        "stop" => "end_turn",
        "tool_calls" => "tool_use",
        "length" => "max_tokens",
        "content_filter" => {
            tracing::warn!(
                target: "translation",
                "openai content_filter mapped to anthropic end_turn"
            );
            "end_turn"
        }
        other => {
            tracing::debug!(
                target: "translation",
                finish_reason = %other,
                "unknown finish_reason; using end_turn"
            );
            "end_turn"
        }
    }
}

/// Simple pseudo-unique ID generator (avoids adding uuid dependency here since
/// real usage is driven from outside; just needs to be unique enough for tests).
fn uuid_like() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("{nanos:08x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse an Anthropic SSE string into (event_name, data_value).
    fn parse_anth_event(s: &str) -> (String, Value) {
        let mut event_name = String::new();
        let mut data = String::new();
        for line in s.lines() {
            if let Some(n) = line.strip_prefix("event: ") {
                event_name = n.to_string();
            } else if let Some(d) = line.strip_prefix("data: ") {
                data = d.to_string();
            }
        }
        (
            event_name,
            serde_json::from_str(&data).unwrap_or(Value::Null),
        )
    }

    fn chunk(id: &str, model: &str, delta: Value, finish_reason: Option<&str>) -> Value {
        json!({
            "id": id,
            "object": "chat.completion.chunk",
            "created": 1_700_000_000u64,
            "model": model,
            "choices": [{
                "index": 0,
                "delta": delta,
                "finish_reason": finish_reason
            }]
        })
    }

    fn collect(fsm: &mut OpenAiToAnthropicStream, chunks: &[Value]) -> Vec<(String, Value)> {
        let mut events = Vec::new();
        for c in chunks {
            for s in fsm.feed_chunk(c) {
                events.push(parse_anth_event(&s));
            }
        }
        for s in fsm.finish() {
            events.push(parse_anth_event(&s));
        }
        events
    }

    #[test]
    fn text_only_response_emits_events_in_order() {
        let mut fsm = OpenAiToAnthropicStream::new();
        let chunks = vec![
            chunk("c1", "glm-4", json!({"role": "assistant"}), None),
            chunk("c1", "glm-4", json!({"content": "Hello "}), None),
            chunk("c1", "glm-4", json!({"content": "world"}), None),
            chunk("c1", "glm-4", json!({}), Some("stop")),
        ];
        let events = collect(&mut fsm, &chunks);
        let names: Vec<&str> = events.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            &[
                "message_start",
                "content_block_start",
                "content_block_delta",
                "content_block_delta",
                "content_block_stop",
                "message_delta",
                "message_stop"
            ]
        );
        assert_eq!(events[0].1["type"], "message_start");
        assert_eq!(events[1].1["content_block"]["type"], "text");
        assert_eq!(events[2].1["delta"]["text"], "Hello ");
        assert_eq!(events[3].1["delta"]["text"], "world");
        assert_eq!(events[5].1["delta"]["stop_reason"], "end_turn");
    }

    #[test]
    fn single_tool_call_streams_args_across_chunks() {
        let mut fsm = OpenAiToAnthropicStream::new();
        let chunks = vec![
            chunk("c1", "glm-4", json!({"role": "assistant"}), None),
            chunk(
                "c1",
                "glm-4",
                json!({"tool_calls": [{"index": 0, "id": "call_1", "type": "function", "function": {"name": "search", "arguments": ""}}]}),
                None,
            ),
            chunk(
                "c1",
                "glm-4",
                json!({"tool_calls": [{"index": 0, "function": {"arguments": "\"q\":\"hel"}}]}),
                None,
            ),
            chunk(
                "c1",
                "glm-4",
                json!({"tool_calls": [{"index": 0, "function": {"arguments": "lo\""}}]}),
                None,
            ),
            chunk("c1", "glm-4", json!({}), Some("tool_calls")),
        ];
        let events = collect(&mut fsm, &chunks);
        let names: Vec<&str> = events.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            &[
                "message_start",
                "content_block_start",
                "content_block_delta",
                "content_block_delta",
                "content_block_stop",
                "message_delta",
                "message_stop"
            ]
        );
        // content_block_start should be tool_use
        assert_eq!(events[1].1["content_block"]["type"], "tool_use");
        assert_eq!(events[1].1["content_block"]["name"], "search");
        assert_eq!(events[1].1["content_block"]["id"], "call_1");
        // deltas are input_json_delta
        assert_eq!(events[2].1["delta"]["type"], "input_json_delta");
        assert_eq!(events[2].1["delta"]["partial_json"], "\"q\":\"hel");
        assert_eq!(events[3].1["delta"]["partial_json"], "lo\"");
        // message_delta stop_reason = tool_use
        assert_eq!(events[5].1["delta"]["stop_reason"], "tool_use");
    }

    #[test]
    fn text_then_tool_call_closes_text_block_first() {
        let mut fsm = OpenAiToAnthropicStream::new();
        let chunks = vec![
            chunk("c1", "m", json!({"role": "assistant"}), None),
            chunk("c1", "m", json!({"content": "Thinking..."}), None),
            chunk(
                "c1",
                "m",
                json!({"tool_calls": [{"index": 0, "id": "tc1", "type": "function", "function": {"name": "calc", "arguments": ""}}]}),
                None,
            ),
            chunk(
                "c1",
                "m",
                json!({"tool_calls": [{"index": 0, "function": {"arguments": "{}"}}]}),
                None,
            ),
            chunk("c1", "m", json!({}), Some("tool_calls")),
        ];
        let events = collect(&mut fsm, &chunks);
        let names: Vec<&str> = events.iter().map(|(n, _)| n.as_str()).collect();
        // Expected: message_start, cbs(text), cbd(text), cbs_stop(text), cbs(tool), cbd(tool), cbs_stop(tool), message_delta, message_stop
        assert_eq!(names[0], "message_start");
        assert_eq!(names[1], "content_block_start"); // text
        assert_eq!(events[1].1["content_block"]["type"], "text");
        assert_eq!(names[2], "content_block_delta"); // text delta
        assert_eq!(names[3], "content_block_stop"); // text block closed BEFORE tool
        assert_eq!(events[3].1["index"], 0); // text was index 0
        assert_eq!(names[4], "content_block_start"); // tool block
        assert_eq!(events[4].1["content_block"]["type"], "tool_use");
        // tool block should have index >= 1
        let tool_idx = events[4].1["index"].as_u64().unwrap();
        assert!(tool_idx >= 1, "tool block index should be >= 1, got {tool_idx}");
    }

    #[test]
    fn two_parallel_tool_calls_get_separate_block_indices() {
        let mut fsm = OpenAiToAnthropicStream::new();
        let chunks = vec![
            chunk("c1", "m", json!({"role": "assistant"}), None),
            // First tool call introduced
            chunk(
                "c1",
                "m",
                json!({"tool_calls": [{"index": 0, "id": "tc0", "type": "function", "function": {"name": "tool_a", "arguments": ""}}]}),
                None,
            ),
            // Second tool call introduced
            chunk(
                "c1",
                "m",
                json!({"tool_calls": [{"index": 1, "id": "tc1", "type": "function", "function": {"name": "tool_b", "arguments": ""}}]}),
                None,
            ),
            // Args for tool 0
            chunk(
                "c1",
                "m",
                json!({"tool_calls": [{"index": 0, "function": {"arguments": "{\"a\":1}"}}]}),
                None,
            ),
            // Args for tool 1
            chunk(
                "c1",
                "m",
                json!({"tool_calls": [{"index": 1, "function": {"arguments": "{\"b\":2}"}}]}),
                None,
            ),
            chunk("c1", "m", json!({}), Some("tool_calls")),
        ];
        let events = collect(&mut fsm, &chunks);

        // Find all content_block_start events
        let cbs_events: Vec<&(String, Value)> = events
            .iter()
            .filter(|(n, _)| n == "content_block_start")
            .collect();
        assert_eq!(cbs_events.len(), 2, "should have 2 tool block starts");

        let idx0 = cbs_events[0].1["index"].as_u64().unwrap();
        let idx1 = cbs_events[1].1["index"].as_u64().unwrap();
        assert_ne!(idx0, idx1, "two tool blocks must have different indices");

        assert_eq!(cbs_events[0].1["content_block"]["name"], "tool_a");
        assert_eq!(cbs_events[1].1["content_block"]["name"], "tool_b");
    }

    #[test]
    fn finish_reason_mappings() {
        let cases = [
            ("stop", "end_turn"),
            ("tool_calls", "tool_use"),
            ("length", "max_tokens"),
            ("content_filter", "end_turn"),
        ];
        for (openai_reason, anth_reason) in cases {
            let mut fsm = OpenAiToAnthropicStream::new();
            let chunks = vec![
                chunk("id", "m", json!({"role": "assistant"}), None),
                chunk("id", "m", json!({}), Some(openai_reason)),
            ];
            let events = collect(&mut fsm, &chunks);
            let msg_delta = events
                .iter()
                .find(|(n, _)| n == "message_delta")
                .expect("message_delta should be emitted");
            assert_eq!(
                msg_delta.1["delta"]["stop_reason"],
                anth_reason,
                "finish_reason {openai_reason} should map to {anth_reason}"
            );
        }
    }

    #[test]
    fn empty_content_emits_only_message_lifecycle() {
        let mut fsm = OpenAiToAnthropicStream::new();
        let chunks = vec![
            chunk("c1", "m", json!({"role": "assistant"}), None),
            chunk("c1", "m", json!({}), Some("stop")),
        ];
        let events = collect(&mut fsm, &chunks);
        let names: Vec<&str> = events.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, &["message_start", "message_delta", "message_stop"]);
    }

    #[test]
    fn usage_in_final_chunk_propagates_to_message_delta() {
        let mut fsm = OpenAiToAnthropicStream::new();
        let final_chunk = {
            let mut c = chunk("c1", "m", json!({}), Some("stop"));
            c["usage"] = json!({
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "prompt_tokens_details": {"cached_tokens": 3}
            });
            c
        };
        let chunks = vec![
            chunk("c1", "m", json!({"role": "assistant"}), None),
            final_chunk,
        ];
        let events = collect(&mut fsm, &chunks);
        let msg_delta = events
            .iter()
            .find(|(n, _)| n == "message_delta")
            .expect("message_delta should be emitted");
        assert_eq!(msg_delta.1["usage"]["output_tokens"], 5u64);
        assert_eq!(msg_delta.1["usage"]["cache_read_input_tokens"], 3u64);
    }

    #[test]
    fn done_sentinel_after_finish_is_idempotent() {
        let mut fsm = OpenAiToAnthropicStream::new();
        let chunks = vec![chunk("c1", "m", json!({"role": "assistant"}), None)];
        for c in &chunks {
            let _ = fsm.feed_chunk(c);
        }
        let first = fsm.finish();
        let second = fsm.finish();
        assert!(!first.is_empty(), "first finish should emit events");
        assert!(second.is_empty(), "second finish should emit nothing");
    }
}
