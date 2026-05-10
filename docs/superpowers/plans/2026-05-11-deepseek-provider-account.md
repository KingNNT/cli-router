# DeepSeek Provider + Account Usage Display — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add DeepSeek as a fully supported upstream provider with account usage display (balance + per-model breakdown) on the proxy-tui Account tab.

**Architecture:** DeepSeek is OpenAI-compatible so the provider adapter mirrors ZaiProvider. Account usage fetches balance from DeepSeek's `/user/balance` API, and per-model breakdown is queried from the proxy's own SQLite request log. Both sources merge at the admin use case layer.

**Tech Stack:** Rust, axum (server), ratatui (TUI), ureq (HTTP client), serde (serialization), rusqlite (request log)

---

### Task 1: Add ModelBreakdownItem DTO (proxy-admin-api)

**Files:**
- Modify: `crates/proxy-admin-api/src/lib.rs`

- [ ] **Step 1: Add `ModelBreakdownItemDto` struct and field to `ModelUsageDto`**

Add after the `ModelUsageDto` definition (after line ~386):

```rust
/// Per-model usage breakdown item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelBreakdownItemDto {
    pub model: String,
    pub tokens: u64,
    pub calls: u64,
}
```

Then modify `ModelUsageDto` to add the new field (after line ~385):

```rust
pub struct ModelUsageDto {
    pub total_tokens: u64,
    pub total_calls: u64,
    pub period_start_ms: i64,
    pub period_end_ms: i64,
    #[serde(default)]
    pub model_breakdown: Vec<ModelBreakdownItemDto>,
}
```

- [ ] **Step 2: Add test for round-trip serialization**

Add inside the `mod account_usage_tests` block (after the existing `provider_usage_status_serializes_as_snake_case` test):

```rust
#[test]
fn model_breakdown_dto_round_trips_through_json() {
    let original = ModelUsageDto {
        total_tokens: 1_200_000,
        total_calls: 500,
        period_start_ms: 1_746_220_800_000,
        period_end_ms: 1_746_292_800_000,
        model_breakdown: vec![
            ModelBreakdownItemDto {
                model: "deepseek-chat".into(),
                tokens: 800_000,
                calls: 300,
            },
            ModelBreakdownItemDto {
                model: "deepseek-reasoner".into(),
                tokens: 400_000,
                calls: 200,
            },
        ],
    };
    let json = serde_json::to_string(&original).unwrap();
    let decoded: ModelUsageDto = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, original);
    assert_eq!(decoded.model_breakdown.len(), 2);
}

#[test]
fn model_usage_dto_defaults_empty_breakdown() {
    let json = r#"{"total_tokens":100,"total_calls":1,"period_start_ms":0,"period_end_ms":0}"#;
    let decoded: ModelUsageDto = serde_json::from_str(json).unwrap();
    assert!(decoded.model_breakdown.is_empty());
}
```

- [ ] **Step 3: Run tests and verify**

Run: `cargo test -p proxy-admin-api -- account_usage`
Expected: All tests PASS

- [ ] **Step 4: Commit**

```bash
git add crates/proxy-admin-api/src/lib.rs
git commit -m "feat(api): add ModelBreakdownItemDto with per-model breakdown to ModelUsageDto"
```

---

### Task 2: Add ModelBreakdownItem domain type (proxy)

**Files:**
- Modify: `crates/proxy/src/domain/account_usage.rs`

- [ ] **Step 1: Add `ModelBreakdownItem` and field to `ModelUsageSnapshot`**

Open `crates/proxy/src/domain/account_usage.rs`. Add after the `ModelUsageSnapshot` struct (after line ~55):

```rust
/// Per-model usage breakdown within a snapshot period.
#[derive(Debug, Clone)]
pub struct ModelBreakdownItem {
    pub model: String,
    pub tokens: u64,
    pub calls: u64,
}
```

Then modify `ModelUsageSnapshot` to include the breakdown field:

```rust
#[derive(Debug, Clone)]
pub struct ModelUsageSnapshot {
    pub total_tokens: u64,
    pub total_calls: u64,
    pub period_start_ms: i64,
    pub period_end_ms: i64,
    pub model_breakdown: Vec<ModelBreakdownItem>,
}
```

- [ ] **Step 2: Compile to verify**

Run: `cargo check -p proxy 2>&1 | head -30`
Expected: Compiles successfully (existing consumers of `ModelUsageSnapshot` may need updates, that's expected)

- [ ] **Step 3: Commit**

```bash
git add crates/proxy/src/domain/account_usage.rs
git commit -m "feat(domain): add ModelBreakdownItem for per-model usage breakdown"
```

---

### Task 3: Add ProviderKind::DeepSeek to config (proxy)

**Files:**
- Modify: `crates/proxy/src/config.rs`

- [ ] **Step 1: Add `DeepSeek` variant to `ProviderKind` enum**

Change line ~55:
```rust
pub enum ProviderKind {
    Anthropic,
    Zai,
    DeepSeek,
}
```

- [ ] **Step 2: Wire `parse_kind` to accept `"deepseek"`**

Change the function at line ~357. Add a new arm:
```rust
fn parse_kind(s: &str) -> Option<ProviderKind> {
    match s.trim().to_ascii_lowercase().as_str() {
        "anthropic" => Some(ProviderKind::Anthropic),
        "zai" | "z.ai" | "z-ai" => Some(ProviderKind::Zai),
        "deepseek" | "deep-seek" => Some(ProviderKind::DeepSeek),
        _ => None,
    }
}
```

- [ ] **Step 3: Add test for parse_kind**

Inside the existing `mod tests` block in `config.rs`, add:

```rust
#[test]
fn parse_kind_accepts_deepseek_aliases() {
    assert_eq!(parse_kind("deepseek"), Some(ProviderKind::DeepSeek));
    assert_eq!(parse_kind("deep-seek"), Some(ProviderKind::DeepSeek));
    assert_eq!(parse_kind("DEEPSEEK"), Some(ProviderKind::DeepSeek));
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p proxy -- config::tests`
Expected: All tests PASS

- [ ] **Step 5: Commit**

```bash
git add crates/proxy/src/config.rs
git commit -m "feat(config): add ProviderKind::DeepSeek with parse_kind support"
```

---

### Task 4: Create DeepSeek provider adapter (proxy)

**Files:**
- Create: `crates/proxy/src/adapters/providers/deepseek.rs`
- Modify: `crates/proxy/src/adapters/providers/mod.rs`

- [ ] **Step 1: Create `deepseek.rs`**

Create `crates/proxy/src/adapters/providers/deepseek.rs`:

```rust
//! DeepSeek provider — implements `Provider` against DeepSeek's
//! OpenAI-compatible endpoint at `https://api.deepseek.com/v1`.
//!
//! DeepSeek's chat completions API is fully OpenAI-compatible, so we
//! forward requests directly without format conversion.

use super::messages_protocol::{self, AuthHeader};
use crate::application::errors::ProxyError;
use crate::application::ports::{ApiFormat, Provider, UpstreamResponse, UsageParser};
use crate::domain::UsageRecord;
use async_trait::async_trait;
use axum::http::HeaderMap;
use bytes::Bytes;

const DEFAULT_BASE_URL: &str = "https://api.deepseek.com/v1";

pub struct DeepSeekProvider {
    base_url: String,
    http: reqwest::Client,
    auth: AuthHeader,
}

impl DeepSeekProvider {
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

#[async_trait]
impl Provider for DeepSeekProvider {
    fn name(&self) -> &'static str {
        "deepseek"
    }

    fn supported_formats(&self) -> &[ApiFormat] {
        &[ApiFormat::OpenAi]
    }

    async fn forward(
        &self,
        format: ApiFormat,
        method: axum::http::Method,
        path: &str,
        headers: HeaderMap,
        body: Bytes,
    ) -> Result<UpstreamResponse, ProxyError> {
        let auth_value = match &self.auth {
            AuthHeader::Passthrough => {
                headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .map(|v| v.to_string())
            }
            AuthHeader::ApiKey(key) | AuthHeader::Bearer(key) => {
                Some(format!("Bearer {key}"))
            }
            AuthHeader::OAuth { access_token, .. } => {
                Some(format!("Bearer {access_token}"))
            }
        };

        let upstream_path = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{}", path)
        };

        let mut req = self
            .http
            .request(method.clone(), format!("{}{}", self.base_url, upstream_path))
            .body(body)
            .map_err(|e| ProxyError::UpstreamForward(format!("build request: {e}")))?;

        if let Some(auth) = auth_value {
            req.headers_mut()
                .insert("authorization", auth.parse().unwrap());
        }

        // Copy content-type if present.
        if let Some(ct) = headers.get("content-type") {
            req.headers_mut().insert("content-type", ct.clone());
        }

        let resp = req
            .send()
            .await
            .map_err(|e| ProxyError::UpstreamForward(format!("send: {e}")))?;

        let status = resp.status();
        let resp_headers = resp.headers().clone();
        let body = resp
            .bytes()
            .await
            .map_err(|e| ProxyError::UpstreamForward(format!("read body: {e}")))?;

        Ok(UpstreamResponse {
            status,
            headers: resp_headers,
            body,
        })
    }

    fn resolve_endpoint_for_format(&self, _format: ApiFormat) -> String {
        self.base_url.clone()
    }

    fn usage_parser_for_format(&self, _format: ApiFormat) -> Option<Box<dyn UsageParser>> {
        Some(Box::new(messages_protocol::OpenAiUsageParser))
    }

    fn should_rewrite_model(&self) -> bool {
        false
    }

    fn extract_model_from_response(
        &self,
        _body: &Bytes,
    ) -> Result<Option<String>, ProxyError> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_returns_deepseek() {
        let p = DeepSeekProvider::new(reqwest::Client::new());
        assert_eq!(p.name(), "deepseek");
    }

    #[test]
    fn supported_formats_is_openai_only() {
        let p = DeepSeekProvider::new(reqwest::Client::new());
        assert_eq!(p.supported_formats(), &[ApiFormat::OpenAi]);
    }
}
```

- [ ] **Step 2: Register module in `mod.rs`**

Open `crates/proxy/src/adapters/providers/mod.rs`. Add:

After line 3 (after `pub mod affinity;`):
```rust
pub mod deepseek;
```

After line 20 (after `pub use zai::ZaiProvider;`):
```rust
pub use deepseek::DeepSeekProvider;
```

- [ ] **Step 3: Compile and run tests**

Run: `cargo test -p proxy -- deepseek`
Expected: Tests PASS

- [ ] **Step 4: Commit**

```bash
git add crates/proxy/src/adapters/providers/deepseek.rs crates/proxy/src/adapters/providers/mod.rs
git commit -m "feat(proxy): add DeepSeekProvider adapter for OpenAI-compatible forwarding"
```

---

### Task 5: Wire DeepSeek in builder (proxy)

**Files:**
- Modify: `crates/proxy/src/adapters/providers/builder.rs`

- [ ] **Step 1: Wire `build_leaf` for DeepSeek**

Add import at the top (line ~6):
```rust
use super::DeepSeekProvider;
```

In `build_leaf` function, add a new arm after the `ProviderKind::Zai` arm (after line ~46):

```rust
ProviderKind::DeepSeek => {
    Arc::new(DeepSeekProvider::configure(http, p.base_url.clone(), auth))
}
```

- [ ] **Step 2: Wire `build_account_usage` for DeepSeek**

In `build_account_usage`, add a new arm in the `match p.kind` block (after the `ProviderKind::Anthropic` arm at line ~148):

```rust
ProviderKind::DeepSeek => {
    let token = resolve_auth_token(&p.auth);
    Arc::new(DeepSeekAccountUsage::new(p.name.clone(), token))
}
```

Add the import at line ~5:
```rust
use super::account_usage::DeepSeekAccountUsage;
```

- [ ] **Step 3: Compile to verify**

Run: `cargo check -p proxy 2>&1 | head -40`
Note: This will fail because `DeepSeekAccountUsage` doesn't exist yet. That's Task 6.

Expected: Error about missing `DeepSeekAccountUsage` — expected.

- [ ] **Step 4: Commit**

```bash
git add crates/proxy/src/adapters/providers/builder.rs
git commit -m "feat(builder): wire DeepSeek provider in build_leaf and build_account_usage"
```

---

### Task 6: Create DeepSeek account usage adapter (proxy)

**Files:**
- Create: `crates/proxy/src/adapters/providers/account_usage/deepseek.rs`
- Modify: `crates/proxy/src/adapters/providers/account_usage/mod.rs`

- [ ] **Step 1: Create `deepseek.rs`**

Create `crates/proxy/src/adapters/providers/account_usage/deepseek.rs`:

```rust
//! DeepSeek account usage adapter.
//!
//! Queries DeepSeek's `/user/balance` endpoint to show account balance
//! as a usage window. DeepSeek does not expose a public token-usage API.

use std::time::Duration;

use serde::Deserialize;

use crate::application::errors::ProxyError;
use crate::application::ports::AccountUsagePort;
use crate::domain::account_usage::{
    AccountUsageStatus, ProviderAccountUsage, UsageWindow,
};

/// DeepSeek account usage adapter. Queries balance API.
pub struct DeepSeekAccountUsage {
    provider_name: String,
    auth_token: String,
    agent: ureq::Agent,
}

impl DeepSeekAccountUsage {
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

impl AccountUsagePort for DeepSeekAccountUsage {
    fn fetch_usage(&self) -> Option<Result<ProviderAccountUsage, ProxyError>> {
        let url = "https://api.deepseek.com/user/balance";

        let resp = match self
            .agent
            .get(url)
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

        let mut windows = Vec::new();

        for info in &envelope.balance_infos {
            let total: f64 = info.total_balance.parse().unwrap_or(0.0);
            let topped_up: f64 = info.topped_up_balance.parse().unwrap_or(0.0);
            let used = ((total - topped_up) * 100.0).max(0.0) as u64;
            let limit = (total * 100.0).max(0.0) as u64;
            let used_pct = if limit > 0 {
                (used as f64 / limit as f64) * 100.0
            } else {
                0.0
            };

            windows.push(UsageWindow {
                label: format!("Top-up Balance ({})", info.currency),
                used_pct,
                used: Some(used),
                limit: Some(limit),
                resets_at_ms: None,
                sub_items: vec![],
            });
        }

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
// DeepSeek response JSON shapes
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct BalanceResponse {
    is_available: bool,
    #[serde(default)]
    balance_infos: Vec<BalanceInfo>,
}

#[derive(Debug, Deserialize)]
struct BalanceInfo {
    currency: String,
    total_balance: String,
    #[serde(default)]
    granted_balance: String,
    #[serde(default)]
    topped_up_balance: String,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_balance_response() {
        let json = r#"{
            "is_available": true,
            "balance_infos": [
                {
                    "currency": "CNY",
                    "total_balance": "110.00",
                    "granted_balance": "10.00",
                    "topped_up_balance": "100.00"
                }
            ]
        }"#;
        let resp: BalanceResponse = serde_json::from_str(json).unwrap();
        assert!(resp.is_available);
        assert_eq!(resp.balance_infos.len(), 1);
        assert_eq!(resp.balance_infos[0].currency, "CNY");
        assert_eq!(resp.balance_infos[0].total_balance, "110.00");
        assert_eq!(resp.balance_infos[0].topped_up_balance, "100.00");
    }

    #[test]
    fn deserialize_balance_no_balances() {
        let json = r#"{"is_available": true}"#;
        let resp: BalanceResponse = serde_json::from_str(json).unwrap();
        assert!(resp.is_available);
        assert!(resp.balance_infos.is_empty());
    }

    #[test]
    fn deserialize_balance_zero_values() {
        let json = r#"{
            "is_available": false,
            "balance_infos": [
                {
                    "currency": "USD",
                    "total_balance": "0.00",
                    "granted_balance": "0.00",
                    "topped_up_balance": "0.00"
                }
            ]
        }"#;
        let resp: BalanceResponse = serde_json::from_str(json).unwrap();
        assert!(!resp.is_available);
        assert_eq!(resp.balance_infos[0].total_balance, "0.00");
    }

    #[test]
    fn balance_to_usage_window() {
        let adapter = DeepSeekAccountUsage::new(
            "deepseek".into(),
            "sk-test".into(),
        );
        // We test the mapping logic via a mock by constructing windows manually.
        // Since AccountUsagePort::fetch_usage is async (blocking), we test
        // the struct creation path through the deserialize test above.
        // This test just verifies the adapter builds.
        assert_eq!(adapter.provider_name, "deepseek");
        assert_eq!(adapter.auth_token, "sk-test");
    }
}
```

- [ ] **Step 2: Register module in `account_usage/mod.rs`**

Open `crates/proxy/src/adapters/providers/account_usage/mod.rs`. Add:

```rust
pub mod deepseek;

pub use deepseek::DeepSeekAccountUsage;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p proxy -- account_usage::deepseek`
Expected: All tests PASS

- [ ] **Step 4: Commit**

```bash
git add crates/proxy/src/adapters/providers/account_usage/deepseek.rs crates/proxy/src/adapters/providers/account_usage/mod.rs
git commit -m "feat(proxy): add DeepSeekAccountUsage adapter for balance API"
```

---

### Task 7: Wire kind_to_str for DeepSeek (admin use case)

**Files:**
- Modify: `crates/proxy/src/application/use_cases/admin.rs`

- [ ] **Step 1: Add DeepSeek arm to `kind_to_str`**

Change the function at line ~690:

```rust
fn kind_to_str(k: ProviderKind) -> &'static str {
    match k {
        ProviderKind::Anthropic => "anthropic",
        ProviderKind::Zai => "zai",
        ProviderKind::DeepSeek => "deepseek",
    }
}
```

- [ ] **Step 2: Add DeepSeek arm to `kind_from_str` (if exists)**

Check if there's a `kind_from_str` or reverse mapping. If the admin use case builds providers from payload kind strings, DeepSeek needs handling too. Search:

Run: `cargo check -p proxy 2>&1 | grep -i "DeepSeek\|deepseek\|non-exhaustive"`
Expected: If no errors about missing match arms, we're good.

- [ ] **Step 3: Compile to verify**

Run: `cargo check -p proxy 2>&1 | head -20`
Expected: Compiles successfully.

- [ ] **Step 4: Commit**

```bash
git add crates/proxy/src/application/use_cases/admin.rs
git commit -m "feat(admin): add DeepSeek to kind_to_str mapping"
```

---

### Task 8: Add per-model breakdown query to GetAccountUsage (admin use case)

**Files:**
- Modify: `crates/proxy/src/application/use_cases/admin.rs`
- Modify: `crates/proxy/src/application/ports/request_log_read.rs`

- [ ] **Step 1: Add query method to `RequestLogReadPort` trait**

Add to `crates/proxy/src/application/ports/request_log_read.rs` after `count_translations` (line ~34):

```rust
/// Per-model token+calls breakdown for a provider in the given time range.
fn model_breakdown(
    &self,
    provider: &str,
    from_ms: i64,
    to_ms: i64,
) -> Result<Vec<ModelBreakdownRow>, ProxyError>;
```

Add the row type at the top of the file (after existing imports):

```rust
/// One row of per-model usage breakdown.
#[derive(Debug, Clone)]
pub struct ModelBreakdownRow {
    pub model: String,
    pub tokens: u64,
    pub calls: u64,
}
```

- [ ] **Step 2: Implement the query in the SQLite adapter**

Open `crates/proxy/src/adapters/storage/sqlite_request_log.rs`. Find the `impl RequestLogReadPort for SqliteRequestLogRepository` block (starts around line 101). Add this method before the closing `}` of the impl block:

```rust
    fn model_breakdown(
        &self,
        provider: &str,
        from_ms: i64,
        to_ms: i64,
    ) -> Result<Vec<ModelBreakdownRow>, ProxyError> {
        let conn = self.conn.lock().expect("repo mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT model,
                    COALESCE(SUM(input_tokens), 0) + COALESCE(SUM(output_tokens), 0) AS tokens,
                    COUNT(*) AS calls
             FROM requests
             WHERE provider = ?1
               AND started_at BETWEEN ?2 AND ?3
               AND status = 'completed'
             GROUP BY model
             ORDER BY tokens DESC",
        )?;

        let rows = stmt
            .query_map(params![provider, from_ms, to_ms], |row| {
                Ok(ModelBreakdownRow {
                    model: row.get(0)?,
                    tokens: row.get::<_, i64>(1)? as u64,
                    calls: row.get::<_, i64>(2)? as u64,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(rows)
    }
```

Add the import for `ModelBreakdownRow` at the top of the file (near line ~8, after existing `use` statements):

```rust
use crate::application::ports::ModelBreakdownRow;
```

- [ ] **Step 3: Update `GetAccountUsage` to accept request log and merge breakdowns**

Modify `GetAccountUsage` struct and its `new`/`execute`:

```rust
use crate::domain::account_usage::{ModelBreakdownItem, ModelUsageSnapshot};

pub struct GetAccountUsage {
    adapters: HashMap<String, Arc<dyn AccountUsagePort>>,
    read: Arc<dyn RequestLogReadPort>,
}

impl GetAccountUsage {
    pub fn new(
        adapters: HashMap<String, Arc<dyn AccountUsagePort>>,
        read: Arc<dyn RequestLogReadPort>,
    ) -> Self {
        Self { adapters, read }
    }

    pub fn execute(&self) -> AccountUsageResponse {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let from_ms = now_ms - 24 * 3600 * 1000;

        let mut providers: Vec<ProviderAccountUsageDto> = self
            .adapters
            .iter()
            .map(|(name, adapter)| {
                let mut dto = match adapter.fetch_usage() {
                    None => ProviderAccountUsageDto {
                        provider: name.clone(),
                        status: ProviderUsageStatus::NotSupported,
                        plan: None,
                        windows: vec![],
                        model_usage: None,
                    },
                    Some(Ok(usage)) => account_usage_to_dto(usage),
                    Some(Err(e)) => {
                        tracing::warn!(error = %e, "account usage fetch failed");
                        ProviderAccountUsageDto {
                            provider: name.clone(),
                            status: ProviderUsageStatus::Error,
                            plan: None,
                            windows: vec![],
                            model_usage: None,
                        }
                    }
                };

                // Enrich with per-model breakdown from proxy request log.
                match self.read.model_breakdown(name, from_ms, now_ms) {
                    Ok(rows) if !rows.is_empty() => {
                        let breakdown: Vec<ModelBreakdownItemDto> = rows
                            .into_iter()
                            .map(|r| ModelBreakdownItemDto {
                                model: r.model,
                                tokens: r.tokens,
                                calls: r.calls,
                            })
                            .collect();
                        match &mut dto.model_usage {
                            Some(mu) => {
                                mu.model_breakdown = breakdown;
                            }
                            None => {
                                let total_tokens = breakdown.iter().map(|b| b.tokens).sum();
                                let total_calls = breakdown.iter().map(|b| b.calls).sum();
                                dto.model_usage = Some(ModelUsageDto {
                                    total_tokens,
                                    total_calls,
                                    period_start_ms: from_ms,
                                    period_end_ms: now_ms,
                                    model_breakdown: breakdown,
                                });
                            }
                        }
                    }
                    Ok(_) => {} // empty — no breakdown to add
                    Err(e) => {
                        tracing::debug!(error = %e, provider = %name, "model_breakdown query failed (non-fatal)");
                    }
                }

                dto
            })
            .collect();

        providers.sort_by(|a, b| a.provider.cmp(&b.provider));
        AccountUsageResponse { providers }
    }
}
```

Add the required import at the top of the file (near line ~10):
```rust
use crate::domain::account_usage::ModelUsageSnapshot;
```

And import `ModelBreakdownItemDto` and `ModelUsageDto` from proxy_admin_api:
```rust
use proxy_admin_api::{
    AffinityPayload, AuthPayload, ConfigPayload, MatchPayload, ModelBreakdownItemDto, ModelUsageDto,
    ProviderPayload, QuotaPayload, RecentRequestItem, RecentRequestsResponse, RoutingRulePayload,
    StatusResponse, TestProviderResponse,
};
```

Also add `RequestLogReadPort` to imports if not already present:
```rust
use crate::application::ports::{AccountUsagePort, QuotaPort, RequestLogReadPort, UpstreamResponse};
```

- [ ] **Step 4: Update StubRead test helper in admin.rs**

Find `impl RequestLogReadPort for StubRead` (around line ~850). Add the `model_breakdown` method before the closing `}` of the impl block:

```rust
        fn model_breakdown(
            &self,
            _provider: &str,
            _from_ms: i64,
            _to_ms: i64,
        ) -> Result<Vec<ModelBreakdownRow>, ProxyError> {
            Ok(vec![])
        }
```

Add the import at the top of the test module (or use the full path):
```rust
use crate::application::ports::ModelBreakdownRow;
```

- [ ] **Step 6: Update main.rs wiring**

Open `crates/proxy/src/main.rs`. Find the `GetAccountUsage::new` call (around line ~152) and add the `read` parameter:

```rust
let account_usage = Arc::new(proxy::application::use_cases::admin::GetAccountUsage::new(
    account_usage_map,
    read_log.clone(),
));
```

- [ ] **Step 7: Compile and fix errors**

Run: `cargo check -p proxy 2>&1`
Fix any compile errors. Key things to check:
- Imports are correct
- `ModelBreakdownItem` is imported in admin.rs
- `ModelBreakdownItemDto` is imported from proxy_admin_api
- The SQLite adapter implements the new trait method
- `main.rs` passes the `read_log` parameter

- [ ] **Step 8: Run tests**

Run: `cargo test -p proxy 2>&1 | tail -20`
Expected: All tests PASS

- [ ] **Step 9: Commit**

```bash
git add crates/proxy/src/application/use_cases/admin.rs crates/proxy/src/application/ports/request_log_read.rs crates/proxy/src/adapters/storage/sqlite_request_log.rs crates/proxy/src/main.rs
git commit -m "feat(admin): add per-model breakdown from request log to account usage"
```

---

### Task 9: Add ProviderKind::DeepSeek to TUI (proxy-tui)

**Files:**
- Modify: `crates/proxy-tui/src/app.rs`

- [ ] **Step 1: Add `DeepSeek` variant to `ProviderKind`**

Change line ~282:

```rust
pub enum ProviderKind {
    Anthropic,
    Zai,
    DeepSeek,
}
```

- [ ] **Step 2: Add label, cycle, and from_str**

In the `impl ProviderKind` block (lines ~287-309), update each function:

```rust
impl ProviderKind {
    pub fn label(self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::Zai => "zai",
            ProviderKind::DeepSeek => "deepseek",
        }
    }
    pub fn cycle_next(self) -> Self {
        match self {
            ProviderKind::Anthropic => ProviderKind::Zai,
            ProviderKind::Zai => ProviderKind::DeepSeek,
            ProviderKind::DeepSeek => ProviderKind::Anthropic,
        }
    }
    pub fn cycle_prev(self) -> Self {
        match self {
            ProviderKind::Anthropic => ProviderKind::DeepSeek,
            ProviderKind::Zai => ProviderKind::Anthropic,
            ProviderKind::DeepSeek => ProviderKind::Zai,
        }
    }
    pub fn from_str_or_default(s: &str) -> Self {
        match s {
            "zai" => ProviderKind::Zai,
            "deepseek" => ProviderKind::DeepSeek,
            _ => ProviderKind::Anthropic,
        }
    }
}
```

- [ ] **Step 3: Add default model suggestion for DeepSeek**

In `main.rs` of proxy-tui, in `open_test_modal` (around line ~866):

```rust
let suggested = match prov.kind.as_str() {
    "anthropic" => "claude-3-5-haiku-latest",
    "zai" => "glm-4.5-air",
    "deepseek" => "deepseek-chat",
    _ => "",
};
```

- [ ] **Step 4: Update tests**

Find the `provider_kind_cycle` test (lines ~801-804) and update:

```rust
#[test]
fn provider_kind_cycle() {
    assert_eq!(ProviderKind::Anthropic.cycle_next(), ProviderKind::Zai);
    assert_eq!(ProviderKind::Zai.cycle_next(), ProviderKind::DeepSeek);
    assert_eq!(ProviderKind::DeepSeek.cycle_next(), ProviderKind::Anthropic);
    // prev direction
    assert_eq!(ProviderKind::Anthropic.cycle_prev(), ProviderKind::DeepSeek);
    assert_eq!(ProviderKind::DeepSeek.cycle_prev(), ProviderKind::Zai);
    assert_eq!(ProviderKind::Zai.cycle_prev(), ProviderKind::Anthropic);
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p proxy-tui -- form_field`
Expected: Tests PASS

- [ ] **Step 6: Commit**

```bash
git add crates/proxy-tui/src/app.rs crates/proxy-tui/src/main.rs
git commit -m "feat(tui): add ProviderKind::DeepSeek with cycle, label, and default model"
```

---

### Task 10: Render per-model breakdown in TUI Account tab (proxy-tui)

**Files:**
- Modify: `crates/proxy-tui/src/views/account.rs`

- [ ] **Step 1: Render per-model breakdown lines**

In `render_provider`, after the existing model_usage rendering block (lines ~124-131), add per-model breakdown rendering:

Change the existing model_usage rendering:
```rust
    // Model usage.
    if let Some(m) = &p.model_usage {
        lines.push(ratatui::text::Line::raw(format!(
            "│  Model usage (24h):  Tokens: {}   Calls: {}",
            fmt_num_compact(m.total_tokens),
            fmt_num_compact(m.total_calls),
        )));
        // Per-model breakdown
        for item in &m.model_breakdown {
            lines.push(ratatui::text::Line::raw(format!(
                "│    {:<24} {} tokens  {} calls",
                item.model,
                fmt_num_compact(item.tokens),
                fmt_num_compact(item.calls),
            )));
        }
    }
```

- [ ] **Step 2: Handle balance windows (string-encoded values)**

Balance windows use `used`/`limit` stored as u64 (cents). For balance display, the progress bar already works since it uses `used_pct`. The label already includes currency. No change needed to `render_window` since it already renders `used`/`limit` as numbers.

However, balance windows have no `resets_at_ms` (returns empty string), which is already handled by the existing `format!("resets in {}", ...)` code that returns empty when `resets_at_ms` is `None`.

- [ ] **Step 3: Compile and verify**

Run: `cargo check -p proxy-tui 2>&1 | head -20`
Expected: Compiles successfully.

- [ ] **Step 4: Commit**

```bash
git add crates/proxy-tui/src/views/account.rs
git commit -m "feat(tui): render per-model breakdown lines on Account tab"
```

---

### Task 11: Integration verification (build + full test)

**Files:**
- None (verification only)

- [ ] **Step 1: Build the entire workspace**

Run: `cargo build 2>&1 | tail -20`
Expected: Build successful, no errors

- [ ] **Step 2: Run all tests**

Run: `cargo test 2>&1 | tail -30`
Expected: All tests PASS

- [ ] **Step 3: Clippy check**

Run: `cargo clippy --all-targets 2>&1 | tail -20`
Expected: No warnings for new code

- [ ] **Step 4: Commit (if any fixes were needed)**

```bash
git add -u
git commit -m "chore: fix clippy warnings and test failures from DeepSeek integration"
```

---

### Task 12: Update validate.rs if needed (proxy-tui)

**Files:**
- Modify: `crates/proxy-tui/src/validate.rs` (only if `ProviderKind::DeepSeek` needs special handling)

- [ ] **Step 1: Check if validate.rs needs changes**

Search for ProviderKind or kind matching in validate.rs. The validation typically only checks `kind.label()` against known values, and since `ProviderKind::DeepSeek` returns `"deepseek"` from `label()`, it should already work.

Run: `rg "ProviderKind\|kind" crates/proxy-tui/src/validate.rs`
If no ProviderKind-specific logic: skip this task.

- [ ] **Step 2: Skip if no changes needed**

If validate.rs needs no changes (likely), mark this task as done.

- [ ] **Step 3: Commit (if changes were needed)**

```bash
git add crates/proxy-tui/src/validate.rs
git commit -m "feat(validate): support DeepSeek provider kind in form validation"
```

---

### Task 13: Final verification and cleanup

**Files:**
- None (verification only)

- [ ] **Step 1: Full workspace build**

Run: `cargo build --release 2>&1 | tail -10`
Expected: Build successful

- [ ] **Step 2: Full test suite**

Run: `cargo test --release 2>&1 | tail -10`
Expected: All tests PASS, 0 failures

- [ ] **Step 3: Verify git status is clean**

Run: `git status`
Expected: Working tree clean, all changes committed

- [ ] **Step 4: Final review of commit log**

Run: `git log --oneline -13`
Expected: All 12-13 commits are meaningful and include the feat/chore/doc prefixes.
