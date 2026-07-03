# Module boundaries

The crate is organised as four concentric rings. Source dependencies point strictly inward: `frameworks → adapters → application → domain`. A layer must not `use` anything from a layer outside itself.

## `domain/` — entities, value objects, pure services

- No imports from other rings.
- May depend on `std` and `chrono` only.
- Entities (`UsageRecord`, `DailyUsage`, `ModelUsage`, `Overview`, `DayModelRow`) are plain data. Value objects (`Cost`, `TokenCount`, `TokenBreakdown`, `DateRange`, `ModelId`, `ProjectPath`) enforce invariants via constructors.
- Services (`aggregation`, `budget`, `forecast`, `aliases`) are pure functions or compile-time tables over entities.
- Const tables encoding business rules are fine (e.g. `services::aliases::ALIASES`). They're not infrastructure — they're a compile-time declaration of domain knowledge. Tests exercise them via `canonicalize` / `canonical_pricing` helpers.

## `application/` — use cases, ports, DTOs

- Depends on `domain` only.
- Ports (`UsageRepository`, `Clock`) are traits the outer layers implement.
- Use cases hold `Arc<dyn Port>` and are constructed in `main.rs`.
- DTOs (`Filter`, `Get<X>Input`, `Get<X>Output`) define the boundary surface. Each use case owns its Input/Output pair.
- `ApplicationError` wraps `AdapterError` via `#[from]`. This is the one permitted outward type reference — documented and ergonomic in Rust.

## `adapters/` — gateways, presenters, view models, clock

- Depends on `application` and `domain`.
- **Gateways** implement ports against concrete drivers. `adapters::gateways::sqlite::*` owns all `rusqlite` use and JSON extraction. `adapters::gateways::http::*` owns outbound HTTP via `ureq`. The WHERE-clause builder lives here, not in `frameworks`.
- **Composite adapters** — an adapter may *wrap* another adapter of the same port. `CompositePricingRepository` layers `domain::services::aliases::canonical_pricing()` output in front of `SqlitePricingRepository`. The composite itself is still an adapter; the port surface doesn't change.
- **Clock** (`adapters::clock::SystemClock`) implements `Clock` using `chrono::Local`.
- **Presenters** are pure functions DTO → view model. Formatting helpers (`fmt_num`, `fmt_cost`, `format_date_*`, `trunc_model`) live in `adapters::presenters::formatting`.
- **View models** are plain structs of pre-formatted `String`s. No domain types.

## `frameworks/` — ratatui/crossterm drivers, controllers, TUI state

- May depend on all inner rings.
- Owns: terminal setup (`tui::terminal`), event loop (`tui::event_loop`), renderer (`tui::renderer::*`), controllers (`tui::controllers::*`), TUI state (`AppState`, `View`).
- **Controllers** map input events to use-case calls and update `AppState`. They consume raw `crossterm` events and framework types directly, so they live with the framework, not in adapters — keeping them in adapters would force an outward `adapters → frameworks` dependency.
- Renderer consumes view models only — never domain types or DTOs.
- `AppState` caches view models so re-navigation doesn't re-query. Clearing a `*_vm` to `None` invalidates it.

## `main.rs` — composition root

- The only file that constructs concrete adapter/framework types and wires them together.
- Pattern: open connection → build gateway → build clock → build use cases → build controller → build `AppState` → enter terminal → run event loop → restore terminal.

## Enforcement

There is no automated linting for the dependency rule yet. When adding a module, double-check its `use` statements point inward only. `cargo build` will reject cycles.
