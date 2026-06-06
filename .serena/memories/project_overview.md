# cli-router

Rust workspace with 3 binary apps + 2 library crates. Built with axum (proxy), Ratatui (TUIs), and clean-architecture rings.

## Crates

- **`proxy`** — localhost HTTP proxy in front of LLM providers (Anthropic, Z.ai, DeepSeek, OpenAI, Codex, MiniMax). Multi-provider routing with glob model match, `provider/model` namespace override, round-robin load balancing with 429 cooldown, affinity-based session stickiness, priority. **SQLite-backed config** (replaces TOML as single source of truth) — `DbConfigRepository` reads/writes normalized config tables in the same `proxy.db`. Admin API for live config editing (`GetStatus`, `GetConfig`, `UpdateConfig`, `TestProvider`, `GetRecentRequests`, `GetUsageSummary`, `GetAccountUsage`, `GetQuotaStatus`). OAuth flows for Anthropic (PKCE) and OpenAI with background token refresh + 401 retry. Hot reload via `LiveProvider`. Accepts both Anthropic (`POST /v1/messages`) and OpenAI (`POST /v1/chat/completions`) formats with cross-format translation (Anthropic↔OpenAI). Token counting endpoint (`POST /v1/messages/count_tokens`) with upstream forwarding and local estimation fallback. Swagger/OpenAPI docs via utoipa. Captures token usage to SQLite (`~/.local/share/cli-router/proxy.db`).
- **`proxy-tui`** — Ratatui admin client for the proxy. Status, config editor (tabbed: Providers/Routing/Quotas/Settings, dual-mode: structured forms or raw TOML), Account view (balances, quota, per-model breakdown), Usage view (aggregate summaries), OAuth flows (Anthropic + OpenAI), provider testing, first-run wizard.
- **`analysis`** — Ratatui TUI reading OpenCode SQLite (`~/.local/share/opencode/opencode.db`) and Claude Code JSONL sessions; renders ccusage-style dashboards (cost, tokens, models, projects). Menu-driven. Has both `lib` and `bin` targets.
- **`shared`** — domain types, ports (`Clock`, `PricingRepository`), pricing adapters (`SqlitePricingRepository`, `CompositePricingRepository`), shared SQLite helpers. Used by all apps.
- **`proxy-admin-api`** — wire DTOs (`AuthPayload`, `ConfigPayload`, `StatusResponse`, `AccountUsageResponse`, `QuotaStatusResponse`, `UsageSummaryResponse`, `StartOAuthResponse`, …) with `utoipa::ToSchema` derives. Pure data + serde, zero logic. Keeps daemon and TUI in sync.

## Tech stack

Rust **edition 2024**, toolchain `stable` (pinned via `rust-toolchain.toml`, no MSRV). Key deps: axum, tokio, reqwest (rustls), rusqlite (bundled), ratatui, crossterm, serde, globset, ureq (pricing sync), thiserror, tracing, uuid, async-trait, futures, bytes, chrono, http, clap, base64, sha2, siphasher, tower, tower-http, utoipa, utoipa-redoc, utoipa-swagger-ui. Dev: wiremock, proptest.

No `anyhow` — typed error enum per ring is deliberate. Edition 2024 means let-chains (`if let Some(x) = opt && cond`) are idiomatic and `std::env::set_var` requires `unsafe`.

## Architecture

Clean-architecture rings inside each app, dependencies point inward:

```
frameworks/tui  →  adapters  →  application  →  domain
```

- **`domain/`** — entities, value objects, pure services. `std` + `chrono` only.
- **`application/`** — use cases + port traits. `Arc<dyn Port + Send + Sync>` for DI.
- **`adapters/`** — concrete port impls (gateways, presenters, view models).
- **`frameworks/`** (or `tui/`) — drivers (axum / ratatui / crossterm).
- **`main.rs`** — composition root, only file constructing concrete types.

Two enforcement levels: cargo-level between libraries and apps (compiler refuses reverse imports); module-level within each app (convention + code review).

### Proxy adapter layout

- `adapters/providers/` — `AnthropicProvider`, `ZaiProvider`, `DeepSeekProvider` (OpenAI-only, `https://api.deepseek.com/v1`), `OpenAiProvider` (OpenAI-compatible, OAuth support), `CodexProvider` (Codex CLI compatible, `CodexAuto` auth, configurable `reasoning_effort`), `MinimaxProvider` (MiniMax with configurable `thinking_mode`, `reasoning_split` injection, thinking content stripping), `minimax_stream` (stream/buffer thinking content filters with `ThinkingMode::SplitOnly` / `ThinkingMode::StripAll`), `RoutingProvider` (glob + namespace + load-balancing), `LiveProvider` (hot reload), `builder` (constructs providers from config), `affinity` (conversation hashing for session stickiness), `messages_protocol` (shared `forward`/`forward_openai`), `token_refresh` (background OAuth refresh).
  - `adapters/providers/account_usage/` — `AnthropicAccountUsage`, `ZaiAccountUsage`, `DeepSeekAccountUsage`, `CodexAccountUsage` (fetches Codex rate limits), `MinimaxAccountUsage` (fetches MiniMax CN/international balance), `NoopAccountUsage` (OpenAI uses noop; fetches provider balance/usage APIs).
- `adapters/oauth/` — `anthropic` (PKCE flow), `openai` (OAuth flow).
- `adapters/storage/` — `SqliteRequestLogRepository`, `DbConfigRepository` (SQLite-backed config, single source of truth), schema migrations.
- `adapters/usage/` — `AnthropicSseParser`.
- `adapters/translation/` — Cross-format translation between Anthropic and OpenAI protocols. `anthropic_to_openai/` (request, response, stream converters), `openai_to_anthropic/` (request, response, stream converters), `stream_wrap.rs`.
- `adapters/quota/` — quota enforcement logic.

### Proxy domain

`crates/proxy/src/domain/` — `RequestStart`, `RequestUsage`, `UsageRecord`, `UsageState`, `RequestStatus`, `ProviderAccountUsage`, `UsageSummary`, `DailyTotal`, `ModelTotal`, `QuotaSnapshot`, `QuotaCheck`, `AccountUsageStatus`, `ModelBreakdownItem`.

### Proxy application ports

`crates/proxy/src/application/ports/` — `Provider` (upstream forwarding with `ApiFormat`, `Direction`, `UpstreamResponse`, `BoxedByteStream`), `AccountUsagePort`, `ConfigRepository` (load/save config from DB), `RequestLogPort`, `RequestLogReadPort` (with `QuotaSeedRow`, `TranslationCounts`, `ModelBreakdownRow`), `UsageParser`, `QuotaPort`.

### Proxy admin use cases

- `GetStatus` — uptime and request counts
- `GetConfig` / `UpdateConfig` — config read/write (backed by DB) with hot reload
- `GetRecentRequests` — paginated recent request log
- `GetUsageSummary` — aggregate usage (daily totals, per-model breakdowns)
- `GetAccountUsage` — provider account balances and per-model breakdown from upstream APIs
- `GetQuotaStatus` — per-provider quota health (remaining, reset time)
- `TestProvider` — connectivity test for a named provider
- `StartAnthropicOAuth` / `CompleteAnthropicOAuth` — PKCE OAuth flow for Anthropic
- `StartOpenAiOAuth` / `CompleteOpenAiOAuth` — OAuth flow for OpenAI

### Proxy frameworks

`crates/proxy/src/frameworks/` — `server` (builds axum Router), `handler` (route handlers for `/v1/messages`, `/v1/messages/count_tokens`, `/v1/chat/completions`), `admin` (admin route handlers), `openapi` (utoipa OpenAPI spec, Swagger UI / ReDoc), `stream` (`TeedStream`), `error` (`ProxyError` IntoResponse).

### Proxy config

TOML `config.rs` defines config types but **SQLite is the single source of truth** (via `DbConfigRepository`). TOML is used only for the `--import-config` one-time migration flag. Multi-provider config with routing rules, `${ENV}` interpolation (env vars sourced from `~/.config/cli-router/.env` for the launchd service). `AuthConfig` variants: `Passthrough`, `ApiKey`, `Bearer`, `AnthropicOAuth`, `OpenAiOAuth`, `CodexAuto`. `ProviderKind`: `anthropic`, `zai`/`z.ai`/`z-ai`, `deepseek`/`deep_seek`, `openai`/`open_ai`, `codex`, `minimax`. Config fields include `docs_port` and `docs_enabled` for the OpenAPI docs server, `reasoning_effort` (per-provider, codex + anthropic only, values like `high`/`low`), `thinking_mode` (per-provider, minimax only, `SplitOnly` default or `StripAll`), and `openai_base_url` (per-provider override for OpenAI-compatible endpoint).

## Rules (in `.claude/rules/`)

- `data-shape.md` — SQL/JSON invariants (assistant-role filter, epoch-ms timestamps, COALESCE, two-key pricing lookup).
- `module-boundaries.md` — what belongs in which ring.
- `rust-style.md` — per-ring error enums, dependency policy, DI conventions, edition-2024 idioms.