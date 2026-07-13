# Kimi (Moonshot) Provider Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Kimi (Moonshot AI) as a first-class proxy provider with dual OpenAI/Anthropic endpoints and account-balance tracking.

**Architecture:** `KimiProvider` mirrors `ZaiProvider` (dual `base_url` + `openai_base_url`, `native_format() = OpenAI`). `KimiAccountUsage` mirrors `DeepSeekAccountUsage` (balance API → balance `UsageWindow`). A new `ProviderKind::Kimi` variant is wired through every exhaustive match in the `proxy` crate, plus the independent `ProviderKind` enum in `proxy-tui`.

**Tech Stack:** Rust (edition 2024), axum, reqwest, ureq (account usage), serde, async-trait, thiserror.

## Global Constraints

- Edition 2024; prefer let-chains. `cargo clippy --workspace -- -D warnings` must pass.
- Per-ring error types; no `anyhow`. Cross-ring conversions via `#[from]`.
- No new dependencies.
- Provider kind canonical string is `"kimi"`; accepted aliases: `"kimi"`, `"moonshot"`.
- Default base URLs (global region): Anthropic `https://api.moonshot.ai/anthropic`, OpenAI `https://api.moonshot.ai/v1`.
- Balance endpoint: `https://api.moonshot.ai/v1/users/me/balance`, header `Authorization: Bearer <token>`.
- Use Serena symbolic tools for reads/edits on code files per project setup.

---

### Task 1: KimiProvider core adapter

**Files:**
- Create: `crates/proxy/src/adapters/providers/kimi.rs`
- Modify: `crates/proxy/src/adapters/providers/mod.rs` (add `pub mod kimi;` after line 11 `pub mod minimax;`, and `pub use kimi::KimiProvider;` after line 26 `pub use minimax::MinimaxProvider;`)

**Interfaces:**
- Consumes: `super::messages_protocol::{self, AuthHeader}`; `crate::application::ports::{ApiFormat, Provider, UpstreamResponse, UsageParser}`.
- Produces: `KimiProvider` with `pub fn configure(http: reqwest::Client, base_url: Option<String>, openai_base_url: Option<String>, auth: AuthHeader) -> Self` and `impl Provider` where `name() == "kimi"`, `native_format() == ApiFormat::OpenAI`.

- [ ] **Step 1: Write the failing test**

Create `crates/proxy/src/adapters/providers/kimi.rs` with only the test module (implementation follows in Step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_is_kimi() {
        let p = KimiProvider::new(reqwest::Client::new());
        assert_eq!(p.name(), "kimi");
    }

    #[test]
    fn native_format_is_openai() {
        let p = KimiProvider::new(reqwest::Client::new());
        assert_eq!(p.native_format(), ApiFormat::OpenAI);
    }

    #[test]
    fn configure_defaults_to_moonshot_ai() {
        let p = KimiProvider::configure(reqwest::Client::new(), None, None, AuthHeader::Passthrough);
        assert_eq!(p.base_url, "https://api.moonshot.ai/anthropic");
        assert_eq!(p.openai_base_url.as_deref(), Some("https://api.moonshot.ai/v1"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p proxy kimi:: 2>&1 | tail -20`
Expected: FAIL — compile error, `KimiProvider` not found.

- [ ] **Step 3: Write minimal implementation**

Prepend above the test module in `crates/proxy/src/adapters/providers/kimi.rs`:

```rust
//! Kimi (Moonshot AI) provider — implements `Provider` against Moonshot's
//! Anthropic-compatible endpoint at `https://api.moonshot.ai/anthropic` and its
//! OpenAI-compatible endpoint at `https://api.moonshot.ai/v1`.
//!
//! Mirrors `ZaiProvider`: `native_format()` is `OpenAI`, so incoming Anthropic
//! requests are translated to OpenAI and sent to `/v1`; `forward()` still offers
//! the Anthropic passthrough path.

use super::messages_protocol::{self, AuthHeader};
use crate::application::errors::ProxyError;
use crate::application::ports::{ApiFormat, Provider, UpstreamResponse, UsageParser};
use crate::domain::UsageRecord;
use async_trait::async_trait;
use axum::http::HeaderMap;
use bytes::Bytes;

const DEFAULT_BASE_URL: &str = "https://api.moonshot.ai/anthropic";
const DEFAULT_OPENAI_BASE_URL: &str = "https://api.moonshot.ai/v1";

pub struct KimiProvider {
    base_url: String,
    openai_base_url: Option<String>,
    http: reqwest::Client,
    auth: AuthHeader,
}

impl KimiProvider {
    pub fn new(http: reqwest::Client) -> Self {
        Self::build(
            http,
            DEFAULT_BASE_URL.into(),
            Some(DEFAULT_OPENAI_BASE_URL.into()),
            AuthHeader::Passthrough,
        )
    }

    pub fn with_auth(http: reqwest::Client, auth: AuthHeader) -> Self {
        Self::build(
            http,
            DEFAULT_BASE_URL.into(),
            Some(DEFAULT_OPENAI_BASE_URL.into()),
            auth,
        )
    }

    pub fn configure(
        http: reqwest::Client,
        base_url: Option<String>,
        openai_base_url: Option<String>,
        auth: AuthHeader,
    ) -> Self {
        Self::build(
            http,
            base_url.unwrap_or_else(|| DEFAULT_BASE_URL.into()),
            Some(openai_base_url.unwrap_or_else(|| DEFAULT_OPENAI_BASE_URL.into())),
            auth,
        )
    }

    fn build(
        http: reqwest::Client,
        base_url: String,
        openai_base_url: Option<String>,
        auth: AuthHeader,
    ) -> Self {
        Self {
            base_url,
            openai_base_url,
            http,
            auth,
        }
    }
}

#[async_trait]
impl Provider for KimiProvider {
    fn name(&self) -> &'static str {
        "kimi"
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
        messages_protocol::forward(
            &self.http,
            &self.base_url,
            &self.auth,
            path,
            headers,
            body,
            streaming,
            self.name(),
        )
        .await
    }

    async fn forward_openai(
        &self,
        path: &str,
        headers: &HeaderMap,
        body: Bytes,
        streaming: bool,
    ) -> Result<UpstreamResponse, ProxyError> {
        let openai_base = self.openai_base_url.as_deref().ok_or_else(|| {
            ProxyError::BadRequest(
                "provider 'kimi' does not support OpenAI chat completions format".into(),
            )
        })?;
        // OpenAI format uses Authorization: Bearer. Convert ApiKey → Bearer.
        let openai_auth = match &self.auth {
            AuthHeader::Passthrough => AuthHeader::Passthrough,
            AuthHeader::ApiKey(v) => AuthHeader::Bearer(v.clone()),
            other => other.clone(),
        };
        messages_protocol::forward(
            &self.http,
            openai_base,
            &openai_auth,
            path,
            headers,
            body,
            streaming,
            self.name(),
        )
        .await
    }
}
```

Then register in `crates/proxy/src/adapters/providers/mod.rs`: add `pub mod kimi;` in the module block (alphabetical-ish, after `pub mod deepseek;`) and `pub use kimi::KimiProvider;` in the `pub use` block (after `pub use deepseek::DeepSeekProvider;`).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p proxy kimi:: 2>&1 | tail -20`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/proxy/src/adapters/providers/kimi.rs crates/proxy/src/adapters/providers/mod.rs
git commit -m "feat(proxy): add KimiProvider adapter"
```

---

### Task 2: KimiAccountUsage balance adapter

**Files:**
- Create: `crates/proxy/src/adapters/providers/account_usage/kimi.rs`
- Modify: `crates/proxy/src/adapters/providers/account_usage/mod.rs` (add `pub mod kimi;` after line 5 `pub mod deepseek;`, and `pub use kimi::KimiAccountUsage;` after line 12 `pub use deepseek::DeepSeekAccountUsage;`)

**Interfaces:**
- Consumes: `crate::application::ports::AccountUsagePort`; `crate::domain::account_usage::{AccountUsageStatus, ProviderAccountUsage, UsageSubItem, UsageWindow}`; `crate::application::errors::ProxyError`.
- Produces: `KimiAccountUsage` with `pub fn new(provider_name: String, auth_token: String) -> Self` and `impl AccountUsagePort` (`fetch_usage(&self) -> Option<Result<ProviderAccountUsage, ProxyError>>`).

- [ ] **Step 1: Write the failing test**

Create `crates/proxy/src/adapters/providers/account_usage/kimi.rs` with only the test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_balance_response() {
        let json = r#"{
            "code": 0,
            "data": { "available_balance": 49.58, "voucher_balance": 46.58, "cash_balance": 3.00 },
            "scode": "0x0",
            "status": true
        }"#;
        let resp: BalanceResponse = serde_json::from_str(json).unwrap();
        let data = resp.data.unwrap();
        assert!((data.available_balance - 49.58).abs() < 1e-6);
        assert!((data.cash_balance - 3.00).abs() < 1e-6);
        assert!((data.voucher_balance - 46.58).abs() < 1e-6);
    }

    #[test]
    fn deserialize_balance_missing_data() {
        let json = r#"{"code": 0, "status": true}"#;
        let resp: BalanceResponse = serde_json::from_str(json).unwrap();
        assert!(resp.data.is_none());
    }

    #[test]
    fn window_maps_available_and_is_balance_info() {
        let data = BalanceData { available_balance: 49.58, voucher_balance: 46.58, cash_balance: 3.0 };
        let w = balance_to_window(&data);
        assert!(w.is_balance_info);
        assert_eq!(w.label, "Balance");
        assert_eq!(w.sub_items[0].label, "available: 49.58");
    }

    #[test]
    fn empty_token_returns_none() {
        let a = KimiAccountUsage::new("kimi".into(), String::new());
        assert!(a.fetch_usage().is_none());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p proxy account_usage::kimi:: 2>&1 | tail -20`
Expected: FAIL — compile error, `BalanceResponse` / `KimiAccountUsage` not found.

- [ ] **Step 3: Write minimal implementation**

Prepend above the test module in `crates/proxy/src/adapters/providers/account_usage/kimi.rs`:

```rust
//! Kimi (Moonshot) account usage adapter.
//!
//! Queries Moonshot's `/v1/users/me/balance` endpoint to show account balance
//! as a usage window. Moonshot does not expose a public token-usage API.

use std::time::Duration;

use serde::Deserialize;

use crate::application::errors::ProxyError;
use crate::application::ports::AccountUsagePort;
use crate::domain::account_usage::{
    AccountUsageStatus, ProviderAccountUsage, UsageSubItem, UsageWindow,
};

const BALANCE_URL: &str = "https://api.moonshot.ai/v1/users/me/balance";

/// Kimi account usage adapter. Queries the Moonshot balance API.
pub struct KimiAccountUsage {
    provider_name: String,
    auth_token: String,
    agent: ureq::Agent,
}

impl KimiAccountUsage {
    pub fn new(provider_name: String, auth_token: String) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_read(Duration::from_secs(10))
            .timeout_write(Duration::from_secs(10))
            .build();
        Self {
            provider_name,
            auth_token,
            agent,
        }
    }
}

/// Converts Moonshot balance data into a balance usage window. Moonshot exposes
/// only remaining balance (no spend history), so values render as sub-items
/// rather than a progress bar.
fn balance_to_window(d: &BalanceData) -> UsageWindow {
    UsageWindow {
        label: "Balance".to_string(),
        used_pct: 0.0,
        used: None,
        limit: None,
        resets_at_ms: None,
        sub_items: vec![
            UsageSubItem {
                label: format!("available: {:.2}", d.available_balance),
                used: 0,
            },
            UsageSubItem {
                label: format!("cash: {:.2}", d.cash_balance),
                used: 0,
            },
            UsageSubItem {
                label: format!("voucher: {:.2}", d.voucher_balance),
                used: 0,
            },
        ],
        is_balance_info: true,
    }
}

impl AccountUsagePort for KimiAccountUsage {
    fn fetch_usage(&self) -> Option<Result<ProviderAccountUsage, ProxyError>> {
        if self.auth_token.is_empty() {
            return None;
        }

        let resp = match self
            .agent
            .get(BALANCE_URL)
            .set("Authorization", &format!("Bearer {}", self.auth_token))
            .set("Accept", "application/json")
            .call()
        {
            Ok(r) => r,
            Err(e) => {
                return Some(Err(ProxyError::UpstreamUsage {
                    provider: self.provider_name.clone(),
                    message: format!("balance API: {e}"),
                }));
            }
        };

        let envelope: BalanceResponse = match resp.into_json() {
            Ok(e) => e,
            Err(e) => {
                return Some(Err(ProxyError::UpstreamUsage {
                    provider: self.provider_name.clone(),
                    message: format!("balance decode: {e}"),
                }));
            }
        };

        let windows: Vec<UsageWindow> = envelope
            .data
            .as_ref()
            .map(|d| vec![balance_to_window(d)])
            .unwrap_or_default();

        Some(Ok(ProviderAccountUsage {
            provider: self.provider_name.clone(),
            status: AccountUsageStatus::Available,
            plan: None,
            windows,
            model_usage: None,
        }))
    }
}

// ---------------------------------------------------------------------------
// Moonshot response JSON shapes
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct BalanceResponse {
    #[serde(default)]
    data: Option<BalanceData>,
}

#[derive(Debug, Deserialize)]
struct BalanceData {
    #[serde(default)]
    available_balance: f64,
    #[serde(default)]
    voucher_balance: f64,
    #[serde(default)]
    cash_balance: f64,
}
```

Then register in `crates/proxy/src/adapters/providers/account_usage/mod.rs`: add `pub mod kimi;` and `pub use kimi::KimiAccountUsage;`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p proxy account_usage::kimi:: 2>&1 | tail -20`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/proxy/src/adapters/providers/account_usage/kimi.rs crates/proxy/src/adapters/providers/account_usage/mod.rs
git commit -m "feat(proxy): add KimiAccountUsage balance adapter"
```

---

### Task 3: Register ProviderKind::Kimi across the proxy crate

This task adds the enum variant and every exhaustive-match arm in the `proxy` crate. Because the matches are exhaustive (no `_`), the crate does not compile until all arms exist — so all edits land in one commit.

**Files:**
- Modify: `crates/proxy/src/config.rs` — `ProviderKind` enum (lines 47-58), `parse_kind` test helper (lines 298-307), tests module.
- Modify: `crates/proxy/src/adapters/providers/builder.rs` — imports (lines 6, 10), `build_provider` match (near line 86), `build_account_usage` match (near line 224).
- Modify: `crates/proxy/src/adapters/storage/db_config.rs` — `parse_kind` (lines 294-304), `kind_to_str` (lines 306-315).
- Modify: `crates/proxy/src/application/use_cases/admin.rs` — `kind_to_str` (lines 863-872), `str_to_kind` (lines 875-887).

**Interfaces:**
- Consumes: `KimiProvider::configure` (Task 1), `KimiAccountUsage::new` (Task 2), `resolve_auth_token` (existing in builder.rs).
- Produces: `ProviderKind::Kimi` usable everywhere; canonical string `"kimi"`, aliases `"kimi"`/`"moonshot"`.

- [ ] **Step 1: Write the failing test**

In `crates/proxy/src/config.rs` tests module, add:

```rust
#[test]
fn provider_kind_deserializes_kimi_aliases() {
    assert_eq!(
        serde_json::from_str::<ProviderKind>("\"kimi\"").unwrap(),
        ProviderKind::Kimi
    );
    assert_eq!(
        serde_json::from_str::<ProviderKind>("\"moonshot\"").unwrap(),
        ProviderKind::Kimi
    );
}

#[test]
fn parse_kind_accepts_kimi_aliases() {
    assert_eq!(parse_kind("kimi"), Some(ProviderKind::Kimi));
    assert_eq!(parse_kind("moonshot"), Some(ProviderKind::Kimi));
    assert_eq!(parse_kind("KIMI"), Some(ProviderKind::Kimi));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p proxy provider_kind_deserializes_kimi_aliases 2>&1 | tail -20`
Expected: FAIL — `ProviderKind::Kimi` not found (compile error).

- [ ] **Step 3: Write minimal implementation**

**3a.** `crates/proxy/src/config.rs` — add the variant to `ProviderKind` (after `Minimax,`):

```rust
    Minimax,
    #[serde(alias = "kimi", alias = "moonshot")]
    Kimi,
```

**3b.** `crates/proxy/src/config.rs` — add to the `parse_kind` test helper (before `_ => None,`):

```rust
        "kimi" | "moonshot" => Some(ProviderKind::Kimi),
```

**3c.** `crates/proxy/src/adapters/providers/builder.rs` — extend imports. On line 10 add `KimiProvider` to the provider `use` list, and on line 6 add `KimiAccountUsage` to the account-usage `use` list. Then add to the `build_provider` match (after the `ProviderKind::Minimax => { ... }` arm), mirroring the Zai arm:

```rust
        ProviderKind::Kimi => Arc::new(KimiProvider::configure(
            http,
            p.base_url.clone(),
            p.openai_base_url.clone(),
            auth,
        )),
```

And add to the `build_account_usage` match (after the `ProviderKind::Minimax => { ... }` arm):

```rust
                ProviderKind::Kimi => {
                    let token = resolve_auth_token(&p.auth);
                    Arc::new(KimiAccountUsage::new(p.name.clone(), token))
                }
```

**3d.** `crates/proxy/src/adapters/storage/db_config.rs` — in `parse_kind` add before `_ =>`:

```rust
        "kimi" | "moonshot" => ProviderKind::Kimi,
```

and in `kind_to_str` add before the closing brace:

```rust
        ProviderKind::Kimi => "kimi",
```

**3e.** `crates/proxy/src/application/use_cases/admin.rs` — in `kind_to_str` add:

```rust
        ProviderKind::Kimi => "kimi",
```

and in `str_to_kind` add before the `other =>` arm:

```rust
        "kimi" | "moonshot" => Ok(ProviderKind::Kimi),
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p proxy 2>&1 | tail -25`
Expected: PASS — new alias tests pass, whole `proxy` crate compiles (all exhaustive matches satisfied).

- [ ] **Step 5: Commit**

```bash
git add crates/proxy/src/config.rs crates/proxy/src/adapters/providers/builder.rs crates/proxy/src/adapters/storage/db_config.rs crates/proxy/src/application/use_cases/admin.rs
git commit -m "feat(proxy): register Kimi provider kind and wiring"
```

---

### Task 4: proxy-tui support for Kimi

The `proxy-tui` crate has its own `ProviderKind` enum (string-based, independent of the proxy crate). Add `Kimi` to it and its exhaustive matches, plus the suggested test model.

**Files:**
- Modify: `crates/proxy-tui/src/app.rs` — `ProviderKind` enum (lines 286-295), `label` (298-307), `cycle_next` (308-317), `cycle_prev` (318-327), `from_str_or_default` (328-337), tests (near lines 1135-1145).
- Modify: `crates/proxy-tui/src/main.rs` — `open_test_modal` suggested-model match (lines 869-875).

**Interfaces:**
- Consumes: nothing from earlier tasks (separate crate).
- Produces: `ProviderKind::Kimi` in `proxy-tui` with `label() == "kimi"`, cycle order `… Minimax → Kimi → Anthropic`.

- [ ] **Step 1: Write the failing test**

In `crates/proxy-tui/src/app.rs` tests, extend the cycle round-trip assertions. Add:

```rust
#[test]
fn cycle_includes_kimi() {
    assert_eq!(ProviderKind::Minimax.cycle_next(), ProviderKind::Kimi);
    assert_eq!(ProviderKind::Kimi.cycle_next(), ProviderKind::Anthropic);
    assert_eq!(ProviderKind::Anthropic.cycle_prev(), ProviderKind::Kimi);
    assert_eq!(ProviderKind::Kimi.cycle_prev(), ProviderKind::Minimax);
    assert_eq!(ProviderKind::from_str_or_default("kimi"), ProviderKind::Kimi);
    assert_eq!(ProviderKind::Kimi.label(), "kimi");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p proxy-tui cycle_includes_kimi 2>&1 | tail -20`
Expected: FAIL — `ProviderKind::Kimi` not found (compile error).

- [ ] **Step 3: Write minimal implementation**

**3a.** `crates/proxy-tui/src/app.rs` — add `Kimi,` to the enum after `Minimax,`:

```rust
    Minimax,
    Kimi,
```

**3b.** `label` — add before the closing brace:

```rust
            ProviderKind::Kimi => "kimi",
```

**3c.** `cycle_next` — change the `Minimax` arm and add a `Kimi` arm so the cycle closes through Kimi:

```rust
            ProviderKind::Codex => ProviderKind::Minimax,
            ProviderKind::Minimax => ProviderKind::Kimi,
            ProviderKind::Kimi => ProviderKind::Anthropic,
```

**3d.** `cycle_prev` — change the `Anthropic` arm and add a `Kimi` arm:

```rust
            ProviderKind::Anthropic => ProviderKind::Kimi,
            ProviderKind::Zai => ProviderKind::Anthropic,
            ProviderKind::DeepSeek => ProviderKind::Zai,
            ProviderKind::OpenAi => ProviderKind::DeepSeek,
            ProviderKind::Codex => ProviderKind::OpenAi,
            ProviderKind::Minimax => ProviderKind::Codex,
            ProviderKind::Kimi => ProviderKind::Minimax,
```

**3e.** `from_str_or_default` — add before `_ =>`:

```rust
            "kimi" | "moonshot" => ProviderKind::Kimi,
```

**3f.** `crates/proxy-tui/src/main.rs` — in `open_test_modal`'s `match prov.kind.as_str()`, add before `_ => ""`:

```rust
        "kimi" => "kimi-k2-0711-preview",
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p proxy-tui 2>&1 | tail -25`
Expected: PASS — `cycle_includes_kimi` passes; existing cycle tests still pass; crate compiles.

- [ ] **Step 5: Commit**

```bash
git add crates/proxy-tui/src/app.rs crates/proxy-tui/src/main.rs
git commit -m "feat(proxy-tui): add Kimi provider kind"
```

---

### Task 5: Workspace gates

**Files:** none (verification only).

- [ ] **Step 1: Full workspace build + tests**

Run: `cargo test --workspace 2>&1 | tail -30`
Expected: PASS (all crates).

- [ ] **Step 2: Clippy**

Run: `cargo clippy --workspace -- -D warnings 2>&1 | tail -30`
Expected: no warnings.

- [ ] **Step 3: Format**

Run: `cargo fmt --all && git diff --stat`
Expected: no or minimal formatting drift; if `cargo fmt` changed files, review and commit.

- [ ] **Step 4: Commit any fmt drift (if produced)**

```bash
git add -A
git commit -m "style(proxy): cargo fmt drift for Kimi provider"
```

---

## Self-Review

- **Spec coverage:** Provider core (Task 1), account usage (Task 2), config enum + aliases + builder wiring + kind round-trip in db_config/admin (Task 3), proxy-tui enum + test model (Task 4), gates (Task 5). All spec sections mapped.
- **Deferred/non-goals:** `ui.rs:1817` fallback list and `proxy-admin-api` example data are example-only and not required (per spec §6); left untouched. No OAuth, no `tool_choice` stripping (spec non-goals).
- **Type consistency:** `KimiProvider::configure(http, base_url, openai_base_url, auth)` and `KimiAccountUsage::new(provider_name, token)` are used identically in Task 3 as defined in Tasks 1–2. `balance_to_window`/`BalanceResponse`/`BalanceData` names are consistent within Task 2. TUI `cycle_next`/`cycle_prev`/`label`/`from_str_or_default` match existing signatures.
- **Placeholder scan:** none.
