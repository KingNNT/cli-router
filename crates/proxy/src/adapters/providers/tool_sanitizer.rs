//! Provider-agnostic sanitizer for Anthropic responses: drops no-op `bash`
//! tool calls (empty / `:` / `true` command) and forces `stop_reason=end_turn`
//! when no real tool_use remains. Fails open — any parse error returns input
//! unchanged. Currently used only by the Kimi provider on the Anthropic path.

use crate::application::ports::BoxedByteStream;
use bytes::Bytes;
use futures::StreamExt;
use serde_json::{Value, json};

/// True iff `block` is a `tool_use` whose `input.command` is a string that,
/// trimmed, is empty or a shell no-op (`:` / `true`).
pub fn is_noop_tool_use(block: &Value) -> bool {
    if block.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
        return false;
    }
    match block.pointer("/input/command").and_then(|c| c.as_str()) {
        Some(cmd) => matches!(cmd.trim(), "" | ":" | "true"),
        None => false,
    }
}

/// Drop no-op tool_use blocks from an Anthropic message body. If no `tool_use`
/// block survives, force `stop_reason=end_turn`. Fail open: any parse failure
/// or unexpected shape returns the original bytes unchanged.
pub fn sanitize_buffered(body: &[u8]) -> Bytes {
    let Ok(mut value) = serde_json::from_slice::<Value>(body) else {
        return Bytes::copy_from_slice(body);
    };
    let Some(content) = value.get("content").and_then(|c| c.as_array()) else {
        return Bytes::copy_from_slice(body);
    };
    let kept: Vec<Value> = content
        .iter()
        .filter(|b| !is_noop_tool_use(b))
        .cloned()
        .collect();
    let has_tool_use = kept
        .iter()
        .any(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_use"));
    value["content"] = Value::Array(kept);
    if !has_tool_use && value.get("stop_reason").and_then(|s| s.as_str()) == Some("tool_use") {
        value["stop_reason"] = json!("end_turn");
    }
    serde_json::to_vec(&value)
        .map(Bytes::from)
        .unwrap_or_else(|_| Bytes::copy_from_slice(body))
}

fn sse(event: &str, data: &Value) -> String {
    format!("event: {event}\ndata: {}\n\n", data)
}

struct PendingTool {
    frames: Vec<String>, // raw start + delta frames, in order
    args: String,        // accumulated partial_json
}

/// Streaming SSE state machine that swallows no-op `bash` tool_use blocks
/// from an Anthropic response stream and rewrites `stop_reason` to
/// `end_turn` when no real tool call survives. Mirrors the buffered-path
/// logic in [`is_noop_tool_use`] / [`sanitize_buffered`] for the streaming
/// case, where a tool_use block's `input` arrives incrementally across
/// `input_json_delta` frames and can't be evaluated until `content_block_stop`.
pub struct AnthropicNoopFilter {
    pending_tool: Option<PendingTool>,
    emitted_real_tool: bool,
    pending_message_delta: Option<Value>,
    closed: bool,
}

impl Default for AnthropicNoopFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl AnthropicNoopFilter {
    pub fn new() -> Self {
        Self {
            pending_tool: None,
            emitted_real_tool: false,
            pending_message_delta: None,
            closed: false,
        }
    }

    /// Flush a buffered tool block unchanged (used on the valid path or when
    /// failing open). Returns its frames and marks a real tool as emitted.
    fn flush_pending_tool(&mut self) -> Vec<String> {
        match self.pending_tool.take() {
            Some(p) => {
                self.emitted_real_tool = true;
                p.frames
            }
            None => Vec::new(),
        }
    }

    pub fn feed(&mut self, event: &str, data: &Value) -> Vec<String> {
        if self.closed {
            return Vec::new();
        }
        match event {
            "content_block_start" => {
                let is_tool = data.pointer("/content_block/type").and_then(|t| t.as_str())
                    == Some("tool_use");
                if is_tool {
                    // Start buffering a fresh tool block. If one was already
                    // pending (missing stop — shouldn't happen), flush it first
                    // so a valid tool is never lost.
                    let flushed = self.flush_pending_tool();
                    self.pending_tool = Some(PendingTool {
                        frames: vec![sse(event, data)],
                        args: String::new(),
                    });
                    flushed
                } else {
                    vec![sse(event, data)]
                }
            }
            "content_block_delta" => {
                let is_json_delta = data.pointer("/delta/type").and_then(|t| t.as_str())
                    == Some("input_json_delta");
                if let Some(p) = self.pending_tool.as_mut()
                    && is_json_delta
                {
                    if let Some(frag) = data.pointer("/delta/partial_json").and_then(|v| v.as_str())
                    {
                        p.args.push_str(frag);
                    }
                    p.frames.push(sse(event, data));
                    Vec::new()
                } else {
                    vec![sse(event, data)]
                }
            }
            "content_block_stop" => {
                if let Some(mut p) = self.pending_tool.take() {
                    p.frames.push(sse(event, data));
                    // Decide: parse accumulated args; no-op → swallow, else flush.
                    let block = serde_json::from_str::<Value>(&p.args)
                        .map(|input| json!({"type": "tool_use", "input": input}))
                        .ok();
                    let noop = block.as_ref().map(is_noop_tool_use).unwrap_or(false);
                    if noop {
                        Vec::new() // swallow entire block
                    } else {
                        self.emitted_real_tool = true;
                        p.frames
                    }
                } else {
                    vec![sse(event, data)]
                }
            }
            "message_delta" => {
                // Buffer; emit at message_stop (may rewrite stop_reason).
                self.pending_message_delta = Some(data.clone());
                Vec::new()
            }
            "message_stop" => {
                let mut out = Vec::new();
                if let Some(mut md) = self.pending_message_delta.take() {
                    if !self.emitted_real_tool
                        && md.pointer("/delta/stop_reason").and_then(|s| s.as_str())
                            == Some("tool_use")
                    {
                        md["delta"]["stop_reason"] = json!("end_turn");
                    }
                    out.push(sse("message_delta", &md));
                }
                out.push(sse(event, data));
                self.closed = true;
                out
            }
            _ => vec![sse(event, data)],
        }
    }
}

/// Wrap an upstream Anthropic SSE byte stream, filtering no-op tool calls.
/// Splits on blank-line frame boundaries, feeds each complete `event:/data:`
/// frame through `AnthropicNoopFilter`, and re-emits the filtered frames.
pub fn sanitize_stream(upstream: BoxedByteStream) -> BoxedByteStream {
    let mut filter = AnthropicNoopFilter::new();
    let mut buf = String::new();

    let filtered = upstream.flat_map(move |chunk_result| {
        let mut emit: Vec<Result<Bytes, Box<dyn std::error::Error + Send + Sync>>> = Vec::new();
        match chunk_result {
            Err(e) => emit.push(Err(e)),
            Ok(chunk_bytes) => {
                buf.push_str(&String::from_utf8_lossy(&chunk_bytes));
                while let Some(idx) = buf.find("\n\n") {
                    let frame = buf[..idx].to_string();
                    buf.drain(..idx + 2);
                    if frame.trim().is_empty() {
                        continue;
                    }
                    let mut event = String::new();
                    let mut data_str = String::new();
                    for line in frame.lines() {
                        if let Some(n) = line.strip_prefix("event: ") {
                            event = n.to_string();
                        } else if let Some(d) = line.strip_prefix("data: ") {
                            data_str = d.to_string();
                        }
                    }
                    match serde_json::from_str::<Value>(&data_str) {
                        Ok(data) => {
                            for out_frame in filter.feed(&event, &data) {
                                emit.push(Ok(Bytes::from(out_frame)));
                            }
                        }
                        // Fail open: forward the frame verbatim if data isn't JSON.
                        Err(_) => emit.push(Ok(Bytes::from(format!("{frame}\n\n")))),
                    }
                }
            }
        }
        futures::stream::iter(emit)
    });

    Box::pin(filtered)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_use(cmd: Value) -> Value {
        json!({"type": "tool_use", "id": "t1", "name": "bash", "input": {"command": cmd}})
    }

    #[test]
    fn noop_detects_empty_whitespace_and_shell_noops() {
        assert!(is_noop_tool_use(&tool_use(json!(""))));
        assert!(is_noop_tool_use(&tool_use(json!("   "))));
        assert!(is_noop_tool_use(&tool_use(json!(":"))));
        assert!(is_noop_tool_use(&tool_use(json!("true"))));
        assert!(is_noop_tool_use(&tool_use(json!(" : "))));
    }

    #[test]
    fn noop_rejects_real_commands_and_non_tool_blocks() {
        assert!(!is_noop_tool_use(&tool_use(json!("ls -la"))));
        assert!(!is_noop_tool_use(&json!({"type": "text", "text": "hi"})));
        assert!(!is_noop_tool_use(
            &json!({"type": "tool_use", "id": "t", "name": "bash", "input": {}})
        ));
        assert!(!is_noop_tool_use(
            &json!({"type": "tool_use", "name": "x", "input": {"command": 5}})
        ));
    }

    #[test]
    fn buffered_drops_only_noop_tool_keeps_stop_reason() {
        let body = json!({
            "type": "message", "role": "assistant", "stop_reason": "tool_use",
            "content": [
                {"type": "tool_use", "id": "a", "name": "bash", "input": {"command": "ls"}},
                {"type": "tool_use", "id": "b", "name": "bash", "input": {"command": ":"}}
            ]
        });
        let out: Value =
            serde_json::from_slice(&sanitize_buffered(body.to_string().as_bytes())).unwrap();
        assert_eq!(out["content"].as_array().unwrap().len(), 1);
        assert_eq!(out["content"][0]["id"], "a");
        assert_eq!(out["stop_reason"], "tool_use"); // a real tool remains
    }

    #[test]
    fn buffered_drops_sole_noop_tool_and_forces_end_turn() {
        let body = json!({
            "type": "message", "role": "assistant", "stop_reason": "tool_use",
            "content": [
                {"type": "text", "text": "All done."},
                {"type": "tool_use", "id": "b", "name": "bash", "input": {"command": ""}}
            ]
        });
        let out: Value =
            serde_json::from_slice(&sanitize_buffered(body.to_string().as_bytes())).unwrap();
        let content = out["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(out["stop_reason"], "end_turn");
    }

    #[test]
    fn buffered_leaves_valid_tool_untouched() {
        let body = json!({
            "type": "message", "role": "assistant", "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": "a", "name": "bash", "input": {"command": "pwd"}}]
        });
        let out: Value =
            serde_json::from_slice(&sanitize_buffered(body.to_string().as_bytes())).unwrap();
        assert_eq!(out["content"].as_array().unwrap().len(), 1);
        assert_eq!(out["stop_reason"], "tool_use");
    }

    #[test]
    fn buffered_returns_malformed_input_unchanged() {
        let raw = b"not json at all";
        assert_eq!(sanitize_buffered(raw).as_ref(), raw);
    }

    #[test]
    fn buffered_returns_valid_json_without_content_field_unchanged() {
        let raw = br#"{"stop_reason":"tool_use"}"#;
        assert_eq!(sanitize_buffered(raw).as_ref(), raw.as_slice());
    }

    fn frame(event: &str, data: &Value) -> (String, Value) {
        (event.to_string(), data.clone())
    }

    fn run(events: &[(String, Value)]) -> Vec<(String, Value)> {
        let mut f = AnthropicNoopFilter::new();
        let mut out = Vec::new();
        for (name, data) in events {
            for s in f.feed(name, data) {
                // parse "event: X\ndata: Y\n\n"
                let mut ev = String::new();
                let mut dt = String::new();
                for line in s.lines() {
                    if let Some(n) = line.strip_prefix("event: ") {
                        ev = n.into();
                    } else if let Some(d) = line.strip_prefix("data: ") {
                        dt = d.into();
                    }
                }
                out.push((ev, serde_json::from_str(&dt).unwrap()));
            }
        }
        out
    }

    fn tool_stream(cmd_json: &str) -> Vec<(String, Value)> {
        vec![
            frame(
                "message_start",
                &json!({"type":"message_start","message":{"id":"m","model":"kimi","content":[],"stop_reason":null,"usage":{"input_tokens":1,"output_tokens":0}}}),
            ),
            frame(
                "content_block_start",
                &json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t1","name":"bash","input":{}}}),
            ),
            frame(
                "content_block_delta",
                &json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":cmd_json}}),
            ),
            frame(
                "content_block_stop",
                &json!({"type":"content_block_stop","index":0}),
            ),
            frame(
                "message_delta",
                &json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":3}}),
            ),
            frame("message_stop", &json!({"type":"message_stop"})),
        ]
    }

    #[test]
    fn stream_swallows_noop_tool_and_rewrites_stop_reason() {
        let out = run(&tool_stream("{\"command\":\":\"}"));
        let names: Vec<&str> = out.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, &["message_start", "message_delta", "message_stop"]);
        let md = out.iter().find(|(n, _)| n == "message_delta").unwrap();
        assert_eq!(md.1["delta"]["stop_reason"], "end_turn");
        assert_eq!(md.1["usage"]["output_tokens"], 3); // usage preserved
    }

    #[test]
    fn stream_keeps_valid_tool_and_stop_reason() {
        let out = run(&tool_stream("{\"command\":\"ls\"}"));
        let names: Vec<&str> = out.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            &[
                "message_start",
                "content_block_start",
                "content_block_delta",
                "content_block_stop",
                "message_delta",
                "message_stop"
            ]
        );
        let md = out.iter().find(|(n, _)| n == "message_delta").unwrap();
        assert_eq!(md.1["delta"]["stop_reason"], "tool_use");
    }

    #[test]
    fn stream_command_split_across_deltas_is_evaluated_whole() {
        let mut events = vec![
            frame(
                "message_start",
                &json!({"type":"message_start","message":{"id":"m","model":"k","content":[]}}),
            ),
            frame(
                "content_block_start",
                &json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t1","name":"bash","input":{}}}),
            ),
            frame(
                "content_block_delta",
                &json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"comm"}}),
            ),
            frame(
                "content_block_delta",
                &json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"and\":\"\"}"}}),
            ),
            frame(
                "content_block_stop",
                &json!({"type":"content_block_stop","index":0}),
            ),
            frame(
                "message_delta",
                &json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":2}}),
            ),
            frame("message_stop", &json!({"type":"message_stop"})),
        ];
        let out = run(&std::mem::take(&mut events));
        let names: Vec<&str> = out.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, &["message_start", "message_delta", "message_stop"]);
    }

    #[test]
    fn stream_keeps_text_and_drops_noop_tool() {
        let events = vec![
            frame(
                "message_start",
                &json!({"type":"message_start","message":{"id":"m","model":"k","content":[]}}),
            ),
            frame(
                "content_block_start",
                &json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            ),
            frame(
                "content_block_delta",
                &json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Done."}}),
            ),
            frame(
                "content_block_stop",
                &json!({"type":"content_block_stop","index":0}),
            ),
            frame(
                "content_block_start",
                &json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"t1","name":"bash","input":{}}}),
            ),
            frame(
                "content_block_delta",
                &json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"command\":\"true\"}"}}),
            ),
            frame(
                "content_block_stop",
                &json!({"type":"content_block_stop","index":1}),
            ),
            frame(
                "message_delta",
                &json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":4}}),
            ),
            frame("message_stop", &json!({"type":"message_stop"})),
        ];
        let out = run(&events);
        let names: Vec<&str> = out.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            &[
                "message_start",
                "content_block_start",
                "content_block_delta",
                "content_block_stop",
                "message_delta",
                "message_stop"
            ]
        );
        // the surviving content_block_start is the text block
        let cbs = out
            .iter()
            .find(|(n, _)| n == "content_block_start")
            .unwrap();
        assert_eq!(cbs.1["content_block"]["type"], "text");
        assert_eq!(
            out.iter().find(|(n, _)| n == "message_delta").unwrap().1["delta"]["stop_reason"],
            "end_turn"
        );
    }
}
