//! Translate upstream Anthropic SSE events into OpenAI chat.completion.chunk format.
//!
//! Directory `openai_to_anthropic/` = client speaks OpenAI, upstream speaks Anthropic.
//! Stream direction: upstream → client, so we translate Anthropic events → OpenAI chunks.

use serde_json::{Value, json};

fn chunk(id: &str, model: &str, delta: Value, finish_reason: Option<&'static str>, usage: Option<Value>) -> String {
    let finish = match finish_reason {
        Some(r) => Value::String(r.to_string()),
        None => Value::Null,
    };
    let mut obj = json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": 0u64,
        "model": model,
        "choices": [{
            "index": 0,
            "delta": delta,
            "finish_reason": finish
        }]
    });
    if let Some(u) = usage {
        obj["usage"] = u;
    }
    format!("data: {}\n\n", obj)
}

pub struct AnthropicToOpenAiStream {
    chunk_id: String,
    model: String,
    sent_role: bool,
    open_tool_idx: Option<usize>,
    next_tool_idx: usize,
    finish_reason: Option<&'static str>,
    pending_usage: Option<Value>,
    closed: bool,
}

impl Default for AnthropicToOpenAiStream {
    fn default() -> Self {
        Self::new()
    }
}

impl AnthropicToOpenAiStream {
    pub fn new() -> Self {
        Self {
            chunk_id: String::new(),
            model: String::new(),
            sent_role: false,
            open_tool_idx: None,
            next_tool_idx: 0,
            finish_reason: None,
            pending_usage: None,
            closed: false,
        }
    }

    /// Feed one upstream Anthropic event.
    /// `event_name` is the value after `event:`, `data` is parsed JSON from `data:`.
    /// Returns 0+ OpenAI chunk strings (each `data: {...}\n\n`), and possibly `data: [DONE]\n\n`.
    pub fn feed_event(&mut self, event_name: &str, data: &Value) -> Vec<String> {
        if self.closed {
            return Vec::new();
        }

        let mut out = Vec::new();

        match event_name {
            "message_start" => {
                // Capture id and model from the nested message object
                let message = data.get("message").unwrap_or(&Value::Null);
                self.chunk_id = message
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("msg_unknown")
                    .to_string();
                self.model = message
                    .get("model")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                // Emit the role chunk
                out.push(chunk(
                    &self.chunk_id,
                    &self.model,
                    json!({"role": "assistant"}),
                    None,
                    None,
                ));
                self.sent_role = true;
            }

            "content_block_start" => {
                let content_block = data.get("content_block").unwrap_or(&Value::Null);
                match content_block.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                    "text" => {
                        // No emit; text deltas follow
                    }
                    "tool_use" => {
                        let tool_idx = self.next_tool_idx;
                        self.next_tool_idx += 1;
                        self.open_tool_idx = Some(tool_idx);

                        let id = content_block
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let name = content_block
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();

                        let delta = json!({
                            "tool_calls": [{
                                "index": tool_idx,
                                "id": id,
                                "type": "function",
                                "function": {
                                    "name": name,
                                    "arguments": ""
                                }
                            }]
                        });
                        out.push(chunk(&self.chunk_id, &self.model, delta, None, None));
                    }
                    other => {
                        tracing::debug!(
                            target: "translation",
                            block_type = %other,
                            "unknown content block type in anthropic stream, skipping"
                        );
                    }
                }
            }

            "content_block_delta" => {
                let delta = data.get("delta").unwrap_or(&Value::Null);
                match delta.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                    "text_delta" => {
                        let text = delta.get("text").and_then(|t| t.as_str()).unwrap_or("");
                        let oai_delta = json!({"content": text});
                        out.push(chunk(&self.chunk_id, &self.model, oai_delta, None, None));
                    }
                    "input_json_delta" => {
                        let partial = delta
                            .get("partial_json")
                            .and_then(|p| p.as_str())
                            .unwrap_or("");
                        if let Some(tool_idx) = self.open_tool_idx {
                            let oai_delta = json!({
                                "tool_calls": [{
                                    "index": tool_idx,
                                    "function": {
                                        "arguments": partial
                                    }
                                }]
                            });
                            out.push(chunk(&self.chunk_id, &self.model, oai_delta, None, None));
                        } else {
                            tracing::warn!(
                                target: "translation",
                                "input_json_delta received but no open tool block"
                            );
                        }
                    }
                    other => {
                        tracing::debug!(
                            target: "translation",
                            delta_type = %other,
                            "unknown delta type in anthropic stream, skipping"
                        );
                    }
                }
            }

            "content_block_stop" => {
                // No emit; track that current tool is closed
                self.open_tool_idx = None;
            }

            "message_delta" => {
                // Capture stop_reason and usage for the final chunk
                let delta = data.get("delta").unwrap_or(&Value::Null);
                let stop_reason = delta
                    .get("stop_reason")
                    .and_then(|r| r.as_str())
                    .map(map_stop_reason);
                if let Some(reason) = stop_reason {
                    self.finish_reason = Some(reason);
                }

                // Capture usage
                let usage = data.get("usage");
                if let Some(u) = usage {
                    let output_tokens = u
                        .get("output_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let input_tokens = u
                        .get("input_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let cached = u
                        .get("cache_read_input_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    self.pending_usage = Some(json!({
                        "prompt_tokens": input_tokens,
                        "completion_tokens": output_tokens,
                        "total_tokens": input_tokens + output_tokens,
                        "prompt_tokens_details": {"cached_tokens": cached}
                    }));
                }
            }

            "message_stop" => {
                // Emit final chunk + [DONE]
                let fr = self.finish_reason.unwrap_or("stop");
                let usage = self.pending_usage.clone().unwrap_or(json!({
                    "prompt_tokens": 0,
                    "completion_tokens": 0,
                    "total_tokens": 0,
                    "prompt_tokens_details": {"cached_tokens": 0}
                }));
                out.push(chunk(
                    &self.chunk_id,
                    &self.model,
                    json!({}),
                    Some(fr),
                    Some(usage),
                ));
                out.push("data: [DONE]\n\n".to_string());
                self.closed = true;
            }

            other => {
                tracing::debug!(
                    target: "translation",
                    event_name = %other,
                    "unknown anthropic stream event, skipping"
                );
            }
        }

        out
    }
}

fn map_stop_reason(reason: &str) -> &'static str {
    match reason {
        "end_turn" => "stop",
        "tool_use" => "tool_calls",
        "max_tokens" => "length",
        "stop_sequence" => "stop",
        other => {
            tracing::debug!(
                target: "translation",
                stop_reason = %other,
                "unknown anthropic stop_reason; using stop"
            );
            "stop"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse an OpenAI SSE data line into Value.
    /// Returns None for `[DONE]` sentinel.
    fn parse_oai_chunk(s: &str) -> Option<Value> {
        let data = s.strip_prefix("data: ")?.trim_end_matches('\n');
        if data == "[DONE]" {
            return None;
        }
        serde_json::from_str(data).ok()
    }

    fn anth_event(event_name: &str, data: Value) -> (String, Value) {
        (event_name.to_string(), data)
    }

    fn collect(fsm: &mut AnthropicToOpenAiStream, events: &[(String, Value)]) -> Vec<Option<Value>> {
        let mut out = Vec::new();
        for (name, data) in events {
            for s in fsm.feed_event(name, data) {
                out.push(parse_oai_chunk(&s));
            }
        }
        out
    }

    #[test]
    fn text_only_response_emits_chunks_then_done() {
        let mut fsm = AnthropicToOpenAiStream::new();
        let events = vec![
            anth_event(
                "message_start",
                json!({
                    "type": "message_start",
                    "message": {
                        "id": "msg_abc", "role": "assistant", "model": "claude-opus-4",
                        "content": [], "stop_reason": null, "stop_sequence": null,
                        "usage": {"input_tokens": 10, "output_tokens": 0}
                    }
                }),
            ),
            anth_event(
                "content_block_start",
                json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
            ),
            anth_event(
                "content_block_delta",
                json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Hello "}}),
            ),
            anth_event(
                "content_block_delta",
                json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "world"}}),
            ),
            anth_event("content_block_stop", json!({"type": "content_block_stop", "index": 0})),
            anth_event(
                "message_delta",
                json!({"type": "message_delta", "delta": {"stop_reason": "end_turn", "stop_sequence": null}, "usage": {"output_tokens": 5}}),
            ),
            anth_event("message_stop", json!({"type": "message_stop"})),
        ];

        let chunks = collect(&mut fsm, &events);

        // Last item is [DONE] (None)
        assert!(chunks.last().unwrap().is_none(), "last item should be [DONE]");

        let data_chunks: Vec<&Value> = chunks.iter().filter_map(|c| c.as_ref()).collect();
        // role chunk, text delta x2, final chunk
        assert_eq!(data_chunks.len(), 4);

        // First chunk has role
        assert_eq!(data_chunks[0]["choices"][0]["delta"]["role"], "assistant");
        // Text deltas
        assert_eq!(data_chunks[1]["choices"][0]["delta"]["content"], "Hello ");
        assert_eq!(data_chunks[2]["choices"][0]["delta"]["content"], "world");
        // Final chunk has finish_reason
        assert_eq!(data_chunks[3]["choices"][0]["finish_reason"], "stop");
    }

    #[test]
    fn single_tool_call_streams_with_args_buffer() {
        let mut fsm = AnthropicToOpenAiStream::new();
        let events = vec![
            anth_event(
                "message_start",
                json!({"type": "message_start", "message": {"id": "msg_t1", "model": "claude-3", "content": [], "stop_reason": null, "stop_sequence": null, "usage": {"input_tokens": 5, "output_tokens": 0}}}),
            ),
            anth_event(
                "content_block_start",
                json!({"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "toolu_1", "name": "search", "input": {}}}),
            ),
            anth_event(
                "content_block_delta",
                json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "{\"q\":"}}),
            ),
            anth_event(
                "content_block_delta",
                json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "\"hello\"}"}}),
            ),
            anth_event("content_block_stop", json!({"type": "content_block_stop", "index": 0})),
            anth_event(
                "message_delta",
                json!({"type": "message_delta", "delta": {"stop_reason": "tool_use", "stop_sequence": null}, "usage": {"output_tokens": 15}}),
            ),
            anth_event("message_stop", json!({"type": "message_stop"})),
        ];

        let chunks = collect(&mut fsm, &events);
        let data_chunks: Vec<&Value> = chunks.iter().filter_map(|c| c.as_ref()).collect();

        // role chunk, tool_call_start chunk, 2 arg delta chunks, final chunk
        assert_eq!(data_chunks.len(), 5);

        // Tool call start chunk
        let tc_start = &data_chunks[1]["choices"][0]["delta"]["tool_calls"][0];
        assert_eq!(tc_start["id"], "toolu_1");
        assert_eq!(tc_start["function"]["name"], "search");
        assert_eq!(tc_start["function"]["arguments"], "");

        // Arg delta chunks
        assert_eq!(
            data_chunks[2]["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"],
            "{\"q\":"
        );
        assert_eq!(
            data_chunks[3]["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"],
            "\"hello\"}"
        );

        // Final chunk finish_reason
        assert_eq!(data_chunks[4]["choices"][0]["finish_reason"], "tool_calls");

        // DONE sentinel
        assert!(chunks.last().unwrap().is_none());
    }

    #[test]
    fn text_then_tool_call_in_same_message() {
        let mut fsm = AnthropicToOpenAiStream::new();
        let events = vec![
            anth_event(
                "message_start",
                json!({"type": "message_start", "message": {"id": "msg_m", "model": "claude-3", "content": [], "stop_reason": null, "stop_sequence": null, "usage": {"input_tokens": 8, "output_tokens": 0}}}),
            ),
            anth_event(
                "content_block_start",
                json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
            ),
            anth_event(
                "content_block_delta",
                json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Let me search."}}),
            ),
            anth_event("content_block_stop", json!({"type": "content_block_stop", "index": 0})),
            anth_event(
                "content_block_start",
                json!({"type": "content_block_start", "index": 1, "content_block": {"type": "tool_use", "id": "toolu_2", "name": "web_search", "input": {}}}),
            ),
            anth_event(
                "content_block_delta",
                json!({"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": "{\"query\":\"rust\"}"}}),
            ),
            anth_event("content_block_stop", json!({"type": "content_block_stop", "index": 1})),
            anth_event(
                "message_delta",
                json!({"type": "message_delta", "delta": {"stop_reason": "tool_use", "stop_sequence": null}, "usage": {"output_tokens": 20}}),
            ),
            anth_event("message_stop", json!({"type": "message_stop"})),
        ];

        let chunks = collect(&mut fsm, &events);
        let data_chunks: Vec<&Value> = chunks.iter().filter_map(|c| c.as_ref()).collect();

        // role + text_delta + tool_start + tool_arg_delta + final
        assert_eq!(data_chunks.len(), 5);
        assert_eq!(data_chunks[0]["choices"][0]["delta"]["role"], "assistant");
        assert_eq!(data_chunks[1]["choices"][0]["delta"]["content"], "Let me search.");
        assert_eq!(
            data_chunks[2]["choices"][0]["delta"]["tool_calls"][0]["function"]["name"],
            "web_search"
        );
        assert_eq!(data_chunks[4]["choices"][0]["finish_reason"], "tool_calls");
    }

    #[test]
    fn two_parallel_tool_calls() {
        let mut fsm = AnthropicToOpenAiStream::new();
        let events = vec![
            anth_event(
                "message_start",
                json!({"type": "message_start", "message": {"id": "msg_p", "model": "claude-3", "content": [], "stop_reason": null, "stop_sequence": null, "usage": {"input_tokens": 12, "output_tokens": 0}}}),
            ),
            anth_event(
                "content_block_start",
                json!({"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "tc_a", "name": "tool_a", "input": {}}}),
            ),
            anth_event(
                "content_block_delta",
                json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "{\"a\":1}"}}),
            ),
            anth_event("content_block_stop", json!({"type": "content_block_stop", "index": 0})),
            anth_event(
                "content_block_start",
                json!({"type": "content_block_start", "index": 1, "content_block": {"type": "tool_use", "id": "tc_b", "name": "tool_b", "input": {}}}),
            ),
            anth_event(
                "content_block_delta",
                json!({"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": "{\"b\":2}"}}),
            ),
            anth_event("content_block_stop", json!({"type": "content_block_stop", "index": 1})),
            anth_event(
                "message_delta",
                json!({"type": "message_delta", "delta": {"stop_reason": "tool_use", "stop_sequence": null}, "usage": {"output_tokens": 25}}),
            ),
            anth_event("message_stop", json!({"type": "message_stop"})),
        ];

        let chunks = collect(&mut fsm, &events);
        let data_chunks: Vec<&Value> = chunks.iter().filter_map(|c| c.as_ref()).collect();

        // role + (tool_start + tool_arg) × 2 + final = 6 chunks
        assert_eq!(data_chunks.len(), 6);

        let tool_start_a = &data_chunks[1]["choices"][0]["delta"]["tool_calls"][0];
        let tool_start_b = &data_chunks[3]["choices"][0]["delta"]["tool_calls"][0];

        assert_eq!(tool_start_a["id"], "tc_a");
        assert_eq!(tool_start_b["id"], "tc_b");
        assert_eq!(tool_start_a["index"], 0u64);
        assert_eq!(tool_start_b["index"], 1u64);
    }

    #[test]
    fn stop_reason_mappings() {
        let cases = [
            ("end_turn", "stop"),
            ("tool_use", "tool_calls"),
            ("max_tokens", "length"),
            ("stop_sequence", "stop"),
        ];
        for (anth_reason, oai_reason) in cases {
            let mut fsm = AnthropicToOpenAiStream::new();
            let events = vec![
                anth_event(
                    "message_start",
                    json!({"type": "message_start", "message": {"id": "msg_r", "model": "m", "content": [], "stop_reason": null, "stop_sequence": null, "usage": {"input_tokens": 1, "output_tokens": 0}}}),
                ),
                anth_event(
                    "message_delta",
                    json!({"type": "message_delta", "delta": {"stop_reason": anth_reason, "stop_sequence": null}, "usage": {"output_tokens": 1}}),
                ),
                anth_event("message_stop", json!({"type": "message_stop"})),
            ];
            let chunks = collect(&mut fsm, &events);
            let data_chunks: Vec<&Value> = chunks.iter().filter_map(|c| c.as_ref()).collect();
            let final_chunk = data_chunks.last().unwrap();
            assert_eq!(
                final_chunk["choices"][0]["finish_reason"],
                oai_reason,
                "stop_reason {anth_reason} should map to {oai_reason}"
            );
        }
    }

    #[test]
    fn empty_content_emits_role_then_finish_then_done() {
        let mut fsm = AnthropicToOpenAiStream::new();
        let events = vec![
            anth_event(
                "message_start",
                json!({"type": "message_start", "message": {"id": "msg_e", "model": "m", "content": [], "stop_reason": null, "stop_sequence": null, "usage": {"input_tokens": 0, "output_tokens": 0}}}),
            ),
            anth_event(
                "message_delta",
                json!({"type": "message_delta", "delta": {"stop_reason": "end_turn", "stop_sequence": null}, "usage": {"output_tokens": 0}}),
            ),
            anth_event("message_stop", json!({"type": "message_stop"})),
        ];

        let chunks = collect(&mut fsm, &events);
        // role chunk, final chunk, [DONE]
        assert_eq!(chunks.len(), 3);
        assert!(chunks[0].is_some()); // role
        assert!(chunks[1].is_some()); // final
        assert!(chunks[2].is_none()); // [DONE]

        let role_chunk = chunks[0].as_ref().unwrap();
        let final_chunk = chunks[1].as_ref().unwrap();
        assert_eq!(role_chunk["choices"][0]["delta"]["role"], "assistant");
        assert_eq!(final_chunk["choices"][0]["finish_reason"], "stop");
    }

    #[test]
    fn usage_in_message_delta_propagates_to_final_chunk() {
        let mut fsm = AnthropicToOpenAiStream::new();
        let events = vec![
            anth_event(
                "message_start",
                json!({"type": "message_start", "message": {"id": "msg_u", "model": "m", "content": [], "stop_reason": null, "stop_sequence": null, "usage": {"input_tokens": 100, "output_tokens": 0}}}),
            ),
            anth_event(
                "message_delta",
                json!({
                    "type": "message_delta",
                    "delta": {"stop_reason": "end_turn", "stop_sequence": null},
                    "usage": {
                        "output_tokens": 50,
                        "input_tokens": 100,
                        "cache_read_input_tokens": 30
                    }
                }),
            ),
            anth_event("message_stop", json!({"type": "message_stop"})),
        ];

        let chunks = collect(&mut fsm, &events);
        let data_chunks: Vec<&Value> = chunks.iter().filter_map(|c| c.as_ref()).collect();
        let final_chunk = data_chunks.last().unwrap();

        assert_eq!(final_chunk["usage"]["completion_tokens"], 50u64);
        assert_eq!(final_chunk["usage"]["prompt_tokens"], 100u64);
        assert_eq!(final_chunk["usage"]["total_tokens"], 150u64);
        assert_eq!(
            final_chunk["usage"]["prompt_tokens_details"]["cached_tokens"],
            30u64
        );
    }

    #[test]
    fn message_stop_emits_done_sentinel() {
        let mut fsm = AnthropicToOpenAiStream::new();
        let events = vec![
            anth_event(
                "message_start",
                json!({"type": "message_start", "message": {"id": "msg_d", "model": "m", "content": [], "stop_reason": null, "stop_sequence": null, "usage": {"input_tokens": 0, "output_tokens": 0}}}),
            ),
            anth_event(
                "message_delta",
                json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 1}}),
            ),
            anth_event("message_stop", json!({"type": "message_stop"})),
        ];

        // Collect raw strings to check for [DONE]
        let mut fsm2 = AnthropicToOpenAiStream::new();
        let mut raw: Vec<String> = Vec::new();
        for (name, data) in &events {
            raw.extend(fsm2.feed_event(name, data));
        }

        let _ = collect(&mut fsm, &events); // run fsm for side effects

        let last_raw = raw.last().expect("should have at least one event");
        assert_eq!(last_raw, "data: [DONE]\n\n");
    }
}
