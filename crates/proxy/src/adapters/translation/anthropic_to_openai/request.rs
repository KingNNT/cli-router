//! Translate Anthropic /v1/messages request body to OpenAI /v1/chat/completions.

use serde_json::{json, Value};

use crate::application::errors::ProxyError;

pub fn translate(body: &[u8]) -> Result<Vec<u8>, ProxyError> {
    let v: Value = serde_json::from_slice(body)
        .map_err(|e| ProxyError::BadRequest(format!("invalid anthropic request: {e}")))?;
    let translated = translate_value(&v)?;
    serde_json::to_vec(&translated)
        .map_err(|e| ProxyError::BadRequest(format!("translation serialize: {e}")))
}

fn translate_value(v: &Value) -> Result<Value, ProxyError> {
    let mut out = json!({});
    let m = out.as_object_mut().unwrap();

    // model — passthrough
    if let Some(model) = v.get("model") {
        m.insert("model".into(), model.clone());
    }

    // max_tokens — passthrough (required by Anthropic, optional by OpenAI)
    if let Some(mt) = v.get("max_tokens") {
        m.insert("max_tokens".into(), mt.clone());
    }

    // sampling — passthrough except top_k (drop) and stop_sequences (rename to stop)
    for k in ["temperature", "top_p", "stream"] {
        if let Some(val) = v.get(k) {
            m.insert(k.into(), val.clone());
        }
    }
    if let Some(stops) = v.get("stop_sequences") {
        m.insert("stop".into(), stops.clone());
    }
    // top_k: drop with no error.

    // metadata.user_id → user
    if let Some(user) = v.pointer("/metadata/user_id") {
        m.insert("user".into(), user.clone());
    }

    // tools — wrap each in function envelope
    if let Some(tools) = v.get("tools").and_then(|t| t.as_array()) {
        let translated_tools: Vec<Value> = tools.iter().map(translate_tool_def).collect();
        m.insert("tools".into(), Value::Array(translated_tools));
    }

    // tool_choice
    if let Some(tc) = v.get("tool_choice") {
        m.insert("tool_choice".into(), translate_tool_choice(tc));
    }

    // messages: build OpenAI messages from Anthropic system + messages
    let messages = build_openai_messages(v)?;
    m.insert("messages".into(), Value::Array(messages));

    Ok(out)
}

fn translate_tool_def(tool: &Value) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": tool.get("name").cloned().unwrap_or(Value::Null),
            "description": tool.get("description").cloned().unwrap_or(Value::Null),
            "parameters": tool.get("input_schema").cloned().unwrap_or(json!({})),
        }
    })
}

fn translate_tool_choice(tc: &Value) -> Value {
    match tc {
        Value::String(s) if s == "auto" => json!("auto"),
        Value::String(s) if s == "any" => json!("required"),
        Value::String(s) if s == "none" => json!("none"),
        Value::Object(map) if map.get("type").and_then(|t| t.as_str()) == Some("tool") => {
            json!({"type": "function", "function": {"name": map.get("name").cloned().unwrap_or(Value::Null)}})
        }
        other => other.clone(),
    }
}

fn build_openai_messages(req: &Value) -> Result<Vec<Value>, ProxyError> {
    let mut out = Vec::new();

    // System: top-level field becomes a message.
    if let Some(sys) = req.get("system") {
        let sys_text = flatten_text(sys);
        if !sys_text.is_empty() {
            out.push(json!({"role": "system", "content": sys_text}));
        }
    }

    if let Some(msgs) = req.get("messages").and_then(|m| m.as_array()) {
        for msg in msgs {
            let translated = translate_message(msg)?;
            for m in translated {
                out.push(m);
            }
        }
    }

    Ok(out)
}

/// One Anthropic message may produce multiple OpenAI messages
/// (e.g. tool_result blocks become separate role=tool messages).
fn translate_message(msg: &Value) -> Result<Vec<Value>, ProxyError> {
    let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("user");
    let content = msg.get("content").cloned().unwrap_or(Value::Null);

    match &content {
        Value::String(s) => Ok(vec![json!({"role": role, "content": s})]),
        Value::Array(blocks) => {
            let mut text_parts = Vec::<String>::new();
            let mut tool_calls = Vec::<Value>::new();
            let mut tool_messages = Vec::<Value>::new();

            for block in blocks {
                let btype = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match btype {
                    "text" => {
                        if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                            text_parts.push(t.to_string());
                        }
                    }
                    "tool_use" => {
                        let id = block.get("id").cloned().unwrap_or(Value::String("".into()));
                        let name = block.get("name").cloned().unwrap_or(Value::Null);
                        let input = block.get("input").cloned().unwrap_or(json!({}));
                        let args = serde_json::to_string(&input).unwrap_or_else(|_| "{}".into());
                        tool_calls.push(json!({
                            "id": id,
                            "type": "function",
                            "function": {"name": name, "arguments": args}
                        }));
                    }
                    "tool_result" => {
                        let tool_use_id =
                            block.get("tool_use_id").cloned().unwrap_or(Value::Null);
                        let tr_content =
                            block.get("content").cloned().unwrap_or(Value::Null);
                        let flat = flatten_text(&tr_content);
                        tool_messages.push(json!({
                            "role": "tool",
                            "tool_call_id": tool_use_id,
                            "content": flat,
                        }));
                    }
                    "image" => {
                        // out of scope: drop with warning
                        tracing::warn!(target: "translation", "image block dropped (out of scope)");
                    }
                    other => {
                        tracing::debug!(target: "translation", block_type = %other, "unknown block type, dropped");
                    }
                }
            }

            let mut result = Vec::new();
            if !text_parts.is_empty() || !tool_calls.is_empty() {
                let mut msg_out = json!({"role": role});
                let mo = msg_out.as_object_mut().unwrap();
                if !text_parts.is_empty() {
                    mo.insert("content".into(), Value::String(text_parts.join("\n")));
                } else {
                    mo.insert("content".into(), Value::Null);
                }
                if !tool_calls.is_empty() {
                    mo.insert("tool_calls".into(), Value::Array(tool_calls));
                }
                result.push(msg_out);
            }
            result.extend(tool_messages);
            Ok(result)
        }
        _ => Ok(vec![]), // unrecognized content type
    }
}

/// Flatten a JSON value to plain text. String → itself; array of {type:"text", text} → joined.
fn flatten_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(items) => {
            let mut parts = Vec::new();
            for item in items {
                if let Some(t) = item.get("text").and_then(|x| x.as_str()) {
                    parts.push(t.to_string());
                } else if let Some(s) = item.as_str() {
                    parts.push(s.to_string());
                }
            }
            parts.join("\n")
        }
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_message() {
        let body =
            br#"{"model":"x","max_tokens":10,"messages":[{"role":"user","content":"hi"}]}"#;
        let out: Value = serde_json::from_slice(&translate(body).unwrap()).unwrap();
        assert_eq!(out["model"], "x");
        assert_eq!(out["max_tokens"], 10);
        assert_eq!(out["messages"][0]["role"], "user");
        assert_eq!(out["messages"][0]["content"], "hi");
    }

    #[test]
    fn system_string_becomes_first_message() {
        let body = br#"{"model":"x","max_tokens":10,"system":"You are X","messages":[{"role":"user","content":"hi"}]}"#;
        let out: Value = serde_json::from_slice(&translate(body).unwrap()).unwrap();
        assert_eq!(out["messages"][0]["role"], "system");
        assert_eq!(out["messages"][0]["content"], "You are X");
        assert_eq!(out["messages"][1]["role"], "user");
    }

    #[test]
    fn system_array_blocks_join() {
        let body = br#"{"model":"x","max_tokens":10,"system":[{"type":"text","text":"a"},{"type":"text","text":"b"}],"messages":[]}"#;
        let out: Value = serde_json::from_slice(&translate(body).unwrap()).unwrap();
        assert_eq!(out["messages"][0]["content"], "a\nb");
    }

    #[test]
    fn tool_use_becomes_tool_calls() {
        let body = br#"{"model":"x","max_tokens":10,"messages":[
            {"role":"assistant","content":[
                {"type":"text","text":"calling tool"},
                {"type":"tool_use","id":"t1","name":"search","input":{"q":"hello"}}
            ]}
        ]}"#;
        let out: Value = serde_json::from_slice(&translate(body).unwrap()).unwrap();
        assert_eq!(out["messages"][0]["role"], "assistant");
        assert_eq!(out["messages"][0]["content"], "calling tool");
        assert_eq!(out["messages"][0]["tool_calls"][0]["id"], "t1");
        assert_eq!(out["messages"][0]["tool_calls"][0]["type"], "function");
        assert_eq!(out["messages"][0]["tool_calls"][0]["function"]["name"], "search");
        assert!(out["messages"][0]["tool_calls"][0]["function"]["arguments"]
            .as_str()
            .unwrap()
            .contains("hello"));
    }

    #[test]
    fn tool_result_becomes_role_tool_message() {
        let body = br#"{"model":"x","max_tokens":10,"messages":[
            {"role":"user","content":[
                {"type":"tool_result","tool_use_id":"t1","content":"42"}
            ]}
        ]}"#;
        let out: Value = serde_json::from_slice(&translate(body).unwrap()).unwrap();
        assert_eq!(out["messages"][0]["role"], "tool");
        assert_eq!(out["messages"][0]["tool_call_id"], "t1");
        assert_eq!(out["messages"][0]["content"], "42");
    }

    #[test]
    fn tools_wrap_in_function_envelope() {
        let body = br#"{"model":"x","max_tokens":10,"messages":[],"tools":[
            {"name":"search","description":"web search","input_schema":{"type":"object"}}
        ]}"#;
        let out: Value = serde_json::from_slice(&translate(body).unwrap()).unwrap();
        assert_eq!(out["tools"][0]["type"], "function");
        assert_eq!(out["tools"][0]["function"]["name"], "search");
        assert_eq!(out["tools"][0]["function"]["parameters"]["type"], "object");
    }

    #[test]
    fn tool_choice_any_becomes_required() {
        let body = br#"{"model":"x","max_tokens":10,"messages":[],"tool_choice":"any"}"#;
        let out: Value = serde_json::from_slice(&translate(body).unwrap()).unwrap();
        assert_eq!(out["tool_choice"], "required");
    }

    #[test]
    fn tool_choice_named_rewraps() {
        let body = br#"{"model":"x","max_tokens":10,"messages":[],"tool_choice":{"type":"tool","name":"search"}}"#;
        let out: Value = serde_json::from_slice(&translate(body).unwrap()).unwrap();
        assert_eq!(out["tool_choice"]["type"], "function");
        assert_eq!(out["tool_choice"]["function"]["name"], "search");
    }

    #[test]
    fn stop_sequences_rename_to_stop() {
        let body =
            br#"{"model":"x","max_tokens":10,"messages":[],"stop_sequences":["END"]}"#;
        let out: Value = serde_json::from_slice(&translate(body).unwrap()).unwrap();
        assert_eq!(out["stop"][0], "END");
        assert!(out.get("stop_sequences").is_none());
    }

    #[test]
    fn top_k_dropped() {
        let body = br#"{"model":"x","max_tokens":10,"messages":[],"top_k":5}"#;
        let out: Value = serde_json::from_slice(&translate(body).unwrap()).unwrap();
        assert!(out.get("top_k").is_none());
    }

    #[test]
    fn cache_control_markers_dropped_from_text_blocks() {
        let body = br#"{"model":"x","max_tokens":10,"messages":[
            {"role":"user","content":[{"type":"text","text":"hi","cache_control":{"type":"ephemeral"}}]}
        ]}"#;
        let out: Value = serde_json::from_slice(&translate(body).unwrap()).unwrap();
        // cache_control is not in the output — text block contents are flattened to a plain string
        assert_eq!(out["messages"][0]["content"], "hi");
    }

    #[test]
    fn metadata_user_id_becomes_user() {
        let body =
            br#"{"model":"x","max_tokens":10,"messages":[],"metadata":{"user_id":"u42"}}"#;
        let out: Value = serde_json::from_slice(&translate(body).unwrap()).unwrap();
        assert_eq!(out["user"], "u42");
        assert!(out.get("metadata").is_none());
    }
}
