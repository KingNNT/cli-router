# cli-router

A Rust workspace with three binary apps and two shared library crates:

- **`proxy`** — localhost HTTP proxy in front of LLM providers (Anthropic, Z.ai, DeepSeek, OpenAI, Codex, MiniMax) with multi-provider routing, admin API, OAuth flows for Anthropic and OpenAI with automatic token refresh, cross-format translation (Anthropic↔OpenAI), Swagger/OpenAPI docs, and hot reload. Captures token usage to SQLite per request. Config stored in SQLite (single source of truth).
- **`proxy-tui`** — terminal admin client for the proxy. View status, edit config, manage providers, test connectivity, and run OAuth flows (Anthropic and OpenAI) — all from the terminal.
- **`analysis`** — terminal UI for analysing OpenCode and Claude Code usage (costs, token counts, model breakdowns, project insights). Companion to the proxy for historical analysis.

Shared libraries:
- **`shared`** — domain types, pricing logic, and adapters used by all apps.
- **`proxy-admin-api`** — wire types (DTOs) that keep the proxy daemon and the TUI client in sync.

Built with [axum](https://github.com/tokio-rs/axum) (proxy), [Ratatui](https://ratatui.rs/) (TUIs), and clean-architecture principles.

## Features

### `proxy` — HTTP proxy with usage capture and admin control plane

- **Multi-provider routing** — configure multiple LLM providers (Anthropic, Z.ai, DeepSeek, OpenAI, Codex, MiniMax) with glob-based model matching and fallback chains. Override per-request with `provider-name/model` namespace syntax (e.g. `zai/glm-5`).
- **Dual-protocol support** — accepts both Anthropic (`/v1/messages`) and OpenAI (`/v1/chat/completions`) request formats with automatic cross-format translation. Works with Claude Code, OpenCode, Cursor, Codex CLI, and any OpenAI-compatible client.
- **Token counting** — `/v1/messages/count_tokens` endpoint with upstream forwarding and local estimation fallback.
- **Streaming support** — forwards SSE streaming and buffered JSON responses unchanged
- **Token usage logging** — parses upstream events to extract input/output/cache token counts, looks up cost, writes to `~/.local/share/cli-router/proxy.db`
- **SQLite-backed config** — config stored in the same SQLite database (single source of truth). Admin API reads/writes DB directly. TOML import via `--import-config` for migration only. Per-provider config includes `thinking_level`/`thinking_force` (upstream reasoning effort, per kind) and `thinking_mode` (MiniMax).
- **Admin API** — `GET/PUT /admin/config`, `GET /admin/status`, `GET /admin/requests/recent`, `GET /admin/usage/summary`, `GET /admin/account/usage`, `GET /admin/quota/status`, `POST /admin/providers/:name/test`, `POST /admin/oauth/anthropic/{start,complete}`, `POST /admin/oauth/openai/{start,complete}`
- **OAuth** — PKCE-based browser flow for Anthropic, OAuth flow for OpenAI, both with automatic token refresh (background task refreshes tokens every 60s, persists to DB). Codex `CodexAuto` auth reads from `~/.codex/auth.json`.
- **401 retry** — on auth failure, automatically refreshes OAuth token and retries once
- **Hot reload** — config changes via admin API take effect immediately without daemon restart
- **Swagger/OpenAPI docs** — optional docs server (configurable `docs_port`) with utoipa annotations on all handlers, Swagger UI, and ReDoc

### `proxy-tui` — admin terminal client

- **Status view** — uptime, request counts by provider and status
- **Config editor** — organized tabs (Providers, Routing, Quotas, Settings) with keyboard and mouse navigation. Structured forms for all config fields.
- **Account view** — provider balances, quota status, and per-model usage breakdown
- **Usage view** — aggregate usage summaries (daily totals, per-model breakdowns) from the proxy request log
- **OAuth flow** — start/complete Anthropic and OpenAI OAuth with browser-based flows
- **Provider testing** — ping any configured provider to verify connectivity
- **First-run wizard** — guided setup when no config file exists

### `analysis` — usage dashboard

- **Dashboard** — at-a-glance overview of usage and spend
- **Models breakdown** — per-model cost and token analysis
- **Projects breakdown** — usage aggregated by project path
- **Pricing sync** — fetches live pricing from LiteLLM
- **Dual data sources** — reads from both the local OpenCode SQLite database and Claude Code's JSONL session files

## Prerequisites

- Rust stable (pinned via `rust-toolchain.toml`)
- For the proxy: an API key or OAuth session for at least one provider (Anthropic, Z.ai, DeepSeek, OpenAI, Codex, or MiniMax)
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

Release binaries: `target/release/cli-router-proxy`, `target/release/cli-router-proxy-tui`, and `target/release/cli-router-analysis` after `cargo build --release --workspace`.

## Task runner

The project uses [mise](https://mise.jdx.dev/) for development, install, and service tasks:

```bash
mise run dev:proxy          # Run proxy in foreground against dev DB
mise run dev:proxy-tui      # Run proxy-tui against dev proxy
mise run dev:init           # Clone prod DB into dev
mise run dev:reset          # Delete dev DBs and re-clone from prod
mise run prod:apps:install  # Install all binaries into ~/.cargo/bin
mise run prod:deploy        # Install binaries and register service
mise run prod:service:status     # Show service status (LaunchAgent on macOS, systemd --user on Linux)
mise run prod:service:logs       # Tail service logs (log files on macOS, journalctl on Linux)
```

Run `mise tasks` for the full list.

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
