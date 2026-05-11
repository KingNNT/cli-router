# cli-router

Rust workspace with 3 binary apps + 2 library crates. Built with axum (proxy), Ratatui (TUIs), and clean-architecture rings.

## Crates

- **`proxy`** — localhost HTTP proxy in front of LLM providers (Anthropic, Z.ai, DeepSeek). DeepSeek has its own `DeepSeekProvider` (OpenAI-only, at `https://api.deepseek.com/v1`). It does NOT support the Anthropic messages format — `forward()` returns an error. Uses `NoopAccountUsage` since DeepSeek has no account-usage API. Multi-provider routing with glob model match, `provider/model` namespace override, round-robin load balancing with 429 cooldown, priority. Admin API for live config editing. Anthropic OAuth PKCE flow with background token refresh + 401 retry. Hot reload via `LiveProvider`. Accepts both Anthropic (`POST /v1/messages`) and OpenAI (`POST /v1/chat/completions`). Captures token usage to SQLite (`~/.local/share/cli-router/proxy.db`).
- **`proxy-tui`** — Ratatui admin client for the proxy. Status, config editor, OAuth flow, provider testing.
- **`analysis`** — Ratatui TUI reading OpenCode SQLite (`~/.local/share/opencode/opencode.db`) and Claude Code JSONL sessions; renders ccusage-style dashboards (cost, tokens, models, projects). Menu-driven.
- **`shared`** — domain types, ports (`Clock`, `PricingRepository`), pricing adapters (`SqlitePricingRepository`, `CompositePricingRepository`), shared SQLite helpers. Used by all apps.
- **`proxy-admin-api`** — wire DTOs (`AuthPayload`, `ConfigPayload`, `StatusResponse`, …). Pure data + serde, zero logic. Keeps daemon and TUI in sync.

## Tech stack

Rust **edition 2024**, toolchain `stable` (pinned via `rust-toolchain.toml`, no MSRV). Key deps: axum, tokio, reqwest (rustls), rusqlite (bundled), ratatui, crossterm, serde, globset, ureq (pricing sync), thiserror, tracing, uuid, async-trait, futures, bytes, chrono, http. Dev: wiremock.

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

- `adapters/providers/` — `AnthropicProvider`, `ZaiProvider`, `RoutingProvider` (glob + namespace + load-balancing), `LiveProvider` (hot reload), `builder`, `messages_protocol` (shared `forward`/`forward_openai`), `token_refresh` (background OAuth refresh).
- `adapters/oauth/` — Anthropic PKCE flow.
- `adapters/storage/` — `SqliteRequestLogRepository`, schema migrations.
- `adapters/usage/` — `AnthropicSseParser`.

### Proxy domain

`crates/proxy/src/domain/` — `RequestStart`, `RequestUsage`, `UsageRecord`, `RequestStatus` (in `request_log.rs`, `usage_record.rs`).

### Proxy config

Both `config.rs` and `config/` directory exist. TOML config with multi-provider, routing rules, `${ENV}` interpolation (env vars sourced from `~/.config/cli-router/.env` for the launchd service), but `std::env::var` doesn't see launchd plist vars on macOS — `interpolate()` falls back to reading `~/.config/cli-router/.env` directly., and `AuthConfig` variants (`Passthrough`, `ApiKey`, `Bearer`, `AnthropicOAuth`). Lives at `~/.config/cli-router/config.toml`.

## Rules (in `.claude/rules/`)

- `data-shape.md` — SQL/JSON invariants (assistant-role filter, epoch-ms timestamps, COALESCE, two-key pricing lookup).
- `module-boundaries.md` — what belongs in which ring.
- `rust-style.md` — per-ring error enums, dependency policy, DI conventions, edition-2024 idioms.
