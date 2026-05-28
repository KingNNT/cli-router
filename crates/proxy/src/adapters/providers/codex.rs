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
    default_reasoning_effort: Option<String>,
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
        Self::configure_with_reasoning_effort(http, base_url, auth, None)
    }

    pub fn configure_with_reasoning_effort(
        http: reqwest::Client,
        base_url: Option<String>,
        auth: AuthHeader,
        default_reasoning_effort: Option<String>,
    ) -> Self {
        Self::build(
            http,
            base_url.unwrap_or_else(|| DEFAULT_BASE_URL.into()),
            auth,
            default_reasoning_effort,
        )
    }

    fn build(
        http: reqwest::Client,
        base_url: String,
        auth: AuthHeader,
        default_reasoning_effort: Option<String>,
    ) -> Self {
        Self {
            base_url,
            http,
            auth,
            default_reasoning_effort,
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
        let chat_body: Value = serde_json::from_slice(&body)
            .map_err(|e| ProxyError::BadRequest(format!("invalid JSON body: {e}")))?;

        // Translate to Responses API payload (always sets stream:true)
        let responses_body = translate_request_with_default_reasoning_effort(
            &chat_body,
            self.default_reasoning_effort.as_deref(),
        )
        .map_err(|e| ProxyError::BadRequest(format!("codex request translation failed: {e}")))?;

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
                                let idx = tc
                                    .get("index")
                                    .and_then(|i| i.as_u64())
                                    .unwrap_or(0) as usize;
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
                                if let Some(id) =
                                    tc.get("id").and_then(|v| v.as_str())
                                {
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
// Request translation: Chat Completions → Responses API
// ---------------------------------------------------------------------------

/// Translate a single Chat Completions tool definition to Responses API format.
///
/// Chat Completions: `{"type":"function","function":{"name":"...","description":"...","parameters":{...}}}`
/// Responses API:    `{"type":"function","name":"...","description":"...","parameters":{...}}`
fn translate_tool(tool: &Value) -> Value {
    let tool_type = tool
        .get("type")
        .and_then(|t| t.as_str())
        .unwrap_or("function");

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

            // Use explicit `strict` if the client set it AND it is true.
            // When strict is true, the Responses API validates that tool-call
            // arguments match the schema exactly.  This only works when the
            // schema is the ORIGINAL schema — not after we've sanitised it
            // (stripping keywords, collapsing additionalProperties, etc.).
            //
            // When translating from Anthropic format, the schema has been
            // through strip_schema_keywords() which may have changed it
            // significantly (e.g. collapsing additionalProperties objects to
            // false, stripping propertyNames, synthesising required arrays).
            // In that case strict mode will REJECT the call because Codex
            // can't produce arguments matching a schema it never saw the
            // original form of.
            //
            // Rule: only enable strict when the client explicitly set it to
            // true (i.e. the schema is untouched, coming from a native OpenAI
            // client like OpenCode).  Never auto-detect after translation.
            let strict = func.get("strict").and_then(|s| s.as_bool()).unwrap_or(false);

            json!({
                "type": "function",
                "name": name,
                "description": description,
                "parameters": parameters,
                "strict": strict,
            })
        }
        _ => tool.clone(), // Pass through unknown tool types unchanged.
    }
}


fn text_content_to_string(content: Option<&Value>) -> String {
    let Some(content) = content else {
        return String::new();
    };

    if let Some(text) = content.as_str() {
        return text.to_string();
    }

    if let Some(parts) = content.as_array() {
        return parts
            .iter()
            .filter_map(|part| {
                let part_type = part.get("type").and_then(|v| v.as_str());
                if part_type == Some("text") || part_type == Some("input_text") {
                    part.get("text").and_then(|v| v.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("");
    }

    String::new()
}

#[cfg(test)]
fn translate_request(chat: &Value) -> Result<Value, String> {
    translate_request_with_default_reasoning_effort(chat, None)
}

fn translate_request_with_default_reasoning_effort(
    chat: &Value,
    default_reasoning_effort: Option<&str>,
) -> Result<Value, String> {
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
                let content = text_content_to_string(msg.get("content"));
                instructions = Value::String(content);
                continue;
            }

            if role == "tool" {
                let output = text_content_to_string(msg.get("content"));
                input_messages.push(json!({
                    "type": "function_call_output",
                    "call_id": msg
                        .get("tool_call_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or(""),
                    "output": output
                }));
                continue;
            }

            if role == "assistant"
                && let Some(tool_calls) = msg.get("tool_calls").and_then(|v| v.as_array())
            {
                for tool_call in tool_calls {
                    let function = tool_call.get("function").unwrap_or(&Value::Null);
                    input_messages.push(json!({
                        "type": "function_call",
                        "call_id": tool_call
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or(""),
                        "name": function
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or(""),
                        "arguments": function
                            .get("arguments")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                    }));
                }

                let has_text_content = msg
                    .get("content")
                    .and_then(|c| c.as_str())
                    .map(|s| !s.is_empty())
                    .unwrap_or(false);
                if !has_text_content {
                    continue;
                }
            }

            // Codex backend accepts plain string content for simple text messages.
            let content = text_content_to_string(msg.get("content"));

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
    } else if let Some(effort) = default_reasoning_effort {
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
        let translated_tools: Vec<Value> = tools.iter().map(translate_tool).collect();
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
                            if c.get("type").and_then(|t| t.as_str()) == Some("output_text")
                                && let Some(text) = c.get("text").and_then(|t| t.as_str())
                            {
                                content.push_str(text);
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

    let finish_reason = if message
        .get("tool_calls")
        .is_none_or(|tc| tc.as_array().is_none_or(|a| a.is_empty()))
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
        let input_tokens = usage.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        if input_tokens == 0 {
            tracing::warn!(
                target: "codex::usage",
                usage = %usage,
                "codex buffered response reported zero input tokens"
            );
        }
        result["usage"] = json!({
            "prompt_tokens": input_tokens,
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

type BoxedByteStream =
    Pin<Box<dyn Stream<Item = Result<Bytes, Box<dyn std::error::Error + Send + Sync>>> + Send>>;

use std::collections::{HashMap, VecDeque};

struct ResponsesSseTranslator {
    inner: BoxedByteStream,
    buffer: String,
    pending: VecDeque<String>,
    // State captured from response.created event and injected into all chunks.
    response_id: String,
    model: String,
    // Track whether the first content chunk was emitted (to inject role).
    first_content_sent: bool,
    // Track whether any tool-call delta was emitted. Codex can answer OpenCode
    // requests by calling a tool without emitting text; those streams must end
    // with finish_reason=tool_calls so AI SDK clients execute the tool instead
    // of treating the assistant turn as silent.
    tool_call_started: bool,
    // Responses argument deltas are keyed by output item id, while Chat
    // Completions chunks must use call_id. Keep the mapping from
    // response.output_item.added/done so later argument deltas can be emitted
    // with the correct Chat Completions tool_call id.
    tool_call_item_to_call_id: HashMap<String, String>,
    deferred_tool_call_id: String,
    deferred_tool_arguments: String,
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
            tool_call_started: false,
            tool_call_item_to_call_id: HashMap::new(),
            deferred_tool_call_id: String::new(),
            deferred_tool_arguments: String::new(),
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

            // Debug-log every non-trivial event so silent/empty responses are
            // visible in proxy logs without external packet capture.
            tracing::debug!(
                target: "codex::sse",
                event_type = %event_type,
                "codex upstream SSE event"
            );

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
                // Primary text delta event (Responses API standard).
                "response.output_text.delta" |
                // Fallback: some Codex backends / versions use this name instead.
                "response.text.delta" => {
                    if let Some(delta) = event.get("delta").and_then(|d| d.as_str()) {
                        self.emit_text_delta(delta);
                    }
                }
                "response.function_call_arguments.delta" => {
                    if let Some(args_delta) = event.get("delta").and_then(|d| d.as_str()) {
                        let call_id = event
                            .get("call_id")
                            .and_then(|c| c.as_str())
                            .map(str::to_string)
                            .or_else(|| {
                                event
                                    .get("item_id")
                                    .and_then(|i| i.as_str())
                                    .and_then(|item_id| self.tool_call_item_to_call_id.get(item_id))
                                    .cloned()
                            });

                        if let Some(call_id) = call_id {
                            if !self.tool_call_started {
                                self.deferred_tool_call_id = call_id.clone();
                                self.deferred_tool_arguments.push_str(args_delta);
                                continue;
                            }
                            self.emit_tool_call_arguments(&call_id, args_delta);
                        }
                    }
                }
                "response.output_item.added" | "response.output_item.done" => {
                    if let Some(item) = event.get("item")
                        && item.get("type").and_then(|t| t.as_str()) == Some("function_call")
                    {
                        let call_id = item
                            .get("call_id")
                            .or_else(|| item.get("id"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("call_codex");
                        if let Some(item_id) = item.get("id").and_then(|v| v.as_str()) {
                            self.tool_call_item_to_call_id
                                .insert(item_id.to_string(), call_id.to_string());
                        }

                        if self.tool_call_started {
                            continue;
                        }

                        let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
                        if !name.is_empty() {
                            self.emit_tool_call_start(call_id, name);
                            if let Some(arguments) = item
                                .get("arguments")
                                .and_then(|v| v.as_str())
                                .filter(|s| !s.is_empty())
                            {
                                self.emit_tool_call_arguments(call_id, arguments);
                            } else if !self.deferred_tool_arguments.is_empty() {
                                let args = std::mem::take(&mut self.deferred_tool_arguments);
                                self.emit_tool_call_arguments(call_id, &args);
                            }
                        }
                    }
                }
                "response.completed" => {
                    // Also capture model from completed event in case created didn't have it.
                    if let Some(resp) = event.get("response") {
                        if let Some(m) = resp.get("model").and_then(|v| v.as_str()) {
                            self.model = m.to_string();
                        }

                        // Fallback: if no text deltas were streamed, extract the
                        // full text from the completed response's output array.
                        // The Codex backend can return 200 OK with content only
                        // in the completed event (no individual deltas), especially
                        // with reasoning models like GPT-5.5.
                        if !self.first_content_sent
                            && let Some(output) = resp.get("output").and_then(|o| o.as_array())
                        {
                                let mut collected = String::new();
                                for item in output {
                                    let item_type = item
                                        .get("type")
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("");
                                    match item_type {
                                        "message" => {
                                            if let Some(content) =
                                                item.get("content").and_then(|c| c.as_array())
                                            {
                                                for c in content {
                                                    if c.get("type").and_then(|t| t.as_str())
                                                        == Some("output_text")
                                                        && let Some(text) =
                                                            c.get("text").and_then(|t| t.as_str())
                                                    {
                                                        collected.push_str(text);
                                                    }
                                                }
                                            }
                                        }
                                        "function_call" => {
                                            // Tool calls in completed output are handled
                                            // by the function_call_arguments.delta path.
                                        }
                                        _ => {}
                                    }
                                }
                                if !collected.is_empty() {
                                    tracing::debug!(
                                        target: "codex::sse",
                                        len = collected.len(),
                                        "extracted text from response.completed fallback (no deltas received)"
                                    );
                                    self.emit_text_delta(&collected);
                                }
                        }

                        let mut chunk = self.base_chunk();
                        chunk["choices"] = json!([{
                            "index": 0,
                            "delta": {},
                            "finish_reason": if self.tool_call_started { "tool_calls" } else { "stop" }
                        }]);
                        if let Some(usage) = resp.get("usage") {
                            let input_tokens = usage.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                            if input_tokens == 0 {
                                tracing::warn!(
                                    target: "codex::usage",
                                    usage = %usage,
                                    "codex completed response reported zero input tokens"
                                );
                            }
                            chunk["usage"] = json!({
                                "prompt_tokens": input_tokens,
                                "completion_tokens": usage.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                                "total_tokens": usage.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                            });
                        }
                        // Re-build with updated model.
                        chunk["id"] = json!(if self.response_id.is_empty() { "chatcmpl-codex" } else { &self.response_id });
                        chunk["model"] = json!(if self.model.is_empty() { "unknown" } else { &self.model });
                        self.pending.push_back(format!("data: {}\n\n", chunk));
                    } else {
                        // No response object in completed event — emit bare stop chunk.
                        let mut chunk = self.base_chunk();
                        chunk["choices"] = json!([{
                            "index": 0,
                            "delta": {},
                            "finish_reason": if self.tool_call_started { "tool_calls" } else { "stop" }
                        }]);
                        self.pending.push_back(format!("data: {}\n\n", chunk));
                    }
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

    /// Emit a text content delta as a Chat Completions SSE chunk. Inserts the
    /// standard `{"role":"assistant"}` preamble on the first call.
    fn emit_text_delta(&mut self, delta: &str) {
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

    /// Emit the initial OpenAI Chat Completions tool-call delta containing the
    /// call id, type, function name, and an empty arguments string. Later
    /// `response.function_call_arguments.delta` events append the arguments.
    fn emit_tool_call_start(&mut self, call_id: &str, name: &str) {
        let mut chunk = self.base_chunk();
        chunk["choices"] = json!([{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": call_id,
                    "type": "function",
                    "function": {
                        "name": name,
                        "arguments": ""
                    }
                }]
            },
            "finish_reason": null
        }]);
        self.pending.push_back(format!("data: {}\n\n", chunk));
        self.tool_call_started = true;
    }

    fn emit_tool_call_arguments(&mut self, call_id: &str, arguments: &str) {
        let mut chunk = self.base_chunk();
        chunk["choices"] = json!([{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": call_id,
                    "type": "function",
                    "function": {
                        "arguments": arguments
                    }
                }]
            },
            "finish_reason": null
        }]);
        self.pending.push_back(format!("data: {}\n\n", chunk));
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
                    // Warn if the stream completed without producing any content.
                    // This helps diagnose the "silent response" bug where the
                    // Codex backend returns 200 OK but emits no text deltas.
                    if !self.first_content_sent {
                        tracing::warn!(
                            target: "codex::sse",
                            response_id = %self.response_id,
                            model = %self.model,
                            "codex stream ended with zero content — \
                             upstream may have returned a degraded/empty response"
                        );
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

        let result = translate_request(&chat).unwrap();
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
    fn translate_uses_default_reasoning_effort_when_request_omits_it() {
        let chat = json!({
            "model": "codex-mini",
            "messages": [{"role": "user", "content": "Hello"}]
        });
        let result = translate_request_with_default_reasoning_effort(&chat, Some("high")).unwrap();
        assert_eq!(result["reasoning"]["effort"], "high");
    }

    #[test]
    fn translate_request_reasoning_effort_overrides_provider_default() {
        let chat = json!({
            "model": "codex-mini",
            "messages": [{"role": "user", "content": "Hello"}],
            "reasoning_effort": "low"
        });
        let result = translate_request_with_default_reasoning_effort(&chat, Some("high")).unwrap();
        assert_eq!(result["reasoning"]["effort"], "low");
    }

    #[test]
    fn translate_without_default_keeps_reasoning_absent() {
        let chat = json!({
            "model": "codex-mini",
            "messages": [{"role": "user", "content": "Hello"}]
        });
        let result = translate_request_with_default_reasoning_effort(&chat, None).unwrap();
        assert!(result.get("reasoning").is_none());
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

        let result = translate_request(&chat).unwrap();
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
        let result = translate_request(&chat).unwrap();
        let tools = result["tools"].as_array().unwrap();
        assert_eq!(tools[0]["strict"], false, "strict should be false without explicit flag");
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
        let result = translate_request(&chat).unwrap();
        let tools = result["tools"].as_array().unwrap();
        assert_eq!(tools[0]["strict"], false, "should not be strict for loose schema");
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
        let result = translate_request(&chat).unwrap();
        let tools = result["tools"].as_array().unwrap();
        assert_eq!(tools[0]["strict"], false, "explicit strict:false must override auto-detect");
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

        let result = translate_request(&chat).unwrap();
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
