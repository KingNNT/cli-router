# OpenAI Provider with Codex OAuth — Design Spec

Date: 2026-05-18

## Goal

Add OpenAI as a provider to cli-router, supporting two authentication methods:

1. **API key (Bearer)** — standard `sk-...` token, pay-as-you-go via OpenAI Platform billing
2. **Codex OAuth** — browser-based PKCE flow that authenticates against the user's ChatGPT account, billing model usage to their ChatGPT Plus/Pro/Team/Enterprise subscription

## Approach

Single `ProviderKind::OpenAi` with auth-determined behaviour. This mirrors the existing Anthropic pattern (one provider kind, multiple auth types). The provider adapter is structurally identical to `DeepSeekProvider` since Codex OAuth uses the same `/v1/chat/completions` endpoint and response format as the standard OpenAI API.

---

## 1. Config

### ProviderKind

```rust
// crates/proxy/src/config.rs
pub enum ProviderKind {
    Anthropic,
    Zai,
    DeepSeek,
    OpenAi,  // serde(alias = "open_ai")
}
```

Serde aliases: `openai`, `open_ai`. The `parse_kind` function in config.rs also needs an `"openai"` arm.

### AuthConfig

```rust
pub enum AuthConfig {
    Passthrough,
    ApiKey { value: String },
    Bearer { value: String },
    AnthropicOAuth {
        access_token: String,
        refresh_token: String,
        expires_at_ms: u64,
    },
    OpenAiOAuth {
        access_token: String,
        refresh_token: String,
        expires_at_ms: u64,
    },
}
```

Serde tag: `openai_oauth`.

### ProviderConfig — no structural changes

Existing fields `kind`, `auth`, `base_url` are sufficient.

### TOML examples

```toml
# API key mode — pay-as-you-go
[[providers]]
name = "openai"
kind = "openai"
auth = { type = "bearer", value = "${OPENAI_API_KEY}" }
base_url = "https://api.openai.com/v1"

# Codex OAuth mode — ChatGPT subscription
[[providers]]
name = "codex"
kind = "openai"
auth = { type = "openai_oauth", access_token = "", refresh_token = "", expires_at_ms = 0 }
# base_url defaults to "https://api.openai.com/v1"
```

Routing works identically to other providers:

```toml
[[routing]]
match_spec = { model = "gpt-5*" }
provider = "openai"

[[routing]]
match_spec = { model = "o3*" }
provider = "codex"
```

---

## 2. Provider Adapter

### New file: `crates/proxy/src/adapters/providers/openai.rs`

```rust
pub struct OpenAiProvider {
    base_url: String,  // default: "https://api.openai.com/v1"
    http: reqwest::Client,
    auth: AuthHeader,
}
```

Constructor pattern matches other providers: `new()`, `with_base_url()`, `with_auth()`, `configure()`.

### Provider trait impl

| Method | Behaviour |
|---|---|
| `name()` | `"openai"` |
| `native_format()` | `ApiFormat::OpenAI` |
| `parse_model()` | `messages_protocol::parse_model(body)` |
| `usage_parser()` | `messages_protocol::openai_usage_parser()` |
| `parse_usage_json()` | `messages_protocol::parse_openai_usage_json(body)` |
| `usage_parser_openai()` | same as `usage_parser()` |
| `parse_usage_json_openai()` | same as `parse_usage_json()` |
| `forward()` | Error — does not support Anthropic messages format |
| `forward_openai()` | `messages_protocol::forward()` with base_url, auth, streaming |

### builder.rs changes

```rust
// build_leaf — new arm
ProviderKind::OpenAi => Arc::new(OpenAiProvider::configure(
    http,
    p.base_url.clone(),
    auth,
)),

// build_account_usage — noop for now
ProviderKind::OpenAi => Arc::new(NoopAccountUsage),
```

OpenAI's billing/usage API is in flux and Codex quota tracking requires a different approach. Account usage returns `None` (noop) in the initial implementation.

---

## 3. Codex OAuth Flow

### New file: `crates/proxy/src/adapters/oauth/openai.rs`

Mirrors `oauth/anthropic.rs` exactly. **Name collision note:** the `oauth/mod.rs` re-exports Anthropic types at the top level (`OAuthError`, `OAuthSessionStore`, etc.). The OpenAI module types must either stay namespaced (accessed as `oauth::openai::OAuthError`) or be prefixed (`OpenAiOAuthError`). The recommended approach: keep them namespaced — callers use `oauth::openai::generate_pkce()` etc. The mod.rs re-export list is NOT extended for OpenAI.

### Constants

```rust
const CLIENT_ID: &str = "...";        // Extracted from Codex CLI / CLIProxyAPI
const AUTHORIZE_URL: &str = "https://auth.openai.com/authorize";
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const REDIRECT_URI: &str = "http://localhost:1455/callback";
const SCOPES: &str = "openid email profile";
```

The exact `CLIENT_ID` and scopes must be verified against the Codex CLI source or CLIProxyAPI's Go implementation during implementation.

### Functions

| Function | Purpose |
|---|---|
| `generate_pkce()` | Create PKCE verifier + challenge + state |
| `build_authorize_url(pkce)` | Build the browser URL for the consent screen |
| `exchange_code(http, code, verifier)` | Exchange auth code for access + refresh tokens |
| `refresh_token(http, refresh_token)` | Refresh an expiring access token |

Returns `OAuthTokens { access_token, refresh_token, expires_in }` — same shape as Anthropic.

### Admin API endpoints

Two new routes, modelled on the existing Anthropic OAuth routes:

```
POST /admin/oauth/openai/start     → { authorize_url, state }
POST /admin/oauth/openai/complete  → accepts { code, state }, exchanges, writes to config
```

New use-case structs: `StartOpenAiOAuth`, `CompleteOpenAiOAuth` — same shape as the Anthropic equivalents. These live in `application/use_cases/admin.rs` alongside the Anthropic OAuth use cases. They import `oauth::openai::*` directly.

### DTOs (proxy-admin-api crate)

The `AuthPayload` enum in `proxy-admin-api` needs a new `OpenAiOAuth` variant (mirroring `AnthropicOAuth`). The mapping functions `auth_to_payload` / `payload_to_auth` in `admin.rs` need matching arms. Similarly, `kind_to_str` / `str_to_kind` need `"openai"` arms.

### TUI integration

The proxy-tui OAuth screen gains a second option: "OpenAI (Codex)" alongside "Anthropic". Same browser flow: TUI calls start → opens browser → user signs in with ChatGPT → callback → TUI calls complete.

### Token refresh

The existing `token_refresh.rs` background task (60s interval) is extended to match `AuthConfig::OpenAiOAuth`:

```rust
// In refresh_expiring(), alongside AnthropicOAuth:
AuthConfig::OpenAiOAuth { refresh_token, expires_at_ms, .. }
    if now_ms + 300_000 >= *expires_at_ms && !refresh_token.is_empty() =>
{
    Some((p.name.clone(), refresh_token.clone()))
}
```

The refresh loop collects `(name, refresh_token, oauth_kind)` tuples from both `AnthropicOAuth` and `OpenAiOAuth` providers, then dispatches to `oauth::anthropic::refresh_token()` or `oauth::openai::refresh_token()` based on `oauth_kind`.

### 401 retry

The existing 401-retry mechanism (catch auth failure, refresh token, retry once) is extended to also handle `OpenAiOAuth` tokens — same pattern as Anthropic.

---

## 4. Module Boundaries

```
crates/proxy/src/
├── config.rs                               ← +ProviderKind::OpenAi, +AuthConfig::OpenAiOAuth
├── adapters/
│   ├── oauth/
│   │   ├── mod.rs                          ← add: pub mod openai; (but no top-level re-exports)
│   │   ├── anthropic.rs                    ← untouched
│   │   └── openai.rs                       ← NEW
│   ├── providers/
│   │   ├── openai.rs                       ← NEW
│   │   ├── builder.rs                      ← +OpenAi arms in build_leaf, build_account_usage
│   │   ├── token_refresh.rs                ← +OpenAiOAuth match arm, dispatch to openai::refresh_token
│   │   ├── live.rs                         ← no changes (generic over Provider trait)
│   │   └── account_usage/
│   │       ├── mod.rs                      ← no changes needed (noop returned from builder)
│   │       └── noop.rs                     ← existing
├── application/
│   └── use_cases/
│       └── admin.rs                        ← +StartOpenAiOAuth, CompleteOpenAiOAuth, +arms in kind_to_str/str_to_kind/auth_to_payload/payload_to_auth
├── frameworks/
│   └── admin.rs                            ← +2 routes: /admin/oauth/openai/{start,complete}, +fields in AdminState
└── ...
```

**proxy-admin-api crate** (shared DTOs):
```
crates/proxy-admin-api/src/
└── lib.rs                                  ← +AuthPayload::OpenAiOAuth variant
```
```

---

## 5. Error Handling

- `OAuthError` enum in `oauth/openai.rs` — mirrors `oauth/anthropic.rs`
- `ProxyError::BadRequest` for unsupported format (Anthropic requests to an OpenAI provider)
- 401 retry: extend existing mechanism to handle `OpenAiOAuth` token refresh
- Background refresh failure: logged as warning, same as Anthropic

---

## 6. Testing

### Unit tests (in-module)

**`providers/openai.rs`:**
- `name_is_openai`
- `native_format_is_openai`
- `default_base_url_points_to_openai_api`
- `with_base_url_overrides_default`
- `default_auth_is_passthrough`
- `configure_sets_base_url_and_auth`
- `configure_uses_default_base_url_when_none`
- `parses_model_from_body`

**`oauth/openai.rs`:**
- PKCE verifier length and uniqueness
- PKCE challenge distinct from verifier
- Authorize URL contains required params (client_id, redirect_uri, scope, code_challenge, state)
- Session store round-trip
- Session store caps at 32 entries

**`config.rs`:**
- `parse_kind_accepts_openai_aliases`
- `toml_parses_openai_provider`
- `toml_parses_openai_oauth_auth`

### Integration tests (wiremock)

- OpenAI provider forwards `/v1/chat/completions` with correct `Authorization: Bearer` header
- OAuth code exchange returns tokens and writes to config
- Token refresh returns new tokens and persists
- 401 retry: auth failure triggers refresh, second attempt succeeds
- Forward (Anthropic format) returns error

---

## 7. Out of Scope

- Account usage / billing API for OpenAI (noop — can be added later)
- Device code flow for headless environments (can be added later)
- OpenAI Responses API (`/v1/responses`) — only `/v1/chat/completions`
- Multi-account load balancing (already works via multiple `[[providers]]` entries)
- WebSocket support (can be added later if needed)

---

## 8. Research References

- **CLIProxyAPI** (33k ⭐): https://github.com/router-for-me/CLIProxyAPI — Go proxy that wraps Codex OAuth. Our approach mirrors its auth pattern.
- **OpenAI Codex Auth docs**: https://developers.openai.com/codex/auth — Documents browser login, device code flow, token caching at `~/.codex/auth.json`.
- **Puter OAuth article**: https://developer.puter.com/tutorials/openai-oauth/ — Background on OpenAI OAuth surfaces and the Codex OAuth pattern.
- OpenAI has not yet banned third-party Codex OAuth usage (unlike Anthropic, which banned Claude Code OAuth tokens for third-party use in April 2026).
