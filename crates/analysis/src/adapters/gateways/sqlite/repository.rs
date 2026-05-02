use std::sync::{Arc, Mutex};

use chrono::NaiveDate;
use rusqlite::Connection;

use crate::adapters::gateways::sqlite::query_builder::{self, WhereClause};
use crate::application::dto::Filter;
use crate::application::ports::UsageRepository;
use shared::adapters::AdapterError;
use shared::application::errors::ApplicationError;
use shared::domain::entities::{DayModelRow, ModelUsage, Overview, ProjectUsage};
use shared::domain::value_objects::{
    Cost, DateRange, ModelId, ProjectPath, TokenBreakdown, TokenCount,
};

pub struct SqliteUsageRepository {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteUsageRepository {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }
}

fn parse_date(s: &str) -> Result<NaiveDate, AdapterError> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|e| AdapterError::DataMapping(format!("invalid date '{}': {}", s, e)))
}

fn parse_tokens(
    input: i64,
    output: i64,
    reasoning: i64,
    cache_read: i64,
    cache_write: i64,
) -> TokenBreakdown {
    TokenBreakdown {
        input: TokenCount::from_i64(input),
        output: TokenCount::from_i64(output),
        reasoning: TokenCount::from_i64(reasoning),
        cache_read: TokenCount::from_i64(cache_read),
        cache_write: TokenCount::from_i64(cache_write),
    }
}

fn parse_cost(cost: f64) -> Result<Cost, AdapterError> {
    Cost::new(cost).map_err(AdapterError::InvalidDomainValue)
}

impl UsageRepository for SqliteUsageRepository {
    fn overview(&self, filter: &Filter) -> Result<Overview, ApplicationError> {
        let WhereClause {
            sql: where_sql,
            params,
        } = query_builder::build(filter);
        let p_refs = query_builder::param_refs(&params);

        let sql = format!(
            "SELECT \
                COUNT(DISTINCT m.session_id), \
                COUNT(*), \
                COALESCE(SUM(json_extract(m.data, '$.tokens.input')), 0), \
                COALESCE(SUM(json_extract(m.data, '$.tokens.output')), 0), \
                COALESCE(SUM(json_extract(m.data, '$.tokens.reasoning')), 0), \
                COALESCE(SUM(json_extract(m.data, '$.tokens.cache.read')), 0), \
                COALESCE(SUM(json_extract(m.data, '$.tokens.cache.write')), 0), \
                COALESCE(SUM(json_extract(m.data, '$.cost')), 0.0) \
            FROM message m LEFT JOIN session s ON m.session_id = s.id {}",
            where_sql
        );

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&sql).map_err(AdapterError::from)?;
        let mut rows = stmt.query(p_refs.as_slice()).map_err(AdapterError::from)?;

        let row = rows.next().map_err(AdapterError::from)?;
        let range = filter.date_range.unwrap_or_else(DateRange::unbounded);
        match row {
            Some(r) => Ok(Overview {
                range,
                session_count: r.get::<_, i64>(0).unwrap_or(0).max(0) as u64,
                message_count: r.get::<_, i64>(1).unwrap_or(0).max(0) as u64,
                tokens: parse_tokens(
                    r.get(2).unwrap_or(0),
                    r.get(3).unwrap_or(0),
                    r.get(4).unwrap_or(0),
                    r.get(5).unwrap_or(0),
                    r.get(6).unwrap_or(0),
                ),
                cost: parse_cost(r.get::<_, f64>(7).unwrap_or(0.0))
                    .map_err(ApplicationError::from)?,
            }),
            None => Ok(Overview {
                range,
                ..Overview::default()
            }),
        }
    }

    fn daily_by_model(&self, filter: &Filter) -> Result<Vec<DayModelRow>, ApplicationError> {
        let WhereClause {
            sql: where_sql,
            params,
        } = query_builder::build(filter);
        let p_refs = query_builder::param_refs(&params);

        let sql = format!(
            "SELECT \
                date(datetime(json_extract(m.data, '$.time.created') / 1000, 'unixepoch')), \
                json_extract(m.data, '$.modelID'), \
                COALESCE(SUM(json_extract(m.data, '$.tokens.input')), 0), \
                COALESCE(SUM(json_extract(m.data, '$.tokens.output')), 0), \
                COALESCE(SUM(json_extract(m.data, '$.tokens.reasoning')), 0), \
                COALESCE(SUM(json_extract(m.data, '$.tokens.cache.write')), 0), \
                COALESCE(SUM(json_extract(m.data, '$.tokens.cache.read')), 0), \
                COALESCE(SUM(json_extract(m.data, '$.cost')), 0.0) \
            FROM message m LEFT JOIN session s ON m.session_id = s.id \
            {} \
            GROUP BY \
                date(datetime(json_extract(m.data, '$.time.created') / 1000, 'unixepoch')), \
                json_extract(m.data, '$.modelID') \
            ORDER BY \
                date(datetime(json_extract(m.data, '$.time.created') / 1000, 'unixepoch')) DESC, \
                COALESCE(SUM(json_extract(m.data, '$.cost')), 0.0) DESC",
            where_sql
        );

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&sql).map_err(AdapterError::from)?;
        let iter = stmt
            .query_map(p_refs.as_slice(), |r| {
                let date_str: String = r.get(0)?;
                let model_str: String = r.get(1)?;
                Ok((
                    date_str,
                    model_str,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, i64>(6)?,
                    r.get::<_, f64>(7)?,
                ))
            })
            .map_err(AdapterError::from)?;

        let mut out = Vec::new();
        for row in iter {
            let (date_str, model_str, input, output, reasoning, cache_write, cache_read, cost) =
                row.map_err(AdapterError::from)?;
            let date = parse_date(&date_str).map_err(ApplicationError::from)?;
            let model = ModelId::new(model_str)
                .map_err(|e| ApplicationError::from(AdapterError::from(e)))?;
            out.push(DayModelRow {
                date,
                model,
                tokens: parse_tokens(input, output, reasoning, cache_read, cache_write),
                cost: parse_cost(cost).map_err(ApplicationError::from)?,
            });
        }
        Ok(out)
    }

    fn by_model(&self, filter: &Filter) -> Result<Vec<ModelUsage>, ApplicationError> {
        let WhereClause {
            sql: where_sql,
            params,
        } = query_builder::build(filter);
        let p_refs = query_builder::param_refs(&params);

        let sql = format!(
            "SELECT \
                json_extract(m.data, '$.modelID'), \
                COUNT(*), \
                COALESCE(SUM(json_extract(m.data, '$.tokens.input')), 0), \
                COALESCE(SUM(json_extract(m.data, '$.tokens.output')), 0), \
                COALESCE(SUM(json_extract(m.data, '$.tokens.reasoning')), 0), \
                COALESCE(SUM(json_extract(m.data, '$.tokens.cache.read')), 0), \
                COALESCE(SUM(json_extract(m.data, '$.tokens.cache.write')), 0), \
                COALESCE(SUM(json_extract(m.data, '$.cost')), 0.0) \
            FROM message m LEFT JOIN session s ON m.session_id = s.id \
            {} \
            GROUP BY json_extract(m.data, '$.modelID') \
            ORDER BY COALESCE(SUM(json_extract(m.data, '$.cost')), 0.0) DESC",
            where_sql
        );

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&sql).map_err(AdapterError::from)?;
        let iter = stmt
            .query_map(p_refs.as_slice(), |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, i64>(6)?,
                    r.get::<_, f64>(7)?,
                ))
            })
            .map_err(AdapterError::from)?;

        let mut out = Vec::new();
        for row in iter {
            let (model_str, count, input, output, reasoning, cache_read, cache_write, cost) =
                row.map_err(AdapterError::from)?;
            let model = ModelId::new(model_str)
                .map_err(|e| ApplicationError::from(AdapterError::from(e)))?;
            out.push(ModelUsage {
                model,
                message_count: count.max(0) as u64,
                tokens: parse_tokens(input, output, reasoning, cache_read, cache_write),
                cost: parse_cost(cost).map_err(ApplicationError::from)?,
            });
        }
        Ok(out)
    }

    fn by_project(&self, filter: &Filter) -> Result<Vec<ProjectUsage>, ApplicationError> {
        let WhereClause {
            sql: where_sql,
            params,
        } = query_builder::build(filter);
        let p_refs = query_builder::param_refs(&params);

        let sql = format!(
            "SELECT \
                s.directory, \
                COUNT(*), \
                COALESCE(SUM(json_extract(m.data, '$.tokens.input')), 0), \
                COALESCE(SUM(json_extract(m.data, '$.tokens.output')), 0), \
                COALESCE(SUM(json_extract(m.data, '$.tokens.reasoning')), 0), \
                COALESCE(SUM(json_extract(m.data, '$.tokens.cache.read')), 0), \
                COALESCE(SUM(json_extract(m.data, '$.tokens.cache.write')), 0), \
                COALESCE(SUM(json_extract(m.data, '$.cost')), 0.0) \
            FROM message m JOIN session s ON m.session_id = s.id \
            {} \
            GROUP BY s.directory \
            ORDER BY COALESCE(SUM(json_extract(m.data, '$.cost')), 0.0) DESC",
            where_sql
        );

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&sql).map_err(AdapterError::from)?;
        let iter = stmt
            .query_map(p_refs.as_slice(), |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, i64>(6)?,
                    r.get::<_, f64>(7)?,
                ))
            })
            .map_err(AdapterError::from)?;

        let mut out = Vec::new();
        for row in iter {
            let (project_str, count, input, output, reasoning, cache_read, cache_write, cost) =
                row.map_err(AdapterError::from)?;
            let project = ProjectPath::new(project_str)
                .map_err(|e| ApplicationError::from(AdapterError::from(e)))?;
            out.push(ProjectUsage {
                project,
                message_count: count.max(0) as u64,
                tokens: parse_tokens(input, output, reasoning, cache_read, cache_write),
                cost: parse_cost(cost).map_err(ApplicationError::from)?,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed() -> Arc<Mutex<Connection>> {
        let conn = Connection::open_in_memory().unwrap();
        let sql = include_str!("../../../../tests/fixtures/seed.sql");
        conn.execute_batch(sql).unwrap();
        Arc::new(Mutex::new(conn))
    }

    #[test]
    fn overview_totals_match_fixture() {
        let repo = SqliteUsageRepository::new(seed());
        let o = repo.overview(&Filter::default()).unwrap();
        assert_eq!(o.message_count, 3, "user-role row must be filtered out");
        assert_eq!(o.session_count, 2);
        assert_eq!(o.tokens.input.value(), 3500);
        assert_eq!(o.tokens.output.value(), 1800);
        assert_eq!(o.tokens.cache_read.value(), 600);
        assert_eq!(o.tokens.cache_write.value(), 300);
        assert!((o.cost.value() - 2.45).abs() < 1e-9);
    }

    #[test]
    fn daily_by_model_returns_rows_sorted_by_date_desc() {
        let repo = SqliteUsageRepository::new(seed());
        let rows = repo.daily_by_model(&Filter::default()).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows[0].date >= rows[1].date);
    }

    #[test]
    fn by_model_sorted_by_cost_desc() {
        let repo = SqliteUsageRepository::new(seed());
        let rows = repo.by_model(&Filter::default()).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows[0].cost.value() >= rows[1].cost.value());
    }

    #[test]
    fn by_project_sorted_by_cost_desc() {
        let repo = SqliteUsageRepository::new(seed());
        let rows = repo.by_project(&Filter::default()).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows[0].cost.value() >= rows[1].cost.value());
        assert_eq!(rows[0].project.as_str(), "/work/alpha");
    }
}
