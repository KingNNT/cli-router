//! Translate OpenAI /v1/chat/completions request body to Anthropic /v1/messages.

use serde_json::{Value, json};

use crate::application::errors::ProxyError;

pub fn translate(body: &[u8]) -> Result<Vec<u8>, ProxyError> {
    let v: Value =
        serde_json::from_slice(body).map_err(|e| ProxyError::TranslationInvalidRequest {
            field: "body",
            reason: format!("parse failed: {e}"),
        })?;
    let translated = translate_value(&v)?;
    serde_json::to_vec(&translated).map_err(|e| ProxyError::TranslationInvalidRequest {
        field: "body",
        reason: format!("serialize failed: {e}"),
    })
}

fn translate_value(v: &Value) -> Result<Value, ProxyError> {
    let mut out = json!({});
    let m = out.as_object_mut().unwrap();

    // model — passthrough
    if let Some(model) = v.get("model") {
        m.insert("model".into(), model.clone());
    }

    // max_tokens: required by Anthropic; default to 4096 if absent (OpenAI treats it as optional)
    let max_tokens = v.get("max_tokens").cloned().unwrap_or(json!(4096));
    m.insert("max_tokens".into(), max_tokens);

    // sampling — passthrough
    for k in ["temperature", "top_p", "stream"] {
        if let Some(val) = v.get(k) {
            m.insert(k.into(), val.clone());
        }
    }

    // stop → stop_sequences
    if let Some(stop) = v.get("stop") {
        m.insert("stop_sequences".into(), stop.clone());
    }

    // user → metadata.user_id
    if let Some(user) = v.get("user") {
        m.insert("metadata".into(), json!({"user_id": user}));
    }

    // tools — unwrap function envelope
    if let Some(tools) = v.get("tools").and_then(|t| t.as_array()) {
        let translated_tools: Vec<Value> = tools.iter().map(translate_tool_def).collect();
        m.insert("tools".into(), Value::Array(translated_tools));
    }

    // tool_choice — reverse mapping
    if let Some(tc) = v.get("tool_choice") {
        m.insert("tool_choice".into(), translate_tool_choice(tc));
    }

    // messages: extract system + build Anthropic messages
    let (system, messages) = build_anthropic_messages(v)?;
    if let Some(sys) = system {
        m.insert("system".into(), Value::String(sys));
    }
    m.insert("messages".into(), Value::Array(messages));

    Ok(out)
}

fn translate_tool_def(tool: &Value) -> Value {
    let func = tool.get("function").cloned().unwrap_or(json!({}));
    json!({
        "name": func.get("name").cloned().unwrap_or(Value::Null),
        "description": func.get("description").cloned().unwrap_or(Value::Null),
        "input_schema": func.get("parameters").cloned().unwrap_or(json!({})),
    })
}

/// Anthropic accepts `tool_choice` only as an object (`{"type": "auto"}`,
/// `{"type": "any"}`, `{"type": "none"}`, `{"type": "tool", "name": …}`).
/// OpenAI spells the first three as bare strings, so every one of them has to
/// be wrapped — forwarding the string reaches the upstream as
/// `"tool_choice": "auto"` and is rejected outright.
fn translate_tool_choice(tc: &Value) -> Value {
    match tc {
        Value::String(s) if s == "auto" => json!({"type": "auto"}),
        Value::String(s) if s == "required" => json!({"type": "any"}),
        Value::String(s) if s == "none" => json!({"type": "none"}),
        Value::Object(map) if map.get("type").and_then(|t| t.as_str()) == Some("function") => {
            let name = map
                .get("function")
                .and_then(|f| f.get("name"))
                .cloned()
                .unwrap_or(Value::Null);
            json!({"type": "tool", "name": name})
        }
        other => other.clone(),
    }
}

/// Parse the OpenAI messages array.
///
/// Rules:
/// - The first message with role=system is extracted to the top-level `system` field.
/// - Subsequent system messages are dropped with a debug-log warning. This is a deliberate
///   simplification: Anthropic only has a single top-level system field; merging later system
///   messages into user turns would be surprising and lossy. Debug-logging ensures the drop
///   is visible in traces.
/// - role=tool messages become a standalone user message containing a single tool_result block.
///   Each tool result stays in its own user turn so the mapping is 1:1 and unambiguous.
fn build_anthropic_messages(req: &Value) -> Result<(Option<String>, Vec<Value>), ProxyError> {
    let mut system: Option<String> = None;
    let mut messages: Vec<Value> = Vec::new();

    let msgs = match req.get("messages").and_then(|m| m.as_array()) {
        Some(a) => a,
        None => return Ok((None, messages)),
    };

    for msg in msgs {
        let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");

        match role {
            "system" => {
                let text = extract_text_content(msg);
                if system.is_none() {
                    system = Some(text);
                } else {
                    // subsequent system messages: drop with warning
                    tracing::debug!(
                        target: "translation",
                        "subsequent system message dropped (Anthropic only supports one system field)"
                    );
                }
            }
            "user" => {
                messages.push(translate_user_message(msg));
            }
            "assistant" => {
                messages.push(translate_assistant_message(msg)?);
            }
            "tool" => {
                // Each role=tool message becomes a standalone Anthropic user message with a
                // single tool_result block. Using separate user messages (rather than merging
                // consecutive tool results into one) keeps the translation 1:1 and predictable.
                messages.push(translate_tool_message(msg));
            }
            other => {
                tracing::debug!(
                    target: "translation",
                    role = %other,
                    "unknown message role, dropped"
                );
            }
        }
    }

    Ok((system, messages))
}

/// Extract the text content from a message for use in the system field.
fn extract_text_content(msg: &Value) -> String {
    match msg.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => {
            let mut out = Vec::new();
            for part in parts {
                if let Some("text") = part.get("type").and_then(|t| t.as_str())
                    && let Some(t) = part.get("text").and_then(|v| v.as_str())
                {
                    out.push(t.to_string());
                }
            }
            out.join("\n")
        }
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

/// Translate a user-role OpenAI message to Anthropic format.
///
/// Content may be a string, an array of {type:"text"|"image_url"} parts, or null.
/// image_url parts are dropped with a warning (multimodal out of scope).
fn translate_user_message(msg: &Value) -> Value {
    match msg.get("content") {
        Some(Value::String(s)) => json!({"role": "user", "content": s}),
        Some(Value::Array(parts)) => {
            let mut blocks: Vec<Value> = Vec::new();
            for part in parts {
                match part.get("type").and_then(|t| t.as_str()) {
                    Some("text") => {
                        let text = part
                            .get("text")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        blocks.push(json!({"type": "text", "text": text}));
                    }
                    Some("image_url") => {
                        tracing::warn!(
                            target: "translation",
                            "image_url content part dropped (multimodal out of scope)"
                        );
                    }
                    Some(other) => {
                        tracing::debug!(
                            target: "translation",
                            part_type = %other,
                            "unknown content part type, dropped"
                        );
                    }
                    None => {}
                }
            }
            if blocks.len() == 1
                && let Some(text) = blocks[0].get("text").and_then(|v| v.as_str())
            {
                return json!({"role": "user", "content": text});
            }
            json!({"role": "user", "content": blocks})
        }
        _ => json!({"role": "user", "content": ""}),
    }
}

/// Translate an assistant-role OpenAI message to Anthropic format.
///
/// tool_calls are translated to tool_use blocks in the content array.
/// Plain text content and tool_calls can coexist in the same message.
fn translate_assistant_message(msg: &Value) -> Result<Value, ProxyError> {
    let mut content: Vec<Value> = Vec::new();

    // Text content
    match msg.get("content") {
        Some(Value::String(s)) if !s.is_empty() => {
            content.push(json!({"type": "text", "text": s}));
        }
        Some(Value::Array(parts)) => {
            for part in parts {
                if let Some("text") = part.get("type").and_then(|t| t.as_str()) {
                    let text = part
                        .get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if !text.is_empty() {
                        content.push(json!({"type": "text", "text": text}));
                    }
                }
            }
        }
        _ => {}
    }

    // tool_calls → tool_use blocks
    if let Some(tool_calls) = msg.get("tool_calls").and_then(|tc| tc.as_array()) {
        for tc in tool_calls {
            let id = tc.get("id").cloned().unwrap_or(Value::Null);
            let func = tc.get("function").cloned().unwrap_or(json!({}));
            let name = func.get("name").cloned().unwrap_or(Value::Null);
            let arguments = func
                .get("arguments")
                .and_then(|a| a.as_str())
                .unwrap_or("{}");
            let input: Value = serde_json::from_str(arguments).map_err(|e| {
                ProxyError::TranslationInvalidRequest {
                    field: "tool_call.arguments",
                    reason: format!("not valid JSON: {e}"),
                }
            })?;
            content.push(json!({"type": "tool_use", "id": id, "name": name, "input": input}));
        }
    }

    // If content is a single text block, simplify to string form
    if content.len() == 1
        && let Some(text) = content[0].get("text").and_then(|v| v.as_str())
    {
        return Ok(json!({"role": "assistant", "content": text}));
    }

    Ok(json!({"role": "assistant", "content": content}))
}

/// Translate a role=tool OpenAI message to an Anthropic user message with a tool_result block.
fn translate_tool_message(msg: &Value) -> Value {
    let tool_call_id = msg.get("tool_call_id").cloned().unwrap_or(Value::Null);
    let content_str = msg
        .get("content")
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();
    json!({
        "role": "user",
        "content": [{
            "type": "tool_result",
            "tool_use_id": tool_call_id,
            "content": content_str,
        }]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_message_round_trips() {
        let body = br#"{"model":"x","messages":[{"role":"user","content":"hi"}]}"#;
        let out: Value = serde_json::from_slice(&translate(body).unwrap()).unwrap();
        assert_eq!(out["model"], "x");
        assert_eq!(out["messages"][0]["role"], "user");
        assert_eq!(out["messages"][0]["content"], "hi");
    }

    #[test]
    fn first_system_message_extracts_to_top_level() {
        let body = br#"{"model":"x","messages":[
            {"role":"system","content":"Be helpful."},
            {"role":"user","content":"Hello"}
        ]}"#;
        let out: Value = serde_json::from_slice(&translate(body).unwrap()).unwrap();
        assert_eq!(out["system"], "Be helpful.");
        assert_eq!(out["messages"].as_array().unwrap().len(), 1);
        assert_eq!(out["messages"][0]["role"], "user");
    }

    #[test]
    fn multiple_system_messages_keeps_first_drops_rest() {
        // Second system message is silently dropped (debug-logged).
        // The Anthropic API only supports a single top-level system field.
        let body = br#"{"model":"x","messages":[
            {"role":"system","content":"First system."},
            {"role":"system","content":"Second system (dropped)."},
            {"role":"user","content":"Hello"}
        ]}"#;
        let out: Value = serde_json::from_slice(&translate(body).unwrap()).unwrap();
        assert_eq!(out["system"], "First system.");
        // Only the user message survives in messages[]
        assert_eq!(out["messages"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn tool_calls_become_tool_use_blocks() {
        let body = br#"{"model":"x","messages":[
            {"role":"assistant","content":null,"tool_calls":[
                {"id":"tc1","type":"function","function":{"name":"search","arguments":"{\"q\":\"hello\"}"}}
            ]}
        ]}"#;
        let out: Value = serde_json::from_slice(&translate(body).unwrap()).unwrap();
        let content = &out["messages"][0]["content"];
        assert!(content.is_array());
        let block = &content[0];
        assert_eq!(block["type"], "tool_use");
        assert_eq!(block["id"], "tc1");
        assert_eq!(block["name"], "search");
        assert_eq!(block["input"]["q"], "hello");
    }

    #[test]
    fn role_tool_becomes_tool_result_block() {
        // role=tool messages become standalone user messages with a tool_result block
        let body = br#"{"model":"x","messages":[
            {"role":"tool","tool_call_id":"tc1","content":"42"}
        ]}"#;
        let out: Value = serde_json::from_slice(&translate(body).unwrap()).unwrap();
        let msg = &out["messages"][0];
        assert_eq!(msg["role"], "user");
        let block = &msg["content"][0];
        assert_eq!(block["type"], "tool_result");
        assert_eq!(block["tool_use_id"], "tc1");
        assert_eq!(block["content"], "42");
    }

    #[test]
    fn tools_unwrap_function_envelope() {
        let body = br#"{"model":"x","messages":[],"tools":[
            {"type":"function","function":{"name":"search","description":"web search","parameters":{"type":"object"}}}
        ]}"#;
        let out: Value = serde_json::from_slice(&translate(body).unwrap()).unwrap();
        let tool = &out["tools"][0];
        assert_eq!(tool["name"], "search");
        assert_eq!(tool["description"], "web search");
        assert_eq!(tool["input_schema"]["type"], "object");
        assert!(tool.get("function").is_none());
    }

    #[test]
    fn tool_choice_required_becomes_any() {
        let body = br#"{"model":"x","messages":[],"tool_choice":"required"}"#;
        let out: Value = serde_json::from_slice(&translate(body).unwrap()).unwrap();
        assert_eq!(out["tool_choice"], json!({"type": "any"}));
    }

    /// OpenAI spells `tool_choice` as a bare string; Anthropic only accepts an
    /// object. Emitting the string reaches the upstream as `"tool_choice":
    /// "auto"` and is rejected — MiniMax answers `400 invalid params`. The AI
    /// SDK that opencode uses sends `"auto"` on every tool-bearing request, so
    /// this is the common path, not an edge case.
    #[test]
    fn tool_choice_strings_become_anthropic_objects() {
        for (openai, anthropic) in [
            ("auto", json!({"type": "auto"})),
            ("required", json!({"type": "any"})),
            ("none", json!({"type": "none"})),
        ] {
            let body = format!(r#"{{"model":"x","messages":[],"tool_choice":"{openai}"}}"#);
            let out: Value = serde_json::from_slice(&translate(body.as_bytes()).unwrap()).unwrap();
            assert_eq!(
                out["tool_choice"], anthropic,
                "OpenAI tool_choice {openai:?} must translate to an object"
            );
            assert!(
                !out["tool_choice"].is_string(),
                "a bare string is rejected by the Anthropic API"
            );
        }
    }

    #[test]
    fn tool_choice_named_function_unwraps() {
        let body = br#"{"model":"x","messages":[],"tool_choice":{"type":"function","function":{"name":"search"}}}"#;
        let out: Value = serde_json::from_slice(&translate(body).unwrap()).unwrap();
        assert_eq!(out["tool_choice"]["type"], "tool");
        assert_eq!(out["tool_choice"]["name"], "search");
    }

    #[test]
    fn stop_renames_to_stop_sequences() {
        let body = br#"{"model":"x","messages":[],"stop":["END"]}"#;
        let out: Value = serde_json::from_slice(&translate(body).unwrap()).unwrap();
        assert_eq!(out["stop_sequences"][0], "END");
        assert!(out.get("stop").is_none());
    }

    #[test]
    fn missing_max_tokens_defaults_to_4096() {
        let body = br#"{"model":"x","messages":[]}"#;
        let out: Value = serde_json::from_slice(&translate(body).unwrap()).unwrap();
        assert_eq!(out["max_tokens"], 4096);
    }

    #[test]
    fn provided_max_tokens_is_preserved() {
        let body = br#"{"model":"x","messages":[],"max_tokens":1024}"#;
        let out: Value = serde_json::from_slice(&translate(body).unwrap()).unwrap();
        assert_eq!(out["max_tokens"], 1024);
    }

    #[test]
    fn image_url_part_dropped_with_warning() {
        let body = br#"{"model":"x","messages":[
            {"role":"user","content":[
                {"type":"text","text":"describe this"},
                {"type":"image_url","image_url":{"url":"https://example.com/img.png"}}
            ]}
        ]}"#;
        let out: Value = serde_json::from_slice(&translate(body).unwrap()).unwrap();
        // image_url dropped; only the text block remains → simplified to string
        assert_eq!(out["messages"][0]["content"], "describe this");
    }

    #[test]
    fn user_renames_to_metadata_user_id() {
        let body = br#"{"model":"x","messages":[],"user":"u42"}"#;
        let out: Value = serde_json::from_slice(&translate(body).unwrap()).unwrap();
        assert_eq!(out["metadata"]["user_id"], "u42");
        assert!(out.get("user").is_none());
    }
}
