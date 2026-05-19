# Swagger/OpenAPI Documentation for CLI Router Proxy

**Date:** 2026-05-19
**Status:** Design

## Goal

Add interactive Swagger UI documentation covering all 15 proxy endpoints (3 proxy + 12 admin), served on a separate port from the main proxy.

## Approach

Use `utoipa` + `utoipa-swagger-ui` + `utoipa-redoc` — the standard Rust OpenAPI toolchain for Axum. Annotate existing types and handlers with derive macros; spec is generated at compile time.

## Architecture

Two-server design:

```
Main proxy (port 8787)                Docs server (port 8788)
┌──────────────────────┐              ┌──────────────────────┐
│ /v1/messages         │              │ /swagger-ui/         │
│ /v1/messages/        │              │   → Interactive UI   │
│   count_tokens       │              │ /api-docs/openapi    │
│ /v1/chat/completions │              │   → Raw JSON spec    │
│ /admin/*             │              │ /redoc               │
└──────────────────────┘              │   → Alternative UI   │
                                      └──────────────────────┘
```

- Docs server is a separate tokio task spawned alongside the main proxy.
- Default docs port: 8788, configurable via `docs_port` in config.toml.
- OpenAPI spec built once at startup from utoipa macros.
- Can be disabled with `docs_enabled = false`.

## Typed Schemas for Proxy Endpoints

The 3 proxy endpoints currently use raw `Request` bodies. New typed structs for documentation only (handlers remain unchanged):

### Anthropic Messages (`POST /v1/messages`)

```rust
#[derive(ToSchema, Deserialize)]
pub struct AnthropicMessagesRequest {
    pub model: String,
    pub messages: Vec<AnthropicMessage>,
    pub max_tokens: u32,
    pub stream: Option<bool>,
    pub system: Option<String>,
    pub temperature: Option<f64>,
}

#[derive(ToSchema, Deserialize)]
pub struct AnthropicMessage {
    pub role: String,
    pub content: Value,
}
```

Response documented as generic JSON with description (shape varies by provider/streaming).

### OpenAI Chat Completions (`POST /v1/chat/completions`)

```rust
#[derive(ToSchema, Deserialize)]
pub struct OpenAIChatRequest {
    pub model: String,
    pub messages: Vec<OpenAIChatMessage>,
    pub max_tokens: Option<u32>,
    pub stream: Option<bool>,
    pub temperature: Option<f64>,
}

#[derive(ToSchema, Deserialize)]
pub struct OpenAIChatMessage {
    pub role: String,
    pub content: Value,
}
```

### Count Tokens (`POST /v1/messages/count_tokens`)

```rust
#[derive(ToSchema, Deserialize)]
pub struct CountTokensRequest {
    pub model: String,
    pub messages: Vec<AnthropicMessage>,
}
```

### Admin API

All existing types in `proxy-admin-api` get `#[derive(ToSchema)]` added — they already have `Serialize`/`Deserialize`.

## Handler Annotations

Each handler gets `#[utoipa::path]` with method, path, request/response types, and tag. Actual handler code is unchanged.

Example:

```rust
#[utoipa::path(
    get,
    path = "/admin/status",
    responses(
        (status = 200, description = "Proxy status", body = StatusResponse),
        (status = 500, description = "Internal error", body = ApiError),
    ),
    tag = "Admin"
)]
async fn status_handler(...) { ... }
```

## Tags

| Tag | Endpoints |
|-----|-----------|
| Proxy | `/v1/messages`, `/v1/messages/count_tokens`, `/v1/chat/completions` |
| Admin | `/admin/status`, `/admin/config` (GET/PUT), `/admin/requests/recent` |
| Usage | `/admin/usage/summary`, `/admin/account/usage`, `/admin/quota/status` |
| Providers | `/admin/providers/:name/test` |
| OAuth | `/admin/oauth/anthropic/start`, `/admin/oauth/anthropic/complete`, `/admin/oauth/openai/start`, `/admin/oauth/openai/complete` |

## Spec Assembly

`build_openapi_spec()` in `crates/proxy/src/frameworks/openapi.rs` creates the `OpenApi` with all paths and schemas.

## Docs Server Startup

In `main.rs`:

```rust
let spec = build_openapi_spec();

let swagger = SwaggerUi::new("/swagger-ui")
    .url("/api-docs/openapi.json", spec.clone());

let redoc = Redoc::new("/redoc")
    .url("/api-docs/openapi.json", spec);

let docs_app = Router::new()
    .merge(swagger)
    .merge(redoc);

tokio::spawn(serve_docs(docs_app, docs_port));
```

## Configuration

```toml
# config.toml
docs_port = 8788       # default: 8788
docs_enabled = true    # default: true
```

## File Changes

| File | Change |
|------|--------|
| `Cargo.toml` (workspace) | Add `utoipa`, `utoipa-swagger-ui`, `utoipa-redoc` to workspace deps |
| `crates/proxy/Cargo.toml` | Add the 3 crates as dependencies |
| `crates/proxy-admin-api/Cargo.toml` | Add `utoipa` as dependency |
| `crates/proxy-admin-api/src/lib.rs` | Add `#[derive(ToSchema)]` to all structs and enums |
| `crates/proxy/src/frameworks/openapi.rs` | **New file** — typed proxy structs, `build_openapi_spec()`, path annotations |
| `crates/proxy/src/frameworks/admin.rs` | Add `#[utoipa::path]` annotations to 12 admin handlers |
| `crates/proxy/src/frameworks/handler.rs` | Add `#[utoipa::path]` annotations to 3 proxy handlers |
| `crates/proxy/src/frameworks/mod.rs` | Export new `openapi` module |
| `crates/proxy/src/frameworks/server.rs` | Add docs server spawn logic |
| `crates/proxy/src/config.rs` | Add `docs_port` and `docs_enabled` fields |
| `crates/proxy/src/main.rs` | Wire up docs server startup |
| `docs/proxy-usage.md` | Add Swagger UI section |

## Out of Scope

- Modifying actual handler logic — annotations only
- Authentication on the docs server (localhost-only already)
- API versioning in the spec
