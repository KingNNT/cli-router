# Kimi (Moonshot) provider — design

**Date:** 2026-07-14
**Status:** Approved

## Goal

Add **Kimi** (Moonshot AI) as a first-class provider in the proxy, alongside the
existing Anthropic / Zai / DeepSeek / OpenAI / Codex / MiniMax providers. Kimi is
reachable through both an OpenAI-compatible endpoint and an Anthropic-compatible
endpoint, so the provider mirrors the dual-endpoint `ZaiProvider` shape.

## Decisions

- **Dual endpoint (like Zai).** `KimiProvider` carries both a `base_url`
  (Anthropic path) and an `openai_base_url` (OpenAI path).
- **Region: global (`.ai`).** Default base URLs point at `api.moonshot.ai`.
- **`native_format() = ApiFormat::OpenAI`** — mirror `ZaiProvider`. Incoming
  Anthropic requests from Claude Code are translated to OpenAI and sent to the
  `/v1` endpoint; the Anthropic passthrough path remains available via `forward()`.
- **Account balance tracking** via Moonshot's balance API, mirroring
  `DeepSeekAccountUsage`.
- **serde aliases:** both `"kimi"` and `"moonshot"` parse to `ProviderKind::Kimi`.

## Components

### 1. Provider core — `crates/proxy/src/adapters/providers/kimi.rs` (new)

Mirror `ZaiProvider` one-for-one:

```rust
const DEFAULT_BASE_URL: &str = "https://api.moonshot.ai/anthropic";
const DEFAULT_OPENAI_BASE_URL: &str = "https://api.moonshot.ai/v1";

pub struct KimiProvider {
    base_url: String,
    openai_base_url: Option<String>,
    http: reqwest::Client,
    auth: AuthHeader,
}
```

- Constructors: `new`, `with_base_url`, `with_auth`, `configure(http, base_url,
  openai_base_url, auth)`, private `build`.
- `Provider` impl:
  - `name()` → `"kimi"`
  - `native_format()` → `ApiFormat::OpenAI`
  - `parse_model` / `usage_parser` / `parse_usage_json` /
    `usage_parser_openai` / `parse_usage_json_openai` → delegate to
    `messages_protocol` (OpenAI variants, same as Zai/DeepSeek).
  - `forward()` → `messages_protocol::forward` against `base_url` (Anthropic passthrough).
  - `forward_openai()` → resolve `openai_base_url` (error if absent), convert
    `AuthHeader::ApiKey → Bearer`, then `messages_protocol::forward`.
- Register in `providers/mod.rs`: `pub mod kimi;` + `pub use kimi::KimiProvider;`.

### 2. Account usage — `crates/proxy/src/adapters/providers/account_usage/kimi.rs` (new)

Mirror `DeepSeekAccountUsage`:

- Struct holds `provider_name`, `auth_token`, `ureq::Agent` (10s timeouts).
- `fetch_usage()`:
  - Return `None` when `auth_token` is empty.
  - `GET https://api.moonshot.ai/v1/users/me/balance` with
    `Authorization: Bearer <token>` and `Accept: application/json`.
  - Decode envelope; map to a single balance `UsageWindow`:

    ```json
    { "code": 0, "data": { "available_balance": 49.58,
                            "voucher_balance": 46.58,
                            "cash_balance": 3.00 },
      "status": true }
    ```

  - `UsageWindow { label: "Balance", used_pct: 0.0, used: None, limit: None,
    resets_at_ms: None, is_balance_info: true, sub_items: [available, cash,
    voucher] }`.
  - Errors → `ProxyError::UpstreamUsage { provider, message }`.
- Register in `account_usage/mod.rs`: `pub mod kimi;` + `pub use kimi::KimiAccountUsage;`.

### 3. Config enum — `crates/proxy/src/config.rs`

- Add `Kimi` variant to `ProviderKind` with `#[serde(alias = "kimi", alias = "moonshot")]`.
- `parse_kind`: `"kimi" | "moonshot" => Some(ProviderKind::Kimi)`.

### 4. Wiring — `crates/proxy/src/adapters/providers/builder.rs`

- `build_provider`: `ProviderKind::Kimi => Arc::new(KimiProvider::configure(http,
  p.base_url.clone(), p.openai_base_url.clone(), auth))` (identical to the Zai arm).
- `build_account_usage`: `ProviderKind::Kimi => { let token =
  resolve_auth_token(&p.auth); Arc::new(KimiAccountUsage::new(p.name.clone(), token)) }`.

### 5. Kind string round-trip

- `adapters/storage/db_config.rs`: `"kimi" | "moonshot" => Kimi` and `Kimi => "kimi"`.
- `application/use_cases/admin.rs`: `Kimi => "kimi"` and `"kimi" => Ok(Kimi)`.

### 6. proxy-tui — `crates/proxy-tui/`

- `app.rs`: add `Kimi` to the TUI `ProviderKind`, plus `as_str` (`"kimi"`),
  `cycle_next` / `cycle_prev` (insert into the cycle order), and the from-str parse arm.
- `main.rs`: suggested test model `"kimi" => "kimi-k2-0711-preview"`.
- Review `ui.rs` provider fallback list; extend if it enumerates known kinds.

### 7. Tests

- `kimi.rs`: `name()` == `"kimi"`; base-URL defaults.
- `config.rs`: `parse_kind` accepts `"kimi"` and `"moonshot"`.
- `app.rs`: `cycle_next` / `cycle_prev` round-trip including `Kimi`.
- `account_usage/kimi.rs`: balance JSON decodes and maps to a balance window.

## Non-goals

- OAuth flows (Moonshot uses a static API key).
- Model-specific request munging (no `tool_choice` stripping like DeepSeek unless
  a concrete Kimi model is found to reject it).
- Changing routing/affinity — no per-kind logic exists there today.

## Enforcement note

`build_provider` and `build_account_usage` match `ProviderKind` exhaustively (no
`_` arm), so the compiler flags every site that must gain a `Kimi` arm. Use
`cargo check --workspace` as the completeness checklist.
