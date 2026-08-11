# Dashboard Total Token Breakdown Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the analysis dashboard's single `Total` column with a five-column token breakdown (`In / CR / Out / CW / Rsn`), and add a `Rsn` column to every per-model group.

**Architecture:** Two rings change, both inside the `analysis` crate. `adapters/view_models` gains a `reasoning` field and a new `TotalBreakdownVM` struct; `adapters/presenters/dashboard_presenter.rs` stops collapsing `TokenBreakdown` into a `u64`; `tui/renderer/dashboard.rs` widens the model groups from 5 to 6 sub-columns and swaps the one `Total` column for five. No domain, application, or gateway changes — `GetDashboardOutput` already carries the full `TokenBreakdown`.

**Tech Stack:** Rust (edition 2024), ratatui, `cargo test`, `cargo clippy`, `cargo fmt`.

**Spec:** `docs/superpowers/specs/2026-08-11-dashboard-total-token-breakdown-design.md`

## Global Constraints

- Edition 2024. Let-chains are stable; clippy runs with `-D warnings`.
- Ring dependency rule: `tui → adapters → application → domain`. Renderer consumes view models only.
- No new dependencies. No `anyhow`.
- View models are plain structs of pre-formatted `String`s. All formatting lives in the presenter via `fmt_cell` / `fmt_cost`.
- Zero token counts render as `—` (existing `fmt_cell` behaviour). The `Rsn` column stays visible even when every value is `—`.
- Column order is fixed: model groups are `In Out CR CW Rsn $`; the aggregate block is `In CR Out CW Rsn`. These orders differ deliberately — do not "harmonise" them.
- The pre-commit hook runs the full `cargo test` suite; allow ~5 minutes for each `git commit`.

---

### Task 1: Add the `Rsn` column to per-model groups

Adds `reasoning` to `ModelBreakdownVM`, fills it in the presenter, and widens each model group from 5 to 6 sub-columns in the renderer. The aggregate `Total` column is untouched by this task.

**Files:**
- Modify: `crates/analysis/src/adapters/view_models/dashboard_vm.rs`
- Modify: `crates/analysis/src/adapters/presenters/dashboard_presenter.rs`
- Modify: `crates/analysis/src/tui/renderer/dashboard.rs`

**Interfaces:**
- Consumes: `shared::domain::value_objects::TokenBreakdown` (fields `input`, `output`, `reasoning`, `cache_read`, `cache_write`, each a `TokenCount` with `.value() -> u64`).
- Produces: `ModelBreakdownVM { input, output, cache_read, cache_write, reasoning, cost }` — all `String`. Renderer helper `fn breakdown_texts(b: &ModelBreakdownVM) -> [&str; 6]` returning the six texts in render order (`input, output, cache_read, cache_write, reasoning, cost`).

- [ ] **Step 1: Write the failing presenter test**

Add to the `tests` module in `crates/analysis/src/adapters/presenters/dashboard_presenter.rs`, right after the existing `cells_preserve_token_breakdown` test:

```rust
    #[test]
    fn cells_expose_reasoning_tokens() {
        let mut r = row(2026, 4, 23, "m", 1_000, 0.0);
        r.tokens.reasoning = TokenCount::new(250);
        let vm = present(&output(vec![r]));
        assert_eq!(vm.rows[0].model_cells[0].reasoning, "250");
        assert_eq!(vm.column_totals[0].reasoning, "250");
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p analysis cells_expose_reasoning_tokens`
Expected: FAIL — compile error `no field 'reasoning' on type 'ModelBreakdownVM'`.

- [ ] **Step 3: Add the field to the view model**

In `crates/analysis/src/adapters/view_models/dashboard_vm.rs`, replace the `ModelBreakdownVM` struct with:

```rust
#[derive(Debug, Clone, Default)]
pub struct ModelBreakdownVM {
    pub input: String,
    pub output: String,
    pub cache_read: String,
    pub cache_write: String,
    pub reasoning: String,
    pub cost: String,
}
```

- [ ] **Step 4: Fill the field in the presenter**

In `crates/analysis/src/adapters/presenters/dashboard_presenter.rs`, replace the `ModelBreakdownVM { … }` literal at the end of `breakdown_cell` with:

```rust
    ModelBreakdownVM {
        input: fmt_cell(tb.input.value()),
        output: fmt_cell(tb.output.value()),
        cache_read: fmt_cell(tb.cache_read.value()),
        cache_write: fmt_cell(tb.cache_write.value()),
        reasoning: fmt_cell(tb.reasoning.value()),
        cost: cost_str,
    }
```

- [ ] **Step 5: Fix the three renderer test fixtures that build `ModelBreakdownVM` literally**

In the `tests` module of `crates/analysis/src/tui/renderer/dashboard.rs` there are three struct literals that now miss a field. Add a `reasoning` line to each, immediately before its `cost` line:

In `sample_vm`:

```rust
        let cell = ModelBreakdownVM {
            input: "27".into(),
            output: "3.0K".into(),
            cache_read: "372.0K".into(),
            cache_write: "82.9K".into(),
            reasoning: "1.1K".into(),
            cost: "$0.78".into(),
        };
```

In `many_models_in_narrow_terminal_still_show_full_first_model_name`:

```rust
        let cell = ModelBreakdownVM {
            input: "16.3K".into(),
            output: "947.0K".into(),
            cache_read: "240.3K".into(),
            cache_write: "3.83M".into(),
            reasoning: "12.0K".into(),
            cost: "$167.86".into(),
        };
```

In `vertical_separator_drawn_between_model_groups`:

```rust
        let cell = ModelBreakdownVM {
            input: "10".into(),
            output: "20".into(),
            cache_read: "30".into(),
            cache_write: "40".into(),
            reasoning: "50".into(),
            cost: "$0.10".into(),
        };
```

- [ ] **Step 6: Run the presenter test to verify it passes**

Run: `cargo test -p analysis cells_expose_reasoning_tokens`
Expected: PASS. (The renderer still shows only 5 sub-columns — that is the next step.)

- [ ] **Step 7: Write the failing renderer test**

Add to the `tests` module in `crates/analysis/src/tui/renderer/dashboard.rs`, after `model_header_shows_pricing_note`:

```rust
    #[test]
    fn model_group_header_includes_reasoning_column() {
        let vm = sample_vm("claude-opus-4-7");
        let rendered = render_to_string(&vm, 140, 12);
        assert!(
            rendered.contains("Rsn"),
            "expected Rsn sub-column header; got:\n{}",
            rendered
        );
        assert!(
            rendered.contains("1.1K"),
            "expected reasoning value in the model group; got:\n{}",
            rendered
        );
    }
```

- [ ] **Step 8: Run it to verify it fails**

Run: `cargo test -p analysis model_group_header_includes_reasoning_column`
Expected: FAIL — assertion `expected Rsn sub-column header`.

- [ ] **Step 9: Widen the sub-column label table**

In `crates/analysis/src/tui/renderer/dashboard.rs`, replace the `SUB_LABELS` constant:

```rust
const SUB_LABELS: [&str; 6] = ["In", "Out", "CR", "CW", "Rsn", "$"];
```

- [ ] **Step 10: Add the shared text-extraction helper**

In `crates/analysis/src/tui/renderer/dashboard.rs`, insert this function immediately above the existing `fn push_breakdown_cells`:

```rust
/// The six sub-column texts of a model group, in render order.
fn breakdown_texts(b: &ModelBreakdownVM) -> [&str; 6] {
    [
        &b.input,
        &b.output,
        &b.cache_read,
        &b.cache_write,
        &b.reasoning,
        &b.cost,
    ]
}

fn widen_to_fit(widths: &mut [u16; 6], cell: &ModelBreakdownVM) {
    for (si, text) in breakdown_texts(cell).iter().enumerate() {
        widths[si] = widths[si].max(text.chars().count() as u16);
    }
}
```

- [ ] **Step 11: Widen the width computation**

In `crates/analysis/src/tui/renderer/dashboard.rs`, replace the whole `compute_sub` closure (from `let compute_sub = |mi: usize| -> [u16; 5] {` through its closing `};`) with:

```rust
    let compute_sub = |mi: usize| -> [u16; 6] {
        let model_label = vm.model_columns[mi].model.as_str();
        let pricing_note = vm.model_columns[mi].pricing_note.as_str();
        let mut widths = [MIN_SUB_COL_WIDTH; 6];
        for (si, label) in SUB_LABELS.iter().enumerate() {
            widths[si] = widths[si].max(label.chars().count() as u16);
        }
        for r in &vm.rows {
            if let Some(cell) = r.model_cells.get(mi) {
                widen_to_fit(&mut widths, cell);
            }
        }
        if let Some(tot) = vm.column_totals.get(mi) {
            widen_to_fit(&mut widths, tot);
        }
        // First sub-col doubles as model name header — widen to fit.
        widths[0] = widths[0].max(model_label.chars().count() as u16);
        widths[0] = widths[0].max(pricing_note.chars().count() as u16);
        widths[0] = widths[0].clamp(MIN_SUB_COL_WIDTH, MAX_NAME_WIDTH);
        for w in widths.iter_mut().skip(1) {
            *w = (*w).clamp(MIN_SUB_COL_WIDTH, MAX_NUMERIC_WIDTH);
        }
        widths
    };
```

- [ ] **Step 12: Update the group-width budget arithmetic**

In the same file, in the model-fitting loop, replace the `sub_widths` declaration and the `group_width` line.

Replace:

```rust
    let mut sub_widths: Vec<[u16; 5]> = Vec::new();
```

with:

```rust
    let mut sub_widths: Vec<[u16; 6]> = Vec::new();
```

Replace:

```rust
        // 5 sub-cols + 1 group separator column = 6 gaps + separator width.
        let group_width: u16 = widths.iter().sum::<u16>() + spacing_per_col * 6 + GROUP_SEP_WIDTH;
```

with:

```rust
        // 6 sub-cols + 1 group separator column = 7 gaps + separator width.
        let group_width: u16 = widths.iter().sum::<u16>() + spacing_per_col * 7 + GROUP_SEP_WIDTH;
```

- [ ] **Step 13: Update the cell styling for six sub-columns**

In the same file, replace the body of `push_breakdown_cells` with:

```rust
fn push_breakdown_cells(cells: &mut Vec<Cell<'static>>, b: &ModelBreakdownVM, bold: bool) {
    let base = if bold {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    for (i, text) in breakdown_texts(b).iter().enumerate() {
        let style = if *text == "—" {
            base.fg(Color::DarkGray)
        } else if i == 5 {
            // Cost column: keep bright, even when not bold.
            base
        } else if i >= 2 {
            // CR / CW / Rsn: dim to keep focus on In/Out.
            if bold {
                base.fg(Color::DarkGray)
            } else {
                Style::default().fg(Color::DarkGray)
            }
        } else {
            base
        };
        cells.push(Cell::from(Span::styled((*text).to_string(), style)));
    }
}
```

- [ ] **Step 14: Give `model_header_shows_pricing_note` room for the sixth column**

That test renders an 80-column terminal with a 27-char model name. At 5 sub-columns the group needed exactly the available 51 cells; the sixth column pushes it to 57, so ratatui compresses every column proportionally and the pricing note is truncated away. Widen the terminal instead of shrinking the layout — 120 columns also leaves room for the five aggregate columns added in Task 2.

In `crates/analysis/src/tui/renderer/dashboard.rs`, inside `model_header_shows_pricing_note`, replace:

```rust
        let rendered = render_to_string(&vm, 80, 12);
```

with:

```rust
        let rendered = render_to_string(&vm, 120, 12);
```

- [ ] **Step 15: Run the full analysis suite**

Run: `cargo test -p analysis`
Expected: PASS, including `model_group_header_includes_reasoning_column`, `cells_expose_reasoning_tokens`, and the pre-existing width/truncation tests.

- [ ] **Step 16: Lint and format**

Run: `cargo fmt && cargo clippy -p analysis -- -D warnings`
Expected: no warnings, no diff left unformatted.

- [ ] **Step 17: Commit**

```bash
git add crates/analysis/src/adapters/view_models/dashboard_vm.rs \
        crates/analysis/src/adapters/presenters/dashboard_presenter.rs \
        crates/analysis/src/tui/renderer/dashboard.rs
git commit -m "feat(analysis): show reasoning tokens per model group"
```

---

### Task 2: Split the aggregate `Total` column into five

Replaces the single `Total` string on `DayPivotRowVM` and `DashboardViewModel` with a five-field struct, and renders it as five columns under a `Total` header.

**Files:**
- Modify: `crates/analysis/src/adapters/view_models/dashboard_vm.rs`
- Modify: `crates/analysis/src/adapters/view_models/mod.rs`
- Modify: `crates/analysis/src/adapters/presenters/dashboard_presenter.rs`
- Modify: `crates/analysis/src/tui/renderer/dashboard.rs`

**Interfaces:**
- Consumes: `ModelBreakdownVM.reasoning` and `breakdown_texts` from Task 1.
- Produces:
  - `TotalBreakdownVM { input, cache_read, output, cache_write, reasoning }` — all `String`, exported from `crate::adapters::view_models`.
  - `DayPivotRowVM.total: TotalBreakdownVM` (was `String`).
  - `DashboardViewModel.grand_total: TotalBreakdownVM` (was `String`).
  - Presenter helper `fn total_breakdown_cell(tb: &TokenBreakdown) -> TotalBreakdownVM`.
  - Renderer helper `fn total_texts(t: &TotalBreakdownVM) -> [&str; 5]` in aggregate render order (`input, cache_read, output, cache_write, reasoning`).

- [ ] **Step 1: Write the failing presenter tests**

In `crates/analysis/src/adapters/presenters/dashboard_presenter.rs`, replace the existing `row_total_sums_all_tokens_across_models` and `column_totals_and_grand_total_are_populated` tests with these three:

```rust
    #[test]
    fn row_total_splits_tokens_by_category_across_models() {
        let vm = present(&output(vec![
            row_full((2026, 4, 23), "a", (100, 50, 20, 10)),
            row_full((2026, 4, 23), "b", (200, 100, 30, 20)),
        ]));
        let total = &vm.rows[0].total;
        assert_eq!(total.input, "300");
        assert_eq!(total.cache_read, "50");
        assert_eq!(total.output, "150");
        assert_eq!(total.cache_write, "30");
        assert_eq!(total.reasoning, "—");
    }

    #[test]
    fn row_total_carries_reasoning_tokens() {
        let mut r = row_full((2026, 4, 23), "a", (100, 50, 20, 10));
        r.tokens.reasoning = TokenCount::new(7);
        let vm = present(&output(vec![r]));
        assert_eq!(vm.rows[0].total.reasoning, "7");
        assert_eq!(vm.grand_total.reasoning, "7");
    }

    #[test]
    fn column_totals_and_grand_total_are_populated() {
        let vm = present(&output(vec![
            row_full((2026, 4, 23), "a", (100, 50, 20, 10)),
            row_full((2026, 4, 22), "a", (200, 100, 30, 20)),
        ]));
        assert_eq!(vm.column_totals.len(), 1);
        assert_eq!(vm.column_totals[0].input, "300");
        assert_eq!(vm.column_totals[0].output, "150");
        assert_eq!(vm.grand_total.input, "300");
        assert_eq!(vm.grand_total.cache_read, "50");
        assert_eq!(vm.grand_total.output, "150");
        assert_eq!(vm.grand_total.cache_write, "30");
    }
```

Note: `row_full` maps its tuple as `(input, output, cache_read, cache_write)`, so `(100, 50, 20, 10)` means input 100, output 50, cache_read 20, cache_write 10.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p analysis row_total_splits_tokens_by_category_across_models`
Expected: FAIL — compile error `no field 'input' on type 'String'`.

- [ ] **Step 3: Add `TotalBreakdownVM` and retype the two total fields**

In `crates/analysis/src/adapters/view_models/dashboard_vm.rs`, change the `grand_total` field of `DashboardViewModel` and the `total` field of `DayPivotRowVM`, then append the new struct. The file's structs become:

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
    pub grand_total: TotalBreakdownVM,
    pub grand_cost: String,
    pub empty: bool,
}

#[derive(Debug, Clone)]
pub struct DayPivotRowVM {
    pub date_label: String,
    pub model_cells: Vec<ModelBreakdownVM>,
    pub total: TotalBreakdownVM,
    pub total_cost: String,
}

#[derive(Debug, Clone, Default)]
pub struct ModelBreakdownVM {
    pub input: String,
    pub output: String,
    pub cache_read: String,
    pub cache_write: String,
    pub reasoning: String,
    pub cost: String,
}

/// Aggregate token counts across every model, in the order they are rendered:
/// raw input, input served from cache, raw output, output written to cache,
/// reasoning. No cost field — the dashboard renders `Cost` as its own column.
#[derive(Debug, Clone, Default)]
pub struct TotalBreakdownVM {
    pub input: String,
    pub cache_read: String,
    pub output: String,
    pub cache_write: String,
    pub reasoning: String,
}
```

- [ ] **Step 4: Export the new struct**

In `crates/analysis/src/adapters/view_models/mod.rs`, replace the dashboard re-export line with:

```rust
pub use dashboard_vm::{
    DashboardViewModel, DayPivotRowVM, ModelBreakdownVM, ModelColumnVM, TotalBreakdownVM,
};
```

- [ ] **Step 5: Accumulate a `TokenBreakdown` in the presenter's row loop**

In `crates/analysis/src/adapters/presenters/dashboard_presenter.rs`, replace the `let rows: Vec<DayPivotRowVM> = …` block with:

```rust
    let rows: Vec<DayPivotRowVM> = days
        .iter()
        .map(|date| {
            let mut row_tokens = TokenBreakdown::default();
            let mut row_cost: f64 = 0.0;
            let model_cells: Vec<ModelBreakdownVM> = columns
                .iter()
                .map(|m| {
                    let (tb, cost) = cell_index
                        .get(&(*date, m.clone()))
                        .copied()
                        .unwrap_or_default();
                    row_tokens += tb;
                    row_cost += cost;
                    breakdown_cell(&tb, cost, m, &out.unpriced_models)
                })
                .collect();
            DayPivotRowVM {
                date_label: format_date_md(*date),
                model_cells,
                total: total_breakdown_cell(&row_tokens),
                total_cost: cost_cell(row_cost, row_tokens.total().value()),
            }
        })
        .collect();
```

- [ ] **Step 6: Build the grand total from a `TokenBreakdown`**

In the same file, replace the four grand-total lines:

```rust
    let grand_total_raw: u64 = out.rows.iter().map(|r| r.tokens.total().value()).sum();
    let grand_total = total_cell(grand_total_raw);
    let grand_cost_raw: f64 = out.rows.iter().map(|r| r.cost.value()).sum();
    let grand_cost = cost_cell(grand_cost_raw, grand_total_raw);
```

with:

```rust
    let grand_tokens = out
        .rows
        .iter()
        .fold(TokenBreakdown::default(), |acc, r| acc + r.tokens);
    let grand_total = total_breakdown_cell(&grand_tokens);
    let grand_cost_raw: f64 = out.rows.iter().map(|r| r.cost.value()).sum();
    let grand_cost = cost_cell(grand_cost_raw, grand_tokens.total().value());
```

- [ ] **Step 7: Replace `total_cell` with `total_breakdown_cell`**

In the same file, replace the whole `total_cell` function:

```rust
fn total_cell(n: u64) -> String {
    fmt_cell(n)
}
```

with:

```rust
fn total_breakdown_cell(tb: &TokenBreakdown) -> TotalBreakdownVM {
    TotalBreakdownVM {
        input: fmt_cell(tb.input.value()),
        cache_read: fmt_cell(tb.cache_read.value()),
        output: fmt_cell(tb.output.value()),
        cache_write: fmt_cell(tb.cache_write.value()),
        reasoning: fmt_cell(tb.reasoning.value()),
    }
}
```

Then add `TotalBreakdownVM` to the view-model import at the top of the file:

```rust
use crate::adapters::view_models::{
    DashboardViewModel, DayPivotRowVM, ModelBreakdownVM, ModelColumnVM, TotalBreakdownVM,
};
```

- [ ] **Step 8: Run the presenter tests**

Run: `cargo test -p analysis --lib adapters::presenters::dashboard_presenter`
Expected: PASS for all presenter tests. The renderer will still fail to compile — that is the next step.

- [ ] **Step 9: Write the failing renderer test**

In the `tests` module of `crates/analysis/src/tui/renderer/dashboard.rs`, add after `model_group_header_includes_reasoning_column`:

```rust
    #[test]
    fn aggregate_block_renders_five_total_columns() {
        let vm = sample_vm("claude-opus-4-7");
        let rendered = render_to_string(&vm, 160, 12);
        // "Rsn" twice: once in the model group header, once in the aggregate
        // block header. Asserting on "Total" alone would pass on the
        // grand-total row label even with no aggregate header at all.
        assert_eq!(
            rendered.matches("Rsn").count(),
            2,
            "expected Rsn in both the model group and the aggregate header; got:\n{}",
            rendered
        );
        for value in ["9.9K", "8.8K", "7.7K", "6.6K", "5.5K"] {
            assert!(
                rendered.contains(value),
                "expected aggregate value {} in output; got:\n{}",
                value,
                rendered
            );
        }
    }
```

- [ ] **Step 10: Update the renderer test fixtures**

In the same `tests` module, three fixtures build totals as `String`. Replace each.

In `sample_vm`, replace `total: "450K".into(),` with:

```rust
                total: TotalBreakdownVM {
                    input: "9.9K".into(),
                    cache_read: "8.8K".into(),
                    output: "7.7K".into(),
                    cache_write: "6.6K".into(),
                    reasoning: "5.5K".into(),
                },
```

and replace `grand_total: "450K".into(),` with:

```rust
            grand_total: TotalBreakdownVM {
                input: "9.9K".into(),
                cache_read: "8.8K".into(),
                output: "7.7K".into(),
                cache_write: "6.6K".into(),
                reasoning: "5.5K".into(),
            },
```

In `many_models_in_narrow_terminal_still_show_full_first_model_name`, replace `total: "9.31M".into(),` with:

```rust
                total: TotalBreakdownVM {
                    input: "163K".into(),
                    cache_read: "2.40M".into(),
                    output: "9.47M".into(),
                    cache_write: "38.3M".into(),
                    reasoning: "120K".into(),
                },
```

and replace `grand_total: "9.31M".into(),` with:

```rust
            grand_total: TotalBreakdownVM {
                input: "163K".into(),
                cache_read: "2.40M".into(),
                output: "9.47M".into(),
                cache_write: "38.3M".into(),
                reasoning: "120K".into(),
            },
```

In `vertical_separator_drawn_between_model_groups`, replace `total: "200".into(),` with:

```rust
                total: TotalBreakdownVM {
                    input: "20".into(),
                    cache_read: "60".into(),
                    output: "40".into(),
                    cache_write: "80".into(),
                    reasoning: "100".into(),
                },
```

and replace `grand_total: "200".into(),` with:

```rust
            grand_total: TotalBreakdownVM {
                input: "20".into(),
                cache_read: "60".into(),
                output: "40".into(),
                cache_write: "80".into(),
                reasoning: "100".into(),
            },
```

Extend the `use` inside the `tests` module:

```rust
    use crate::adapters::view_models::{
        DashboardViewModel, DayPivotRowVM, ModelBreakdownVM, ModelColumnVM, TotalBreakdownVM,
    };
```

Finally, `vertical_separator_drawn_between_model_groups` asserts a separator appears *between* two model groups, but the five aggregate columns leave only 70 cells of budget at 120 columns — enough for one 44-cell group, not two. Widen it so the test keeps testing what it claims. Replace:

```rust
        let rendered = render_to_string(&vm, 120, 12);
```

with:

```rust
        let rendered = render_to_string(&vm, 160, 12);
```

- [ ] **Step 11: Run it to verify it fails**

Run: `cargo test -p analysis aggregate_block_renders_five_total_columns`
Expected: FAIL — compile error on `r.total.clone()` / `vm.grand_total.clone()` in `draw` (`Cell::from` cannot take a `TotalBreakdownVM`).

- [ ] **Step 12: Swap the width constant for the aggregate label table**

In `crates/analysis/src/tui/renderer/dashboard.rs`, delete the line:

```rust
const TOTAL_COL_WIDTH: u16 = 8;
```

and add, just below `SUB_LABELS`:

```rust
const TOTAL_SUB_LABELS: [&str; 5] = ["In", "CR", "Out", "CW", "Rsn"];
```

Add `TotalBreakdownVM` to the view-model import near the top of the file:

```rust
use crate::adapters::view_models::{DashboardViewModel, ModelBreakdownVM, TotalBreakdownVM};
```

- [ ] **Step 13: Add the aggregate text helper and cell pusher**

In the same file, insert both functions immediately after `push_breakdown_cells`:

```rust
/// The five aggregate texts, in render order.
fn total_texts(t: &TotalBreakdownVM) -> [&str; 5] {
    [
        &t.input,
        &t.cache_read,
        &t.output,
        &t.cache_write,
        &t.reasoning,
    ]
}

fn push_total_cells(cells: &mut Vec<Cell<'static>>, t: &TotalBreakdownVM, style: Style) {
    for text in total_texts(t) {
        let cell_style = if text == "—" {
            style.fg(Color::DarkGray)
        } else {
            style
        };
        cells.push(Cell::from(Span::styled(text.to_string(), cell_style)));
    }
}
```

- [ ] **Step 14: Compute the aggregate column widths**

In `draw`, insert this block immediately after the `compute_sub` closure ends and before the comment `// Fit as many model groups as the table area allows…`:

```rust
    // Aggregate block widths, measured over every row plus the grand total.
    let mut total_widths = [MIN_SUB_COL_WIDTH; 5];
    for (si, label) in TOTAL_SUB_LABELS.iter().enumerate() {
        total_widths[si] = total_widths[si].max(label.chars().count() as u16);
    }
    // The first aggregate column carries the "Total" header on its top line.
    total_widths[0] = total_widths[0].max("Total".chars().count() as u16);
    for r in &vm.rows {
        for (si, text) in total_texts(&r.total).iter().enumerate() {
            total_widths[si] = total_widths[si].max(text.chars().count() as u16);
        }
    }
    for (si, text) in total_texts(&vm.grand_total).iter().enumerate() {
        total_widths[si] = total_widths[si].max(text.chars().count() as u16);
    }
    for w in total_widths.iter_mut() {
        *w = (*w).clamp(MIN_SUB_COL_WIDTH, MAX_NUMERIC_WIDTH);
    }
```

- [ ] **Step 15: Update the budget arithmetic**

In `draw`, replace:

```rust
    let fixed = DATE_COL_WIDTH + TOTAL_COL_WIDTH + COST_COL_WIDTH;
```

with:

```rust
    let fixed = DATE_COL_WIDTH + total_widths.iter().sum::<u16>() + COST_COL_WIDTH;
```

and replace:

```rust
        // spacing for Date|...|Total|Cost — 3 gaps outside model groups.
        .saturating_sub(spacing_per_col * 3);
```

with:

```rust
        // spacing for Date|...|In|CR|Out|CW|Rsn|Cost — 7 gaps outside model groups.
        .saturating_sub(spacing_per_col * 7);
```

- [ ] **Step 16: Emit five aggregate columns instead of one**

In `draw`, replace:

```rust
    all_widths.push(TOTAL_COL_WIDTH);
    is_sep.push(false);
    all_widths.push(COST_COL_WIDTH);
    is_sep.push(false);
```

with:

```rust
    for &w in &total_widths {
        all_widths.push(w);
        is_sep.push(false);
    }
    all_widths.push(COST_COL_WIDTH);
    is_sep.push(false);
```

- [ ] **Step 17: Update the header row**

In `draw`, replace:

```rust
    header_cells.push(two_line_header("", "Total"));
    header_cells.push(two_line_header("", "Cost"));
```

with:

```rust
    for (si, label) in TOTAL_SUB_LABELS.iter().enumerate() {
        let top = if si == 0 { "Total" } else { "" };
        header_cells.push(two_line_header(top, label));
    }
    header_cells.push(two_line_header("", "Cost"));
```

- [ ] **Step 18: Update the per-day body rows**

In `draw`, replace:

```rust
        cells.push(Cell::from(Span::styled(
            r.total.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        cells.push(Cell::from(Span::styled(
            r.total_cost.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        )));
```

with:

```rust
        push_total_cells(
            &mut cells,
            &r.total,
            Style::default().add_modifier(Modifier::BOLD),
        );
        cells.push(Cell::from(Span::styled(
            r.total_cost.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        )));
```

- [ ] **Step 19: Update the grand-total row**

In `draw`, replace:

```rust
    total_cells.push(Cell::from(Span::styled(
        vm.grand_total.clone(),
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )));
    total_cells.push(Cell::from(Span::styled(
        vm.grand_cost.clone(),
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )));
```

with:

```rust
    push_total_cells(
        &mut total_cells,
        &vm.grand_total,
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    );
    total_cells.push(Cell::from(Span::styled(
        vm.grand_cost.clone(),
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )));
```

- [ ] **Step 20: Run the full analysis suite**

Run: `cargo test -p analysis`
Expected: PASS, including `aggregate_block_renders_five_total_columns`, the three new presenter tests, and every pre-existing test.

- [ ] **Step 21: Lint and format**

Run: `cargo fmt && cargo clippy --workspace -- -D warnings`
Expected: no warnings.

- [ ] **Step 22: Verify the workspace still builds and tests green**

Run: `cargo test --workspace`
Expected: PASS.

- [ ] **Step 23: Eyeball the real TUI**

Run: `cargo run -p analysis`
Expected: the dashboard shows six sub-columns per model (`In Out CR CW Rsn $`) and a five-column block headed `Total` (`In CR Out CW Rsn`) before `Cost`. On a narrow terminal, `shift+←/→` still scrolls the model groups. Press `q` to quit.

- [ ] **Step 24: Commit**

```bash
git add crates/analysis/src/adapters/view_models/dashboard_vm.rs \
        crates/analysis/src/adapters/view_models/mod.rs \
        crates/analysis/src/adapters/presenters/dashboard_presenter.rs \
        crates/analysis/src/tui/renderer/dashboard.rs
git commit -m "feat(analysis): split dashboard total into token category columns"
```
