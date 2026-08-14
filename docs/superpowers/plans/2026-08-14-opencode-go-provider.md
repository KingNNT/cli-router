# OpenCode Go Provider Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an `opencode_go` provider kind that serves all 19 OpenCode Go models through one provider entry, picking the upstream wire format per model.

**Architecture:** OpenCode Go serves different models on different endpoints of the same base (`/chat/completions`, `/messages`, `/responses`). A config-stored glob table (`model_formats`) maps model → wire format; `Provider::supported_formats_for(model)` feeds that into the existing routing/translation decision. The Responses endpoint reuses `CodexProvider`'s Chat→Responses translation, extracted into a shared module parameterized by a `ResponsesDialect`.

**Tech Stack:** Rust (edition 2024), axum, reqwest, tokio, serde_json, globset, rusqlite, ratatui, wiremock (dev).

**Spec:** `docs/superpowers/specs/2026-08-14-opencode-go-provider-design.md`

## Global Constraints

- All crates are **edition 2024**: prefer let-chains (`if let Some(x) = o && cond`), clippy runs with `-D warnings`.
- Ring dependencies point inward only: `frameworks → adapters → application → domain`. Never import `frameworks` from `adapters`, or `adapters` from `application`.
- No new dependencies. Everything needed (`globset`, `serde_json`, `futures`) is already in `crates/proxy/Cargo.toml`.
- Error types are per-ring and use `thiserror`; do not add `anyhow`.
- `proxy-tui` must **not** depend on the `proxy` crate. Tables that exist in both (kind labels, default URLs, thinking levels) are duplicated by hand — keep the doc comment that says so.
- Config lives in SQLite and is the single source of truth. New config fields need: `config.rs` field → `schema.rs` migration → `db_config.rs` read/write → `proxy-admin-api` DTO → `admin.rs` mapping → `proxy-tui` form.
- Compact rule-string grammar, used verbatim everywhere (DB column, DTO, TUI field):
  `glob=format[,glob=format]*` with `format ∈ {anthropic, openai, responses}`.
- Preset seed value for the `opencode_go` kind, used verbatim:
  `minimax-*=anthropic,qwen3.*=anthropic,grok-4.5=responses,gpt-5.6-luna=responses`
- Base URLs for the kind, used verbatim:
  anthropic `https://opencode.ai/zen/go`, openai `https://opencode.ai/zen/go/v1`.
- **The pre-commit hook runs the full `cargo test --workspace` and takes >10 minutes.** Run the fast filtered test during the TDD loop (`cargo test -p proxy <filter>`), and run `git commit` as a background command so it isn't killed by a foreground timeout.
- Commit messages follow Conventional Commits. **Never** add `Co-Authored-By` or generated-by trailers.
- Work happens on branch `feature/opencode-go-provider` (already created, spec already committed there).

---

### Task 1: Extract the Responses translation behind a dialect

Pure refactor. `CodexProvider` keeps every current behavior; its existing tests are the regression guard. Nothing about OpenCode Go is wired yet.

**Files:**
- Create: `crates/proxy/src/adapters/translation/openai_to_responses.rs`
- Modify: `crates/proxy/src/adapters/translation/mod.rs`
- Modify: `crates/proxy/src/adapters/providers/codex.rs` (delete lines 376–1014 worth of moved items, rewire call sites)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub struct ResponsesDialect { pub force_stream: bool, pub store: Option<bool>, pub include_encrypted_reasoning: bool, pub drop_max_output_tokens: bool, pub default_instructions: &'static str }`
  - `impl ResponsesDialect { pub const fn codex() -> Self; pub const fn vanilla() -> Self }`
  - `pub fn translate_request(chat: &serde_json::Value, dialect: &ResponsesDialect) -> Result<serde_json::Value, String>`
  - `pub fn translate_buffered_response(responses_body: &serde_json::Value) -> Result<serde_json::Value, String>`
  - `pub struct ResponsesSseTranslator` with `pub fn new(inner: crate::application::ports::BoxedByteStream) -> Self`, implementing `futures::Stream<Item = Result<Bytes, BoxedError>>`
  - `pub fn translate_tool(tool: &Value) -> Value`, `pub fn text_content_to_string(content: Option<&Value>) -> String`

- [ ] **Step 1: Move the code verbatim into the new module**

Cut these items out of `crates/proxy/src/adapters/providers/codex.rs` and paste them into the new file `crates/proxy/src/adapters/translation/openai_to_responses.rs`, making each one `pub`:

| item | current line |
|---|---|
| `fn translate_tool` | 376 |
| `fn text_content_to_string` | 427 |
| `fn translate_request` | 454 |
| `fn translate_buffered_response` | 573 |
| `struct ResponsesSseTranslator` + `impl ResponsesSseTranslator` + `impl Stream for ResponsesSseTranslator` | 688, 711, 1016 |

Delete codex's local `type BoxedByteStream` (line 683) and use the canonical one instead. Module header:

```rust
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
```

Register it in `crates/proxy/src/adapters/translation/mod.rs`:

```rust
pub mod anthropic_to_openai;
pub mod openai_to_anthropic;
pub mod openai_to_responses;
pub mod stream_wrap;
```

- [ ] **Step 2: Add the dialect type**

Append to the new module, above `translate_request`:

```rust
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
    /// Used when the request has no system/developer message.
    pub default_instructions: &'static str,
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
            default_instructions: "You are a helpful assistant.",
        }
    }

    /// A plain OpenAI-compatible Responses endpoint (OpenCode Go).
    pub const fn vanilla() -> Self {
        Self {
            force_stream: false,
            store: None,
            include_encrypted_reasoning: false,
            drop_max_output_tokens: false,
            default_instructions: "You are a helpful assistant.",
        }
    }
}
```

- [ ] **Step 3: Write the failing tests for the dialect**

Append to `openai_to_responses.rs`:

```rust
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
```

- [ ] **Step 4: Run the tests to verify they fail**

Run: `cargo test -p proxy openai_to_responses`
Expected: compile error — `translate_request` still takes one argument.

- [ ] **Step 5: Make `translate_request` dialect-driven**

Change its signature and the four hardcoded spots. Everything else in the function stays exactly as it was:

```rust
pub fn translate_request(chat: &Value, dialect: &ResponsesDialect) -> Result<Value, String> {
    // ...unchanged body up to the instructions default...
    let mut instructions = Value::String(dialect.default_instructions.to_string());

    // ...unchanged message loop, `out.insert("instructions"...)`, `out.insert("input"...)`,
    // and the reasoning_effort → reasoning.effort mapping...

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

    // ...unchanged tools / tool_choice / temperature passthrough...
}
```

- [ ] **Step 6: Rewire `CodexProvider` and its tests**

In `codex.rs`, add the import and pass the Codex dialect at every call site:

```rust
use crate::adapters::translation::openai_to_responses::{
    ResponsesDialect, ResponsesSseTranslator, translate_buffered_response, translate_request,
};
```

In `forward_openai` (line ~144):

```rust
let responses_body = translate_request(&chat_body, &ResponsesDialect::codex())
    .map_err(|e| ProxyError::BadRequest(format!("codex request translation failed: {e}")))?;
```

In `codex.rs`'s `mod tests`, update every `translate_request(&chat)` call to
`translate_request(&chat, &ResponsesDialect::codex())`. Leave the assertions untouched — they are the regression guard. Tests that only exercise `translate_tool` / `text_content_to_string` / the SSE translator move to the new module together with a `use crate::adapters::translation::openai_to_responses::*;` import, or stay in `codex.rs` importing the same path; either is fine as long as none is deleted.

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -p proxy openai_to_responses && cargo test -p proxy codex`
Expected: PASS, with every pre-existing codex test still present and green.

- [ ] **Step 8: Lint**

Run: `cargo clippy -p proxy -- -D warnings && cargo fmt`
Expected: clean.

- [ ] **Step 9: Commit**

```bash
git add crates/proxy/src/adapters/translation/ crates/proxy/src/adapters/providers/codex.rs
git commit -m "refactor(proxy): extract the Responses translation behind a dialect

CodexProvider was the only holder of Chat Completions -> Responses
translation, with the ChatGPT backend's quirks hardcoded into it. The
translation now lives in adapters/translation and takes a ResponsesDialect,
so other providers with a /responses endpoint can reuse it."
```

---

### Task 2: `model_formats` config field and parser

**Files:**
- Modify: `crates/proxy/src/config.rs`
- Modify (mechanical, add `model_formats: None,`): every `ProviderConfig { … }` literal — 40 sites across `crates/proxy/tests/quota_enforcement.rs`, `crates/proxy/tests/integration.rs`, `crates/proxy/src/config.rs`, `crates/proxy/src/adapters/providers/account_usage/anthropic.rs`, `crates/proxy/src/adapters/storage/db_config.rs`, `crates/proxy/src/application/use_cases/admin.rs`, `crates/proxy/src/adapters/providers/builder.rs`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub enum WireFormat { Anthropic, OpenAi, Responses }` (serde `snake_case`, alias `openai`)
  - `pub fn parse_model_formats(s: &str) -> Result<Vec<(String, WireFormat)>, ConfigError>`
  - `ProviderConfig.model_formats: Option<String>`

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `crates/proxy/src/config.rs`:

```rust
#[test]
fn parse_model_formats_reads_all_three_formats() {
    let rules = parse_model_formats("minimax-*=anthropic, glm-*=openai ,grok-4.5=responses").unwrap();
    assert_eq!(
        rules,
        vec![
            ("minimax-*".to_string(), WireFormat::Anthropic),
            ("glm-*".to_string(), WireFormat::OpenAi),
            ("grok-4.5".to_string(), WireFormat::Responses),
        ]
    );
}

#[test]
fn parse_model_formats_accepts_empty_and_trailing_commas() {
    assert!(parse_model_formats("").unwrap().is_empty());
    assert_eq!(parse_model_formats("glm-*=openai,").unwrap().len(), 1);
}

#[test]
fn parse_model_formats_rejects_unknown_format() {
    let err = parse_model_formats("glm-*=grpc").unwrap_err().to_string();
    assert!(err.contains("glm-*=grpc"), "{err}");
}

#[test]
fn parse_model_formats_rejects_missing_equals() {
    assert!(parse_model_formats("glm-*").is_err());
}

#[test]
fn parse_model_formats_rejects_empty_glob() {
    assert!(parse_model_formats("=openai").is_err());
}

#[test]
fn parse_model_formats_rejects_invalid_glob() {
    assert!(parse_model_formats("gl[m-*=openai").is_err());
}

#[test]
fn validate_rejects_provider_with_bad_model_formats() {
    let mut cfg = Config {
        port: 8787,
        proxy_db: PathBuf::from("/tmp/p.db"),
        pricing_db: PathBuf::from("/tmp/pr.db"),
        providers: vec![ProviderConfig {
            name: "p".into(),
            enabled: true,
            kind: ProviderKind::Zai,
            auth: AuthConfig::default(),
            anthropic_base_url: None,
            openai_base_url: Some("https://example.test/v1".into()),
            format_mode: FormatMode::Both,
            thinking_level: ThinkingLevel::default(),
            thinking_force: false,
            thinking_mode: ThinkingMode::default(),
            max_concurrent: None,
            sanitize_empty_tools: false,
            model_formats: None,
        }],
        routing: vec![RoutingRule {
            match_spec: MatchSpec { model: None },
            provider: "p".into(),
            fallback: vec![],
            strategy: RoutingStrategy::default(),
            priority: 0,
        }],
        affinity: AffinityConfig::default(),
        quota: vec![],
    };
    assert!(cfg.validate().is_ok());
    cfg.providers[0].model_formats = Some("glm-*=nope".into());
    assert!(cfg.validate().is_err());
}
```

If the `RoutingRule` / `MatchSpec` field names in this test don't compile, copy the exact shape from the neighbouring `validate_rejects_*` tests already in the file rather than inventing one.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p proxy config::tests::parse_model_formats`
Expected: FAIL — `parse_model_formats` and `WireFormat` don't exist.

- [ ] **Step 3: Implement the type, the field, and the parser**

In `crates/proxy/src/config.rs`, next to `FormatMode`:

```rust
/// The wire format used to talk to an upstream for one specific model.
/// `Responses` is the OpenAI Responses API (`/responses`), which the proxy
/// only ever speaks upstream — clients never send it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireFormat {
    Anthropic,
    #[serde(alias = "openai")]
    OpenAi,
    Responses,
}
```

Add the field to `ProviderConfig`:

```rust
    /// Per-model wire-format overrides as `glob=format` pairs joined by
    /// commas, e.g. `minimax-*=anthropic,grok-4.5=responses`. First match
    /// wins. Empty or absent means the provider's format is decided by its
    /// configured URLs alone, as before. OpenCode Go is the motivating case:
    /// it serves different models on `/chat/completions`, `/messages` and
    /// `/responses` under one base URL.
    #[serde(default)]
    pub model_formats: Option<String>,
```

And the parser near `parse_kind`:

```rust
/// Parse the compact `glob=format[,glob=format]*` rule string. Blank segments
/// are skipped so a trailing comma is harmless. Globs are compiled here purely
/// to reject bad patterns at save time; the compiled matchers are rebuilt by
/// the provider builder.
pub fn parse_model_formats(s: &str) -> Result<Vec<(String, WireFormat)>, ConfigError> {
    let mut out = Vec::new();
    for segment in s.split(',') {
        let segment = segment.trim();
        if segment.is_empty() {
            continue;
        }
        let invalid = || {
            ConfigError::Validation(format!(
                "invalid model_formats entry '{segment}': expected <glob>=anthropic|openai|responses"
            ))
        };
        let (glob, format) = segment.split_once('=').ok_or_else(invalid)?;
        let glob = glob.trim();
        if glob.is_empty() {
            return Err(invalid());
        }
        let format = match format.trim().to_ascii_lowercase().as_str() {
            "anthropic" => WireFormat::Anthropic,
            "openai" | "open_ai" => WireFormat::OpenAi,
            "responses" => WireFormat::Responses,
            _ => return Err(invalid()),
        };
        globset::Glob::new(glob).map_err(|e| {
            ConfigError::Validation(format!("invalid model_formats glob '{glob}': {e}"))
        })?;
        out.push((glob.to_string(), format));
    }
    Ok(out)
}
```

In `Config::validate()`, inside the existing `for p in &self.providers` loop (after the duplicate-name check):

```rust
            if let Some(rules) = &p.model_formats {
                parse_model_formats(rules)?;
            }
```

- [ ] **Step 4: Fix the 40 struct literals**

Run `rg -n "ProviderConfig \{" crates` and add `model_formats: None,` to each literal. Nothing else changes.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p proxy config::`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/proxy
git commit -m "feat(config): add per-model wire-format overrides

A provider whose upstream serves different models on different endpoints
cannot be described by base URLs alone. model_formats maps a model glob to
a wire format, validated at save time so a bad rule fails in the admin API
rather than on the forward path."
```

---

### Task 3: The `opencode_go` provider kind

**Files:**
- Modify: `crates/proxy/src/config.rs` (`ProviderKind`, `parse_kind`)
- Modify: `crates/proxy/src/adapters/providers/upstream.rs` (`quirks_for`, `default_urls`, plus a new `preset_model_formats`)
- Modify: `crates/proxy/src/adapters/providers/thinking.rs` (`thinking_levels`, `thinking_patch`)
- Modify: `crates/proxy/src/adapters/providers/builder.rs` (account-usage match)
- Modify: `crates/proxy/src/application/use_cases/admin.rs` (`kind_to_str`, `str_to_kind`)
- Modify: `crates/proxy/src/adapters/storage/db_config.rs` (`kind_to_str`, `parse_kind`)

**Interfaces:**
- Consumes: `WireFormat` (Task 2).
- Produces: `ProviderKind::OpencodeGo`; `pub fn preset_model_formats(kind: ProviderKind) -> Option<&'static str>` in `upstream.rs`.

- [ ] **Step 1: Write the failing tests**

In `crates/proxy/src/config.rs` `mod tests`:

```rust
#[test]
fn parse_kind_accepts_opencode_go_aliases() {
    assert_eq!(parse_kind("opencode_go"), Some(ProviderKind::OpencodeGo));
    assert_eq!(parse_kind("opencode-go"), Some(ProviderKind::OpencodeGo));
    assert_eq!(parse_kind("OPENCODE_GO"), Some(ProviderKind::OpencodeGo));
}

#[test]
fn provider_kind_deserializes_opencode_go_aliases() {
    assert_eq!(
        serde_json::from_str::<ProviderKind>("\"opencode_go\"").unwrap(),
        ProviderKind::OpencodeGo
    );
    assert_eq!(
        serde_json::from_str::<ProviderKind>("\"opencode-go\"").unwrap(),
        ProviderKind::OpencodeGo
    );
}
```

In `crates/proxy/src/adapters/providers/upstream.rs` `mod tests`:

```rust
#[test]
fn opencode_go_preset_serves_both_bases_and_seeds_rules() {
    let (anthropic, openai) = default_urls(ProviderKind::OpencodeGo);
    assert_eq!(anthropic, Some("https://opencode.ai/zen/go"));
    assert_eq!(openai, Some("https://opencode.ai/zen/go/v1"));
    assert_eq!(
        preset_model_formats(ProviderKind::OpencodeGo),
        Some("minimax-*=anthropic,qwen3.*=anthropic,grok-4.5=responses,gpt-5.6-luna=responses")
    );
    assert!(preset_model_formats(ProviderKind::Zai).is_none());
}

#[test]
fn opencode_go_preset_has_no_quirks() {
    let q = quirks_for(ProviderKind::OpencodeGo, ThinkingMode::SplitOnly, false);
    assert!(!q.reasoning_split);
    assert!(!q.strip_tool_choice);
    assert!(!q.rename_max_tokens);
    assert!(!q.sanitize_empty_tools);
    assert!(q.strip_thinking.is_none());
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p proxy opencode_go`
Expected: FAIL — no `OpencodeGo` variant.

- [ ] **Step 3: Add the variant and every match arm**

`config.rs`:

```rust
pub enum ProviderKind {
    // ...existing variants...
    #[serde(alias = "opencode-go", alias = "opencode_go")]
    OpencodeGo,
}
```

```rust
fn parse_kind(s: &str) -> Option<ProviderKind> {
    match s.trim().to_ascii_lowercase().as_str() {
        // ...existing arms...
        "opencode_go" | "opencode-go" => Some(ProviderKind::OpencodeGo),
        _ => None,
    }
}
```

`upstream.rs`:

```rust
        ProviderKind::Anthropic | ProviderKind::Zai | ProviderKind::OpencodeGo => Quirks::none(),
```

```rust
        ProviderKind::OpencodeGo => (
            Some("https://opencode.ai/zen/go"),
            Some("https://opencode.ai/zen/go/v1"),
        ),
```

```rust
/// Seed value for `ProviderConfig::model_formats` when a provider of this kind
/// is created. Prefill only — once stored it is plain config the user owns.
///
/// OpenCode Go serves MiniMax and Qwen on its Anthropic endpoint, Grok 4.5 and
/// GPT 5.6 Luna on the Responses endpoint, and everything else (GLM, Kimi,
/// DeepSeek, MiMo, Hy3) on Chat Completions, which is the fallthrough.
pub fn preset_model_formats(kind: ProviderKind) -> Option<&'static str> {
    match kind {
        ProviderKind::OpencodeGo => {
            Some("minimax-*=anthropic,qwen3.*=anthropic,grok-4.5=responses,gpt-5.6-luna=responses")
        }
        _ => None,
    }
}
```

`thinking.rs` — the kind offers no levels, so add `ProviderKind::OpencodeGo` to the arm that returns an empty slice, and to the `thinking_patch` arm that returns `None`. Follow whatever the existing `ProviderKind::Zai` arms do if they already express "nothing offered"; otherwise add explicit arms:

```rust
        ProviderKind::OpencodeGo => &[],
```

`builder.rs`, in the account-usage match:

```rust
                ProviderKind::OpencodeGo => Arc::new(super::account_usage::noop::NoopAccountUsage),
```

`admin.rs`:

```rust
        ProviderKind::OpencodeGo => "opencode_go",
```

```rust
        "opencode_go" | "opencode-go" => Ok(ProviderKind::OpencodeGo),
```

`db_config.rs` — the same two mappings, string `"opencode_go"` both ways (accept `"opencode-go"` when reading).

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p proxy opencode_go && cargo build -p proxy`
Expected: PASS, and the build surfaces any non-exhaustive match left behind.

- [ ] **Step 5: Commit**

```bash
git add crates/proxy
git commit -m "feat(proxy): add the opencode_go provider kind

Presets the two OpenCode Go base URLs and seeds the model_formats rules
that route MiniMax/Qwen to the Anthropic endpoint and Grok 4.5 /
GPT 5.6 Luna to Responses."
```

---

### Task 4: Persist and expose `model_formats`

**Files:**
- Modify: `crates/proxy/src/adapters/storage/schema.rs`
- Modify: `crates/proxy/src/adapters/storage/db_config.rs`
- Modify: `crates/proxy-admin-api/src/lib.rs`
- Modify: `crates/proxy/src/application/use_cases/admin.rs`
- Modify (mechanical): the 12 `ProviderPayload { … }` literals in `crates/proxy-admin-api`, `crates/proxy`, `crates/proxy-tui`

**Interfaces:**
- Consumes: `ProviderConfig.model_formats` (Task 2), `ProviderKind::OpencodeGo` (Task 3).
- Produces: `ProviderPayload.model_formats: Option<String>`; the `providers.model_formats` column.

- [ ] **Step 1: Write the failing tests**

In `crates/proxy/src/adapters/storage/db_config.rs` `mod tests` (copy the setup helpers the neighbouring tests use):

```rust
#[test]
fn model_formats_round_trips_through_the_db() {
    let (repo, _tmp) = test_repo();
    let mut cfg = sample_config();
    cfg.providers[0].model_formats = Some("qwen3.*=anthropic,grok-4.5=responses".into());
    repo.save(&cfg).unwrap();
    let loaded = repo.load().unwrap();
    assert_eq!(
        loaded.providers[0].model_formats.as_deref(),
        Some("qwen3.*=anthropic,grok-4.5=responses")
    );
}

#[test]
fn missing_model_formats_column_value_loads_as_none() {
    let (repo, _tmp) = test_repo();
    let cfg = sample_config();
    repo.save(&cfg).unwrap();
    let loaded = repo.load().unwrap();
    assert!(loaded.providers[0].model_formats.is_none());
}
```

In `crates/proxy/src/application/use_cases/admin.rs` `mod tests`:

```rust
#[test]
fn config_payload_carries_model_formats_both_ways() {
    let payload = ProviderPayload {
        // copy the field list from a neighbouring test literal
        model_formats: Some("glm-*=openai".into()),
        ..sample_provider_payload()
    };
    let cfg = payload_to_provider(&payload).unwrap();
    assert_eq!(cfg.model_formats.as_deref(), Some("glm-*=openai"));
    assert_eq!(
        provider_to_payload(&cfg).model_formats.as_deref(),
        Some("glm-*=openai")
    );
}
```

Use whatever the real conversion function names in `admin.rs` are (they sit next to `kind_to_str` / `auth_to_payload`); do not invent new ones.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p proxy model_formats`
Expected: FAIL — no such column / field.

- [ ] **Step 3: Add the migration**

Append to the migration list in `crates/proxy/src/adapters/storage/schema.rs`, following the exact style of the `ALTER TABLE providers ADD COLUMN anthropic_base_url TEXT;` entry:

```sql
ALTER TABLE providers ADD COLUMN model_formats TEXT;
```

- [ ] **Step 4: Read and write the column**

In `db_config.rs`, add `model_formats` as the last column of both the `INSERT INTO providers (...)` statement (with a new `?N` placeholder and `p.model_formats` in `params!`) and the `SELECT ... FROM providers` statement, mapping it with the existing empty-string guard:

```rust
                model_formats: none_if_empty(row.get(17)?),
```

- [ ] **Step 5: Add the DTO field**

In `crates/proxy-admin-api/src/lib.rs`, in `ProviderPayload`:

```rust
    /// Per-model wire-format overrides, `glob=format` pairs joined by commas
    /// (`format` is `anthropic`, `openai` or `responses`). First match wins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_formats: Option<String>,
```

Map it in both directions in `admin.rs`, then run `rg -n "ProviderPayload \{" crates` and add `model_formats: None,` to the remaining literals.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p proxy model_formats && cargo test -p proxy-admin-api && cargo build --workspace`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates
git commit -m "feat(proxy): persist and expose model_formats

Adds the providers.model_formats column, the admin API field, and the
mapping in both directions so the rules survive a restart and can be edited
without touching the daemon."
```

---

### Task 5: Model-aware format selection

**Files:**
- Create: `crates/proxy/src/adapters/providers/model_formats.rs`
- Modify: `crates/proxy/src/adapters/providers/mod.rs`
- Modify: `crates/proxy/src/application/ports/provider.rs`
- Modify: `crates/proxy/src/adapters/providers/upstream.rs`
- Modify: `crates/proxy/src/adapters/providers/builder.rs`
- Modify: `crates/proxy/src/adapters/providers/routing.rs`

**Interfaces:**
- Consumes: `parse_model_formats`, `WireFormat` (Task 2); `preset_model_formats` (Task 3).
- Produces:
  - `pub struct ModelFormatTable` with `pub fn parse(rules: &str) -> Result<Self, String>`, `pub fn empty() -> Self`, `pub fn resolve(&self, model: &str) -> Option<WireFormat>`
  - `Provider::supported_formats_for(&self, model: &str) -> FormatSupport` (defaulted)
  - `UpstreamProvider::with_model_formats(self, table: ModelFormatTable) -> Self`

- [ ] **Step 1: Write the failing tests for the table**

Create `crates/proxy/src/adapters/providers/model_formats.rs` with only the tests first:

```rust
//! Compiled per-model wire-format rules. The config string is the source of
//! truth; this is its runtime form.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WireFormat;

    #[test]
    fn resolves_first_match_in_written_order() {
        let t = ModelFormatTable::parse("qwen3.7-max=responses,qwen3.*=anthropic").unwrap();
        assert_eq!(t.resolve("qwen3.7-max"), Some(WireFormat::Responses));
        assert_eq!(t.resolve("qwen3.6-plus"), Some(WireFormat::Anthropic));
    }

    #[test]
    fn unmatched_model_resolves_to_none() {
        let t = ModelFormatTable::parse("minimax-*=anthropic").unwrap();
        assert_eq!(t.resolve("kimi-k3"), None);
    }

    #[test]
    fn empty_table_never_matches() {
        assert_eq!(ModelFormatTable::empty().resolve("anything"), None);
    }

    #[test]
    fn dot_is_literal_not_a_wildcard() {
        let t = ModelFormatTable::parse("qwen3.*=anthropic").unwrap();
        assert_eq!(t.resolve("qwen3.8-max"), Some(WireFormat::Anthropic));
        assert_eq!(t.resolve("qwen38-max"), None);
    }

    #[test]
    fn parse_error_names_the_bad_entry() {
        let err = ModelFormatTable::parse("glm-*=grpc").unwrap_err();
        assert!(err.contains("glm-*=grpc"), "{err}");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p proxy model_format_table`
Expected: FAIL — module not registered, `ModelFormatTable` missing.

- [ ] **Step 3: Implement the table**

Prepend to the same file, and add `pub mod model_formats;` to `crates/proxy/src/adapters/providers/mod.rs`:

```rust
use crate::config::{WireFormat, parse_model_formats};
use globset::{Glob, GlobMatcher};

#[derive(Debug, Default)]
pub struct ModelFormatTable {
    rules: Vec<(GlobMatcher, WireFormat)>,
}

impl ModelFormatTable {
    pub fn empty() -> Self {
        Self::default()
    }

    /// Compile the compact rule string. Syntax errors carry the offending
    /// entry so the message is actionable in the TUI.
    pub fn parse(rules: &str) -> Result<Self, String> {
        let parsed = parse_model_formats(rules).map_err(|e| e.to_string())?;
        let mut compiled = Vec::with_capacity(parsed.len());
        for (glob, format) in parsed {
            let matcher = Glob::new(&glob)
                .map_err(|e| format!("invalid model_formats glob '{glob}': {e}"))?
                .compile_matcher();
            compiled.push((matcher, format));
        }
        Ok(Self { rules: compiled })
    }

    /// First matching rule wins. `None` means "no opinion" — the caller falls
    /// back to the provider's URL-derived capability.
    pub fn resolve(&self, model: &str) -> Option<WireFormat> {
        self.rules
            .iter()
            .find(|(matcher, _)| matcher.is_match(model))
            .map(|(_, format)| *format)
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p proxy model_format_table`
Expected: PASS.

- [ ] **Step 5: Write the failing test for the provider override**

In `crates/proxy/src/adapters/providers/upstream.rs` `mod tests` (reuse the local constructor helper the other tests use):

```rust
#[test]
fn model_formats_narrow_supported_formats_per_model() {
    let p = UpstreamProvider::new(
        "go".to_string(),
        Some("https://opencode.ai/zen/go".to_string()),
        Some("https://opencode.ai/zen/go/v1".to_string()),
        AuthHeader::ApiKey("k".to_string()),
        Quirks::none(),
        reqwest::Client::new(),
    )
    .with_model_formats(
        ModelFormatTable::parse("qwen3.*=anthropic,grok-4.5=responses").unwrap(),
    );

    // Anthropic-only model.
    let qwen = p.supported_formats_for("qwen3.7-max");
    assert!(qwen.anthropic && !qwen.openai);

    // Responses models ride the OpenAI path; the split happens inside
    // forward_openai.
    let grok = p.supported_formats_for("grok-4.5");
    assert!(grok.openai && !grok.anthropic);

    // No rule → unchanged capability (both URLs configured).
    let kimi = p.supported_formats_for("kimi-k3");
    assert!(kimi.anthropic && kimi.openai);
}
```

- [ ] **Step 6: Run it to verify it fails**

Run: `cargo test -p proxy model_formats_narrow`
Expected: FAIL — no `with_model_formats` / `supported_formats_for`.

- [ ] **Step 7: Add the port method and the override**

`crates/proxy/src/application/ports/provider.rs`, inside `trait Provider`:

```rust
    /// Which formats this provider can serve for one specific model. Providers
    /// whose capability doesn't vary by model inherit the default.
    fn supported_formats_for(&self, model: &str) -> FormatSupport {
        let _ = model;
        self.supported_formats()
    }
```

`upstream.rs` — add the field `model_formats: ModelFormatTable` (initialised to `ModelFormatTable::empty()` in `new`), the builder method, and the override:

```rust
    /// Attach compiled per-model format rules. Empty means "decide from the
    /// configured URLs", i.e. the behavior of every other provider.
    pub fn with_model_formats(mut self, table: ModelFormatTable) -> Self {
        self.model_formats = table;
        self
    }
```

```rust
    fn supported_formats_for(&self, model: &str) -> FormatSupport {
        match self.model_formats.resolve(model) {
            // Responses is an upstream detail of the OpenAI path: routing only
            // needs to know the request goes out as OpenAI.
            Some(WireFormat::OpenAi) | Some(WireFormat::Responses) => FormatSupport {
                anthropic: false,
                openai: true,
            },
            Some(WireFormat::Anthropic) => FormatSupport {
                anthropic: true,
                openai: false,
            },
            None => self.supported_formats(),
        }
    }
```

`builder.rs`, in `build_leaf`, after the URLs are resolved:

```rust
    let model_formats = match p.model_formats.as_deref() {
        Some(rules) if !rules.trim().is_empty() => {
            ModelFormatTable::parse(rules).map_err(BuildError::InvalidModelFormats)?
        }
        _ => ModelFormatTable::empty(),
    };
```

and chain `.with_model_formats(model_formats)` onto the `UpstreamProvider` construction. Add the variant to `BuildError`:

```rust
    #[error("invalid model_formats: {0}")]
    InvalidModelFormats(String),
```

- [ ] **Step 8: Run the tests to verify they pass**

Run: `cargo test -p proxy model_formats_narrow`
Expected: PASS.

- [ ] **Step 9: Write the failing routing test**

`routing.rs`'s `mod tests` already has a fake provider struct used by the existing routing tests. Extend that fake with two fields — a `FormatSupport` returned by `supported_formats`, and an `anthropic_only_model: Option<String>` — and implement the new port method on it:

```rust
    fn supported_formats_for(&self, model: &str) -> FormatSupport {
        if self.anthropic_only_model.as_deref() == Some(model) {
            return FormatSupport { anthropic: true, openai: false };
        }
        self.supported_formats()
    }
```

The fake already records which method it received (the existing tests assert on it); reuse that recorder rather than adding a second one. Then add:

```rust
#[tokio::test]
async fn openai_client_asking_for_an_anthropic_only_model_is_translated() {
    // The provider serves both formats in general, but qwen3.7-max only on
    // its Anthropic endpoint — the OpenCode Go shape.
    let leaf = fake_provider_supporting_both()
        .with_anthropic_only_model("qwen3.7-max");
    let calls = leaf.calls();
    let router = router_with_single_rule("*", leaf);

    let body = Bytes::from(r#"{"model":"qwen3.7-max","messages":[]}"#);
    router
        .forward_openai("/chat/completions", &HeaderMap::new(), body, false)
        .await
        .unwrap();

    // Translated: the leaf was entered through the Anthropic method.
    assert_eq!(calls.lock().unwrap().as_slice(), &["forward"]);
}

#[tokio::test]
async fn openai_client_asking_for_an_unruled_model_still_passes_through() {
    let leaf = fake_provider_supporting_both();
    let calls = leaf.calls();
    let router = router_with_single_rule("*", leaf);

    let body = Bytes::from(r#"{"model":"kimi-k3","messages":[]}"#);
    router
        .forward_openai("/chat/completions", &HeaderMap::new(), body, false)
        .await
        .unwrap();

    assert_eq!(calls.lock().unwrap().as_slice(), &["forward_openai"]);
}
```

`fake_provider_supporting_both` and `router_with_single_rule` stand for whatever the module's existing helpers are named — use them as they are rather than adding parallel ones. The assertion that matters is *which* of `forward` / `forward_openai` the leaf received.

- [ ] **Step 10: Run it to verify it fails**

Run: `cargo test -p proxy anthropic_only_model`
Expected: FAIL — routing still asks `supported_formats()`, so the request passes through to `forward_openai`.

- [ ] **Step 11: Switch the six routing call sites**

In `routing.rs`, replace `provider.supported_formats()` / `entry.provider.supported_formats()` with the model-aware call at all six `select_direction` sites (namespace ×2 at ~321 and ~395, failover ×2 at ~693 and ~952, round-robin ×2 at ~825 and ~1077).

On the two namespace paths the lookup **must** use the bare model, because that is what goes upstream:

```rust
        let (direction, upstream_format) =
            Self::select_direction(ApiFormat::Anthropic, provider.supported_formats_for(bare_model));
```

On the failover and round-robin paths the parsed `model` is already in scope:

```rust
        let (direction, upstream_format) =
            Self::select_direction(client_format, entry.provider.supported_formats_for(&model));
```

- [ ] **Step 12: Run the tests to verify they pass**

Run: `cargo test -p proxy routing && cargo test -p proxy --test integration`
Expected: PASS, with no pre-existing routing test regressing.

- [ ] **Step 13: Lint and commit**

```bash
cargo clippy -p proxy -- -D warnings && cargo fmt
git add crates/proxy
git commit -m "feat(proxy): pick the upstream format per model

Routing decided the wire format from the client's endpoint and the
provider's URLs alone. supported_formats_for lets a provider narrow that
per model, which is what OpenCode Go needs: one base URL, three endpoints
split by model."
```

---

### Task 6: Serve the Responses models

**Files:**
- Modify: `crates/proxy/src/adapters/providers/upstream.rs`
- Test: `crates/proxy/tests/integration.rs`

**Interfaces:**
- Consumes: `ResponsesDialect`, `translate_request`, `translate_buffered_response`, `ResponsesSseTranslator` (Task 1); `ModelFormatTable` (Task 5).
- Produces: nothing new — `forward_openai` gains an internal branch.

- [ ] **Step 1: Write the failing integration test**

In `crates/proxy/tests/integration.rs`, following the existing wiremock tests:

```rust
#[tokio::test]
async fn opencode_go_routes_a_responses_model_to_the_responses_endpoint() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/zen/go/v1/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "resp_1",
            "model": "grok-4.5",
            "output": [{
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "hello"}]
            }],
            "usage": {"input_tokens": 3, "output_tokens": 2}
        })))
        .mount(&server)
        .await;

    let provider = UpstreamProvider::new(
        "go".to_string(),
        Some(format!("{}/zen/go", server.uri())),
        Some(format!("{}/zen/go/v1", server.uri())),
        AuthHeader::ApiKey("secret".to_string()),
        Quirks::none(),
        reqwest::Client::new(),
    )
    .with_model_formats(ModelFormatTable::parse("grok-4.5=responses").unwrap());

    let body = Bytes::from(r#"{"model":"grok-4.5","stream":false,"messages":[{"role":"user","content":"hi"}]}"#);
    let resp = provider
        .forward_openai("/chat/completions", &HeaderMap::new(), body, false)
        .await
        .unwrap();

    let UpstreamResponse::Buffered { status, body, .. } = resp else {
        panic!("expected a buffered response");
    };
    assert_eq!(status, 200);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["object"], "chat.completion");
    assert_eq!(v["choices"][0]["message"]["content"], "hello");
}

#[tokio::test]
async fn opencode_go_routes_an_unruled_model_to_chat_completions() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/zen/go/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "chatcmpl_1",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "hi"},
                "finish_reason": "stop"
            }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let provider = UpstreamProvider::new(
        "go".to_string(),
        Some(format!("{}/zen/go", server.uri())),
        Some(format!("{}/zen/go/v1", server.uri())),
        AuthHeader::ApiKey("secret".to_string()),
        Quirks::none(),
        reqwest::Client::new(),
    )
    .with_model_formats(ModelFormatTable::parse("grok-4.5=responses").unwrap());

    let body = Bytes::from(
        r#"{"model":"kimi-k3","stream":false,"messages":[{"role":"user","content":"hi"}]}"#,
    );
    let resp = provider
        .forward_openai("/chat/completions", &HeaderMap::new(), body, false)
        .await
        .unwrap();

    let UpstreamResponse::Buffered { status, body, .. } = resp else {
        panic!("expected a buffered response");
    };
    assert_eq!(status, 200);
    // Untranslated passthrough: the upstream body arrives verbatim.
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["id"], "chatcmpl_1");
    // `expect(1)` on the mock asserts /chat/completions was the endpoint hit.
}

#[tokio::test]
async fn anthropic_client_reaches_a_responses_model() {
    // Both hops compose: routing translates A→O, the provider then
    // translates O→Responses.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/zen/go/v1/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "resp_2",
            "model": "grok-4.5",
            "output": [{
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "hello"}]
            }],
            "usage": {"input_tokens": 3, "output_tokens": 2}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let leaf = UpstreamProvider::new(
        "go".to_string(),
        Some(format!("{}/zen/go", server.uri())),
        Some(format!("{}/zen/go/v1", server.uri())),
        AuthHeader::ApiKey("secret".to_string()),
        Quirks::none(),
        reqwest::Client::new(),
    )
    .with_model_formats(ModelFormatTable::parse("grok-4.5=responses").unwrap());

    let router = router_with_single_rule("*", leaf);
    let body = Bytes::from(
        r#"{"model":"grok-4.5","max_tokens":32,"messages":[{"role":"user","content":"hi"}]}"#,
    );
    let resp = router
        .forward("/v1/messages", &HeaderMap::new(), body, false)
        .await
        .unwrap();

    let UpstreamResponse::Buffered { status, body, .. } = resp else {
        panic!("expected a buffered response");
    };
    assert_eq!(status, 200);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    // Back in Anthropic shape for the client.
    assert_eq!(v["type"], "message");
    assert_eq!(v["content"][0]["text"], "hello");
}
```

`router_with_single_rule` is the same helper used in Task 5 — build the `RoutingProvider` the way the integration tests in this file already do.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p proxy --test integration opencode_go`
Expected: FAIL — the request goes to `/zen/go/v1/chat/completions` for both models, so the first test 404s on the mock.

- [ ] **Step 3: Add the Responses branch**

In `upstream.rs`, give the struct a `responses_base_url: Option<String>` field defaulting to `None` in `new`, plus:

```rust
    /// Endpoint for models routed to the OpenAI Responses API. Defaults to
    /// `openai_base_url`, which is where OpenCode Go serves `/responses`.
    fn responses_base(&self) -> Option<&str> {
        self.responses_base_url
            .as_deref()
            .or(self.openai_base_url.as_deref())
    }
```

At the top of `forward_openai`, before the existing chat path:

```rust
        let model = messages_protocol::parse_model(&body).unwrap_or_default();
        if self.model_formats.resolve(&model) == Some(WireFormat::Responses) {
            return self.forward_responses(headers, body, streaming).await;
        }
```

Add `forward_responses`, mirroring `CodexProvider::forward_openai` but with the vanilla dialect and honouring the client's `stream` flag:

```rust
    /// POST to `{base}/responses`, translating the Chat Completions body on the
    /// way out and the Responses payload on the way back. Streaming responses
    /// are wrapped in the SSE translator; buffered ones are converted whole.
    async fn forward_responses(
        &self,
        headers: &HeaderMap,
        body: Bytes,
        streaming: bool,
    ) -> Result<UpstreamResponse, ProxyError> {
        let base = self.responses_base().ok_or_else(|| {
            ProxyError::BadRequest(format!("provider '{}' has no Responses endpoint", self.name))
        })?;
        let chat: Value = serde_json::from_slice(&body)
            .map_err(|e| ProxyError::BadRequest(format!("invalid JSON body: {e}")))?;
        let translated = translate_request(&chat, &ResponsesDialect::vanilla())
            .map_err(|e| ProxyError::BadRequest(format!("responses translation failed: {e}")))?;
        let out = Bytes::from(serde_json::to_vec(&translated).map_err(|e| {
            ProxyError::BadRequest(format!("failed to serialize responses body: {e}"))
        })?);

        let resp = messages_protocol::forward(
            &self.http,
            base,
            &self.auth_for_openai(),
            "/responses",
            headers,
            out,
            streaming,
            self.name(),
        )
        .await?;

        Ok(match resp {
            UpstreamResponse::Streaming {
                status,
                headers,
                body,
                provider_id,
                translation_direction,
            } => UpstreamResponse::Streaming {
                status,
                headers,
                body: Box::pin(ResponsesSseTranslator::new(body)),
                provider_id,
                translation_direction,
            },
            UpstreamResponse::Buffered {
                status,
                headers,
                body,
                provider_id,
                translation_direction,
            } if (200..300).contains(&status) => {
                let value: Value = serde_json::from_slice(&body).map_err(|e| {
                    ProxyError::BadRequest(format!("invalid Responses body: {e}"))
                })?;
                let chat = translate_buffered_response(&value).map_err(|e| {
                    ProxyError::BadRequest(format!("responses translation failed: {e}"))
                })?;
                UpstreamResponse::Buffered {
                    status,
                    headers,
                    body: Bytes::from(serde_json::to_vec(&chat).map_err(|e| {
                        ProxyError::BadRequest(format!("failed to serialize chat body: {e}"))
                    })?),
                    provider_id,
                    translation_direction,
                }
            }
            // Non-2xx bodies pass through untranslated, as everywhere else.
            other => other,
        })
    }
```

Factor the existing `AuthHeader::ApiKey → Bearer` conversion in `forward_openai` into `fn auth_for_openai(&self) -> AuthHeader` and call it from both places, so the Responses path authenticates identically to the chat path.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p proxy --test integration opencode_go`
Expected: PASS.

- [ ] **Step 5: Run the whole proxy suite**

Run: `cargo test -p proxy`
Expected: PASS — in particular every Codex test, which shares the translation.

- [ ] **Step 6: Lint and commit**

```bash
cargo clippy -p proxy -- -D warnings && cargo fmt
git add crates/proxy
git commit -m "feat(proxy): serve Responses-only models from UpstreamProvider

Models whose rule says responses are translated to the Responses API and
sent to {base}/responses, reusing the translator extracted from Codex. The
client keeps talking Chat Completions or Anthropic; usage parsing is
unaffected because the translator emits chat.completion chunks."
```

---

### Task 7: TUI support

**Files:**
- Modify: `crates/proxy-tui/src/app.rs`
- Modify: `crates/proxy-tui/src/main.rs`
- Modify: `crates/proxy-tui/src/ui.rs`
- Modify: `crates/proxy-tui/src/validate.rs`

**Interfaces:**
- Consumes: `ProviderPayload.model_formats` (Task 4); the kind string `opencode_go` (Task 3).
- Produces: `FormField::ModelFormats`; `ProviderFormModal.model_formats: String`.

- [ ] **Step 1: Write the failing tests**

In `crates/proxy-tui/src/main.rs` `mod tests` (next to the existing kind-cycling test at ~2152):

```rust
#[test]
fn selecting_opencode_go_prefills_urls_and_rules() {
    let mut m = ProviderFormModal::new_for_add();
    while m.kind != ProviderKind::OpencodeGo {
        m.kind = m.kind.cycle_next();
        apply_kind_defaults(&mut m); // whatever the existing autofill helper is called
    }
    assert_eq!(m.anthropic_base_url, "https://opencode.ai/zen/go");
    assert_eq!(m.openai_base_url, "https://opencode.ai/zen/go/v1");
    assert_eq!(
        m.model_formats,
        "minimax-*=anthropic,qwen3.*=anthropic,grok-4.5=responses,gpt-5.6-luna=responses"
    );
}
```

In `crates/proxy-tui/src/validate.rs` `mod tests`:

```rust
#[test]
fn model_formats_reaches_the_payload() {
    let cfg = empty_cfg();
    let mut input = valid_form_inputs(); // helper used by the neighbouring tests
    input.model_formats = Some("glm-*=openai");
    let payload = validate_provider_form(&input, &cfg).unwrap();
    assert_eq!(payload.model_formats.as_deref(), Some("glm-*=openai"));
}

#[test]
fn malformed_model_formats_is_rejected_before_saving() {
    let cfg = empty_cfg();
    let mut input = valid_form_inputs();
    input.model_formats = Some("glm-*");
    assert!(matches!(
        validate_provider_form(&input, &cfg),
        Err(FormError::InvalidModelFormats(_))
    ));
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p proxy-tui model_formats`
Expected: FAIL — field, variant and error don't exist.

- [ ] **Step 3: Mirror the kind**

In `crates/proxy-tui/src/app.rs`, add `OpencodeGo` to the local `ProviderKind` and to **all four** of `label()` (`"opencode_go"`), `cycle_next()`, `cycle_prev()` and `from_str_or_default()` (accept `"opencode_go"` and `"opencode-go"`). Insert it after `Kimi` in the cycle so the ring stays: `… → Minimax → Kimi → OpencodeGo → Anthropic`. Add its `default_urls()` arm:

```rust
            ProviderKind::OpencodeGo => (
                Some("https://opencode.ai/zen/go"),
                Some("https://opencode.ai/zen/go/v1"),
            ),
```

and a preset mirroring `upstream::preset_model_formats` — keep the "mirrors the proxy crate, kept in sync by hand" doc comment style used by `default_urls`:

```rust
    /// Seeded `model_formats` rules for this kind. Mirrors
    /// `proxy::adapters::providers::upstream::preset_model_formats` (kept in
    /// sync manually — `proxy-tui` does not depend on the `proxy` crate).
    pub fn preset_model_formats(self) -> Option<&'static str> {
        match self {
            ProviderKind::OpencodeGo => {
                Some("minimax-*=anthropic,qwen3.*=anthropic,grok-4.5=responses,gpt-5.6-luna=responses")
            }
            _ => None,
        }
    }
```

Add `ThinkingLevelInput` offerings for the kind: an empty list, matching the proxy-side table from Task 3.

- [ ] **Step 4: Add the form field**

In `app.rs`: add `ModelFormats` to `FormField`, add `pub model_formats: String` to `ProviderFormModal` (empty in `new_for_add`, `p.model_formats.clone().unwrap_or_default()` when loading an existing provider), and push the field in `field_order` right after `OpenaiBaseUrl` — only for kinds that can use it, so the form stays short elsewhere:

```rust
    if provider_kind == ProviderKind::OpencodeGo {
        order.push(FormField::ModelFormats);
    }
```

In `main.rs`: add the arm to the text-field mapping (~1384) `FormField::ModelFormats => Some(&mut m.model_formats)`, pass `model_formats: Some(&m.model_formats)` in the three `FormInputs` constructions (~1235, ~1418, ~1916), and extend the kind-change autofill (~1314) so switching kinds replaces the rules while they still hold the previous kind's preset, exactly as the URL autofill does.

In `ui.rs`: render one more row in the provider modal next to the base-URL rows, labelled `model formats`, and show it in the provider detail view (~1252) with the existing `show_or_placeholder` helper.

- [ ] **Step 5: Validate the string**

In `validate.rs`: add `model_formats: Option<&'a str>` to `FormInputs`, a `FormError::InvalidModelFormats(String)` variant with a message that names the bad entry, and validation before the payload is built. `proxy-tui` cannot call `proxy::config::parse_model_formats` (no dependency), so validate with the same grammar locally:

```rust
/// Mirrors `proxy::config::parse_model_formats` — `proxy-tui` cannot depend on
/// the proxy crate, so the grammar is checked twice on purpose. The daemon
/// remains the authority; this only keeps a typo from reaching it.
fn check_model_formats(s: &str) -> Result<(), FormError> {
    for segment in s.split(',') {
        let segment = segment.trim();
        if segment.is_empty() {
            continue;
        }
        let Some((glob, format)) = segment.split_once('=') else {
            return Err(FormError::InvalidModelFormats(segment.to_string()));
        };
        if glob.trim().is_empty()
            || !matches!(
                format.trim().to_ascii_lowercase().as_str(),
                "anthropic" | "openai" | "responses"
            )
        {
            return Err(FormError::InvalidModelFormats(segment.to_string()));
        }
    }
    Ok(())
}
```

Set the payload field from the trimmed input, `None` when empty.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p proxy-tui`
Expected: PASS.

- [ ] **Step 7: Lint and commit**

```bash
cargo clippy -p proxy-tui -- -D warnings && cargo fmt
git add crates/proxy-tui
git commit -m "feat(proxy-tui): edit model_formats and add the opencode_go kind

Selecting the kind prefills both base URLs and the seeded rules; the rule
string is checked against the same grammar the daemon enforces so a typo
fails in the form instead of at save time."
```

---

### Task 8: Document and verify against the live API

**Files:**
- Modify: `docs/specs/provider-config.md`
- Modify: `CLAUDE.md` (design-docs list)

**Interfaces:**
- Consumes: everything above.
- Produces: no code.

- [ ] **Step 1: Document the kind**

In `docs/specs/provider-config.md`, add `opencode_go` to the provider-kinds table and a subsection in the same shape as the existing per-provider sections, covering: the two base URLs, `auth: api_key`, the seeded `model_formats` string, the three endpoint groups with their model lists (copy the table from the design spec), and the note that models with no rule fall through to `/chat/completions`.

Document the `model_formats` field itself in the field-reference section: grammar, first-match-wins, empty = previous behavior, and that it is validated when saved.

- [ ] **Step 2: Link the design docs**

Add to the design-docs list in `CLAUDE.md`:

```markdown
- **OpenCode Go provider** — spec `docs/superpowers/specs/2026-08-14-opencode-go-provider-design.md`, plan `docs/superpowers/plans/2026-08-14-opencode-go-provider.md`.
```

- [ ] **Step 3: Verify against the live API**

This step needs a real subscription key; it is the only step that can't be done offline. With the key in `~/.opencode-go.key` (never echo it, never commit it):

```bash
KEY=$(cat ~/.opencode-go.key)

# 1. Does the Anthropic endpoint accept x-api-key?
curl -s -o /dev/null -w "messages x-api-key: %{http_code}\n" \
  -X POST https://opencode.ai/zen/go/v1/messages \
  -H "x-api-key: $KEY" -H 'content-type: application/json' \
  -d '{"model":"qwen3.7-max","max_tokens":16,"messages":[{"role":"user","content":"hi"}]}'

# 2. ...or Bearer?
curl -s -o /dev/null -w "messages bearer: %{http_code}\n" \
  -X POST https://opencode.ai/zen/go/v1/messages \
  -H "authorization: Bearer $KEY" -H 'content-type: application/json' \
  -d '{"model":"qwen3.7-max","max_tokens":16,"messages":[{"role":"user","content":"hi"}]}'

# 3. Is the gateway strict? A chat/completions model on /messages and vice versa.
curl -s -o /dev/null -w "chat model on messages: %{http_code}\n" \
  -X POST https://opencode.ai/zen/go/v1/messages \
  -H "authorization: Bearer $KEY" -H 'content-type: application/json' \
  -d '{"model":"kimi-k3","max_tokens":16,"messages":[{"role":"user","content":"hi"}]}'

curl -s -o /dev/null -w "anthropic model on chat: %{http_code}\n" \
  -X POST https://opencode.ai/zen/go/v1/chat/completions \
  -H "authorization: Bearer $KEY" -H 'content-type: application/json' \
  -d '{"model":"qwen3.7-max","max_tokens":16,"messages":[{"role":"user","content":"hi"}]}'
```

Act on the results:
- If `/messages` rejects `x-api-key` but accepts `Bearer`, change the preset auth in `docs/specs/provider-config.md` and in the TUI's default for the kind to `bearer`.
- If the gateway is permissive (both cross-endpoint calls return 200), shorten the seeded rules to `grok-4.5=responses,gpt-5.6-luna=responses` in `upstream::preset_model_formats` and its TUI mirror, and note in the spec that the other groups are advisory.
- Record whichever outcome held in the "Assumptions and risks" section of the design spec, replacing the word "unverified".

- [ ] **Step 4: Commit**

```bash
git add docs CLAUDE.md crates
git commit -m "docs(specs): document the opencode_go provider kind"
```

- [ ] **Step 5: End-to-end smoke test**

Run the proxy against the dev DB, add the provider through the TUI, and send one request per endpoint group:

```bash
mise run dev:proxy      # terminal 1
mise run dev:proxy-tui  # terminal 2 — add an opencode_go provider, paste the key
```

```bash
# chat/completions group
curl -s localhost:8787/v1/chat/completions -H 'content-type: application/json' \
  -d '{"model":"opencode-go/kimi-k3","messages":[{"role":"user","content":"say hi"}]}' | head -c 400
# anthropic group, from an Anthropic client
curl -s localhost:8787/v1/messages -H 'content-type: application/json' \
  -d '{"model":"opencode-go/qwen3.7-max","max_tokens":32,"messages":[{"role":"user","content":"say hi"}]}' | head -c 400
# responses group
curl -s localhost:8787/v1/chat/completions -H 'content-type: application/json' \
  -d '{"model":"opencode-go/grok-4.5","messages":[{"role":"user","content":"say hi"}]}' | head -c 400
```

Expected: all three return content, and `mise run dev:proxy` logs one upstream URL per group — `/zen/go/v1/chat/completions`, `/zen/go/v1/messages`, `/zen/go/v1/responses`.
