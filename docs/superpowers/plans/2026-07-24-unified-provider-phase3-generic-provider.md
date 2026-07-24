# Unified Provider — Phase 3: Generic `UpstreamProvider` + `Quirks` + `preset`

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a single generic `UpstreamProvider` that serves both formats from configured URLs, with per-kind behavior expressed as `Quirks` and a `preset(kind)` table. Fully unit-tested but **not yet wired** into the builder (Phase 4 cuts over).

**Architecture:** `UpstreamProvider` holds both base URLs, the resolved `AuthHeader`, and a `Quirks` bundle. `supported_formats()` is derived from which URL is present. `forward()` (Anthropic upstream) and `forward_openai()` (OpenAI upstream) call the shared `messages_protocol::forward`, applying quirks around it — request quirks before the call, response quirks after. Quirk implementations are the existing ones (`minimax_stream`, `tool_sanitizer`, the request injectors), moved, not rewritten. Codex is out of scope (stays bespoke).

**Tech Stack:** Rust (edition 2024), async-trait, reqwest, `cargo test`.

## Global Constraints

- Edition 2024; no new dependencies. Per-ring error types; `ProxyError` on the
  provider surface.
- Do not delete any existing provider struct in this phase and do not touch
  `build_leaf` — this phase only *adds* code.
- Do not touch `native_format()` (removed in Phase 5). `UpstreamProvider` does
  not override it; it inherits the trait default and overrides
  `supported_formats()` instead (routing already reads that, from Phase 1).
- `cargo test --workspace` + `cargo clippy --workspace -- -D warnings` green
  before every commit. Conventional Commits, no attribution. Branch, not
  `develop`.

## Reference: the quirk sources to port (read these, do not rewrite)

| Quirk | Source today | Where it applies |
|---|---|---|
| `reasoning_split` | `minimax.rs::inject_reasoning_split` | OpenAI request body |
| `strip_thinking` | `minimax_stream::clean_thinking_buffered` / `clean_thinking_stream` | OpenAI response |
| `sanitize_empty_tools` | `super::tool_sanitizer::sanitize_buffered` / `sanitize_stream` (see `kimi.rs`) | response |
| `reorder_tool_responses` | the request reorder in `deepseek.rs` (commit c39b6ae/4cea164) | OpenAI request body |
| `reasoning_effort` (Anthropic-style) | `anthropic.rs::inject_effort` | Anthropic request body |

The canonical shape of an upstream call is `minimax.rs::forward` /
`forward_openai`: `messages_protocol::forward(&self.http, base_url, &auth,
path, headers, body, streaming, self.name())`.

---

### Task 1: `Quirks` struct

**Files:**
- Create: `crates/proxy/src/adapters/providers/upstream.rs`
- Modify: `crates/proxy/src/adapters/providers/mod.rs` (add `mod upstream;` and
  re-exports)

**Interfaces:**
- Produces: `pub struct Quirks { … }` + `Quirks::none()`.

- [ ] **Step 1: Write the failing test**

Create `crates/proxy/src/adapters/providers/upstream.rs` with only:

```rust
use super::minimax_stream::ThinkingMode;

/// Per-provider request/response behaviors on the forward path, as data.
#[derive(Debug, Clone, Default)]
pub struct Quirks {
    /// Inject `reasoning_split: true` into the OpenAI request body (MiniMax).
    pub reasoning_split: bool,
    /// Filter thinking content from the OpenAI response (MiniMax).
    pub strip_thinking: Option<ThinkingMode>,
    /// Reorder tool responses after assistant tool_calls in the OpenAI request
    /// (DeepSeek).
    pub reorder_tool_responses: bool,
    /// Sanitize empty-tool responses (Kimi).
    pub sanitize_empty_tools: bool,
    /// Inject reasoning effort into the Anthropic request output config
    /// (Anthropic).
    pub reasoning_effort: Option<String>,
}

impl Quirks {
    pub fn none() -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quirks_none_is_all_off() {
        let q = Quirks::none();
        assert!(!q.reasoning_split);
        assert!(q.strip_thinking.is_none());
        assert!(!q.reorder_tool_responses);
        assert!(!q.sanitize_empty_tools);
        assert!(q.reasoning_effort.is_none());
    }
}
```

Add `mod upstream;` to `crates/proxy/src/adapters/providers/mod.rs` next to the
other `mod` declarations.

- [ ] **Step 2: Run test**

Run: `cargo test -p proxy --lib upstream::tests::quirks_none`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/proxy/src/adapters/providers/upstream.rs crates/proxy/src/adapters/providers/mod.rs
git commit -m "feat(proxy): add Quirks provider-behavior struct"
```

---

### Task 2: `preset(kind)` → `Quirks`

**Files:**
- Modify: `crates/proxy/src/adapters/providers/upstream.rs`

**Interfaces:**
- Consumes: `crate::config::ProviderKind`, `Quirks`, `ThinkingMode`.
- Produces: `pub fn quirks_for(kind: ProviderKind, thinking: ThinkingMode,
  reasoning_effort: Option<String>, sanitize_empty_tools: bool) -> Quirks`.

The preset folds the config-overridable fields (`thinking_mode`,
`reasoning_effort`, `sanitize_empty_tools`) into the returned `Quirks` so the
builder passes them straight through.

- [ ] **Step 1: Write the failing test**

Append to `upstream.rs` tests:

```rust
    use crate::config::ProviderKind;

    #[test]
    fn preset_minimax_enables_reasoning_split_and_strip() {
        let q = quirks_for(ProviderKind::Minimax, ThinkingMode::SplitOnly, None, false);
        assert!(q.reasoning_split);
        assert_eq!(q.strip_thinking, Some(ThinkingMode::SplitOnly));
    }

    #[test]
    fn preset_deepseek_reorders_tools() {
        let q = quirks_for(ProviderKind::DeepSeek, ThinkingMode::SplitOnly, None, false);
        assert!(q.reorder_tool_responses);
        assert!(!q.reasoning_split);
    }

    #[test]
    fn preset_kimi_sanitizes_when_configured() {
        let q = quirks_for(ProviderKind::Kimi, ThinkingMode::SplitOnly, None, true);
        assert!(q.sanitize_empty_tools);
    }

    #[test]
    fn preset_anthropic_carries_reasoning_effort() {
        let q = quirks_for(ProviderKind::Anthropic, ThinkingMode::SplitOnly, Some("high".into()), false);
        assert_eq!(q.reasoning_effort.as_deref(), Some("high"));
    }

    #[test]
    fn preset_zai_and_openai_have_no_quirks() {
        for k in [ProviderKind::Zai, ProviderKind::OpenAi] {
            let q = quirks_for(k, ThinkingMode::SplitOnly, None, false);
            assert!(!q.reasoning_split && !q.reorder_tool_responses && !q.sanitize_empty_tools);
        }
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p proxy --lib upstream::tests::preset`
Expected: FAIL — `quirks_for` not found.

- [ ] **Step 3: Implement**

Add to `upstream.rs`:

```rust
use crate::config::ProviderKind;

/// Build the `Quirks` for a kind, folding in the config-overridable fields.
pub fn quirks_for(
    kind: ProviderKind,
    thinking: ThinkingMode,
    reasoning_effort: Option<String>,
    sanitize_empty_tools: bool,
) -> Quirks {
    match kind {
        ProviderKind::Minimax => Quirks {
            reasoning_split: true,
            strip_thinking: Some(thinking),
            ..Quirks::none()
        },
        ProviderKind::DeepSeek => Quirks {
            reorder_tool_responses: true,
            ..Quirks::none()
        },
        ProviderKind::Kimi => Quirks {
            sanitize_empty_tools,
            ..Quirks::none()
        },
        ProviderKind::Anthropic => Quirks {
            reasoning_effort,
            ..Quirks::none()
        },
        ProviderKind::Zai | ProviderKind::OpenAi => Quirks::none(),
        // Codex is bespoke and never built as an UpstreamProvider.
        ProviderKind::Codex => Quirks::none(),
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p proxy --lib upstream::tests::preset`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/proxy/src/adapters/providers/upstream.rs
git commit -m "feat(proxy): add quirks_for preset mapping kinds to Quirks"
```

---

### Task 3: `UpstreamProvider` — construction + `supported_formats()`

**Files:**
- Modify: `crates/proxy/src/adapters/providers/upstream.rs`

**Interfaces:**
- Consumes: `AuthHeader` (`super::messages_protocol::AuthHeader`),
  `FormatSupport`, `ApiFormat`, `Provider` (`crate::application::ports`).
- Produces:
  - `pub struct UpstreamProvider { … }`
  - `UpstreamProvider::new(name: String, anthropic_base_url: Option<String>,
    openai_base_url: Option<String>, auth: AuthHeader, quirks: Quirks,
    http: reqwest::Client) -> Self`
  - `impl Provider for UpstreamProvider` with `name()` and
    `supported_formats()` (other methods land in Task 4).

- [ ] **Step 1: Write the failing test**

Append to `upstream.rs` tests:

```rust
    use crate::application::ports::{ApiFormat, Provider};
    use super::super::messages_protocol::AuthHeader;

    fn up(anthropic: Option<&str>, openai: Option<&str>) -> UpstreamProvider {
        UpstreamProvider::new(
            "p".into(),
            anthropic.map(str::to_string),
            openai.map(str::to_string),
            AuthHeader::Passthrough,
            Quirks::none(),
            reqwest::Client::new(),
        )
    }

    #[test]
    fn supported_formats_reflect_configured_urls() {
        let both = up(Some("https://a"), Some("https://o"));
        assert!(both.supported_formats().has(ApiFormat::Anthropic));
        assert!(both.supported_formats().has(ApiFormat::OpenAI));

        let anth = up(Some("https://a"), None);
        assert!(anth.supported_formats().has(ApiFormat::Anthropic));
        assert!(!anth.supported_formats().has(ApiFormat::OpenAI));

        let oai = up(None, Some("https://o"));
        assert!(!oai.supported_formats().has(ApiFormat::Anthropic));
        assert!(oai.supported_formats().has(ApiFormat::OpenAI));
    }

    #[test]
    fn name_is_the_configured_name() {
        assert_eq!(up(Some("https://a"), None).name(), "p");
    }
```

Note: `Provider::name()` returns `&str`; store the name as `String` and return
`self.name.as_str()`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p proxy --lib upstream::tests::supported_formats_reflect`
Expected: FAIL — `UpstreamProvider` not found.

- [ ] **Step 3: Implement struct + partial trait**

Add to `upstream.rs`:

```rust
use super::messages_protocol::AuthHeader;
use crate::application::errors::ProxyError;
use crate::application::ports::{
    ApiFormat, FormatSupport, Provider, UpstreamResponse, UsageParser,
};
use crate::domain::UsageRecord;
use async_trait::async_trait;
use axum::http::HeaderMap;
use bytes::Bytes;

pub struct UpstreamProvider {
    name: String,
    anthropic_base_url: Option<String>,
    openai_base_url: Option<String>,
    auth: AuthHeader,
    quirks: Quirks,
    http: reqwest::Client,
}

impl UpstreamProvider {
    pub fn new(
        name: String,
        anthropic_base_url: Option<String>,
        openai_base_url: Option<String>,
        auth: AuthHeader,
        quirks: Quirks,
        http: reqwest::Client,
    ) -> Self {
        Self { name, anthropic_base_url, openai_base_url, auth, quirks, http }
    }
}

#[async_trait]
impl Provider for UpstreamProvider {
    fn name(&self) -> &str {
        self.name.as_str()
    }

    fn supported_formats(&self) -> FormatSupport {
        FormatSupport {
            anthropic: self.anthropic_base_url.as_deref().is_some_and(|s| !s.is_empty()),
            openai: self.openai_base_url.as_deref().is_some_and(|s| !s.is_empty()),
        }
    }

    // parse_model / usage parsers / forward / forward_openai — Task 4.
    fn parse_model(&self, body: &[u8]) -> Result<String, String> {
        super::messages_protocol::parse_model(body)
    }
    fn usage_parser(&self) -> Box<dyn UsageParser> {
        super::messages_protocol::usage_parser()
    }
    fn parse_usage_json(&self, body: &[u8]) -> Result<UsageRecord, String> {
        super::messages_protocol::parse_usage_json(body)
    }
    fn usage_parser_openai(&self) -> Box<dyn UsageParser> {
        super::messages_protocol::openai_usage_parser()
    }
    fn parse_usage_json_openai(&self, body: &[u8]) -> Result<UsageRecord, String> {
        super::messages_protocol::parse_openai_usage_json(body)
    }
    async fn forward(
        &self, path: &str, headers: &HeaderMap, body: Bytes, streaming: bool,
    ) -> Result<UpstreamResponse, ProxyError> {
        let _ = (path, headers, body, streaming);
        Err(ProxyError::BadRequest("not yet implemented".into()))
    }
    async fn forward_openai(
        &self, path: &str, headers: &HeaderMap, body: Bytes, streaming: bool,
    ) -> Result<UpstreamResponse, ProxyError> {
        let _ = (path, headers, body, streaming);
        Err(ProxyError::BadRequest("not yet implemented".into()))
    }
}
```

Confirm `Provider::name` returns `&str` in `provider.rs`; if it returns
`&'static str`, change that signature to `&str` in this task (a compatible
widening — `&'static str` coerces to `&str` at all existing call sites) and fix
any resulting mismatches in the existing structs by keeping their `&'static
str` literals (they still satisfy `&str`).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p proxy --lib upstream::tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/proxy/src/adapters/providers/upstream.rs crates/proxy/src/application/ports/provider.rs
git commit -m "feat(proxy): add UpstreamProvider with URL-driven supported_formats"
```

---

### Task 4: `UpstreamProvider` forward paths with quirks

**Files:**
- Modify: `crates/proxy/src/adapters/providers/upstream.rs`

**Interfaces:**
- Consumes: `messages_protocol::forward`, the quirk sources in the Reference
  table, `Quirks` (Task 1).
- Produces: working `forward` / `forward_openai` that apply quirks.

- [ ] **Step 1: Write the failing tests (quirk application, no network)**

Test the pure request/response quirk helpers, not the HTTP call. Add helpers
`apply_openai_request_quirks(&self, body: Bytes) -> Bytes` and
`apply_anthropic_request_quirks(&self, body: Bytes) -> Bytes` and a response
transform, then test them:

```rust
    #[test]
    fn openai_request_quirks_inject_reasoning_split_for_minimax() {
        let p = UpstreamProvider::new(
            "mm".into(), None, Some("https://o".into()),
            AuthHeader::Bearer("k".into()),
            quirks_for(ProviderKind::Minimax, ThinkingMode::SplitOnly, None, false),
            reqwest::Client::new(),
        );
        let out = p.apply_openai_request_quirks(Bytes::from(r#"{"model":"m","messages":[]}"#));
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["reasoning_split"], true);
    }

    #[test]
    fn anthropic_request_quirks_inject_effort_for_anthropic() {
        let p = UpstreamProvider::new(
            "an".into(), Some("https://a".into()), None,
            AuthHeader::Passthrough,
            quirks_for(ProviderKind::Anthropic, ThinkingMode::SplitOnly, Some("high".into()), false),
            reqwest::Client::new(),
        );
        let out = p.apply_anthropic_request_quirks(Bytes::from(r#"{"model":"m","messages":[]}"#));
        // inject_effort adds an output config; assert the body changed / contains effort
        assert_ne!(out, Bytes::from(r#"{"model":"m","messages":[]}"#));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p proxy --lib upstream::tests::openai_request_quirks`
Expected: FAIL — helper methods not defined.

- [ ] **Step 3: Implement the quirk helpers + wire the forward paths**

Add to `impl UpstreamProvider`. Port the quirk bodies from the Reference table
(move the free functions `inject_reasoning_split` from `minimax.rs` and
`inject_effort` from `anthropic.rs` into `upstream.rs`, or call them if you make
them `pub(super)` — prefer moving so the old structs can be deleted in Phase 4).

```rust
impl UpstreamProvider {
    fn apply_openai_request_quirks(&self, body: Bytes) -> Bytes {
        let mut body = body;
        if self.quirks.reasoning_split {
            body = inject_reasoning_split(&body); // moved from minimax.rs
        }
        if self.quirks.reorder_tool_responses {
            body = reorder_tool_responses(&body); // moved from deepseek.rs
        }
        body
    }

    fn apply_anthropic_request_quirks(&self, body: Bytes) -> Bytes {
        let mut body = body;
        if let Some(effort) = self.quirks.reasoning_effort.as_deref() {
            inject_effort(&mut body, Some(effort)); // moved from anthropic.rs
        }
        body
    }

    fn apply_openai_response_quirks(&self, resp: UpstreamResponse) -> UpstreamResponse {
        // strip_thinking then sanitize_empty_tools, mirroring minimax.rs and kimi.rs
        // exactly (buffered vs streaming branches). See those files for the shape.
        // ... port verbatim ...
        resp
    }
}
```

Then implement `forward` / `forward_openai`:

```rust
    async fn forward(
        &self, path: &str, headers: &HeaderMap, body: Bytes, streaming: bool,
    ) -> Result<UpstreamResponse, ProxyError> {
        let base = self.anthropic_base_url.as_deref().ok_or_else(|| {
            ProxyError::BadRequest(format!("provider '{}' has no Anthropic endpoint", self.name))
        })?;
        let body = self.apply_anthropic_request_quirks(body);
        super::messages_protocol::forward(
            &self.http, base, &self.auth, path, headers, body, streaming, self.name(),
        ).await
    }

    async fn forward_openai(
        &self, path: &str, headers: &HeaderMap, body: Bytes, streaming: bool,
    ) -> Result<UpstreamResponse, ProxyError> {
        let base = self.openai_base_url.as_deref().ok_or_else(|| {
            ProxyError::BadRequest(format!("provider '{}' has no OpenAI endpoint", self.name))
        })?;
        // OpenAI-compatible endpoints authenticate with Authorization: Bearer;
        // convert ApiKey → Bearer as MiniMax does today.
        let auth = match &self.auth {
            AuthHeader::ApiKey(v) => AuthHeader::Bearer(v.clone()),
            other => other.clone(),
        };
        let body = self.apply_openai_request_quirks(body);
        let resp = super::messages_protocol::forward(
            &self.http, base, &auth, path, headers, body, streaming, self.name(),
        ).await?;
        Ok(self.apply_openai_response_quirks(resp))
    }
```

Replace the two placeholder `forward`/`forward_openai` bodies from Task 3.
Ensure `AuthHeader` derives/имplements `Clone` (it does — used by other
providers).

- [ ] **Step 4: Run tests**

Run: `cargo test -p proxy --lib upstream`
Expected: PASS.

Run: `cargo clippy -p proxy -- -D warnings`
Expected: no warnings. Note: `inject_reasoning_split`, `inject_effort`,
`reorder_tool_responses` may now be duplicated (moved copy + original in the
old struct). If a `dead_code` or duplicate warning appears, keep the moved copy
in `upstream.rs` and leave the originals until Phase 4 deletes their structs —
add `#[allow(dead_code)]` on the moved copy only if clippy blocks the commit.

- [ ] **Step 5: Commit**

```bash
git add crates/proxy/src/adapters/providers/upstream.rs
git commit -m "feat(proxy): UpstreamProvider forward paths apply request/response quirks"
```

---

## Self-Review

- **Spec coverage:** spec §"`UpstreamProvider`" (Tasks 3–4), §"`Quirks`" (Task
  1), §"`preset(kind)`" (Task 2, as `quirks_for`). Codex excluded per the
  updated spec. Not wired — Phase 4 does the cutover.
- **Placeholder scan:** `apply_openai_response_quirks` says "port verbatim from
  minimax.rs/kimi.rs" — this is a move of existing, tested code, with the exact
  source named, not an invention. All novel code (struct, supported_formats,
  forward paths, preset) is shown in full.
- **Type consistency:** `Quirks`, `quirks_for`, `UpstreamProvider::new`,
  `supported_formats`, and the `apply_*_quirks` helper names are consistent
  across tasks. `name()` returns `&str`.

## After this phase

Phase 4 switches `build_leaf` to construct `UpstreamProvider` for the six
genericized kinds (Codex still builds `CodexProvider`), then deletes the six
structs one at a time. Dual-for-all turns on here. Phase 5 removes
`native_format()` and adds preset-default URL prefill to the TUI wizard.
