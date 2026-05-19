//! Codex provider — implements `Provider` against OpenAI's Codex Responses API
//! at `https://chatgpt.com/backend-api/codex/responses`.
//!
//! Translates Chat Completions requests into the OpenAI Responses API format
//! and translates responses (both streaming and buffered) back.

use super::messages_protocol::{self, AuthHeader, HOP_BY_HOP};
use crate::application::errors::ProxyError;
use crate::application::ports::{ApiFormat, Provider, UpstreamResponse, UsageParser};
use crate::domain::UsageRecord;
use async_trait::async_trait;
use axum::http::HeaderMap;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use serde_json::{Value, json};
use std::pin::Pin;
use std::task::{Context, Poll};

const DEFAULT_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";

// ---------------------------------------------------------------------------
// Struct & constructors
// ---------------------------------------------------------------------------

pub struct CodexProvider {
    base_url: String,
    http: reqwest::Client,
    auth: AuthHeader,
}

impl CodexProvider {
    pub fn new(http: reqwest::Client) -> Self {
        Self::build(http, DEFAULT_BASE_URL.into(), AuthHeader::Passthrough)
    }

    pub fn with_base_url(http: reqwest::Client, base_url: impl Into<String>) -> Self {
        Self::build(http, base_url.into(), AuthHeader::Passthrough)
    }

    pub fn with_auth(http: reqwest::Client, auth: AuthHeader) -> Self {
        Self::build(http, DEFAULT_BASE_URL.into(), auth)
    }

    pub fn configure(
        http: reqwest::Client,
        base_url: Option<String>,
        auth: AuthHeader,
    ) -> Self {
        Self::build(
            http,
            base_url.unwrap_or_else(|| DEFAULT_BASE_URL.into()),
            auth,
        )
    }

    fn build(http: reqwest::Client, base_url: String, auth: AuthHeader) -> Self {
        Self {
            base_url,
            http,
            auth,
        }
    }
}

// ---------------------------------------------------------------------------
// Provider trait impl
// ---------------------------------------------------------------------------

#[async_trait]
impl Provider for CodexProvider {
    fn name(&self) -> &'static str {
        "codex"
    }

    fn native_format(&self) -> ApiFormat {
        ApiFormat::OpenAI
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
        // Parse incoming Chat Completions body
        let chat_body: Value = serde_json::from_slice(&body).map_err(|e| {
            ProxyError::BadRequest(format!("invalid JSON body: {e}"))
        })?;

        // Translate to Responses API payload (always sets stream:true)
        let responses_body = translate_request(&chat_body).map_err(|e| {
            ProxyError::BadRequest(format!("codex request translation failed: {e}"))
        })?;

        let url = format!("{}/responses", self.base_url.trim_end_matches('/'));
        let serialized = serde_json::to_vec(&responses_body).map_err(|e| {
            ProxyError::BadRequest(format!("failed to serialize responses body: {e}"))
        })?;

        // Build the upstream request
        let mut req = self.http.post(&url).body(serialized);
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
            let mut usage: Option<Value> = None;
            let mut model = chat_body
                .get("model")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown")
                .to_string();

            let mut stream = translated;
            while let Some(chunk) = stream.next().await {
                let bytes = chunk.map_err(|e| {
                    ProxyError::BadRequest(format!("codex stream error: {e}"))
                })?;
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
                        // Accumulate content deltas
                        if let Some(content) = val
                            .get("choices")
                            .and_then(|c| c.get(0))
                            .and_then(|c| c.get("delta"))
                            .and_then(|d| d.get("content"))
                            .and_then(|c| c.as_str())
                        {
                            collected_content.push_str(content);
                        }
                        // Capture usage from final chunk
                        if let Some(u) = val.get("usage").cloned() {
                            usage = Some(u);
                        }
                    }
                }
            }

            let message = json!({
                "role": "assistant",
                "content": if collected_content.is_empty() {
                    Value::Null
                } else {
                    Value::String(collected_content)
                }
            });
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
                    "finish_reason": "stop"
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
// Request translation: Chat Completions → Responses API
// ---------------------------------------------------------------------------

/// Translate a single Chat Completions tool definition to Responses API format.
///
/// Chat Completions: `{"type":"function","function":{"name":"...","description":"...","parameters":{...}}}`
/// Responses API:    `{"type":"function","name":"...","description":"...","parameters":{...}}`
fn translate_tool(tool: &Value) -> Value {
    let tool_type = tool.get("type").and_then(|t| t.as_str()).unwrap_or("function");

    match tool_type {
        "function" => {
            // Unwrap the nested "function" envelope.
            let func = tool.get("function").cloned().unwrap_or_default();
            let name = func.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let description = func
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("");
            let parameters = func.get("parameters").cloned().unwrap_or(json!({}));

            json!({
                "type": "function",
                "name": name,
                "description": description,
                "parameters": parameters,
            })
        }
        _ => tool.clone(), // Pass through unknown tool types unchanged.
    }
}

fn translate_request(chat: &Value) -> Result<Value, String> {
    let mut out = serde_json::Map::new();

    // model → passthrough
    if let Some(model) = chat.get("model") {
        out.insert("model".into(), model.clone());
    }

    // Extract instructions from system/developer messages
    let mut instructions = Value::String("You are a helpful assistant.".to_string());
    let mut input_messages = Vec::new();

    if let Some(messages) = chat.get("messages").and_then(|m| m.as_array()) {
        for msg in messages {
            let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("user");

            if role == "system" || role == "developer" {
                let content = msg
                    .get("content")
                    .map(|c| c.as_str().unwrap_or("").to_string())
                    .unwrap_or_default();
                instructions = Value::String(content);
                continue;
            }

            // Codex backend accepts plain string content for simple text messages.
            let content = msg
                .get("content")
                .map(|c| c.as_str().unwrap_or("").to_string())
                .unwrap_or_default();

            input_messages.push(json!({
                "type": "message",
                "role": role,
                "content": content
            }));
        }
    }

    out.insert("instructions".into(), instructions);
    out.insert("input".into(), Value::Array(input_messages));

    // NOTE: max_output_tokens is NOT sent because the Codex backend
    // rejects it with "Unsupported parameter: max_output_tokens".

    // reasoning_effort → reasoning.effort
    if let Some(effort) = chat.get("reasoning_effort") {
        out.insert("reasoning".into(), json!({ "effort": effort }));
    }

    // Required fields
    out.insert("store".into(), json!(false));
    out.insert("include".into(), json!(["reasoning.encrypted_content"]));

    // Codex backend requires stream=true; always set it.
    out.insert("stream".into(), json!(true));

    // Translate tools: Chat Completions wraps in {"type":"function","function":{...}}
    // but the Responses API expects {"type":"function","name":"...","parameters":{...}}
    // (flat structure without the nested "function" envelope).
    if let Some(tools) = chat.get("tools").and_then(|t| t.as_array()) {
        let translated_tools: Vec<Value> = tools
            .iter()
            .map(|tool| translate_tool(tool))
            .collect();
        out.insert("tools".into(), Value::Array(translated_tools));
    }
    if let Some(tool_choice) = chat.get("tool_choice") {
        out.insert("tool_choice".into(), tool_choice.clone());
    }
    if let Some(temperature) = chat.get("temperature") {
        out.insert("temperature".into(), temperature.clone());
    }

    Ok(Value::Object(out))
}

// ---------------------------------------------------------------------------
// Buffered response translation: Responses API JSON → Chat Completions JSON
// Used by unit tests and available for future non-SSE Codex backends.
// ---------------------------------------------------------------------------

#[allow(dead_code)]

fn translate_buffered_response(responses_body: &Value) -> Result<Value, String> {
    // Extract output text from the response
    let mut content = String::new();
    let mut tool_calls = Vec::new();
    let mut tc_index: u32 = 0;

    if let Some(output) = responses_body.get("output").and_then(|o| o.as_array()) {
        for item in output {
            let item_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
            match item_type {
                "message" => {
                    if let Some(content_arr) = item.get("content").and_then(|c| c.as_array()) {
                        for c in content_arr {
                            if c.get("type")
                                .and_then(|t| t.as_str())
                                .map_or(false, |t| t == "output_text")
                            {
                                if let Some(text) = c.get("text").and_then(|t| t.as_str()) {
                                    content.push_str(text);
                                }
                            }
                        }
                    }
                }
                "function_call" => {
                    if let (Some(call_id), Some(name), Some(args)) = (
                        item.get("call_id").and_then(|c| c.as_str()),
                        item.get("name").and_then(|n| n.as_str()),
                        item.get("arguments").and_then(|a| a.as_str()),
                    ) {
                        tool_calls.push(json!({
                            "index": tc_index,
                            "id": call_id,
                            "type": "function",
                            "function": {
                                "name": name,
                                "arguments": args
                            }
                        }));
                        tc_index += 1;
                    }
                }
                _ => {}
            }
        }
    }

    let mut message = json!({
        "role": "assistant",
        "content": if content.is_empty() { Value::Null } else { Value::String(content) }
    });
    if !tool_calls.is_empty() {
        message["tool_calls"] = Value::Array(tool_calls);
    }

    let finish_reason = if !message
        .get("tool_calls")
        .map_or(false, |tc| tc.as_array().map_or(false, |a| !a.is_empty()))
    {
        "stop"
    } else {
        "tool_calls"
    };

    let model = responses_body
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or("unknown");

    let id = responses_body
        .get("id")
        .and_then(|i| i.as_str())
        .unwrap_or("");

    let mut result = json!({
        "id": id,
        "object": "chat.completion",
        "model": model,
        "choices": [{
            "index": 0,
            "message": message,
            "finish_reason": finish_reason
        }]
    });

    // Map usage: input_tokens -> prompt_tokens, output_tokens -> completion_tokens
    if let Some(usage) = responses_body.get("usage") {
        result["usage"] = json!({
            "prompt_tokens": usage.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
            "completion_tokens": usage.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
            "total_tokens": usage.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
        });
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// SSE stream translator: wraps a byte stream and translates Responses API
// SSE events into Chat Completions SSE chunks in real time.
// ---------------------------------------------------------------------------

type BoxedByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, Box<dyn std::error::Error + Send + Sync>>> + Send>>;

use std::collections::VecDeque;

struct ResponsesSseTranslator {
    inner: BoxedByteStream,
    buffer: String,
    pending: VecDeque<String>,
    // State captured from response.created event and injected into all chunks.
    response_id: String,
    model: String,
    // Track whether the first content chunk was emitted (to inject role).
    first_content_sent: bool,
}

impl ResponsesSseTranslator {
    fn new(inner: BoxedByteStream) -> Self {
        Self {
            inner,
            buffer: String::new(),
            pending: VecDeque::new(),
            response_id: String::new(),
            model: String::new(),
            first_content_sent: false,
        }
    }

    /// Build a base Chat Completions SSE chunk with the required envelope fields.
    fn base_chunk(&self) -> Value {
        json!({
            "id": if self.response_id.is_empty() { "chatcmpl-codex" } else { &self.response_id },
            "object": "chat.completion.chunk",
            "created": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            "model": if self.model.is_empty() { "unknown" } else { &self.model },
        })
    }

    /// Translate a complete SSE event from the buffer into Chat Completions chunks.
    fn translate_event(&mut self, raw_event: &str) {
        for line in raw_event.lines() {
            let line = line.trim();
            if !line.starts_with("data: ") {
                continue;
            }
            let data = &line[6..];

            if data == "[DONE]" {
                self.pending.push_back("data: [DONE]\n\n".to_string());
                continue;
            }

            let event: Value = match serde_json::from_str(data) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let event_type = event.get("type").and_then(|t| t.as_str()).unwrap_or("");

            match event_type {
                "response.created" | "response.in_progress" => {
                    // Capture response_id and model from the initial events.
                    if let Some(resp) = event.get("response") {
                        if let Some(id) = resp.get("id").and_then(|v| v.as_str()) {
                            self.response_id = id.to_string();
                        }
                        if let Some(m) = resp.get("model").and_then(|v| v.as_str()) {
                            self.model = m.to_string();
                        }
                    }
                }
                "response.output_text.delta" => {
                    if let Some(delta) = event.get("delta").and_then(|d| d.as_str()) {
                        // Emit a separate role-only chunk first (standard OpenAI format).
                        if !self.first_content_sent {
                            let mut role_chunk = self.base_chunk();
                            role_chunk["choices"] = json!([{
                                "index": 0,
                                "delta": {"role": "assistant"},
                                "finish_reason": null
                            }]);
                            self.pending.push_back(format!("data: {}\n\n", role_chunk));
                            self.first_content_sent = true;
                        }
                        let mut chunk = self.base_chunk();
                        chunk["choices"] = json!([{
                            "index": 0,
                            "delta": {"content": delta},
                            "finish_reason": null
                        }]);
                        self.pending.push_back(format!("data: {}\n\n", chunk));
                    }
                }
                "response.function_call_arguments.delta" => {
                    if let (Some(call_id), Some(args_delta)) = (
                        event.get("call_id").and_then(|c| c.as_str()),
                        event.get("delta").and_then(|d| d.as_str()),
                    ) {
                        let mut chunk = self.base_chunk();
                        chunk["choices"] = json!([{
                            "index": 0,
                            "delta": {
                                "tool_calls": [{
                                    "index": 0,
                                    "id": call_id,
                                    "type": "function",
                                    "function": {
                                        "arguments": args_delta
                                    }
                                }]
                            },
                            "finish_reason": null
                        }]);
                        self.pending.push_back(format!("data: {}\n\n", chunk));
                    }
                }
                "response.completed" => {
                    let mut chunk = self.base_chunk();
                    chunk["choices"] = json!([{
                        "index": 0,
                        "delta": {},
                        "finish_reason": "stop"
                    }]);
                    // Also capture model from completed event in case created didn't have it.
                    if let Some(resp) = event.get("response") {
                        if let Some(m) = resp.get("model").and_then(|v| v.as_str()) {
                            self.model = m.to_string();
                        }
                        if let Some(usage) = resp.get("usage") {
                            chunk["usage"] = json!({
                                "prompt_tokens": usage.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                                "completion_tokens": usage.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                                "total_tokens": usage.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                            });
                        }
                    }
                    // Re-build with updated model.
                    chunk["id"] = json!(if self.response_id.is_empty() { "chatcmpl-codex" } else { &self.response_id });
                    chunk["model"] = json!(if self.model.is_empty() { "unknown" } else { &self.model });
                    self.pending.push_back(format!("data: {}\n\n", chunk));
                    // The Codex backend may not always send response.done, so emit
                    // [DONE] here as well to ensure the stream terminates properly.
                    self.pending.push_back("data: [DONE]\n\n".to_string());
                }
                "response.done" => {
                    self.pending.push_back("data: [DONE]\n\n".to_string());
                }
                _ => {
                    // Skip all other event types (metadata, reasoning, etc.)
                }
            }
        }
    }
}

impl Stream for ResponsesSseTranslator {
    type Item = Result<Bytes, Box<dyn std::error::Error + Send + Sync>>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            // Return any pending translated chunks first
            if let Some(chunk) = self.pending.pop_front() {
                return Poll::Ready(Some(Ok(Bytes::from(chunk))));
            }

            // Poll inner stream for more bytes
            match self.inner.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(bytes))) => {
                    self.buffer.push_str(&String::from_utf8_lossy(&bytes));

                    // Extract complete SSE events (delimited by double newline)
                    while let Some(pos) = self.buffer.find("\n\n") {
                        let event = self.buffer[..pos].to_string();
                        self.buffer = self.buffer[pos + 2..].to_string();

                        self.translate_event(&event);
                    }
                    // Loop back to check pending
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Some(Err(e)));
                }
                Poll::Ready(None) => {
                    // If there's remaining data in the buffer, try to process it
                    if !self.buffer.trim().is_empty() {
                        let event = std::mem::take(&mut self.buffer);
                        self.translate_event(&event);
                        if let Some(chunk) = self.pending.pop_front() {
                            return Poll::Ready(Some(Ok(Bytes::from(chunk))));
                        }
                    }
                    return Poll::Ready(None);
                }
                Poll::Pending => {
                    return Poll::Pending;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Task 2: Construction tests --

    #[test]
    fn name_is_codex() {
        let p = CodexProvider::new(reqwest::Client::new());
        assert_eq!(p.name(), "codex");
    }

    #[test]
    fn native_format_is_openai() {
        let p = CodexProvider::new(reqwest::Client::new());
        assert_eq!(p.native_format(), ApiFormat::OpenAI);
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
        let result = translate_request(&chat).unwrap();
        assert_eq!(result["model"], "codex-mini");
        let input = result["input"].as_array().unwrap();
        assert_eq!(input.len(), 1);
        assert_eq!(input[0]["role"], "user");
        // Content is a plain string for the Codex backend.
        assert_eq!(input[0]["content"], "Hello");
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
        let result = translate_request(&chat).unwrap();
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
        let result = translate_request(&chat).unwrap();
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
        let result = translate_request(&chat).unwrap();
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
        let result = translate_request(&chat).unwrap();
        assert_eq!(result["reasoning"]["effort"], "high");
    }

    #[test]
    fn translate_includes_required_fields() {
        let chat = json!({
            "model": "codex-mini",
            "messages": [{"role": "user", "content": "Hello"}]
        });
        let result = translate_request(&chat).unwrap();
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
        let result = translate_request(&chat).unwrap();
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
        let result = translate_request(&chat).unwrap();
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
        let result = translate_request(&chat).unwrap();
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
        let result = translate_request(&chat).unwrap();
        let tools = result["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);

        // First tool: flattened from envelope.
        assert_eq!(tools[0]["type"], "function");
        assert_eq!(tools[0]["name"], "shell");
        assert_eq!(tools[0]["description"], "Run a shell command");
        assert!(tools[0]["parameters"]["properties"]["command"].is_object());
        // Must NOT have nested "function" key.
        assert!(tools[0].get("function").is_none(), "tools must be unwrapped from function envelope");

        // Second tool.
        assert_eq!(tools[1]["name"], "read_file");
        assert!(tools[1].get("function").is_none());
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
}
