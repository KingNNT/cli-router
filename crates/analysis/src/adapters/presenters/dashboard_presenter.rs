use std::collections::BTreeMap;

use chrono::NaiveDate;

use crate::adapters::presenters::formatting::{fmt_cost, fmt_num, fmt_num_compact, format_date_md};
use crate::adapters::view_models::{DashboardViewModel, DayPivotRowVM, ModelBreakdownVM};
use crate::application::dto::GetDashboardOutput;
use shared::domain::value_objects::{ModelId, TokenBreakdown};

pub fn present_dashboard(
    out: &GetDashboardOutput,
    window_tabs: &[&str],
    selected_window_index: usize,
) -> DashboardViewModel {
    // Column order: models sorted by total tokens DESC, tie-break by name.
    let mut model_totals: BTreeMap<ModelId, u64> = BTreeMap::new();
    for r in &out.rows {
        *model_totals.entry(r.model.clone()).or_insert(0) += r.tokens.total().value();
    }
    let mut ordered_models: Vec<(ModelId, u64)> = model_totals.into_iter().collect();
    ordered_models.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.as_str().cmp(b.0.as_str())));
    let columns: Vec<ModelId> = ordered_models.into_iter().map(|(m, _)| m).collect();
    let model_columns: Vec<String> = columns.iter().map(|m| m.as_str().to_string()).collect();

    // (date, model) -> summed (breakdown, cost).
    let mut cell_index: BTreeMap<(NaiveDate, ModelId), (TokenBreakdown, f64)> = BTreeMap::new();
    for r in &out.rows {
        let entry = cell_index.entry((r.date, r.model.clone())).or_default();
        entry.0 += r.tokens;
        entry.1 += r.cost.value();
    }

    // Unique days, sorted DESC.
    let mut day_set: BTreeMap<NaiveDate, ()> = BTreeMap::new();
    for r in &out.rows {
        day_set.insert(r.date, ());
    }
    let days: Vec<NaiveDate> = day_set.into_iter().rev().map(|(d, _)| d).collect();

    let rows: Vec<DayPivotRowVM> = days
        .iter()
        .map(|date| {
            let mut row_total: u64 = 0;
            let mut row_cost: f64 = 0.0;
            let model_cells: Vec<ModelBreakdownVM> = columns
                .iter()
                .map(|m| {
                    let (tb, cost) = cell_index
                        .get(&(*date, m.clone()))
                        .copied()
                        .unwrap_or_default();
                    row_total += tb.total().value();
                    row_cost += cost;
                    breakdown_cell(&tb, cost, m, &out.unpriced_models)
                })
                .collect();
            DayPivotRowVM {
                date_label: format_date_md(*date),
                model_cells,
                total: total_cell(row_total),
                total_cost: cost_cell(row_cost, row_total),
            }
        })
        .collect();

    // Column totals per model (summed across all days).
    let column_totals: Vec<ModelBreakdownVM> = columns
        .iter()
        .map(|m| {
            let mut tb = TokenBreakdown::default();
            let mut cost: f64 = 0.0;
            for d in &days {
                if let Some((cell_tb, cell_cost)) = cell_index.get(&(*d, m.clone())) {
                    tb += *cell_tb;
                    cost += *cell_cost;
                }
            }
            breakdown_cell(&tb, cost, m, &out.unpriced_models)
        })
        .collect();

    let grand_total_raw: u64 = out.rows.iter().map(|r| r.tokens.total().value()).sum();
    let grand_total = total_cell(grand_total_raw);
    let grand_cost_raw: f64 = out.rows.iter().map(|r| r.cost.value()).sum();
    let grand_cost = cost_cell(grand_cost_raw, grand_total_raw);

    let pricing_note = if out.missing_pricing_count > 0 {
        Some(format!(
            "· {} models missing pricing",
            out.missing_pricing_count
        ))
    } else {
        None
    };

    let has_nonzero_tokens = out.rows.iter().any(|r| r.tokens.total().value() > 0);
    let banner = if out.overview.cost.value() == 0.0 && has_nonzero_tokens {
        Some(
            "No priced activity in this window. Press `s` to sync pricing, or `d` to expand the time window."
                .to_string(),
        )
    } else {
        None
    };

    DashboardViewModel {
        status_msgs: fmt_num(out.overview.message_count),
        status_cost: fmt_cost(out.overview.cost.value()),
        pricing_note,
        banner,
        window_tabs: window_tabs.iter().map(|s| s.to_string()).collect(),
        selected_window_index,
        model_columns,
        rows,
        column_totals,
        grand_total,
        grand_cost,
        empty: out.rows.is_empty(),
    }
}

fn fmt_cell(n: u64) -> String {
    if n == 0 {
        "—".to_string()
    } else {
        fmt_num_compact(n)
    }
}

fn breakdown_cell(
    tb: &TokenBreakdown,
    cost: f64,
    model: &ModelId,
    unpriced: &std::collections::HashSet<ModelId>,
) -> ModelBreakdownVM {
    let tokens = tb.total().value();
    let cost_str = if tokens == 0 {
        "—".to_string()
    } else if cost == 0.0 && unpriced.contains(model) {
        "?".to_string()
    } else {
        fmt_cost(cost)
    };
    ModelBreakdownVM {
        input: fmt_cell(tb.input.value()),
        output: fmt_cell(tb.output.value()),
        cache_read: fmt_cell(tb.cache_read.value()),
        cache_write: fmt_cell(tb.cache_write.value()),
        cost: cost_str,
    }
}

fn total_cell(n: u64) -> String {
    fmt_cell(n)
}

fn cost_cell(cost: f64, tokens: u64) -> String {
    if tokens == 0 {
        "—".to_string()
    } else {
        fmt_cost(cost)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::dto::Filter;
    use shared::domain::entities::{DayModelRow, Overview};
    use shared::domain::value_objects::{Cost, DateRange, ModelId, TokenCount};
    use std::collections::HashSet;

    fn row(y: i32, m: u32, d: u32, model: &str, input: u64, cost: f64) -> DayModelRow {
        DayModelRow {
            date: NaiveDate::from_ymd_opt(y, m, d).unwrap(),
            model: ModelId::new(model).unwrap(),
            tokens: TokenBreakdown {
                input: TokenCount::new(input),
                ..Default::default()
            },
            cost: Cost::new(cost).unwrap(),
        }
    }

    fn row_full(ymd: (i32, u32, u32), model: &str, tokens: (u64, u64, u64, u64)) -> DayModelRow {
        DayModelRow {
            date: NaiveDate::from_ymd_opt(ymd.0, ymd.1, ymd.2).unwrap(),
            model: ModelId::new(model).unwrap(),
            tokens: TokenBreakdown {
                input: TokenCount::new(tokens.0),
                output: TokenCount::new(tokens.1),
                cache_read: TokenCount::new(tokens.2),
                cache_write: TokenCount::new(tokens.3),
                ..Default::default()
            },
            cost: Cost::new(0.0).unwrap(),
        }
    }

    fn output(rows: Vec<DayModelRow>) -> GetDashboardOutput {
        GetDashboardOutput {
            filter_applied: Filter {
                date_range: Some(DateRange::last_n_days(
                    NaiveDate::from_ymd_opt(2026, 4, 23).unwrap(),
                    30,
                )),
                ..Filter::default()
            },
            overview: Overview {
                message_count: 3,
                cost: Cost::new(4.2).unwrap(),
                ..Overview::default()
            },
            rows,
            missing_pricing_count: 0,
            unpriced_models: HashSet::new(),
            last_pricing_sync: None,
        }
    }

    fn present(out: &GetDashboardOutput) -> DashboardViewModel {
        present_dashboard(out, &["30d"], 0)
    }

    #[test]
    fn empty_rows_produce_empty_vm() {
        let vm = present(&output(vec![]));
        assert!(vm.empty);
        assert!(vm.rows.is_empty());
        assert!(vm.model_columns.is_empty());
    }

    #[test]
    fn rows_are_sorted_desc_by_date() {
        let vm = present(&output(vec![
            row(2026, 4, 22, "m", 100, 1.0),
            row(2026, 4, 23, "m", 200, 2.0),
        ]));
        assert_eq!(vm.rows.len(), 2);
        assert_eq!(vm.rows[0].date_label, "04-23");
        assert_eq!(vm.rows[1].date_label, "04-22");
    }

    #[test]
    fn model_columns_ordered_by_total_tokens_desc() {
        let vm = present(&output(vec![
            row(2026, 4, 23, "cheap", 100, 0.0),
            row(2026, 4, 23, "heavy", 5_000, 0.0),
            row(2026, 4, 22, "cheap", 50, 0.0),
        ]));
        assert_eq!(
            vm.model_columns,
            vec!["heavy".to_string(), "cheap".to_string()]
        );
    }

    #[test]
    fn cells_preserve_token_breakdown() {
        let vm = present(&output(vec![row_full(
            (2026, 4, 23),
            "m",
            (1_000, 500, 200, 100),
        )]));
        let cell = &vm.rows[0].model_cells[0];
        assert_eq!(cell.input, "1.0K");
        assert_eq!(cell.output, "500");
        assert_eq!(cell.cache_read, "200");
        assert_eq!(cell.cache_write, "100");
    }

    #[test]
    fn cost_cell_propagates_per_day_per_model() {
        // Two rows on same day/model should sum.
        let mut r1 = row(2026, 4, 23, "m", 100, 0.5);
        let r2 = row(2026, 4, 23, "m", 200, 1.25);
        r1.tokens.output = TokenCount::new(50);
        let vm = present(&output(vec![r1, r2]));
        assert_eq!(vm.rows[0].model_cells[0].cost, "$1.75");
        assert_eq!(vm.rows[0].total_cost, "$1.75");
        assert_eq!(vm.grand_cost, "$1.75");
    }

    #[test]
    fn cost_cell_shows_question_mark_for_unpriced_with_tokens() {
        let m = ModelId::new("unknown/x").unwrap();
        let mut unpriced = HashSet::new();
        unpriced.insert(m.clone());
        let r = DayModelRow {
            date: NaiveDate::from_ymd_opt(2026, 4, 23).unwrap(),
            model: m,
            tokens: TokenBreakdown {
                input: TokenCount::new(500),
                ..Default::default()
            },
            cost: Cost::new(0.0).unwrap(),
        };
        let out = GetDashboardOutput {
            filter_applied: Filter::default(),
            overview: Overview::default(),
            rows: vec![r],
            missing_pricing_count: 1,
            unpriced_models: unpriced,
            last_pricing_sync: None,
        };
        let vm = present(&out);
        assert_eq!(vm.rows[0].model_cells[0].cost, "?");
    }

    #[test]
    fn cost_cell_shows_dash_when_zero_tokens() {
        let vm = present(&output(vec![row(2026, 4, 23, "m", 0, 0.0)]));
        assert_eq!(vm.rows[0].model_cells[0].cost, "—");
        assert_eq!(vm.rows[0].total_cost, "—");
    }

    #[test]
    fn missing_cell_renders_all_dashes() {
        let vm = present(&output(vec![
            row(2026, 4, 23, "a", 100, 0.0),
            row(2026, 4, 22, "b", 200, 0.0),
        ]));
        // columns: b(200), a(100)
        assert_eq!(vm.model_columns, vec!["b".to_string(), "a".to_string()]);
        // 04-23 row: b missing, a has input=100
        let r0 = &vm.rows[0];
        assert_eq!(r0.date_label, "04-23");
        assert_eq!(r0.model_cells[0].input, "—");
        assert_eq!(r0.model_cells[0].output, "—");
        assert_eq!(r0.model_cells[1].input, "100");
    }

    #[test]
    fn row_total_sums_all_tokens_across_models() {
        let vm = present(&output(vec![
            row_full((2026, 4, 23), "a", (100, 50, 20, 10)),
            row_full((2026, 4, 23), "b", (200, 100, 30, 20)),
        ]));
        // 100+50+20+10 + 200+100+30+20 = 530
        assert_eq!(vm.rows[0].total, "530");
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
        // grand total = 300+150+50+30 = 530
        assert_eq!(vm.grand_total, "530");
    }

    #[test]
    fn status_formatting() {
        let vm = present(&output(vec![row(2026, 4, 23, "m", 1, 0.1)]));
        assert_eq!(vm.status_msgs, "3");
        assert_eq!(vm.status_cost, "$4.20");
    }

    #[test]
    fn pricing_note_reflects_missing_count() {
        let out_none = GetDashboardOutput {
            filter_applied: Filter::default(),
            overview: Overview::default(),
            rows: vec![row(2026, 4, 23, "m", 1, 0.0)],
            missing_pricing_count: 0,
            unpriced_models: HashSet::new(),
            last_pricing_sync: None,
        };
        assert_eq!(present(&out_none).pricing_note, None);

        let out_two = GetDashboardOutput {
            filter_applied: Filter::default(),
            overview: Overview::default(),
            rows: vec![row(2026, 4, 23, "m", 1, 0.0)],
            missing_pricing_count: 2,
            unpriced_models: HashSet::new(),
            last_pricing_sync: None,
        };
        assert_eq!(
            present(&out_two).pricing_note,
            Some("· 2 models missing pricing".to_string())
        );
    }

    #[test]
    fn banner_shown_when_cost_zero_but_tokens_exist() {
        let out = GetDashboardOutput {
            filter_applied: Filter::default(),
            overview: Overview {
                cost: Cost::new(0.0).unwrap(),
                ..Overview::default()
            },
            rows: vec![row(2026, 4, 23, "m", 100, 0.0)],
            missing_pricing_count: 1,
            unpriced_models: {
                let mut s = HashSet::new();
                s.insert(ModelId::new("m").unwrap());
                s
            },
            last_pricing_sync: None,
        };
        assert!(present(&out).banner.is_some());
    }

    #[test]
    fn banner_absent_when_cost_nonzero() {
        let out = GetDashboardOutput {
            filter_applied: Filter::default(),
            overview: Overview {
                cost: Cost::new(1.23).unwrap(),
                ..Overview::default()
            },
            rows: vec![row(2026, 4, 23, "m", 100, 1.23)],
            missing_pricing_count: 0,
            unpriced_models: HashSet::new(),
            last_pricing_sync: None,
        };
        assert!(present(&out).banner.is_none());
    }
}
