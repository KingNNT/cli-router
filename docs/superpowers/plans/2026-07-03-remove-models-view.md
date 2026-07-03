# Remove Models View Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Delete the Models sidebar view end-to-end from the `analysis` crate, leaving a two-view sidebar (Dashboard, Pricing).

**Architecture:** Pure deletion, top-down so every commit keeps the full workspace green (the repo's pre-commit hook runs `cargo test --workspace`). After the first commit removes all consumers (view/controller/main), the second commit removes the now-dead infrastructure (use case, DTO, presenter, VM, port method, gateway impls, entity, shared aggregation functions).

**Tech Stack:** Rust workspace, ratatui TUI, clean architecture rings.

**Critical scope clarification (verified during planning):**
- **DELETE:** `ModelUsage` entity, `by_model` port method, `group_by_model` + `aggregate_model_usage_by_alias` aggregation functions.
- **KEEP:** `daily_by_model` (Dashboard's data shape), `DayModelRow`, `overview`, `aggregate_day_model_rows_by_alias`, `ModelId`, `ModelPricing`. All used by Dashboard/Pricing.

**Reference spec:** `docs/superpowers/specs/2026-07-03-remove-models-view-design.md`

---

## Commit-Unit 1: Remove Models view + controller + main wiring (always-green)

This commit removes every *consumer* of the Models pipeline. After it, the Models infrastructure still exists but is unused; all tests still pass.

**Files:**
- Delete: `crates/analysis/src/tui/renderer/models.rs`
- Modify: `crates/analysis/src/tui/renderer/mod.rs`, `crates/analysis/src/tui/app_state.rs`, `crates/analysis/src/tui/controllers/tui_controller.rs`, `crates/analysis/src/main.rs`

### Step 1: Delete the renderer
```bash
rm crates/analysis/src/tui/renderer/models.rs
```

### Step 2: `tui/renderer/mod.rs`
- Remove `pub mod models;`.
- Remove the `View::Models => { ... }` arm from `dispatch_view` (keep Dashboard + Pricing arms).

### Step 3: `tui/app_state.rs`
- Remove `Models` from `enum View { Dashboard, Models, Pricing }` → `{ Dashboard, Pricing }`.
- Change `View::ALL: [View; 3]` → `[View; 2]`, dropping `View::Models`.
- Remove `View::Models => "Models"` from `label`.
- Remove `models_vm: Option<ModelsViewModel>,` field.
- Remove `models_offset: usize,` field.
- Remove `models_vm: None,` and `models_offset: 0,` from `with_data_source`.
- Remove `self.models_offset = 0;` from `sidebar_up` and `sidebar_down`.
- Remove `self.models_vm = None;` and `self.models_offset = 0;` from `invalidate_all_vms`.
- Remove `View::Models => self.models_offset,` from `current_offset`.
- Remove `View::Models => self.models_offset = value,` from `set_current_offset`.
- Remove `ModelsViewModel` from the `use crate::adapters::view_models::{...}` import.

### Step 4: `tui/controllers/tui_controller.rs`

(a) Imports — remove `present_models` from presenter import; `GetModelsBreakdownInput` from dto import; `GetModelsBreakdown` from use_cases import.

(b) Remove `pub get_models_breakdown: Arc<GetModelsBreakdown>,` field from `TuiController` struct; remove the ctor parameter and the `get_models_breakdown,` initializer line.

(c) Remove the `View::Models` arm from `ensure_vm_for_current_view`.

(d) In `ctl_with_source_rows` test helper (~line 439), remove the `gm` construction and the `gm` arg from `TuiController::new(gd, gm, ...)`.

(e) Fix the warmup test assertion (~line 485-487): the comment `// lazy views (Models, Pricing) remain invalidated until visited.` becomes `// lazy view (Pricing) remains invalidated until visited.`; remove the line `assert!(state.models_vm.is_none());`.

(f) In `esc_backspace_left_h_backtab_return_to_sidebar` test (~line 660, 669):
- The `cases` array has `(KeyCode::Left, View::Models)` — remove that entry (the test iterates views; Models no longer exists). The remaining cases are Dashboard/Backspace/h/BackTab.
- The `sidebar_selected` match: remove `View::Models => 1,` and change `View::Pricing => 2,` to `View::Pricing => 1,`.

(g) Shift ALL `sidebar_selected = 2` (Pricing) → `sidebar_selected = 1`. Every occurrence targets Pricing (verified: each is followed by `state.view = View::Pricing;`). ~9 occurrences. Run `rg -n "sidebar_selected = 2" crates/analysis/src/tui/controllers/tui_controller.rs` to find them.

(h) DELETE the `left_from_non_dashboard_content_goes_back_to_sidebar` test entirely (~lines 1002-1013) — it specifically uses `View::Models` as the non-dashboard view. There's no other non-dashboard view to test this with except Pricing, but Pricing has its own dedicated tests. Removing is cleanest.

### Step 5: `crates/analysis/src/main.rs`
- Remove `GetModelsBreakdown` from the use_cases import.
- Remove the `let get_models = Arc::new(GetModelsBreakdown::new(...));` construction.
- Remove `get_models,` from the `TuiController::new(...)` call.

### Step 6: Verify + commit
```bash
cargo test -p analysis
git add -A
git commit -m "refactor(analysis): remove Models view, controller wiring, and main construction"
```

---

## Commit-Unit 2: Remove all dead Models infrastructure

After the view/controller/main wiring is gone, remove the now-unused infrastructure.

**Files deleted (6):**
- `crates/shared/src/domain/entities/model_usage.rs`
- `crates/analysis/src/application/dto/models_breakdown.rs`
- `crates/analysis/src/application/use_cases/get_models_breakdown.rs`
- `crates/analysis/src/adapters/view_models/models_vm.rs`
- `crates/analysis/src/adapters/presenters/models_presenter.rs`
- (`models.rs` renderer already deleted in Commit-Unit 1)

**Files modified:**
- `crates/shared/src/domain/entities/mod.rs` — remove `pub mod model_usage;` + `pub use model_usage::ModelUsage;`
- `crates/shared/src/domain/services/aggregation.rs` — remove `group_by_model` fn, `aggregate_model_usage_by_alias` fn, the `model_row` test helper, and the tests `group_by_model_buckets_and_sorts_by_cost_desc`, `model_usage_single_source_passes_through_unchanged`, `model_usage_multi_source_collapses_and_sums_message_count`, `model_usage_mixed_aliased_and_not_keeps_both`. KEEP `aggregate_day_model_rows_by_alias`, `group_by_day`, `sum_costs`, `sum_tokens`. Remove `ModelUsage` from imports if it becomes unused there.
- `crates/analysis/src/application/dto/mod.rs` — remove models_breakdown mod + re-export.
- `crates/analysis/src/application/use_cases/mod.rs` — remove get_models_breakdown mod + re-export.
- `crates/analysis/src/application/ports/usage_repository.rs` — remove `by_model` method + `ModelUsage` import.
- `crates/analysis/src/application/test_support.rs` — remove `by_model` field/method + `ModelUsage` import.
- `crates/analysis/src/adapters/view_models/mod.rs` — remove models_vm mod + re-export.
- `crates/analysis/src/adapters/presenters/mod.rs` — remove models_presenter mod declaration.
- `crates/analysis/src/adapters/gateways/sqlite/repository.rs` — remove `by_model` impl + `ModelUsage` import; remove `by_model_sorted_by_cost_desc` test. Check if `ModelId`/other imports still used by surviving methods.
- `crates/analysis/src/adapters/gateways/claudecode/repository.rs` — remove `by_model` impl + `ModelUsage` import.
- `crates/analysis/src/adapters/gateways/dispatching_usage_repository.rs` — remove `by_model` impl + `ModelUsage` import.
- `crates/analysis/tests/integration.rs` — remove `GetModelsBreakdownInput` import, `GetModelsBreakdown` import, and the `models_breakdown_returns_one_row_per_model_sorted_by_cost` test function.

### Verification
```bash
cargo test --workspace
rg -n "ModelUsage|GetModelsBreakdown|by_model\b|models_breakdown|present_models|ModelsViewModel|models_vm|models_offset" crates/analysis crates/shared
```
Expected: zero matches.

### Commit
```bash
cargo fmt
git add -A
git commit -m "refactor(analysis,shared): remove all dead Models infrastructure"
```

---

## Commit-Unit 3: Doc cleanup

Update stale references in:
- `CLAUDE.md` — architecture tree (remove `GetModelsBreakdown` from use cases list; update `daily_by_model` mention if needed).
- `.claude/rules/module-boundaries.md` line 9 — remove `ModelUsage` from the entities list.

### Commit
```bash
git add CLAUDE.md .claude/rules/module-boundaries.md
git commit -m "docs: drop stale ModelUsage / GetModelsBreakdown references"
```

---

## Self-Review

**Spec coverage:** Commit-Unit 1 covers view/controller/main (consumers). Commit-Unit 2 covers all infrastructure (domain/dto/port/usecase/adapters/tests/shared-services). Commit-Unit 3 covers docs.

**Placeholder scan:** None — exact files and methods listed.

**Type consistency:** `by_model` → `ModelUsage` (deleted together). `daily_by_model` → `DayModelRow` (kept together). `View::ALL` shrinks 3→2 consistently in app_state + all test index shifts (Pricing 2→1). `TuiController::new` arity 5→4 consistent between controller definition + main.rs call site.
