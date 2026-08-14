//! Codex provider — implements `Provider` against OpenAI's Codex Responses API
//! at `https://chatgpt.com/backend-api/codex/responses`.
//!
//! Translates Chat Completions requests into the OpenAI Responses API format
//! and translates responses (both streaming and buffered) back.

use super::messages_protocol::{self, AuthHeader, HOP_BY_HOP};
use crate::adapters::translation::openai_to_responses::{
    ResponsesDialect, ResponsesSseTranslator, translate_request,
};
use crate::application::errors::ProxyError;
use crate::application::ports::{
    ApiFormat, BoxedByteStream, FormatSupport, Provider, UpstreamResponse, UsageParser,
};
use crate::domain::UsageRecord;
use async_trait::async_trait;
use axum::http::HeaderMap;
use bytes::Bytes;
use futures::StreamExt;
use serde_json::{Value, json};

const DEFAULT_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";

// ---------------------------------------------------------------------------
// Struct & constructors
// ---------------------------------------------------------------------------

pub struct CodexProvider {
    base_url: String,
    http: reqwest::Client,
    auth: AuthHeader,
    thinking: Option<super::thinking::ThinkingInjection>,
}

impl CodexProvider {
    pub fn new(http: reqwest::Client) -> Self {
        Self::build(http, DEFAULT_BASE_URL.into(), AuthHeader::Passthrough, None)
    }

    pub fn with_base_url(http: reqwest::Client, base_url: impl Into<String>) -> Self {
        Self::build(http, base_url.into(), AuthHeader::Passthrough, None)
    }

    pub fn with_auth(http: reqwest::Client, auth: AuthHeader) -> Self {
        Self::build(http, DEFAULT_BASE_URL.into(), auth, None)
    }

    pub fn configure(http: reqwest::Client, base_url: Option<String>, auth: AuthHeader) -> Self {
        Self::configure_with_thinking(http, base_url, auth, None)
    }

    pub fn configure_with_thinking(
        http: reqwest::Client,
        base_url: Option<String>,
        auth: AuthHeader,
        thinking: Option<super::thinking::ThinkingInjection>,
    ) -> Self {
        Self::build(
            http,
            base_url.unwrap_or_else(|| DEFAULT_BASE_URL.into()),
            auth,
            thinking,
        )
    }

    fn build(
        http: reqwest::Client,
        base_url: String,
        auth: AuthHeader,
        thinking: Option<super::thinking::ThinkingInjection>,
    ) -> Self {
        Self {
            base_url,
            http,
            auth,
            thinking,
        }
    }
}

// ---------------------------------------------------------------------------
// Provider trait impl
// ---------------------------------------------------------------------------

#[async_trait]
impl Provider for CodexProvider {
    fn name(&self) -> &str {
        "codex"
    }

    fn supported_formats(&self) -> FormatSupport {
        FormatSupport::single(ApiFormat::OpenAI)
    }

    fn parse_model(&self, body: &[u8]) -> Result<String, String> {
        messages_protocol::parse_model(body)
    }

    fn usage_parser(&self) -> Box<dyn UsageParser> {
        messages_protocol::openai_usage_parser()
    }

    fn parse_usage_json(&self, body: &[u8]) -> Result<UsageRecord, String> {
        messages_protocol::parse_openai_usage_json(body)
    }

    fn usage_parser_openai(&self) -> Box<dyn UsageParser> {
        messages_protocol::openai_usage_parser()
    }

    fn parse_usage_json_openai(&self, body: &[u8]) -> Result<UsageRecord, String> {
        messages_protocol::parse_openai_usage_json(body)
    }

    async fn forward(
        &self,
        path: &str,
        headers: &HeaderMap,
        body: Bytes,
        streaming: bool,
    ) -> Result<UpstreamResponse, ProxyError> {
        let _ = (path, headers, body, streaming);
        Err(ProxyError::BadRequest(
            "provider 'codex' does not support Anthropic messages format; use the OpenAI-compatible endpoint".into(),
        ))
    }

    async fn forward_openai(
        &self,
        _path: &str,
        headers: &HeaderMap,
        body: Bytes,
        streaming: bool,
    ) -> Result<UpstreamResponse, ProxyError> {
        let body = match &self.thinking {
            Some(injection) => injection.apply(body),
            None => body,
        };

        // Parse incoming Chat Completions body
        let chat_body: Value = serde_json::from_slice(&body)
            .map_err(|e| ProxyError::BadRequest(format!("invalid JSON body: {e}")))?;

        // Translate to Responses API payload (always sets stream:true)
        let responses_body =
            translate_request(&chat_body, &ResponsesDialect::codex()).map_err(|e| {
                ProxyError::BadRequest(format!("codex request translation failed: {e}"))
            })?;

        let url = format!("{}/responses", self.base_url.trim_end_matches('/'));
        let serialized = serde_json::to_vec(&responses_body).map_err(|e| {
            ProxyError::BadRequest(format!("failed to serialize responses body: {e}"))
        })?;

        // Build the upstream request. `identity` because the SSE translator
        // below reads the raw bytes — see HOP_BY_HOP.
        let mut req = self
            .http
            .post(&url)
            .body(serialized)
            .header("accept-encoding", "identity");
        let strip_auth = !matches!(self.auth, AuthHeader::Passthrough);
        for (k, v) in headers {
            if HOP_BY_HOP.contains(&k.as_str()) {
                continue;
            }
            // Strip content-type to avoid duplicates — we set it ourselves below.
            if k.as_str() == "content-type" {
                continue;
            }
            if strip_auth && matches!(k.as_str(), "x-api-key" | "authorization") {
                continue;
            }
            req = req.header(k, v);
        }
        match &self.auth {
            AuthHeader::Passthrough => {}
            AuthHeader::ApiKey(v) => {
                req = req.header("x-api-key", v);
            }
            AuthHeader::Bearer(v) => {
                req = req.header("authorization", format!("Bearer {v}"));
            }
            AuthHeader::OAuth { access_token, .. } => {
                req = req.header("authorization", format!("Bearer {access_token}"));
            }
        }
        req = req.header("content-type", "application/json");

        let resp = req.send().await?;
        let status = resp.status().as_u16();
        let mut headers_out = HeaderMap::new();
        for (k, v) in resp.headers() {
            if HOP_BY_HOP.contains(&k.as_str()) {
                continue;
            }
            headers_out.insert(k.clone(), v.clone());
        }

        // Pass through error responses untranslated.
        if status >= 400 {
            let raw_body = resp.bytes().await?;
            return Ok(UpstreamResponse::Buffered {
                status,
                headers: headers_out,
                body: raw_body,
                provider_id: self.name().to_string(),
                translation_direction: None,
            });
        }

        // Codex backend always streams, so we always wrap with the translator.
        let upstream_bytes: BoxedByteStream = Box::pin(resp.bytes_stream().map(|res| {
            res.map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })
        }));
        let translated: BoxedByteStream = Box::pin(ResponsesSseTranslator::new(upstream_bytes));

        if streaming {
            // The Codex backend doesn't return content-type: text/event-stream,
            // so we must inject it for SSE clients (like the AI SDK).
            headers_out.insert(
                axum::http::header::CONTENT_TYPE,
                axum::http::HeaderValue::from_static("text/event-stream"),
            );
            Ok(UpstreamResponse::Streaming {
                status,
                headers: headers_out,
                body: translated,
                provider_id: self.name().to_string(),
                translation_direction: None,
            })
        } else {
            // Buffer the translated SSE stream into a single Chat Completions response.
            use futures::StreamExt;
            let mut collected_content = String::new();
            let mut tool_calls: Vec<Value> = Vec::new();
            let mut tool_args_accum: std::collections::HashMap<usize, String> =
                std::collections::HashMap::new();
            let mut finish_reason = "stop".to_string();
            let mut usage: Option<Value> = None;
            let mut model = chat_body
                .get("model")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown")
                .to_string();

            let mut stream = translated;
            while let Some(chunk) = stream.next().await {
                let bytes = chunk
                    .map_err(|e| ProxyError::BadRequest(format!("codex stream error: {e}")))?;
                let text = String::from_utf8_lossy(&bytes);
                // Parse SSE data lines
                for line in text.lines() {
                    let Some(data) = line.strip_prefix("data: ") else {
                        continue;
                    };
                    if data.trim() == "[DONE]" {
                        continue;
                    }
                    if let Ok(val) = serde_json::from_str::<Value>(data) {
                        let choice = val.get("choices").and_then(|c| c.get(0));
                        // Accumulate content deltas
                        if let Some(content) = choice
                            .and_then(|c| c.get("delta"))
                            .and_then(|d| d.get("content"))
                            .and_then(|c| c.as_str())
                        {
                            collected_content.push_str(content);
                        }
                        // Accumulate tool_call deltas (streaming chunks split
                        // name+id and arguments across separate deltas).
                        if let Some(tcs) = choice
                            .and_then(|c| c.get("delta"))
                            .and_then(|d| d.get("tool_calls"))
                            .and_then(|v| v.as_array())
                        {
                            for tc in tcs {
                                let idx =
                                    tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                                // Ensure slot exists
                                while tool_calls.len() <= idx {
                                    tool_calls.push(json!({
                                        "index": tool_calls.len(),
                                        "id": "",
                                        "type": "function",
                                        "function": {"name": "", "arguments": ""}
                                    }));
                                }
                                // Fill id / name on first appearance
                                if let Some(id) = tc.get("id").and_then(|v| v.as_str()) {
                                    tool_calls[idx]["id"] = json!(id);
                                }
                                if let Some(name) =
                                    tc.pointer("/function/name").and_then(|v| v.as_str())
                                {
                                    tool_calls[idx]["function"]["name"] = json!(name);
                                }
                                // Accumulate arguments
                                if let Some(args) =
                                    tc.pointer("/function/arguments").and_then(|v| v.as_str())
                                {
                                    let entry = tool_args_accum.entry(idx).or_default();
                                    entry.push_str(args);
                                }
                            }
                        }
                        // Capture finish_reason from final chunk
                        if let Some(fr) = choice
                            .and_then(|c| c.get("finish_reason"))
                            .and_then(|v| v.as_str())
                        {
                            finish_reason = fr.to_string();
                        }
                        // Capture usage from final chunk
                        if let Some(u) = val.get("usage").cloned() {
                            usage = Some(u);
                        }
                    }
                }
            }

            // Merge accumulated arguments into tool_call slots
            for (idx, args) in tool_args_accum {
                if idx < tool_calls.len() {
                    tool_calls[idx]["function"]["arguments"] = json!(args);
                }
            }

            let mut message = json!({
                "role": "assistant",
                "content": if collected_content.is_empty() {
                    Value::Null
                } else {
                    Value::String(collected_content)
                }
            });
            if !tool_calls.is_empty() {
                message["tool_calls"] = Value::Array(tool_calls);
            }
            // Strip the namespace prefix from the model for the response.
            if let Some(slash_pos) = model.find('/') {
                model = model[slash_pos + 1..].to_string();
            }

            let mut result = json!({
                "id": "",
                "object": "chat.completion",
                "model": model,
                "choices": [{
                    "index": 0,
                    "message": message,
                    "finish_reason": finish_reason
                }]
            });
            if let Some(u) = usage {
                result["usage"] = u;
            }
            let body = serde_json::to_vec(&result).unwrap_or_default().into();
            Ok(UpstreamResponse::Buffered {
                status,
                headers: headers_out,
                body,
                provider_id: self.name().to_string(),
                translation_direction: None,
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::translation::openai_to_responses::translate_buffered_response;

    // -- Task 2: Construction tests --

    #[test]
    fn name_is_codex() {
        let p = CodexProvider::new(reqwest::Client::new());
        assert_eq!(p.name(), "codex");
    }

    #[test]
    fn supported_formats_is_openai_only() {
        let p = CodexProvider::new(reqwest::Client::new());
        assert!(p.supported_formats().has(ApiFormat::OpenAI));
        assert!(!p.supported_formats().has(ApiFormat::Anthropic));
    }

    #[test]
    fn default_base_url_points_to_codex() {
        let p = CodexProvider::new(reqwest::Client::new());
        assert_eq!(p.base_url, "https://chatgpt.com/backend-api/codex");
    }

    #[test]
    fn with_base_url_overrides_default() {
        let p = CodexProvider::with_base_url(reqwest::Client::new(), "http://localhost:1234");
        assert_eq!(p.base_url, "http://localhost:1234");
    }

    #[test]
    fn default_auth_is_passthrough() {
        let p = CodexProvider::new(reqwest::Client::new());
        assert!(matches!(p.auth, AuthHeader::Passthrough));
    }

    #[test]
    fn configure_sets_base_url_and_auth() {
        let p = CodexProvider::configure(
            reqwest::Client::new(),
            Some("http://localhost:1234".into()),
            AuthHeader::Bearer("sk-test".into()),
        );
        assert_eq!(p.base_url, "http://localhost:1234");
        assert!(matches!(p.auth, AuthHeader::Bearer(_)));
    }

    #[test]
    fn configure_uses_default_base_url_when_none() {
        let p = CodexProvider::configure(reqwest::Client::new(), None, AuthHeader::Passthrough);
        assert_eq!(p.base_url, "https://chatgpt.com/backend-api/codex");
    }

    #[test]
    fn parses_model_from_body() {
        let body = br#"{"model":"codex-mini","messages":[]}"#;
        assert_eq!(
            CodexProvider::new(reqwest::Client::new())
                .parse_model(body)
                .unwrap(),
            "codex-mini"
        );
    }

    // -- Task 3: Request translation tests --

    #[test]
    fn translate_simple_message() {
        let chat = json!({
            "model": "codex-mini",
            "messages": [{"role": "user", "content": "Hello"}]
        });
        let result = translate_request(&chat, &ResponsesDialect::codex()).unwrap();
        assert_eq!(result["model"], "codex-mini");
        let input = result["input"].as_array().unwrap();
        assert_eq!(input.len(), 1);
        assert_eq!(input[0]["role"], "user");
        // Content is a plain string for the Codex backend.
        assert_eq!(input[0]["content"], "Hello");
    }

    #[test]
    fn translate_openai_text_content_parts_preserves_text() {
        let chat = json!({
            "model": "codex-mini",
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "Check the issue again"}
                ]
            }]
        });

        let result = translate_request(&chat, &ResponsesDialect::codex()).unwrap();
        let input = result["input"].as_array().unwrap();
        assert_eq!(input.len(), 1);
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[0]["content"], "Check the issue again");
    }

    #[test]
    fn translate_extracts_system_to_instructions() {
        let chat = json!({
            "model": "codex-mini",
            "messages": [
                {"role": "system", "content": "Be concise"},
                {"role": "user", "content": "Hi"}
            ]
        });
        let result = translate_request(&chat, &ResponsesDialect::codex()).unwrap();
        assert_eq!(result["instructions"], "Be concise");
        // System message should not be in input
        let input = result["input"].as_array().unwrap();
        assert_eq!(input.len(), 1);
        assert_eq!(input[0]["role"], "user");
    }

    #[test]
    fn translate_extracts_developer_to_instructions() {
        let chat = json!({
            "model": "codex-mini",
            "messages": [
                {"role": "developer", "content": "You are a code assistant"},
                {"role": "user", "content": "Write code"}
            ]
        });
        let result = translate_request(&chat, &ResponsesDialect::codex()).unwrap();
        assert_eq!(result["instructions"], "You are a code assistant");
        let input = result["input"].as_array().unwrap();
        assert_eq!(input.len(), 1);
        assert_eq!(input[0]["role"], "user");
    }

    #[test]
    fn translate_drops_max_tokens() {
        // The Codex backend rejects max_output_tokens, so we must not send it.
        let chat = json!({
            "model": "codex-mini",
            "messages": [{"role": "user", "content": "Hello"}],
            "max_tokens": 4096
        });
        let result = translate_request(&chat, &ResponsesDialect::codex()).unwrap();
        assert!(
            result.get("max_output_tokens").is_none(),
            "max_output_tokens must NOT be sent to Codex backend"
        );
        assert!(
            result.get("max_tokens").is_none(),
            "max_tokens must not appear in translated request"
        );
    }

    #[test]
    fn translate_reasoning_effort() {
        let chat = json!({
            "model": "codex-mini",
            "messages": [{"role": "user", "content": "Hello"}],
            "reasoning_effort": "high"
        });
        let result = translate_request(&chat, &ResponsesDialect::codex()).unwrap();
        assert_eq!(result["reasoning"]["effort"], "high");
    }

    #[test]
    fn translate_without_reasoning_effort_keeps_reasoning_absent() {
        let chat = json!({
            "model": "codex-mini",
            "messages": [{"role": "user", "content": "Hello"}]
        });
        let result = translate_request(&chat, &ResponsesDialect::codex()).unwrap();
        assert!(result.get("reasoning").is_none());
    }

    /// Codex takes a Chat Completions body and translates it to the Responses
    /// API. The patch must land before that translation so the existing
    /// `reasoning_effort` → `reasoning.effort` mapping picks it up.
    #[test]
    fn thinking_patch_reaches_reasoning_effort_through_translation() {
        let injection = crate::adapters::providers::thinking::ThinkingInjection::new(
            serde_json::json!({"reasoning_effort": "xhigh"}),
            true,
        );
        let patched = injection.apply(Bytes::from(
            r#"{"model":"gpt-5.5","messages":[{"role":"user","content":"hi"}]}"#,
        ));
        let chat: Value = serde_json::from_slice(&patched).unwrap();
        let responses = translate_request(&chat, &ResponsesDialect::codex()).unwrap();
        assert_eq!(responses["reasoning"]["effort"], "xhigh");
    }

    #[test]
    fn translate_includes_required_fields() {
        let chat = json!({
            "model": "codex-mini",
            "messages": [{"role": "user", "content": "Hello"}]
        });
        let result = translate_request(&chat, &ResponsesDialect::codex()).unwrap();
        assert_eq!(result["store"], false);
        assert_eq!(result["stream"], true, "Codex backend requires stream=true");
        let include = result["include"].as_array().unwrap();
        assert!(include.contains(&json!("reasoning.encrypted_content")));
    }

    #[test]
    fn translate_default_instructions_when_no_system() {
        let chat = json!({
            "model": "codex-mini",
            "messages": [{"role": "user", "content": "Hello"}]
        });
        let result = translate_request(&chat, &ResponsesDialect::codex()).unwrap();
        assert_eq!(result["instructions"], "You are a helpful assistant.");
    }

    #[test]
    fn translate_multi_turn_conversation() {
        let chat = json!({
            "model": "codex-mini",
            "messages": [
                {"role": "system", "content": "Be helpful"},
                {"role": "user", "content": "Hello"},
                {"role": "assistant", "content": "Hi there!"},
                {"role": "user", "content": "How are you?"}
            ]
        });
        let result = translate_request(&chat, &ResponsesDialect::codex()).unwrap();
        assert_eq!(result["instructions"], "Be helpful");
        let input = result["input"].as_array().unwrap();
        assert_eq!(input.len(), 3);
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[0]["content"], "Hello");
        assert_eq!(input[1]["role"], "assistant");
        assert_eq!(input[1]["content"], "Hi there!");
        assert_eq!(input[2]["role"], "user");
        assert_eq!(input[2]["content"], "How are you?");
    }

    #[test]
    fn translate_passthrough_fields() {
        let chat = json!({
            "model": "codex-mini",
            "messages": [],
            "stream": true,
            "temperature": 0.7,
            "tool_choice": "auto"
        });
        let result = translate_request(&chat, &ResponsesDialect::codex()).unwrap();
        assert_eq!(result["stream"], true);
        assert_eq!(result["temperature"], 0.7);
        assert_eq!(result["tool_choice"], "auto");
    }

    #[test]
    fn translate_tools_unwraps_function_envelope() {
        let chat = json!({
            "model": "codex-mini",
            "messages": [],
            "tools": [
                {
                    "type": "function",
                    "function": {
                        "name": "shell",
                        "description": "Run a shell command",
                        "parameters": {
                            "type": "object",
                            "properties": {"command": {"type": "string"}},
                            "required": ["command"]
                        }
                    }
                },
                {
                    "type": "function",
                    "function": {
                        "name": "read_file",
                        "description": "Read a file",
                        "parameters": {"type": "object", "properties": {"path": {"type": "string"}}}
                    }
                }
            ]
        });
        let result = translate_request(&chat, &ResponsesDialect::codex()).unwrap();
        let tools = result["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);

        // First tool: flattened from envelope.
        assert_eq!(tools[0]["type"], "function");
        assert_eq!(tools[0]["name"], "shell");
        assert_eq!(tools[0]["description"], "Run a shell command");
        assert!(tools[0]["parameters"]["properties"]["command"].is_object());
        // Must NOT have nested "function" key.
        assert!(
            tools[0].get("function").is_none(),
            "tools must be unwrapped from function envelope"
        );

        // Second tool.
        assert_eq!(tools[1]["name"], "read_file");
        assert!(tools[1].get("function").is_none());
    }

    #[test]
    fn translate_tools_preserves_strict_schema_flag() {
        let chat = json!({
            "model": "codex-mini",
            "messages": [],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "skill",
                    "description": "Load a skill by name",
                    "parameters": {
                        "type": "object",
                        "properties": {"name": {"type": "string"}},
                        "required": ["name"],
                        "additionalProperties": false
                    },
                    "strict": true
                }
            }]
        });

        let result = translate_request(&chat, &ResponsesDialect::codex()).unwrap();
        let tools = result["tools"].as_array().unwrap();

        assert_eq!(tools[0]["name"], "skill");
        assert_eq!(tools[0]["strict"], true);
        assert_eq!(tools[0]["parameters"]["required"], json!(["name"]));
    }

    #[test]
    fn translate_tool_no_auto_detect_strict_without_explicit_flag() {
        // After schema sanitization, the schema may look strict but we should
        // NOT auto-detect strict mode because the schema has been modified.
        // Only explicit `strict: true` from the client enables strict mode.
        let chat = json!({
            "model": "codex-mini",
            "messages": [],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "Read",
                    "description": "Read a file",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "filePath": {"type": "string", "description": "Path to read"}
                        },
                        "required": ["filePath"],
                        "additionalProperties": false
                    }
                }
            }]
        });
        let result = translate_request(&chat, &ResponsesDialect::codex()).unwrap();
        let tools = result["tools"].as_array().unwrap();
        assert_eq!(
            tools[0]["strict"], false,
            "strict should be false without explicit flag"
        );
    }

    #[test]
    fn translate_tool_no_strict_when_schema_is_loose() {
        // Schema without additionalProperties:false → should not be strict.
        let chat = json!({
            "model": "codex-mini",
            "messages": [],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "search",
                    "description": "Search",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "query": {"type": "string"}
                        },
                        "required": ["query"]
                    }
                }
            }]
        });
        let result = translate_request(&chat, &ResponsesDialect::codex()).unwrap();
        let tools = result["tools"].as_array().unwrap();
        assert_eq!(
            tools[0]["strict"], false,
            "should not be strict for loose schema"
        );
    }

    #[test]
    fn translate_tool_explicit_strict_overrides_auto_detect() {
        // Even if schema looks strict, explicit strict:false should win.
        let chat = json!({
            "model": "codex-mini",
            "messages": [],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "test",
                    "parameters": {
                        "type": "object",
                        "properties": {"x": {"type": "string"}},
                        "required": ["x"],
                        "additionalProperties": false
                    },
                    "strict": false
                }
            }]
        });
        let result = translate_request(&chat, &ResponsesDialect::codex()).unwrap();
        let tools = result["tools"].as_array().unwrap();
        assert_eq!(
            tools[0]["strict"], false,
            "explicit strict:false must override auto-detect"
        );
    }

    #[test]
    fn translate_tool_call_messages_to_responses_function_items() {
        let chat = json!({
            "model": "codex-mini",
            "messages": [
                {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_123",
                        "type": "function",
                        "function": {"name": "initial_instructions", "arguments": "{}"}
                    }]
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_123",
                    "content": "Instructions loaded"
                },
                {"role": "user", "content": "Continue"}
            ]
        });

        let result = translate_request(&chat, &ResponsesDialect::codex()).unwrap();
        let input = result["input"].as_array().unwrap();

        assert_eq!(input[0]["type"], "function_call");
        assert_eq!(input[0]["call_id"], "call_123");
        assert_eq!(input[0]["name"], "initial_instructions");
        assert_eq!(input[0]["arguments"], "{}");
        assert_eq!(input[1]["type"], "function_call_output");
        assert_eq!(input[1]["call_id"], "call_123");
        assert_eq!(input[1]["output"], "Instructions loaded");
        assert_eq!(input[2]["type"], "message");
        assert_eq!(input[2]["role"], "user");
    }

    // -- Task 4: Response translation tests --

    // SSE translation is now handled by ResponsesSseTranslator —
    // see integration tests in crates/proxy/tests/codex.rs for coverage.

    #[test]
    fn buffered_simple_text_response() {
        let responses = json!({
            "id": "resp_test",
            "model": "codex-mini",
            "output": [
                {
                    "type": "message",
                    "content": [{"type": "output_text", "text": "Hello world"}]
                }
            ],
            "usage": {"input_tokens": 5, "output_tokens": 10, "total_tokens": 15}
        });
        let result = translate_buffered_response(&responses).unwrap();
        assert_eq!(result["id"], "resp_test");
        assert_eq!(result["object"], "chat.completion");
        assert_eq!(result["choices"][0]["message"]["role"], "assistant");
        assert_eq!(result["choices"][0]["message"]["content"], "Hello world");
        assert_eq!(result["choices"][0]["finish_reason"], "stop");
        assert_eq!(result["model"], "codex-mini");
        assert_eq!(result["usage"]["prompt_tokens"], 5);
        assert_eq!(result["usage"]["completion_tokens"], 10);
        assert_eq!(result["usage"]["total_tokens"], 15);
    }

    #[test]
    fn buffered_tool_call_response() {
        let responses = json!({
            "model": "codex-mini",
            "output": [
                {
                    "type": "function_call",
                    "call_id": "call_abc",
                    "name": "get_weather",
                    "arguments": "{\"city\":\"SF\"}"
                }
            ]
        });
        let result = translate_buffered_response(&responses).unwrap();
        assert_eq!(result["choices"][0]["finish_reason"], "tool_calls");
        let tc = &result["choices"][0]["message"]["tool_calls"][0];
        assert_eq!(tc["id"], "call_abc");
        assert_eq!(tc["function"]["name"], "get_weather");
        assert_eq!(tc["function"]["arguments"], "{\"city\":\"SF\"}");
    }

    // -- SSE translator tests (streaming) --

    use futures::stream::{self, StreamExt};

    /// Helper: feed SSE events through the translator and collect all output chunks.
    fn translate_sse_chunks(events: &[&str]) -> Vec<String> {
        let chunks: Vec<Result<Bytes, Box<dyn std::error::Error + Send + Sync>>> = events
            .iter()
            .map(|e| Ok(Bytes::from(e.to_string())))
            .collect();
        let inner: BoxedByteStream = Box::pin(stream::iter(chunks));
        let mut translator = ResponsesSseTranslator::new(inner);
        let mut output = Vec::new();
        while let Some(item) = futures::executor::block_on(translator.next()) {
            if let Ok(bytes) = item {
                output.push(String::from_utf8_lossy(&bytes).to_string());
            }
        }
        output
    }

    #[test]
    fn sse_fallback_extracts_text_from_response_completed_when_no_deltas() {
        // Simulate the Codex backend returning content only in the
        // response.completed event (no response.output_text.delta events).
        let events = &[
            "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_123\",\"model\":\"gpt-5.5\"}}\n\n",
            "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_123\",\"model\":\"gpt-5.5\",\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"Hello from completed fallback!\"}]}],\"usage\":{\"input_tokens\":10,\"output_tokens\":5,\"total_tokens\":15}}}\n\n",
        ];
        let output = translate_sse_chunks(events);
        let combined = output.join("");

        // Should contain the text extracted from response.completed
        assert!(
            combined.contains("Hello from completed fallback!"),
            "should extract text from response.completed when no deltas, got: {combined}"
        );
        // Should contain usage
        assert!(
            combined.contains("\"prompt_tokens\":10"),
            "should contain usage from completed, got: {combined}"
        );
        // Should end with [DONE]
        assert!(
            combined.contains("[DONE]"),
            "should end with [DONE], got: {combined}"
        );
    }

    #[test]
    fn sse_handles_response_text_delta_alias() {
        // Some Codex backends use "response.text.delta" instead of
        // "response.output_text.delta".
        let events = &[
            "event: response.text.delta\ndata: {\"type\":\"response.text.delta\",\"delta\":\"Hello!\"}\n\n",
            "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-5.5\"}}\n\n",
        ];
        let output = translate_sse_chunks(events);
        let combined = output.join("");

        assert!(
            combined.contains("\"content\":\"Hello!\""),
            "response.text.delta should be translated like response.output_text.delta, got: {combined}"
        );
    }

    #[test]
    fn sse_normal_deltas_still_work() {
        // Verify the existing delta path still works after refactor.
        let events = &[
            "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"Hi \"}\n\n",
            "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"there!\"}\n\n",
            "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-5.4\",\"usage\":{\"input_tokens\":5,\"output_tokens\":3,\"total_tokens\":8}}}\n\n",
        ];
        let output = translate_sse_chunks(events);
        let combined = output.join("");

        assert!(
            combined.contains("\"content\":\"Hi \""),
            "first delta should appear, got: {combined}"
        );
        assert!(
            combined.contains("\"content\":\"there!\""),
            "second delta should appear, got: {combined}"
        );
        assert!(
            combined.contains("\"prompt_tokens\":5"),
            "usage should appear, got: {combined}"
        );
        assert!(
            combined.contains("[DONE]"),
            "should end with [DONE], got: {combined}"
        );
    }

    #[test]
    fn sse_completed_fallback_does_not_double_emit_when_deltas_exist() {
        // If deltas were received, response.completed should NOT re-emit text.
        let events = &[
            "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"Only once\"}\n\n",
            "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-5.5\",\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"Only once\"}]}]}}\n\n",
        ];
        let output = translate_sse_chunks(events);
        let combined = output.join("");

        // Count occurrences of the content string
        let count = combined.matches("\"content\":\"Only once\"").count();
        assert_eq!(
            count, 1,
            "text should appear exactly once (not doubled), but found {count} times in: {combined}"
        );
    }

    #[test]
    fn sse_translates_function_call_item_metadata_and_finishes_with_tool_calls() {
        // OpenCode receives Codex tool calls via Responses API output item events.
        // Without forwarding the tool call name and a tool_calls finish reason,
        // the AI SDK sees an empty assistant turn even though the model emitted
        // a valid function call.
        let events = &[
            "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_tool\",\"model\":\"gpt-5.5\"}}\n\n",
            "event: response.output_item.added\ndata: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"function_call\",\"call_id\":\"call_123\",\"name\":\"initial_instructions\",\"arguments\":\"\"}}\n\n",
            "event: response.function_call_arguments.delta\ndata: {\"type\":\"response.function_call_arguments.delta\",\"call_id\":\"call_123\",\"delta\":\"{}\"}\n\n",
            "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-5.5\",\"usage\":{\"input_tokens\":10,\"output_tokens\":5,\"total_tokens\":15}}}\n\n",
        ];
        let output = translate_sse_chunks(events);
        let combined = output.join("");

        assert!(
            combined.contains("\"tool_calls\":[{\"function\":{\"arguments\":\"\",\"name\":\"initial_instructions\"},\"id\":\"call_123\",\"index\":0,\"type\":\"function\"}]"),
            "tool call metadata must include id, type, function name, and empty arguments, got: {combined}"
        );
        assert!(
            combined.contains("\"arguments\":\"{}\""),
            "tool call argument delta should be forwarded, got: {combined}"
        );
        assert!(
            combined.contains("\"finish_reason\":\"tool_calls\""),
            "tool call streams must finish with finish_reason=tool_calls, got: {combined}"
        );
    }

    #[test]
    fn sse_waits_for_function_call_name_when_added_event_has_only_type() {
        let events = &[
            "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_tool\",\"model\":\"gpt-5.5\"}}\n\n",
            "event: response.output_item.added\ndata: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"function_call\"}}\n\n",
            "event: response.output_item.done\ndata: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"call_id\":\"call_123\",\"name\":\"initial_instructions\",\"arguments\":\"{\\\"name\\\":\\\"x\\\"}\"}}\n\n",
            "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-5.5\"}}\n\n",
        ];
        let output = translate_sse_chunks(events);
        let combined = output.join("");

        assert!(
            combined.contains("\"name\":\"initial_instructions\""),
            "tool name from output_item.done should be emitted, got: {combined}"
        );
        assert!(
            combined.contains("\"arguments\":\"{\\\"name\\\":\\\"x\\\"}\""),
            "full tool arguments from output_item.done should be emitted, got: {combined}"
        );
        assert!(
            !combined.contains("\"name\":\"\""),
            "must not emit nameless tool-call start chunk, got: {combined}"
        );
    }

    #[test]
    fn sse_uses_item_id_to_forward_streamed_function_call_arguments() {
        // Responses API argument deltas identify the function call by item_id,
        // not call_id. The preceding output_item.added event contains both ids;
        // the translator must map item_id back to call_id so Chat Completions
        // clients receive the required JSON arguments instead of "" / {}.
        let events = &[
            "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_tool\",\"model\":\"gpt-5.5\"}}\n\n",
            "event: response.output_item.added\ndata: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"function_call\",\"id\":\"fc_123\",\"call_id\":\"call_123\",\"name\":\"skill\",\"arguments\":\"\"}}\n\n",
            "event: response.function_call_arguments.delta\ndata: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_123\",\"output_index\":0,\"delta\":\"{\\\"\"}\n\n",
            "event: response.function_call_arguments.delta\ndata: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_123\",\"output_index\":0,\"delta\":\"name\\\":\\\"systematic-debugging\\\"}\"}\n\n",
            "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-5.5\"}}\n\n",
        ];
        let output = translate_sse_chunks(events);
        let combined = output.join("");

        assert!(
            combined.contains("\"name\":\"skill\""),
            "tool call name should be emitted, got: {combined}"
        );
        assert!(
            combined.contains("\"arguments\":\"{\\\"\""),
            "first item_id-keyed argument delta should be forwarded using call_id, got: {combined}"
        );
        assert!(
            combined.contains("\"arguments\":\"name\\\":\\\"systematic-debugging\\\"}\""),
            "second item_id-keyed argument delta should be forwarded using call_id, got: {combined}"
        );
    }
}
