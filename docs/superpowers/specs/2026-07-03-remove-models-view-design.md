# Remove Models View Design

## Summary

The `analysis` TUI currently has three sidebar views — Dashboard, **Models**, Pricing. The Models view breaks token/cost usage down per model. It is no longer wanted and should be removed end-to-end, leaving a two-view sidebar (Dashboard, Pricing).

This is a pure deletion across all clean-architecture rings of the `analysis` crate plus two dead code paths in `shared`. No behavior is added; the remaining views are unchanged.

## Current State

The Models feature spans every ring:

- **Domain / DTO** — `application/dto/models_breakdown.rs` defines `GetModelsBreakdownInput` / `GetModelsBreakdownOutput` (carrying `Vec<ModelUsage>` + missing-pricing metadata).
- **Port** — `application/ports/usage_repository.rs` declares `fn by_model(&self, filter: &Filter) -> Result<Vec<ModelUsage>, ApplicationError>`.
- **Use case** — `application/use_cases/get_models_breakdown.rs` (`GetModelsBreakdown`) and its re-export.
- **Adapters**
  - `view_models/models_vm.rs` — `ModelsViewModel` / `ModelRowVM`.
  - `presenters/models_presenter.rs` — `present_models`.
  - `gateways/sqlite/repository.rs`, `gateways/claudecode/repository.rs`, `gateways/dispatching_usage_repository.rs` — `by_model` implementations + the `by_model_sorted_by_cost_desc` sqlite unit test.
  - `application/test_support.rs` — `FakeUsageRepository.by_model` field + impl.
- **TUI framework**
  - `tui/app_state.rs` — `View::Models` variant (index 1 of 3), `models_vm` / `models_offset` fields, and `Models` arms in `label`, `current_offset`, `set_current_offset`, `sidebar_up`, `sidebar_down`, `invalidate_all_vms`, `with_data_source`.
  - `tui/renderer/models.rs` — the renderer module.
  - `tui/renderer/mod.rs` — the `View::Models` dispatch arm and `mod models` declaration.
  - `tui/controllers/tui_controller.rs` — `get_models_breakdown` field, ctor parameter, `View::Models` arm in `ensure_vm_for_current_view`, imports, the `ctl_with_source_rows` helper, and several tests that reference Models / `sidebar_selected = 1`.
- **Composition root** — `main.rs` constructs `GetModelsBreakdown` and passes it into `TuiController`.
- **Tests** — `tests/integration.rs` imports the models types and has `models_breakdown_returns_one_row_per_model_sorted_by_cost`.
- **Shared services** — `shared/src/domain/services/aggregation.rs` has `group_by_model` and `aggregate_model_usage_by_alias`, both used only by the Models pipeline (and their own unit tests).

## Goals

- Remove the Models view and every piece of code that exists solely to support it.
- Leave Dashboard and Pricing byte-for-byte unchanged in behavior.
- Keep all workspace gates green (`cargo check/clippy/test/fmt`).
- Leave no dead code behind.

## Non-Goals

- Do **not** touch the `proxy`, `proxy-tui`, or `proxy-admin-api` crates.
- Do **not** change the SQLite schema.
- Do **not** remove `daily_by_model` or `DayModelRow` — these are the Dashboard's data shape (`GetDashboard::execute` calls `usage_repo.daily_by_model`). They MUST STAY.
- Do **not** remove `aggregate_day_model_rows_by_alias` — Dashboard uses it.
- Do **not** remove `overview` — Dashboard uses it.
- Do **not** remove `ModelId`, `ModelPricing`, or pricing lookup — Dashboard and Pricing use them.
- Do **not** refactor the remaining views beyond what deletion requires.

## Preserve / Delete Boundary

| DELETE | KEEP (used by Dashboard/Pricing) |
|--------|---------------------------------|
| `ModelUsage` entity | `DayModelRow`, `Overview`, `UsageRecord`, `ModelPricing` |
| `by_model` port method | `daily_by_model`, `overview` |
| `group_by_model`, `aggregate_model_usage_by_alias` | `aggregate_day_model_rows_by_alias`, `group_by_day`, `sum_costs`, `sum_tokens` |
| `GetModelsBreakdown` + DTO | `GetDashboard`, `GetPricing`, `SyncPricing` |
| Models presenter + VM | Dashboard/Pricing presenters + VMs |
| Models renderer + `View::Models` | Dashboard/Pricing renderers |

## Verification

After each commit and at the end:

```bash
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt
```

Final residual grep:
```bash
rg -n "ModelUsage|GetModelsBreakdown|by_model\b|models_breakdown|present_models|ModelsViewModel|View::Models|models_vm|models_offset" crates/analysis crates/shared
```
Expected: zero matches. (`daily_by_model`, `DayModelRow`, `ModelId`, `ModelPricing` MUST still appear — they stay.)

## Risks & Mitigations

| Risk | Mitigation |
| --- | --- |
| Accidentally removing `daily_by_model` (sounds like "by_model"). | Explicit non-goal; the two methods are distinct. `by_model` groups to `ModelUsage`; `daily_by_model` groups to `DayModelRow`. |
| Test index shifts (Pricing 2 → 1) break silently. | Compiler/runtime catches; controller tests are audited in the plan. |
| `aggregate_model_usage_by_alias` has hidden callers. | Verified via `find_referencing_symbols`: only `GetModelsBreakdown` + own tests. |

## Out of Scope

- Renaming `View::ALL` or renumbering beyond what deletion forces.
- Any change to Dashboard or Pricing rendering.
