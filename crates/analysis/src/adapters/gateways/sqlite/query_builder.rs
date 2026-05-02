use rusqlite::types::ToSql;

use crate::application::dto::Filter;

/// Always-on assistant-role predicate — see `.claude/rules/data-shape.md`.
const ROLE_PREDICATE: &str = "json_extract(m.data, '$.role') = 'assistant'";

pub struct WhereClause {
    pub sql: String,
    pub params: Vec<Box<dyn ToSql>>,
}

pub fn build(filter: &Filter) -> WhereClause {
    let mut conditions: Vec<String> = vec![ROLE_PREDICATE.to_string()];
    let mut params: Vec<Box<dyn ToSql>> = Vec::new();

    if let Some(range) = &filter.date_range {
        if let Some(from) = range.from {
            let ts = from
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc()
                .timestamp_millis();
            conditions.push(format!(
                "json_extract(m.data, '$.time.created') >= ?{}",
                params.len() + 1
            ));
            params.push(Box::new(ts));
        }
        if let Some(to) = range.to {
            let ts = to
                .and_hms_opt(23, 59, 59)
                .unwrap()
                .and_utc()
                .timestamp_millis();
            conditions.push(format!(
                "json_extract(m.data, '$.time.created') <= ?{}",
                params.len() + 1
            ));
            params.push(Box::new(ts));
        }
    }

    if let Some(model) = &filter.model {
        conditions.push(format!(
            "json_extract(m.data, '$.modelID') LIKE ?{}",
            params.len() + 1
        ));
        params.push(Box::new(format!("%{}%", model.as_str())));
    }

    if let Some(provider) = &filter.provider {
        conditions.push(format!(
            "json_extract(m.data, '$.providerID') LIKE ?{}",
            params.len() + 1
        ));
        params.push(Box::new(format!("%{}%", provider)));
    }

    if let Some(session_id) = &filter.session_id {
        conditions.push(format!("m.session_id = ?{}", params.len() + 1));
        params.push(Box::new(session_id.clone()));
    }

    if let Some(project) = &filter.project {
        conditions.push(format!("s.directory LIKE ?{}", params.len() + 1));
        params.push(Box::new(format!("%{}%", project.as_str())));
    }

    WhereClause {
        sql: format!("WHERE {}", conditions.join(" AND ")),
        params,
    }
}

pub fn param_refs(params: &[Box<dyn ToSql>]) -> Vec<&dyn ToSql> {
    params.iter().map(|p| p.as_ref()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use shared::domain::value_objects::{DateRange, ModelId, ProjectPath};
    use chrono::NaiveDate;

    #[test]
    fn empty_filter_yields_role_only() {
        let w = build(&Filter::default());
        assert_eq!(w.sql, format!("WHERE {}", ROLE_PREDICATE));
        assert!(w.params.is_empty());
    }

    #[test]
    fn date_range_adds_two_params() {
        let range = DateRange::new(
            Some(NaiveDate::from_ymd_opt(2026, 4, 1).unwrap()),
            Some(NaiveDate::from_ymd_opt(2026, 4, 23).unwrap()),
        )
        .unwrap();
        let w = build(&Filter {
            date_range: Some(range),
            ..Filter::default()
        });
        assert_eq!(w.params.len(), 2);
        assert!(w.sql.contains("$.time.created") && w.sql.contains(">=") && w.sql.contains("<="));
    }

    #[test]
    fn model_filter_adds_one_param_and_like() {
        let w = build(&Filter {
            model: Some(ModelId::new("opus").unwrap()),
            ..Filter::default()
        });
        assert_eq!(w.params.len(), 1);
        assert!(w.sql.contains("$.modelID"));
        assert!(w.sql.contains("LIKE"));
    }

    #[test]
    fn project_filter_uses_session_directory() {
        let w = build(&Filter {
            project: Some(ProjectPath::new("/work").unwrap()),
            ..Filter::default()
        });
        assert!(w.sql.contains("s.directory LIKE"));
    }

    #[test]
    fn session_id_uses_equality() {
        let w = build(&Filter {
            session_id: Some("abc".into()),
            ..Filter::default()
        });
        assert!(w.sql.contains("m.session_id = ?1"));
    }
}
