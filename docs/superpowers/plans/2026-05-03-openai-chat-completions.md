# OpenAI Chat Completions Endpoint Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `/v1/chat/completions` route to the proxy so OpenAI-compatible clients (OpenCode, Cursor) can use it.

**Architecture:** Add `forward_openai()` to the `Provider` trait (default returns error). `ZaiProvider` overrides it to forward to Z.AI's OpenAI endpoint. New handler mirrors existing `messages` handler. `HandleMessages` use case accepts an `api_format` parameter to choose which `forward_*` to call.

**Tech Stack:** Rust, axum, async_trait, serde_json

---

## File Structure

| File | Change | Responsibility |
|------|--------|----------------|
| `crates/proxy/src/config.rs` | Modify | Add `openai_base_url: Option<String>` to `ProviderConfig` |
| `crates/proxy/src/application/ports/provider.rs` | Modify | Add `forward_openai()` to `Provider` trait |
| `crates/proxy/src/adapters/providers/messages_protocol.rs` | Modify | Add `forward_openai()` free function |
| `crates/proxy/src/adapters/providers/zai.rs` | Modify | Add `openai_base_url` field, implement `forward_openai()` |
| `crates/proxy/src/adapters/providers/anthropic.rs` | Modify | Use default `forward_openai()` (returns error) |
| `crates/proxy/src/adapters/providers/routing.rs` | Modify | Implement `forward_openai()` for `RoutingProvider` |
| `crates/proxy/src/adapters/providers/live.rs` | Modify | Delegate `forward_openai()` |
| `crates/proxy/src/adapters/providers/builder.rs` | Modify | Pass `openai_base_url` to `ZaiProvider` |
| `crates/proxy/src/application/use_cases/handle_messages.rs` | Modify | Add `ApiFormat` enum, use it to choose `forward` vs `forward_openai` |
| `crates/proxy/src/frameworks/handler.rs` | Modify | Add `chat_completions` handler |
| `crates/proxy/src/frameworks/server.rs` | Modify | Add `/v1/chat/completions` route |

---

### Task 1: Add `forward_openai` to the `Provider` trait

**Files:**
- Modify: `crates/proxy/src/application/ports/provider.rs`

- [ ] **Step 1: Write the failing test**

Add a test that verifies a provider without OpenAI support returns an error when `forward_openai` is called. Add this to `crates/proxy/src/adapters/providers/anthropic.rs` tests:

```rust
#[tokio::test]
async fn forward_openai_returns_error() {
    let p = AnthropicProvider::new(reqwest::Client::new());
    let result = p
        .forward_openai(
            "/v1/chat/completions",
            &HeaderMap::new(),
            Bytes::from_static(br#"{"model":"test"}"#),
            false,
        )
        .await;
    match result {
        Err(ProxyError::BadRequest(msg)) => {
            assert!(msg.contains("does not support OpenAI"));
        }
        other => panic!("expected BadRequest, got {:?}", other),
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p proxy -- anthropic::tests::forward_openai_returns_error`
Expected: compile error — `forward_openai` does not exist on `Provider`

- [ ] **Step 3: Add `forward_openai` to the `Provider` trait**

In `crates/proxy/src/application/ports/provider.rs`, add to the trait after the existing `forward` method:

```rust
    /// Forward an OpenAI-format request (e.g. `/v1/chat/completions`).
    /// Default implementation returns an error — only providers with an
    /// OpenAI-compatible endpoint should override this.
    async fn forward_openai(
        &self,
        path: &str,
        headers: &HeaderMap,
        body: Bytes,
        streaming: bool,
    ) -> Result<UpstreamResponse, ProxyError> {
        let _ = (path, headers, body, streaming);
        Err(ProxyError::BadRequest(format!(
            "provider '{}' does not support OpenAI chat completions format",
            self.name()
        )))
    }
```

Also add the missing imports at the top of the file. Current imports:

```rust
use crate::application::errors::ProxyError;
use crate::application::ports::{UpstreamResponse, UsageParser};
use crate::domain::UsageRecord;
use async_trait::async_trait;
use bytes::Bytes;
use http::HeaderMap;
```

No changes needed — `UpstreamResponse` and `ProxyError` are already imported.

- [ ] **Step 4: Run all proxy tests**

Run: `cargo test -p proxy`
Expected: All tests pass. `AnthropicProvider` inherits the default `forward_openai` which returns error.

- [ ] **Step 5: Commit**

```bash
git add crates/proxy/src/application/ports/provider.rs crates/proxy/src/adapters/providers/anthropic.rs
git commit -m "feat(proxy): add forward_openai to Provider trait with default error"
```

---

### Task 2: Add `openai_base_url` to `ProviderConfig` and `ZaiProvider`

**Files:**
- Modify: `crates/proxy/src/config.rs`
- Modify: `crates/proxy/src/adapters/providers/zai.rs`
- Modify: `crates/proxy/src/adapters/providers/builder.rs`

- [ ] **Step 1: Add `openai_base_url` to `ProviderConfig`**

In `crates/proxy/src/config.rs`, add the field to `ProviderConfig` (after `base_url` at line 39):

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub name: String,
    pub kind: ProviderKind,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub openai_base_url: Option<String>,
}
```

- [ ] **Step 2: Add `openai_base_url` field to `ZaiProvider`**

In `crates/proxy/src/adapters/providers/zai.rs`, update the struct and constructors:

```rust
pub struct ZaiProvider {
    base_url: String,
    openai_base_url: Option<String>,
    http: reqwest::Client,
    auth: AuthHeader,
}
```

Update `new()`:

```rust
    pub fn new(http: reqwest::Client) -> Self {
        Self::build(
            http,
            "https://api.z.ai/api/anthropic".into(),
            Some("https://api.z.ai/api/paas/v4".into()),
            AuthHeader::Passthrough,
        )
    }
```

Update `with_base_url()`:

```rust
    pub fn with_base_url(http: reqwest::Client, base_url: impl Into<String>) -> Self {
        Self::build(http, base_url.into(), None, AuthHeader::Passthrough)
    }
```

Update `with_auth()`:

```rust
    pub fn with_auth(http: reqwest::Client, auth: AuthHeader) -> Self {
        Self::build(
            http,
            "https://api.z.ai/api/anthropic".into(),
            Some("https://api.z.ai/api/paas/v4".into()),
            auth,
        )
    }
```

Update `configure()` — this is the one called from the builder:

```rust
    pub fn configure(
        http: reqwest::Client,
        base_url: Option<String>,
        openai_base_url: Option<String>,
        auth: AuthHeader,
    ) -> Self {
        Self::build(
            http,
            base_url.unwrap_or_else(|| "https://api.z.ai/api/anthropic".into()),
            Some(
                openai_base_url
                    .unwrap_or_else(|| "https://api.z.ai/api/paas/v4".into()),
            ),
            auth,
        )
    }
```

Update `build()`:

```rust
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
```

- [ ] **Step 3: Implement `forward_openai` for `ZaiProvider`**

Add this inside the `impl Provider for ZaiProvider` block, after the existing `forward` method:

```rust
    async fn forward_openai(
        &self,
        path: &str,
        headers: &HeaderMap,
        body: Bytes,
        streaming: bool,
    ) -> Result<UpstreamResponse, ProxyError> {
        let openai_base = self
            .openai_base_url
            .as_deref()
            .ok_or_else(|| {
                ProxyError::BadRequest(
                    "provider 'zai' does not support OpenAI chat completions format".into(),
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
        )
        .await
    }
```

- [ ] **Step 4: Update `builder.rs` to pass `openai_base_url`**

In `crates/proxy/src/adapters/providers/builder.rs`, update `build_leaf` to pass the new field:

Current code:
```rust
pub fn build_leaf(p: &ProviderConfig, http: reqwest::Client) -> Arc<dyn Provider> {
    // ...
    match p.kind {
        ProviderKind::Anthropic => {
            Arc::new(AnthropicProvider::configure(http, p.base_url.clone(), auth))
        }
        ProviderKind::Zai => Arc::new(ZaiProvider::configure(http, p.base_url.clone(), auth)),
    }
}
```

Change the `Zai` arm to:
```rust
        ProviderKind::Zai => Arc::new(ZaiProvider::configure(
            http,
            p.base_url.clone(),
            p.openai_base_url.clone(),
            auth,
        )),
```

- [ ] **Step 5: Update ZaiProvider tests**

In `crates/proxy/src/adapters/providers/zai.rs`, update tests that call `configure`:

```rust
    #[test]
    fn configure_sets_both_base_url_and_auth() {
        let p = ZaiProvider::configure(
            reqwest::Client::new(),
            Some("http://localhost:1234".into()),
            None,
            AuthHeader::ApiKey("zai-test".into()),
        );
        assert_eq!(p.base_url, "http://localhost:1234");
        assert_eq!(p.openai_base_url, Some("https://api.z.ai/api/paas/v4".into()));
        assert!(matches!(p.auth, AuthHeader::ApiKey(_)));
    }

    #[test]
    fn default_openai_base_url_points_to_zai_paas() {
        assert_eq!(
            provider().openai_base_url,
            Some("https://api.z.ai/api/paas/v4".into())
        );
    }

    #[test]
    fn forward_openai_returns_error_when_no_openai_base_url() {
        let p = ZaiProvider::with_base_url(reqwest::Client::new(), "http://localhost:1234");
        assert!(p.openai_base_url.is_none());
    }
```

Remove the old `configure_sets_both_base_url_and_auth` test and replace with the new one above.

- [ ] **Step 6: Run all proxy tests**

Run: `cargo test -p proxy`
Expected: All tests pass

- [ ] **Step 7: Commit**

```bash
git add crates/proxy/src/config.rs crates/proxy/src/adapters/providers/zai.rs crates/proxy/src/adapters/providers/builder.rs
git commit -m "feat(proxy): add openai_base_url to ZaiProvider with forward_openai impl"
```

---

### Task 3: Implement `forward_openai` for `RoutingProvider` and `LiveProvider`

**Files:**
- Modify: `crates/proxy/src/adapters/providers/routing.rs`
- Modify: `crates/proxy/src/adapters/providers/live.rs`

- [ ] **Step 1: Add `forward_openai` to `RoutingProvider`**

In `crates/proxy/src/adapters/providers/routing.rs`, add inside the `impl Provider for RoutingProvider` block, after the existing `forward` method. This mirrors the `forward` method but calls `forward_openai` on the leaf provider:

```rust
    async fn forward_openai(
        &self,
        path: &str,
        headers: &HeaderMap,
        body: Bytes,
        streaming: bool,
    ) -> Result<UpstreamResponse, ProxyError> {
        let model = messages_protocol::parse_model(&body).map_err(ProxyError::BadRequest)?;

        // Namespace routing.
        if let Some((provider, bare_model)) = self.resolve_provider(&model) {
            let rewritten = rewrite_model_in_body(&body, bare_model)
                .map_err(ProxyError::BadRequest)?;
            let rewritten_body = Bytes::from(rewritten);
            return provider
                .forward_openai(path, headers, rewritten_body, streaming)
                .await;
        }

        if let Some((ns, _)) = split_namespace(&model) {
            return Err(ProxyError::BadRequest(format!(
                "unknown provider namespace '{ns}'"
            )));
        }

        // Glob-based routing — use the route's primary/fallback pool.
        let route = self.select(&model).ok_or_else(|| {
            ProxyError::BadRequest(format!("no routing rule matches model '{model}'"))
        })?;

        match route.strategy {
            RoutingStrategy::Failover => {
                self.forward_failover_openai(route, path, headers, body, streaming)
                    .await
            }
            RoutingStrategy::RoundRobin => {
                self.forward_round_robin_openai(route, path, headers, body, streaming)
                    .await
            }
        }
    }
```

Add the `_openai` variants of failover and round-robin. These are identical to the existing methods but call `forward_openai` instead of `forward`:

```rust
    async fn forward_failover_openai(
        &self,
        route: &Route,
        path: &str,
        headers: &HeaderMap,
        body: Bytes,
        streaming: bool,
    ) -> Result<UpstreamResponse, ProxyError> {
        let total = route.pool.len();
        let mut last_err: Option<ProxyError> = None;

        for (i, entry) in route.pool.iter().enumerate() {
            let attempt_name = entry.provider.name();
            match entry
                .provider
                .forward_openai(path, headers, body.clone(), streaming)
                .await
            {
                Ok(UpstreamResponse::Buffered {
                    status,
                    headers: resp_headers,
                    body: resp_body,
                }) if status >= 500 => {
                    let preview =
                        String::from_utf8_lossy(&resp_body[..resp_body.len().min(200)])
                            .to_string();
                    tracing::warn!(
                        provider = attempt_name,
                        status,
                        body = %preview,
                        attempt = i,
                        "upstream 5xx; trying next fallback"
                    );
                    if i + 1 == total {
                        return Ok(UpstreamResponse::Buffered {
                            status,
                            headers: resp_headers,
                            body: resp_body,
                        });
                    }
                    continue;
                }
                Ok(other) => return Ok(other),
                Err(e) => {
                    tracing::warn!(
                        provider = attempt_name,
                        error = %e,
                        attempt = i,
                        "upstream error; trying next fallback"
                    );
                    last_err = Some(e);
                    continue;
                }
            }
        }

        Err(last_err.unwrap_or_else(|| ProxyError::BadRequest("routing chain exhausted".into())))
    }

    async fn forward_round_robin_openai(
        &self,
        route: &Route,
        path: &str,
        headers: &HeaderMap,
        body: Bytes,
        streaming: bool,
    ) -> Result<UpstreamResponse, ProxyError> {
        let pool_size = route.pool.len();
        let start = self.rr_counter.fetch_add(1, Ordering::Relaxed) % pool_size;

        for offset in 0..pool_size {
            let idx = (start + offset) % pool_size;
            let entry = &route.pool[idx];
            let attempt_name = entry.provider.name();

            if entry.is_cooling_down() {
                tracing::debug!(
                    provider = attempt_name,
                    idx,
                    "skipping cooling-down provider"
                );
                continue;
            }

            match entry
                .provider
                .forward_openai(path, headers, body.clone(), streaming)
                .await
            {
                Ok(UpstreamResponse::Buffered {
                    status,
                    headers: resp_headers,
                    body: _,
                }) if status == 429 => {
                    let cooldown_ms = extract_retry_after_ms(&resp_headers) * 1000;
                    tracing::warn!(
                        provider = attempt_name,
                        status,
                        cooldown_secs = cooldown_ms / 1000,
                        "upstream 429 (rate limited); cooling down"
                    );
                    entry.set_cooldown(cooldown_ms);
                    continue;
                }
                Ok(UpstreamResponse::Buffered {
                    status,
                    headers: _,
                    body: resp_body,
                }) if status >= 500 => {
                    let preview =
                        String::from_utf8_lossy(&resp_body[..resp_body.len().min(200)])
                            .to_string();
                    tracing::warn!(
                        provider = attempt_name,
                        status,
                        body = %preview,
                        "upstream 5xx; cooling down"
                    );
                    entry.set_cooldown(DEFAULT_COOLDOWN_SECS * 1000);
                    continue;
                }
                Ok(other) => return Ok(other),
                Err(e) => {
                    tracing::warn!(
                        provider = attempt_name,
                        error = %e,
                        "upstream error; cooling down"
                    );
                    entry.set_cooldown(DEFAULT_COOLDOWN_SECS * 1000);
                    continue;
                }
            }
        }

        let min_remaining = route
            .pool
            .iter()
            .map(|e| e.remaining_cooldown_ms())
            .filter(|&ms| ms > 0)
            .min()
            .unwrap_or(DEFAULT_COOLDOWN_SECS * 1000);

        let retry_after_secs = min_remaining.div_ceil(1000);

        Err(ProxyError::UpstreamRateLimited {
            retry_after_secs,
            message: format!(
                "all {} providers in round-robin pool are rate-limited",
                pool_size
            ),
        })
    }
```

- [ ] **Step 2: Add `forward_openai` to `LiveProvider`**

In `crates/proxy/src/adapters/providers/live.rs`, add inside the `impl Provider for LiveProvider` block:

```rust
    async fn forward_openai(
        &self,
        path: &str,
        headers: &HeaderMap,
        body: Bytes,
        streaming: bool,
    ) -> Result<UpstreamResponse, ProxyError> {
        let cur = self.current();
        cur.forward_openai(path, headers, body, streaming).await
    }
```

- [ ] **Step 3: Run all proxy tests**

Run: `cargo test -p proxy`
Expected: All tests pass

- [ ] **Step 4: Commit**

```bash
git add crates/proxy/src/adapters/providers/routing.rs crates/proxy/src/adapters/providers/live.rs
git commit -m "feat(proxy): implement forward_openai for RoutingProvider and LiveProvider"
```

---

### Task 4: Add `ApiFormat` to `HandleMessages` use case

**Files:**
- Modify: `crates/proxy/src/application/use_cases/handle_messages.rs`

- [ ] **Step 1: Add `ApiFormat` enum**

Add this at the top of the file, after the imports:

```rust
/// Which API format the client used — determines which `forward_*` method
/// the use case calls on the provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiFormat {
    Anthropic,
    OpenAI,
}
```

- [ ] **Step 2: Add `api_format` to `HandleMessagesInput`**

Change:

```rust
pub struct HandleMessagesInput {
    pub headers: HeaderMap,
    pub body: Bytes,
}
```

To:

```rust
pub struct HandleMessagesInput {
    pub headers: HeaderMap,
    pub body: Bytes,
    pub api_format: ApiFormat,
}
```

- [ ] **Step 3: Update `execute()` to use `api_format`**

Replace the hardcoded `"/v1/messages"` path with format-aware logic. Change the `forward` call (around lines 82-90):

From:
```rust
        let upstream = self
            .provider
            .forward(
                "/v1/messages",
                &input.headers,
                input.body.clone(),
                streaming,
            )
            .await?;
```

To:
```rust
        let upstream = match input.api_format {
            ApiFormat::Anthropic => {
                self.provider
                    .forward(
                        "/v1/messages",
                        &input.headers,
                        input.body.clone(),
                        streaming,
                    )
                    .await?
            }
            ApiFormat::OpenAI => {
                self.provider
                    .forward_openai(
                        "/v1/chat/completions",
                        &input.headers,
                        input.body.clone(),
                        streaming,
                    )
                    .await?
            }
        };
```

- [ ] **Step 4: Fix all call sites of `HandleMessagesInput`**

Search for all places that construct `HandleMessagesInput` and add `api_format: ApiFormat::Anthropic`. There are two places:

1. **In `frameworks/handler.rs`** — the `messages` function (we'll fix this in Task 5)
2. **In tests** — `crates/proxy/src/application/use_cases/handle_messages.rs` tests

In the test module at the bottom of `handle_messages.rs`, add `api_format: ApiFormat::Anthropic` to every `HandleMessagesInput` construction. There are 5 test functions that create `HandleMessagesInput`:

- `invalid_model_returns_bad_request_without_inserting_row`
- `buffered_2xx_records_completed_row`
- `buffered_non2xx_records_errored_row_and_forwards_status`
- `streaming_returns_streaming_output`
- `forward_transport_error_propagates_as_proxy_error`

For each, add `api_format: ApiFormat::Anthropic` to the struct literal. Example:

```rust
            HandleMessagesInput {
                headers: HeaderMap::new(),
                body: Bytes::from_static(b"{}"),
                api_format: ApiFormat::Anthropic,
            }
```

- [ ] **Step 5: Run all proxy tests**

Run: `cargo test -p proxy`
Expected: All tests pass

- [ ] **Step 6: Commit**

```bash
git add crates/proxy/src/application/use_cases/handle_messages.rs
git commit -m "feat(proxy): add ApiFormat to HandleMessages for dual-protocol support"
```

---

### Task 5: Add `chat_completions` handler and route

**Files:**
- Modify: `crates/proxy/src/frameworks/handler.rs`
- Modify: `crates/proxy/src/frameworks/server.rs`

- [ ] **Step 1: Update `messages` handler to pass `ApiFormat`**

In `crates/proxy/src/frameworks/handler.rs`, update the `messages` function's `execute` call to include the format:

```rust
    let output = use_case
        .execute(HandleMessagesInput {
            headers: parts.headers,
            body: body_bytes,
            api_format: ApiFormat::Anthropic,
        })
        .await?;
```

Wait — we need to use the correct type. The `ApiFormat` is in `crate::application::use_cases::ApiFormat`. Add the import and update:

Current import line:
```rust
use crate::application::use_cases::{HandleMessages, HandleMessagesInput, HandleMessagesOutput};
```

Add `ApiFormat`:
```rust
use crate::application::use_cases::{ApiFormat, HandleMessages, HandleMessagesInput, HandleMessagesOutput};
```

Then update the `messages` handler to pass `api_format: ApiFormat::Anthropic`.

- [ ] **Step 2: Add `chat_completions` handler**

Add this function in `handler.rs` after the `messages` function:

```rust
pub async fn chat_completions(
    State(use_case): State<Arc<HandleMessages>>,
    req: Request,
) -> Result<Response, ProxyError> {
    let (parts, body) = req.into_parts();
    let body_bytes = axum::body::to_bytes(body, BODY_LIMIT)
        .await
        .map_err(|e| ProxyError::BadRequest(format!("body read: {e}")))?;

    let output = use_case
        .execute(HandleMessagesInput {
            headers: parts.headers,
            body: body_bytes,
            api_format: ApiFormat::OpenAI,
        })
        .await?;

    Ok(match output {
        HandleMessagesOutput::Buffered {
            status,
            headers,
            body,
        } => build_response(status, headers, Body::from(body)),
        HandleMessagesOutput::Streaming {
            status,
            headers,
            body,
            usage_parser,
            on_finish,
        } => {
            let teed = TeedStream::new(body, usage_parser, on_finish);
            build_response(status, headers, Body::from_stream(teed))
        }
    })
}
```

- [ ] **Step 3: Add the route in `server.rs`**

In `crates/proxy/src/frameworks/server.rs`, update `build_router`:

Current:
```rust
    let data = Router::new()
        .route("/v1/messages", post(messages))
        .with_state(use_case);
```

Change to:
```rust
    let data = Router::new()
        .route("/v1/messages", post(messages))
        .route("/v1/chat/completions", post(chat_completions))
        .with_state(use_case);
```

Add the import for `chat_completions`:

Current:
```rust
use super::handler::messages;
```

Change to:
```rust
use super::handler::{chat_completions, messages};
```

- [ ] **Step 4: Run all tests**

Run: `cargo test --workspace`
Expected: All tests pass

- [ ] **Step 5: Commit**

```bash
git add crates/proxy/src/frameworks/handler.rs crates/proxy/src/frameworks/server.rs
git commit -m "feat(proxy): add /v1/chat/completions route with OpenAI format handler"
```

---

### Task 6: Update docs

**Files:**
- Modify: `docs/proxy-usage.md`
- Modify: `README.md`

- [ ] **Step 1: Update Quick Start in proxy-usage.md**

Change the Quick Start section to mention both formats. Find:

```markdown
This uses a default config: single Anthropic provider, passthrough auth (forwards the client's key).
```

Replace with:

```markdown
This uses a default config: single Anthropic provider, passthrough auth (forwards the client's key).

The proxy accepts both **Anthropic** (`/v1/messages`) and **OpenAI** (`/v1/chat/completions`) request formats. Point any compatible tool at it — Claude Code, OpenCode, Cursor, etc.
```

- [ ] **Step 2: Update config example to show `openai_base_url`**

In the Multi-Provider Setup section, add `openai_base_url` to the Z.AI provider:

```toml
[[providers]]
name = "zai"
kind = "zai"
auth = { type = "api_key", value = "${ZAI_API_KEY}" }
openai_base_url = "https://api.z.ai/api/paas/v4"
```

Also add it to the Full Config Example's Z.AI provider entry.

- [ ] **Step 3: Add OpenAI format example to "Verifying It Works"**

Add a section after "### 3. Send a real request through the proxy":

```markdown
### 4. Test the OpenAI-compatible endpoint

```bash
curl http://127.0.0.1:8787/v1/chat/completions \
  -H "content-type: application/json" \
  -H "Authorization: Bearer dummy" \
  -d '{
    "model": "zai/glm-5",
    "max_tokens": 50,
    "messages": [{"role": "user", "content": "Say hello"}]
  }'
```

#### Configure OpenCode

```json
{
  "provider": {
    "cli-router": {
      "npm": "@ai-sdk/openai-compatible",
      "name": "CLI Router",
      "options": {
        "baseURL": "http://127.0.0.1:8787/v1"
      },
      "models": {
        "zai/glm-5.1": { "name": "z.ai/GLM 5.1" },
        "zai/glm-5": { "name": "z.ai/GLM 5" }
      }
    }
  }
}
```

Run `/connect` in OpenCode, select the `cli-router` provider, and enter any non-empty string as the API key (the proxy replaces it with your configured key).
```

- [ ] **Step 4: Commit**

```bash
git add docs/proxy-usage.md README.md
git commit -m "docs: document OpenAI chat completions endpoint and OpenCode setup"
```

---

### Task 7: Verify everything works end-to-end

- [ ] **Step 1: Run full test suite**

Run: `cargo test --workspace`
Expected: All tests pass

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --workspace -- -D warnings`
Expected: No warnings

- [ ] **Step 3: Manual smoke test (if proxy is running with updated binary)**

```bash
# OpenAI format with namespace
curl http://127.0.0.1:8787/v1/chat/completions \
  -H "content-type: application/json" \
  -H "Authorization: Bearer dummy" \
  -d '{"model":"zai/glm-5","max_tokens":50,"messages":[{"role":"user","content":"Say hello"}]}'
```

Expected: Successful response from Z.AI.

- [ ] **Step 4: Test Anthropic format still works**

```bash
curl http://127.0.0.1:8787/v1/messages \
  -H "content-type: application/json" \
  -H "x-api-key: dummy" \
  -H "anthropic-version: 2023-06-01" \
  -d '{"model":"zai/glm-5","max_tokens":50,"messages":[{"role":"user","content":"Say hello"}]}'
```

Expected: Successful response (same as before).
