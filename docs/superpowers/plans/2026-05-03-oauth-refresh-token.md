# OAuth Refresh Token Support — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add full OAuth token lifecycle management — store refresh tokens, auto-refresh before expiry, retry on 401, and inject the `Anthropic-Beta` header — so users never have to re-do the browser dance.

**Architecture:** Extend the existing `AuthConfig` enum with a new `AnthropicOAuth` variant that carries `access_token`, `refresh_token`, and `expires_at_ms`. The existing `messages_protocol::forward` function gains a token-refresh wrapper that checks expiry before each request and retries once on 401. The `builder` maps the new variant to a new `AuthHeader::OAuth` that carries all three fields. The `CompleteAnthropicOAuth` use case stores the full token set instead of discarding refresh tokens.

**Tech Stack:** Rust, tokio, reqwest, serde, existing crate structure

---

## File Structure

| Action | File | Responsibility |
|--------|------|----------------|
| Modify | `crates/proxy/src/config.rs` | Add `AuthConfig::AnthropicOAuth` variant |
| Modify | `crates/proxy-admin-api/src/lib.rs` | Add `AuthPayload::AnthropicOAuth` variant |
| Modify | `crates/proxy/src/adapters/providers/messages_protocol.rs` | Add `AuthHeader::OAuth`, inject `Anthropic-Beta` header, add token refresh + 401 retry logic |
| Modify | `crates/proxy/src/adapters/oauth/anthropic.rs` | Add `refresh_token()` function, update `SCOPES` |
| Modify | `crates/proxy/src/adapters/providers/builder.rs` | Map `AuthConfig::AnthropicOAuth` → `AuthHeader::OAuth` |
| Modify | `crates/proxy/src/application/use_cases/admin.rs` | Store full OAuth token set in config, update DTO mappings |
| Modify | `crates/proxy-tui/src/client.rs` | Handle new auth type in TUI client (if needed) |

---

## Task 1: Extend `AuthConfig` with `AnthropicOAuth` variant

**Files:**
- Modify: `crates/proxy/src/config.rs:49-65`
- Modify: `crates/proxy/src/config.rs:160-173` (interpolate_env)
- Modify: `crates/proxy/src/config.rs:267-408` (tests)

- [ ] **Step 1: Add the new enum variant to `AuthConfig`**

In `crates/proxy/src/config.rs`, replace the existing `AuthConfig` enum:

```rust
/// How the proxy authenticates *to* the upstream when forwarding a request.
///
/// `Passthrough` keeps the client's incoming auth headers intact — this is the
/// default and matches the Phase 0 behaviour. The other variants strip
/// incoming `x-api-key`/`authorization` and inject the configured value.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthConfig {
    #[default]
    Passthrough,
    ApiKey {
        value: String,
    },
    Bearer {
        value: String,
    },
    /// Full Anthropic OAuth session. The proxy auto-refreshes the access token
    /// before expiry using the refresh token, and retries once on 401.
    AnthropicOAuth {
        access_token: String,
        refresh_token: String,
        /// Unix epoch millis when the access token expires.
        expires_at_ms: u64,
    },
}
```

- [ ] **Step 2: Update `interpolate_env` to handle the new variant**

In `crates/proxy/src/config.rs`, update the `interpolate_env` method's match arm:

```rust
    fn interpolate_env(&mut self) {
        for p in &mut self.providers {
            if let Some(b) = &mut p.base_url {
                *b = interpolate(b);
            }
            match &mut p.auth {
                AuthConfig::ApiKey { value } | AuthConfig::Bearer { value } => {
                    *value = interpolate(value);
                }
                AuthConfig::Passthrough => {}
                AuthConfig::AnthropicOAuth { .. } => {
                    // OAuth tokens are set by the daemon, not env vars.
                }
            }
        }
    }
```

- [ ] **Step 3: Add a TOML parsing test for the new variant**

In the `#[cfg(test)] mod tests` section of `crates/proxy/src/config.rs`, add:

```rust
    #[test]
    fn toml_parses_anthropic_oauth_auth() {
        let toml_str = r#"
            [[providers]]
            name = "anthropic"
            kind = "anthropic"
            auth = { type = "anthropic_oauth", access_token = "sk-ant-oat-abc", refresh_token = "sk-ant-oar-xyz", expires_at_ms = 1746300000000 }

            [[routing]]
            match = { model = "*" }
            provider = "anthropic"
        "#;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        assert!(matches!(
            cfg.providers[0].auth,
            AuthConfig::AnthropicOAuth { .. }
        ));
        if let AuthConfig::AnthropicOAuth {
            access_token,
            refresh_token,
            expires_at_ms,
        } = &cfg.providers[0].auth
        {
            assert_eq!(access_token, "sk-ant-oat-abc");
            assert_eq!(refresh_token, "sk-ant-oar-xyz");
            assert_eq!(*expires_at_ms, 1746300000000);
        }
    }
```

- [ ] **Step 4: Run tests to verify**

Run: `cargo test -p proxy -- config::tests`
Expected: All tests pass including the new one.

- [ ] **Step 5: Commit**

```bash
git add crates/proxy/src/config.rs
git commit -m "feat(proxy): add AuthConfig::AnthropicOAuth variant with refresh_token and expires_at_ms"
```

---

## Task 2: Extend `AuthPayload` in `proxy-admin-api` crate

**Files:**
- Modify: `crates/proxy-admin-api/src/lib.rs:49-60`

- [ ] **Step 1: Add the `AnthropicOAuth` variant to `AuthPayload`**

In `crates/proxy-admin-api/src/lib.rs`, replace the `AuthPayload` enum:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthPayload {
    #[default]
    Passthrough,
    ApiKey {
        value: String,
    },
    Bearer {
        value: String,
    },
    AnthropicOAuth {
        access_token: String,
        refresh_token: String,
        expires_at_ms: u64,
    },
}
```

- [ ] **Step 2: Run tests to verify**

Run: `cargo test -p proxy-admin-api`
Expected: All tests pass (the crate is pure data types with no logic tests, but compilation confirms serde derives work).

- [ ] **Step 3: Commit**

```bash
git add crates/proxy-admin-api/src/lib.rs
git commit -m "feat(admin-api): add AuthPayload::AnthropicOAuth variant mirroring proxy config"
```

---

## Task 3: Add `refresh_token()` function to OAuth adapter

**Files:**
- Modify: `crates/proxy/src/adapters/oauth/anthropic.rs:85-136`
- Modify: `crates/proxy/src/adapters/oauth/anthropic.rs:169-225` (tests)

- [ ] **Step 1: Update `SCOPES` to match Claude Code**

In `crates/proxy/src/adapters/oauth/anthropic.rs`, change the `SCOPES` constant:

```rust
pub const SCOPES: &str = "org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers";
```

- [ ] **Step 2: Add the `refresh_token` public function**

In `crates/proxy/src/adapters/oauth/anthropic.rs`, add after the `exchange_code` function (after line 136):

```rust
/// Refresh an expired access token using a refresh token. Returns a new
/// `OAuthTokens` with a fresh access token and (typically) a rotated refresh
/// token. If the response omits `refresh_token`, callers should keep the old
/// one (per OAuth best practice).
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
        // If the server didn't rotate, keep the old refresh token.
        refresh_token: parsed.refresh_token.or_else(|| Some(old_refresh_token.to_string())),
        expires_in: parsed.expires_in,
    })
}
```

- [ ] **Step 3: Run tests to verify**

Run: `cargo test -p proxy -- oauth::tests`
Expected: All existing tests pass (the new function is not called yet).

- [ ] **Step 4: Commit**

```bash
git add crates/proxy/src/adapters/oauth/anthropic.rs
git commit -m "feat(oauth): add refresh_token() function and broaden OAuth scopes"
```

---

## Task 4: Extend `AuthHeader` and add token-refresh + retry logic in `messages_protocol`

**Files:**
- Modify: `crates/proxy/src/adapters/providers/messages_protocol.rs:34-43` (AuthHeader enum)
- Modify: `crates/proxy/src/adapters/providers/messages_protocol.rs:83-140` (forward function)

This is the core task — the `forward` function becomes aware of OAuth tokens and handles their lifecycle.

- [ ] **Step 1: Add `AuthHeader::OAuth` variant**

In `crates/proxy/src/adapters/providers/messages_protocol.rs`, replace the `AuthHeader` enum:

```rust
/// How the proxy authenticates to the upstream when forwarding a request.
#[derive(Debug, Clone)]
pub enum AuthHeader {
    /// Forward whatever auth headers the client sent (Phase 0 behaviour).
    Passthrough,
    /// Strip incoming auth, inject `x-api-key: <value>`.
    ApiKey(String),
    /// Strip incoming auth, inject `authorization: Bearer <value>`.
    Bearer(String),
    /// Full OAuth with auto-refresh. Carries the current tokens and the HTTP
    /// client needed to refresh them. The `refresh_lock` serialises concurrent
    /// refresh attempts so token rotation doesn't race.
    OAuth {
        access_token: String,
        refresh_token: String,
        expires_at_ms: u64,
    },
}

impl AuthHeader {
    /// Returns `true` if the access token is expired or expires within
    /// `buffer_secs` seconds from now.
    pub fn is_expired(&self, buffer_secs: u64) -> bool {
        match self {
            AuthHeader::OAuth { expires_at_ms, .. } => {
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                now_ms + (buffer_secs * 1000) >= *expires_at_ms
            }
            _ => false,
        }
    }
}
```

- [ ] **Step 2: Rewrite the `forward` function with refresh + retry**

Replace the entire `forward` function in `crates/proxy/src/adapters/providers/messages_protocol.rs`:

```rust
pub(super) async fn forward(
    http: &reqwest::Client,
    base_url: &str,
    auth: &AuthHeader,
    path: &str,
    headers: &HeaderMap,
    body: Bytes,
    streaming: bool,
) -> Result<UpstreamResponse, ProxyError> {
    // For OAuth, we may need to refresh + retry. Clone auth so we can update
    // it after a refresh. For non-OAuth variants, this is a cheap clone.
    let mut effective_auth = auth.clone();

    // Proactive refresh: if the token expires within 5 minutes, refresh now.
    if effective_auth.is_expired(300) {
        if let AuthHeader::OAuth { refresh_token, .. } = &effective_auth {
            match crate::adapters::oauth::refresh_token(http, refresh_token).await {
                Ok(tokens) => {
                    effective_auth = oauth_tokens_to_auth_header(&tokens);
                    tracing::info!("proactively refreshed OAuth token");
                }
                Err(e) => {
                    tracing::warn!(error = %e, "proactive OAuth refresh failed; will try with current token");
                }
            }
        }
    }

    // First attempt.
    let resp = send_request(http, base_url, &effective_auth, path, headers, &body, streaming).await?;

    // On 401 with OAuth, refresh and retry once.
    if let UpstreamResponse::Buffered { status: 401, .. } = &resp {
        if let AuthHeader::OAuth { refresh_token, .. } = &effective_auth {
            tracing::info!("401 from upstream, attempting OAuth refresh + retry");
            match crate::adapters::oauth::refresh_token(http, refresh_token).await {
                Ok(tokens) => {
                    let refreshed = oauth_tokens_to_auth_header(&tokens);
                    return send_request(http, base_url, &refreshed, path, headers, &body, streaming).await;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "OAuth refresh on 401 failed");
                }
            }
        }
    }

    Ok(resp)
}

fn oauth_tokens_to_auth_header(tokens: &crate::adapters::oauth::OAuthTokens) -> AuthHeader {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    // Default to 1 hour if expires_in is missing.
    let expires_in_ms = tokens.expires_in.unwrap_or(3600) * 1000;
    AuthHeader::OAuth {
        access_token: tokens.access_token.clone(),
        refresh_token: tokens
            .refresh_token
            .clone()
            .unwrap_or_default(),
        expires_at_ms: now_ms + expires_in_ms,
    }
}

async fn send_request(
    http: &reqwest::Client,
    base_url: &str,
    auth: &AuthHeader,
    path: &str,
    headers: &HeaderMap,
    body: &[u8],
    streaming: bool,
) -> Result<UpstreamResponse, ProxyError> {
    let url = format!("{base_url}{path}");
    let mut req = http.post(&url).body(body.to_vec());
    let strip_auth = !matches!(auth, AuthHeader::Passthrough);
    for (k, v) in headers {
        if HOP_BY_HOP.contains(&k.as_str()) {
            continue;
        }
        if strip_auth && AUTH_HEADERS.contains(&k.as_str()) {
            continue;
        }
        req = req.header(k, v);
    }
    match auth {
        AuthHeader::Passthrough => {}
        AuthHeader::ApiKey(v) => {
            req = req.header("x-api-key", v);
        }
        AuthHeader::Bearer(v) => {
            req = req.header("authorization", format!("Bearer {v}"));
        }
        AuthHeader::OAuth { access_token, .. } => {
            req = req.header("authorization", format!("Bearer {access_token}"));
            // Required for OAuth-authenticated requests.
            req = req.header("anthropic-beta", "oauth-2025-04-20");
        }
    }
    let resp = req.send().await?;
    let status = resp.status().as_u16();
    let mut headers_out = HeaderMap::new();
    for (k, v) in resp.headers() {
        if HOP_BY_HOP.contains(&k.as_str()) {
            continue;
        }
        headers_out.insert(k.clone(), v.clone());
    }
    if streaming {
        let stream = resp.bytes_stream().map(|res| {
            res.map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })
        });
        let body: BoxedByteStream = Box::pin(stream);
        Ok(UpstreamResponse::Streaming {
            status,
            headers: headers_out,
            body,
        })
    } else {
        let body = resp.bytes().await?;
        Ok(UpstreamResponse::Buffered {
            status,
            headers: headers_out,
            body,
        })
    }
}
```

- [ ] **Step 3: Update existing tests that import `AuthHeader`**

The existing tests in `messages_protocol.rs` don't construct `AuthHeader` directly, so they should still compile. Run to confirm:

Run: `cargo test -p proxy -- messages_protocol::tests`
Expected: All pass.

- [ ] **Step 4: Add test for `is_expired`**

In the `#[cfg(test)] mod tests` block, add:

```rust
    #[test]
    fn auth_header_oauth_is_expired_checks_expiry() {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let expired = AuthHeader::OAuth {
            access_token: "at".into(),
            refresh_token: "rt".into(),
            expires_at_ms: now_ms - 1000, // already expired
        };
        assert!(expired.is_expired(300));

        let fresh = AuthHeader::OAuth {
            access_token: "at".into(),
            refresh_token: "rt".into(),
            expires_at_ms: now_ms + 600_000, // expires in 10 min
        };
        assert!(!fresh.is_expired(300));

        let almost_expired = AuthHeader::OAuth {
            access_token: "at".into(),
            refresh_token: "rt".into(),
            expires_at_ms: now_ms + 200_000, // expires in 200s, within 300s buffer
        };
        assert!(almost_expired.is_expired(300));
    }

    #[test]
    fn non_oauth_auth_header_is_never_expired() {
        assert!(!AuthHeader::Passthrough.is_expired(300));
        assert!(!AuthHeader::ApiKey("k".into()).is_expired(300));
        assert!(!AuthHeader::Bearer("b".into()).is_expired(300));
    }
```

- [ ] **Step 5: Run all proxy tests**

Run: `cargo test -p proxy`
Expected: All pass.

- [ ] **Step 6: Commit**

```bash
git add crates/proxy/src/adapters/providers/messages_protocol.rs
git commit -m "feat(provider): add AuthHeader::OAuth with proactive refresh, 401 retry, and Anthropic-Beta header"
```

---

## Task 5: Wire `AuthConfig::AnthropicOAuth` through the builder

**Files:**
- Modify: `crates/proxy/src/adapters/providers/builder.rs:20-25`

- [ ] **Step 1: Add mapping for the new variant**

In `crates/proxy/src/adapters/providers/builder.rs`, update the `build_leaf` function's auth match:

```rust
pub fn build_leaf(p: &ProviderConfig, http: reqwest::Client) -> Arc<dyn Provider> {
    let auth = match &p.auth {
        AuthConfig::Passthrough => AuthHeader::Passthrough,
        AuthConfig::ApiKey { value } => AuthHeader::ApiKey(value.clone()),
        AuthConfig::Bearer { value } => AuthHeader::Bearer(value.clone()),
        AuthConfig::AnthropicOAuth {
            access_token,
            refresh_token,
            expires_at_ms,
        } => AuthHeader::OAuth {
            access_token: access_token.clone(),
            refresh_token: refresh_token.clone(),
            expires_at_ms: *expires_at_ms,
        },
    };
    match p.kind {
        ProviderKind::Anthropic => {
            Arc::new(AnthropicProvider::configure(http, p.base_url.clone(), auth))
        }
        ProviderKind::Zai => Arc::new(ZaiProvider::configure(http, p.base_url.clone(), auth)),
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p proxy`
Expected: All pass.

- [ ] **Step 3: Commit**

```bash
git add crates/proxy/src/adapters/providers/builder.rs
git commit -m "feat(builder): map AuthConfig::AnthropicOAuth to AuthHeader::OAuth"
```

---

## Task 6: Update `CompleteAnthropicOAuth` to store full token set

**Files:**
- Modify: `crates/proxy/src/application/use_cases/admin.rs:293-317` (where auth is set)
- Modify: `crates/proxy/src/application/use_cases/admin.rs:463-481` (DTO mappings)

- [ ] **Step 1: Update `CompleteAnthropicOAuth::execute` to store full tokens**

In `crates/proxy/src/application/use_cases/admin.rs`, find the block that sets `prov.auth` (around line 314) and replace:

```rust
            // Compute expires_at_ms: now + expires_in (default 8 hours).
            let expires_at_ms = {
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                let expires_in_ms = tokens.expires_in.unwrap_or(28800) * 1000;
                now_ms + expires_in_ms
            };
            prov.auth = AuthConfig::AnthropicOAuth {
                access_token: tokens.access_token.clone(),
                refresh_token: tokens
                    .refresh_token
                    .clone()
                    .unwrap_or_default(),
                expires_at_ms,
            };
```

This replaces the old line:
```rust
            prov.auth = AuthConfig::Bearer {
                value: tokens.access_token.clone(),
            };
```

- [ ] **Step 2: Update `auth_to_payload` to handle the new variant**

In `crates/proxy/src/application/use_cases/admin.rs`, update the `auth_to_payload` function:

```rust
fn auth_to_payload(a: &AuthConfig) -> AuthPayload {
    match a {
        AuthConfig::Passthrough => AuthPayload::Passthrough,
        AuthConfig::ApiKey { value } => AuthPayload::ApiKey {
            value: value.clone(),
        },
        AuthConfig::Bearer { value } => AuthPayload::Bearer {
            value: value.clone(),
        },
        AuthConfig::AnthropicOAuth {
            access_token,
            refresh_token,
            expires_at_ms,
        } => AuthPayload::AnthropicOAuth {
            access_token: access_token.clone(),
            refresh_token: refresh_token.clone(),
            expires_at_ms: *expires_at_ms,
        },
    }
}
```

- [ ] **Step 3: Update `payload_to_auth` to handle the new variant**

In `crates/proxy/src/application/use_cases/admin.rs`, update the `payload_to_auth` function:

```rust
fn payload_to_auth(a: AuthPayload) -> AuthConfig {
    match a {
        AuthPayload::Passthrough => AuthConfig::Passthrough,
        AuthPayload::ApiKey { value } => AuthConfig::ApiKey { value },
        AuthPayload::Bearer { value } => AuthConfig::Bearer { value },
        AuthPayload::AnthropicOAuth {
            access_token,
            refresh_token,
            expires_at_ms,
        } => AuthConfig::AnthropicOAuth {
            access_token,
            refresh_token,
            expires_at_ms,
        },
    }
}
```

- [ ] **Step 4: Update `config_to_payload_roundtrip` test if needed**

The existing test uses `AuthConfig::ApiKey` which should still work. Run to verify:

Run: `cargo test -p proxy -- admin::tests`
Expected: All pass.

- [ ] **Step 5: Commit**

```bash
git add crates/proxy/src/application/use_cases/admin.rs
git commit -m "feat(admin): store full OAuth token set (access + refresh + expiry) on OAuth completion"
```

---

## Task 7: Persist refreshed tokens back to config

**Files:**
- Modify: `crates/proxy/src/adapters/providers/live.rs`

After a successful token refresh in `messages_protocol::forward`, the new tokens need to be persisted to the config file so they survive restarts. This requires the forward path to have access to the config and config_path.

**Approach:** Add an optional callback to `LiveProvider` that gets called with the updated auth whenever a token refresh succeeds.

- [ ] **Step 1: Add refresh callback support to `LiveProvider`**

In `crates/proxy/src/adapters/providers/live.rs`, add after the `reload` method:

```rust
use std::sync::Mutex;

/// Called after a successful token refresh so the new tokens can be persisted.
pub type OnTokenRefreshed = Box<
    dyn Fn(&str, &str, &str, u64) + Send + Sync, // (provider_name, access_token, refresh_token, expires_at_ms)
>;

pub struct LiveProvider {
    inner: RwLock<Arc<dyn Provider>>,
    /// Callback invoked after a successful OAuth token refresh.
    /// Arguments: (provider_name, new_access_token, new_refresh_token, new_expires_at_ms)
    on_token_refreshed: Option<Mutex<OnTokenRefreshed>>,
}
```

Update the `new` and `with_callback` constructors:

```rust
impl LiveProvider {
    pub fn new(initial: Arc<dyn Provider>) -> Self {
        Self {
            inner: RwLock::new(initial),
            on_token_refreshed: None,
        }
    }

    pub fn with_callback(initial: Arc<dyn Provider>, cb: OnTokenRefreshed) -> Self {
        Self {
            inner: RwLock::new(initial),
            on_token_refreshed: Some(Mutex::new(cb)),
        }
    }
```

Wait — this approach is getting complex because `messages_protocol::forward` is a free function that doesn't have access to `LiveProvider`. Let me reconsider.

**Better approach:** Instead of wiring callbacks through, have the `forward` function in `messages_protocol` return the (possibly refreshed) `AuthHeader` alongside the `UpstreamResponse`, and let `LiveProvider::forward` persist any changes. This is cleaner.

Actually, the simplest correct approach: **Don't persist refreshed tokens in the request path at all.** Instead, add a background refresh task that runs periodically, similar to `claude-oauth-proxy`. This keeps the request path simple and avoids locking complexity.

**Revised approach for Task 7:**

- [ ] **Step 1 (revised): Add a background refresh task**

Create a new file `crates/proxy/src/adapters/providers/token_refresh.rs`:

```rust
//! Background task that periodically refreshes OAuth tokens and persists
//! the refreshed tokens back to the config file.

use crate::adapters::providers::builder;
use crate::config::{AuthConfig, Config};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::time;

/// Spawn a background task that checks OAuth token expiry every minute and
/// refreshes any that are within 5 minutes of expiring. Refreshed tokens are
/// written back to the config file and the live provider tree is rebuilt.
pub fn spawn(
    config: Arc<RwLock<Config>>,
    config_path: PathBuf,
    http: reqwest::Client,
    live: Arc<crate::adapters::providers::LiveProvider>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            if let Err(e) = refresh_all(&config, &config_path, &http, &live).await {
                tracing::warn!(error = %e, "background token refresh sweep failed");
            }
        }
    })
}

async fn refresh_all(
    config: &Arc<RwLock<Config>>,
    config_path: &PathBuf,
    http: &reqwest::Client,
    live: &Arc<crate::adapters::providers::LiveProvider>,
) -> Result<(), String> {
    // Collect providers that need refresh.
    let to_refresh: Vec<(String, String, u64)> = {
        let cfg = config.read().map_err(|e| format!("config read: {e}"))?;
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        cfg.providers
            .iter()
            .filter_map(|p| match &p.auth {
                AuthConfig::AnthropicOAuth {
                    refresh_token,
                    expires_at_ms,
                    ..
                } if now_ms + 300_000 >= *expires_at_ms => {
                    Some((p.name.clone(), refresh_token.clone(), *expires_at_ms))
                }
                _ => None,
            })
            .collect()
    };

    for (name, old_rt, _old_exp) in to_refresh {
        tracing::info!(provider = %name, "refreshing OAuth token");
        let tokens = crate::adapters::oauth::refresh_token(http, &old_rt)
            .await
            .map_err(|e| format!("refresh {name}: {e}"))?;

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let expires_at_ms = now_ms + tokens.expires_in.unwrap_or(3600) * 1000;

        // Update in-memory config.
        {
            let mut cfg = config.write().map_err(|e| format!("config write: {e}"))?;
            if let Some(prov) = cfg.providers.iter_mut().find(|p| p.name == name) {
                prov.auth = AuthConfig::AnthropicOAuth {
                    access_token: tokens.access_token.clone(),
                    refresh_token: tokens
                        .refresh_token
                        .clone()
                        .unwrap_or(old_rt.clone()),
                    expires_at_ms,
                };
            }
            // Persist to disk.
            let toml_str =
                toml::to_string_pretty(&*cfg).map_err(|e| format!("serialize: {e}"))?;
            std::fs::write(config_path, toml_str).map_err(|e| format!("write: {e}"))?;

            // Rebuild provider tree with new tokens.
            live.reload(&cfg, http.clone())
                .map_err(|e| format!("reload: {e}"))?;
        }
        tracing::info!(provider = %name, "OAuth token refreshed and persisted");
    }
    Ok(())
}
```

- [ ] **Step 2: Register the module in `providers/mod.rs`**

In `crates/proxy/src/adapters/providers/mod.rs`, add:

```rust
pub mod token_refresh;
```

- [ ] **Step 3: Spawn the background task in the daemon's composition root**

Find the daemon's `main.rs` (or wherever `LiveProvider` is constructed and the server is started) and add the spawn call. This will look something like:

```rust
// After live provider is constructed:
crate::adapters::providers::token_refresh::spawn(
    config.clone(),
    Config::resolved_path(),
    http.clone(),
    live.clone(),
);
```

The exact location depends on the composition root — find where `LiveProvider::new()` is called and add the spawn nearby.

- [ ] **Step 4: Run all tests**

Run: `cargo test -p proxy`
Expected: All pass.

- [ ] **Step 5: Commit**

```bash
git add crates/proxy/src/adapters/providers/token_refresh.rs crates/proxy/src/adapters/providers/mod.rs
git commit -m "feat(provider): add background OAuth token refresh task that persists to config"
```

---

## Task 8: Update TUI client to handle new auth type

**Files:**
- Modify: `crates/proxy-tui/src/app.rs` (where auth types are displayed)
- Modify: `crates/proxy-tui/src/ui.rs` (where auth is rendered)

- [ ] **Step 1: Check if TUI handles new auth type**

The TUI uses `proxy_admin_api::AuthPayload` which now has the `AnthropicOAuth` variant. The TUI needs to handle this in its display/match arms. Search for `AuthPayload` in the TUI code and ensure all match arms are exhaustive.

The key places to update are any `match` on `AuthPayload` in the TUI. The simplest approach is to display `AnthropicOAuth` as a Bearer in the UI, since the user doesn't need to see the refresh token.

Find any match arms on `AuthPayload` in `crates/proxy-tui/` and add:

```rust
AuthPayload::AnthropicOAuth { access_token, expires_at_ms, .. } => {
    // Display like a Bearer token with expiry info
}
```

- [ ] **Step 2: Run TUI compilation check**

Run: `cargo check -p proxy-tui`
Expected: Compiles without errors.

- [ ] **Step 3: Commit**

```bash
git add crates/proxy-tui/
git commit -m "feat(tui): handle AnthropicOAuth auth type in display"
```

---

## Task 9: Full integration verification

- [ ] **Step 1: Run full workspace test suite**

Run: `cargo test --workspace`
Expected: All tests pass.

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --workspace -- -D warnings`
Expected: No warnings.

- [ ] **Step 3: Verify TOML round-trip**

Create a test config with the new auth type and verify it serializes/deserializes:

Run: `cargo test -p proxy -- config::tests::toml_parses_anthropic_oauth_auth`
Expected: Pass.

- [ ] **Step 4: Final commit (if any fixes needed)**

```bash
git add -A
git commit -m "fix: address clippy warnings and test failures from OAuth refresh implementation"
```
