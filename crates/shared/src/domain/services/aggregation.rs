use std::collections::BTreeMap;

use crate::domain::entities::{DailyUsage, DayModelRow, ModelUsage, UsageRecord};
use crate::domain::services::aliases::canonicalize;
use crate::domain::value_objects::{Cost, ModelId, TokenBreakdown};

pub fn sum_costs(records: &[UsageRecord]) -> Cost {
    records.iter().map(|r| r.cost).sum()
}

pub fn sum_tokens(records: &[UsageRecord]) -> TokenBreakdown {
    records.iter().map(|r| r.tokens).sum()
}

pub fn group_by_day(records: &[UsageRecord]) -> Vec<DailyUsage> {
    let mut map: BTreeMap<chrono::NaiveDate, (u64, TokenBreakdown, Cost)> = BTreeMap::new();
    for r in records {
        let entry = map
            .entry(r.date)
            .or_insert((0, TokenBreakdown::default(), Cost::zero()));
        entry.0 += 1;
        entry.1 += r.tokens;
        entry.2 += r.cost;
    }
    // DESC by date — matches the SQL ORDER BY in the gateway.
    map.into_iter()
        .rev()
        .map(|(date, (count, tokens, cost))| DailyUsage {
            date,
            message_count: count,
            tokens,
            cost,
        })
        .collect()
}

pub fn group_by_model(records: &[UsageRecord]) -> Vec<ModelUsage> {
    let mut map: BTreeMap<ModelId, (u64, TokenBreakdown, Cost)> = BTreeMap::new();
    for r in records {
        let entry =
            map.entry(r.model.clone())
                .or_insert((0, TokenBreakdown::default(), Cost::zero()));
        entry.0 += 1;
        entry.1 += r.tokens;
        entry.2 += r.cost;
    }
    let mut out: Vec<ModelUsage> = map
        .into_iter()
        .map(|(model, (count, tokens, cost))| ModelUsage {
            model,
            message_count: count,
            tokens,
            cost,
        })
        .collect();
    // DESC by cost — matches SQL ORDER BY.
    out.sort_by(|a, b| {
        b.cost
            .partial_cmp(&a.cost)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

/// Collapse rows by (date, canonical_or_raw_model). Each group sums its
/// tokens and cost. Aliased groups take `ModelId(canonical)`; unaliased
/// groups keep their original `ModelId`. Return order is not guaranteed —
/// callers sort.
pub fn aggregate_day_model_rows_by_alias(rows: Vec<DayModelRow>) -> Vec<DayModelRow> {
    type Group = (ModelId, TokenBreakdown, Cost);
    let mut groups: BTreeMap<(chrono::NaiveDate, String), Group> = BTreeMap::new();

    for row in rows {
        let key_str: String = match canonicalize(row.model.as_str()) {
            Some(c) => c.to_string(),
            None => row.model.as_str().to_string(),
        };
        let display_model: ModelId = match canonicalize(row.model.as_str()) {
            Some(c) => ModelId::new(c).expect("canonical must be non-empty"),
            None => row.model.clone(),
        };
        let entry = groups
            .entry((row.date, key_str))
            .or_insert_with(|| (display_model, TokenBreakdown::default(), Cost::zero()));
        entry.1 += row.tokens;
        entry.2 += row.cost;
    }

    groups
        .into_iter()
        .map(|((date, _key), (model, tokens, cost))| DayModelRow {
            date,
            model,
            tokens,
            cost,
        })
        .collect()
}

/// Collapse model-usage rows by canonical-or-raw name. Sums message_count,
/// tokens, and cost. Return order is not guaranteed.
pub fn aggregate_model_usage_by_alias(rows: Vec<ModelUsage>) -> Vec<ModelUsage> {
    type Group = (ModelId, u64, TokenBreakdown, Cost);
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();

    for row in rows {
        let key_str: String = match canonicalize(row.model.as_str()) {
            Some(c) => c.to_string(),
            None => row.model.as_str().to_string(),
        };
        let display_model: ModelId = match canonicalize(row.model.as_str()) {
            Some(c) => ModelId::new(c).expect("canonical must be non-empty"),
            None => row.model.clone(),
        };
        let entry = groups
            .entry(key_str)
            .or_insert_with(|| (display_model, 0, TokenBreakdown::default(), Cost::zero()));
        entry.1 += row.message_count;
        entry.2 += row.tokens;
        entry.3 += row.cost;
    }

    groups
        .into_iter()
        .map(|(_key, (model, message_count, tokens, cost))| ModelUsage {
            model,
            message_count,
            tokens,
            cost,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::value_objects::{ProjectPath, TokenCount};
    use chrono::NaiveDate;

    fn record(date: (i32, u32, u32), model: &str, cost: f64, input: u64) -> UsageRecord {
        UsageRecord {
            date: NaiveDate::from_ymd_opt(date.0, date.1, date.2).unwrap(),
            model: ModelId::new(model).unwrap(),
            project: ProjectPath::new("/tmp").unwrap(),
            tokens: TokenBreakdown {
                input: TokenCount::new(input),
                ..Default::default()
            },
            cost: Cost::new(cost).unwrap(),
            session_id: "s".into(),
        }
    }

    #[test]
    fn sum_costs_sums_all() {
        let recs = vec![
            record((2026, 4, 23), "m", 1.0, 10),
            record((2026, 4, 22), "m", 2.0, 20),
        ];
        assert_eq!(sum_costs(&recs).value(), 3.0);
    }

    #[test]
    fn sum_tokens_sums_breakdown() {
        let recs = vec![
            record((2026, 4, 23), "m", 0.0, 5),
            record((2026, 4, 22), "m", 0.0, 7),
        ];
        assert_eq!(sum_tokens(&recs).input.value(), 12);
    }

    #[test]
    fn group_by_day_buckets_and_sorts_desc() {
        let recs = vec![
            record((2026, 4, 23), "m", 1.0, 10),
            record((2026, 4, 22), "m", 2.0, 20),
            record((2026, 4, 23), "m", 3.0, 30),
        ];
        let daily = group_by_day(&recs);
        assert_eq!(daily.len(), 2);
        assert_eq!(daily[0].date, NaiveDate::from_ymd_opt(2026, 4, 23).unwrap());
        assert_eq!(daily[0].message_count, 2);
        assert_eq!(daily[0].cost.value(), 4.0);
        assert_eq!(daily[0].tokens.input.value(), 40);
        assert_eq!(daily[1].date, NaiveDate::from_ymd_opt(2026, 4, 22).unwrap());
    }

    #[test]
    fn group_by_model_buckets_and_sorts_by_cost_desc() {
        let recs = vec![
            record((2026, 4, 23), "cheap", 0.5, 10),
            record((2026, 4, 23), "pricey", 9.0, 10),
        ];
        let by_model = group_by_model(&recs);
        assert_eq!(by_model.len(), 2);
        assert_eq!(by_model[0].model.as_str(), "pricey");
        assert_eq!(by_model[1].model.as_str(), "cheap");
    }
}

#[cfg(test)]
mod alias_tests {
    use super::*;
    use crate::domain::entities::{DayModelRow, ModelUsage};
    use crate::domain::value_objects::{Cost, ModelId, TokenBreakdown, TokenCount};
    use chrono::NaiveDate;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn day_row(date: NaiveDate, model: &str, input: u64, cost: f64) -> DayModelRow {
        DayModelRow {
            date,
            model: ModelId::new(model).unwrap(),
            tokens: TokenBreakdown {
                input: TokenCount::new(input),
                ..Default::default()
            },
            cost: Cost::new(cost).unwrap(),
        }
    }

    fn model_row(model: &str, messages: u64, input: u64, cost: f64) -> ModelUsage {
        ModelUsage {
            model: ModelId::new(model).unwrap(),
            message_count: messages,
            tokens: TokenBreakdown {
                input: TokenCount::new(input),
                ..Default::default()
            },
            cost: Cost::new(cost).unwrap(),
        }
    }

    #[test]
    fn day_rows_single_source_passes_through_unchanged() {
        let d = day(2026, 4, 23);
        let out = aggregate_day_model_rows_by_alias(vec![day_row(d, "unknown-model", 100, 1.0)]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].model.as_str(), "unknown-model");
        assert_eq!(out[0].tokens.input.value(), 100);
        assert!((out[0].cost.value() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn day_rows_multi_source_with_same_alias_collapse_to_one_row() {
        let d = day(2026, 4, 23);
        let out = aggregate_day_model_rows_by_alias(vec![
            day_row(d, "anthropic.claude-opus-4-6-v1", 1000, 0.5),
            day_row(d, "us.anthropic.claude-opus-4-6-v1", 2000, 1.0),
        ]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].model.as_str(), "opus4.6");
        assert_eq!(out[0].tokens.input.value(), 3000);
        assert!((out[0].cost.value() - 1.5).abs() < 1e-9);
        assert_eq!(out[0].date, d);
    }

    #[test]
    fn day_rows_mixed_aliased_and_not_keeps_both() {
        let d = day(2026, 4, 23);
        let out = aggregate_day_model_rows_by_alias(vec![
            day_row(d, "anthropic.claude-opus-4-6-v1", 1000, 0.5),
            day_row(d, "unknown/other", 500, 0.25),
        ]);
        assert_eq!(out.len(), 2);
        let opus = out.iter().find(|r| r.model.as_str() == "opus4.6").unwrap();
        let unk = out
            .iter()
            .find(|r| r.model.as_str() == "unknown/other")
            .unwrap();
        assert_eq!(opus.tokens.input.value(), 1000);
        assert_eq!(unk.tokens.input.value(), 500);
    }

    #[test]
    fn day_rows_preserves_date_dimension_for_different_days() {
        let d1 = day(2026, 4, 22);
        let d2 = day(2026, 4, 23);
        let out = aggregate_day_model_rows_by_alias(vec![
            day_row(d1, "anthropic.claude-opus-4-6-v1", 1000, 0.5),
            day_row(d2, "anthropic.claude-opus-4-6-v1", 2000, 1.0),
        ]);
        assert_eq!(out.len(), 2);
        let d1_row = out.iter().find(|r| r.date == d1).unwrap();
        let d2_row = out.iter().find(|r| r.date == d2).unwrap();
        assert_eq!(d1_row.tokens.input.value(), 1000);
        assert_eq!(d2_row.tokens.input.value(), 2000);
    }

    #[test]
    fn model_usage_single_source_passes_through_unchanged() {
        let out = aggregate_model_usage_by_alias(vec![model_row("solo-model", 3, 100, 1.0)]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].model.as_str(), "solo-model");
        assert_eq!(out[0].message_count, 3);
    }

    #[test]
    fn model_usage_multi_source_collapses_and_sums_message_count() {
        let out = aggregate_model_usage_by_alias(vec![
            model_row("anthropic.claude-opus-4-6-v1", 10, 1000, 0.5),
            model_row("us.anthropic.claude-opus-4-6-v1", 7, 2000, 1.0),
        ]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].model.as_str(), "opus4.6");
        assert_eq!(out[0].message_count, 17);
        assert_eq!(out[0].tokens.input.value(), 3000);
        assert!((out[0].cost.value() - 1.5).abs() < 1e-9);
    }

    #[test]
    fn model_usage_mixed_aliased_and_not_keeps_both() {
        let out = aggregate_model_usage_by_alias(vec![
            model_row("anthropic.claude-opus-4-6-v1", 5, 1000, 0.5),
            model_row("zai/glm-5.1", 2, 500, 0.0),
        ]);
        assert_eq!(out.len(), 2);
    }
}
