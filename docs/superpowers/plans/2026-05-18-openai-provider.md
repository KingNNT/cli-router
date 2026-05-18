# OpenAI Provider with Codex OAuth — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add OpenAI as a provider to cli-router supporting both API key (Bearer) and Codex OAuth (ChatGPT subscription) authentication.

**Architecture:** Single `ProviderKind::OpenAi` with auth-determined behaviour. The provider adapter mirrors `DeepSeekProvider` since OpenAI's Codex OAuth uses the same `/v1/chat/completions` endpoint. A new `oauth::openai` module implements the Codex PKCE browser flow, matching the existing `oauth::anthropic` pattern.

**Tech Stack:** Rust, reqwest, axum, serde, sha2, base64, wiremock (tests)

**Spec:** `docs/superpowers/specs/2026-05-18-openai-provider-design.md`

---

## File Structure

| Action | File | Responsibility |
|--------|------|----------------|
| Create | `crates/proxy/src/adapters/providers/openai.rs` | OpenAI provider adapter |
| Create | `crates/proxy/src/adapters/oauth/openai.rs` | Codex OAuth PKCE flow |
| Modify | `crates/proxy/src/config.rs` | +ProviderKind::OpenAi, +AuthConfig::OpenAiOAuth |
| Modify | `crates/proxy/src/adapters/providers/mod.rs` | +pub mod openai |
| Modify | `crates/proxy/src/adapters/providers/builder.rs` | +OpenAi arms in build_leaf, build_account_usage |
| Modify | `crates/proxy/src/adapters/oauth/mod.rs` | +pub mod openai |
| Modify | `crates/proxy/src/adapters/providers/token_refresh.rs` | +OpenAiOAuth refresh |
| Modify | `crates/proxy/src/application/use_cases/admin.rs` | +StartOpenAiOAuth, +CompleteOpenAiOAuth, +mapping arms |
| Modify | `crates/proxy/src/frameworks/admin.rs` | +2 OAuth routes, +AdminState fields |
| Modify | `crates/proxy/src/main.rs` | Wire new OAuth use cases into AdminState |
| Modify | `crates/proxy-admin-api/src/lib.rs` | +AuthPayload::OpenAiOAuth |
| Modify | `crates/proxy-tui/src/app.rs` | +AuthInputKind::OAuthOpenAi |
| Modify | `crates/proxy-tui/src/ui.rs` | +OpenAi auth rendering |
| Modify | `crates/proxy-tui/src/client.rs` | +oauth_start_openai, +oauth_complete_openai |
| Modify | `crates/proxy-tui/src/main.rs` | +OpenAi OAuth form submission |

---

### Task 1: Add `ProviderKind::OpenAi` and `AuthConfig::OpenAiOAuth` to config

**Files:**
- Modify: `crates/proxy/src/config.rs`
- Test: inline `#[cfg(test)] mod tests`

- [ ] **Step 1: Write the failing tests**

Add tests at the bottom of the `tests` module in `config.rs`:

```rust
#[test]
fn parse_kind_accepts_openai_aliases() {
    assert_eq!(parse_kind("openai"), Some(ProviderKind::OpenAi));
    assert_eq!(parse_kind("open_ai"), Some(ProviderKind::OpenAi));
    assert_eq!(parse_kind("OpenAI"), Some(ProviderKind::OpenAi));
}

#[test]
fn toml_parses_openai_provider() {
    let raw = r#"
[[providers]]
name = "openai"
kind = "openai"
auth = { type = "bearer", value = "sk-test" }
base_url = "https://api.openai.com/v1"
"#;
    let cfg: Config = toml::from_str(raw).unwrap();
    assert_eq!(cfg.providers.len(), 1);
    assert_eq!(cfg.providers[0].kind, ProviderKind::OpenAi);
    assert!(matches!(cfg.providers[0].auth, AuthConfig::Bearer { .. }));
}

#[test]
fn toml_parses_openai_oauth_auth() {
    let raw = r#"
[[providers]]
name = "codex"
kind = "openai"
auth = { type = "openai_oauth", access_token = "at", refresh_token = "rt", expires_at_ms = 123 }
"#;
    let cfg: Config = toml::from_str(raw).unwrap();
    assert!(matches!(
        &cfg.providers[0].auth,
        AuthConfig::OpenAiOAuth { access_token, refresh_token, expires_at_ms }
        if access_token == "at" && refresh_token == "rt" && *expires_at_ms == 123
    ));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p proxy -- parse_kind_accepts_openai toml_parses_openai_provider toml_parses_openai_oauth`
Expected: compile errors — `ProviderKind::OpenAi` and `AuthConfig::OpenAiOAuth` don't exist yet.

- [ ] **Step 3: Add `ProviderKind::OpenAi`**

In `config.rs`, add the variant to the enum:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Anthropic,
    Zai,
    #[serde(alias = "deepseek")]
    DeepSeek,
    #[serde(alias = "open_ai")]
    OpenAi,
}
```

- [ ] **Step 4: Add `AuthConfig::OpenAiOAuth`**

In `config.rs`, add the variant and update the `Debug` impl:

```rust
// In AuthConfig enum, after AnthropicOAuth:
    /// Full OpenAI Codex OAuth session. The proxy auto-refreshes the access
    /// token before expiry using the refresh token, and retries once on 401.
    #[serde(rename = "openai_oauth")]
    OpenAiOAuth {
        access_token: String,
        refresh_token: String,
        /// Unix epoch millis when the access token expires.
        expires_at_ms: u64,
    },
```

Update the `Debug` impl for `AuthConfig`:

```rust
// Add this arm to the match in fmt():
            AuthConfig::OpenAiOAuth { expires_at_ms, .. } => f
                .debug_struct("OpenAiOAuth")
                .field("access_token", &REDACTED)
                .field("refresh_token", &REDACTED)
                .field("expires_at_ms", expires_at_ms)
                .finish(),
```

- [ ] **Step 5: Add `parse_kind` arm**

In `parse_kind()`, add:

```rust
        "openai" | "open_ai" => Some(ProviderKind::OpenAi),
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p proxy -- parse_kind_accepts_openai toml_parses_openai_provider toml_parses_openai_oauth`
Expected: all 3 PASS

- [ ] **Step 7: Run full workspace tests**

Run: `cargo test --workspace`
Expected: some failures in `admin.rs` and `builder.rs` — match arms are now non-exhaustive. That's expected, we fix them in subsequent tasks.

- [ ] **Step 8: Commit**

```
feat(config): add ProviderKind::OpenAi and AuthConfig::OpenAiOAuth
```

---

### Task 2: Add `AuthPayload::OpenAiOAuth` to proxy-admin-api

**Files:**
- Modify: `crates/proxy-admin-api/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Add test in the appropriate test module in `lib.rs`:

```rust
#[test]
fn openai_oauth_payload_round_trips() {
    let auth = AuthPayload::OpenAiOAuth {
        access_token: "at".into(),
        refresh_token: "rt".into(),
        expires_at_ms: 999,
    };
    let json = serde_json::to_string(&auth).unwrap();
    assert!(json.contains("\"type\":\"openai_oauth\""));
    let back: AuthPayload = serde_json::from_str(&json).unwrap();
    assert_eq!(auth, back);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p proxy-admin-api -- openai_oauth_payload_round_trips`
Expected: compile error — `AuthPayload::OpenAiOAuth` doesn't exist.

- [ ] **Step 3: Add the variant**

In the `AuthPayload` enum:

```rust
    #[serde(rename = "openai_oauth")]
    OpenAiOAuth {
        access_token: String,
        refresh_token: String,
        expires_at_ms: u64,
    },
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p proxy-admin-api -- openai_oauth_payload_round_trips`
Expected: PASS

- [ ] **Step 5: Run full proxy-admin-api tests**

Run: `cargo test -p proxy-admin-api`
Expected: all PASS

- [ ] **Step 6: Commit**

```
feat(admin-api): add AuthPayload::OpenAiOAuth DTO
```

---

### Task 3: Create `OpenAiProvider` adapter

**Files:**
- Create: `crates/proxy/src/adapters/providers/openai.rs`
- Modify: `crates/proxy/src/adapters/providers/mod.rs`
- Modify: `crates/proxy/src/adapters/providers/builder.rs`

- [ ] **Step 1: Create the provider file**

Create `crates/proxy/src/adapters/providers/openai.rs`:

```rust
//! OpenAI provider adapter.
//!
//! Supports both API key (Bearer) and Codex OAuth authentication.
//! Speaks the OpenAI `/v1/chat/completions` protocol natively.

use super::messages_protocol::AuthHeader;
use crate::application::errors::ProxyError;
use crate::application::ports::upstream::{ApiFormat, UpstreamResponse, Provider};
use async_trait::async_trait;
use axum::http::HeaderMap;
use bytes::Bytes;
use std::sync::Arc;

const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

pub struct OpenAiProvider {
    base_url: String,
    http: reqwest::Client,
    auth: AuthHeader,
}

impl OpenAiProvider {
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
impl Provider for OpenAiProvider {
    fn name(&self) -> &'static str {
        "openai"
    }

    fn native_format(&self) -> ApiFormat {
        ApiFormat::OpenAI
    }

    fn parse_model(&self, body: &[u8]) -> Result<String, String> {
        super::messages_protocol::parse_model(body)
    }

    fn usage_parser(&self) -> Box<dyn super::messages_protocol::UsageParser> {
        super::messages_protocol::openai_usage_parser()
    }

    fn parse_usage_json(&self, body: &[u8]) -> Result<crate::shared::domain::entities::usage_record::UsageRecord, String> {
        super::messages_protocol::parse_openai_usage_json(body)
    }

    fn usage_parser_openai(&self) -> Box<dyn super::messages_protocol::UsageParser> {
        super::messages_protocol::openai_usage_parser()
    }

    fn parse_usage_json_openai(&self, body: &[u8]) -> Result<crate::shared::domain::entities::usage_record::UsageRecord, String> {
        super::messages_protocol::parse_openai_usage_json(body)
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
            "provider 'openai' does not support Anthropic messages format; use the OpenAI-compatible endpoint".into(),
        ))
    }

    async fn forward_openai(
        &self,
        path: &str,
        headers: &HeaderMap,
        body: Bytes,
        streaming: bool,
    ) -> Result<UpstreamResponse, ProxyError> {
        super::messages_protocol::forward(
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> OpenAiProvider {
        OpenAiProvider::new(reqwest::Client::new())
    }

    #[test]
    fn name_is_openai() {
        assert_eq!(provider().name(), "openai");
    }

    #[test]
    fn native_format_is_openai() {
        assert!(matches!(provider().native_format(), ApiFormat::OpenAI));
    }

    #[test]
    fn default_base_url_points_to_openai_api() {
        let p = provider();
        assert!(p.base_url.starts_with("https://api.openai.com"));
    }

    #[test]
    fn with_base_url_overrides_default() {
        let p = OpenAiProvider::with_base_url(reqwest::Client::new(), "https://custom.api/v1");
        assert_eq!(p.base_url, "https://custom.api/v1");
    }

    #[test]
    fn default_auth_is_passthrough() {
        let p = provider();
        assert!(matches!(p.auth, AuthHeader::Passthrough));
    }

    #[test]
    fn configure_sets_base_url_and_auth() {
        let p = OpenAiProvider::configure(
            reqwest::Client::new(),
            Some("https://custom.api/v1".into()),
            AuthHeader::Bearer("sk-test".into()),
        );
        assert_eq!(p.base_url, "https://custom.api/v1");
        assert!(matches!(p.auth, AuthHeader::Bearer(_)));
    }

    #[test]
    fn configure_uses_default_base_url_when_none() {
        let p = OpenAiProvider::configure(reqwest::Client::new(), None, AuthHeader::Passthrough);
        assert!(p.base_url.starts_with("https://api.openai.com"));
    }

    #[test]
    fn parses_model_from_body() {
        let body = br#"{"model":"gpt-5","messages":[]}"#;
        assert_eq!(provider().parse_model(body).unwrap(), "gpt-5");
    }
}
```

- [ ] **Step 2: Register the module**

In `crates/proxy/src/adapters/providers/mod.rs`, add:

```rust
pub mod openai;
```

And add to the `pub use` block:

```rust
pub use openai::OpenAiProvider;
```

- [ ] **Step 3: Update builder.rs**

Add `OpenAiProvider` to the import at the top of `builder.rs`, then add the `build_leaf` arm:

```rust
        ProviderKind::OpenAi => Arc::new(OpenAiProvider::configure(
            http,
            p.base_url.clone(),
            auth,
        )),
```

Add the `build_account_usage` arm:

```rust
        ProviderKind::OpenAi => {
            Arc::new(super::account_usage::noop::NoopAccountUsage)
        }
```

- [ ] **Step 4: Run provider tests**

Run: `cargo test -p proxy -- adapters::providers::openai`
Expected: all 8 tests PASS

- [ ] **Step 5: Commit**

```
feat(proxy): add OpenAiProvider adapter with OpenAI-native forwarding
```

---

### Task 4: Create Codex OAuth module

**Files:**
- Create: `crates/proxy/src/adapters/oauth/openai.rs`
- Modify: `crates/proxy/src/adapters/oauth/mod.rs`

- [ ] **Step 1: Create the OAuth module**

Create `crates/proxy/src/adapters/oauth/openai.rs`:

```rust
//! OpenAI Codex OAuth (PKCE) — browser-flow variant.
//!
//! Reuses the OpenAI Codex OAuth endpoints that `codex login` uses.
//! The flow:
//!
//! 1. Daemon generates a PKCE verifier + challenge + state.
//! 2. Caller opens the resulting authorization URL in a browser.
//! 3. User signs in with their ChatGPT account.
//! 4. Browser redirects to localhost callback with `code` and `state`.
//! 5. Caller submits `code` to `complete_oauth` — we exchange for tokens
//!    and return the access token.
//!
//! Risk: the `CLIENT_ID` is extracted from the Codex CLI client. OpenAI
//! may rotate or restrict it without notice. Additionally, OpenAI has not
//! yet banned third-party Codex OAuth usage (unlike Anthropic for Claude
//! Code OAuth in April 2026), but may do so in the future.

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Mutex;
use thiserror::Error;
use uuid::Uuid;

/// Codex CLI's registered OAuth client ID.
/// TODO: verify against Codex CLI source or CLIProxyAPI implementation.
pub const CLIENT_ID: &str = "TdJIcbe16WoTHtN95nyywh5E4yOo6ItG";
pub const AUTHORIZE_URL: &str = "https://auth.openai.com/authorize";
pub const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
pub const REDIRECT_URI: &str = "http://localhost:1455/callback";
pub const SCOPES: &str = "openid email profile";

#[derive(Debug, Error)]
pub enum OAuthError {
    #[error("unknown state — start a new flow")]
    UnknownState,
    #[error("token exchange transport: {0}")]
    Transport(String),
    #[error("token exchange status {0}: {1}")]
    Upstream(u16, String),
    #[error("token response decode: {0}")]
    Decode(String),
}

#[derive(Debug, Clone)]
pub struct PkceCodes {
    pub verifier: String,
    pub challenge: String,
    pub state: String,
}

/// Build a PKCE verifier (RFC 7636: 43-128 chars from the unreserved set)
/// from two stitched UUIDs base64-url-encoded with no padding.
pub fn generate_pkce() -> PkceCodes {
    let raw = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw.as_bytes());
    let mut h = Sha256::new();
    h.update(verifier.as_bytes());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(h.finalize());
    let state = verifier.clone();
    PkceCodes {
        verifier,
        challenge,
        state,
    }
}

pub fn build_authorize_url(codes: &PkceCodes) -> String {
    let scopes_enc = SCOPES.replace(' ', "%20");
    let redirect_enc = REDIRECT_URI.replace(':', "%3A").replace('/', "%2F");
    format!(
        "{AUTHORIZE_URL}?\
         response_type=code&\
         client_id={CLIENT_ID}&\
         redirect_uri={redirect_enc}&\
         scope={scopes_enc}&\
         state={state}&\
         code_challenge={challenge}&\
         code_challenge_method=S256",
        state = codes.state,
        challenge = codes.challenge,
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<u64>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
}

pub async fn exchange_code(
    http: &reqwest::Client,
    code: &str,
    state: &str,
    verifier: &str,
) -> Result<OAuthTokens, OAuthError> {
    let body = serde_json::json!({
        "grant_type": "authorization_code",
        "code": code,
        "state": state,
        "client_id": CLIENT_ID,
        "redirect_uri": REDIRECT_URI,
        "code_verifier": verifier,
    });
    let resp = http
        .post(TOKEN_URL)
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .map_err(|e| OAuthError::Transport(e.to_string()))?;
    let status = resp.status().as_u16();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| OAuthError::Transport(e.to_string()))?;
    if !(200..300).contains(&status) {
        let msg = String::from_utf8_lossy(&bytes).to_string();
        return Err(OAuthError::Upstream(status, msg));
    }
    let parsed: TokenResponse =
        serde_json::from_slice(&bytes).map_err(|e| OAuthError::Decode(e.to_string()))?;
    Ok(OAuthTokens {
        access_token: parsed.access_token,
        refresh_token: parsed.refresh_token,
        expires_in: parsed.expires_in,
    })
}

/// Refresh an expired access token using a refresh token.
pub async fn refresh_token(
    http: &reqwest::Client,
    old_refresh_token: &str,
) -> Result<OAuthTokens, OAuthError> {
    let body = serde_json::json!({
        "grant_type": "refresh_token",
        "client_id": CLIENT_ID,
        "refresh_token": old_refresh_token,
    });
    let resp = http
        .post(TOKEN_URL)
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .map_err(|e| OAuthError::Transport(e.to_string()))?;
    let status = resp.status().as_u16();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| OAuthError::Transport(e.to_string()))?;
    if !(200..300).contains(&status) {
        let msg = String::from_utf8_lossy(&bytes).to_string();
        return Err(OAuthError::Upstream(status, msg));
    }
    let parsed: TokenResponse =
        serde_json::from_slice(&bytes).map_err(|e| OAuthError::Decode(e.to_string()))?;
    Ok(OAuthTokens {
        access_token: parsed.access_token,
        refresh_token: parsed
            .refresh_token
            .or_else(|| Some(old_refresh_token.to_string())),
        expires_in: parsed.expires_in,
    })
}

/// In-memory ledger of pending OpenAI OAuth flows. Keyed by `state_id`.
#[derive(Default)]
pub struct OAuthSessionStore {
    inner: Mutex<HashMap<String, PkceCodes>>,
}

impl OAuthSessionStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, codes: PkceCodes) {
        let mut g = self.inner.lock().expect("oauth sessions mutex poisoned");
        if g.len() >= 32 {
            if let Some(stale) = g.keys().next().cloned() {
                g.remove(&stale);
            }
        }
        g.insert(codes.state.clone(), codes);
    }

    pub fn take(&self, state_id: &str) -> Option<PkceCodes> {
        let mut g = self.inner.lock().expect("oauth sessions mutex poisoned");
        g.remove(state_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_verifier_is_43_chars_or_more() {
        let c = generate_pkce();
        assert!(c.verifier.len() >= 43);
        assert!(c.verifier.len() <= 128);
    }

    #[test]
    fn pkce_challenge_is_distinct_from_verifier() {
        let c = generate_pkce();
        assert_ne!(c.verifier, c.challenge);
    }

    #[test]
    fn authorize_url_contains_required_params() {
        let c = generate_pkce();
        let url = build_authorize_url(&c);
        assert!(url.starts_with("https://auth.openai.com/authorize?"));
        assert!(url.contains(&format!("state={}", c.state)));
        assert!(url.contains(&format!("code_challenge={}", c.challenge)));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains(CLIENT_ID));
    }

    #[test]
    fn session_store_round_trip() {
        let s = OAuthSessionStore::new();
        let c = generate_pkce();
        let state = c.state.clone();
        s.insert(c);
        let taken = s.take(&state).unwrap();
        assert_eq!(taken.state, state);
        assert!(s.take(&state).is_none());
    }

    #[test]
    fn session_store_caps_at_32_entries() {
        let s = OAuthSessionStore::new();
        for _ in 0..40 {
            s.insert(generate_pkce());
        }
        let g = s.inner.lock().unwrap();
        assert!(g.len() <= 32);
    }
}
```

- [ ] **Step 2: Register the module**

In `crates/proxy/src/adapters/oauth/mod.rs`, update to:

```rust
//! OAuth adapters.

pub mod anthropic;
pub mod openai;

pub use anthropic::{
    OAuthError, OAuthSessionStore, OAuthTokens, PkceCodes, build_authorize_url, exchange_code,
    generate_pkce, refresh_token,
};
// OpenAI types stay namespaced — access via oauth::openai::*
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p proxy -- adapters::oauth::openai`
Expected: all 5 tests PASS

- [ ] **Step 4: Commit**

```
feat(proxy): add Codex OAuth module with PKCE browser flow
```

---

### Task 5: Update token refresh for OpenAiOAuth

**Files:**
- Modify: `crates/proxy/src/adapters/providers/token_refresh.rs`

- [ ] **Step 1: Update `refresh_expiring` to collect OpenAiOAuth providers**

In the `to_refresh` collection, the `filter_map` needs to also match `AuthConfig::OpenAiOAuth`. Change the collection to return `(String, String, OAuthKind)` — add a simple enum or use a tuple tag.

Add an enum at the top of the function or as a helper:

```rust
enum OAuthKind {
    Anthropic,
    OpenAi,
}
```

Update the `filter_map`:

```rust
        cfg.providers
            .iter()
            .filter_map(|p| match &p.auth {
                AuthConfig::AnthropicOAuth {
                    refresh_token,
                    expires_at_ms,
                    ..
                } if now_ms + 300_000 >= *expires_at_ms && !refresh_token.is_empty() => {
                    Some((p.name.clone(), refresh_token.clone(), OAuthKind::Anthropic))
                }
                AuthConfig::OpenAiOAuth {
                    refresh_token,
                    expires_at_ms,
                    ..
                } if now_ms + 300_000 >= *expires_at_ms && !refresh_token.is_empty() => {
                    Some((p.name.clone(), refresh_token.clone(), OAuthKind::OpenAi))
                }
                _ => None,
            })
            .collect()
```

- [ ] **Step 2: Dispatch refresh to correct OAuth module**

In the `for (name, old_rt) in to_refresh` loop (now `for (name, old_rt, kind) in to_refresh`), dispatch:

```rust
        let tokens = match kind {
            OAuthKind::Anthropic => {
                crate::adapters::oauth::anthropic::refresh_token(http, &old_rt)
                    .await
                    .map_err(|e| format!("refresh {name}: {e}"))?
            }
            OAuthKind::OpenAi => {
                crate::adapters::oauth::openai::refresh_token(http, &old_rt)
                    .await
                    .map_err(|e| format!("refresh {name}: {e}"))?
            }
        };
```

- [ ] **Step 3: Update config write-back for OpenAiOAuth**

After the tokens are received, the in-memory config update also needs to handle `OpenAiOAuth`. The `expires_at_ms` computation stays the same. Update the `prov.auth =` match:

```rust
            if let Some(prov) = cfg.providers.iter_mut().find(|p| p.name == name) {
                prov.auth = match kind {
                    OAuthKind::Anthropic => AuthConfig::AnthropicOAuth {
                        access_token: tokens.access_token.clone(),
                        refresh_token: tokens.refresh_token.clone().unwrap_or(old_rt),
                        expires_at_ms,
                    },
                    OAuthKind::OpenAi => AuthConfig::OpenAiOAuth {
                        access_token: tokens.access_token.clone(),
                        refresh_token: tokens.refresh_token.clone().unwrap_or(old_rt),
                        expires_at_ms,
                    },
                };
            }
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p proxy -- token_refresh`
Expected: existing tests still pass

- [ ] **Step 5: Commit**

```
feat(proxy): extend token refresh to handle OpenAiOAuth sessions
```

---

### Task 6: Add OpenAI OAuth use cases and admin routes

**Files:**
- Modify: `crates/proxy/src/application/use_cases/admin.rs`
- Modify: `crates/proxy/src/application/use_cases/mod.rs` (if needed)
- Modify: `crates/proxy/src/frameworks/admin.rs`

- [ ] **Step 1: Add use-case structs in `admin.rs`**

Add `StartOpenAiOAuth` and `CompleteOpenAiOAuth` after the existing Anthropic OAuth structs. They use `crate::adapters::oauth::openai::*` directly:

```rust
// ---- OAuth (OpenAI / Codex) ----

pub struct StartOpenAiOAuth {
    sessions: Arc<crate::adapters::oauth::openai::OAuthSessionStore>,
}

impl StartOpenAiOAuth {
    pub fn new(sessions: Arc<crate::adapters::oauth::openai::OAuthSessionStore>) -> Self {
        Self { sessions }
    }

    pub fn execute(&self) -> proxy_admin_api::StartOAuthResponse {
        let codes = crate::adapters::oauth::openai::generate_pkce();
        let url = crate::adapters::oauth::openai::build_authorize_url(&codes);
        let state_id = codes.state.clone();
        self.sessions.insert(codes);
        proxy_admin_api::StartOAuthResponse {
            authorization_url: url,
            state_id,
        }
    }
}

pub struct CompleteOpenAiOAuth {
    sessions: Arc<crate::adapters::oauth::openai::OAuthSessionStore>,
    http: reqwest::Client,
    config: Arc<RwLock<Config>>,
    config_path: PathBuf,
    live: Arc<crate::adapters::providers::LiveProvider>,
}

impl CompleteOpenAiOAuth {
    pub fn new(
        sessions: Arc<crate::adapters::oauth::openai::OAuthSessionStore>,
        http: reqwest::Client,
        config: Arc<RwLock<Config>>,
        config_path: PathBuf,
        live: Arc<crate::adapters::providers::LiveProvider>,
    ) -> Self {
        Self { sessions, http, config, config_path, live }
    }

    pub async fn execute(
        &self,
        req: proxy_admin_api::CompleteOAuthRequest,
    ) -> proxy_admin_api::CompleteOAuthResponse {
        let codes = match self.sessions.take(&req.state_id) {
            Some(c) => c,
            None => {
                return proxy_admin_api::CompleteOAuthResponse {
                    success: false,
                    error: Some("unknown or expired state_id".into()),
                    config: None,
                };
            }
        };
        let trimmed = req.code.trim();
        let code = trimmed.split('#').next().unwrap_or(trimmed);
        let tokens = match crate::adapters::oauth::openai::exchange_code(
            &self.http,
            code,
            &codes.state,
            &codes.verifier,
        )
        .await
        {
            Ok(t) => t,
            Err(e) => {
                return proxy_admin_api::CompleteOAuthResponse {
                    success: false,
                    error: Some(format!("{e}")),
                    config: None,
                };
            }
        };
        let (new_payload, new_cfg_clone) = {
            let mut cur = self.config.write().expect("config rwlock poisoned");
            let prov = match cur
                .providers
                .iter_mut()
                .find(|p| p.name == req.provider_name)
            {
                Some(p) => p,
                None => {
                    return proxy_admin_api::CompleteOAuthResponse {
                        success: false,
                        error: Some(format!(
                            "provider '{}' not in current config",
                            req.provider_name
                        )),
                        config: None,
                    };
                }
            };
            let expires_at_ms = {
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                let expires_in_ms = tokens.expires_in.unwrap_or(3600) * 1000;
                now_ms + expires_in_ms
            };
            prov.auth = AuthConfig::OpenAiOAuth {
                access_token: tokens.access_token.clone(),
                refresh_token: tokens.refresh_token.clone().unwrap_or_default(),
                expires_at_ms,
            };
            (config_to_payload(&cur), cur.clone())
        };
        let toml_str = match toml::to_string_pretty(&new_cfg_clone) {
            Ok(s) => s,
            Err(e) => {
                return proxy_admin_api::CompleteOAuthResponse {
                    success: false,
                    error: Some(format!("config serialize: {e}")),
                    config: None,
                };
            }
        };
        if let Some(parent) = self.config_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(&self.config_path, toml_str) {
            return proxy_admin_api::CompleteOAuthResponse {
                success: false,
                error: Some(format!("write config: {e}")),
                config: None,
            };
        }
        if let Err(e) = self.live.reload(&new_cfg_clone, self.http.clone()) {
            return proxy_admin_api::CompleteOAuthResponse {
                success: false,
                error: Some(format!("provider rebuild: {e}")),
                config: None,
            };
        }
        proxy_admin_api::CompleteOAuthResponse {
            success: true,
            error: None,
            config: Some(new_payload),
        }
    }
}
```

- [ ] **Step 2: Update `kind_to_str` and `str_to_kind`**

In the `kind_to_str` function:

```rust
        ProviderKind::OpenAi => "openai",
```

In the `str_to_kind` function:

```rust
        "openai" | "open_ai" => Ok(ProviderKind::OpenAi),
```

- [ ] **Step 3: Update `auth_to_payload` and `payload_to_auth`**

In `auth_to_payload`:

```rust
        AuthConfig::OpenAiOAuth {
            access_token,
            refresh_token,
            expires_at_ms,
        } => AuthPayload::OpenAiOAuth {
            access_token: access_token.clone(),
            refresh_token: refresh_token.clone(),
            expires_at_ms: *expires_at_ms,
        },
```

In `payload_to_auth`:

```rust
        AuthPayload::OpenAiOAuth {
            access_token,
            refresh_token,
            expires_at_ms,
        } => AuthConfig::OpenAiOAuth {
            access_token,
            refresh_token,
            expires_at_ms,
        },
```

- [ ] **Step 4: Update `use_cases/mod.rs` re-exports**

Add `StartOpenAiOAuth, CompleteOpenAiOAuth` to the pub use line.

- [ ] **Step 5: Update `AdminState` in `frameworks/admin.rs`**

Add fields:

```rust
    pub start_openai_oauth: Arc<StartOpenAiOAuth>,
    pub complete_openai_oauth: Arc<CompleteOpenAiOAuth>,
```

Add new routes in `build_admin_router`:

```rust
        .route("/admin/oauth/openai/start", post(openai_oauth_start_handler))
        .route(
            "/admin/oauth/openai/complete",
            post(openai_oauth_complete_handler),
        )
```

Add handlers:

```rust
async fn openai_oauth_start_handler(
    State(s): State<AdminState>,
    Json(_req): Json<StartOAuthRequest>,
) -> Json<StartOAuthResponse> {
    Json(s.start_openai_oauth.execute())
}

async fn openai_oauth_complete_handler(
    State(s): State<AdminState>,
    Json(req): Json<CompleteOAuthRequest>,
) -> Json<CompleteOAuthResponse> {
    Json(s.complete_openai_oauth.execute(req).await)
}
```

Update imports to include `StartOpenAiOAuth, CompleteOpenAiOAuth`.

- [ ] **Step 6: Wire up in `main.rs`**

In `main.rs`, where `AdminState` is constructed, add the OpenAI OAuth session store and use-case wiring:

```rust
    let openai_oauth_sessions = Arc::new(
        crate::adapters::oauth::openai::OAuthSessionStore::new()
    );
```

And in the `AdminState` construction:

```rust
    start_openai_oauth: Arc::new(StartOpenAiOAuth::new(openai_oauth_sessions)),
    complete_openai_oauth: Arc::new(CompleteOpenAiOAuth::new(
        openai_oauth_sessions,
        http.clone(),
        config.clone(),
        config_path.clone(),
        live.clone(),
    )),
```

- [ ] **Step 7: Run full workspace tests**

Run: `cargo test --workspace`
Expected: all PASS (proxy, proxy-admin-api, proxy-tui may have TUI-related match issues — fix any remaining non-exhaustive match arms)

- [ ] **Step 8: Commit**

```
feat(proxy): add OpenAI Codex OAuth use cases and admin API routes
```

---

### Task 7: Update proxy-tui for OpenAI OAuth

**Files:**
- Modify: `crates/proxy-tui/src/app.rs`
- Modify: `crates/proxy-tui/src/ui.rs`
- Modify: `crates/proxy-tui/src/client.rs`
- Modify: `crates/proxy-tui/src/main.rs`
- Modify: `crates/proxy-tui/src/wizard.rs` (if needed)

- [ ] **Step 1: Add `AuthInputKind::OAuthOpenAi`**

In `crates/proxy-tui/src/app.rs`, add to the `AuthInputKind` enum:

```rust
    OAuthOpenAi,
```

Update the `label()` method:

```rust
            AuthInputKind::OAuthOpenAi => "oauth (openai/codex)",
```

Update the `cycle()` method — after `OAuthAnthropic`:

```rust
            AuthInputKind::OAuthAnthropic => AuthInputKind::OAuthOpenAi,
            AuthInputKind::OAuthOpenAi => AuthInputKind::Passthrough,
```

Update `from_payload()`:

```rust
            AuthPayload::OpenAiOAuth { .. } => AuthInputKind::OAuthOpenAi,
```

Update the `visible_fields()` match — `OAuthOpenAi` behaves like `OAuthAnthropic`:

```rust
            AuthInputKind::Passthrough | AuthInputKind::OAuthAnthropic | AuthInputKind::OAuthOpenAi => &[
```

- [ ] **Step 2: Update `ui.rs` auth rendering**

Find the `AuthPayload` match that renders auth type label and add:

```rust
        AuthPayload::OpenAiOAuth { .. } => "openai_oauth".into(),
```

- [ ] **Step 3: Add client methods**

In `crates/proxy-tui/src/client.rs`, add:

```rust
    pub fn oauth_start_openai(&self, provider_name: &str) -> Result<StartOAuthResponse, ClientError> {
        let url = format!("{}/admin/oauth/openai/start", self.base_url);
        let body = StartOAuthRequest {
            provider_name: provider_name.into(),
        };
        let resp = ureq::Agent::new()
            .post(&url)
            .send_json(serde_json::to_value(body).unwrap())?;
        resp.into_json::<StartOAuthResponse>()
    }

    pub fn oauth_complete_openai(
        &self,
        state_id: &str,
        code: &str,
        provider_name: &str,
    ) -> Result<CompleteOAuthResponse, ClientError> {
        let url = format!("{}/admin/oauth/openai/complete", self.base_url);
        let body = CompleteOAuthRequest {
            state_id: state_id.into(),
            code: code.into(),
            provider_name: provider_name.into(),
        };
        let resp = ureq::Agent::new()
            .post(&url)
            .send_json(serde_json::to_value(body).unwrap())?;
        resp.into_json::<CompleteOAuthResponse>()
    }
```

- [ ] **Step 4: Update `main.rs` OAuth form submission**

In the submit functions, add handling for `OAuthOpenAi` that mirrors `OAuthAnthropic` but calls `client.oauth_start_openai()` and `client.oauth_complete_openai()`. Find the match on `m.auth_kind`:

```rust
            AuthInputKind::OAuthOpenAi => {
                match m.mode {
                    FormMode::Add => submit_oauth_add_openai(client, state, m),
                    FormMode::Edit { .. } => submit_oauth_edit_openai(client, state, m),
                }
            }
```

Create `submit_oauth_add_openai` and `submit_oauth_edit_openai` functions that mirror the Anthropic versions but call the OpenAI client methods.

- [ ] **Step 5: Update `wizard.rs`**

If the wizard has a match on `AuthInputKind`, add:

```rust
        crate::app::AuthInputKind::OAuthOpenAi => AuthPayload::Passthrough,
```

- [ ] **Step 6: Run workspace tests**

Run: `cargo test --workspace`
Expected: all PASS

- [ ] **Step 7: Commit**

```
feat(tui): add OpenAI Codex OAuth option to provider form
```

---

### Task 8: Run full verification

- [ ] **Step 1: Run all tests**

Run: `cargo test --workspace`
Expected: all PASS

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --workspace -- -D warnings`
Expected: no warnings

- [ ] **Step 3: Build release**

Run: `cargo build --release --workspace`
Expected: clean build

- [ ] **Step 4: Final commit (if any fixups needed)**

```
chore: fix clippy warnings from OpenAI provider integration
```
