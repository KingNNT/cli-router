# Account Usage Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a new Account tab to `proxy-tui` that shows live upstream provider account-level quota and usage data. The proxy daemon queries each provider's usage API and exposes a single merged admin endpoint.

**Architecture:** New `AccountUsagePort` trait with per-provider adapters (`ZaiAccountUsage` hits Z.ai's three monitoring endpoints, `AnthropicAccountUsage` returns `None`). A `GetAccountUsage` use case iterates all adapters and produces a merged DTO. One new admin endpoint `GET /admin/account/usage` serves it. `proxy-tui` gains a new `View::Account` tab rendered in `views/account.rs`.

**Tech Stack:** Rust 2024, axum, ureq (new dep for proxy crate), ratatui, serde, thiserror, wiremock (dev).

**Spec:** `docs/superpowers/specs/2026-05-05-account-usage-design.md`

---

## File Structure

### New files

- `crates/proxy/src/domain/account_usage.rs` — domain types (`ProviderAccountUsage`, `UsageWindow`, etc.)
- `crates/proxy/src/application/ports/account_usage.rs` — `AccountUsagePort` trait
- `crates/proxy/src/adapters/providers/account_usage/mod.rs` — module declaration
- `crates/proxy/src/adapters/providers/account_usage/anthropic.rs` — no-op adapter
- `crates/proxy/src/adapters/providers/account_usage/zai.rs` — Z.ai adapter (ureq)
- `crates/proxy-tui/src/views/account.rs` — Account tab renderer
- `crates/proxy/tests/account_usage_api.rs` — integration test

### Modified files

- `crates/proxy/Cargo.toml` — add `ureq` dependency
- `crates/proxy/src/domain/mod.rs` — declare `account_usage`
- `crates/proxy/src/application/ports/mod.rs` — declare + re-export `account_usage`
- `crates/proxy/src/application/use_cases/admin.rs` — add `GetAccountUsage`
- `crates/proxy/src/application/errors.rs` — add `UpstreamUsage` error variant
- `crates/proxy/src/adapters/providers/builder.rs` — add `build_account_usage`
- `crates/proxy/src/frameworks/admin.rs` — add route + handler + `AdminState` field
- `crates/proxy/src/main.rs` — construct use case, add to `AdminState`
- `crates/proxy-admin-api/src/lib.rs` — add DTOs + serde test
- `crates/proxy-tui/src/app.rs` — add `View::Account`, `AccountPaneState`
- `crates/proxy-tui/src/client.rs` — add `get_account_usage`
- `crates/proxy-tui/src/main.rs` — wire keys + initial fetch + renderer dispatch
- `crates/proxy-tui/src/views/mod.rs` — declare `account` module
- `crates/proxy-tui/src/ui.rs` — dispatch `View::Account` to `views::account`

---

## Task 1: Domain types for account usage

**Files:**
- Create: `crates/proxy/src/domain/account_usage.rs`
- Modify: `crates/proxy/src/domain/mod.rs`

- [ ] **Step 1: Create `domain/account_usage.rs`**

```rust
//! Domain types for upstream provider account-level usage and quota.

/// One provider's account-level usage snapshot.
#[derive(Debug, Clone)]
pub struct ProviderAccountUsage {
    pub provider: String,
    pub status: AccountUsageStatus,
    pub plan: Option<String>,
    pub windows: Vec<UsageWindow>,
    pub model_usage: Option<ModelUsageSnapshot>,
}

/// Whether the adapter could retrieve usage data.
#[derive(Debug, Clone, PartialEq)]
pub enum AccountUsageStatus {
    /// Data successfully retrieved.
    Available,
    /// This provider kind has no public usage API (e.g. Anthropic).
    NotSupported,
    /// Upstream call failed.
    Error(String),
}

/// One quota window (e.g. "5h Token", "Weekly", "MCP (1 Month)").
#[derive(Debug, Clone)]
pub struct UsageWindow {
    /// Human-readable label, e.g. "5h Token", "Weekly", "MCP (1 Month)".
    pub label: String,
    /// Percentage used, 0.0–100.0.
    pub used_pct: f64,
    /// Absolute usage count, if the API returns it.
    pub used: Option<u64>,
    /// Absolute limit, if the API returns it.
    pub limit: Option<u64>,
    /// When this window resets, in epoch milliseconds.
    pub resets_at_ms: Option<i64>,
    /// Sub-items (e.g. individual MCP tool counts under the MCP window).
    pub sub_items: Vec<UsageSubItem>,
}

/// A sub-item within a quota window (e.g. "Network Searches: 5678").
#[derive(Debug, Clone)]
pub struct UsageSubItem {
    pub label: String,
    pub used: u64,
}

/// Model usage snapshot over a time window (typically 24h).
#[derive(Debug, Clone)]
pub struct ModelUsageSnapshot {
    pub total_tokens: u64,
    pub total_calls: u64,
    pub period_start_ms: i64,
    pub period_end_ms: i64,
}
```

- [ ] **Step 2: Declare the module in `domain/mod.rs`**

Append to `crates/proxy/src/domain/mod.rs`:

```rust
pub mod account_usage;
```

Match the existing pattern — the file already declares `quota`, `request_log`, `usage_record`, `usage_summary`.

- [ ] **Step 3: Verify it compiles**

Run:
```bash
cargo check -p proxy
```
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/proxy/src/domain/account_usage.rs crates/proxy/src/domain/mod.rs
git commit -m "feat(proxy): add account usage domain types"
```

---

## Task 2: Application port + error variant

**Files:**
- Create: `crates/proxy/src/application/ports/account_usage.rs`
- Modify: `crates/proxy/src/application/ports/mod.rs`
- Modify: `crates/proxy/src/application/errors.rs`

- [ ] **Step 1: Create the port file**

Create `crates/proxy/src/application/ports/account_usage.rs`:

```rust
//! AccountUsagePort — application-layer interface for querying upstream
//! provider account-level usage and quota.

use crate::application::errors::ProxyError;
use crate::domain::account_usage::ProviderAccountUsage;

/// Each configured provider gets one adapter implementing this trait.
/// The use case iterates all adapters and merges the results.
pub trait AccountUsagePort: Send + Sync {
    /// Fetch account-level usage/quota from the upstream provider.
    ///
    /// Returns `None` if this provider kind does not support usage queries
    /// (e.g. Anthropic has no public usage API).
    /// Returns `Some(Ok(..))` with the snapshot on success.
    /// Returns `Some(Err(..))` on upstream failure.
    fn fetch_usage(&self) -> Option<Result<ProviderAccountUsage, ProxyError>>;
}
```

- [ ] **Step 2: Declare and re-export in `ports/mod.rs`**

Append to `crates/proxy/src/application/ports/mod.rs`:

```rust
pub mod account_usage;

pub use account_usage::AccountUsagePort;
```

Match the existing pattern — the file already declares `provider`, `quota`, `request_log`, `request_log_read`, `upstream`, `usage_parser`.

- [ ] **Step 3: Add error variant for upstream usage failures**

In `crates/proxy/src/application/errors.rs`, add a new variant to `ProxyError`:

```rust
#[error("upstream usage query failed for {provider}: {message}")]
UpstreamUsage {
    provider: String,
    message: String,
},
```

Add it before the closing `}` of the enum. Match the existing style — all variants use thiserror `#[error(...)]` attributes.

- [ ] **Step 4: Map the new variant to 500 in the framework error handler**

In `crates/proxy/src/frameworks/error.rs`, find the `IntoResponse` impl for `ProxyError` and add a branch:

```rust
ProxyError::UpstreamUsage { provider, message } => {
    let body = serde_json::json!({
        "error": {
            "type": "upstream_usage_error",
            "message": format!("upstream usage query failed for {provider}: {message}"),
        }
    });
    (StatusCode::BAD_GATEWAY, Json(body)).into_response()
}
```

Use `StatusCode::BAD_GATEWAY` (502) — the proxy's upstream call failed. Match the existing pattern used by `UpstreamRateLimited` and other variants.

- [ ] **Step 5: Verify it compiles**

Run:
```bash
cargo check -p proxy
```
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/proxy/src/application/ports/account_usage.rs \
        crates/proxy/src/application/ports/mod.rs \
        crates/proxy/src/application/errors.rs \
        crates/proxy/src/frameworks/error.rs
git commit -m "feat(proxy): add AccountUsagePort trait and UpstreamUsage error"
```

---

## Task 3: DTOs in proxy-admin-api

**Files:**
- Modify: `crates/proxy-admin-api/src/lib.rs`

- [ ] **Step 1: Add DTOs at the end of the file (before any `#[cfg(test)]` blocks)**

Append to `crates/proxy-admin-api/src/lib.rs`:

```rust
// ---------------------------------------------------------------------------
// Account usage DTOs
// ---------------------------------------------------------------------------

/// `GET /admin/account/usage`
///
/// Merged account-level usage from all configured providers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountUsageResponse {
    pub providers: Vec<ProviderAccountUsageDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderAccountUsageDto {
    pub provider: String,
    pub status: ProviderUsageStatus,
    pub plan: Option<String>,
    pub windows: Vec<UsageWindowDto>,
    pub model_usage: Option<ModelUsageDto>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderUsageStatus {
    Available,
    NotSupported,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageWindowDto {
    pub label: String,
    pub used_pct: f64,
    pub used: Option<u64>,
    pub limit: Option<u64>,
    pub resets_at_ms: Option<i64>,
    pub sub_items: Vec<UsageSubItemDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageSubItemDto {
    pub label: String,
    pub used: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelUsageDto {
    pub total_tokens: u64,
    pub total_calls: u64,
    pub period_start_ms: i64,
    pub period_end_ms: i64,
}
```

- [ ] **Step 2: Add a serde round-trip test**

Append to `crates/proxy-admin-api/src/lib.rs` (after existing test modules):

```rust
#[cfg(test)]
mod account_usage_tests {
    use super::*;

    #[test]
    fn account_usage_response_round_trips_through_json() {
        let original = AccountUsageResponse {
            providers: vec![
                ProviderAccountUsageDto {
                    provider: "zai".into(),
                    status: ProviderUsageStatus::Available,
                    plan: Some("pro".into()),
                    windows: vec![UsageWindowDto {
                        label: "5h Token".into(),
                        used_pct: 40.5,
                        used: Some(16_200_000),
                        limit: Some(40_000_000),
                        resets_at_ms: Some(1_746_300_000_000),
                        sub_items: vec![],
                    }],
                    model_usage: Some(ModelUsageDto {
                        total_tokens: 12_500_000,
                        total_calls: 1_234,
                        period_start_ms: 1_746_220_800_000,
                        period_end_ms: 1_746_292_800_000,
                    }),
                },
                ProviderAccountUsageDto {
                    provider: "anthropic".into(),
                    status: ProviderUsageStatus::NotSupported,
                    plan: None,
                    windows: vec![],
                    model_usage: None,
                },
            ],
        };

        let json = serde_json::to_string(&original).unwrap();
        let decoded: AccountUsageResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn provider_usage_status_serializes_as_snake_case() {
        let s = serde_json::to_string(&ProviderUsageStatus::NotSupported).unwrap();
        assert_eq!(s, "\"not_supported\"");
    }
}
```

- [ ] **Step 3: Run the test**

Run:
```bash
cargo test -p proxy-admin-api account_usage
```
Expected: PASS (2 tests).

- [ ] **Step 4: Commit**

```bash
git add crates/proxy-admin-api/src/lib.rs
git commit -m "feat(proxy-admin-api): add account usage DTOs"
```

---

## Task 4: Anthropic no-op adapter

**Files:**
- Create: `crates/proxy/src/adapters/providers/account_usage/mod.rs`
- Create: `crates/proxy/src/adapters/providers/account_usage/anthropic.rs`

This task also sets up the module structure. The Z.ai adapter comes in Task 5.

- [ ] **Step 1: Create `account_usage/mod.rs`**

Create `crates/proxy/src/adapters/providers/account_usage/mod.rs`:

```rust
//! Per-provider adapters for upstream account-level usage queries.

pub mod anthropic;
pub mod zai;

pub use anthropic::AnthropicAccountUsage;
pub use zai::ZaiAccountUsage;
```

Note: `zai` module is declared now but the file is created in Task 5. This is fine — Rust allows declaring a module and creating the file later, but compilation will fail until Task 5 is done. Tasks 4 and 5 should be committed together if you want a green build between them. Alternatively, you can create a stub `zai.rs` with just the struct definition and fill it in Task 5. The plan treats them as separate logical tasks but they may be committed together.

- [ ] **Step 2: Create `anthropic.rs`**

Create `crates/proxy/src/adapters/providers/account_usage/anthropic.rs`:

```rust
//! Anthropic has no public usage API. This adapter always returns `None`.

use crate::application::errors::ProxyError;
use crate::application::ports::AccountUsagePort;
use crate::domain::account_usage::ProviderAccountUsage;

/// No-op account usage adapter for Anthropic providers.
/// Always returns `None` because Anthropic has no public usage/quota API.
pub struct AnthropicAccountUsage;

impl AccountUsagePort for AnthropicAccountUsage {
    fn fetch_usage(&self) -> Option<Result<ProviderAccountUsage, ProxyError>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_returns_none() {
        let adapter = AnthropicAccountUsage;
        assert!(adapter.fetch_usage().is_none());
    }
}
```

- [ ] **Step 3: Verify the anthropic test passes**

This will fail to compile because `zai.rs` doesn't exist yet. Create a minimal stub for `zai.rs` to unblock:

Create `crates/proxy/src/adapters/providers/account_usage/zai.rs`:

```rust
//! Z.ai account usage adapter — stub, implemented in Task 5.

use crate::application::errors::ProxyError;
use crate::application::ports::AccountUsagePort;
use crate::domain::account_usage::ProviderAccountUsage;

pub struct ZaiAccountUsage;

impl AccountUsagePort for ZaiAccountUsage {
    fn fetch_usage(&self) -> Option<Result<ProviderAccountUsage, ProxyError>> {
        None // stub — replaced in Task 5
    }
}
```

Run:
```bash
cargo test -p proxy --lib adapters::providers::account_usage::anthropic
```
Expected: PASS (1 test).

- [ ] **Step 4: Commit Tasks 4 + 5 stub together**

```bash
git add crates/proxy/src/adapters/providers/account_usage/
git commit -m "feat(proxy): add account_usage adapter module + anthropic no-op + zai stub"
```

---

## Task 5: Z.ai adapter (TDD)

**Files:**
- Modify: `crates/proxy/src/adapters/providers/account_usage/zai.rs`
- Modify: `crates/proxy/Cargo.toml` — add `ureq` dependency

The adapter hits three Z.ai endpoints:
1. `GET /api/monitor/usage/quota/limit` → quota windows + plan tier
2. `GET /api/monitor/usage/model-usage?startTime=...&endTime=...` → 24h model stats
3. `GET /api/monitor/usage/tool-usage?startTime=...&endTime=...` → MCP tool counts

All via sync `ureq` calls. Auth: `Authorization: <token>` (no "Bearer" prefix).

- [ ] **Step 1: Add `ureq` to proxy's Cargo.toml**

In `crates/proxy/Cargo.toml`, under `[dependencies]`, add:

```toml
ureq = { workspace = true }
```

- [ ] **Step 2: Replace the zai.rs stub with the full implementation**

Replace the entire contents of `crates/proxy/src/adapters/providers/account_usage/zai.rs` with:

```rust
//! Z.ai account usage adapter.
//!
//! Queries three Z.ai monitoring endpoints to gather quota windows,
//! model usage, and MCP tool breakdown.

use std::collections::HashMap;
use std::time::Duration;

use crate::application::errors::ProxyError;
use crate::application::ports::AccountUsagePort;
use crate::domain::account_usage::{
    AccountUsageStatus, ModelUsageSnapshot, ProviderAccountUsage, UsageSubItem, UsageWindow,
};

/// Z.ai account usage adapter. Queries Z.ai's monitoring API.
pub struct ZaiAccountUsage {
    provider_name: String,
    auth_token: String,
    base_url: String,
    agent: ureq::Agent,
}

impl ZaiAccountUsage {
    pub fn new(provider_name: String, auth_token: String, base_url: String) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_read(Duration::from_secs(10))
            .timeout_write(Duration::from_secs(10))
            .build();
        Self {
            provider_name,
            auth_token,
            base_url,
            agent,
        }
    }

    fn fetch_quota_limit(&self) -> Result<QuotaLimitResponse, ureq::Error> {
        let url = format!("{}/api/monitor/usage/quota/limit", self.base_url);
        self.agent
            .get(&url)
            .set("Authorization", &self.auth_token)
            .set("Accept-Language", "en-US,en")
            .call()?
            .body_mut()
            .read_json()
    }

    fn fetch_model_usage(
        &self,
        start_ms: i64,
        end_ms: i64,
    ) -> Result<ModelUsageResponse, ureq::Error> {
        let url = format!(
            "{}/api/monitor/usage/model-usage?startTime={start_ms}&endTime={end_ms}",
            self.base_url
        );
        self.agent
            .get(&url)
            .set("Authorization", &self.auth_token)
            .set("Accept-Language", "en-US,en")
            .call()?
            .body_mut()
            .read_json()
    }

    fn fetch_tool_usage(
        &self,
        start_ms: i64,
        end_ms: i64,
    ) -> Result<ToolUsageResponse, ureq::Error> {
        let url = format!(
            "{}/api/monitor/usage/tool-usage?startTime={start_ms}&endTime={end_ms}",
            self.base_url
        );
        self.agent
            .get(&url)
            .set("Authorization", &self.auth_token)
            .set("Accept-Language", "en-US,en")
            .call()?
            .body_mut()
            .read_json()
    }
}

impl AccountUsagePort for ZaiAccountUsage {
    fn fetch_usage(&self) -> Option<Result<ProviderAccountUsage, ProxyError>> {
        let mut windows = Vec::new();
        let mut plan = None;
        let mut model_usage = None;

        // 1. Quota/limit endpoint (primary).
        match self.fetch_quota_limit() {
            Ok(data) => {
                plan = data.level;
                if let Some(limits) = data.limits {
                    let mut token_idx = 0;
                    for limit in &limits {
                        match limit.limit_type.as_str() {
                            "TOKENS_LIMIT" => {
                                let label = if token_idx == 0 {
                                    "5h Token"
                                } else {
                                    "Weekly"
                                };
                                token_idx += 1;
                                windows.push(UsageWindow {
                                    label: label.to_string(),
                                    used_pct: limit.percentage.unwrap_or(0.0),
                                    used: limit
                                        .used
                                        .or_else(|| {
                                            limit
                                                .total
                                                .map(|t| {
                                                    ((limit.percentage.unwrap_or(0.0) / 100.0)
                                                        * t as f64)
                                                        as u64
                                                })
                                        }),
                                    limit: limit.total,
                                    resets_at_ms: limit.next_reset_time,
                                    sub_items: vec![],
                                });
                            }
                            "TIME_LIMIT" => {
                                // For TIME_LIMIT: `usage` is the total limit,
                                // `current_value` is the used amount.
                                let mut sub_items = Vec::new();
                                if let Some(details) = &limit.usage_details {
                                    for d in details {
                                        sub_items.push(UsageSubItem {
                                            label: mcp_tool_label(&d.model_code),
                                            used: d.usage,
                                        });
                                    }
                                }
                                windows.push(UsageWindow {
                                    label: "MCP (1 Month)".to_string(),
                                    used_pct: limit.percentage.unwrap_or(0.0),
                                    used: limit.current_value,
                                    limit: limit.usage,
                                    resets_at_ms: limit.next_reset_time,
                                    sub_items,
                                });
                            }
                            _ => {}
                        }
                    }
                }
            }
            Err(e) => {
                return Some(Err(ProxyError::UpstreamUsage {
                    provider: self.provider_name.clone(),
                    message: format!("quota/limit: {e}"),
                }));
            }
        }

        // 2. Model usage (24h window). Failure here is non-fatal.
        let now_ms = now_epoch_millis();
        let start_ms = now_ms - 24 * 3600 * 1000;
        if let Ok(data) = self.fetch_model_usage(start_ms, now_ms) {
            if let Some(total) = data.total_usage {
                model_usage = Some(ModelUsageSnapshot {
                    total_tokens: total.total_tokens_usage.unwrap_or(0),
                    total_calls: total.total_model_call_count.unwrap_or(0),
                    period_start_ms: start_ms,
                    period_end_ms: now_ms,
                });
            }
        }

        // 3. Tool usage (24h window). Merge into MCP window's sub_items
        //    if the MCP window exists but has no sub_items.
        if let Ok(data) = self.fetch_tool_usage(start_ms, now_ms) {
            if let Some(total) = data.total_usage {
                // Find MCP window sub-items. Only fill if empty.
                if let Some(mcp) = windows.iter_mut().find(|w| w.label == "MCP (1 Month)") {
                    if mcp.sub_items.is_empty() {
                        if total.total_network_search_count > 0 {
                            mcp.sub_items.push(UsageSubItem {
                                label: "Network Searches".to_string(),
                                used: total.total_network_search_count,
                            });
                        }
                        if total.total_web_read_mcp_count > 0 {
                            mcp.sub_items.push(UsageSubItem {
                                label: "Web Reads".to_string(),
                                used: total.total_web_read_mcp_count,
                            });
                        }
                        if total.total_zread_mcp_count > 0 {
                            mcp.sub_items.push(UsageSubItem {
                                label: "ZRead Calls".to_string(),
                                used: total.total_zread_mcp_count,
                            });
                        }
                    }
                }
            }
        }

        Some(Ok(ProviderAccountUsage {
            provider: self.provider_name.clone(),
            status: AccountUsageStatus::Available,
            plan,
            windows,
            model_usage,
        }))
    }
}

fn mcp_tool_label(code: &str) -> String {
    match code {
        "search-prime" => "Network Searches".to_string(),
        "web-reader" => "Web Reads".to_string(),
        "zread" => "ZRead Calls".to_string(),
        other => other.to_string(),
    }
}

fn now_epoch_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

// ---------------------------------------------------------------------------
// Z.ai response JSON shapes (deserialized with serde)
// ---------------------------------------------------------------------------

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct QuotaLimitResponse {
    level: Option<String>,
    limits: Option<Vec<QuotaLimitItem>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuotaLimitItem {
    #[serde(rename = "type")]
    limit_type: String,
    percentage: Option<f64>,
    total: Option<u64>,
    #[serde(default)]
    current_value: Option<u64>,
    #[serde(default)]
    usage: Option<u64>,
    #[serde(default)]
    usage_details: Option<Vec<UsageDetail>>,
    next_reset_time: Option<i64>,
    // Some items report `used` directly; capture it if present.
    #[serde(default)]
    used: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct UsageDetail {
    model_code: String,
    usage: u64,
}

#[derive(Debug, Deserialize)]
struct ModelUsageResponse {
    #[serde(rename = "totalUsage")]
    total_usage: Option<ModelTotalUsage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelTotalUsage {
    #[serde(default)]
    total_tokens_usage: Option<u64>,
    #[serde(default)]
    total_model_call_count: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ToolUsageResponse {
    #[serde(rename = "totalUsage")]
    total_usage: Option<ToolTotalUsage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ToolTotalUsage {
    #[serde(default)]
    total_network_search_count: u64,
    #[serde(default)]
    total_web_read_mcp_count: u64,
    #[serde(default)]
    total_zread_mcp_count: u64,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_tool_label_maps_known_codes() {
        assert_eq!(mcp_tool_label("search-prime"), "Network Searches");
        assert_eq!(mcp_tool_label("web-reader"), "Web Reads");
        assert_eq!(mcp_tool_label("zread"), "ZRead Calls");
        assert_eq!(mcp_tool_label("other"), "other");
    }

    #[test]
    fn deserialize_quota_limit_response() {
        let json = r#"{
            "level": "pro",
            "limits": [
                {
                    "type": "TOKENS_LIMIT",
                    "percentage": 40.5,
                    "total": 40000000,
                    "nextResetTime": 1746300000000
                },
                {
                    "type": "TIME_LIMIT",
                    "percentage": 12.3,
                    "currentValue": 123,
                    "usage": 1000,
                    "usageDetails": [
                        {"modelCode": "search-prime", "usage": 5678}
                    ]
                }
            ]
        }"#;
        let resp: QuotaLimitResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.level.as_deref(), Some("pro"));
        let limits = resp.limits.unwrap();
        assert_eq!(limits.len(), 2);
        assert_eq!(limits[0].limit_type, "TOKENS_LIMIT");
        assert_eq!(limits[0].percentage, Some(40.5));
        assert_eq!(limits[1].limit_type, "TIME_LIMIT");
        assert_eq!(limits[1].current_value, Some(123));
    }

    #[test]
    fn deserialize_model_usage_response() {
        let json = r#"{
            "totalUsage": {
                "totalTokensUsage": 12500000,
                "totalModelCallCount": 1234
            }
        }"#;
        let resp: ModelUsageResponse = serde_json::from_str(json).unwrap();
        let total = resp.total_usage.unwrap();
        assert_eq!(total.total_tokens_usage, Some(12_500_000));
        assert_eq!(total.total_model_call_count, Some(1234));
    }

    #[test]
    fn deserialize_tool_usage_response() {
        let json = r#"{
            "totalUsage": {
                "totalNetworkSearchCount": 5678,
                "totalWebReadMcpCount": 2345,
                "totalZreadMcpCount": 890
            }
        }"#;
        let resp: ToolUsageResponse = serde_json::from_str(json).unwrap();
        let total = resp.total_usage.unwrap();
        assert_eq!(total.total_network_search_count, 5678);
        assert_eq!(total.total_web_read_mcp_count, 2345);
        assert_eq!(total.total_zread_mcp_count, 890);
    }
}
```

- [ ] **Step 3: Run the unit tests**

Run:
```bash
cargo test -p proxy --lib adapters::providers::account_usage::zai
```
Expected: PASS (4 tests — label mapping + 3 deserialize tests).

- [ ] **Step 4: Verify full proxy build**

Run:
```bash
cargo build -p proxy
```
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/proxy/Cargo.toml crates/proxy/src/adapters/providers/account_usage/zai.rs
git commit -m "feat(proxy): Z.ai account usage adapter with quota/model/tool endpoints"
```

---

## Task 6: Builder method + use case

**Files:**
- Modify: `crates/proxy/src/adapters/providers/builder.rs`
- Modify: `crates/proxy/src/application/use_cases/admin.rs`

- [ ] **Step 1: Add `build_account_usage` to `builder.rs`**

Append to `crates/proxy/src/adapters/providers/builder.rs`:

```rust
use std::collections::HashMap;
use std::sync::Arc;

use crate::application::ports::AccountUsagePort;
use crate::adapters::providers::account_usage::{AnthropicAccountUsage, ZaiAccountUsage};

/// Build the per-name account usage adapter map.
///
/// Z.ai providers get a `ZaiAccountUsage` adapter (hits Z.ai's monitoring API).
/// All other kinds get `AnthropicAccountUsage` (returns `None` — no public API).
pub fn build_account_usage(
    providers: &[ProviderConfig],
) -> HashMap<String, Arc<dyn AccountUsagePort>> {
    providers
        .iter()
        .map(|p| {
            let adapter: Arc<dyn AccountUsagePort> = match p.kind {
                ProviderKind::Zai => {
                    let base_url = p
                        .openai_base_url
                        .as_deref()
                        .and_then(|u| {
                            // Extract scheme + host from openai_base_url
                            // e.g. "https://api.z.ai/api/coding/paas/v4" → "https://api.z.ai"
                            u.find("://").map(|idx| {
                                let rest = &u[idx + 3..];
                                let end = rest.find('/').unwrap_or(rest.len());
                                format!("{}://{}", &u[..idx + 3], &rest[..end])
                            })
                        })
                        .or(p.base_url.clone())
                        .unwrap_or_else(|| "https://api.z.ai".to_string());

                    let token = resolve_auth_token(&p.auth);
                    Arc::new(ZaiAccountUsage::new(p.name.clone(), token, base_url))
                }
                ProviderKind::Anthropic => Arc::new(AnthropicAccountUsage),
            };
            (p.name.clone(), adapter)
        })
        .collect()
}

/// Extract the bearer token value from an auth config for use in
/// the Z.ai monitoring API (`Authorization: <token>` header).
fn resolve_auth_token(auth: &AuthConfig) -> String {
    match auth {
        AuthConfig::ApiKey { value } => value.clone(),
        AuthConfig::Bearer { value } => value.clone(),
        AuthConfig::AnthropicOAuth { access_token, .. } => access_token.clone(),
        AuthConfig::Passthrough => String::new(),
    }
}
```

Add the necessary imports at the top of the file if not already present:

```rust
use crate::config::{AuthConfig, ProviderConfig, ProviderKind};
```

These may already be imported. Check the existing imports and add only what's missing.

- [ ] **Step 2: Add `GetAccountUsage` use case to `admin.rs`**

Append to `crates/proxy/src/application/use_cases/admin.rs`:

```rust
// ---------------------------------------------------------------------------
// GetAccountUsage
// ---------------------------------------------------------------------------

use std::collections::HashMap;
use crate::application::ports::AccountUsagePort;
use crate::domain::account_usage::{AccountUsageStatus, ProviderAccountUsage};
use proxy_admin_api::{
    AccountUsageResponse, ModelUsageDto, ProviderAccountUsageDto, ProviderUsageStatus,
    UsageSubItemDto, UsageWindowDto,
};

pub struct GetAccountUsage {
    adapters: HashMap<String, Arc<dyn AccountUsagePort>>,
}

impl GetAccountUsage {
    pub fn new(adapters: HashMap<String, Arc<dyn AccountUsagePort>>) -> Self {
        Self { adapters }
    }

    pub fn execute(&self) -> AccountUsageResponse {
        let mut providers: Vec<ProviderAccountUsageDto> = self
            .adapters
            .iter()
            .map(|(name, adapter)| match adapter.fetch_usage() {
                None => ProviderAccountUsageDto {
                    provider: name.clone(),
                    status: ProviderUsageStatus::NotSupported,
                    plan: None,
                    windows: vec![],
                    model_usage: None,
                },
                Some(Ok(usage)) => account_usage_to_dto(usage),
                Some(Err(_)) => ProviderAccountUsageDto {
                    provider: name.clone(),
                    status: ProviderUsageStatus::Error,
                    plan: None,
                    windows: vec![],
                    model_usage: None,
                },
            })
            .collect();

        // Sort by provider name for deterministic ordering.
        providers.sort_by(|a, b| a.provider.cmp(&b.provider));

        AccountUsageResponse { providers }
    }
}

fn account_usage_to_dto(u: ProviderAccountUsage) -> ProviderAccountUsageDto {
    ProviderAccountUsageDto {
        provider: u.provider,
        status: match u.status {
            AccountUsageStatus::Available => ProviderUsageStatus::Available,
            AccountUsageStatus::NotSupported => ProviderUsageStatus::NotSupported,
            AccountUsageStatus::Error(_) => ProviderUsageStatus::Error,
        },
        plan: u.plan,
        windows: u
            .windows
            .into_iter()
            .map(|w| UsageWindowDto {
                label: w.label,
                used_pct: w.used_pct,
                used: w.used,
                limit: w.limit,
                resets_at_ms: w.resets_at_ms,
                sub_items: w
                    .sub_items
                    .into_iter()
                    .map(|s| UsageSubItemDto {
                        label: s.label,
                        used: s.used,
                    })
                    .collect(),
            })
            .collect(),
        model_usage: u.model_usage.map(|m| ModelUsageDto {
            total_tokens: m.total_tokens,
            total_calls: m.total_calls,
            period_start_ms: m.period_start_ms,
            period_end_ms: m.period_end_ms,
        }),
    }
}
```

Note: Check the existing imports in `admin.rs`. The file already imports `proxy_admin_api`, `Arc`, `ProxyError`, etc. Add only missing imports. The `use` block for `GetAccountUsage` should be placed near the struct definition, not at file top (the file may already have wildcard or selective imports for `proxy_admin_api`).

- [ ] **Step 3: Add stub-based tests for the use case**

Append to the existing `#[cfg(test)] mod tests` block in `admin.rs`:

```rust
use crate::application::ports::AccountUsagePort;
use crate::domain::account_usage::{
    AccountUsageStatus, ModelUsageSnapshot, ProviderAccountUsage, UsageWindow,
};

struct StubAccountUsage {
    result: Option<Result<ProviderAccountUsage, ProxyError>>,
}

impl AccountUsagePort for StubAccountUsage {
    fn fetch_usage(&self) -> Option<Result<ProviderAccountUsage, ProxyError>> {
        self.result.clone()
    }
}

#[test]
fn get_account_usage_returns_not_supported_for_none() {
    let mut map = HashMap::new();
    map.insert(
        "anthropic".to_string(),
        Arc::new(StubAccountUsage { result: None }) as Arc<dyn AccountUsagePort>,
    );
    let uc = GetAccountUsage::new(map);
    let resp = uc.execute();
    assert_eq!(resp.providers.len(), 1);
    assert_eq!(resp.providers[0].status, ProviderUsageStatus::NotSupported);
}

#[test]
fn get_account_usage_returns_available_on_success() {
    let usage = ProviderAccountUsage {
        provider: "zai".to_string(),
        status: AccountUsageStatus::Available,
        plan: Some("pro".to_string()),
        windows: vec![UsageWindow {
            label: "5h Token".to_string(),
            used_pct: 40.0,
            used: Some(16_000_000),
            limit: Some(40_000_000),
            resets_at_ms: Some(1_746_300_000_000),
            sub_items: vec![],
        }],
        model_usage: None,
    };
    let mut map = HashMap::new();
    map.insert(
        "zai".to_string(),
        Arc::new(StubAccountUsage {
            result: Some(Ok(usage)),
        }) as Arc<dyn AccountUsagePort>,
    );
    let uc = GetAccountUsage::new(map);
    let resp = uc.execute();
    assert_eq!(resp.providers.len(), 1);
    assert_eq!(resp.providers[0].status, ProviderUsageStatus::Available);
    assert_eq!(resp.providers[0].plan.as_deref(), Some("pro"));
    assert_eq!(resp.providers[0].windows.len(), 1);
}

#[test]
fn get_account_usage_returns_error_on_failure() {
    let mut map = HashMap::new();
    map.insert(
        "bad".to_string(),
        Arc::new(StubAccountUsage {
            result: Some(Err(ProxyError::UpstreamUsage {
                provider: "bad".to_string(),
                message: "timeout".to_string(),
            })),
        }) as Arc<dyn AccountUsagePort>,
    );
    let uc = GetAccountUsage::new(map);
    let resp = uc.execute();
    assert_eq!(resp.providers.len(), 1);
    assert_eq!(resp.providers[0].status, ProviderUsageStatus::Error);
}

#[test]
fn get_account_usage_sorts_providers_by_name() {
    let mut map = HashMap::new();
    map.insert(
        "zai".to_string(),
        Arc::new(StubAccountUsage { result: None }) as Arc<dyn AccountUsagePort>,
    );
    map.insert(
        "anthropic".to_string(),
        Arc::new(StubAccountUsage { result: None }) as Arc<dyn AccountUsagePort>,
    );
    let uc = GetAccountUsage::new(map);
    let resp = uc.execute();
    assert_eq!(resp.providers[0].provider, "anthropic");
    assert_eq!(resp.providers[1].provider, "zai");
}

#[test]
fn get_account_usage_empty_map_returns_empty() {
    let uc = GetAccountUsage::new(HashMap::new());
    let resp = uc.execute();
    assert!(resp.providers.is_empty());
}
```

Note: `HashMap` and `Arc` should already be imported in the test module. If not, add the imports. `ProxyError` and `ProviderUsageStatus` are already used in existing tests in the same module.

- [ ] **Step 4: Run the tests**

Run:
```bash
cargo test -p proxy --lib application::use_cases::admin::tests
```
Expected: PASS (existing + 5 new tests).

- [ ] **Step 5: Commit**

```bash
git add crates/proxy/src/adapters/providers/builder.rs \
        crates/proxy/src/application/use_cases/admin.rs
git commit -m "feat(proxy): add build_account_usage + GetAccountUsage use case"
```

---

## Task 7: Wire endpoint into admin router

**Files:**
- Modify: `crates/proxy/src/frameworks/admin.rs`
- Modify: `crates/proxy/src/main.rs`

- [ ] **Step 1: Add `account_usage` field to `AdminState`**

In `crates/proxy/src/frameworks/admin.rs`, add to `AdminState`:

```rust
pub account_usage: Arc<GetAccountUsage>,
```

Add the import for `GetAccountUsage` to the existing use-case imports at the top of the file.

- [ ] **Step 2: Add the route and handler**

In `build_admin_router`, add before `.with_state(state)`:

```rust
.route("/admin/account/usage", get(account_usage_handler))
```

Add the handler function:

```rust
async fn account_usage_handler(
    State(uc): State<Arc<GetAccountUsage>>,
) -> Result<Json<proxy_admin_api::AccountUsageResponse>, ProxyError> {
    Ok(Json(uc.execute()))
}
```

Add `FromRef` impl for substate extraction (match the existing pattern for `GetUsageSummary`):

```rust
impl FromRef<AdminState> for Arc<GetAccountUsage> {
    fn from_ref(s: &AdminState) -> Self {
        s.account_usage.clone()
    }
}
```

Add `FromRef` to the axum imports if not already there:

```rust
use axum::extract::{FromRef, Query, State};
```

- [ ] **Step 3: Construct in `main.rs`**

In `crates/proxy/src/main.rs`, after the `let quota_status = ...` line, add:

```rust
let account_usage_map =
    proxy::adapters::providers::builder::build_account_usage(&cfg.providers);
let account_usage = Arc::new(
    proxy::application::use_cases::admin::GetAccountUsage::new(account_usage_map),
);
```

Add `account_usage,` to the `AdminState { ... }` struct literal.

- [ ] **Step 4: Verify it builds**

Run:
```bash
cargo build -p proxy
```
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/proxy/src/frameworks/admin.rs crates/proxy/src/main.rs
git commit -m "feat(proxy): expose GET /admin/account/usage endpoint"
```

---

## Task 8: Integration test

**Files:**
- Create: `crates/proxy/tests/account_usage_api.rs`

- [ ] **Step 1: Write the integration test**

Create `crates/proxy/tests/account_usage_api.rs`:

```rust
//! Integration test: GET /admin/account/usage with stub adapters.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use proxy::application::errors::ProxyError;
use proxy::application::ports::AccountUsagePort;
use proxy::application::use_cases::admin::GetAccountUsage;
use proxy::domain::account_usage::{
    AccountUsageStatus, ModelUsageSnapshot, ProviderAccountUsage, UsageWindow,
};
use proxy_admin_api::AccountUsageResponse;
use tower::ServiceExt;

// Stub adapter for testing.
struct StubUsage {
    result: Option<Result<ProviderAccountUsage, ProxyError>>,
}

impl AccountUsagePort for StubUsage {
    fn fetch_usage(&self) -> Option<Result<ProviderAccountUsage, ProxyError>> {
        self.result.clone()
    }
}

async fn handler(
    State(uc): State<Arc<GetAccountUsage>>,
) -> Json<AccountUsageResponse> {
    Json(uc.execute())
}

#[tokio::test]
async fn account_usage_endpoint_returns_merged_providers() {
    let usage = ProviderAccountUsage {
        provider: "zai".to_string(),
        status: AccountUsageStatus::Available,
        plan: Some("pro".to_string()),
        windows: vec![UsageWindow {
            label: "5h Token".to_string(),
            used_pct: 40.0,
            used: Some(16_000_000),
            limit: Some(40_000_000),
            resets_at_ms: Some(1_746_300_000_000),
            sub_items: vec![],
        }],
        model_usage: Some(ModelUsageSnapshot {
            total_tokens: 12_500_000,
            total_calls: 1_234,
            period_start_ms: 1_746_220_800_000,
            period_end_ms: 1_746_292_800_000,
        }),
    };

    let mut map = HashMap::new();
    map.insert(
        "zai".to_string(),
        Arc::new(StubUsage {
            result: Some(Ok(usage)),
        }) as Arc<dyn AccountUsagePort>,
    );
    map.insert(
        "anthropic".to_string(),
        Arc::new(StubUsage { result: None }) as Arc<dyn AccountUsagePort>,
    );

    let uc = Arc::new(GetAccountUsage::new(map));
    let app = Router::new()
        .route("/admin/account/usage", get(handler))
        .with_state(uc);

    let req = Request::builder()
        .uri("/admin/account/usage")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let parsed: AccountUsageResponse = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(parsed.providers.len(), 2);
    // Sorted alphabetically: anthropic first, zai second.
    assert_eq!(parsed.providers[0].provider, "anthropic");
    assert_eq!(
        parsed.providers[0].status,
        proxy_admin_api::ProviderUsageStatus::NotSupported
    );
    assert_eq!(parsed.providers[1].provider, "zai");
    assert_eq!(
        parsed.providers[1].status,
        proxy_admin_api::ProviderUsageStatus::Available
    );
    assert_eq!(parsed.providers[1].plan.as_deref(), Some("pro"));
    assert_eq!(parsed.providers[1].windows.len(), 1);
    assert!(parsed.providers[1].model_usage.is_some());
}

#[tokio::test]
async fn account_usage_endpoint_returns_error_provider() {
    let mut map = HashMap::new();
    map.insert(
        "broken".to_string(),
        Arc::new(StubUsage {
            result: Some(Err(ProxyError::UpstreamUsage {
                provider: "broken".to_string(),
                message: "timeout".to_string(),
            })),
        }) as Arc<dyn AccountUsagePort>,
    );

    let uc = Arc::new(GetAccountUsage::new(map));
    let app = Router::new()
        .route("/admin/account/usage", get(handler))
        .with_state(uc);

    let req = Request::builder()
        .uri("/admin/account/usage")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let parsed: AccountUsageResponse = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(parsed.providers.len(), 1);
    assert_eq!(
        parsed.providers[0].status,
        proxy_admin_api::ProviderUsageStatus::Error
    );
}
```

Note: If `tower` isn't in proxy's dev-deps, add it. It should already be there per Task 7 of the usage-dashboard plan.

- [ ] **Step 2: Run the integration tests**

Run:
```bash
cargo test -p proxy --test account_usage_api
```
Expected: PASS (2 tests).

- [ ] **Step 3: Run full proxy suite + clippy**

Run:
```bash
cargo test -p proxy
cargo clippy -p proxy -- -D warnings
```
Expected: green.

- [ ] **Step 4: Commit**

```bash
git add crates/proxy/tests/account_usage_api.rs
git commit -m "test(proxy): integration test for GET /admin/account/usage"
```

---

## Task 9: proxy-tui — View::Account + state

**Files:**
- Modify: `crates/proxy-tui/src/app.rs`

- [ ] **Step 1: Add `View::Account` to the enum and `ALL_VIEWS`**

In `crates/proxy-tui/src/app.rs`:

Find the `View` enum and add `Account` as the last variant:

```rust
pub enum View {
    Status,
    Providers,
    Routing,
    Requests,
    Usage,
    Account,
}
```

Find the `label` method and add:

```rust
View::Account => "Account",
```

Find `ALL_VIEWS` and append `View::Account`:

```rust
pub const ALL_VIEWS: &[View] = &[
    View::Status,
    View::Providers,
    View::Routing,
    View::Requests,
    View::Usage,
    View::Account,
];
```

- [ ] **Step 2: Add `AccountPaneState`**

Add to `crates/proxy-tui/src/app.rs`:

```rust
use proxy_admin_api::AccountUsageResponse;

#[derive(Debug, Clone, Default)]
pub struct AccountPaneState {
    pub usage: Option<AccountUsageResponse>,
    pub last_error: Option<String>,
    pub loading: bool,
    pub scroll_offset: usize,
}
```

Add the import for `AccountUsageResponse` to the existing `proxy_admin_api` import line.

- [ ] **Step 3: Add `account` field to `AppState`**

Find the `AppState` struct and add:

```rust
pub account: AccountPaneState,
```

Add `account: AccountPaneState::default()` in `AppState::new()` (or `Default::default()` impl).

- [ ] **Step 4: Verify proxy-tui builds**

Run:
```bash
cargo build -p proxy-tui
```
Expected: clean (new field is unused so far).

- [ ] **Step 5: Commit**

```bash
git add crates/proxy-tui/src/app.rs
git commit -m "feat(proxy-tui): add View::Account and AccountPaneState"
```

---

## Task 10: proxy-tui — client method

**Files:**
- Modify: `crates/proxy-tui/src/client.rs`

- [ ] **Step 1: Add `get_account_usage` method**

Find the `impl AdminClient` block and add:

```rust
pub fn get_account_usage(&self) -> Result<AccountUsageResponse, ClientError> {
    get_json(&format!("{}/admin/account/usage", self.base_url))
}
```

Add `AccountUsageResponse` to the existing `proxy_admin_api` import line at the top of the file.

- [ ] **Step 2: Add wiremock test**

Append to `crates/proxy-tui/src/client.rs`:

```rust
#[cfg(test)]
mod account_usage_client_tests {
    use super::*;
    use proxy_admin_api::{
        AccountUsageResponse, ProviderAccountUsageDto, ProviderUsageStatus,
    };
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn get_account_usage_calls_endpoint_and_decodes_response() {
        let server = MockServer::start().await;
        let payload = AccountUsageResponse {
            providers: vec![ProviderAccountUsageDto {
                provider: "zai".into(),
                status: ProviderUsageStatus::Available,
                plan: Some("pro".into()),
                windows: vec![],
                model_usage: None,
            }],
        };

        Mock::given(method("GET"))
            .and(path("/admin/account/usage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&payload))
            .mount(&server)
            .await;

        let client = AdminClient::new(server.uri());
        let got = client.get_account_usage().unwrap();
        assert_eq!(got, payload);
    }
}
```

Note: `wiremock` and `tokio` should already be in `proxy-tui`'s dev-deps (added for the usage-dashboard feature).

- [ ] **Step 3: Run the test**

Run:
```bash
cargo test -p proxy-tui get_account_usage_calls_endpoint_and_decodes_response
```
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/proxy-tui/src/client.rs
git commit -m "feat(proxy-tui): add get_account_usage client method"
```

---

## Task 11: proxy-tui — Account tab renderer

**Files:**
- Modify: `crates/proxy-tui/src/views/mod.rs`
- Create: `crates/proxy-tui/src/views/account.rs`

- [ ] **Step 1: Declare `account` module**

In `crates/proxy-tui/src/views/mod.rs`, add:

```rust
pub mod account;
```

- [ ] **Step 2: Create `views/account.rs`**

Create `crates/proxy-tui/src/views/account.rs`:

```rust
//! Renderer for the Account tab — upstream provider quota and usage.

use crate::app::AccountPaneState;
use proxy_admin_api::{
    ProviderAccountUsageDto, ProviderUsageStatus, UsageWindowDto,
};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use shared::adapters::presenters::formatting::fmt_num_compact;

pub fn draw(f: &mut Frame<'_>, area: Rect, state: &AccountPaneState) {
    if let Some(err) = &state.last_error {
        let msg = Paragraph::new(format!("Error: {err}\n(showing last good data if any)"))
            .wrap(Wrap { trim: true });
        f.render_widget(msg, area);
        return;
    }

    let Some(usage) = &state.usage else {
        let msg = if state.loading {
            "Loading..."
        } else {
            "No data yet — press [r] to fetch."
        };
        let p = Paragraph::new(msg);
        f.render_widget(p, area);
        return;
    };

    if usage.providers.is_empty() {
        let p = Paragraph::new("No providers configured.");
        f.render_widget(p, area);
        return;
    }

    // Render all provider blocks into one paragraph, skipping entries
    // above the scroll offset.
    let mut lines: Vec<ratatui::text::Line> = Vec::new();
    let mut visible_idx = 0;

    for provider in &usage.providers {
        if visible_idx < state.scroll_offset {
            visible_idx += 1;
            continue;
        }
        render_provider(&mut lines, provider);
        lines.push(ratatui::text::Line::raw(""));
        visible_idx += 1;
    }

    if lines.is_empty() {
        let p = Paragraph::new("Scroll up to see providers.");
        f.render_widget(p, area);
        return;
    }

    // Remove trailing empty line.
    if lines.last().map(|l| l.to_string().is_empty()).unwrap_or(false) {
        lines.pop();
    }

    let p = Paragraph::new(lines).wrap(Wrap { trim: false });
    f.render_widget(p, area);
}

fn render_provider(lines: &mut Vec<ratatui::text::Line>, p: &ProviderAccountUsageDto) {
    // Title line: provider name + plan tier.
    let title = match (&p.plan, p.status) {
        (Some(plan), ProviderUsageStatus::Available) => {
            format!("╭─ {} ({}) ─", p.provider, capitalize(plan))
        }
        (_, ProviderUsageStatus::NotSupported) => {
            format!("╭─ {} ─", p.provider)
        }
        (_, ProviderUsageStatus::Error) => {
            format!("╭─ {} (error) ─", p.provider)
        }
        (None, ProviderUsageStatus::Available) => {
            format!("╭─ {} ─", p.provider)
        }
    };
    lines.push(ratatui::text::Line::styled(
        title,
        Style::default().add_modifier(Modifier::BOLD),
    ));

    match p.status {
        ProviderUsageStatus::NotSupported => {
            lines.push(ratatui::text::Line::raw(
                "│  No account usage API available for this provider.",
            ));
            lines.push(ratatui::text::Line::raw("╰─"));
            return;
        }
        ProviderUsageStatus::Error => {
            lines.push(ratatui::text::Line::raw(
                "│  Failed to fetch usage data from this provider.",
            ));
            lines.push(ratatui::text::Line::raw("╰─"));
            return;
        }
        ProviderUsageStatus::Available => {}
    }

    // Quota windows.
    for window in &p.windows {
        render_window(lines, window);
    }

    // Model usage.
    if let Some(m) = &p.model_usage {
        lines.push(ratatui::text::Line::raw(format!(
            "│  Model usage (24h):  Tokens: {}   Calls: {}",
            fmt_num_compact(m.total_tokens),
            fmt_num_compact(m.total_calls),
        )));
    }

    lines.push(ratatui::text::Line::raw("╰─"));
}

fn render_window(lines: &mut Vec<ratatui::text::Line>, w: &UsageWindowDto) {
    let bar = progress_bar(w.used_pct);
    let color = if w.used_pct >= 80.0 {
        Color::Red
    } else if w.used_pct >= 50.0 {
        Color::Yellow
    } else {
        Color::Green
    };

    let reset = match w.resets_at_ms {
        Some(ms) => {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64;
            let diff_secs = ((ms - now_ms) / 1000).max(0);
            format!("resets in {}", fmt_duration_secs(diff_secs as u64))
        }
        None => String::new(),
    };

    // Main window line: label + bar + percentage + reset.
    let main_line = format!(
        "│  {:16} {}  {:5.1}%   {}",
        w.label,
        bar,
        w.used_pct,
        reset,
    );
    lines.push(ratatui::text::Line::from(vec![
        ratatui::text::Span::raw(format!(
            "│  {:16} ",
            w.label,
        )),
        ratatui::text::Span::styled(bar, Style::default().fg(color)),
        ratatui::text::Span::raw(format!("  {:5.1}%   {}", w.used_pct, reset)),
    ]));

    // Detail line: used / limit.
    if let (Some(used), Some(limit)) = (w.used, w.limit) {
        lines.push(ratatui::text::Line::raw(format!(
            "│  {:16} {} / {}",
            "",
            fmt_num_compact(used),
            fmt_num_compact(limit),
        )));
    }

    // Sub-items (e.g. MCP tool breakdown).
    if !w.sub_items.is_empty() {
        let sub_line: String = w
            .sub_items
            .iter()
            .map(|s| format!("{}: {}", s.label, fmt_num_compact(s.used)))
            .collect::<Vec<_>>()
            .join("  ");
        lines.push(ratatui::text::Line::raw(format!(
            "│  {:16} {}",
            "", sub_line,
        )));
    }
}

fn progress_bar(pct: f64) -> String {
    let filled = (pct.min(100.0) / 5.0).round() as usize; // 20-char bar
    let empty = 20 - filled.min(20);
    "\u{2588}".repeat(filled.min(20)) + &"\u{2591}".repeat(empty)
}

fn fmt_duration_secs(secs: u64) -> String {
    if secs >= 24 * 3600 {
        format!("{}d {}h", secs / 86400, (secs % 86400) / 3600)
    } else if secs >= 3600 {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    } else {
        format!("{}m {}s", secs / 60, secs % 60)
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}
```

- [ ] **Step 3: Verify proxy-tui builds**

Run:
```bash
cargo build -p proxy-tui
```
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/proxy-tui/src/views/mod.rs crates/proxy-tui/src/views/account.rs
git commit -m "feat(proxy-tui): add Account tab renderer with progress bars"
```

---

## Task 12: proxy-tui — Wire into event loop and renderer

**Files:**
- Modify: `crates/proxy-tui/src/main.rs`
- Modify: `crates/proxy-tui/src/ui.rs`

- [ ] **Step 1: Add Account key to `handle_key` in `main.rs`**

Find the `handle_key` function. In the "Always-available keys" section, after the `KeyCode::Char('5')` arm that switches to Usage, add:

```rust
KeyCode::Char('6') => {
    state.set_view(View::Account);
    if state.account.usage.is_none() && state.account.last_error.is_none() {
        fetch_account(client, state);
    }
    return;
}
```

In the "Usage-tab-specific keys" block, the `if state.view == View::Usage` section stays the same.

After the Usage-tab block (but before the final `match k.code` for all other tabs), add an Account-tab-specific block:

```rust
// Account-tab-specific keys.
if state.view == View::Account {
    match k.code {
        KeyCode::Char('r') => fetch_account(client, state),
        KeyCode::Up => {
            state.account.scroll_offset = state.account.scroll_offset.saturating_sub(1);
        }
        KeyCode::Down => {
            if let Some(u) = &state.account.usage
                && state.account.scroll_offset + 1 < u.providers.len()
            {
                state.account.scroll_offset += 1;
            }
        }
        _ => {}
    }
    return;
}
```

Also update the tab-switching `match k.code` at the bottom of `handle_key`:
- Change `KeyCode::Char('5')` comment to include Account: the existing `5` already switches to Usage.
- The number keys 1–5 map to existing tabs. `6` now maps to Account (added above in the always-available section).

- [ ] **Step 2: Add `fetch_account` function**

Add near the existing `fetch_usage` function:

```rust
fn fetch_account(client: &AdminClient, state: &mut AppState) {
    state.account.loading = true;
    match client.get_account_usage() {
        Ok(resp) => {
            state.account.usage = Some(resp);
            state.account.last_error = None;
            state.account.scroll_offset = 0;
        }
        Err(e) => {
            state.account.last_error = Some(format!("{e}"));
        }
    }
    state.account.loading = false;
}
```

- [ ] **Step 3: Add `View::Account` dispatch to `ui.rs`**

Find the `match state.view` block in `crates/proxy-tui/src/ui.rs` (or equivalent rendering dispatch). Add:

```rust
View::Account => crate::views::account::draw(f, content_area, &state.account),
```

Match the existing pattern — the other views dispatch similarly (e.g. `View::Usage => crate::views::usage::draw(...)`).

- [ ] **Step 4: Add Account to `refresh_view`**

In `crates/proxy-tui/src/main.rs`, find `fn refresh_view`. Add a case:

```rust
View::Account => {} // refreshes on demand via fetch_account
```

This matches the Usage tab pattern — no auto-refresh polling.

- [ ] **Step 5: Build and verify**

Run:
```bash
cargo build --workspace
cargo clippy --workspace -- -D warnings
```
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/proxy-tui/src/main.rs crates/proxy-tui/src/ui.rs
git commit -m "feat(proxy-tui): wire Account tab into event loop and renderer"
```

---

## Task 13: Workspace gates

- [ ] **Step 1: Format**

Run:
```bash
cargo fmt --all
```

- [ ] **Step 2: Check format**

Run:
```bash
cargo fmt --all -- --check
```
Expected: no output (clean).

- [ ] **Step 3: Clippy**

Run:
```bash
cargo clippy --workspace --tests -- -D warnings
```
Expected: clean.

- [ ] **Step 4: Full test suite**

Run:
```bash
cargo test --workspace
```
Expected: all tests pass.

- [ ] **Step 5: Final commit if any fmt/clippy churn**

```bash
git add -A
git commit -m "chore: cargo fmt/clippy cleanup"
```
(Only if there are changes — skip if clean.)

---

## Task 14: Manual smoke test

- [ ] **Step 1: Start proxy**

```bash
cargo run -p proxy
```

- [ ] **Step 2: Hit the new endpoint**

```bash
curl -s http://127.0.0.1:8787/admin/account/usage | jq
```

Expected: JSON with one entry per configured provider. Z.ai providers show quota windows. Anthropic providers show `not_supported`.

- [ ] **Step 3: Start TUI**

```bash
cargo run -p proxy-tui
```

Press `6` to switch to Account tab. Verify:
- Z.ai provider shows quota bars with percentages and reset timers.
- Anthropic provider shows "No account usage API available."
- Press `r` to refresh.
- Press `↑` / `↓` to scroll if many providers.
