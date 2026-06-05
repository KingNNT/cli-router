//! Translate OpenAI chat completion response back to Anthropic /v1/messages.

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
    // Detect OpenAI error responses (e.g. 400 "invalid_function_parameters").
    // These have an "error" object but no "choices" array — feeding them through
    // the success path produces garbage like {"id":"msg_unknown","model":null,…}.
    // Convert to Anthropic error format instead.
    if v.get("error").is_some() && v.get("choices").is_none() {
        let err = &v["error"];
        tracing::debug!(
            target: "translation",
            direction = "openai_to_anthropic",
            error_type = err.get("type").and_then(|t| t.as_str()).unwrap_or("unknown"),
            error_message = err.get("message").and_then(|m| m.as_str()).unwrap_or(""),
            "translating OpenAI error response to Anthropic error format"
        );
        return Ok(json!({
            "type": "error",
            "error": {
                "type": err.get("type").and_then(|t| t.as_str()).unwrap_or("api_error"),
                "message": err.get("message").and_then(|m| m.as_str()).unwrap_or("unknown error"),
            }
        }));
    }

    // 1. Build content array: text from message.content + tool_use blocks from message.tool_calls
    let choice0 = v
        .get("choices")
        .and_then(|c| c.get(0))
        .unwrap_or(&Value::Null);
    let message = choice0.get("message").unwrap_or(&Value::Null);

    let mut content = Vec::<Value>::new();

    if let Some(text) = message.get("content").and_then(|c| c.as_str())
        && !text.is_empty()
    {
        content.push(json!({"type": "text", "text": text}));
    }

    if let Some(tool_calls) = message.get("tool_calls").and_then(|t| t.as_array()) {
        for tc in tool_calls {
            let id = tc.get("id").cloned().unwrap_or(Value::String("".into()));
            let name = tc.pointer("/function/name").cloned().unwrap_or(Value::Null);
            let args_str = tc
                .pointer("/function/arguments")
                .and_then(|a| a.as_str())
                .unwrap_or("{}");
            let input: Value =
                serde_json::from_str(args_str).unwrap_or(Value::Object(serde_json::Map::new()));
            content.push(json!({"type": "tool_use", "id": id, "name": name, "input": input}));
        }
    }

    // 2. Map finish_reason → stop_reason
    let stop_reason = match choice0.get("finish_reason").and_then(|s| s.as_str()) {
        Some("stop") => Value::String("end_turn".into()),
        Some("tool_calls") => Value::String("tool_use".into()),
        Some("length") => Value::String("max_tokens".into()),
        Some("content_filter") => {
            tracing::warn!(
                target: "translation",
                "openai content_filter mapped to anthropic end_turn"
            );
            Value::String("end_turn".into())
        }
        Some(other) => {
            tracing::debug!(
                target: "translation",
                finish_reason = %other,
                "unknown finish_reason; using end_turn"
            );
            Value::String("end_turn".into())
        }
        None => Value::Null,
    };

    // 3. Usage mapping
    let usage_in = v.get("usage").unwrap_or(&Value::Null);
    let mut usage_out = json!({
        "input_tokens": usage_in.get("prompt_tokens").cloned().unwrap_or(json!(0)),
        "output_tokens": usage_in.get("completion_tokens").cloned().unwrap_or(json!(0)),
    });
    if let Some(cached) = usage_in.pointer("/prompt_tokens_details/cached_tokens") {
        usage_out["cache_read_input_tokens"] = cached.clone();
    }

    // 4. id: prefix with msg_ if not already present
    let id = v
        .get("id")
        .and_then(|i| i.as_str())
        .map(|s| {
            if s.starts_with("msg_") {
                s.to_string()
            } else {
                format!("msg_{s}")
            }
        })
        .unwrap_or_else(|| "msg_unknown".into());

    Ok(json!({
        "id": id,
        "type": "message",
        "role": "assistant",
        "model": v.get("model").cloned().unwrap_or(Value::Null),
        "content": content,
        "stop_reason": stop_reason,
        "stop_sequence": Value::Null,
        "usage": usage_out,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(body: &[u8]) -> Value {
        serde_json::from_slice(&translate(body).unwrap()).unwrap()
    }

    fn openai_response(
        content: Option<&str>,
        tool_calls: Option<Value>,
        finish_reason: &str,
        id: &str,
        model: &str,
        usage: Value,
    ) -> Vec<u8> {
        let mut message = json!({"role": "assistant"});
        if let Some(c) = content {
            message["content"] = json!(c);
        }
        if let Some(tc) = tool_calls {
            message["tool_calls"] = tc;
        }
        serde_json::to_vec(&json!({
            "id": id,
            "object": "chat.completion",
            "model": model,
            "choices": [{"index": 0, "message": message, "finish_reason": finish_reason}],
            "usage": usage,
        }))
        .unwrap()
    }

    #[test]
    fn text_only_response() {
        let body = openai_response(
            Some("Hello world"),
            None,
            "stop",
            "chatcmpl-abc",
            "gpt-4",
            json!({"prompt_tokens": 10, "completion_tokens": 5}),
        );
        let out = run(&body);
        assert_eq!(out["type"], "message");
        assert_eq!(out["role"], "assistant");
        assert_eq!(out["content"][0]["type"], "text");
        assert_eq!(out["content"][0]["text"], "Hello world");
        assert_eq!(out["stop_reason"], "end_turn");
        assert_eq!(out["model"], "gpt-4");
    }

    #[test]
    fn tool_call_only_response() {
        let tool_calls = json!([{
            "id": "call_1",
            "type": "function",
            "function": {"name": "get_weather", "arguments": r#"{"city":"NYC"}"#}
        }]);
        let body = openai_response(
            None,
            Some(tool_calls),
            "tool_calls",
            "chatcmpl-xyz",
            "gpt-4",
            json!({"prompt_tokens": 20, "completion_tokens": 30}),
        );
        let out = run(&body);
        assert_eq!(out["content"].as_array().unwrap().len(), 1);
        assert_eq!(out["content"][0]["type"], "tool_use");
        assert_eq!(out["content"][0]["id"], "call_1");
        assert_eq!(out["content"][0]["name"], "get_weather");
        assert_eq!(out["content"][0]["input"]["city"], "NYC");
        assert_eq!(out["stop_reason"], "tool_use");
    }

    #[test]
    fn text_plus_tool_call() {
        let tool_calls = json!([{
            "id": "call_2",
            "type": "function",
            "function": {"name": "search", "arguments": r#"{"q":"rust"}"#}
        }]);
        let body = openai_response(
            Some("Let me search for you."),
            Some(tool_calls),
            "tool_calls",
            "chatcmpl-both",
            "gpt-4",
            json!({"prompt_tokens": 15, "completion_tokens": 10}),
        );
        let out = run(&body);
        assert_eq!(out["content"].as_array().unwrap().len(), 2);
        assert_eq!(out["content"][0]["type"], "text");
        assert_eq!(out["content"][0]["text"], "Let me search for you.");
        assert_eq!(out["content"][1]["type"], "tool_use");
        assert_eq!(out["content"][1]["name"], "search");
    }

    #[test]
    fn finish_reason_mappings() {
        let cases = [
            ("stop", "end_turn"),
            ("tool_calls", "tool_use"),
            ("length", "max_tokens"),
            ("content_filter", "end_turn"),
        ];
        for (openai_reason, anthropic_reason) in cases {
            let body = openai_response(
                Some("text"),
                None,
                openai_reason,
                "id",
                "model",
                json!({"prompt_tokens": 1, "completion_tokens": 1}),
            );
            let out = run(&body);
            assert_eq!(
                out["stop_reason"], anthropic_reason,
                "finish_reason {openai_reason} should map to {anthropic_reason}"
            );
        }
    }

    #[test]
    fn usage_translates_with_cached_tokens() {
        let body = openai_response(
            Some("hi"),
            None,
            "stop",
            "id",
            "model",
            json!({
                "prompt_tokens": 100,
                "completion_tokens": 50,
                "prompt_tokens_details": {"cached_tokens": 40}
            }),
        );
        let out = run(&body);
        assert_eq!(out["usage"]["input_tokens"], 100);
        assert_eq!(out["usage"]["output_tokens"], 50);
        assert_eq!(out["usage"]["cache_read_input_tokens"], 40);
    }

    #[test]
    fn usage_without_cached_tokens_omits_cache_field() {
        let body = openai_response(
            Some("hi"),
            None,
            "stop",
            "id",
            "model",
            json!({"prompt_tokens": 10, "completion_tokens": 5}),
        );
        let out = run(&body);
        assert!(out["usage"].get("cache_read_input_tokens").is_none());
    }

    #[test]
    fn empty_content_yields_empty_array() {
        let body = openai_response(
            Some(""),
            None,
            "stop",
            "id",
            "model",
            json!({"prompt_tokens": 0, "completion_tokens": 0}),
        );
        let out = run(&body);
        assert_eq!(out["content"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn id_gets_msg_prefix_when_missing() {
        let body = openai_response(
            Some("hi"),
            None,
            "stop",
            "chatcmpl-abc123",
            "model",
            json!({"prompt_tokens": 1, "completion_tokens": 1}),
        );
        let out = run(&body);
        assert!(
            out["id"].as_str().unwrap().starts_with("msg_"),
            "id should have msg_ prefix"
        );
        assert_eq!(out["id"], "msg_chatcmpl-abc123");
    }

    #[test]
    fn id_keeps_msg_prefix_when_present() {
        let body = openai_response(
            Some("hi"),
            None,
            "stop",
            "msg_already_prefixed",
            "model",
            json!({"prompt_tokens": 1, "completion_tokens": 1}),
        );
        let out = run(&body);
        assert_eq!(out["id"], "msg_already_prefixed");
    }

    #[test]
    fn multi_tool_calls_each_become_tool_use_block() {
        let tool_calls = json!([
            {"id": "tc1", "type": "function", "function": {"name": "tool_a", "arguments": r#"{"a":1}"#}},
            {"id": "tc2", "type": "function", "function": {"name": "tool_b", "arguments": r#"{"b":2}"#}},
            {"id": "tc3", "type": "function", "function": {"name": "tool_c", "arguments": r#"{"c":3}"#}},
        ]);
        let body = openai_response(
            None,
            Some(tool_calls),
            "tool_calls",
            "id",
            "model",
            json!({"prompt_tokens": 5, "completion_tokens": 10}),
        );
        let out = run(&body);
        let content = out["content"].as_array().unwrap();
        assert_eq!(content.len(), 3);
        assert_eq!(content[0]["id"], "tc1");
        assert_eq!(content[1]["id"], "tc2");
        assert_eq!(content[2]["id"], "tc3");
        assert_eq!(content[0]["input"]["a"], 1);
        assert_eq!(content[1]["input"]["b"], 2);
        assert_eq!(content[2]["input"]["c"], 3);
    }

    #[test]
    fn top_level_shape_fields_present() {
        let body = openai_response(
            Some("hello"),
            None,
            "stop",
            "chatcmpl-1",
            "gpt-4o",
            json!({"prompt_tokens": 5, "completion_tokens": 3}),
        );
        let out = run(&body);
        assert_eq!(out["type"], "message");
        assert_eq!(out["role"], "assistant");
        assert_eq!(out["stop_sequence"], Value::Null);
        assert!(out.get("model").is_some());
        assert!(out.get("usage").is_some());
    }

    #[test]
    fn openai_error_response_converts_to_anthropic_error() {
        // Simulates the 400 error Codex returns for invalid tool schemas
        let body = serde_json::to_vec(&json!({
            "error": {
                "message": "Invalid schema for function 'playwright_browser_click': Missing 'button'.",
                "type": "invalid_request_error",
                "param": "tools[4].parameters",
                "code": "invalid_function_parameters"
            }
        }))
        .unwrap();
        let out = run(&body);
        assert_eq!(out["type"], "error");
        assert_eq!(out["error"]["type"], "invalid_request_error");
        assert!(
            out["error"]["message"]
                .as_str()
                .unwrap()
                .contains("playwright_browser_click")
        );
        // Must NOT produce the old garbage: {"id":"msg_unknown","model":null,…}
        assert!(out.get("id").is_none(), "error should not have 'id' field");
        assert!(
            out.get("model").is_none(),
            "error should not have 'model' field"
        );
    }
}
