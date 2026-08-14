//! Chat Completions → OpenAI Responses API translation.
//!
//! Extracted from `CodexProvider` so any provider with a `/responses`
//! endpoint can reuse it. Upstream-specific behavior lives in
//! [`ResponsesDialect`] rather than in the translation itself.

use crate::application::ports::{BoxedByteStream, BoxedError};
use bytes::Bytes;
use futures::Stream;
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::pin::Pin;
use std::task::{Context, Poll};

/// Translate a single Chat Completions tool definition to Responses API format.
///
/// Chat Completions: `{"type":"function","function":{"name":"...","description":"...","parameters":{...}}}`
/// Responses API:    `{"type":"function","name":"...","description":"...","parameters":{...}}`
pub fn translate_tool(tool: &Value) -> Value {
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
            let strict = func
                .get("strict")
                .and_then(|s| s.as_bool())
                .unwrap_or(false);

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

pub fn text_content_to_string(content: Option<&Value>) -> String {
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

/// Upstream-specific behavior on the Responses path. The translation itself
/// is shared; these knobs are what the ChatGPT backend needs and a plain
/// Responses gateway does not.
#[derive(Debug, Clone, Copy)]
pub struct ResponsesDialect {
    /// Force `stream: true` regardless of what the client asked for.
    pub force_stream: bool,
    /// Value for `store`, or `None` to omit the field.
    pub store: Option<bool>,
    /// Send `include: ["reasoning.encrypted_content"]`.
    pub include_encrypted_reasoning: bool,
    /// Drop the client's token cap instead of mapping it to
    /// `max_output_tokens`.
    pub drop_max_output_tokens: bool,
    /// Used when the request has no system/developer message. `None` omits
    /// the `instructions` key entirely so the upstream sees exactly what the
    /// client sent.
    pub default_instructions: Option<&'static str>,
}

impl ResponsesDialect {
    /// The ChatGPT backend behind `CodexProvider`: it rejects
    /// `max_output_tokens`, requires `store: false` plus encrypted reasoning,
    /// and only answers streaming requests.
    pub const fn codex() -> Self {
        Self {
            force_stream: true,
            store: Some(false),
            include_encrypted_reasoning: true,
            drop_max_output_tokens: true,
            default_instructions: Some("You are a helpful assistant."),
        }
    }

    /// A plain OpenAI-compatible Responses endpoint (OpenCode Go). It has no
    /// requirement Codex's does, so a request without a system message ships
    /// without `instructions` rather than acquiring a prompt the client never
    /// wrote.
    pub const fn vanilla() -> Self {
        Self {
            force_stream: false,
            store: None,
            include_encrypted_reasoning: false,
            drop_max_output_tokens: false,
            default_instructions: None,
        }
    }
}

pub fn translate_request(chat: &Value, dialect: &ResponsesDialect) -> Result<Value, String> {
    let mut out = serde_json::Map::new();

    // model → passthrough
    if let Some(model) = chat.get("model") {
        out.insert("model".into(), model.clone());
    }

    // Extract instructions from system/developer messages
    let mut instructions = dialect
        .default_instructions
        .map(|s| Value::String(s.to_string()));
    let mut input_messages = Vec::new();

    if let Some(messages) = chat.get("messages").and_then(|m| m.as_array()) {
        for msg in messages {
            let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("user");

            if role == "system" || role == "developer" {
                let content = text_content_to_string(msg.get("content"));
                instructions = Some(Value::String(content));
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

    // Only present when the client sent a system/developer message or the
    // dialect requires a default.
    if let Some(instructions) = instructions {
        out.insert("instructions".into(), instructions);
    }
    out.insert("input".into(), Value::Array(input_messages));

    // reasoning_effort → reasoning.effort
    if let Some(effort) = chat.get("reasoning_effort") {
        out.insert("reasoning".into(), json!({ "effort": effort }));
    }

    if !dialect.drop_max_output_tokens
        && let Some(cap) = chat
            .get("max_tokens")
            .or_else(|| chat.get("max_completion_tokens"))
    {
        out.insert("max_output_tokens".into(), cap.clone());
    }

    if let Some(store) = dialect.store {
        out.insert("store".into(), json!(store));
    }
    if dialect.include_encrypted_reasoning {
        out.insert("include".into(), json!(["reasoning.encrypted_content"]));
    }
    if dialect.force_stream {
        out.insert("stream".into(), json!(true));
    } else if let Some(stream) = chat.get("stream") {
        out.insert("stream".into(), stream.clone());
    }

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

pub fn translate_buffered_response(responses_body: &Value) -> Result<Value, String> {
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
        let input_tokens = usage
            .get("input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
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

pub struct ResponsesSseTranslator {
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
    pub fn new(inner: BoxedByteStream) -> Self {
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
    type Item = Result<Bytes, BoxedError>;

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

#[cfg(test)]
mod tests {
    use super::*;

    fn chat() -> Value {
        json!({
            "model": "grok-4.5",
            "max_tokens": 512,
            "stream": false,
            "messages": [{"role": "user", "content": "hi"}]
        })
    }

    #[test]
    fn codex_dialect_keeps_current_behavior() {
        let out = translate_request(&chat(), &ResponsesDialect::codex()).unwrap();
        assert_eq!(out["store"], json!(false));
        assert_eq!(out["include"], json!(["reasoning.encrypted_content"]));
        assert_eq!(out["stream"], json!(true));
        assert!(out.get("max_output_tokens").is_none());
        assert!(out.get("max_tokens").is_none());
    }

    /// Codex needs a default because its backend requires the field; a plain
    /// Responses gateway does not, and inventing one would put a prompt the
    /// client never wrote in front of the model — visible nowhere in config,
    /// and applied only to the models a `model_formats` rule happens to send
    /// down this path.
    #[test]
    fn vanilla_dialect_omits_instructions_without_a_system_message() {
        let out = translate_request(&chat(), &ResponsesDialect::vanilla()).unwrap();
        assert!(
            out.get("instructions").is_none(),
            "vanilla must not invent instructions, got: {out}"
        );

        let codex = translate_request(&chat(), &ResponsesDialect::codex()).unwrap();
        assert_eq!(codex["instructions"], "You are a helpful assistant.");
    }

    /// A system message is still extracted on the vanilla dialect — only the
    /// invented default is gone.
    #[test]
    fn vanilla_dialect_still_extracts_a_system_message() {
        let mut with_system = chat();
        with_system["messages"] = json!([
            {"role": "system", "content": "Be terse."},
            {"role": "user", "content": "hi"}
        ]);
        let out = translate_request(&with_system, &ResponsesDialect::vanilla()).unwrap();
        assert_eq!(out["instructions"], "Be terse.");
    }

    #[test]
    fn vanilla_dialect_omits_store_and_include() {
        let out = translate_request(&chat(), &ResponsesDialect::vanilla()).unwrap();
        assert!(out.get("store").is_none());
        assert!(out.get("include").is_none());
    }

    #[test]
    fn vanilla_dialect_preserves_client_stream_flag() {
        let out = translate_request(&chat(), &ResponsesDialect::vanilla()).unwrap();
        assert_eq!(out["stream"], json!(false));

        let mut streaming = chat();
        streaming["stream"] = json!(true);
        let out = translate_request(&streaming, &ResponsesDialect::vanilla()).unwrap();
        assert_eq!(out["stream"], json!(true));
    }

    #[test]
    fn vanilla_dialect_maps_max_tokens_to_max_output_tokens() {
        let out = translate_request(&chat(), &ResponsesDialect::vanilla()).unwrap();
        assert_eq!(out["max_output_tokens"], json!(512));
        assert!(out.get("max_tokens").is_none());
    }

    #[test]
    fn vanilla_dialect_accepts_max_completion_tokens() {
        let mut c = chat();
        c.as_object_mut().unwrap().remove("max_tokens");
        c["max_completion_tokens"] = json!(64);
        let out = translate_request(&c, &ResponsesDialect::vanilla()).unwrap();
        assert_eq!(out["max_output_tokens"], json!(64));
    }
}
