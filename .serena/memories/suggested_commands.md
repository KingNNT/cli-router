# Commands

## Run binaries
- `cargo run -p proxy` — HTTP proxy (default `127.0.0.1:8787`)
- `cargo run -p proxy-tui` — proxy admin TUI (proxy must be running)
- `cargo run -p analysis` — usage dashboard TUI

## Workspace gates
- `cargo build --workspace` — build everything
- `cargo check --workspace` — fast type-check
- `cargo test --workspace` — unit + integration + doc tests
- `cargo clippy --workspace -- -D warnings`
- `cargo fmt`

## Targeted tests
- `cargo test -p proxy` — proxy crate tests only
- `cargo test -p proxy -- routing::tests` — routing module tests
- `cargo test -p proxy -- <test_name>` — specific test by substring

## Coverage (requires `cargo install cargo-llvm-cov`)
- `cargo coverage` — terminal summary
- `cargo coverage-html` — open HTML report

## Release
- `cargo build --release --workspace` — binaries land at `target/release/{proxy,analysis,proxy-tui}`

## Platform
Darwin/macOS, zsh. Standard git available. Pricing sync hits the network via `ureq` (rustls, blocking). Proxy uses `reqwest` over rustls.
