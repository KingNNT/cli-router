# cli-router

A Rust workspace with two binary apps that share a common library:

- **`proxy`** — localhost HTTP proxy in front of LLM providers (Anthropic today) that captures token usage to a SQLite log per request.
- **`analysis`** — terminal UI for analysing OpenCode and Claude Code usage (costs, token counts, model breakdowns, project insights). Companion to the proxy for historical analysis.

Both apps share domain types, pricing logic, and a small set of adapters via the `shared` library crate.

Built with [axum](https://github.com/tokio-rs/axum) (proxy), [Ratatui](https://ratatui.rs/) (TUI), and clean-architecture principles.

## Features

### `proxy` — HTTP proxy with usage capture

- Forwards `POST /v1/messages` to `https://api.anthropic.com` (configurable upstream)
- Streams responses back unchanged (SSE for streaming, JSON for non-streaming)
- Parses upstream events to extract input/output/cache token counts
- Looks up cost via the same pricing infrastructure the TUI uses
- Writes one row per request to `~/.local/share/cli-router/proxy.db`
- Drops a request row to `errored` if the client disconnects mid-stream

### `analysis` — TUI dashboard

- **Dashboard** — at-a-glance overview of usage and spend
- **Models breakdown** — per-model cost and token analysis
- **Projects breakdown** — usage aggregated by project path
- **Pricing sync** — fetches live pricing from LiteLLM
- **Dual data sources** — reads from both the local OpenCode SQLite database and Claude Code's JSONL session files

## Prerequisites

- Rust stable (pinned via `rust-toolchain.toml`)
- For the proxy: an Anthropic API key (forwarded from the client; the proxy never reads it)
- For the TUI: a working OpenCode or Claude Code installation with usage data

## Build & run

```sh
# Build everything
cargo build --workspace

# Run the proxy
cargo run -p proxy
# Then point your tool at it:
ANTHROPIC_BASE_URL=http://127.0.0.1:8787 claude --print "hi"

# Run the TUI
cargo run -p analysis
```

Release binaries: `target/release/proxy` and `target/release/analysis` after `cargo build --release --workspace`.

## Architecture

The workspace has three crates:

```
crates/
├── shared/      ← library: types and adapters used by both apps
├── analysis/    ← TUI binary (analysis)
└── proxy/       ← HTTP proxy binary (proxy)
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

- **Cargo-level** between `shared` and the apps: the compiler refuses any `analysis → proxy` or `proxy → analysis` import.
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
