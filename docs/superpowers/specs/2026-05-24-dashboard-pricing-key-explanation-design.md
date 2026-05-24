# Dashboard Pricing Key Explanation Design

## Summary

The analysis dashboard should explain, for each model column, which pricing model/key was used to calculate costs. This makes fallback pricing visible when a usage model is priced through an alias or family fallback, and it makes missing pricing obvious when no pricing entry is found.

## Goals

- Show a small explanatory label for every model column on the analysis dashboard.
- Reuse the same pricing lookup behavior used during dashboard cost reconciliation.
- Distinguish exact pricing, alias/fallback pricing, and missing pricing.
- Keep the dashboard readable in a terminal UI.

## Non-Goals

- No pricing database or schema changes.
- No fuzzy or new pricing lookup behavior beyond the existing shared resolver.
- No changes to proxy request accounting.
- No large redesign of the dashboard table layout.

## User-Facing Behavior

Each model column in the dashboard has a compact second line or dim note under the model name:

```text
openai/gpt-5.1-codex-latest
priced as gpt5.1-codex
```

Exact pricing matches should remain explicit:

```text
gpt-5.1
exact price
```

Unpriced models should be clear:

```text
unknown-model
no price
```

The cost cells keep the existing rendering behavior. This feature only explains which pricing key drives those costs.

## Architecture

### Pricing match metadata

During `GetDashboard::execute`, dashboard pricing reconciliation already checks candidate pricing keys for each model. Extend that process so it records the winning pricing key for each aggregated model:

- `Some(key)` when a pricing entry was found.
- `None` when no pricing entry was found.

The candidate order should come from `shared::domain::services::aliases::pricing_lookup_keys` so the explanation matches the real calculation path. If the dashboard still uses `ModelId::lookup_keys()`, replace that use with the shared pricing resolver for both candidate collection and row reconciliation.

### DTO and view model shape

Add column-level pricing metadata to the dashboard output/view model rather than recomputing it in the renderer.

Recommended application DTO addition:

```rust
pub struct DashboardModelPricing {
    pub model: ModelId,
    pub pricing_key: Option<String>,
}
```

`GetDashboardOutput` should include a collection such as:

```rust
pub model_pricing: Vec<DashboardModelPricing>
```

The presenter should transform this into a display-friendly structure, for example:

```rust
pub struct ModelColumnVM {
    pub model: String,
    pub pricing_note: String,
}
```

`DashboardViewModel.model_columns` can change from `Vec<String>` to `Vec<ModelColumnVM>` if that best fits the renderer.

### Label rules

For each model column:

- If `pricing_key` is `None`, render `no price`.
- If `pricing_key` equals the display model string, render `exact price`.
- Otherwise render `priced as <pricing_key>`.

This avoids requiring the use case to classify whether a non-exact key came from a direct suffix, exact alias, or family fallback. The user-visible need is to know the actual key used for calculation.

### Dashboard renderer

Update the dashboard renderer where model column headers are built. Render the model name and the pricing note together, with the note in a dim/subtle style when possible.

The renderer should handle narrow terminals gracefully:

- Prefer showing the note when there is space.
- If existing layout constraints make a two-line header difficult, append a compact note such as `↦ gpt5.1-codex` or `no price` in the same header cell.
- Do not hide the model name.

## Data Flow

1. Dashboard use case loads usage rows.
2. Rows are aggregated by alias as today.
3. For each aggregated model, the use case gathers candidate keys through `pricing_lookup_keys(model.as_str())` or equivalent.
4. Pricing repository fetches all candidates.
5. Reconciliation calculates cost with the first matching candidate and stores that matched key in `model_pricing`.
6. Presenter builds column labels and pricing notes.
7. Renderer displays the model column name with its pricing explanation.

## Error Handling

- Missing pricing is not an error. It should be represented as `pricing_key = None` and rendered as `no price`.
- Repository failures remain existing application errors.
- If a model appears in rows but metadata is missing due to an implementation bug, the presenter should default that column to `no price` rather than panic.

## Testing

Add or update tests for:

- Dashboard use case records the exact pricing key when the display model has an exact price.
- Dashboard use case records a fallback/alias pricing key when the display model is priced through another key.
- Dashboard use case records `None` for unpriced models.
- Presenter renders `exact price`, `priced as <key>`, and `no price` notes.
- Renderer/header tests, if present, verify that the pricing note appears in dashboard model column headers.

## Acceptance Criteria

- Every dashboard model column has a visible pricing explanation.
- The explanation names the same pricing key used for cost calculation.
- Exact, fallback/alias, and missing pricing cases are distinguishable.
- Existing dashboard totals and cost cells continue to behave the same.
- Tests cover pricing-key metadata and presenter label formatting.
