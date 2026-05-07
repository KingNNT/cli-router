# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

A Rust workspace named **`cli-router`** with **3 binary apps** and **2 library crates**:

- **`analysis`** — interactive Ratatui TUI that reads the OpenCode SQLite database at `~/.local/share/opencode/opencode.db` and Claude Code's JSONL session files, then renders token/cost usage as a ccusage-style dashboard. Menu-driven, not argv-driven.
- **`proxy`** — localhost HTTP proxy in front of LLM providers (Anthropic, Z.ai). Multi-provider routing with glob-based model matching, `provider/model` namespace overrides, round-robin load balancing with 429 cooldown, admin API for live config editing, Anthropic OAuth PKCE flow with automatic token refresh, and hot reload. Accepts both Anthropic (`POST /v1/messages`) and OpenAI (`POST /v1/chat/completions`) formats, captures token usage from streaming and non-streaming responses, and writes one row per request to a local SQLite file.
- **`proxy-tui`** — Ratatui admin client for the proxy daemon. Connects to the proxy's admin API to view status, edit config, manage providers, test connectivity, and initiate OAuth flows.

Shared libraries:
- **`shared`** — domain types, ports, and pricing adapters used by both apps.
- **`proxy-admin-api`** — wire types (DTOs) for the proxy admin API. Pure data + serde, zero logic. Both the daemon and the TUI depend on this crate so on-the-wire shapes stay in sync.

## Commands

The workspace has 5 crates. Crate directory names match package names one-for-one.

```bash
# Run a binary
cargo run -p proxy            # HTTP proxy (default 127.0.0.1:8787)
cargo run -p analysis         # TUI dashboard
cargo run -p proxy-tui        # Proxy admin TUI

# Workspace gates
cargo build --workspace       # build everything
cargo check --workspace       # fast type-check
cargo test --workspace        # unit + integration + doc tests
cargo clippy --workspace -- -D warnings
cargo fmt

# Coverage (requires `cargo install cargo-llvm-cov`)
cargo coverage                # summary in terminal
cargo coverage-html           # open HTML report
```

Release binaries land at `target/release/{cli-router-proxy,analysis,cli-router-proxy-tui}` after `cargo build --release --workspace`.

Tests live alongside code (`#[cfg(test)] mod tests`), as crate-level integration tests under `crates/<crate>/tests/`, and as `///` doc examples on public value-object constructors.

## Architecture (at a glance)

Five crates, two enforcement levels.

```
crates/
├── shared/              # library — code consumed by BOTH apps
│   └── src/
│       ├── domain/         entities, value objects, services (pure types)
│       ├── application/    ApplicationError, Clock + PricingRepository ports, shared test fakes
│       └── adapters/       AdapterError, SystemClock, SqlitePricingRepository,
│                           CompositePricingRepository, shared SQLite connection helpers
│
├── analysis/            # APP 1 — Ratatui TUI binary `analysis`
│   └── src/
│       ├── application/    UsageRepository + PricingSource ports,
│       │                   GetDashboard / GetModelsBreakdown / GetProjectsBreakdown /
│       │                   GetPricing / SyncPricing use cases, dtos, FakeUsageRepository
│       ├── adapters/       SqliteUsageRepository, ClaudeCodeUsageRepository,
│       │                   DispatchingUsageRepository, LiteLlmPricingSource,
│       │                   presenters, view models
│       ├── tui/            framework ring — ratatui renderer, AppState, event loop,
│       │                   terminal setup, controllers
│       └── main.rs         composition root
│
├── proxy-admin-api/     # library — shared DTOs for admin API
│   └── src/lib.rs          AuthPayload, ConfigPayload, StatusResponse, etc.
│
├── proxy-tui/           # APP 3 — Ratatui admin client binary `proxy-tui`
│   └── src/
│       ├── app.rs          TUI state machine (EditAuthModal, OAuthAwaitingCode, etc.)
│       ├── client.rs       HTTP client for the proxy admin API
│       ├── ui.rs           Ratatui rendering (status, config, edit modals)
│       ├── terminal.rs     terminal setup/teardown
│       └── main.rs         event loop
│
└── proxy/               # APP 2 — axum HTTP proxy binary `proxy`
    └── src/
        ├── domain/         RequestStart, RequestUsage, UsageRecord, RequestStatus
        ├── application/    Provider + RequestLogPort + UsageParser ports,
        │                   HandleMessages use case (with ApiFormat for dual-protocol),
        │                   admin use cases (GetStatus, GetConfig, UpdateConfig,
        │                   TestProvider, StartAnthropicOAuth, CompleteAnthropicOAuth)
        ├── adapters/
        │   ├── providers/  AnthropicProvider, ZaiProvider, RoutingProvider
        │   │               (glob match + namespace + load balancing),
        │   │               LiveProvider (hot reload), builder,
        │   │               messages_protocol (shared forward/forward_openai logic),
        │   │               token_refresh (background OAuth refresh)
        │   ├── oauth/      Anthropic PKCE flow
        │   ├── storage/    SqliteRequestLogRepository, schema migrations
        │   └── usage/      AnthropicSseParser
        ├── config.rs        TOML config with multi-provider, routing rules,
        │                   AuthConfig variants (Passthrough, ApiKey, Bearer, AnthropicOAuth)
        ├── frameworks/     framework ring — axum router (`/v1/messages`,
        │                   `/v1/chat/completions`, `/admin/*`), admin handler glue,
        │                   TeedStream, ProxyError IntoResponse
        └── main.rs         composition root
```

### Dependency rule (clean architecture)

Inside each app, four concentric rings — dependencies point strictly inward:

```
frameworks/tui  →  adapters  →  application  →  domain
```

- **`domain/`** — entities, value objects, pure services. Depends only on `std` + `chrono`.
- **`application/`** — use cases + ports (trait definitions). Depends only on `domain` and (where needed) the workspace `shared::application::ports::*` for cross-cutting ports like `Clock`, `PricingRepository`.
- **`adapters/`** — concrete implementations of ports (gateways, presenters, view models). Depends on `application` + `domain`.
- **`frameworks/`** (or `tui/` in analysis) — drivers (axum, ratatui, crossterm). Depends on all inner rings.
- **`main.rs`** — composition root. The only file that constructs concrete adapter/framework types and wires them together.

### Two enforcement levels

- **Cargo-level** between `shared`/`proxy-admin-api` and the apps: `shared/Cargo.toml` and `proxy-admin-api/Cargo.toml` have zero deps on `analysis` or `proxy`. The compiler refuses any reverse import. Apps depend on them via `path = "../shared"`.
- **Module-level** within each app: rings (`domain/`, `application/`, `adapters/`, `frameworks/`) are convention-enforced — `cargo` doesn't catch a `crate::frameworks::*` import inside `crate::application/`, but the spec defines it as a violation and `rg` greps catch it in code review.

### Key data flows

**Proxy request path:**
```
Client → axum handler (`/v1/messages` or `/v1/chat/completions`)
  → HandleMessages use case (ApiFormat::Anthropic | ApiFormat::OpenAI)
  → LiveProvider → RoutingProvider (namespace override → glob match
    → priority + round-robin load balancing with 429 cooldown)
  → AnthropicProvider/ZaiProvider
  → messages_protocol::forward / forward_openai (auth injection,
    streaming/buffered)
  → Upstream → response → usage logging
```

**OAuth token lifecycle:**
```
TUI → POST /admin/oauth/anthropic/start → PKCE codes generated
TUI → browser opens authorize URL → user pastes code back
TUI → POST /admin/oauth/anthropic/complete → exchange code for tokens
  → AuthConfig::AnthropicOAuth stored in config.toml
  → Background task (token_refresh.rs) refreshes every 60s, persists to disk
  → 401-retry safety net in messages_protocol::forward
```

## Rules

Detailed conventions live in `.claude/rules/`:

- [`data-shape.md`](rules/data-shape.md) — SQL/JSON invariants every query relies on (assistant-role filter, epoch-ms timestamps, COALESCE, session vs. message metadata, two-key pricing lookup).
- [`module-boundaries.md`](rules/module-boundaries.md) — what belongs in which ring and why.
- [`rust-style.md`](rules/rust-style.md) — repo-specific Rust conventions (per-ring error types, `#[allow(dead_code)]` query helpers, dependency policy).

## Design docs

- **Proxy usage guide** — `docs/proxy-usage.md` (config examples, OAuth, namespace routing, load balancing, priority).
- **OAuth refresh token support** — plan `docs/superpowers/plans/2026-05-03-oauth-refresh-token.md`.
- **Provider namespace routing** — spec `docs/superpowers/specs/2026-05-03-provider-namespace-design.md`, plan `docs/superpowers/plans/2026-05-03-provider-namespace.md`.
- **Load balancing** — spec `docs/superpowers/specs/2026-05-03-load-balancing-design.md`.
- **OpenAI chat completions endpoint** — spec `docs/superpowers/specs/2026-05-03-openai-chat-completions-design.md`, plan `docs/superpowers/plans/2026-05-03-openai-chat-completions.md`.

Consult these for motivation before changing data shapes, ring boundaries, or proxy contracts.
