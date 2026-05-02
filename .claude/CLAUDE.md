# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

A Rust workspace named **`cli-router`** with **2 binary apps** that share a common library:

- **`analysis`** — interactive Ratatui TUI that reads the OpenCode SQLite database at `~/.local/share/opencode/opencode.db` and Claude Code's JSONL session files, then renders token/cost usage as a ccusage-style dashboard. Menu-driven, not argv-driven.
- **`proxy`** — localhost HTTP proxy in front of LLM providers (Anthropic only today). Forwards `POST /v1/messages` to the upstream, captures token usage from streaming and non-streaming responses, and writes one row per request to a local SQLite file at `~/.local/share/cli-router/proxy.db`.

The proxy and the TUI are peers — independent applications that share domain types, ports, and a small set of pricing-related adapters via the `shared` crate.

## Commands

The workspace has 3 crates: `shared` (library), `analysis` (TUI binary), `proxy` (HTTP proxy binary). Crate directory names match package names one-for-one.

```bash
# Run a binary
cargo run -p proxy            # HTTP proxy (default 127.0.0.1:8787)
cargo run -p analysis         # TUI dashboard

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

Release binaries land at `target/release/{proxy,analysis}` after `cargo build --release --workspace`.

Tests live alongside code (`#[cfg(test)] mod tests`), as crate-level integration tests under `crates/<crate>/tests/`, and as `///` doc examples on public value-object constructors.

## Architecture (at a glance)

Three crates, two enforcement levels.

```
crates/
├── shared/         # library — code consumed by BOTH apps
│   └── src/
│       ├── domain/         entities, value objects, services (pure types)
│       ├── application/    ApplicationError, Clock + PricingRepository ports, shared test fakes
│       └── adapters/       AdapterError, SystemClock, SqlitePricingRepository,
│                           CompositePricingRepository, shared SQLite connection helpers
│
├── analysis/       # APP 1 — Ratatui TUI binary `analysis`
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
└── proxy/          # APP 2 — axum HTTP proxy binary `proxy`
    └── src/
        ├── domain/         RequestStart, RequestUsage, UsageRecord, RequestStatus
        ├── application/    Provider + RequestLogPort + UsageParser ports,
        │                   HandleMessages use case + 5 unit tests
        ├── adapters/       AnthropicProvider, SqliteRequestLogRepository,
        │                   AnthropicSseParser, schema migrations
        ├── frameworks/     framework ring — axum router, handler glue,
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

- **Cargo-level** between `shared` and the apps: `shared/Cargo.toml` has zero deps on `analysis` or `proxy`. The compiler refuses any reverse import. Apps depend on `shared` via `path = "../shared"`.
- **Module-level** within each app: rings (`domain/`, `application/`, `adapters/`, `frameworks/`) are convention-enforced — `cargo` doesn't catch a `crate::frameworks::*` import inside `crate::application/`, but the spec defines it as a violation and `rg` greps catch it in code review.

## Rules

Detailed conventions live in `.claude/rules/`:

- [`data-shape.md`](rules/data-shape.md) — SQL/JSON invariants every query relies on (assistant-role filter, epoch-ms timestamps, COALESCE, session vs. message metadata, two-key pricing lookup).
- [`module-boundaries.md`](rules/module-boundaries.md) — what belongs in which ring and why.
- [`rust-style.md`](rules/rust-style.md) — repo-specific Rust conventions (per-ring error types, `#[allow(dead_code)]` query helpers, dependency policy).

## Design docs

- **Workspace + proxy MVP** — spec `docs/superpowers/specs/2026-05-02-workspace-and-proxy-mvp-design.md`, plan `docs/superpowers/plans/2026-05-02-workspace-and-proxy-mvp.md`.
- **Proxy clean-architecture refactor** — spec `docs/superpowers/specs/2026-05-02-proxy-clean-architecture-design.md`, plan `docs/superpowers/plans/2026-05-02-proxy-clean-architecture.md`.

Consult these for motivation before changing data shapes, ring boundaries, or proxy contracts.
