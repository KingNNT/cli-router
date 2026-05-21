# Codex Account Usage Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show real Codex quota windows in the `proxy-tui` Account tab by querying Codex backend rate-limit headers through the existing account-usage admin path.

**Architecture:** Add a focused `CodexAccountUsage` adapter in the proxy provider account-usage module. It resolves Codex-compatible auth, sends a minimal backend probe, parses `x-codex-*` rate-limit headers into existing `UsageWindow` domain objects, and is wired from `build_account_usage()` for `ProviderKind::Codex`.

**Tech Stack:** Rust 2024, `ureq` for synchronous adapter HTTP, existing `AccountUsagePort`, existing `ProxyError::UpstreamUsage`, existing `~/.codex/auth.json` reader, Ratatui Account view through existing DTOs.

---

## File Structure

- Create `crates/proxy/src/adapters/providers/account_usage/codex.rs`
  - Owns Codex account-usage auth resolution, probe request, header parsing, and unit tests.
  - Keeps undocumented Codex header behavior isolated from other provider code.
- Modify `crates/proxy/src/adapters/providers/account_usage/mod.rs`
  - Exports `CodexAccountUsage` and declares the new module.
- Modify `crates/proxy/src/adapters/providers/builder.rs`
  - Wires `ProviderKind::Codex` to `CodexAccountUsage` instead of `NoopAccountUsage`.
  - Passes provider name, optional base URL, and cloned auth config.
- No TUI file changes are planned for the first pass.
  - Existing `GetAccountUsage` DTO mapping and `crates/proxy-tui/src/views/account.rs` already render upstream windows.

---

### Task 1: Add Codex Header Parser Tests and Helpers

**Files:**
- Create: `crates/proxy/src/adapters/providers/account_usage/codex.rs`

- [ ] **Step 1: Create the new file with failing parser tests**

Create `crates/proxy/src/adapters/providers/account_usage/codex.rs` with this content:

```rust
//! Codex account usage adapter.
//!
//! Codex does not expose a documented standalone usage endpoint. The official
//! Codex CLI derives `/status` quota data from `x-codex-*` response headers
//! returned by the Codex backend. This module mirrors that header parsing for
//! cli-router's Account tab.

use crate::domain::account_usage::UsageWindow;

const MINUTES_PER_5_HOURS: i64 = 5 * 60;
const MINUTES_PER_DAY: i64 = 24 * 60;
const MINUTES_PER_WEEK: i64 = 7 * 24 * 60;
const MINUTES_PER_MONTH: i64 = 30 * 24 * 60;
const MINUTES_PER_YEAR: i64 = 365 * 24 * 60;

fn parse_windows_from_headers(headers: &http::HeaderMap) -> Vec<UsageWindow> {
    let mut windows = Vec::new();

    if let Some(window) = parse_window(
        headers,
        "x-codex-primary-used-percent",
        "x-codex-primary-window-minutes",
        "x-codex-primary-reset-at",
        false,
    ) {
        windows.push(window);
    }

    if let Some(window) = parse_window(
        headers,
        "x-codex-secondary-used-percent",
        "x-codex-secondary-window-minutes",
        "x-codex-secondary-reset-at",
        true,
    ) {
        windows.push(window);
    }

    windows
}

fn parse_window(
    headers: &http::HeaderMap,
    used_percent_header: &str,
    window_minutes_header: &str,
    reset_at_header: &str,
    is_secondary: bool,
) -> Option<UsageWindow> {
    let used_pct = parse_header_f64(headers, used_percent_header)?;
    let window_minutes = parse_header_i64(headers, window_minutes_header);
    let reset_at_ms = parse_header_i64(headers, reset_at_header).map(|seconds| seconds * 1000);

    Some(UsageWindow {
        label: limit_label(window_minutes, is_secondary),
        used_pct,
        used: None,
        limit: None,
        resets_at_ms: reset_at_ms,
        sub_items: vec![],
    })
}

fn parse_header_f64(headers: &http::HeaderMap, name: &str) -> Option<f64> {
    headers
        .get(name)?
        .to_str()
        .ok()?
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
}

fn parse_header_i64(headers: &http::HeaderMap, name: &str) -> Option<i64> {
    headers.get(name)?.to_str().ok()?.parse::<i64>().ok()
}

fn limit_label(window_minutes: Option<i64>, is_secondary: bool) -> String {
    match window_minutes.and_then(limit_duration_label) {
        Some(duration) => format!("{duration} limit"),
        None if is_secondary => "Secondary usage limit".to_string(),
        None => "Usage limit".to_string(),
    }
}

fn limit_duration_label(window_minutes: i64) -> Option<&'static str> {
    let minutes = window_minutes.max(0);
    if is_approximate_window(minutes, MINUTES_PER_5_HOURS) {
        Some("5h")
    } else if is_approximate_window(minutes, MINUTES_PER_DAY) {
        Some("Daily")
    } else if is_approximate_window(minutes, MINUTES_PER_WEEK) {
        Some("Weekly")
    } else if is_approximate_window(minutes, MINUTES_PER_MONTH) {
        Some("Monthly")
    } else if is_approximate_window(minutes, MINUTES_PER_YEAR) {
        Some("Annual")
    } else {
        None
    }
}

fn is_approximate_window(minutes: i64, expected_minutes: i64) -> bool {
    let minutes = minutes as f64;
    let expected = expected_minutes as f64;
    minutes >= expected * 0.95 && minutes <= expected * 1.05
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderValue;

    #[test]
    fn parses_primary_5h_window() {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            "x-codex-primary-used-percent",
            HeaderValue::from_static("12.5"),
        );
        headers.insert(
            "x-codex-primary-window-minutes",
            HeaderValue::from_static("300"),
        );
        headers.insert(
            "x-codex-primary-reset-at",
            HeaderValue::from_static("1779333600"),
        );

        let windows = parse_windows_from_headers(&headers);

        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].label, "5h limit");
        assert_eq!(windows[0].used_pct, 12.5);
        assert_eq!(windows[0].resets_at_ms, Some(1_779_333_600_000));
        assert_eq!(windows[0].used, None);
        assert_eq!(windows[0].limit, None);
    }

    #[test]
    fn parses_secondary_weekly_window() {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            "x-codex-secondary-used-percent",
            HeaderValue::from_static("81"),
        );
        headers.insert(
            "x-codex-secondary-window-minutes",
            HeaderValue::from_static("10080"),
        );

        let windows = parse_windows_from_headers(&headers);

        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].label, "Weekly limit");
        assert_eq!(windows[0].used_pct, 81.0);
        assert_eq!(windows[0].resets_at_ms, None);
    }

    #[test]
    fn empty_headers_return_no_windows() {
        let headers = http::HeaderMap::new();

        let windows = parse_windows_from_headers(&headers);

        assert!(windows.is_empty());
    }

    #[test]
    fn malformed_optional_fields_do_not_drop_window() {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            "x-codex-primary-used-percent",
            HeaderValue::from_static("42"),
        );
        headers.insert(
            "x-codex-primary-window-minutes",
            HeaderValue::from_static("not-a-number"),
        );
        headers.insert(
            "x-codex-primary-reset-at",
            HeaderValue::from_static("also-bad"),
        );

        let windows = parse_windows_from_headers(&headers);

        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].label, "Usage limit");
        assert_eq!(windows[0].used_pct, 42.0);
        assert_eq!(windows[0].resets_at_ms, None);
    }

    #[test]
    fn malformed_required_percent_drops_window() {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            "x-codex-primary-used-percent",
            HeaderValue::from_static("NaN"),
        );
        headers.insert(
            "x-codex-primary-window-minutes",
            HeaderValue::from_static("300"),
        );

        let windows = parse_windows_from_headers(&headers);

        assert!(windows.is_empty());
    }
}
```

- [ ] **Step 2: Run the new tests**

Run:

```bash
cargo test -p proxy adapters::providers::account_usage::codex --lib
```

Expected: FAIL because `codex` is not declared in `account_usage/mod.rs` yet. The failure should mention that the test target or module path cannot be found.

- [ ] **Step 3: Declare the module so parser tests compile**

Modify `crates/proxy/src/adapters/providers/account_usage/mod.rs` to include the new module but do not export `CodexAccountUsage` yet. The file should contain:

```rust
pub mod anthropic;
pub mod codex;
pub mod deepseek;
pub mod noop;
pub mod zai;

pub use anthropic::AnthropicAccountUsage;
pub use deepseek::DeepSeekAccountUsage;
pub use zai::ZaiAccountUsage;
```

- [ ] **Step 4: Run parser tests again**

Run:

```bash
cargo test -p proxy adapters::providers::account_usage::codex --lib
```

Expected: PASS. The output should show the five `codex` tests pass.

- [ ] **Step 5: Commit parser helpers**

Run:

```bash
git add crates/proxy/src/adapters/providers/account_usage/mod.rs crates/proxy/src/adapters/providers/account_usage/codex.rs
git commit -m "test(proxy): add Codex rate limit header parser"
```

Expected: commit succeeds.

---

### Task 2: Implement CodexAccountUsage Auth Resolution and Adapter Shape

**Files:**
- Modify: `crates/proxy/src/adapters/providers/account_usage/codex.rs`
- Modify: `crates/proxy/src/adapters/providers/account_usage/mod.rs`

- [ ] **Step 1: Add failing auth-resolution tests**

Append these tests inside the existing `#[cfg(test)] mod tests` in `codex.rs`:

```rust
    #[test]
    fn bearer_auth_resolves_to_token() {
        let auth = crate::config::AuthConfig::Bearer {
            value: "token-123".to_string(),
        };

        let token = resolve_token(&auth).expect("bearer auth should resolve");

        assert_eq!(token, Some("token-123".to_string()));
    }

    #[test]
    fn openai_oauth_auth_resolves_to_access_token() {
        let auth = crate::config::AuthConfig::OpenAiOAuth {
            access_token: "access-123".to_string(),
            refresh_token: "refresh-123".to_string(),
            expires_at_ms: 1_779_333_600_000,
        };

        let token = resolve_token(&auth).expect("oauth auth should resolve");

        assert_eq!(token, Some("access-123".to_string()));
    }

    #[test]
    fn api_key_auth_is_not_supported_for_chatgpt_plan_limits() {
        let auth = crate::config::AuthConfig::ApiKey {
            value: "sk-test".to_string(),
        };

        let token = resolve_token(&auth).expect("api key should be cleanly unsupported");

        assert_eq!(token, None);
    }

    #[test]
    fn passthrough_auth_is_not_supported_for_account_refresh() {
        let auth = crate::config::AuthConfig::Passthrough;

        let token = resolve_token(&auth).expect("passthrough should be cleanly unsupported");

        assert_eq!(token, None);
    }

    #[test]
    fn codex_account_usage_new_defaults_base_url() {
        let adapter = CodexAccountUsage::new(
            "codex".to_string(),
            None,
            crate::config::AuthConfig::Bearer {
                value: "token-123".to_string(),
            },
        );

        assert_eq!(adapter.base_url, DEFAULT_BASE_URL);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p proxy adapters::providers::account_usage::codex --lib
```

Expected: FAIL with missing `resolve_token`, `CodexAccountUsage`, and `DEFAULT_BASE_URL` errors.

- [ ] **Step 3: Add adapter struct and auth resolution**

Replace the top of `codex.rs` before the parser functions with this expanded code. Keep the parser functions and tests from Task 1 below it.

```rust
//! Codex account usage adapter.
//!
//! Codex does not expose a documented standalone usage endpoint. The official
//! Codex CLI derives `/status` quota data from `x-codex-*` response headers
//! returned by the Codex backend. This module mirrors that header parsing for
//! cli-router's Account tab.

use std::time::Duration;

use crate::application::errors::ProxyError;
use crate::application::ports::AccountUsagePort;
use crate::config::AuthConfig;
use crate::domain::account_usage::{AccountUsageStatus, ProviderAccountUsage, UsageWindow};

const DEFAULT_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
const RESPONSES_PATH: &str = "/responses";
const MINUTES_PER_5_HOURS: i64 = 5 * 60;
const MINUTES_PER_DAY: i64 = 24 * 60;
const MINUTES_PER_WEEK: i64 = 7 * 24 * 60;
const MINUTES_PER_MONTH: i64 = 30 * 24 * 60;
const MINUTES_PER_YEAR: i64 = 365 * 24 * 60;

/// Codex account usage adapter. Queries Codex backend headers for ChatGPT plan limits.
pub struct CodexAccountUsage {
    provider_name: String,
    base_url: String,
    auth: AuthConfig,
    agent: ureq::Agent,
}

impl CodexAccountUsage {
    pub fn new(provider_name: String, base_url: Option<String>, auth: AuthConfig) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_read(Duration::from_secs(10))
            .timeout_write(Duration::from_secs(10))
            .build();
        Self {
            provider_name,
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            auth,
            agent,
        }
    }
}

fn resolve_token(auth: &AuthConfig) -> Result<Option<String>, ProxyError> {
    match auth {
        AuthConfig::CodexAuto => crate::adapters::oauth::openai::read_auth_json()
            .map(|tokens| Some(tokens.access_token))
            .map_err(|e| ProxyError::UpstreamUsage {
                provider: "codex".to_string(),
                message: format!(
                    "CodexAuto requires ~/.codex/auth.json. Run `codex login` first. ({e})"
                ),
            }),
        AuthConfig::OpenAiOAuth { access_token, .. } | AuthConfig::Bearer { value: access_token } => {
            Ok(Some(access_token.clone()))
        }
        AuthConfig::ApiKey { .. } | AuthConfig::Passthrough => Ok(None),
        AuthConfig::AnthropicOAuth { .. } => Ok(None),
    }
}
```

Important: do not duplicate constants that already exist in the file from Task 1. Move the existing constants into this top block so each constant is defined once.

- [ ] **Step 4: Add temporary AccountUsagePort implementation**

Add this implementation before the parser functions in `codex.rs`:

```rust
impl AccountUsagePort for CodexAccountUsage {
    fn fetch_usage(&self) -> Option<Result<ProviderAccountUsage, ProxyError>> {
        let token = match resolve_token(&self.auth) {
            Ok(Some(token)) => token,
            Ok(None) => return None,
            Err(e) => return Some(Err(e)),
        };

        if token.trim().is_empty() {
            return None;
        }

        Some(Ok(ProviderAccountUsage {
            provider: self.provider_name.clone(),
            status: AccountUsageStatus::Available,
            plan: None,
            windows: vec![],
            model_usage: None,
        }))
    }
}
```

This is intentionally incomplete. Task 3 replaces the empty-window behavior with the real probe. Keeping this temporary implementation lets the auth tests pass independently.

- [ ] **Step 5: Export CodexAccountUsage**

Modify `crates/proxy/src/adapters/providers/account_usage/mod.rs` to export the adapter:

```rust
pub mod anthropic;
pub mod codex;
pub mod deepseek;
pub mod noop;
pub mod zai;

pub use anthropic::AnthropicAccountUsage;
pub use codex::CodexAccountUsage;
pub use deepseek::DeepSeekAccountUsage;
pub use zai::ZaiAccountUsage;
```

- [ ] **Step 6: Run codex adapter tests**

Run:

```bash
cargo test -p proxy adapters::providers::account_usage::codex --lib
```

Expected: PASS.

- [ ] **Step 7: Commit adapter shape**

Run:

```bash
git add crates/proxy/src/adapters/providers/account_usage/codex.rs crates/proxy/src/adapters/providers/account_usage/mod.rs
git commit -m "feat(proxy): add Codex account usage adapter shell"
```

Expected: commit succeeds.

---

### Task 3: Implement Codex Backend Probe and Response Mapping

**Files:**
- Modify: `crates/proxy/src/adapters/providers/account_usage/codex.rs`

- [ ] **Step 1: Add failing probe-response mapping tests**

Append these tests inside the existing `#[cfg(test)] mod tests` in `codex.rs`:

```rust
    #[test]
    fn response_headers_map_to_available_provider_usage() {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            "x-codex-primary-used-percent",
            HeaderValue::from_static("10"),
        );
        headers.insert(
            "x-codex-primary-window-minutes",
            HeaderValue::from_static("300"),
        );
        headers.insert(
            "x-codex-secondary-used-percent",
            HeaderValue::from_static("70"),
        );
        headers.insert(
            "x-codex-secondary-window-minutes",
            HeaderValue::from_static("10080"),
        );

        let usage = provider_usage_from_headers("codex-main", &headers)
            .expect("headers should produce usage");

        assert_eq!(usage.provider, "codex-main");
        assert_eq!(usage.status, AccountUsageStatus::Available);
        assert_eq!(usage.windows.len(), 2);
        assert_eq!(usage.windows[0].label, "5h limit");
        assert_eq!(usage.windows[0].used_pct, 10.0);
        assert_eq!(usage.windows[1].label, "Weekly limit");
        assert_eq!(usage.windows[1].used_pct, 70.0);
        assert!(usage.model_usage.is_none());
    }

    #[test]
    fn response_headers_without_limits_return_none() {
        let headers = http::HeaderMap::new();

        let usage = provider_usage_from_headers("codex-main", &headers);

        assert!(usage.is_none());
    }

    #[test]
    fn probe_url_joins_base_url_and_responses_path() {
        assert_eq!(
            probe_url("https://chatgpt.com/backend-api/codex"),
            "https://chatgpt.com/backend-api/codex/responses"
        );
        assert_eq!(
            probe_url("https://chatgpt.com/backend-api/codex/"),
            "https://chatgpt.com/backend-api/codex/responses"
        );
    }

    #[test]
    fn probe_body_is_minimal_streaming_responses_request() {
        let body = codex_probe_body();

        assert_eq!(body["model"], "gpt-5.4-mini");
        assert_eq!(body["instructions"], "");
        assert_eq!(body["stream"], true);
        assert_eq!(body["store"], false);
        assert!(body["input"].as_array().unwrap().is_empty());
        assert!(body["tools"].as_array().unwrap().is_empty());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p proxy adapters::providers::account_usage::codex --lib
```

Expected: FAIL with missing `provider_usage_from_headers`, `probe_url`, and `codex_probe_body`.

- [ ] **Step 3: Add probe helpers and response mapping**

Add these functions after the `impl AccountUsagePort for CodexAccountUsage` block and before `parse_windows_from_headers`:

```rust
fn provider_usage_from_headers(
    provider_name: &str,
    headers: &http::HeaderMap,
) -> Option<ProviderAccountUsage> {
    let windows = parse_windows_from_headers(headers);
    if windows.is_empty() {
        return None;
    }

    Some(ProviderAccountUsage {
        provider: provider_name.to_string(),
        status: AccountUsageStatus::Available,
        plan: None,
        windows,
        model_usage: None,
    })
}

fn probe_url(base_url: &str) -> String {
    format!(
        "{}{}",
        base_url.trim_end_matches('/'),
        RESPONSES_PATH
    )
}

fn codex_probe_body() -> serde_json::Value {
    serde_json::json!({
        "model": "gpt-5.4-mini",
        "instructions": "",
        "input": [],
        "tools": [],
        "tool_choice": "auto",
        "parallel_tool_calls": false,
        "store": false,
        "stream": true
    })
}
```

- [ ] **Step 4: Replace temporary fetch_usage with real probe logic**

Replace the entire `impl AccountUsagePort for CodexAccountUsage` block with:

```rust
impl AccountUsagePort for CodexAccountUsage {
    fn fetch_usage(&self) -> Option<Result<ProviderAccountUsage, ProxyError>> {
        let token = match resolve_token(&self.auth) {
            Ok(Some(token)) => token,
            Ok(None) => return None,
            Err(e) => return Some(Err(e)),
        };

        if token.trim().is_empty() {
            return None;
        }

        let url = probe_url(&self.base_url);
        let body = codex_probe_body();
        let response = self
            .agent
            .post(&url)
            .set("Authorization", &format!("Bearer {token}"))
            .set("Accept", "text/event-stream")
            .set("Content-Type", "application/json")
            .send_json(body);

        match response {
            Ok(resp) => {
                let headers = ureq_headers_to_http(resp.headers_names(), &resp);
                match provider_usage_from_headers(&self.provider_name, &headers) {
                    Some(usage) => Some(Ok(usage)),
                    None => Some(Err(ProxyError::UpstreamUsage {
                        provider: self.provider_name.clone(),
                        message: "Codex response did not include rate-limit headers".to_string(),
                    })),
                }
            }
            Err(ureq::Error::Status(_, resp)) => {
                let headers = ureq_headers_to_http(resp.headers_names(), &resp);
                if let Some(usage) = provider_usage_from_headers(&self.provider_name, &headers) {
                    return Some(Ok(usage));
                }

                Some(Err(ProxyError::UpstreamUsage {
                    provider: self.provider_name.clone(),
                    message: "Codex probe failed and response did not include rate-limit headers"
                        .to_string(),
                }))
            }
            Err(e) => Some(Err(ProxyError::UpstreamUsage {
                provider: self.provider_name.clone(),
                message: format!("Codex probe: {e}"),
            })),
        }
    }
}
```

- [ ] **Step 5: Add ureq header conversion helper**

Add this function after `codex_probe_body()`:

```rust
fn ureq_headers_to_http(names: Vec<String>, response: &ureq::Response) -> http::HeaderMap {
    let mut headers = http::HeaderMap::new();
    for name in names {
        let Some(value) = response.header(&name) else {
            continue;
        };
        let Ok(header_name) = http::HeaderName::from_bytes(name.as_bytes()) else {
            continue;
        };
        let Ok(header_value) = http::HeaderValue::from_str(value) else {
            continue;
        };
        headers.insert(header_name, header_value);
    }
    headers
}
```

- [ ] **Step 6: Run codex adapter tests**

Run:

```bash
cargo test -p proxy adapters::providers::account_usage::codex --lib
```

Expected: PASS.

- [ ] **Step 7: Run formatting**

Run:

```bash
cargo fmt --all
```

Expected: command exits 0.

- [ ] **Step 8: Commit probe implementation**

Run:

```bash
git add crates/proxy/src/adapters/providers/account_usage/codex.rs
git commit -m "feat(proxy): fetch Codex account rate limits"
```

Expected: commit succeeds.

---

### Task 4: Wire CodexAccountUsage into Provider Builder

**Files:**
- Modify: `crates/proxy/src/adapters/providers/builder.rs`

- [ ] **Step 1: Add failing builder test**

Inside the existing `#[cfg(test)] mod tests` in `crates/proxy/src/adapters/providers/builder.rs`, add this test:

```rust
    #[test]
    fn build_account_usage_maps_codex_to_supported_adapter() {
        let config = Config {
            providers: vec![ProviderConfig {
                name: "codex-main".to_string(),
                kind: ProviderKind::Codex,
                base_url: Some("https://example.test/backend-api/codex".to_string()),
                openai_base_url: None,
                auth: AuthConfig::Bearer {
                    value: "token-123".to_string(),
                },
                models: vec!["gpt-5.4-mini".to_string()],
                priority: 0,
            }],
            ..cfg(vec![])
        };
        let adapters = build_account_usage(Arc::new(std::sync::RwLock::new(config)));
        let adapter = adapters.get("codex-main").expect("codex adapter exists");

        let result = adapter.fetch_usage();

        assert!(result.is_some(), "Codex should not use NoopAccountUsage");
    }
```

If the local `ProviderConfig` fields differ, adjust this test to match the helper patterns already used in the same test module. Keep the assertion: `fetch_usage()` must return `Some(...)` for bearer-auth Codex, proving the adapter is not noop.

- [ ] **Step 2: Run the builder test to verify it fails**

Run:

```bash
cargo test -p proxy build_account_usage_maps_codex_to_supported_adapter --lib
```

Expected: FAIL because Codex still maps to `NoopAccountUsage`, so `fetch_usage()` returns `None`.

- [ ] **Step 3: Import CodexAccountUsage**

Modify the import near the top of `builder.rs` from:

```rust
use super::account_usage::{AnthropicAccountUsage, DeepSeekAccountUsage, ZaiAccountUsage};
```

to:

```rust
use super::account_usage::{
    AnthropicAccountUsage, CodexAccountUsage, DeepSeekAccountUsage, ZaiAccountUsage,
};
```

- [ ] **Step 4: Wire Codex provider kind to CodexAccountUsage**

In `build_account_usage()`, replace:

```rust
ProviderKind::Codex => Arc::new(super::account_usage::noop::NoopAccountUsage),
```

with:

```rust
ProviderKind::Codex => Arc::new(CodexAccountUsage::new(
    p.name.clone(),
    p.base_url.clone(),
    p.auth.clone(),
)),
```

- [ ] **Step 5: Run the builder test**

Run:

```bash
cargo test -p proxy build_account_usage_maps_codex_to_supported_adapter --lib
```

Expected: PASS. The test may return `Some(Err(...))` because `https://example.test` is not reachable; that is acceptable because the assertion only verifies Codex is no longer noop.

- [ ] **Step 6: Run related tests**

Run:

```bash
cargo test -p proxy adapters::providers::builder --lib
cargo test -p proxy adapters::providers::account_usage --lib
```

Expected: PASS.

- [ ] **Step 7: Commit builder wiring**

Run:

```bash
git add crates/proxy/src/adapters/providers/builder.rs
git commit -m "feat(proxy): wire Codex account usage adapter"
```

Expected: commit succeeds.

---

### Task 5: Final Verification and Documentation Check

**Files:**
- Modify only if verification reveals compile, formatting, or clippy issues.

- [ ] **Step 1: Run full formatting check**

Run:

```bash
cargo fmt --all -- --check
```

Expected: PASS. If it fails, run `cargo fmt --all`, then re-run the check.

- [ ] **Step 2: Run proxy tests**

Run:

```bash
cargo test -p proxy
```

Expected: PASS.

- [ ] **Step 3: Run proxy-tui tests**

Run:

```bash
cargo test -p proxy-tui
```

Expected: PASS. This confirms existing Account TUI code still compiles against unchanged DTOs.

- [ ] **Step 4: Run workspace check**

Run:

```bash
cargo check --workspace
```

Expected: PASS.

- [ ] **Step 5: Inspect git diff**

Run:

```bash
git status --short
git diff --stat
git diff
```

Expected: only intended Codex account-usage files are changed. `.superpowers/` may be untracked from brainstorming and must not be committed unless the project already tracks it.

- [ ] **Step 6: Commit any verification fixes**

If Step 1–4 required fixes, commit them:

```bash
git add crates/proxy/src/adapters/providers/account_usage/codex.rs crates/proxy/src/adapters/providers/account_usage/mod.rs crates/proxy/src/adapters/providers/builder.rs
git commit -m "fix(proxy): stabilize Codex account usage tests"
```

Expected: commit succeeds if there were fixes. If there were no fixes, skip this step.

---

## Manual Smoke Test

After automated tests pass, run this against a local proxy configured with a Codex provider using `auth = codex_auto` and a valid `~/.codex/auth.json`:

```bash
cargo run -p proxy
```

In another terminal:

```bash
cargo run -p proxy-tui
```

Open the Account tab and press `r` to refresh.

Expected:

- Codex provider does not display “No account usage API available”.
- If Codex backend returns rate-limit headers, Account shows windows such as `5h limit` and `Weekly limit` under `── upstream API ──`.
- If the backend contract has changed, Account shows a provider-level error saying the response did not include rate-limit headers.
- Other providers still render as before.

---

## Self-Review

- Spec coverage:
  - Codex adapter: Task 2 and Task 3.
  - Header parsing: Task 1 and Task 3.
  - Auth modes: Task 2.
  - Builder wiring: Task 4.
  - Existing TUI reuse: Task 5 and manual smoke test.
  - Error behavior for missing headers and failed probes: Task 3.
- Placeholder scan: no forbidden placeholder text or vague implementation steps remain.
- Type consistency:
  - Uses existing `AuthConfig`, `AccountUsagePort`, `ProxyError::UpstreamUsage`, `ProviderAccountUsage`, `UsageWindow`, and `AccountUsageStatus`.
  - Planned `CodexAccountUsage::new(provider_name, base_url, auth)` matches builder wiring.
  - `used_pct` stays percent used, matching existing Account TUI expectations.
