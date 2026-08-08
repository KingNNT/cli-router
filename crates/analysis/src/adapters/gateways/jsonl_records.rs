//! Filtering and aggregation shared by the JSONL-backed usage repositories.
//!
//! `ClaudeCodeUsageRepository` and `CodexUsageRepository` both hold their
//! records in memory and answer the same two `UsageRepository` questions, so
//! the grouping lives here instead of once per adapter. The SQLite repository
//! does this work in SQL and does not use this module.

use std::collections::{HashMap, HashSet};

use chrono::NaiveDate;

use crate::application::dto::Filter;
use shared::domain::entities::{DayModelRow, Overview, UsageRecord};
use shared::domain::value_objects::{Cost, DateRange, ModelId, TokenBreakdown};

pub(crate) fn in_range(date: NaiveDate, range: Option<&DateRange>) -> bool {
    let Some(r) = range else { return true };
    if let Some(from) = r.from
        && date < from
    {
        return false;
    }
    if let Some(to) = r.to
        && date > to
    {
        return false;
    }
    true
}

/// The `provider` field of `Filter` has no counterpart in JSONL records, so it
/// is a pass-through here.
pub(crate) fn matches_filter(r: &UsageRecord, filter: &Filter) -> bool {
    if !in_range(r.date, filter.date_range.as_ref()) {
        return false;
    }
    if let Some(p) = &filter.project
        && r.project.as_str() != p.as_str()
    {
        return false;
    }
    if let Some(m) = &filter.model
        && r.model.as_str() != m.as_str()
    {
        return false;
    }
    if let Some(s) = &filter.session_id
        && r.session_id != *s
    {
        return false;
    }
    true
}

pub(crate) fn overview_from(records: &[UsageRecord], filter: &Filter) -> Overview {
    let mut tokens = TokenBreakdown::default();
    let mut cost = Cost::zero();
    let mut messages: u64 = 0;
    let mut sessions: HashSet<&str> = HashSet::new();
    for r in records.iter().filter(|r| matches_filter(r, filter)) {
        tokens += r.tokens;
        cost += r.cost;
        messages += 1;
        if !r.session_id.is_empty() {
            sessions.insert(r.session_id.as_str());
        }
    }
    let range = filter.date_range.unwrap_or_else(DateRange::unbounded);
    Overview {
        range,
        session_count: sessions.len() as u64,
        message_count: messages,
        tokens,
        cost,
    }
}

pub(crate) fn daily_by_model_from(records: &[UsageRecord], filter: &Filter) -> Vec<DayModelRow> {
    type Group = (TokenBreakdown, Cost);
    let mut map: HashMap<(NaiveDate, String), (ModelId, Group)> = HashMap::new();
    for r in records.iter().filter(|r| matches_filter(r, filter)) {
        let key = (r.date, r.model.as_str().to_string());
        let entry = map
            .entry(key)
            .or_insert_with(|| (r.model.clone(), (TokenBreakdown::default(), Cost::zero())));
        entry.1.0 += r.tokens;
        entry.1.1 += r.cost;
    }
    let mut out: Vec<DayModelRow> = map
        .into_iter()
        .map(|((date, _), (model, (tokens, cost)))| DayModelRow {
            date,
            model,
            tokens,
            cost,
        })
        .collect();
    out.sort_by(|a, b| {
        b.date
            .cmp(&a.date)
            .then_with(|| a.model.as_str().cmp(b.model.as_str()))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use shared::domain::value_objects::{ProjectPath, TokenCount};

    fn record(
        date: (i32, u32, u32),
        model: &str,
        project: &str,
        session: &str,
        input: u64,
    ) -> UsageRecord {
        UsageRecord {
            date: NaiveDate::from_ymd_opt(date.0, date.1, date.2).unwrap(),
            model: ModelId::new(model).unwrap(),
            project: ProjectPath::new(project).unwrap(),
            tokens: TokenBreakdown {
                input: TokenCount::new(input),
                ..Default::default()
            },
            cost: Cost::zero(),
            session_id: session.to_string(),
        }
    }

    fn range(from: (i32, u32, u32), to: (i32, u32, u32)) -> DateRange {
        DateRange::new(
            Some(NaiveDate::from_ymd_opt(from.0, from.1, from.2).unwrap()),
            Some(NaiveDate::from_ymd_opt(to.0, to.1, to.2).unwrap()),
        )
        .unwrap()
    }

    #[test]
    fn unbounded_range_admits_everything() {
        let d = NaiveDate::from_ymd_opt(2026, 8, 8).unwrap();
        assert!(in_range(d, None));
    }

    #[test]
    fn range_excludes_dates_outside_it() {
        let r = range((2026, 8, 1), (2026, 8, 31));
        assert!(in_range(
            NaiveDate::from_ymd_opt(2026, 8, 8).unwrap(),
            Some(&r)
        ));
        assert!(!in_range(
            NaiveDate::from_ymd_opt(2026, 7, 31).unwrap(),
            Some(&r)
        ));
        assert!(!in_range(
            NaiveDate::from_ymd_opt(2026, 9, 1).unwrap(),
            Some(&r)
        ));
    }

    #[test]
    fn provider_filter_is_a_pass_through() {
        let r = record((2026, 8, 8), "m", "/p", "s", 10);
        let filter = Filter {
            provider: Some("anthropic".to_string()),
            ..Filter::default()
        };
        assert!(matches_filter(&r, &filter));
    }

    #[test]
    fn model_project_and_session_filters_narrow_the_set() {
        let r = record((2026, 8, 8), "m1", "/p1", "s1", 10);
        assert!(!matches_filter(
            &r,
            &Filter {
                model: Some(ModelId::new("m2").unwrap()),
                ..Filter::default()
            }
        ));
        assert!(!matches_filter(
            &r,
            &Filter {
                project: Some(ProjectPath::new("/p2").unwrap()),
                ..Filter::default()
            }
        ));
        assert!(!matches_filter(
            &r,
            &Filter {
                session_id: Some("s2".to_string()),
                ..Filter::default()
            }
        ));
    }

    #[test]
    fn overview_counts_messages_and_distinct_sessions() {
        let records = vec![
            record((2026, 8, 8), "m", "/p", "s1", 10),
            record((2026, 8, 8), "m", "/p", "s1", 20),
            record((2026, 8, 7), "m", "/p", "s2", 5),
        ];
        let ov = overview_from(&records, &Filter::default());
        assert_eq!(ov.message_count, 3);
        assert_eq!(ov.session_count, 2);
        assert_eq!(ov.tokens.input.value(), 35);
    }

    #[test]
    fn overview_respects_the_date_range() {
        let records = vec![
            record((2026, 8, 8), "m", "/p", "s1", 10),
            record((2026, 7, 1), "m", "/p", "s2", 999),
        ];
        let filter = Filter {
            date_range: Some(range((2026, 8, 1), (2026, 8, 31))),
            ..Filter::default()
        };
        let ov = overview_from(&records, &filter);
        assert_eq!(ov.tokens.input.value(), 10);
    }

    #[test]
    fn daily_by_model_groups_and_sorts_newest_first() {
        let records = vec![
            record((2026, 8, 7), "m1", "/p", "s", 5),
            record((2026, 8, 8), "m2", "/p", "s", 20),
            record((2026, 8, 8), "m1", "/p", "s", 10),
            record((2026, 8, 8), "m1", "/p", "s", 30),
        ];
        let rows = daily_by_model_from(&records, &Filter::default());
        assert_eq!(rows.len(), 3);
        // Newest date first, models alphabetical within a date.
        assert_eq!(rows[0].date.to_string(), "2026-08-08");
        assert_eq!(rows[0].model.as_str(), "m1");
        assert_eq!(rows[0].tokens.input.value(), 40);
        assert_eq!(rows[1].model.as_str(), "m2");
        assert_eq!(rows[2].date.to_string(), "2026-08-07");
    }
}
