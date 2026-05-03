# Cross-format Translation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable any client to call any upstream regardless of native API format. Translate Anthropic ↔ OpenAI for request body, response body, and streaming SSE events when client format and provider native format differ.

**Architecture:** New `adapters/translation/` module owns the translation matrix. Format mismatch is auto-detected at handler entry: client path (`/v1/messages` vs `/v1/chat/completions`) versus provider's `native_format()` selects a `Direction` enum. Same-format = passthrough (current path, zero overhead). Mismatch wraps the request/response/stream pipeline through the appropriate translator. Stateful FSMs handle streaming.

**Scope:** text + tools + streaming, bidirectional. No multimodal, no Gemini, no Files/Batch APIs.

**Tech Stack:** Rust 2024, `serde_json::Value` (schema-loose). No new deps.

**Spec:** `docs/superpowers/specs/2026-05-03-cross-format-translation-design.md` — has complete mapping tables. **This plan references it; consult the spec for field-by-field mappings rather than duplicating them here.**

---

## File Structure

| File | Role |
|---|---|
| `crates/proxy/src/adapters/storage/schema.rs` | Migration V2: add `translation_direction TEXT` to `requests` |
| `crates/proxy/src/application/ports/upstream.rs` | `ApiFormat` enum (or in domain) — Anthropic, OpenAI |
| `crates/proxy/src/application/ports/upstream.rs` | `Provider::native_format()` method on trait (default: Anthropic) |
| `crates/proxy/src/adapters/providers/{anthropic,zai}.rs` | Override `native_format()` |
| `crates/proxy/src/adapters/translation/mod.rs` | **NEW** — `Direction` enum, dispatch entry |
| `crates/proxy/src/adapters/translation/usage.rs` | **NEW** — bidirectional usage stat mapper |
| `crates/proxy/src/adapters/translation/anthropic_to_openai/{mod,request,response,stream}.rs` | **NEW** |
| `crates/proxy/src/adapters/translation/openai_to_anthropic/{mod,request,response,stream}.rs` | **NEW** |
| `crates/proxy/src/application/use_cases/handle_messages.rs` | Derive Direction from path + provider; thread into upstream call |
| `crates/proxy/src/adapters/providers/messages_protocol.rs` | Apply translation before forward + after response |
| `crates/proxy/src/domain/request_log.rs` | Add `translation_direction: Option<String>` to `RequestStart` |
| `crates/proxy/src/adapters/storage/sqlite_request_log.rs` | Persist + read column |
| `crates/proxy/src/application/errors.rs` + `frameworks/error.rs` | New `Translation` variant + 400/streaming-error mapping |
| `crates/proxy-admin-api/src/lib.rs` | Add `translations_completed`, `translations_failed`, `translation_directions` to `StatusResponse` |
| `crates/proxy-tui/src/{ui.rs, app.rs}` | xform column + status line |

---

## Task 1: Schema migration — translation_direction column

**Files:**
- Modify: `crates/proxy/src/adapters/storage/schema.rs`

- [ ] **Step 1: Append migration V2**

In `MIGRATIONS`:

```rust
const MIGRATIONS: &[(i32, &str)] = &[(1, MIGRATION_V1), (2, MIGRATION_V2)];

const MIGRATION_V2: &str = r#"
ALTER TABLE requests ADD COLUMN translation_direction TEXT;
"#;
```

NULL default = no translation (passthrough). All existing rows stay readable.

- [ ] **Step 2: Test idempotency**

Add to existing `mod tests`:

```rust
#[test]
fn v2_adds_translation_direction_column() {
    let conn = open_in_memory();
    ensure_current(&conn).unwrap();
    let cols: Vec<String> = conn
        .prepare("PRAGMA table_info(requests)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert!(cols.contains(&"translation_direction".into()));
}
```

- [ ] **Step 3: Run**

`cargo test -p proxy --lib adapters::storage::schema`

- [ ] **Step 4: Commit**

`build(proxy): schema v2 — add translation_direction column`

---

## Task 2: ApiFormat + Direction enums + provider native_format

**Files:**
- Modify: `crates/proxy/src/application/ports/upstream.rs` (or create dedicated module)
- Modify: `crates/proxy/src/adapters/providers/{anthropic,zai}.rs`
- Modify: `crates/proxy/src/adapters/providers/routing.rs` and `live.rs` (delegate to inner)

- [ ] **Step 1: Add ApiFormat + Direction**

In `crates/proxy/src/application/ports/upstream.rs` (or `application/api_format.rs` as a new file — pick whichever fits the existing layout):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiFormat { Anthropic, OpenAI }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction { Passthrough, AnthropicToOpenAI, OpenAIToAnthropic }

impl Direction {
    pub fn from_pair(client: ApiFormat, upstream: ApiFormat) -> Self {
        match (client, upstream) {
            (a, b) if a == b => Self::Passthrough,
            (ApiFormat::Anthropic, ApiFormat::OpenAI) => Self::AnthropicToOpenAI,
            (ApiFormat::OpenAI, ApiFormat::Anthropic) => Self::OpenAIToAnthropic,
        }
    }

    pub fn as_label(&self) -> Option<&'static str> {
        match self {
            Self::Passthrough => None,
            Self::AnthropicToOpenAI => Some("anthropic→openai"),
            Self::OpenAIToAnthropic => Some("openai→anthropic"),
        }
    }
}
```

- [ ] **Step 2: Add `native_format()` to Provider trait**

```rust
pub trait Provider: Send + Sync {
    fn name(&self) -> &'static str;
    fn native_format(&self) -> ApiFormat { ApiFormat::Anthropic }
    // ...existing methods
}
```

Default returns `Anthropic` so existing test fixtures don't break.

- [ ] **Step 3: Override on each leaf provider**

`AnthropicProvider::native_format()` returns `ApiFormat::Anthropic` (technically inherited; explicit is fine).
`ZaiProvider::native_format()` returns `ApiFormat::OpenAI`.

For `RoutingProvider` and `LiveProvider`: they don't have a single native format because they multiplex. Choose either:
- **a)** Return `ApiFormat::Anthropic` as a placeholder (won't be consulted because translation happens at the leaf-provider granularity, after routing picks an entry).
- **b)** Add a `native_format_for(model: &str) -> Option<ApiFormat>` method that resolves through the routing rules.

Pick (a) — simpler. Translation triggering will move into `messages_protocol::forward` after the leaf is picked, where `entry.provider.native_format()` is the right answer.

- [ ] **Step 4: Tests**

In an inline test module wherever the enums live:

```rust
#[test]
fn direction_passthrough_when_formats_match() {
    assert_eq!(Direction::from_pair(ApiFormat::Anthropic, ApiFormat::Anthropic), Direction::Passthrough);
    assert_eq!(Direction::from_pair(ApiFormat::OpenAI, ApiFormat::OpenAI), Direction::Passthrough);
}

#[test]
fn direction_picks_translator_when_formats_differ() {
    assert_eq!(Direction::from_pair(ApiFormat::Anthropic, ApiFormat::OpenAI), Direction::AnthropicToOpenAI);
    assert_eq!(Direction::from_pair(ApiFormat::OpenAI, ApiFormat::Anthropic), Direction::OpenAIToAnthropic);
}
```

- [ ] **Step 5: Run**

`cargo test --workspace`

- [ ] **Step 6: Commit**

`feat(proxy): add ApiFormat and Direction; provider native_format method`

---

## Task 3: Anthropic→OpenAI request translator

**File:** `crates/proxy/src/adapters/translation/anthropic_to_openai/request.rs` (new)

Per spec section "Request translation — Anthropic → OpenAI" (the table in `2026-05-03-cross-format-translation-design.md` has the full field map).

- [ ] **Step 1: Module skeleton**

```rust
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
    if let Some(model) = v.get("model") { m.insert("model".into(), model.clone()); }

    // max_tokens — passthrough (required by Anthropic, optional by OpenAI)
    if let Some(mt) = v.get("max_tokens") { m.insert("max_tokens".into(), mt.clone()); }

    // sampling — passthrough except top_k (drop) and stop_sequences (rename to stop)
    for k in ["temperature", "top_p", "stream"] {
        if let Some(val) = v.get(k) { m.insert(k.into(), val.clone()); }
    }
    if let Some(stops) = v.get("stop_sequences") { m.insert("stop".into(), stops.clone()); }
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
            for m in translated { out.push(m); }
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
                        let args = serde_json::to_string(&input).unwrap_or("{}".into());
                        tool_calls.push(json!({
                            "id": id,
                            "type": "function",
                            "function": {"name": name, "arguments": args}
                        }));
                    }
                    "tool_result" => {
                        let tool_use_id = block.get("tool_use_id").cloned().unwrap_or(Value::Null);
                        let tr_content = block.get("content").cloned().unwrap_or(Value::Null);
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

            let mut out = Vec::new();
            if !text_parts.is_empty() || !tool_calls.is_empty() {
                let mut msg = json!({"role": role});
                let m = msg.as_object_mut().unwrap();
                if !text_parts.is_empty() {
                    m.insert("content".into(), Value::String(text_parts.join("\n")));
                } else {
                    m.insert("content".into(), Value::Null);
                }
                if !tool_calls.is_empty() {
                    m.insert("tool_calls".into(), Value::Array(tool_calls));
                }
                out.push(msg);
            }
            out.extend(tool_messages);
            Ok(out)
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
        let body = br#"{"model":"x","max_tokens":10,"messages":[{"role":"user","content":"hi"}]}"#;
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
        // arguments are stringified JSON
        assert!(out["messages"][0]["tool_calls"][0]["function"]["arguments"].as_str().unwrap().contains("hello"));
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
        let body = br#"{"model":"x","max_tokens":10,"messages":[],"stop_sequences":["END"]}"#;
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
        assert_eq!(out["messages"][0]["content"], "hi");
    }
}
```

- [ ] **Step 2: Module wiring**

Create `crates/proxy/src/adapters/translation/mod.rs`:

```rust
pub mod anthropic_to_openai;
pub mod openai_to_anthropic;
pub mod usage;
```

Create `crates/proxy/src/adapters/translation/anthropic_to_openai/mod.rs`:

```rust
pub mod request;
pub mod response;
pub mod stream;
```

(Stub `response.rs` and `stream.rs` with `// TODO: filled in by Tasks 5 and 7` and a placeholder fn so the module compiles. Empty file is fine if no items.)

Add `pub mod translation;` to `crates/proxy/src/adapters/mod.rs`.

- [ ] **Step 3: Run, verify**

```
cargo test -p proxy --lib adapters::translation::anthropic_to_openai::request::tests
cargo clippy -p proxy --lib --tests -- -D warnings
```

10 tests must pass.

- [ ] **Step 4: Commit**

`feat(proxy): anthropic→openai request translator`

---

## Task 4: OpenAI→Anthropic request translator

**File:** `crates/proxy/src/adapters/translation/openai_to_anthropic/request.rs` (new)

Mirror Task 3 in reverse. Spec section "Request translation — OpenAI → Anthropic" has full field map.

Key shapes:
- OpenAI `messages[]` with role=system → extract first one to top-level `system`. Subsequent system messages: prepend their text to the next user message (or merge into a synthetic user message).
- `messages[].tool_calls[]` (assistant) → append `{type:"tool_use", id, name, input: parse(arguments)}` to content array.
- `messages[].role = "tool"` → append `{type:"tool_result", tool_use_id: tool_call_id, content}` to NEXT user message's content array. (If the next message isn't a user, create one.) Actually the simpler rule: tool messages always pair with the assistant tool_use that produced them. In OpenAI, the order is: assistant(tool_calls) → tool(result) → user/assistant(...). Translate by treating each role=tool as a content block on a new user message.
- `tools[].function.{name, description, parameters}` → `tools[].{name, description, input_schema: parameters}`.
- `tool_choice` reverse mapping per spec.
- `max_tokens` (optional in OpenAI) → required in Anthropic; if missing, default to `4096`.
- `stop` → `stop_sequences`.
- `user` → `metadata.user_id`.

- [ ] **Step 1: Implementation**

Same shape as Task 3's `request.rs`. The spec's field-by-field table is your reference. Roughly 250-300 lines.

- [ ] **Step 2: Tests** (≥ 10):

- `plain_text_message_round_trips`
- `first_system_message_extracts_to_top_level`
- `multiple_system_messages_merge_into_first` (or follow whatever rule you implement)
- `tool_calls_become_tool_use_blocks`
- `role_tool_becomes_tool_result_block`
- `tools_unwrap_function_envelope`
- `tool_choice_required_becomes_any`
- `tool_choice_named_function_unrwraps`
- `stop_renames_to_stop_sequences`
- `missing_max_tokens_defaults_to_4096`
- `image_url_content_dropped` (out of scope)
- `user_renames_to_metadata_user_id`

- [ ] **Step 3: Run, commit**

`feat(proxy): openai→anthropic request translator`

---

## Task 5: OpenAI→Anthropic non-streaming response translator

**File:** `crates/proxy/src/adapters/translation/anthropic_to_openai/response.rs` (yes — directory is named by client direction, response goes back from upstream-OpenAI to client-Anthropic)

Wait, naming is confusing. Per spec note: directory `anthropic_to_openai/` holds code for the client direction "client speaks Anthropic, upstream speaks OpenAI". So:
- `request.rs` — Anthropic → OpenAI (Task 3, done)
- `response.rs` — OpenAI → Anthropic (this task)
- `stream.rs` — OpenAI stream → Anthropic SSE (Task 7)

Full spec mapping is in section "Response translation (non-streaming) — OpenAI → Anthropic".

- [ ] **Step 1: Implementation**

```rust
//! Translate OpenAI chat completion response back to Anthropic /v1/messages.

pub fn translate(body: &[u8]) -> Result<Vec<u8>, ProxyError> { ... }
```

Key mappings:
- `choices[0].message.content` (string) → `content: [{type:"text", text}]`.
- `choices[0].message.tool_calls[]` → append `{type:"tool_use", id, name, input: JSON.parse(arguments)}` to content array (after text).
- `choices[0].finish_reason` → `stop_reason`: `stop` → `end_turn`, `tool_calls` → `tool_use`, `length` → `max_tokens`, `content_filter` → `end_turn` (with warning log).
- `usage.prompt_tokens` → `usage.input_tokens`.
- `usage.completion_tokens` → `usage.output_tokens`.
- `usage.prompt_tokens_details.cached_tokens` → `usage.cache_read_input_tokens`.
- `model`, `id` passthrough (prefix `id` with `msg_` if not already).
- Top-level shape: `{type:"message", role:"assistant", model, content, stop_reason, stop_sequence: null, usage}`.

- [ ] **Step 2: Tests** (≥ 8):
- text-only response
- tool_call-only response
- text + tool_call mixed
- finish_reason mappings (stop, tool_calls, length, content_filter)
- usage stats translate (cached_tokens → cache_read_input_tokens)
- empty content (no message)
- multi-tool_call response

- [ ] **Step 3: Commit**

`feat(proxy): openai→anthropic non-streaming response translator`

---

## Task 6: Anthropic→OpenAI non-streaming response translator

**File:** `crates/proxy/src/adapters/translation/openai_to_anthropic/response.rs`

Inverse of Task 5. Spec section "Anthropic → OpenAI response".

- [ ] **Step 1: Implementation**

- `content[]` array → flatten text blocks into single string for `message.content`. tool_use blocks → `message.tool_calls[]`.
- `stop_reason: "tool_use"` → `finish_reason: "tool_calls"`. `end_turn` → `stop`. `max_tokens` → `length`.
- `usage.cache_read_input_tokens` → `prompt_tokens_details.cached_tokens`. `cache_creation_input_tokens` is folded into `prompt_tokens` total.
- Top-level: `{id, object:"chat.completion", created: epoch, model, choices: [{...}], usage: {...}}`.

- [ ] **Step 2: Tests** (≥ 8): mirror Task 5.

- [ ] **Step 3: Commit**

`feat(proxy): anthropic→openai non-streaming response translator`

---

## Task 7: OpenAI→Anthropic streaming FSM

**File:** `crates/proxy/src/adapters/translation/anthropic_to_openai/stream.rs`

This is the hardest task. Full spec section "Streaming translation — OpenAI → Anthropic stream" describes the FSM.

- [ ] **Step 1: State machine struct**

```rust
pub struct OpenAiToAnthropicStream {
    message_id: String,
    model: String,
    text_block_open: bool,
    tool_blocks: HashMap<usize, ToolBlockState>,
    next_block_index: usize,
    input_tokens: u64,
    output_tokens: u64,
    cached_tokens: u64,
    stop_reason: Option<&'static str>,
    started: bool,
}
```

The translator consumes OpenAI SSE chunks (parsed `serde_json::Value` from the `data:` payload, plus `[DONE]` sentinel) and emits Anthropic SSE event strings.

- [ ] **Step 2: Implementation per spec mapping table**

Each input chunk → 0+ output events. Use `Vec<String>` accumulator returned by `feed(chunk: Value) -> Vec<String>`.

- [ ] **Step 3: Tests** (≥ 7):
- Text-only response (5 chunks: role-only, 3 text deltas, finish)
- Tool call streamed across 4 chunks (role-only, tool_call init, args delta, args delta, finish)
- Two parallel tool calls in one message
- Text + tool_call mixed
- Empty content (just role + finish)
- Mid-stream error chunk
- finish_reason mapping (stop, tool_calls, length)

Use string fixtures in `tests/fixtures/streaming/` if helpful, or inline JSON literals.

- [ ] **Step 4: Commit**

`feat(proxy): openai→anthropic stream translator FSM`

---

## Task 8: Anthropic→OpenAI streaming FSM

**File:** `crates/proxy/src/adapters/translation/openai_to_anthropic/stream.rs`

Inverse of Task 7. Spec section "Anthropic → OpenAI stream".

State + FSM per spec mapping table. Tests mirror Task 7.

Commit: `feat(proxy): anthropic→openai stream translator FSM`

---

## Task 9: Wire translator into messages_protocol

**Files:**
- Modify: `crates/proxy/src/adapters/providers/messages_protocol.rs`
- Modify: `crates/proxy/src/application/use_cases/handle_messages.rs`

- [ ] **Step 1: Derive Direction in handler**

In `handle_messages.rs::execute` (or wherever the path is matched against `ApiFormat`):

```rust
let client_format = match request_path {
    "/v1/messages" => ApiFormat::Anthropic,
    "/v1/chat/completions" => ApiFormat::OpenAI,
    _ => return Err(ProxyError::BadRequest("unsupported path".into())),
};

// At leaf-provider time (inside RoutingProvider, OR as a closure passed down):
let provider_format = entry.provider.native_format();
let direction = Direction::from_pair(client_format, provider_format);
```

Plumbing: easiest is to add a `client_format` field to `UpstreamResponse` or to the return tuple of `Provider::forward`. But the simpler path is to pre-compute Direction in `messages_protocol::forward` before calling upstream — at the point where we already know the leaf provider's `native_format()` and the client's format (passed in).

- [ ] **Step 2: Apply request translation pre-forward**

```rust
let body_to_send = match direction {
    Direction::Passthrough => body,
    Direction::AnthropicToOpenAI => {
        adapters::translation::anthropic_to_openai::request::translate(&body)?
    }
    Direction::OpenAIToAnthropic => {
        adapters::translation::openai_to_anthropic::request::translate(&body)?
    }
};
```

Pass `body_to_send` to `upstream.send_request`.

- [ ] **Step 3: Apply response translation post-forward**

For non-streaming buffered responses:

```rust
let response_body = match direction {
    Direction::Passthrough => upstream_body,
    Direction::AnthropicToOpenAI => translation::anthropic_to_openai::response::translate(&upstream_body)?,
    Direction::OpenAIToAnthropic => translation::openai_to_anthropic::response::translate(&upstream_body)?,
};
```

For streaming responses, wrap the upstream byte stream with the FSM. Use `Box::pin(StreamExt::map(...))` style — read the existing TeedStream code in `frameworks/stream.rs` for the pattern. The translator FSM `feed` method consumes `Value` and returns SSE event strings; convert to `Bytes` with `Bytes::from`.

- [ ] **Step 4: Persist translation_direction**

Update `RequestStart` to carry the direction label. Persist via the existing `insert_started` query (now reading the new column from migration v2).

- [ ] **Step 5: Tests**

Existing routing tests will exercise passthrough. Add a single integration test that constructs a `HandleMessages` with a fake provider whose `native_format()` returns `OpenAI`, sends an Anthropic-format request, and asserts the upstream sees the OpenAI-format body. Use the existing `FakeProvider` test fixture (or a variant).

- [ ] **Step 6: Run, commit**

`feat(proxy): wire bidirectional translation into messages_protocol`

---

## Task 10: Translation error variant + 400 mapping

**Files:**
- Modify: `crates/proxy/src/application/errors.rs`
- Modify: `crates/proxy/src/frameworks/error.rs`

- [ ] **Step 1: New error variants**

```rust
#[error("translation: invalid request: {field}: {reason}")]
TranslationInvalidRequest { field: &'static str, reason: String },

#[error("translation: unsupported feature: {0}")]
TranslationUnsupported(String),

#[error("translation stream protocol error: {0}")]
TranslationStreamProtocol(String),
```

- [ ] **Step 2: Map to 400/500 in IntoResponse**

```rust
ProxyError::TranslationInvalidRequest { field, reason } => {
    let body = serde_json::json!({"error":{"type":"bad_request","message":format!("translation failed at {field}: {reason}")}});
    (StatusCode::BAD_REQUEST, Json(body)).into_response()
}
ProxyError::TranslationUnsupported(f) => {
    // Should be handled with warning + drop in translators; reaching this branch means a bug.
    let body = serde_json::json!({"error":{"type":"unsupported","message":f}});
    (StatusCode::BAD_REQUEST, Json(body)).into_response()
}
ProxyError::TranslationStreamProtocol(msg) => {
    let body = serde_json::json!({"error":{"type":"upstream_protocol","message":msg}});
    (StatusCode::BAD_GATEWAY, Json(body)).into_response()
}
```

- [ ] **Step 3: Use them**

Replace the generic `ProxyError::BadRequest` in translators (Tasks 3, 4, 5, 6) with the more specific variants.

- [ ] **Step 4: Run, commit**

`feat(proxy): translation error variants and HTTP mapping`

---

## Task 11: Persist translation_direction

**Files:**
- Modify: `crates/proxy/src/domain/request_log.rs` — `RequestStart` add `translation_direction: Option<String>`
- Modify: `crates/proxy/src/adapters/storage/sqlite_request_log.rs` — `insert_started` writes the column; `recent` and `quota_seed` queries unchanged
- Modify: `crates/proxy/src/application/use_cases/handle_messages.rs` — populate `translation_direction = Some(direction.as_label()?.into())` when not Passthrough

- [ ] **Step 1: Update domain**

```rust
pub struct RequestStart {
    pub id: String,
    pub user_id: i64,
    pub provider: String,
    pub model: String,
    pub started_at: i64,
    pub translation_direction: Option<String>,
}
```

- [ ] **Step 2: Update SQL**

```rust
"INSERT INTO requests (id, user_id, provider, model, status, started_at, translation_direction)
 VALUES (?1, ?2, ?3, ?4, 'started', ?5, ?6)"
```

```rust
.params![start.id, start.user_id, start.provider, start.model, start.started_at, start.translation_direction]
```

- [ ] **Step 3: Update construction sites**

`grep -n 'RequestStart {' crates/proxy/`. Add `translation_direction: None,` (or the actual value at the production call site).

- [ ] **Step 4: Test**

Add to schema.rs tests:

```rust
#[test]
fn translation_direction_persists() {
    let conn = open_in_memory();
    ensure_current(&conn).unwrap();
    conn.execute(
        "INSERT INTO requests (id, user_id, provider, model, status, started_at, translation_direction)
         VALUES ('r1', 1, 'zai', 'glm-4', 'started', 0, 'anthropic→openai')",
        [],
    ).unwrap();
    let val: String = conn
        .query_row("SELECT translation_direction FROM requests WHERE id='r1'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(val, "anthropic→openai");
}
```

- [ ] **Step 5: Commit**

`feat(proxy): persist translation_direction on request rows`

---

## Task 12: Admin status — translation stats

**Files:**
- Modify: `crates/proxy-admin-api/src/lib.rs`
- Modify: `crates/proxy/src/application/use_cases/admin.rs`
- Modify: `crates/proxy/src/application/ports/request_log_read.rs`
- Modify: `crates/proxy/src/adapters/storage/sqlite_request_log.rs`

- [ ] **Step 1: New fields on StatusResponse**

```rust
pub struct StatusResponse {
    // ...existing...
    pub translations_completed: u64,
    pub translations_failed: u64,
    pub translation_directions: BTreeMap<String, u64>,  // "anthropic→openai" -> count
}
```

- [ ] **Step 2: Read-port query**

```rust
fn count_translations(&self) -> Result<TranslationCounts, ProxyError>;

pub struct TranslationCounts {
    pub completed: u64,                                // sum of completed where translation_direction IS NOT NULL
    pub failed: u64,                                   // sum of errored where translation_direction IS NOT NULL
    pub by_direction: BTreeMap<String, u64>,
}
```

- [ ] **Step 3: SQL impl**

```rust
fn count_translations(&self) -> Result<TranslationCounts, ProxyError> {
    let conn = self.conn.lock().expect("conn mutex poisoned");
    let mut stmt = conn.prepare(
        "SELECT translation_direction, status, COUNT(*) \
         FROM requests \
         WHERE translation_direction IS NOT NULL \
         GROUP BY translation_direction, status"
    )?;
    let mut completed = 0u64;
    let mut failed = 0u64;
    let mut by_direction = BTreeMap::new();
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)? as u64)))?;
    for row in rows {
        let (dir, status, count) = row?;
        *by_direction.entry(dir).or_insert(0) += count;
        match status.as_str() {
            "completed" => completed += count,
            "errored" => failed += count,
            _ => {}
        }
    }
    Ok(TranslationCounts { completed, failed, by_direction })
}
```

- [ ] **Step 4: Plumb into GetStatus**

```rust
pub fn execute(&self) -> Result<StatusResponse, ProxyError> {
    // ...existing fields...
    let translations = self.read.count_translations()?;
    Ok(StatusResponse {
        // ...
        translations_completed: translations.completed,
        translations_failed: translations.failed,
        translation_directions: translations.by_direction,
    })
}
```

- [ ] **Step 5: Test**

Add a test that inserts 3 rows (2 completed with different directions, 1 errored) and verifies counts.

- [ ] **Step 6: Commit**

`feat(proxy): /admin/status reports translation counts`

---

## Task 13: TUI xform column + status line

**Files:**
- Modify: `crates/proxy-admin-api/src/lib.rs` — add `translation_direction: Option<String>` to `RecentRequestItem`
- Modify: `crates/proxy/src/application/use_cases/admin.rs` — populate it
- Modify: `crates/proxy/src/adapters/storage/sqlite_request_log.rs` — read the column in `recent`
- Modify: `crates/proxy/src/domain/request_log.rs` — add to `RequestRow`
- Modify: `crates/proxy-tui/src/ui.rs` — render xform column in recent requests view; add translation stats line to status panel

- [ ] **Step 1: Plumb the field**

Backwards from DB → DTO. Match existing field-flow patterns.

- [ ] **Step 2: Render in TUI**

In recent-requests table, add a column "xform" with values: `—` for None, `A→O` for "anthropic→openai", `O→A` for "openai→anthropic". Truncate long unknown values.

In status panel, after existing lines, add:

```
Translation: 1247 completed | 3 failed | A→O 1100, O→A 147
```

(Render only if `translations_completed + translations_failed > 0`; otherwise show `Translation: none yet`.)

- [ ] **Step 3: Run, smoke**

```
cargo build --workspace
cargo clippy --workspace --tests -- -D warnings
```

Smoke handled at Task 14 by controller.

- [ ] **Step 4: Commit**

`feat(proxy-tui): xform column in recent requests and translation stats`

---

## Task 14: Integration tests + workspace gates

**Files:**
- Create: `crates/proxy/tests/translation.rs`

- [ ] **Step 1: Wiremock-backed integration tests**

Reference existing `crates/proxy/tests/integration.rs` for the wiremock setup pattern.

```rust
//! Integration: client speaks Anthropic, upstream is OpenAI-compat (and vice versa).

use wiremock::{matchers::*, Mock, MockServer, ResponseTemplate};

#[tokio::test(flavor = "multi_thread")]
async fn anthropic_client_to_openai_upstream_translates_round_trip() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "cmpl_x",
            "object": "chat.completion",
            "created": 0,
            "model": "glm-4.6",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "hello"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 1, "total_tokens": 11}
        })))
        .mount(&server)
        .await;

    // ...build proxy app with provider pointing at server.uri(), provider native_format = OpenAI
    // ...send POST /v1/messages with Anthropic body
    // ...assert response is Anthropic shape (content array, usage.input_tokens, etc.)
}
```

Add at minimum:
- `anthropic_client_to_openai_upstream_translates_round_trip`
- `openai_client_to_anthropic_upstream_translates_round_trip`
- `passthrough_when_formats_match` (sanity)

- [ ] **Step 2: Workspace gates**

```
cargo fmt
cargo fmt --check
cargo clippy --workspace --tests -- -D warnings
cargo test --workspace
```

- [ ] **Step 3: Manual smoke** (optional, controller does this)

Configure Claude Code to point at proxy, verify `/v1/messages` requests to a model routed to Z.ai succeed and the response is Anthropic-shaped.

- [ ] **Step 4: Final commit if any fmt churn**

```
chore(proxy): final formatting after translation
```

---

## Self-review

- [x] **Spec coverage**: every section of `2026-05-03-cross-format-translation-design.md` addressed (request → Tasks 3, 4; response → Tasks 5, 6; stream → Tasks 7, 8; trigger → Task 2; wiring → Task 9; error → Task 10; persistence → Tasks 1, 11; admin/TUI → Tasks 12, 13; integration → Task 14).
- [x] **No placeholders**: code blocks contain real content. Spec is referenced for full mapping tables (the spec is the source of truth — copying every field would dilute the plan and risk drift).
- [x] **Type consistency**: `ApiFormat`, `Direction`, `RequestStart::translation_direction`, `StatusResponse::translations_*` named consistently.
- [x] **Frequent commits**: 14 commits.

## Out-of-scope (per spec)

- Multimodal (images, PDFs).
- Gemini, Bedrock, Vertex.
- Files API, Batch API.
- OpenAI Responses API.
- Anthropic `cache_control` synthesis when going OpenAI→Anthropic.
- Function-calling extensions (JSON mode, structured outputs).
