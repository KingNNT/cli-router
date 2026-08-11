# Dashboard total token breakdown — design

**Date:** 2026-08-11
**Crate:** `analysis`
**Status:** approved

## Problem

The analysis dashboard collapses every token category into a single `Total`
column. A day's row shows how many tokens were spent, but not what kind. The
per-model groups already break tokens down (`In / Out / CR / CW`), so the
aggregate is the one place where that detail is lost — exactly where it is most
useful for spotting a cache-read-heavy day or an output-heavy one.

Reasoning tokens are worse off: they are summed into `Total` but never displayed
anywhere, so the per-model columns silently fail to add up to the row total on
OpenCode data.

## Goal

Replace the single `Total` column with a five-column breakdown, and surface
reasoning tokens in both the aggregate block and the per-model groups.

## Layout

```
      │ claude-opus-4                       │ Total                     │
Date  │ In    Out   CR    CW    Rsn   $     │ In    CR    Out   CW  Rsn │ Cost
──────┼─────────────────────────────────────┼───────────────────────────┼──────
08-11 │ 1.2k  800   3.4k  120   —     $0.05 │ 1.2k  3.4k  800   120  —  │ $0.05
──────┼─────────────────────────────────────┼───────────────────────────┼──────
Total │ 5.1k  3.2k  14k   480   —     $0.21 │ 5.1k  14k   3.2k  480  —  │ $0.21
```

- The aggregate block holds five columns in the order `In` (input), `CR` (input
  served from cache), `Out` (output), `CW` (output written to cache), `Rsn`
  (reasoning). This order pairs each raw category with its cache counterpart.
- The word `Total` sits on the header's top line above the first of the five,
  reusing the two-line header the model groups already use.
- Each model group grows from five to six sub-columns: `In Out CR CW Rsn $`.
  The existing `In / Out / CR / CW` order is kept so the change reads as an
  addition, not a reshuffle.
- `Cost` stays as the last column.

The two orders differ deliberately: the model groups keep their established
layout, while the aggregate block uses the raw/cache pairing that motivated this
change.

## Components

### `adapters/view_models/dashboard_vm.rs`

- `ModelBreakdownVM` gains `reasoning: String`.
- New `TotalBreakdownVM { input, cache_read, output, cache_write, reasoning }` —
  five pre-formatted strings, no cost field, because the `Cost` column is
  rendered separately.
- `DayPivotRowVM.total` changes from `String` to `TotalBreakdownVM`.
- `DashboardViewModel.grand_total` changes from `String` to `TotalBreakdownVM`.
- `total_cost` and `grand_cost` are unchanged.

### `adapters/presenters/dashboard_presenter.rs`

- The per-day loop accumulates a `TokenBreakdown` instead of a `u64`, so each
  category survives into the view model. The running cost accumulator is
  unchanged.
- `breakdown_cell` fills the new `reasoning` field via the existing `fmt_cell`.
- A new `total_breakdown_cell(&TokenBreakdown) -> TotalBreakdownVM` formats the
  five aggregate cells with `fmt_cell`. `total_cell` is removed — it was a
  one-line alias for `fmt_cell`.
- The grand total sums `TokenBreakdown` across `out.rows`.
- `cost_cell` still needs a token count to decide between `—` and a price; call
  sites pass `tb.total().value()`.

Zero renders as `—` (existing `fmt_cell` behaviour). With Claude Code and Codex
data, `Rsn` is therefore a column of dashes — accepted, because only OpenCode
populates reasoning and hiding the column conditionally would make the table
width jump between sources.

### `tui/renderer/dashboard.rs`

- `SUB_LABELS` becomes `["In", "Out", "CR", "CW", "Rsn", "$"]` (6 entries); a new
  `TOTAL_SUB_LABELS: [&str; 5] = ["In", "CR", "Out", "CW", "Rsn"]` drives the
  aggregate block.
- `compute_sub` returns `[u16; 6]`; the reasoning width is measured from the
  same row and column-total sources as the other numeric cells.
- Model group width becomes `widths.iter().sum() + spacing_per_col * 7 +
  GROUP_SEP_WIDTH` (6 sub-columns + 1 separator column = 7 gaps).
- `TOTAL_COL_WIDTH` is removed. The five aggregate widths are computed from row
  and grand-total content, clamped to `MIN_SUB_COL_WIDTH..=MAX_NUMERIC_WIDTH`,
  and their sum plus their spacing is subtracted from the model budget before
  the fitting loop runs.
- `push_breakdown_cells` handles six entries: cost moves to index 5; indices 2–4
  (`CR`, `CW`, `Rsn`) get the dim style that currently applies to indices ≥ 2.
- A parallel `push_total_cells` renders the five aggregate cells bold, matching
  today's bold `Total` column, with the grand-total row in yellow as before.
- The aggregate header uses `two_line_header("Total", "In")` for the first
  column and `two_line_header("", label)` for the rest.

### Data flow

Unchanged. `GetDashboardOutput` already carries a full `TokenBreakdown` per
`DayModelRow`; the presenter simply stops discarding four fifths of it on the
way to the view model. No repository, port, or DTO changes.

## Error handling

No new failure modes. All new values are formatted counts from data already in
memory. Width computation uses the existing saturating arithmetic, so a terminal
too narrow to hold everything degrades by dropping model groups: the fitting
loop admits a group only when its full requested width still fits the remaining
budget, and stops at the first one that does not. Nothing is ever squeezed in.
Forcing a group that overflows would be worse than dropping it — every column is
a `Constraint::Length`, so ratatui compresses *all* of them proportionally and
clips the aggregate numbers with no ellipsis (`$1376.87` renders as `$1376.`).
The footer reports `0 of N models fit  (shift+← → to scroll)` when none are
visible, and the horizontal scroll still reaches them.

## Trade-offs

The aggregate block grows from 9 columns of terminal width (an 8-wide `Total`
plus its 1-column gap) to 30 — five columns at the 5-column minimum plus five
gaps — and up to 45 when every category needs the full 8-character numeric
budget. Each model group grows by 6 (one extra 5-wide numeric column plus its
gap), up to 9 if `Rsn` needs all 8 characters.

The practical consequence: with realistic Claude Code data and a 15-character
model name, the first model group needs 103 columns of terminal width. Below
that — including at 80 and 100 columns — the dashboard renders `Date`, the
five-column aggregate block, and `Cost`, with no model group at all. This is
accepted: horizontal scrolling already exists for the model groups, and the
aggregate block is the part users read first, so it is the part that must stay
legible when space runs out.

## Testing

Presenter tests:

- Update `row_total_sums_all_tokens_across_models` and
  `column_totals_and_grand_total_are_populated` for the struct-valued totals.
- Update `cells_preserve_token_breakdown`, `missing_cell_renders_all_dashes`,
  and the `cost_cell_*` tests for the new `reasoning` field.
- New: a row with all five categories populated puts each count in its own
  aggregate cell.
- New: the five aggregate cells sum to the total tokens of every model on that
  day.
- New: reasoning tokens reach `ModelBreakdownVM.reasoning`.

Renderer tests:

- Update `sample_vm` for the new fields.
- New: the rendered header contains `Total` and `Rsn`.
- Existing width and truncation tests must still pass with 6 sub-columns.
