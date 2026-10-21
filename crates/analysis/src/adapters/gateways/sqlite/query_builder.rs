use rusqlite::Connection;
use rusqlite::types::ToSql;

use crate::application::dto::Filter;

/// OpenCode database layout. Token, cost and `$.time.created` paths inside
/// `data` are identical across versions; tables, role and model keys differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenCodeSchema {
    /// `message` + `session` tables; role, model and provider are flat keys
    /// in `message.data` (`$.role`, `$.modelID`, `$.providerID`).
    V1,
    /// `session_message` + `session_v2` tables; role is the `type` column,
    /// model and provider are nested (`$.model.id`, `$.model.providerID`).
    V2,
}

impl OpenCodeSchema {
    /// V2 wins when its table exists; anything else is treated as V1.
    pub fn detect(conn: &Connection) -> rusqlite::Result<Self> {
        let has_v2: bool = conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM sqlite_master \
             WHERE type = 'table' AND name = 'session_message')",
            [],
            |r| r.get(0),
        )?;
        Ok(if has_v2 { Self::V2 } else { Self::V1 })
    }

    /// `FROM` clause aliasing messages as `m` and sessions as `s`.
    pub fn from_clause(self) -> &'static str {
        match self {
            Self::V1 => "FROM message m LEFT JOIN session s ON m.session_id = s.id",
            Self::V2 => "FROM session_message m LEFT JOIN session_v2 s ON m.session_id = s.id",
        }
    }

    /// Always-on assistant-role predicate — see `.claude/rules/data-shape.md`.
    pub fn role_predicate(self) -> &'static str {
        match self {
            Self::V1 => "json_extract(m.data, '$.role') = 'assistant'",
            Self::V2 => "m.type = 'assistant'",
        }
    }

    pub fn model_expr(self) -> &'static str {
        match self {
            Self::V1 => "json_extract(m.data, '$.modelID')",
            Self::V2 => "json_extract(m.data, '$.model.id')",
        }
    }

    pub fn provider_expr(self) -> &'static str {
        match self {
            Self::V1 => "json_extract(m.data, '$.providerID')",
            Self::V2 => "json_extract(m.data, '$.model.providerID')",
        }
    }
}

pub struct WhereClause {
    pub sql: String,
    pub params: Vec<Box<dyn ToSql>>,
}

pub fn build(filter: &Filter, schema: OpenCodeSchema) -> WhereClause {
    let mut conditions: Vec<String> = vec![schema.role_predicate().to_string()];
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
            "{} LIKE ?{}",
            schema.model_expr(),
            params.len() + 1
        ));
        params.push(Box::new(format!("%{}%", model.as_str())));
    }

    if let Some(provider) = &filter.provider {
        conditions.push(format!(
            "{} LIKE ?{}",
            schema.provider_expr(),
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
    use chrono::NaiveDate;
    use shared::domain::value_objects::{DateRange, ModelId, ProjectPath};

    #[test]
    fn empty_filter_yields_role_only() {
        let w = build(&Filter::default(), OpenCodeSchema::V1);
        assert_eq!(
            w.sql,
            format!("WHERE {}", OpenCodeSchema::V1.role_predicate())
        );
        assert!(w.params.is_empty());
    }

    #[test]
    fn date_range_adds_two_params() {
        let range = DateRange::new(
            Some(NaiveDate::from_ymd_opt(2026, 4, 1).unwrap()),
            Some(NaiveDate::from_ymd_opt(2026, 4, 23).unwrap()),
        )
        .unwrap();
        let w = build(
            &Filter {
                date_range: Some(range),
                ..Filter::default()
            },
            OpenCodeSchema::V1,
        );
        assert_eq!(w.params.len(), 2);
        assert!(w.sql.contains("$.time.created") && w.sql.contains(">=") && w.sql.contains("<="));
    }

    #[test]
    fn model_filter_adds_one_param_and_like() {
        let w = build(
            &Filter {
                model: Some(ModelId::new("opus").unwrap()),
                ..Filter::default()
            },
            OpenCodeSchema::V1,
        );
        assert_eq!(w.params.len(), 1);
        assert!(w.sql.contains("$.modelID"));
        assert!(w.sql.contains("LIKE"));
    }

    #[test]
    fn project_filter_uses_session_directory() {
        let w = build(
            &Filter {
                project: Some(ProjectPath::new("/work").unwrap()),
                ..Filter::default()
            },
            OpenCodeSchema::V1,
        );
        assert!(w.sql.contains("s.directory LIKE"));
    }

    #[test]
    fn session_id_uses_equality() {
        let w = build(
            &Filter {
                session_id: Some("abc".into()),
                ..Filter::default()
            },
            OpenCodeSchema::V1,
        );
        assert!(w.sql.contains("m.session_id = ?1"));
    }

    #[test]
    fn v2_uses_type_column_and_nested_model_keys() {
        let w = build(
            &Filter {
                model: Some(ModelId::new("opus").unwrap()),
                provider: Some("anthropic".into()),
                ..Filter::default()
            },
            OpenCodeSchema::V2,
        );
        assert!(w.sql.starts_with("WHERE m.type = 'assistant'"));
        assert!(w.sql.contains("$.model.id") && w.sql.contains("$.model.providerID"));
        assert!(!w.sql.contains("$.role") && !w.sql.contains("$.modelID"));
    }

    #[test]
    fn detect_distinguishes_v1_and_v2() {
        let v1 = Connection::open_in_memory().unwrap();
        v1.execute_batch("CREATE TABLE message (id TEXT);").unwrap();
        assert_eq!(OpenCodeSchema::detect(&v1).unwrap(), OpenCodeSchema::V1);

        let v2 = Connection::open_in_memory().unwrap();
        v2.execute_batch("CREATE TABLE session_message (id TEXT);")
            .unwrap();
        assert_eq!(OpenCodeSchema::detect(&v2).unwrap(), OpenCodeSchema::V2);
    }
}
