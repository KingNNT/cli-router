# cli-router

A Rust workspace with three binary apps and two shared library crates:

- **`proxy`** — localhost HTTP proxy in front of LLM providers (Anthropic, Z.ai) with multi-provider routing, admin API, Anthropic OAuth with automatic token refresh, and hot reload. Captures token usage to SQLite per request.
- **`proxy-tui`** — terminal admin client for the proxy. View status, edit config, manage providers, test connectivity, and run OAuth flows — all from the terminal.
- **`analysis`** — terminal UI for analysing OpenCode and Claude Code usage (costs, token counts, model breakdowns, project insights). Companion to the proxy for historical analysis.

Shared libraries:
- **`shared`** — domain types, pricing logic, and adapters used by all apps.
- **`proxy-admin-api`** — wire types (DTOs) that keep the proxy daemon and the TUI client in sync.

Built with [axum](https://github.com/tokio-rs/axum) (proxy), [Ratatui](https://ratatui.rs/) (TUIs), and clean-architecture principles.

## Features

### `proxy` — HTTP proxy with usage capture and admin control plane

- **Multi-provider routing** — configure multiple LLM providers (Anthropic, Z.ai) with glob-based model matching and fallback chains. Override per-request with `provider-name/model` namespace syntax (e.g. `zai/glm-5`).
- **Dual-protocol support** — accepts both Anthropic (`/v1/messages`) and OpenAI (`/v1/chat/completions`) request formats. Works with Claude Code, OpenCode, Cursor, and any OpenAI-compatible client.
- **Streaming support** — forwards SSE streaming and buffered JSON responses unchanged
- **Token usage logging** — parses upstream events to extract input/output/cache token counts, looks up cost, writes to `~/.local/share/cli-router/proxy.db`
- **Admin API** — `GET/PUT /admin/config`, `GET /admin/status`, `GET /admin/requests/recent`, `POST /admin/providers/:name/test`
- **Anthropic OAuth** — PKCE-based browser flow with automatic token refresh (background task refreshes tokens every 60s, persists to config file)
- **401 retry** — on auth failure, automatically refreshes OAuth token and retries once
- **Hot reload** — config changes via admin API take effect immediately without daemon restart
- **Config file** — `~/.config/cli-router/config.toml` with `${ENV}` interpolation

### `proxy-tui` — admin terminal client

- **Status view** — uptime, request counts by provider and status
- **Config editor** — edit providers, auth, and routing rules from the terminal
- **OAuth flow** — start/complete Anthropic OAuth with browser-based PKCE flow
- **Provider testing** — ping any configured provider to verify connectivity

### `analysis` — usage dashboard

- **Dashboard** — at-a-glance overview of usage and spend
- **Models breakdown** — per-model cost and token analysis
- **Projects breakdown** — usage aggregated by project path
- **Pricing sync** — fetches live pricing from LiteLLM
- **Dual data sources** — reads from both the local OpenCode SQLite database and Claude Code's JSONL session files

## Prerequisites

- Rust stable (pinned via `rust-toolchain.toml`)
- For the proxy: an Anthropic API key or OAuth session (configured via TUI or config file)
- For the TUI: a working OpenCode or Claude Code installation with usage data

## Build & run

```sh
# Build everything
cargo build --workspace

# Run the proxy
cargo run -p proxy
# Then point your tool at it:
ANTHROPIC_BASE_URL=http://127.0.0.1:8787 claude --print "hi"

# Run the proxy admin TUI (proxy must be running)
cargo run -p proxy-tui

# Run the analysis TUI
cargo run -p analysis
```

Release binaries: `target/release/proxy`, `target/release/proxy-tui`, and `target/release/analysis` after `cargo build --release --workspace`.

## Architecture

The workspace has five crates:

```
crates/
├── shared/            ← library: domain types and adapters used by all apps
├── proxy-admin-api/   ← library: shared DTOs for the proxy admin API
├── analysis/          ← TUI binary (analysis)
├── proxy/             ← HTTP proxy binary (proxy)
└── proxy-tui/         ← admin TUI binary (proxy-tui)
```

Inside each app, four concentric rings — dependencies point strictly inward:

```
frameworks → adapters → application → domain
```

| Layer            | Responsibility                                                  |
|------------------|-----------------------------------------------------------------|
| **Domain**       | Entities, value objects, pure domain services                   |
| **Application**  | Use cases, port traits, DTOs                                    |
| **Adapters**     | Gateways (SQLite, HTTP, filesystem), presenters, view models    |
| **Frameworks**   | Driver layer (axum router / ratatui renderer + event loop)      |

Two enforcement levels:

- **Cargo-level** between shared libraries and the apps: the compiler refuses any `analysis → proxy` or `proxy → analysis` import.
- **Module-level** within each app: ring boundaries are convention-enforced and verified in code review.

See `.claude/CLAUDE.md` and `docs/superpowers/specs/` for design details.

## Tests

```sh
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo coverage          # summary (requires `cargo install cargo-llvm-cov`)
cargo coverage-html     # HTML report
```

## License

All rights reserved.
