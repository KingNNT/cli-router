# Remove Projects View Design

## Summary

The `analysis` TUI currently exposes four sidebar views — Dashboard, Models, **Projects**, and Pricing. The Projects view breaks token/cost usage down by Claude Code session directory (a per-project grouping). It is no longer wanted and should be removed end-to-end, leaving a three-view sidebar (Dashboard, Models, Pricing).

This is a pure deletion across all clean-architecture rings of the `analysis` crate. No behavior is added, no data shape is changed for the remaining views, and no other crate is touched.

## Current State

The Projects feature spans every ring of the `analysis` app:

- **Domain / DTO** — `application/dto/projects_breakdown.rs` defines `GetProjectsBreakdownInput` / `GetProjectsBreakdownOutput` (carrying `ProjectUsage` rows).
- **Port** — `application/ports/usage_repository.rs` declares `fn by_project(&self, filter: &Filter) -> Result<Vec<ProjectUsage>, ApplicationError>`.
- **Use case** — `application/use_cases/get_projects_breakdown.rs` (`GetProjectsBreakdown`) and its re-export in `use_cases/mod.rs`.
- **Adapters**
  - `view_models/projects_vm.rs` — `ProjectsViewModel`.
  - `presenters/projects_presenter.rs` — `present_projects`.
  - `gateways/sqlite/repository.rs`, `gateways/claudecode/repository.rs`, `gateways/dispatching_usage_repository.rs` — `by_project` implementations plus per-repo `ProjectUsage` / `ProjectUsageRow` query helpers.
- **TUI framework**
  - `tui/app_state.rs` — `View::Projects` variant, `projects_vm` / `projects_offset` fields, and `Projects` arms in `label`, `current_offset`, `set_current_offset`, and `with_data_source`. `View::ALL` is a 4-element array.
  - `tui/renderer/projects.rs` — the renderer module (file).
  - `tui/renderer/mod.rs` — the `View::Projects` dispatch arm and `mod projects` declaration.
  - `tui/renderer/sidebar.rs` — renders `View::ALL` (no Projects-specific code, but index shifts after removal).
  - `tui/controllers/tui_controller.rs` — `get_projects_breakdown` field, ctor parameter, `View::Projects` arm in `ensure_vm_for_current_view`, and the `present_projects` / `GetProjectsBreakdown*` imports. Several unit tests hardcode `sidebar_selected = 3` for Pricing (which becomes index 2) and a `View::Projects => 2` mapping.
- **Composition root** — `main.rs` constructs `GetProjectsBreakdown` and passes it into `TuiController`.
- **Tests** — `tests/integration.rs` imports the projects types and has `projects_breakdown_groups_by_session_directory`.

## Goals

- Remove the Projects view and every piece of code that exists solely to support it.
- Leave the remaining three views (Dashboard, Models, Pricing) byte-for-byte unchanged in behavior.
- Keep `cargo check --workspace`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`, and `cargo fmt` green.
- Leave no dead code behind (no `#[allow(dead_code)]` placeholders).

## Non-Goals

- Do **not** touch the `proxy`, `proxy-tui`, or `proxy-admin-api` crates.
- Do **not** change the SQLite schema or the on-disk database shape.
- Do **not** remove `default_projects_root()` — it locates `~/.claude/projects` and is still required by `ClaudeCodeUsageRepository` to find session JSONL files. It is a *filesystem path*, unrelated to the Projects *view*.
- Do **not** refactor the remaining views, presenters, or repositories beyond what deletion requires.
- Do **not** introduce a feature flag. The removal is unconditional.

## Scope of Deletion

Ordered bottom-up so each layer compiles before the next is touched. Each step must leave `cargo check -p analysis` green before moving on.

### 1. Domain / DTO layer

- Delete file `application/dto/projects_breakdown.rs`.
- Remove its `pub mod projects_breakdown;` line in `application/dto/mod.rs`.
- Remove any re-export of `ProjectUsage` / `GetProjectsBreakdownInput` / `GetProjectsBreakdownOutput` from `lib.rs`.

### 2. Port layer

- In `application/ports/usage_repository.rs`, remove the `fn by_project(...)` trait method.
- Leave the `ProjectUsage` type definition in place for now; its deletion is decided in step 5 once all consumers are known.

### 3. Use case layer

- Delete file `application/use_cases/get_projects_breakdown.rs`.
- Remove its re-export from `application/use_cases/mod.rs`.

### 4. Adapter layer — view models & presenters

- Delete `adapters/view_models/projects_vm.rs` and its `mod` line in `view_models/mod.rs`.
- Delete `adapters/presenters/projects_presenter.rs` and its `mod` line in `presenters/mod.rs`.

### 5. Adapter layer — gateways

For each of the three repository implementations, remove the `by_project` impl and any `ProjectUsage` / `ProjectUsageRow` helpers that exist *only* to serve it:

- `adapters/gateways/sqlite/repository.rs` (and `query_builder.rs` if it has a project-specific builder).
- `adapters/gateways/claudecode/repository.rs`.
- `adapters/gateways/dispatching_usage_repository.rs`.

**Single source of truth for `ProjectUsage`:** after removing all three `by_project` impls above and the port method from step 2, run `find_referencing_symbols` on `ProjectUsage`. If it has zero remaining consumers (expected), delete the type definition wherever it lives (likely `application/ports/usage_repository.rs` or `application/dto/`). If any consumer remains, leave the type.

### 6. TUI framework — renderers

- Delete file `tui/renderer/projects.rs`.
- In `tui/renderer/mod.rs`: remove `mod projects;` / `use projects::*;` (or equivalent) and the `View::Projects` arm of `dispatch_view`.

### 7. TUI framework — app state

In `tui/app_state.rs`:

- Remove the `Projects` variant from `enum View`.
- Change `View::ALL` from `[View; 4]` to `[View; 3]` and drop `View::Projects` from the array.
- Remove the `View::Projects => "Projects"` arm of `impl View::label`.
- Remove the `projects_vm: Option<ProjectsViewModel>` field from `AppState`.
- Remove the `projects_offset: usize` field from `AppState`.
- Remove the `Projects` arms in `current_offset` and `set_current_offset`.
- Remove the `projects_vm: None,` and `projects_offset: 0,` initializers in `with_data_source`.
- Remove the now-unused `ProjectsViewModel` import.

### 8. TUI framework — controller

In `tui/controllers/tui_controller.rs`:

- Remove the `get_projects_breakdown: Arc<GetProjectsBreakdown>` field from `TuiController`.
- Remove the corresponding ctor parameter and the field initializer.
- Remove the `View::Projects` arm from `ensure_vm_for_current_view`.
- Remove `present_projects` from the presenter import list.
- Remove `GetProjectsBreakdown`, `GetProjectsBreakdownInput`, `GetProjectsBreakdownOutput` from imports.
- Update unit tests:
  - Any test setting `sidebar_selected = 3` for Pricing must become `2`.
  - Remove the `View::Projects => 2` arm in the `esc_backspace_left_h_backtab_return_to_sidebar` test and any other `View::Projects` references.
  - Remove or rewrite any test that asserts on Projects navigation (e.g. the mouse-click test that asserts `state.view == View::Projects`).

### 9. Composition root

In `main.rs`:

- Remove `GetProjectsBreakdown` from the `use analysis::application::use_cases::{...}` import.
- Remove the `let get_projects = Arc::new(GetProjectsBreakdown::new(...));` construction.
- Remove `get_projects,` from the `TuiController::new(...)` call.
- **Keep** `default_projects_root` and `ClaudeCodeUsageRepository::new(default_projects_root())` — these still locate session JSONL files.

### 10. Integration tests

In `tests/integration.rs`:

- Remove `GetProjectsBreakdownInput` from the DTO import.
- Remove `GetProjectsBreakdown` from the use case import.
- Delete the `projects_breakdown_groups_by_session_directory` test function.

## Verification

After each layer and at the end:

```bash
cargo check -p analysis
cargo clippy -p analysis -- -D warnings
cargo test -p analysis
cargo fmt
```

Final workspace gate:

```bash
cargo check --workspace
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

## Risks & Mitigations

| Risk | Mitigation |
| --- | --- |
| `ProjectUsage` type is shared with another consumer. | Check references with `find_referencing_symbols` before deleting the type; keep it if still used. |
| `default_projects_root` is accidentally removed. | Explicitly called out as a non-goal; it stays in `claudecode/mod.rs` and `repository.rs`. |
| Test index shifts (Pricing 3 → 2) break silently. | Compiler catches `sidebar_selected = 3` against a 3-element array only at runtime; grep + run `cargo test -p analysis` to catch assertions. |
| A `ProjectUsage` row remains in the SQLite query builder. | Step 5 explicitly audits `query_builder.rs`. |

## Out of Scope

- Renaming `View::ALL` or renumbering sidebar indices beyond what deletion forces.
- Re-theming the sidebar.
- Any change to how `ClaudeCodeUsageRepository` discovers session directories.
