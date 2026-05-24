# Dashboard Pricing Key Explanation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show a small per-model-column note on the analysis dashboard explaining which pricing key/model was used to calculate that column's costs.

**Architecture:** Extend the dashboard use case to record the matched pricing key while it reconciles costs, then pass that metadata through the presenter into dashboard column view models. Update the Ratatui dashboard header to render each model name with `exact price`, `priced as <key>`, or `no price`.

**Tech Stack:** Rust 2024 workspace, shared pricing services, analysis application DTOs, Ratatui dashboard renderer, Cargo test runner.

---

## Files

- Modify: `crates/analysis/src/application/dto/dashboard.rs` — add dashboard model pricing metadata DTO and attach it to `GetDashboardOutput`.
- Modify: `crates/analysis/src/application/use_cases/get_dashboard.rs` — use `pricing_lookup_keys`, record matched pricing keys, and test exact/fallback/missing metadata.
- Modify: `crates/analysis/src/adapters/view_models/dashboard_vm.rs` — replace raw model column strings with model column view models containing pricing notes.
- Modify: `crates/analysis/src/adapters/presenters/dashboard_presenter.rs` — convert model pricing metadata into display labels and test note formatting.
- Modify: `crates/analysis/src/tui/renderer/dashboard.rs` — render model column notes in the two-line header and update renderer tests/sample view models.

---

### Task 1: Add dashboard pricing metadata to the use case

**Files:**
- Modify: `crates/analysis/src/application/dto/dashboard.rs`
- Modify: `crates/analysis/src/application/use_cases/get_dashboard.rs`

- [ ] **Step 1: Add failing DTO/use-case tests for pricing metadata**

In `crates/analysis/src/application/use_cases/get_dashboard.rs`, add these tests inside the existing `#[cfg(test)] mod tests` after `suffix_lookup_matches_bare_key`:

```rust
    #[test]
    fn model_pricing_records_exact_key() {
        let (uc, _) = setup(
            vec![row("anthropic/opus", 1000, 999.99)],
            vec![pricing_for("anthropic/opus", 0.00001)],
        );

        let out = uc.execute(GetDashboardInput::default()).unwrap();

        assert_eq!(out.model_pricing.len(), 1);
        assert_eq!(out.model_pricing[0].model.as_str(), "anthropic/opus");
        assert_eq!(out.model_pricing[0].pricing_key.as_deref(), Some("anthropic/opus"));
    }

    #[test]
    fn model_pricing_records_fallback_key() {
        let (uc, _) = setup(
            vec![row("openai/gpt-5.1-codex-latest", 1000, 999.99)],
            vec![pricing_for("gpt5.1-codex", 0.00003)],
        );

        let out = uc.execute(GetDashboardInput::default()).unwrap();

        assert_eq!(out.model_pricing.len(), 1);
        assert_eq!(out.model_pricing[0].model.as_str(), "openai/gpt-5.1-codex-latest");
        assert_eq!(out.model_pricing[0].pricing_key.as_deref(), Some("gpt5.1-codex"));
        assert!((out.rows[0].cost.value() - 0.03).abs() < 1e-9);
    }

    #[test]
    fn model_pricing_records_missing_price() {
        let (uc, _) = setup(vec![row("unknown/model", 1000, 7.77)], vec![]);

        let out = uc.execute(GetDashboardInput::default()).unwrap();

        assert_eq!(out.model_pricing.len(), 1);
        assert_eq!(out.model_pricing[0].model.as_str(), "unknown/model");
        assert_eq!(out.model_pricing[0].pricing_key, None);
    }
```

- [ ] **Step 2: Run the dashboard use-case tests and verify failure**

Run:

```bash
cargo test -p analysis application::use_cases::get_dashboard::tests::model_pricing -- --nocapture
```

Expected: FAIL because `GetDashboardOutput` does not have a `model_pricing` field yet.

- [ ] **Step 3: Add the metadata DTO**

In `crates/analysis/src/application/dto/dashboard.rs`, insert this struct before `GetDashboardOutput`, and add the new field to `GetDashboardOutput`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashboardModelPricing {
    pub model: ModelId,
    pub pricing_key: Option<String>,
}

#[derive(Debug, Clone)]
pub struct GetDashboardOutput {
    pub filter_applied: Filter,
    pub overview: Overview,
    pub rows: Vec<DayModelRow>,
    pub model_pricing: Vec<DashboardModelPricing>,
    pub missing_pricing_count: usize,
    pub unpriced_models: HashSet<ModelId>,
    pub last_pricing_sync: Option<NaiveDate>,
}
```

- [ ] **Step 4: Record the matched pricing key in `GetDashboard::execute`**

In `crates/analysis/src/application/use_cases/get_dashboard.rs`, change the DTO import and add the shared resolver import near the existing imports:

```rust
use crate::application::dto::{
    DashboardModelPricing, Filter, GetDashboardInput, GetDashboardOutput,
};
use shared::domain::services::aliases::pricing_lookup_keys;
```

Then replace both loops that call `row.model.lookup_keys()` with `pricing_lookup_keys(row.model.as_str())`, and build `model_pricing` during reconciliation:

```rust
        let mut candidates: HashSet<String> = HashSet::new();
        for row in &rows {
            for key in pricing_lookup_keys(row.model.as_str()) {
                candidates.insert(key);
            }
        }
        let candidate_vec: Vec<String> = candidates.into_iter().collect();
        let pricing_map = self.pricing_repo.find_many(&candidate_vec)?;

        // Reconcile each row's cost and remember the pricing key used per model.
        let mut missing = 0usize;
        let mut unpriced_models: HashSet<ModelId> = HashSet::new();
        let mut model_pricing = Vec::with_capacity(rows.len());
        for row in rows.iter_mut() {
            let mut matched_key: Option<String> = None;
            for key in pricing_lookup_keys(row.model.as_str()) {
                if let Some(p) = pricing_map.get(&key) {
                    row.cost = pricing_service::calculate_cost(&row.tokens, p);
                    matched_key = Some(key);
                    break;
                }
            }
            if matched_key.is_none() {
                missing += 1;
                unpriced_models.insert(row.model.clone());
            }
            model_pricing.push(DashboardModelPricing {
                model: row.model.clone(),
                pricing_key: matched_key,
            });
        }
```

In the `Ok(GetDashboardOutput { ... })` expression, include the new field:

```rust
            model_pricing,
```

- [ ] **Step 5: Run the targeted use-case tests and verify pass**

Run:

```bash
cargo test -p analysis application::use_cases::get_dashboard::tests::model_pricing -- --nocapture
```

Expected: PASS for all three `model_pricing_*` tests.

- [ ] **Step 6: Run all dashboard use-case tests**

Run:

```bash
cargo test -p analysis application::use_cases::get_dashboard::tests -- --nocapture
```

Expected: PASS. If another `GetDashboardOutput` initializer fails to compile, add `model_pricing: Vec::new(),` only in tests that manually construct the output.

- [ ] **Step 7: Commit Task 1**

Run:

```bash
git add crates/analysis/src/application/dto/dashboard.rs crates/analysis/src/application/use_cases/get_dashboard.rs
git commit -m "feat(analysis): track dashboard pricing keys"
```

Expected: commit succeeds.

---

### Task 2: Present pricing notes in dashboard column view models

**Files:**
- Modify: `crates/analysis/src/adapters/view_models/dashboard_vm.rs`
- Modify: `crates/analysis/src/adapters/presenters/dashboard_presenter.rs`

- [ ] **Step 1: Add failing presenter tests for column pricing notes**

In `crates/analysis/src/adapters/presenters/dashboard_presenter.rs`, add these tests inside the existing `#[cfg(test)] mod tests` near the other presenter tests:

```rust
    #[test]
    fn model_column_note_shows_exact_price() {
        let out = GetDashboardOutput {
            rows: vec![row("anthropic/opus", 1000, 0.01)],
            model_pricing: vec![DashboardModelPricing {
                model: ModelId::new("anthropic/opus").unwrap(),
                pricing_key: Some("anthropic/opus".to_string()),
            }],
            ..output(vec![])
        };

        let vm = present(&out);

        assert_eq!(vm.model_columns[0].model, "anthropic/opus");
        assert_eq!(vm.model_columns[0].pricing_note, "exact price");
    }

    #[test]
    fn model_column_note_shows_fallback_price() {
        let out = GetDashboardOutput {
            rows: vec![row("openai/gpt-5.1-codex-latest", 1000, 0.03)],
            model_pricing: vec![DashboardModelPricing {
                model: ModelId::new("openai/gpt-5.1-codex-latest").unwrap(),
                pricing_key: Some("gpt5.1-codex".to_string()),
            }],
            ..output(vec![])
        };

        let vm = present(&out);

        assert_eq!(vm.model_columns[0].model, "openai/gpt-5.1-codex-latest");
        assert_eq!(vm.model_columns[0].pricing_note, "priced as gpt5.1-codex");
    }

    #[test]
    fn model_column_note_shows_no_price() {
        let out = GetDashboardOutput {
            rows: vec![row("unknown/model", 1000, 7.77)],
            model_pricing: vec![DashboardModelPricing {
                model: ModelId::new("unknown/model").unwrap(),
                pricing_key: None,
            }],
            ..output(vec![])
        };

        let vm = present(&out);

        assert_eq!(vm.model_columns[0].model, "unknown/model");
        assert_eq!(vm.model_columns[0].pricing_note, "no price");
    }
```

If the test module does not already import `DashboardModelPricing`, update its DTO import to include it:

```rust
use crate::application::dto::{DashboardModelPricing, GetDashboardOutput};
```

- [ ] **Step 2: Run presenter tests and verify failure**

Run:

```bash
cargo test -p analysis adapters::presenters::dashboard_presenter::tests::model_column_note -- --nocapture
```

Expected: FAIL because `model_columns` still contains `String` values.

- [ ] **Step 3: Add model column view model type**

In `crates/analysis/src/adapters/view_models/dashboard_vm.rs`, add `ModelColumnVM` and change `DashboardViewModel.model_columns`:

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelColumnVM {
    pub model: String,
    pub pricing_note: String,
}

#[derive(Debug, Clone, Default)]
pub struct DashboardViewModel {
    pub status_msgs: String,
    pub status_cost: String,
    pub pricing_note: Option<String>,
    pub banner: Option<String>,
    pub window_tabs: Vec<String>,
    pub selected_window_index: usize,
    pub model_columns: Vec<ModelColumnVM>,
    pub rows: Vec<DayPivotRowVM>,
    pub column_totals: Vec<ModelBreakdownVM>,
    pub grand_total: String,
    pub grand_cost: String,
    pub empty: bool,
}
```

- [ ] **Step 4: Build pricing-note labels in the presenter**

In `crates/analysis/src/adapters/presenters/dashboard_presenter.rs`, update the view model import to include `ModelColumnVM`.

Replace the current `model_columns` creation with this code:

```rust
    let pricing_by_model: BTreeMap<ModelId, Option<String>> = out
        .model_pricing
        .iter()
        .map(|p| (p.model.clone(), p.pricing_key.clone()))
        .collect();
    let model_columns: Vec<ModelColumnVM> = columns
        .iter()
        .map(|m| {
            let pricing_key = pricing_by_model.get(m).cloned().unwrap_or(None);
            ModelColumnVM {
                model: m.as_str().to_string(),
                pricing_note: pricing_note_for(m.as_str(), pricing_key.as_deref()),
            }
        })
        .collect();
```

Add this helper near the other private presenter helpers:

```rust
fn pricing_note_for(model: &str, pricing_key: Option<&str>) -> String {
    match pricing_key {
        None => "no price".to_string(),
        Some(key) if key == model => "exact price".to_string(),
        Some(key) => format!("priced as {key}"),
    }
}
```

- [ ] **Step 5: Update existing presenter tests for the new column type**

Search within `crates/analysis/src/adapters/presenters/dashboard_presenter.rs` for assertions that compare `vm.model_columns` directly to `Vec<String>`. Replace them with model-field assertions.

For example, replace:

```rust
assert_eq!(vm.model_columns, vec!["opus".to_string(), "sonnet".to_string()]);
```

with:

```rust
let labels: Vec<&str> = vm.model_columns.iter().map(|c| c.model.as_str()).collect();
assert_eq!(labels, vec!["opus", "sonnet"]);
```

- [ ] **Step 6: Run presenter tests and verify pass**

Run:

```bash
cargo test -p analysis adapters::presenters::dashboard_presenter::tests -- --nocapture
```

Expected: PASS.

- [ ] **Step 7: Commit Task 2**

Run:

```bash
git add crates/analysis/src/adapters/view_models/dashboard_vm.rs crates/analysis/src/adapters/presenters/dashboard_presenter.rs
git commit -m "feat(analysis): present dashboard pricing notes"
```

Expected: commit succeeds.

---

### Task 3: Render pricing notes in dashboard column headers

**Files:**
- Modify: `crates/analysis/src/tui/renderer/dashboard.rs`

- [ ] **Step 1: Add a failing renderer test for pricing note visibility**

In `crates/analysis/src/tui/renderer/dashboard.rs`, update the test module imports/sample helpers as needed for `ModelColumnVM`, then add this test:

```rust
    #[test]
    fn model_header_shows_pricing_note() {
        let mut vm = sample_vm();
        vm.model_columns = vec![ModelColumnVM {
            model: "openai/gpt-5.1-codex-latest".to_string(),
            pricing_note: "priced as gpt5.1-codex".to_string(),
        }];
        vm.column_totals = vec![ModelBreakdownVM::default()];
        for row in &mut vm.rows {
            row.model_cells = vec![ModelBreakdownVM::default()];
        }

        let rendered = render_to_string(&vm, 80, 12);

        assert!(rendered.contains("priced as") || rendered.contains("↦"));
        assert!(rendered.contains("gpt5.1-codex"));
    }
```

- [ ] **Step 2: Run the renderer test and verify failure or compile failure**

Run:

```bash
cargo test -p analysis tui::renderer::dashboard::tests::model_header_shows_pricing_note -- --nocapture
```

Expected: FAIL or fail to compile because renderer still treats model columns as strings.

- [ ] **Step 3: Update renderer column access and width calculation**

In `crates/analysis/src/tui/renderer/dashboard.rs`, replace `vm.model_columns[mi].as_str()` usages with `vm.model_columns[mi].model.as_str()`.

In the `compute_sub` closure, include pricing-note width in the first sub-column budget:

```rust
        let model_label = vm.model_columns[mi].model.as_str();
        let pricing_note = vm.model_columns[mi].pricing_note.as_str();
```

Then replace the first-column width expansion with:

```rust
        widths[0] = widths[0].max(model_label.chars().count() as u16);
        widths[0] = widths[0].max(pricing_note.chars().count() as u16);
        widths[0] = widths[0].clamp(MIN_SUB_COL_WIDTH, MAX_NAME_WIDTH);
```

- [ ] **Step 4: Render compact note under the model name**

Replace the header-cell loop for model columns with this logic:

```rust
    for (idx, &mi) in visible_models.iter().enumerate() {
        let model_label = vm.model_columns[mi].model.as_str();
        let pricing_note = vm.model_columns[mi].pricing_note.as_str();
        for (si, sub) in SUB_LABELS.iter().enumerate() {
            if si == 0 {
                header_cells.push(model_header_with_note(
                    model_label,
                    pricing_note,
                    sub_widths[idx][si] as usize,
                ));
            } else {
                header_cells.push(two_line_header_sub("", sub, sub_widths[idx][si] as usize));
            }
        }
        header_cells.push(two_line_header_bar(sep_style));
    }
```

Add these helper functions near `two_line_header_sub`:

```rust
fn model_header_with_note(model: &str, pricing_note: &str, budget: usize) -> Cell<'static> {
    let model_trunc = truncate_for_header(model, budget);
    let note_trunc = truncate_for_header(pricing_note, budget);
    let text = Text::from(vec![
        Line::from(Span::styled(
            model_trunc,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(note_trunc, Style::default().fg(Color::DarkGray))),
    ]);
    Cell::from(text)
}

fn truncate_for_header(value: &str, budget: usize) -> String {
    if value.chars().count() > budget && budget > 0 {
        let take: String = value.chars().take(budget.saturating_sub(1)).collect();
        format!("{}…", take)
    } else {
        value.to_string()
    }
}
```

Then simplify `two_line_header_sub` so it uses the shared truncation helper:

```rust
fn two_line_header_sub(top: &str, bottom: &str, budget: usize) -> Cell<'static> {
    two_line_header(&truncate_for_header(top, budget), bottom)
}
```

- [ ] **Step 5: Update renderer sample view models**

In the renderer test module, update every direct assignment to `model_columns` from strings to `ModelColumnVM` values. Use this helper in the test module to reduce duplication:

```rust
    fn model_column(model: &str) -> ModelColumnVM {
        ModelColumnVM {
            model: model.to_string(),
            pricing_note: "exact price".to_string(),
        }
    }
```

For example, replace:

```rust
model_columns: vec!["model-a".to_string()],
```

with:

```rust
model_columns: vec![model_column("model-a")],
```

- [ ] **Step 6: Run renderer dashboard tests and verify pass**

Run:

```bash
cargo test -p analysis tui::renderer::dashboard::tests -- --nocapture
```

Expected: PASS.

- [ ] **Step 7: Commit Task 3**

Run:

```bash
git add crates/analysis/src/tui/renderer/dashboard.rs
git commit -m "feat(analysis): show pricing notes in dashboard headers"
```

Expected: commit succeeds.

---

### Task 4: Run final verification

**Files:**
- No code changes expected.

- [ ] **Step 1: Run the analysis crate tests**

Run:

```bash
cargo test -p analysis
```

Expected: PASS.

- [ ] **Step 2: Run workspace tests**

Run:

```bash
cargo test --workspace
```

Expected: PASS.

- [ ] **Step 3: Run clippy**

Run:

```bash
cargo clippy --workspace -- -D warnings
```

Expected: PASS with no warnings.

- [ ] **Step 4: Inspect the final diff**

Run:

```bash
git status --short
git diff --stat
git diff
```

Expected: only intended analysis dashboard pricing-note changes are present.

- [ ] **Step 5: Commit final fixes if verification required changes**

If Steps 1-3 required fixes after Task 3 was committed, commit them:

```bash
git add crates/analysis/src/application/dto/dashboard.rs crates/analysis/src/application/use_cases/get_dashboard.rs crates/analysis/src/adapters/view_models/dashboard_vm.rs crates/analysis/src/adapters/presenters/dashboard_presenter.rs crates/analysis/src/tui/renderer/dashboard.rs
git commit -m "fix(analysis): complete dashboard pricing note verification"
```

Expected: commit succeeds only if there were follow-up fixes. If no fixes were needed, skip this step.
