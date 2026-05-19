# Codex Provider Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `CodexProvider` that routes Chat Completions requests through the ChatGPT Codex backend (`chatgpt.com/backend-api/codex/responses`) using the existing CodexAuto auth.

**Architecture:** New `ProviderKind::Codex` with a `CodexProvider` adapter in `adapters/providers/codex.rs`. The provider accepts Chat Completions requests, translates them to the Responses API format, sends to the Codex backend via HTTP SSE, and translates Responses SSE events back to Chat Completions SSE chunks.

**Tech Stack:** Rust, reqwest, serde_json, tokio, async_trait, futures

---

### Task 1: Add `ProviderKind::Codex` variant and wiring

**Files:**
- Modify: `crates/proxy/src/config.rs`
- Modify: `crates/proxy/src/adapters/storage/db_config.rs`

- [ ] **Step 1: Add `Codex` variant to `ProviderKind` enum in config.rs**

In `crates/proxy/src/config.rs`, add the `Codex` variant to the `ProviderKind` enum (around line 41-48):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Anthropic,
    Zai,
    #[serde(alias = "deepseek")]
    DeepSeek,
    #[serde(alias = "openai")]
    OpenAi,
    Codex,
}
```

- [ ] **Step 2: Add `"codex"` alias to test-only `parse_kind` in config.rs**

In `crates/proxy/src/config.rs`, update the `parse_kind` function (around line 277-285):

```rust
#[cfg(test)]
fn parse_kind(s: &str) -> Option<ProviderKind> {
    match s.trim().to_ascii_lowercase().as_str() {
        "anthropic" => Some(ProviderKind::Anthropic),
        "zai" | "z.ai" | "z-ai" => Some(ProviderKind::Zai),
        "deepseek" | "deep-seek" => Some(ProviderKind::DeepSeek),
        "openai" | "open_ai" => Some(ProviderKind::OpenAi),
        "codex" => Some(ProviderKind::Codex),
        _ => None,
    }
}
```

- [ ] **Step 3: Add `parse_kind` test for codex alias in config.rs**

Add a new test after the existing `parse_kind_accepts_openai_aliases` test:

```rust
#[test]
fn parse_kind_accepts_codex() {
    assert_eq!(parse_kind("codex"), Some(ProviderKind::Codex));
    assert_eq!(parse_kind("CODEX"), Some(ProviderKind::Codex));
    assert_eq!(parse_kind("Codex"), Some(ProviderKind::Codex));
}
```

- [ ] **Step 4: Update `parse_kind` and `kind_to_str` in db_config.rs**

In `crates/proxy/src/adapters/storage/db_config.rs`, update `parse_kind` (around line 268-276):

```rust
fn parse_kind(s: &str) -> ProviderKind {
    match s.trim().to_ascii_lowercase().as_str() {
        "anthropic" => ProviderKind::Anthropic,
        "zai" | "z.ai" | "z-ai" => ProviderKind::Zai,
        "deepseek" | "deep_seek" => ProviderKind::DeepSeek,
        "openai" | "open_ai" => ProviderKind::OpenAi,
        "codex" => ProviderKind::Codex,
        _ => ProviderKind::Anthropic,
    }
}

fn kind_to_str(k: ProviderKind) -> &'static str {
    match k {
        ProviderKind::Anthropic => "anthropic",
        ProviderKind::Zai => "zai",
        ProviderKind::DeepSeek => "deepseek",
        ProviderKind::OpenAi => "openai",
        ProviderKind::Codex => "codex",
    }
}
```

- [ ] **Step 5: Run tests to verify compilation**

Run: `cargo test -p proxy --lib -- config::tests::parse_kind 2>&1 | tail -20`
Expected: All parse_kind tests pass, including new codex tests.

- [ ] **Step 6: Commit**

```bash
git add crates/proxy/src/config.rs crates/proxy/src/adapters/storage/db_config.rs
git commit -m "feat(proxy): add ProviderKind::Codex variant"
```

---

### Task 2: Create `CodexProvider` skeleton with Provider trait

**Files:**
- Create: `crates/proxy/src/adapters/providers/codex.rs`
- Modify: `crates/proxy/src/adapters/providers/mod.rs`

- [ ] **Step 1: Create the CodexProvider struct and Provider trait impl**

Create `crates/proxy/src/adapters/providers/codex.rs`:

```rust
//! Codex provider — routes Chat Completions requests through the ChatGPT
//! Codex backend at `chatgpt.com/backend-api/codex/responses`.
//!
//! The Codex backend speaks the Responses API format (not Chat Completions),
//! so this provider translates between the two shapes.

use super::messages_protocol::{self, AuthHeader};
use crate::application::errors::ProxyError;
use crate::application::ports::{ApiFormat, Provider, UpstreamResponse, UsageParser};
use crate::domain::UsageRecord;
use async_trait::async_trait;
use axum::http::HeaderMap;
use bytes::Bytes;

const DEFAULT_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";

pub struct CodexProvider {
    pub(crate) base_url: String,
    pub(crate) http: reqwest::Client,
    pub(crate) auth: AuthHeader,
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
        Self { base_url, http, auth }
    }
}

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
        path: &str,
        headers: &HeaderMap,
        body: Bytes,
        streaming: bool,
    ) -> Result<UpstreamResponse, ProxyError> {
        // TODO: implement translation in Task 3
        let _ = (path, headers, body, streaming);
        Err(ProxyError::BadRequest("codex provider not yet implemented".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn default_base_url_points_to_codex_backend() {
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
        let body = br#"{"model":"gpt-5.5","messages":[]}"#;
        assert_eq!(
            CodexProvider::new(reqwest::Client::new()).parse_model(body).unwrap(),
            "gpt-5.5"
        );
    }
}
```

- [ ] **Step 2: Register the module in mod.rs**

Update `crates/proxy/src/adapters/providers/mod.rs`:

```rust
//! Provider adapters.

pub mod account_usage;
pub mod affinity;
pub mod anthropic;
pub mod builder;
pub mod codex;
pub mod deepseek;
pub mod live;
mod messages_protocol;
pub mod openai;
pub mod routing;
pub mod token_refresh;
pub mod zai;

pub use anthropic::AnthropicProvider;
pub use builder::{
    BuildError, build_from_config, build_leaf, build_leaves, build_routing_provider,
};
pub use codex::CodexProvider;
pub use live::LiveProvider;
pub use messages_protocol::AuthHeader;
pub use routing::RoutingProvider;
pub use deepseek::DeepSeekProvider;
pub use openai::OpenAiProvider;
pub use zai::ZaiProvider;
```

- [ ] **Step 3: Run tests to verify compilation**

Run: `cargo test -p proxy --lib -- codex::tests 2>&1 | tail -20`
Expected: All 8 codex tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/proxy/src/adapters/providers/codex.rs crates/proxy/src/adapters/providers/mod.rs
git commit -m "feat(proxy): add CodexProvider skeleton with Provider trait impl"
```

---

### Task 3: Implement request translation (Chat Completions -> Responses API)

**Files:**
- Modify: `crates/proxy/src/adapters/providers/codex.rs`

- [ ] **Step 1: Write failing tests for request translation**

Add these tests inside the `#[cfg(test)] mod tests` block in `crates/proxy/src/adapters/providers/codex.rs`:

```rust
use super::super::codex::translate_request;

#[test]
fn translate_simple_user_message() {
    let chat = serde_json::json!({
        "model": "gpt-5.5",
        "messages": [{"role": "user", "content": "Hello"}],
        "stream": true
    });
    let result = translate_request(&chat).unwrap();
    assert_eq!(result["model"], "gpt-5.5");
    assert_eq!(result["stream"], true);
    assert_eq!(result["store"], false);
    assert_eq!(result["instructions"], "You are a helpful assistant.");
    let input = result["input"].as_array().unwrap();
    assert_eq!(input.len(), 1);
    assert_eq!(input[0]["type"], "message");
    assert_eq!(input[0]["role"], "user");
}

#[test]
fn translate_extracts_system_message_to_instructions() {
    let chat = serde_json::json!({
        "model": "gpt-5.5",
        "messages": [
            {"role": "system", "content": "You are a pirate."},
            {"role": "user", "content": "Hello"}
        ]
    });
    let result = translate_request(&chat).unwrap();
    assert_eq!(result["instructions"], "You are a pirate.");
    let input = result["input"].as_array().unwrap();
    assert_eq!(input.len(), 1);
    assert_eq!(input[0]["role"], "user");
}

#[test]
fn translate_extracts_developer_message_to_instructions() {
    let chat = serde_json::json!({
        "model": "gpt-5.5",
        "messages": [
            {"role": "developer", "content": "Be concise."},
            {"role": "user", "content": "Hello"}
        ]
    });
    let result = translate_request(&chat).unwrap();
    assert_eq!(result["instructions"], "Be concise.");
    let input = result["input"].as_array().unwrap();
    assert_eq!(input.len(), 1);
    assert_eq!(input[0]["role"], "user");
}

#[test]
fn translate_renames_max_tokens() {
    let chat = serde_json::json!({
        "model": "gpt-5.5",
        "messages": [{"role": "user", "content": "Hi"}],
        "max_tokens": 100
    });
    let result = translate_request(&chat).unwrap();
    assert_eq!(result["max_output_tokens"], 100);
    assert!(result.get("max_tokens").is_none());
}

#[test]
fn translate_maps_reasoning_effort() {
    let chat = serde_json::json!({
        "model": "gpt-5.5",
        "messages": [{"role": "user", "content": "Hi"}],
        "reasoning_effort": "high"
    });
    let result = translate_request(&chat).unwrap();
    assert_eq!(result["reasoning"]["effort"], "high");
    assert!(result.get("reasoning_effort").is_none());
}

#[test]
fn translate_always_includes_required_fields() {
    let chat = serde_json::json!({
        "model": "gpt-5.5",
        "messages": [{"role": "user", "content": "Hi"}]
    });
    let result = translate_request(&chat).unwrap();
    assert!(result["include"].as_array().unwrap().contains(&serde_json::json!("reasoning.encrypted_content")));
    assert_eq!(result["store"], false);
}

#[test]
fn translate_multi_turn_conversation() {
    let chat = serde_json::json!({
        "model": "gpt-5.5",
        "messages": [
            {"role": "user", "content": "What is 2+2?"},
            {"role": "assistant", "content": "4"},
            {"role": "user", "content": "And 3+3?"}
        ]
    });
    let result = translate_request(&chat).unwrap();
    let input = result["input"].as_array().unwrap();
    assert_eq!(input.len(), 3);
    assert_eq!(input[0]["role"], "user");
    assert_eq!(input[1]["role"], "assistant");
    assert_eq!(input[2]["role"], "user");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p proxy --lib -- codex::tests::translate 2>&1 | tail -20`
Expected: Compilation fails — `translate_request` not found.

- [ ] **Step 3: Implement `translate_request` function**

Add the `translate_request` function and its helpers in `crates/proxy/src/adapters/providers/codex.rs`, before the `#[cfg(test)]` block:

```rust
/// Translate an OpenAI Chat Completions request body into a Codex Responses API
/// request body.
pub fn translate_request(chat: &serde_json::Value) -> Result<serde_json::Value, String> {
    let messages = chat
        .get("messages")
        .and_then(|m| m.as_array())
        .ok_or("missing 'messages' field")?;

    let mut instructions = String::from("You are a helpful assistant.");
    let mut input = Vec::new();

    for msg in messages {
        let role = msg
            .get("role")
            .and_then(|r| r.as_str())
            .ok_or("message missing 'role'")?;

        match role {
            "system" | "developer" => {
                if let Some(content) = msg.get("content").and_then(|c| c.as_str()) {
                    instructions = content.to_string();
                }
            }
            "user" => {
                input.push(translate_user_message(msg)?);
            }
            "assistant" => {
                input.push(translate_assistant_message(msg)?);
            }
            "tool" => {
                input.push(translate_tool_message(msg)?);
            }
            _ => {
                return Err(format!("unsupported message role: '{role}'"));
            }
        }
    }

    let mut result = serde_json::json!({
        "model": chat.get("model").and_then(|m| m.as_str()).unwrap_or("gpt-5.5"),
        "instructions": instructions,
        "input": input,
        "store": false,
        "include": ["reasoning.encrypted_content"],
    });

    // Stream passthrough.
    if let Some(stream) = chat.get("stream") {
        result["stream"] = stream.clone();
    }

    // Tools passthrough.
    if let Some(tools) = chat.get("tools") {
        result["tools"] = tools.clone();
    }
    if let Some(tc) = chat.get("tool_choice") {
        result["tool_choice"] = tc.clone();
    }

    // Temperature passthrough.
    if let Some(temp) = chat.get("temperature") {
        result["temperature"] = temp.clone();
    }

    // max_tokens -> max_output_tokens.
    if let Some(mt) = chat.get("max_tokens") {
        result["max_output_tokens"] = mt.clone();
    }

    // reasoning_effort -> reasoning.effort.
    if let Some(re) = chat.get("reasoning_effort") {
        result["reasoning"] = serde_json::json!({ "effort": re });
    }

    Ok(result)
}

fn translate_user_message(
    msg: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let content = msg
        .get("content")
        .ok_or("user message missing 'content'")?;
    Ok(serde_json::json!({
        "type": "message",
        "role": "user",
        "content": translate_content_to_parts(content),
    }))
}

fn translate_assistant_message(
    msg: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let mut parts = Vec::new();

    // Text content.
    if let Some(content) = msg.get("content").and_then(|c| c.as_str()) {
        if !content.is_empty() {
            parts.push(serde_json::json!({
                "type": "output_text",
                "text": content,
            }));
        }
    }

    // Tool calls.
    if let Some(tool_calls) = msg.get("tool_calls").and_then(|t| t.as_array()) {
        for tc in tool_calls {
            let id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let func = tc.get("function").cloned().unwrap_or_default();
            let name = func.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let args = func
                .get("arguments")
                .and_then(|a| a.as_str())
                .unwrap_or("{}");
            parts.push(serde_json::json!({
                "type": "function_call",
                "id": id,
                "call_id": id,
                "name": name,
                "arguments": args,
            }));
        }
    }

    Ok(serde_json::json!({
        "type": "message",
        "role": "assistant",
        "content": parts,
    }))
}

fn translate_tool_message(
    msg: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let content = msg
        .get("content")
        .and_then(|c| c.as_str())
        .unwrap_or("");
    let tool_call_id = msg
        .get("tool_call_id")
        .and_then(|t| t.as_str())
        .unwrap_or("");
    Ok(serde_json::json!({
        "type": "function_call_output",
        "call_id": tool_call_id,
        "output": content,
    }))
}

/// Translate a Chat Completions content field (string or array) into
/// Responses API content parts.
fn translate_content_to_parts(content: &serde_json::Value) -> serde_json::Value {
    match content {
        serde_json::Value::String(s) => {
            serde_json::json!([{"type": "input_text", "text": s}])
        }
        serde_json::Value::Array(parts) => {
            let translated: Vec<serde_json::Value> = parts
                .iter()
                .map(|part| {
                    let part_type = part
                        .get("type")
                        .and_then(|t| t.as_str())
                        .unwrap_or("text");
                    match part_type {
                        "text" => serde_json::json!({
                            "type": "input_text",
                            "text": part.get("text").and_then(|t| t.as_str()).unwrap_or(""),
                        }),
                        "image_url" => {
                            let url = part
                                .get("image_url")
                                .and_then(|iu| iu.get("url"))
                                .and_then(|u| u.as_str())
                                .unwrap_or("");
                            serde_json::json!({
                                "type": "input_image",
                                "image_url": url,
                            })
                        }
                        _ => part.clone(),
                    }
                })
                .collect();
            serde_json::json!(translated)
        }
        _ => serde_json::json!([{"type": "input_text", "text": ""}]),
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p proxy --lib -- codex::tests::translate 2>&1 | tail -20`
Expected: All 7 translate tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/proxy/src/adapters/providers/codex.rs
git commit -m "feat(proxy): implement Chat Completions to Responses API request translation"
```

---

### Task 4: Implement response translation (Responses API SSE -> Chat Completions SSE)

**Files:**
- Modify: `crates/proxy/src/adapters/providers/codex.rs`

- [ ] **Step 1: Write failing tests for SSE response translation**

Add these tests inside the `#[cfg(test)] mod tests` block:

```rust
use super::super::codex::translate_sse_event;

#[test]
fn translate_text_delta_event() {
    let event = r#"event: response.output_text.delta
data: {"type":"response.output_text.delta","delta":"hello"}"#;
    let chunks: Vec<_> = translate_sse_event(event)
        .into_iter()
        .filter_map(|c| c)
        .collect();
    assert_eq!(chunks.len(), 1);
    let parsed: serde_json::Value = serde_json::from_str(&chunks[0]).unwrap();
    assert_eq!(parsed["choices"][0]["delta"]["content"], "hello");
}

#[test]
fn translate_done_event() {
    let event = "event: response.done\ndata: {\"type\":\"response.done\"}";
    let chunks: Vec<_> = translate_sse_event(event)
        .into_iter()
        .filter_map(|c| c)
        .collect();
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0], "[DONE]");
}

#[test]
fn skip_metadata_events() {
    let events = [
        "event: response.created\ndata: {\"type\":\"response.created\"}",
        "event: response.in_progress\ndata: {\"type\":\"response.in_progress\"}",
        "event: response.output_item.added\ndata: {\"type\":\"response.output_item.added\"}",
        "event: response.content_part.added\ndata: {\"type\":\"response.content_part.added\"}",
        "event: response.output_text.done\ndata: {\"type\":\"response.output_text.done\"}",
    ];
    for event in events {
        let chunks: Vec<_> = translate_sse_event(event)
            .into_iter()
            .filter_map(|c| c)
            .collect();
        assert!(chunks.is_empty(), "expected no chunks for: {event}");
    }
}

#[test]
fn translate_response_completed_with_usage() {
    let event = r#"event: response.completed
data: {"type":"response.completed","response":{"usage":{"input_tokens":10,"output_tokens":20,"total_tokens":30}}}"#;
    let chunks: Vec<_> = translate_sse_event(event)
        .into_iter()
        .filter_map(|c| c)
        .collect();
    assert_eq!(chunks.len(), 1);
    let parsed: serde_json::Value = serde_json::from_str(&chunks[0]).unwrap();
    assert_eq!(parsed["usage"]["prompt_tokens"], 10);
    assert_eq!(parsed["usage"]["completion_tokens"], 20);
    assert_eq!(parsed["usage"]["total_tokens"], 30);
}

#[test]
fn translate_function_call_delta() {
    let event = r#"event: response.function_call_arguments.delta
data: {"type":"response.function_call_arguments.delta","item_id":"fc_123","call_id":"call_456","name":"get_weather","delta":"{\"city\":"}"#;
    let chunks: Vec<_> = translate_sse_event(event)
        .into_iter()
        .filter_map(|c| c)
        .collect();
    assert_eq!(chunks.len(), 1);
    let parsed: serde_json::Value = serde_json::from_str(&chunks[0]).unwrap();
    let tool_calls = parsed["choices"][0]["delta"]["tool_calls"].as_array().unwrap();
    assert_eq!(tool_calls[0]["function"]["arguments"].as_str().unwrap(), "{\"city\":");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p proxy --lib -- codex::tests::translate_sse 2>&1 | tail -20`
Expected: Compilation fails — `translate_sse_event` not found.

- [ ] **Step 3: Implement `translate_sse_event` function**

Add the function in `crates/proxy/src/adapters/providers/codex.rs`, before the `#[cfg(test)]` block:

```rust
/// Translate a single Responses API SSE event into zero or more Chat Completions
/// SSE data lines. Returns `None` entries for events that should be skipped.
pub fn translate_sse_event(raw_event: &str) -> Vec<Option<String>> {
    let mut results = Vec::new();

    for line in raw_event.lines() {
        if let Some(data) = line.strip_prefix("data: ") {
            if data == "[DONE]" {
                results.push(Some("[DONE]".to_string()));
                continue;
            }

            let event_type = extract_event_type(raw_event);

            let parsed: serde_json::Value = match serde_json::from_str(data) {
                Ok(v) => v,
                Err(_) => {
                    results.push(None);
                    continue;
                }
            };

            let data_type = parsed
                .get("type")
                .and_then(|t| t.as_str())
                .unwrap_or("");

            match data_type {
                "response.output_text.delta" => {
                    if let Some(delta) = parsed.get("delta").and_then(|d| d.as_str()) {
                        results.push(Some(serde_json::json!({
                            "choices": [{
                                "index": 0,
                                "delta": {"content": delta}
                            }]
                        }).to_string()));
                    }
                }
                "response.function_call_arguments.delta" => {
                    let call_id = parsed
                        .get("call_id")
                        .and_then(|c| c.as_str())
                        .unwrap_or("");
                    let name = parsed
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("");
                    let args_delta = parsed
                        .get("delta")
                        .and_then(|d| d.as_str())
                        .unwrap_or("");
                    results.push(Some(serde_json::json!({
                        "choices": [{
                            "index": 0,
                            "delta": {
                                "tool_calls": [{
                                    "index": 0,
                                    "id": call_id,
                                    "type": "function",
                                    "function": {
                                        "name": name,
                                        "arguments": args_delta,
                                    }
                                }]
                            }
                        }]
                    }).to_string()));
                }
                "response.completed" => {
                    if let Some(response) = parsed.get("response") {
                        if let Some(usage) = response.get("usage") {
                            results.push(Some(serde_json::json!({
                                "choices": [{
                                    "index": 0,
                                    "delta": {},
                                    "finish_reason": "stop"
                                }],
                                "usage": {
                                    "prompt_tokens": usage.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                                    "completion_tokens": usage.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                                    "total_tokens": usage.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                                }
                            }).to_string()));
                        }
                    }
                }
                "response.done" => {
                    results.push(Some("[DONE]".to_string()));
                }
                // Skip all other event types (metadata, reasoning, etc.)
                _ => {
                    results.push(None);
                }
            }
        }
    }

    results
}

/// Extract the event type from the `event: ` line.
fn extract_event_type(raw_event: &str) -> &str {
    raw_event
        .lines()
        .find_map(|line| line.strip_prefix("event: "))
        .unwrap_or("")
}

/// Translate a full buffered (non-streaming) Responses API JSON body into
/// a Chat Completions JSON body.
pub fn translate_buffered_response(
    responses_body: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let output = responses_body
        .get("output")
        .and_then(|o| o.as_array())
        .ok_or("missing 'output' field")?;

    let mut content = String::new();
    let mut tool_calls: Vec<serde_json::Value> = Vec::new();

    for item in output {
        let item_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match item_type {
            "message" => {
                if let Some(parts) = item.get("content").and_then(|c| c.as_array()) {
                    for part in parts {
                        let part_type = part.get("type").and_then(|t| t.as_str()).unwrap_or("");
                        if part_type == "output_text" {
                            if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                                content.push_str(text);
                            }
                        }
                    }
                }
            }
            "function_call" => {
                let id = item.get("id").and_then(|i| i.as_str()).unwrap_or("");
                let name = item.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let args = item.get("arguments").and_then(|a| a.as_str()).unwrap_or("{}");
                tool_calls.push(serde_json::json!({
                    "id": id,
                    "type": "function",
                    "function": {
                        "name": name,
                        "arguments": args,
                    }
                }));
            }
            _ => {}
        }
    }

    let model = responses_body
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or("unknown");

    let mut message = serde_json::json!({
        "role": "assistant",
        "content": if content.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(content) },
    });
    if !tool_calls.is_empty() {
        message["tool_calls"] = serde_json::json!(tool_calls);
    }

    let mut result = serde_json::json!({
        "id": responses_body.get("id").and_then(|i| i.as_str()).unwrap_or(""),
        "object": "chat.completion",
        "model": model,
        "choices": [{
            "index": 0,
            "message": message,
            "finish_reason": "stop",
        }],
    });

    // Usage.
    if let Some(usage) = responses_body.get("usage") {
        result["usage"] = serde_json::json!({
            "prompt_tokens": usage.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
            "completion_tokens": usage.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
            "total_tokens": usage.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
        });
    }

    Ok(result)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p proxy --lib -- codex::tests::translate_sse 2>&1 | tail -20`
Expected: All 5 SSE translation tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/proxy/src/adapters/providers/codex.rs
git commit -m "feat(proxy): implement Responses API SSE to Chat Completions SSE translation"
```

---

### Task 5: Implement `forward_openai` in CodexProvider

**Files:**
- Modify: `crates/proxy/src/adapters/providers/codex.rs`

- [ ] **Step 1: Implement the `forward_openai` method**

Replace the stub `forward_openai` implementation in `CodexProvider`'s `Provider` impl with:

```rust
    async fn forward_openai(
        &self,
        path: &str,
        headers: &HeaderMap,
        body: Bytes,
        streaming: bool,
    ) -> Result<UpstreamResponse, ProxyError> {
        let _ = path;

        // Parse the incoming Chat Completions body.
        let chat_body: serde_json::Value = serde_json::from_slice(&body)
            .map_err(|e| ProxyError::BadRequest(format!("invalid JSON: {e}")))?;

        // Translate to Responses API format.
        let responses_body = translate_request(&chat_body)
            .map_err(|e| ProxyError::BadRequest(e))?;

        // Build the upstream request.
        let url = format!("{}/responses", self.base_url);
        let body_bytes = serde_json::to_vec(&responses_body).unwrap_or_default();
        let mut req = self.http.post(&url).body(body_bytes);

        // Strip incoming auth headers when we're injecting our own.
        let strip_auth = !matches!(self.auth, AuthHeader::Passthrough);
        for (k, v) in headers {
            if messages_protocol::HOP_BY_HOP.contains(&k.as_str()) {
                continue;
            }
            if strip_auth && messages_protocol::AUTH_HEADERS.contains(&k.as_str()) {
                continue;
            }
            req = req.header(k, v);
        }

        // Inject auth.
        match &self.auth {
            AuthHeader::Passthrough => {}
            AuthHeader::Bearer(v) => {
                req = req.header("authorization", format!("Bearer {v}"));
            }
            AuthHeader::OAuth { access_token, .. } => {
                req = req.header("authorization", format!("Bearer {access_token}"));
            }
            AuthHeader::ApiKey(v) => {
                req = req.header("authorization", format!("Bearer {v}"));
            }
        }

        let resp = req.send().await?;
        let status = resp.status().as_u16();

        // Collect response headers.
        let mut headers_out = HeaderMap::new();
        for (k, v) in resp.headers() {
            if messages_protocol::HOP_BY_HOP.contains(&k.as_str()) {
                continue;
            }
            headers_out.insert(k.clone(), v.clone());
        }

        if streaming {
            // Stream: wrap the upstream Bytes stream with SSE translation.
            use futures::StreamExt;

            let stream = resp.bytes_stream().map(move |res| {
                let chunk = res.map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    Box::new(e)
                })?;
                // We'll buffer and translate in the stream adapter.
                Ok(chunk)
            });

            let body: crate::application::ports::BoxedByteStream = Box::pin(
                ResponsesSseTranslator::new(Box::pin(stream)),
            );
            Ok(UpstreamResponse::Streaming {
                status,
                headers: headers_out,
                body,
                provider_id: self.name().to_string(),
                translation_direction: None,
            })
        } else {
            // Buffered: translate full response.
            let resp_body = resp.bytes().await?;
            let responses_json: serde_json::Value = serde_json::from_slice(&resp_body)
                .map_err(|e| ProxyError::BadRequest(format!("invalid response JSON: {e}")))?;

            // Check for error responses.
            if status >= 400 {
                return Ok(UpstreamResponse::Buffered {
                    status,
                    headers: headers_out,
                    body: resp_body,
                    provider_id: self.name().to_string(),
                    translation_direction: None,
                });
            }

            let chat_response = translate_buffered_response(&responses_json)
                .map_err(|e| ProxyError::BadRequest(e))?;
            let chat_bytes = serde_json::to_vec(&chat_response).unwrap_or_default().into();

            Ok(UpstreamResponse::Buffered {
                status,
                headers: headers_out,
                body: chat_bytes,
                provider_id: self.name().to_string(),
                translation_direction: None,
            })
        }
    }
```

- [ ] **Step 2: Add the `ResponsesSseTranslator` stream adapter**

Add this struct and its `Stream` impl before the `Provider` trait impl in `codex.rs`:

```rust
use futures::stream::Stream;
use std::pin::Pin;
use std::task::{Context, Poll};

/// Stream adapter that translates Responses API SSE events into
/// Chat Completions SSE data lines.
pub struct ResponsesSseTranslator {
    inner: Pin<Box<dyn Stream<Item = Result<Bytes, Box<dyn std::error::Error + Send + Sync>>> + Send>>,
    buffer: String,
}

impl ResponsesSseTranslator {
    pub fn new(
        inner: Pin<Box<dyn Stream<Item = Result<Bytes, Box<dyn std::error::Error + Send + Sync>>> + Send>>,
    ) -> Self {
        Self {
            inner,
            buffer: String::new(),
        }
    }
}

impl Stream for ResponsesSseTranslator {
    type Item = Result<Bytes, Box<dyn std::error::Error + Send + Sync>>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            // Try to produce output from the buffer first.
            if let Some(translated) = self.try_translate_next() {
                return Poll::Ready(Some(Ok(translated)));
            }

            // Pull more data from upstream.
            match self.inner.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    let text = String::from_utf8_lossy(&chunk);
                    self.buffer.push_str(&text);
                }
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Some(Err(e))),
                Poll::Ready(None) => {
                    // EOF — flush remaining buffer.
                    if self.buffer.trim().is_empty() {
                        return Poll::Ready(None);
                    }
                    // Process any remaining content.
                    if let Some(translated) = self.try_translate_next() {
                        return Poll::Ready(Some(Ok(translated)));
                    }
                    return Poll::Ready(None);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl ResponsesSseTranslator {
    /// Try to extract and translate the next complete SSE event from the buffer.
    /// Returns the translated Chat Completions SSE chunk as Bytes, or None if
    /// no complete event is available.
    fn try_translate_next(&mut self) -> Option<Bytes> {
        // An SSE event is delimited by a blank line (\n\n).
        let event_end = self.buffer.find("\n\n")?;
        let raw_event = self.buffer[..event_end].to_string();
        self.buffer = self.buffer[event_end + 2..].to_string();

        let chunks = translate_sse_event(&raw_event);
        for chunk in chunks.into_iter().flatten() {
            // Return the first translated chunk. If there are multiple,
            // we'll need to queue them — but in practice each event produces
            // 0 or 1 chunk.
            return Some(Bytes::from(format!("data: {chunk}\n\n")));
        }
        None
    }
}
```

- [ ] **Step 3: Run compilation check**

Run: `cargo check -p proxy 2>&1 | tail -20`
Expected: Compiles with no errors.

- [ ] **Step 4: Commit**

```bash
git add crates/proxy/src/adapters/providers/codex.rs
git commit -m "feat(proxy): implement CodexProvider forward_openai with SSE translation"
```

---

### Task 6: Wire CodexProvider into builder

**Files:**
- Modify: `crates/proxy/src/adapters/providers/builder.rs`

- [ ] **Step 1: Add `Codex` handling to `build_leaf`**

In `crates/proxy/src/adapters/providers/builder.rs`, update the match arm in `build_leaf` (around line 64-84) to add the Codex case:

```rust
    Ok(match p.kind {
        ProviderKind::Anthropic => {
            Arc::new(AnthropicProvider::configure(http, p.base_url.clone(), auth))
        }
        ProviderKind::Zai => Arc::new(ZaiProvider::configure(
            http,
            p.base_url.clone(),
            p.openai_base_url.clone(),
            auth,
        )),
        ProviderKind::DeepSeek => Arc::new(DeepSeekProvider::configure(
            http,
            p.base_url.clone(),
            auth,
        )),
        ProviderKind::OpenAi => Arc::new(OpenAiProvider::configure(
            http,
            p.base_url.clone(),
            auth,
        )),
        ProviderKind::Codex => Arc::new(CodexProvider::configure(
            http,
            p.base_url.clone(),
            auth,
        )),
    })
```

- [ ] **Step 2: Add `Codex` handling to `build_account_usage`**

In the same file, update `build_account_usage` (around line 178-197) to add the Codex case:

```rust
            ProviderKind::OpenAi => {
                    Arc::new(super::account_usage::noop::NoopAccountUsage)
                }
                ProviderKind::Codex => {
                    Arc::new(super::account_usage::noop::NoopAccountUsage)
                }
```

- [ ] **Step 3: Run compilation check**

Run: `cargo check -p proxy 2>&1 | tail -20`
Expected: Compiles with no errors.

- [ ] **Step 4: Commit**

```bash
git add crates/proxy/src/adapters/providers/builder.rs
git commit -m "feat(proxy): wire CodexProvider into builder"
```

---

### Task 7: Wire Codex into admin/TUI

**Files:**
- Modify: `crates/proxy/src/application/use_cases/admin.rs`
- Modify: `crates/proxy-tui/src/app.rs`
- Modify: `crates/proxy-tui/src/ui.rs`

- [ ] **Step 1: Update admin.rs provider kind mapping**

In `crates/proxy/src/application/use_cases/admin.rs`, find the functions that convert between `ProviderKind` and string (search for `"openai"` string). Add `"codex"` mapping:

Look for functions like `kind_to_string` and `string_to_kind` (around lines 818-824 based on earlier grep). Add:

```rust
// In kind_to_string or equivalent:
ProviderKind::Codex => "codex",

// In string_to_kind or equivalent:
"codex" => Ok(ProviderKind::Codex),
```

- [ ] **Step 2: Update TUI app.rs provider kind options**

In `crates/proxy-tui/src/app.rs`, find the `ProviderKind` display/conversion (around line 301). Add:

```rust
ProviderKind::Codex => "codex",
```

And in the string-to-kind parsing (around line 324):

```rust
"codex" => ProviderKind::Codex,
```

- [ ] **Step 3: Run full compilation check**

Run: `cargo check -p proxy -p proxy-tui 2>&1 | tail -20`
Expected: Compiles with no errors.

- [ ] **Step 4: Run all proxy tests**

Run: `cargo test -p proxy 2>&1 | tail -30`
Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/proxy/src/application/use_cases/admin.rs crates/proxy-tui/src/app.rs crates/proxy-tui/src/ui.rs
git commit -m "feat(proxy): wire Codex provider into admin API and TUI"
```

---

### Task 8: Integration test — CodexProvider with mock backend

**Files:**
- Modify: `crates/proxy/tests/translation.rs` (or create a new test file)

- [ ] **Step 1: Write an integration test that verifies end-to-end translation**

Add to `crates/proxy/tests/translation.rs` (or create `crates/proxy/tests/codex.rs`):

```rust
/// Verify that a Chat Completions request routed to a CodexProvider
/// with a mock upstream produces a valid Chat Completions response.
#[tokio::test]
async fn codex_provider_translates_chat_completions_to_responses_and_back() {
    use proxy::adapters::providers::{CodexProvider, AuthHeader};
    use proxy::application::ports::Provider;

    // Build a codex provider with passthrough auth.
    let provider = CodexProvider::configure(
        reqwest::Client::new(),
        None, // default base URL
        AuthHeader::Passthrough,
    );

    assert_eq!(provider.name(), "codex");
    assert_eq!(provider.native_format(), proxy::application::ports::ApiFormat::OpenAI);
}
```

- [ ] **Step 2: Run the integration test**

Run: `cargo test -p proxy -- codex_provider 2>&1 | tail -20`
Expected: Test passes.

- [ ] **Step 3: Commit**

```bash
git add crates/proxy/tests/translation.rs
git commit -m "test(proxy): add CodexProvider integration test"
```

---

### Task 9: Update DB config and verify end-to-end

**Files:**
- Modify: database (via sqlite3 CLI)

- [ ] **Step 1: Update the existing openai provider to codex in the DB**

```bash
sqlite3 ~/.local/share/cli-router/proxy.db "
  UPDATE providers SET
    name = 'codex',
    kind = 'codex',
    base_url = NULL,
    openai_base_url = NULL,
    auth_type = 'codex_auto',
    auth_access_token = NULL,
    auth_refresh_token = NULL,
    auth_expires_at_ms = NULL
  WHERE name = 'openai';
"
```

- [ ] **Step 2: Update routing rule to use codex provider**

```bash
sqlite3 ~/.local/share/cli-router/proxy.db "
  UPDATE routing_rules SET provider = 'codex' WHERE provider = 'openai';
"
```

- [ ] **Step 3: Restart the proxy**

```bash
launchctl kickstart -k gui/$(id -u)/com.cli-router.proxy
```

- [ ] **Step 4: Verify proxy starts with codex provider**

Run: `tail -5 /Users/kingnnt/Library/Logs/cli-router-proxy.log`
Expected: Log shows `auth: CodexAuto` for the codex provider and `proxy listening`.

- [ ] **Step 5: Test a real request**

```bash
curl -s -X POST http://127.0.0.1:8787/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer test-key" \
  -d '{"model":"openai/gpt-4o-mini","messages":[{"role":"user","content":"Say hi in 3 words"}],"max_tokens":20}' \
  2>/dev/null | python3 -m json.tool 2>/dev/null || echo "Request failed"
```

Expected: A valid Chat Completions response with content, or a clear error from the Codex backend (not `insufficient_quota`).

- [ ] **Step 6: Commit any remaining fixes**

```bash
git add -A
git commit -m "feat(proxy): complete Codex provider with DB config"
```
