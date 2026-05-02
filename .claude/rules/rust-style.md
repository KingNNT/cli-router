# Rust style (repo-specific)

General Rust style follows `rustfmt` defaults (`cargo fmt`) and `clippy` (`cargo clippy`). The notes below are conventions that are specific to this crate.

## Error types

One error enum per ring, each wrapping the one it sits outside of. Use `thiserror` derives.

- `domain::DomainError` — invalid value-object construction.
- `application::ApplicationError` — `Repository(AdapterError)`, `InvalidInput(String)`, `Domain(DomainError)`.
- `adapters::AdapterError` — `Database(rusqlite::Error)`, `Http(Box<ureq::Error>)`, `DataMapping(String)`, `InvalidDomainValue(DomainError)`.
- `frameworks::FrameworkError` — `Io(std::io::Error)`, `Terminal(String)`, `Application(ApplicationError)`.

Cross-ring conversions use `#[from]`. Do not add `anyhow` — the typed error per layer is deliberate for testability and boundary clarity.

## Dependency policy

| crate       | why                                                |
| ----------- | -------------------------------------------------- |
| `rusqlite`   | DB access. Use the `bundled` feature.              |
| `ratatui`    | TUI rendering.                                     |
| `crossterm`  | input events, terminal control.                    |
| `chrono`     | `NaiveDate` in domain; `Local` in `SystemClock`.   |
| `thiserror`  | per-layer error enums.                             |
| `ureq`       | HTTP GET for pricing sync. Blocking, rustls, no tokio. |
| `serde`      | Derive `Deserialize` for LiteLLM JSON parsing.     |
| `serde_json` | JSON parsing.                                      |
| `axum`       | HTTP server framework for the proxy.               |
| `http`       | Header/status type-only crate; admitted into `application` ports so they don't depend on a specific HTTP framework. |
| `tokio`      | async runtime (full features).                     |
| `tokio-util` | async stream/IO helpers.                           |
| `reqwest`    | outbound HTTP client to LLM providers (rustls, stream feature). |
| `tower-http` | middleware layer (request tracing).                |
| `tracing`    | structured logging.                                |
| `tracing-subscriber` | tracing subscriber + env-filter.           |
| `uuid`       | request IDs (v4).                                  |
| `async-trait` | async methods on `Provider` trait.                |
| `bytes`      | shared byte-buffer type used by reqwest/axum streams. |
| `futures`    | stream combinators in proxy.                       |
| `wiremock` (dev) | upstream mock for proxy integration tests.     |

Prefer adding logic over adding dependencies. Anything new needs a reason that can't be solved with a few lines of Rust.

## Dependency injection

Use `Arc<dyn Port + Send + Sync>` for ports. Generics were considered and rejected — storing multiple use cases in the controller would force generic parameters to propagate through `AppState` with no real benefit at CLI scale.

## Formatting helpers

All user-facing formatting (`fmt_num`, `fmt_cost`, `format_date_*`, `trunc_model`) lives in `adapters::presenters::formatting`. Domain types never implement `Display` for user-facing presentation — that's a presenter concern. `Display` may be implemented on value objects for `Debug`-adjacent purposes (e.g. `ModelId`, `ProjectPath` as raw strings).

## `#[allow(dead_code)]` is not a code smell here

Some ports and DTOs carry fields (`provider`, `session_id` on `Filter`; `reasoning` on `TokenBreakdown`) that aren't yet consumed by any use case but are part of the stable port surface. Do not delete them to silence the linter. When a new use case wires them up, remove the attribute in the same commit as the new call site (if any `#[allow]` attributes get added in the future — current code compiles clean).

## Interactive vs. scripted

The tool is interactive by design. Don't add CLI flag parsing (`clap`, hand-rolled `std::env::args`) without a discussion — it changes the UX contract documented in the spec.
