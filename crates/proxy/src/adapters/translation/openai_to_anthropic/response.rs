//! Translate Anthropic /v1/messages response back to OpenAI /v1/chat/completions.

use chrono::Utc;
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
    // Detect Anthropic error responses (e.g. {"type":"error","error":{…}}).
    // These have no content/message structure — convert to OpenAI error format
    // instead of producing garbage through the success path.
    if v.get("type").and_then(|t| t.as_str()) == Some("error") {
        let err = v.get("error").unwrap_or(&Value::Null);
        tracing::debug!(
            target: "translation",
            direction = "anthropic_to_openai",
            error_type = err.get("type").and_then(|t| t.as_str()).unwrap_or("unknown"),
            error_message = err.get("message").and_then(|m| m.as_str()).unwrap_or(""),
            "translating Anthropic error response to OpenAI error format"
        );
        return Ok(json!({
            "error": {
                "message": err.get("message").and_then(|m| m.as_str()).unwrap_or("unknown error"),
                "type": err.get("type").and_then(|t| t.as_str()).unwrap_or("api_error"),
            }
        }));
    }

    // 1. Split content[] into text blocks and tool_use blocks
    let content_arr = v.get("content").and_then(|c| c.as_array());

    let mut text_parts = Vec::<String>::new();
    let mut tool_calls = Vec::<Value>::new();

    if let Some(blocks) = content_arr {
        for block in blocks {
            match block.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                "text" => {
                    if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                        text_parts.push(t.to_string());
                    }
                }
                "tool_use" => {
                    let id = block.get("id").cloned().unwrap_or(Value::String("".into()));
                    let name = block.get("name").cloned().unwrap_or(Value::Null);
                    let input = block.get("input").cloned().unwrap_or(json!({}));
                    let arguments = serde_json::to_string(&input).unwrap_or_else(|_| "{}".into());
                    tool_calls.push(json!({
                        "id": id,
                        "type": "function",
                        "function": {"name": name, "arguments": arguments}
                    }));
                }
                other => {
                    tracing::debug!(
                        target: "translation",
                        block_type = %other,
                        "unknown content block type, dropped"
                    );
                }
            }
        }
    }

    // 2. Build message object
    let content_str = if text_parts.is_empty() {
        String::new()
    } else {
        text_parts.join("\n")
    };

    let mut message = json!({"role": "assistant", "content": content_str});
    if !tool_calls.is_empty() {
        message["tool_calls"] = Value::Array(tool_calls);
    }

    // 3. Map stop_reason → finish_reason
    let finish_reason = match v.get("stop_reason").and_then(|s| s.as_str()) {
        Some("end_turn") => "stop",
        Some("tool_use") => "tool_calls",
        Some("max_tokens") => "length",
        Some("stop_sequence") => "stop",
        Some(other) => {
            tracing::debug!(
                target: "translation",
                stop_reason = %other,
                "unknown stop_reason; using stop"
            );
            "stop"
        }
        None => "stop",
    };

    // 4. Usage mapping
    let usage_in = v.get("usage").unwrap_or(&Value::Null);
    let input_tokens = usage_in
        .get("input_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let output_tokens = usage_in
        .get("output_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let total_tokens = input_tokens + output_tokens;

    let mut usage_out = json!({
        "prompt_tokens": input_tokens,
        "completion_tokens": output_tokens,
        "total_tokens": total_tokens,
    });

    if let Some(cached) = usage_in.get("cache_read_input_tokens") {
        usage_out["prompt_tokens_details"] = json!({"cached_tokens": cached});
    }

    // 5. id — keep as-is (stripping msg_ prefix is optional per spec; we keep it)
    let id = v
        .get("id")
        .and_then(|i| i.as_str())
        .unwrap_or("unknown")
        .to_string();

    Ok(json!({
        "id": id,
        "object": "chat.completion",
        "created": Utc::now().timestamp(),
        "model": v.get("model").cloned().unwrap_or(Value::Null),
        "choices": [{
            "index": 0,
            "message": message,
            "finish_reason": finish_reason,
        }],
        "usage": usage_out,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(body: &[u8]) -> Value {
        serde_json::from_slice(&translate(body).unwrap()).unwrap()
    }

    fn anthropic_response(
        content: Vec<Value>,
        stop_reason: &str,
        id: &str,
        model: &str,
        usage: Value,
    ) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "id": id,
            "type": "message",
            "role": "assistant",
            "model": model,
            "content": content,
            "stop_reason": stop_reason,
            "stop_sequence": null,
            "usage": usage,
        }))
        .unwrap()
    }

    #[test]
    fn text_only_response() {
        let body = anthropic_response(
            vec![json!({"type": "text", "text": "Hello world"})],
            "end_turn",
            "msg_abc",
            "claude-3-5-sonnet",
            json!({"input_tokens": 10, "output_tokens": 5}),
        );
        let out = run(&body);
        assert_eq!(out["object"], "chat.completion");
        assert_eq!(out["choices"][0]["message"]["role"], "assistant");
        assert_eq!(out["choices"][0]["message"]["content"], "Hello world");
        assert_eq!(out["choices"][0]["finish_reason"], "stop");
        assert_eq!(out["model"], "claude-3-5-sonnet");
    }

    #[test]
    fn tool_use_only_response() {
        let body = anthropic_response(
            vec![
                json!({"type": "tool_use", "id": "tu_1", "name": "search", "input": {"q": "rust"}}),
            ],
            "tool_use",
            "msg_xyz",
            "claude-3-5-sonnet",
            json!({"input_tokens": 20, "output_tokens": 30}),
        );
        let out = run(&body);
        assert_eq!(out["choices"][0]["finish_reason"], "tool_calls");
        let tool_calls = &out["choices"][0]["message"]["tool_calls"];
        assert_eq!(tool_calls[0]["id"], "tu_1");
        assert_eq!(tool_calls[0]["type"], "function");
        assert_eq!(tool_calls[0]["function"]["name"], "search");
        let args: Value =
            serde_json::from_str(tool_calls[0]["function"]["arguments"].as_str().unwrap()).unwrap();
        assert_eq!(args["q"], "rust");
    }

    #[test]
    fn text_plus_tool_use() {
        let body = anthropic_response(
            vec![
                json!({"type": "text", "text": "Searching now."}),
                json!({"type": "tool_use", "id": "tu_2", "name": "web", "input": {"url": "https://example.com"}}),
            ],
            "tool_use",
            "msg_both",
            "claude-3-5-sonnet",
            json!({"input_tokens": 15, "output_tokens": 10}),
        );
        let out = run(&body);
        assert_eq!(out["choices"][0]["message"]["content"], "Searching now.");
        assert_eq!(
            out["choices"][0]["message"]["tool_calls"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(out["choices"][0]["finish_reason"], "tool_calls");
    }

    #[test]
    fn stop_reason_mappings() {
        let cases = [
            ("end_turn", "stop"),
            ("tool_use", "tool_calls"),
            ("max_tokens", "length"),
            ("stop_sequence", "stop"),
        ];
        for (anthropic_reason, openai_reason) in cases {
            let body = anthropic_response(
                vec![json!({"type": "text", "text": "x"})],
                anthropic_reason,
                "id",
                "model",
                json!({"input_tokens": 1, "output_tokens": 1}),
            );
            let out = run(&body);
            assert_eq!(
                out["choices"][0]["finish_reason"], openai_reason,
                "stop_reason {anthropic_reason} should map to {openai_reason}"
            );
        }
    }

    #[test]
    fn usage_translates_with_cached_tokens() {
        let body = anthropic_response(
            vec![json!({"type": "text", "text": "hi"})],
            "end_turn",
            "id",
            "model",
            json!({
                "input_tokens": 100,
                "output_tokens": 50,
                "cache_read_input_tokens": 40,
            }),
        );
        let out = run(&body);
        assert_eq!(out["usage"]["prompt_tokens"], 100);
        assert_eq!(out["usage"]["completion_tokens"], 50);
        assert_eq!(out["usage"]["total_tokens"], 150);
        assert_eq!(out["usage"]["prompt_tokens_details"]["cached_tokens"], 40);
    }

    #[test]
    fn usage_without_cached_tokens_omits_details() {
        let body = anthropic_response(
            vec![json!({"type": "text", "text": "hi"})],
            "end_turn",
            "id",
            "model",
            json!({"input_tokens": 10, "output_tokens": 5}),
        );
        let out = run(&body);
        assert_eq!(out["usage"]["total_tokens"], 15);
        assert!(out["usage"].get("prompt_tokens_details").is_none());
    }

    #[test]
    fn empty_content_yields_empty_string() {
        let body = anthropic_response(
            vec![],
            "end_turn",
            "id",
            "model",
            json!({"input_tokens": 0, "output_tokens": 0}),
        );
        let out = run(&body);
        assert_eq!(out["choices"][0]["message"]["content"], "");
        assert!(out["choices"][0]["message"].get("tool_calls").is_none());
    }

    #[test]
    fn id_passthrough() {
        let body = anthropic_response(
            vec![json!({"type": "text", "text": "hi"})],
            "end_turn",
            "msg_original_id",
            "model",
            json!({"input_tokens": 1, "output_tokens": 1}),
        );
        let out = run(&body);
        assert_eq!(out["id"], "msg_original_id");
    }

    #[test]
    fn multi_tool_use_each_become_tool_calls() {
        let body = anthropic_response(
            vec![
                json!({"type": "tool_use", "id": "a", "name": "fn_a", "input": {"x": 1}}),
                json!({"type": "tool_use", "id": "b", "name": "fn_b", "input": {"y": 2}}),
                json!({"type": "tool_use", "id": "c", "name": "fn_c", "input": {"z": 3}}),
            ],
            "tool_use",
            "id",
            "model",
            json!({"input_tokens": 5, "output_tokens": 10}),
        );
        let out = run(&body);
        let tcs = out["choices"][0]["message"]["tool_calls"]
            .as_array()
            .unwrap();
        assert_eq!(tcs.len(), 3);
        assert_eq!(tcs[0]["id"], "a");
        assert_eq!(tcs[1]["id"], "b");
        assert_eq!(tcs[2]["id"], "c");
    }

    #[test]
    fn top_level_shape_fields_present() {
        let body = anthropic_response(
            vec![json!({"type": "text", "text": "hello"})],
            "end_turn",
            "msg_1",
            "claude-opus",
            json!({"input_tokens": 5, "output_tokens": 3}),
        );
        let out = run(&body);
        assert_eq!(out["object"], "chat.completion");
        assert!(out["created"].as_i64().is_some());
        assert!(out.get("model").is_some());
        assert_eq!(out["choices"][0]["index"], 0);
    }

    #[test]
    fn multiple_text_blocks_joined_with_newline() {
        let body = anthropic_response(
            vec![
                json!({"type": "text", "text": "part one"}),
                json!({"type": "text", "text": "part two"}),
            ],
            "end_turn",
            "id",
            "model",
            json!({"input_tokens": 5, "output_tokens": 5}),
        );
        let out = run(&body);
        assert_eq!(
            out["choices"][0]["message"]["content"],
            "part one\npart two"
        );
    }

    #[test]
    fn anthropic_error_response_converts_to_openai_error() {
        let body = serde_json::to_vec(&json!({
            "type": "error",
            "error": {
                "type": "invalid_request_error",
                "message": "model not found"
            }
        }))
        .unwrap();
        let out = run(&body);
        assert!(out.get("error").is_some());
        assert_eq!(out["error"]["type"], "invalid_request_error");
        assert_eq!(out["error"]["message"], "model not found");
        // Must NOT produce garbage through the success path
        assert!(
            out.get("choices").is_none(),
            "error should not have 'choices' field"
        );
    }
}
