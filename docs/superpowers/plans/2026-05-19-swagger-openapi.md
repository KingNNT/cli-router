# Swagger/OpenAPI Documentation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add interactive Swagger UI + ReDoc documentation for all 15 proxy endpoints, served on a separate port.

**Architecture:** Use `utoipa` derive macros to generate an OpenAPI 3.0 spec from Rust types and handler annotations. Serve Swagger UI and ReDoc on a separate tokio task on port 8788.

**Tech Stack:** utoipa 5.5, utoipa-swagger-ui 9.0, utoipa-redoc 6.0, axum 0.7

---

### Task 1: Add workspace dependencies

**Files:**
- Modify: `Cargo.toml` (workspace root)

- [ ] **Step 1: Add utoipa crates to workspace dependencies**

Add these three lines to the `[workspace.dependencies]` section in `Cargo.toml`:

```toml
utoipa = "5"
utoipa-swagger-ui = "9"
utoipa-redoc = "6"
```

- [ ] **Step 2: Verify TOML parses**

Run: `cargo metadata --format-version 1 > /dev/null`
Expected: exits 0

- [ ] **Step 3: Commit**

```
feat(proxy): add utoipa workspace dependencies for OpenAPI docs
```

---

### Task 2: Add utoipa dependency to proxy-admin-api

**Files:**
- Modify: `crates/proxy-admin-api/Cargo.toml`

- [ ] **Step 1: Add utoipa to proxy-admin-api dependencies**

Add `utoipa` to the `[dependencies]` section:

```toml
utoipa = { workspace = true }
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p proxy-admin-api`
Expected: compiles successfully

- [ ] **Step 3: Commit**

```
chore(proxy-admin-api): add utoipa dependency
```

---

### Task 3: Add `#[derive(ToSchema)]` to proxy-admin-api types

**Files:**
- Modify: `crates/proxy-admin-api/src/lib.rs`

This is the largest single file change. Every struct and enum that appears in an API response or request body needs `#[derive(ToSchema)]` and a `schema` rename where the Rust name differs from the desired JSON name.

- [ ] **Step 1: Add `utoipa::ToSchema` derive to all structs and enums**

For each struct/enum in the file, add `utoipa::ToSchema` to the derive clause. The file currently uses `serde::{Serialize, Deserialize}` on most types. The full list of types that need `ToSchema`:

**Structs:**
- `StatusResponse`
- `AffinityStatus`
- `AffinityPayload`
- `QuotaPayload`
- `ConfigPayload`
- `ProviderPayload`
- `RoutingRulePayload`
- `MatchPayload`
- `RecentRequestsResponse`
- `RecentRequestItem`
- `TestProviderRequest`
- `TestProviderResponse`
- `StartOAuthRequest`
- `StartOAuthResponse`
- `CompleteOAuthRequest`
- `CompleteOAuthResponse`
- `UsageSummaryResponse`
- `DailyUsageRow`
- `ModelUsageRow`
- `QuotaStatusListDto`
- `QuotaStatusDto`
- `QuotaMetricDto`
- `AccountUsageResponse`
- `ProviderAccountUsageDto`
- `UsageWindowDto`
- `UsageSubItemDto`
- `ModelBreakdownItemDto`
- `ModelUsageDto`

**Enums:**
- `AuthPayload`
- `RoutingStrategyPayload`
- `QuotaMetricState`
- `ProviderUsageStatus`

Example of the change for a struct:

```rust
// Before:
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusResponse {

// After:
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct StatusResponse {
```

Example for an enum:

```rust
// Before:
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuthPayload {

// After:
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub enum AuthPayload {
```

Add `utoipa::ToSchema` to the derive of every struct and enum listed above. Do NOT change any field names, serde attributes, or logic.

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p proxy-admin-api`
Expected: compiles successfully

- [ ] **Step 3: Commit**

```
feat(proxy-admin-api): add utoipa::ToSchema derive to all API types
```

---

### Task 4: Add utoipa crates to proxy Cargo.toml

**Files:**
- Modify: `crates/proxy/Cargo.toml`

- [ ] **Step 1: Add the three crates**

Add to `[dependencies]`:

```toml
utoipa = { workspace = true }
utoipa-swagger-ui = { workspace = true }
utoipa-redoc = { workspace = true }
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p proxy`
Expected: compiles successfully

- [ ] **Step 3: Commit**

```
chore(proxy): add utoipa, swagger-ui, and redoc dependencies
```

---

### Task 5: Create the OpenAPI module with typed proxy schemas

**Files:**
- Create: `crates/proxy/src/frameworks/openapi.rs`
- Modify: `crates/proxy/src/frameworks/mod.rs`

- [ ] **Step 1: Create `crates/proxy/src/frameworks/openapi.rs`**

This file contains:
1. Typed request structs for the 3 proxy endpoints (for documentation only)
2. The `#[utoipa::path]` annotations collected as an `OpenApi` via `utoipa::OpenApi` derive
3. A `build_openapi_spec()` function

```rust
//! OpenAPI spec generation for the CLI Router Proxy.
//!
//! Uses `utoipa` derive macros to assemble an OpenAPI 3.0 spec from handler
//! annotations and typed request/response schemas. The spec is served on a
//! separate port via Swagger UI and ReDoc.

use utoipa::openapi::security::SecurityAddon;
use utoipa::{Modify, OpenApi};

// ── Typed schemas for proxy endpoints (documentation-only) ──────────

/// Request body for `POST /v1/messages` (Anthropic Messages API).
#[derive(serde::Deserialize, utoipa::ToSchema)]
pub struct AnthropicMessagesRequest {
    /// Model identifier (e.g. "claude-sonnet-4-20250514", "glm-5").
    pub model: String,
    /// Conversation messages.
    pub messages: Vec<AnthropicMessage>,
    /// Maximum tokens to generate.
    pub max_tokens: u32,
    /// Enable server-sent events streaming.
    #[serde(default)]
    pub stream: Option<bool>,
    /// System prompt.
    #[serde(default)]
    pub system: Option<String>,
    /// Sampling temperature (0.0–1.0).
    #[serde(default)]
    pub temperature: Option<f64>,
    /// Top-p nucleus sampling.
    #[serde(default)]
    pub top_p: Option<f64>,
    /// Stop sequences.
    #[serde(default)]
    pub stop_sequences: Option<Vec<String>>,
}

/// A single message in an Anthropic conversation.
#[derive(serde::Deserialize, utoipa::ToSchema)]
pub struct AnthropicMessage {
    /// Role: "user" or "assistant".
    pub role: String,
    /// Message content (string or array of content blocks).
    pub content: serde_json::Value,
}

/// Request body for `POST /v1/chat/completions` (OpenAI Chat API).
#[derive(serde::Deserialize, utoipa::ToSchema)]
pub struct OpenAIChatRequest {
    /// Model identifier. Use `provider/model` namespace syntax to override routing.
    pub model: String,
    /// Conversation messages.
    pub messages: Vec<OpenAIChatMessage>,
    /// Maximum tokens to generate.
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// Enable server-sent events streaming.
    #[serde(default)]
    pub stream: Option<bool>,
    /// Sampling temperature (0.0–2.0).
    #[serde(default)]
    pub temperature: Option<f64>,
    /// Top-p nucleus sampling.
    #[serde(default)]
    pub top_p: Option<f64>,
}

/// A single message in an OpenAI chat conversation.
#[derive(serde::Deserialize, utoipa::ToSchema)]
pub struct OpenAIChatMessage {
    /// Role: "system", "user", "assistant", or "tool".
    pub role: String,
    /// Message content.
    pub content: serde_json::Value,
}

/// Request body for `POST /v1/messages/count_tokens`.
#[derive(serde::Deserialize, utoipa::ToSchema)]
pub struct CountTokensRequest {
    /// Model identifier.
    pub model: String,
    /// Messages to count tokens for.
    pub messages: Vec<AnthropicMessage>,
    /// System prompt (included in count).
    #[serde(default)]
    pub system: Option<String>,
}

// ── OpenAPI spec ─────────────────────────────────────────────────────

/// CLI Router Proxy API — all endpoints.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "CLI Router Proxy API",
        version = "1.0.0",
        description = "HTTP proxy for LLM providers with usage capture, routing, and admin dashboard."
    ),
    tags(
        (name = "Proxy", description = "LLM proxy endpoints"),
        (name = "Admin", description = "Configuration and status"),
        (name = "Usage", description = "Usage metrics and quotas"),
        (name = "Providers", description = "Provider management"),
        (name = "OAuth", description = "OAuth authentication flows"),
    ),
    paths(
        crate::frameworks::handler::messages,
        crate::frameworks::handler::count_tokens,
        crate::frameworks::handler::chat_completions,
        crate::frameworks::admin::status_handler,
        crate::frameworks::admin::get_config_handler,
        crate::frameworks::admin::update_config_handler,
        crate::frameworks::admin::recent_handler,
        crate::frameworks::admin::usage_summary_handler,
        crate::frameworks::admin::account_usage_handler,
        crate::frameworks::admin::quota_status_handler,
        crate::frameworks::admin::test_provider_handler,
        crate::frameworks::admin::oauth_start_handler,
        crate::frameworks::admin::oauth_complete_handler,
        crate::frameworks::admin::openai_oauth_start_handler,
        crate::frameworks::admin::openai_oauth_complete_handler,
    ),
    schemas(
        // Proxy endpoint schemas
        AnthropicMessagesRequest,
        AnthropicMessage,
        OpenAIChatRequest,
        OpenAIChatMessage,
        CountTokensRequest,
        // Admin API schemas — all from proxy-admin-api
        proxy_admin_api::StatusResponse,
        proxy_admin_api::ConfigPayload,
        proxy_admin_api::ProviderPayload,
        proxy_admin_api::AuthPayload,
        proxy_admin_api::RoutingRulePayload,
        proxy_admin_api::RoutingStrategyPayload,
        proxy_admin_api::MatchPayload,
        proxy_admin_api::AffinityPayload,
        proxy_admin_api::QuotaPayload,
        proxy_admin_api::RecentRequestsResponse,
        proxy_admin_api::RecentRequestItem,
        proxy_admin_api::TestProviderRequest,
        proxy_admin_api::TestProviderResponse,
        proxy_admin_api::StartOAuthRequest,
        proxy_admin_api::StartOAuthResponse,
        proxy_admin_api::CompleteOAuthRequest,
        proxy_admin_api::CompleteOAuthResponse,
        proxy_admin_api::UsageSummaryResponse,
        proxy_admin_api::DailyUsageRow,
        proxy_admin_api::ModelUsageRow,
        proxy_admin_api::QuotaStatusListDto,
        proxy_admin_api::QuotaStatusDto,
        proxy_admin_api::QuotaMetricDto,
        proxy_admin_api::QuotaMetricState,
        proxy_admin_api::AccountUsageResponse,
        proxy_admin_api::ProviderAccountUsageDto,
        proxy_admin_api::ProviderUsageStatus,
        proxy_admin_api::UsageWindowDto,
        proxy_admin_api::UsageSubItemDto,
        proxy_admin_api::ModelBreakdownItemDto,
        proxy_admin_api::ModelUsageDto,
    ),
)]
pub struct ApiDoc;

pub fn build_openapi_spec() -> utoipa::OpenApi {
    ApiDoc::openapi()
}
```

- [ ] **Step 2: Register the module in `crates/proxy/src/frameworks/mod.rs`**

Add `pub mod openapi;` to the module declarations. The file becomes:

```rust
//! Frameworks ring: axum-specific glue + drivers.

pub mod admin;
pub mod error;
pub mod handler;
pub mod openapi;
pub mod server;
pub mod stream;

pub use admin::{AdminState, build_admin_router};
pub use error::ProxyError;
pub use server::build_router;
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p proxy`
Expected: this will FAIL because the handlers don't have `#[utoipa::path]` yet. That's expected — proceed to Task 6.

- [ ] **Step 4: Commit**

```
feat(proxy): add openapi module with typed proxy schemas and spec definition
```

---

### Task 6: Add `#[utoipa::path]` annotations to proxy handlers

**Files:**
- Modify: `crates/proxy/src/frameworks/handler.rs`

- [ ] **Step 1: Add annotations to the 3 proxy handler functions**

The handler functions are `messages`, `count_tokens`, and `chat_completions`. Add `#[utoipa::path]` above each `pub async fn`. The function bodies remain unchanged.

For `messages` (line ~34), add above the function:

```rust
#[utoipa::path(
    post,
    path = "/v1/messages",
    request_body = AnthropicMessagesRequest,
    responses(
        (status = 200, description = "Message response (streaming or buffered JSON)"),
        (status = 400, description = "Bad request — invalid body or missing fields"),
        (status = 429, description = "Rate limited — all providers in pool are throttled"),
        (status = 502, description = "Upstream provider error"),
    ),
    tag = "Proxy"
)]
```

You'll also need to add the import at the top of the file:

```rust
use crate::frameworks::openapi::AnthropicMessagesRequest;
```

For `count_tokens` (line ~70), add above the function:

```rust
#[utoipa::path(
    post,
    path = "/v1/messages/count_tokens",
    request_body = CountTokensRequest,
    responses(
        (status = 200, description = "Token count estimate"),
        (status = 400, description = "Bad request"),
        (status = 502, description = "Upstream provider error"),
    ),
    tag = "Proxy"
)]
```

Add the import:

```rust
use crate::frameworks::openapi::CountTokensRequest;
```

For `chat_completions` (line ~96), add above the function:

```rust
#[utoipa::path(
    post,
    path = "/v1/chat/completions",
    request_body = OpenAIChatRequest,
    responses(
        (status = 200, description = "Chat completion response (streaming or buffered)"),
        (status = 400, description = "Bad request — invalid body or missing fields"),
        (status = 429, description = "Rate limited"),
        (status = 502, description = "Upstream provider error"),
    ),
    tag = "Proxy"
)]
```

Add the import:

```rust
use crate::frameworks::openapi::OpenAIChatRequest;
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p proxy`
Expected: still fails because admin handlers don't have annotations yet. That's expected.

- [ ] **Step 3: Commit**

```
feat(proxy): add utoipa path annotations to proxy endpoint handlers
```

---

### Task 7: Add `#[utoipa::path]` annotations to admin handlers

**Files:**
- Modify: `crates/proxy/src/frameworks/admin.rs`

- [ ] **Step 1: Add annotations to all 12 admin handlers**

Add `#[utoipa::path]` above each handler function. Here are all 12:

**`status_handler`:**
```rust
#[utoipa::path(
    get,
    path = "/admin/status",
    responses(
        (status = 200, description = "Proxy uptime and request counts", body = StatusResponse),
        (status = 500, description = "Internal error"),
    ),
    tag = "Admin"
)]
```

**`get_config_handler`:**
```rust
#[utoipa::path(
    get,
    path = "/admin/config",
    responses(
        (status = 200, description = "Current proxy configuration", body = ConfigPayload),
        (status = 500, description = "Internal error"),
    ),
    tag = "Admin"
)]
```

**`update_config_handler`:**
```rust
#[utoipa::path(
    put,
    path = "/admin/config",
    request_body = ConfigPayload,
    responses(
        (status = 200, description = "Config updated, returns new config", body = ConfigPayload),
        (status = 400, description = "Invalid config payload"),
        (status = 500, description = "Internal error"),
    ),
    tag = "Admin"
)]
```

**`recent_handler`:**
```rust
#[utoipa::path(
    get,
    path = "/admin/requests/recent",
    params(
        ("limit" = Option<u32>, Query, description = "Max rows to return"),
        ("offset" = Option<u32>, Query, description = "Row offset for pagination"),
    ),
    responses(
        (status = 200, description = "Recent request log entries", body = RecentRequestsResponse),
        (status = 500, description = "Internal error"),
    ),
    tag = "Admin"
)]
```

**`usage_summary_handler`:**
```rust
#[utoipa::path(
    get,
    path = "/admin/usage/summary",
    params(
        ("from" = i64, Query, description = "Start timestamp (epoch ms)"),
        ("to" = i64, Query, description = "End timestamp (epoch ms)"),
    ),
    responses(
        (status = 200, description = "Aggregate usage summary", body = UsageSummaryResponse),
        (status = 500, description = "Internal error"),
    ),
    tag = "Usage"
)]
```

**`account_usage_handler`:**
```rust
#[utoipa::path(
    get,
    path = "/admin/account/usage",
    responses(
        (status = 200, description = "Provider account balances and quota", body = AccountUsageResponse),
    ),
    tag = "Usage"
)]
```

**`quota_status_handler`:**
```rust
#[utoipa::path(
    get,
    path = "/admin/quota/status",
    responses(
        (status = 200, description = "Per-provider quota health", body = QuotaStatusListDto),
    ),
    tag = "Usage"
)]
```

**`test_provider_handler`:**
```rust
#[utoipa::path(
    post,
    path = "/admin/providers/{name}/test",
    params(
        ("name" = String, Path, description = "Provider name"),
    ),
    request_body = TestProviderRequest,
    responses(
        (status = 200, description = "Provider test result", body = TestProviderResponse),
    ),
    tag = "Providers"
)]
```

**`oauth_start_handler`:**
```rust
#[utoipa::path(
    post,
    path = "/admin/oauth/anthropic/start",
    request_body = StartOAuthRequest,
    responses(
        (status = 200, description = "OAuth authorization URL", body = StartOAuthResponse),
    ),
    tag = "OAuth"
)]
```

**`oauth_complete_handler`:**
```rust
#[utoipa::path(
    post,
    path = "/admin/oauth/anthropic/complete",
    request_body = CompleteOAuthRequest,
    responses(
        (status = 200, description = "OAuth completion result", body = CompleteOAuthResponse),
    ),
    tag = "OAuth"
)]
```

**`openai_oauth_start_handler`:**
```rust
#[utoipa::path(
    post,
    path = "/admin/oauth/openai/start",
    request_body = StartOAuthRequest,
    responses(
        (status = 200, description = "OAuth authorization URL", body = StartOAuthResponse),
    ),
    tag = "OAuth"
)]
```

**`openai_oauth_complete_handler`:**
```rust
#[utoipa::path(
    post,
    path = "/admin/oauth/openai/complete",
    request_body = CompleteOAuthRequest,
    responses(
        (status = 200, description = "OAuth completion result", body = CompleteOAuthResponse),
    ),
    tag = "OAuth"
)]
```

- [ ] **Step 2: Verify the full project compiles**

Run: `cargo check -p proxy`
Expected: compiles successfully (all handler annotations + schemas resolve)

- [ ] **Step 3: Commit**

```
feat(proxy): add utoipa path annotations to all admin handlers
```

---

### Task 8: Add `docs_port` and `docs_enabled` to config

**Files:**
- Modify: `crates/proxy/src/config.rs`

- [ ] **Step 1: Add two fields to the `Config` struct**

Add these fields after the `quota` field in `Config`:

```rust
    /// Port for the Swagger UI / ReDoc docs server. Default: 8788.
    #[serde(default = "default_docs_port")]
    pub docs_port: u16,
    /// Whether to start the docs server. Default: true.
    #[serde(default = "default_true")]
    pub docs_enabled: bool,
```

- [ ] **Step 2: Add the default function**

Add near the other default functions (e.g. after `default_port`):

```rust
fn default_docs_port() -> u16 {
    8788
}
```

- [ ] **Step 3: Update all existing test Config literals**

Every test in `config.rs` that constructs `Config { ... }` must now include the two new fields. Add to each:

```rust
docs_port: 8788,
docs_enabled: true,
```

The affected tests are:
- `validate_rejects_empty_providers`
- `validate_rejects_routing_with_unknown_provider`
- `validate_rejects_duplicate_provider_names`
- `validate_rejects_unknown_fallback_provider`

- [ ] **Step 4: Verify it compiles and tests pass**

Run: `cargo test -p proxy --lib -- config`
Expected: all tests pass

- [ ] **Step 5: Commit**

```
feat(proxy): add docs_port and docs_enabled config fields
```

---

### Task 9: Add docs server startup to `lib.rs`

**Files:**
- Modify: `crates/proxy/src/lib.rs`

- [ ] **Step 1: Add the docs server function**

The current `serve` function only starts the main proxy. Add a separate function to build and return the docs router, then modify `serve` to accept an optional docs port and spawn the docs server.

Replace the entire `serve` function with:

```rust
pub async fn serve(
    addr: SocketAddr,
    use_case: Arc<HandleMessages>,
    admin: frameworks::AdminState,
    docs_port: Option<u16>,
) -> Result<(), std::io::Error> {
    let app = frameworks::build_router(use_case, admin);

    if let Some(port) = docs_port {
        let docs_addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        let docs_app = frameworks::openapi::build_docs_app();
        tracing::info!(address = %docs_addr, "docs server listening");
        tokio::spawn(async move {
            let listener = tokio::net::TcpListener::bind(docs_addr)
                .await
                .expect("failed to bind docs server");
            axum::serve(listener, docs_app).await.expect("docs server error");
        });
    }

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(address = %addr, "proxy listening");
    axum::serve(listener, app).await?;
    Ok(())
}
```

- [ ] **Step 2: Add `build_docs_app` to the openapi module**

Add to `crates/proxy/src/frameworks/openapi.rs`:

```rust
use axum::Router;

pub fn build_docs_app() -> Router {
    use utoipa_redoc::Redoc;
    use utoipa_swagger_ui::SwaggerUi;

    let spec = build_openapi_spec();

    Router::new()
        .merge(
            SwaggerUi::new("/swagger-ui")
                .url("/api-docs/openapi.json", spec.clone()),
        )
        .merge(Redoc::new("/redoc").url("/api-docs/openapi.json", spec))
}
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p proxy`
Expected: compiles successfully

- [ ] **Step 4: Commit**

```
feat(proxy): add docs server with Swagger UI and ReDoc
```

---

### Task 10: Wire up docs server in main.rs

**Files:**
- Modify: `crates/proxy/src/main.rs`

- [ ] **Step 1: Update the `proxy::serve` call to pass docs config**

Find the current call at line ~243:

```rust
    proxy::serve(addr, use_case, admin).await?;
```

Replace with:

```rust
    let docs_port = if cfg.docs_enabled {
        Some(cfg.docs_port)
    } else {
        None
    };
    proxy::serve(addr, use_case, admin, docs_port).await?;
```

- [ ] **Step 2: Verify the full project compiles**

Run: `cargo check -p proxy`
Expected: compiles successfully

- [ ] **Step 3: Commit**

```
feat(proxy): wire up docs server startup from main with config
```

---

### Task 11: Integration test — verify docs server starts

**Files:**
- Create: `crates/proxy/tests/docs_server.rs`

This test follows the same pattern as the existing `integration.rs` — build the router directly with a wiremock upstream, then separately start the docs server and verify it serves the OpenAPI spec.

- [ ] **Step 1: Write the integration test**

```rust
//! Integration test: verify the docs server serves the OpenAPI JSON spec.

use std::sync::{Arc, Mutex};

use proxy::adapters::providers::AnthropicProvider;
use proxy::adapters::storage::{SqliteRequestLogRepository, ensure_current};
use proxy::application::ports::RequestLogPort;
use proxy::application::use_cases::HandleMessages;
use rusqlite::Connection;
use shared::adapters::clock::SystemClock;
use shared::application::ports::{Clock, PricingRepository};
use chrono::NaiveDate;

struct NullPricing;
impl PricingRepository for NullPricing {
    fn upsert_many(
        &self,
        _: &[shared::domain::entities::ModelPricing],
    ) -> Result<usize, shared::application::errors::ApplicationError> {
        Ok(0)
    }
    fn find_many(
        &self,
        _: &[String],
    ) -> Result<
        std::collections::HashMap<String, shared::domain::entities::ModelPricing>,
        shared::application::errors::ApplicationError,
    > {
        Ok(std::collections::HashMap::new())
    }
    fn list(
        &self,
    ) -> Result<
        Vec<shared::domain::entities::ModelPricing>,
        shared::application::errors::ApplicationError,
    > {
        Ok(Vec::new())
    }
    fn last_sync(
        &self,
    ) -> Result<Option<NaiveDate>, shared::application::errors::ApplicationError> {
        Ok(None)
    }
}

#[tokio::test]
async fn docs_server_serves_openapi_spec() {
    // Start the docs server on an ephemeral port.
    let docs_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let docs_addr = docs_listener.local_addr().unwrap();

    let docs_app = proxy::frameworks::openapi::build_docs_app();
    tokio::spawn(async move {
        axum::serve(docs_listener, docs_app).await.unwrap();
    });

    // Give the server a moment to start.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Fetch the OpenAPI JSON.
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{docs_addr}/api-docs/openapi.json"))
        .send()
        .await
        .expect("docs server request failed");
    assert!(
        resp.status().is_success(),
        "expected 200, got {}",
        resp.status()
    );

    let body: serde_json::Value = resp.json().await.expect("invalid JSON");
    assert_eq!(body["info"]["title"], "CLI Router Proxy API");
    assert!(body["paths"].is_object(), "paths should be an object");

    // Verify key endpoints exist in the spec.
    let paths = body["paths"].as_object().unwrap();
    assert!(paths.contains_key("/v1/messages"), "missing /v1/messages");
    assert!(
        paths.contains_key("/v1/chat/completions"),
        "missing /v1/chat/completions"
    );
    assert!(
        paths.contains_key("/admin/status"),
        "missing /admin/status"
    );
    assert!(paths.contains_key("/admin/config"), "missing /admin/config");

    // Verify Swagger UI HTML is served.
    let swagger_resp = client
        .get(format!("http://{docs_addr}/swagger-ui/"))
        .send()
        .await
        .expect("swagger-ui request failed");
    assert!(swagger_resp.status().is_success());
}

#[tokio::test]
async fn docs_server_serves_redoc() {
    let docs_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let docs_addr = docs_listener.local_addr().unwrap();

    let docs_app = proxy::frameworks::openapi::build_docs_app();
    tokio::spawn(async move {
        axum::serve(docs_listener, docs_app).await.unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{docs_addr}/redoc"))
        .send()
        .await
        .expect("redoc request failed");
    assert!(
        resp.status().is_success(),
        "expected 200, got {}",
        resp.status()
    );
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test -p proxy --test docs_server`
Expected: both tests pass

- [ ] **Step 3: Commit**

```
test(proxy): add integration tests for docs server OpenAPI spec
```

---

### Task 12: Update documentation

**Files:**
- Modify: `docs/proxy-usage.md`

- [ ] **Step 1: Add Swagger UI section to the proxy usage doc**

Add a new section after the "Admin API" section. Insert before the "Full Config Example" section:

```markdown
---

## API Documentation (Swagger UI)

The proxy serves interactive API documentation on a **separate port** (default: 8788).

| URL | Description |
|-----|-------------|
| `http://127.0.0.1:8788/swagger-ui/` | Swagger UI — try all endpoints from the browser |
| `http://127.0.0.1:8788/redoc` | ReDoc — clean, readable API reference |
| `http://127.0.0.1:8788/api-docs/openapi.json` | Raw OpenAPI 3.0 JSON spec |

All 15 endpoints are documented with request/response schemas:
- **Proxy**: `/v1/messages`, `/v1/messages/count_tokens`, `/v1/chat/completions`
- **Admin**: status, config, recent requests, usage, quotas, provider testing, OAuth

### Configuration

```toml
# docs server port (default: 8788)
docs_port = 8788

# disable the docs server entirely (default: true)
docs_enabled = false
```

### Quick test:

```bash
# Open in browser
open http://127.0.0.1:8788/swagger-ui/

# Or fetch the spec directly
curl http://127.0.0.1:8788/api-docs/openapi.json | jq .info
```
```

- [ ] **Step 2: Commit**

```
docs(proxy): add Swagger UI section to proxy-usage.md
```

---

### Task 13: Final verification

- [ ] **Step 1: Run all proxy tests**

Run: `cargo test -p proxy`
Expected: all tests pass

- [ ] **Step 2: Run full workspace check**

Run: `cargo check --workspace`
Expected: compiles successfully

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -p proxy --all-targets -- -D warnings`
Expected: no warnings

- [ ] **Step 4: Manual smoke test — start the proxy and verify docs**

```bash
# Start the proxy (needs a valid DB or config)
cargo run -p proxy

# In another terminal:
curl -s http://127.0.0.1:8788/api-docs/openapi.json | jq .info.title
# Expected: "CLI Router Proxy API"

curl -s http://127.0.0.1:8788/swagger-ui/ | head -5
# Expected: HTML for Swagger UI
```
