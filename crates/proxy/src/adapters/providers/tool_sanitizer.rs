//! Provider-agnostic sanitizer for Anthropic responses: drops no-op `bash`
//! tool calls (empty / `:` / `true` command) and forces `stop_reason=end_turn`
//! when no real tool_use remains. Fails open — any parse error returns input
//! unchanged. Currently used only by the Kimi provider on the Anthropic path.

use bytes::Bytes;
use serde_json::{Value, json};

/// True iff `block` is a `tool_use` whose `input.command` is a string that,
/// trimmed, is empty or a shell no-op (`:` / `true`).
// Wired into KimiProvider in a later task; unused until then.
#[allow(dead_code)]
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
// Wired into KimiProvider in a later task; unused until then.
#[allow(dead_code)]
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
}
