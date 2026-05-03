//! SQLite-backed RequestLogPort adapter.

use crate::application::errors::ProxyError;
use crate::application::ports::RequestLogPort;
use crate::application::ports::QuotaSeedRow;
use crate::domain::{DailyTotal, ModelTotal, RequestStart, RequestUsage, UsageSummary};
use rusqlite::{Connection, params};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct SqliteRequestLogRepository {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteRequestLogRepository {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }
}

impl RequestLogPort for SqliteRequestLogRepository {
    fn insert_started(&self, start: &RequestStart) -> Result<(), ProxyError> {
        let conn = self.conn.lock().expect("repo mutex poisoned");
        conn.execute(
            "INSERT INTO requests \
             (id, user_id, provider, model, status, started_at) \
             VALUES (?1, ?2, ?3, ?4, 'started', ?5)",
            params![
                start.id,
                start.user_id,
                start.provider,
                start.model,
                start.started_at,
            ],
        )?;
        Ok(())
    }

    fn complete(&self, id: &str, finished_at: i64, usage: &RequestUsage) -> Result<(), ProxyError> {
        let conn = self.conn.lock().expect("repo mutex poisoned");
        conn.execute(
            "UPDATE requests SET \
                status = 'completed', \
                finished_at = ?2, \
                input_tokens = ?3, \
                output_tokens = ?4, \
                cache_read_tokens = ?5, \
                cache_creation_tokens = ?6, \
                cost_usd = ?7 \
             WHERE id = ?1",
            params![
                id,
                finished_at,
                usage.input_tokens.map(|v| v as i64),
                usage.output_tokens.map(|v| v as i64),
                usage.cache_read_tokens.map(|v| v as i64),
                usage.cache_creation_tokens.map(|v| v as i64),
                usage.cost_usd,
            ],
        )?;
        Ok(())
    }

    fn fail(
        &self,
        id: &str,
        finished_at: i64,
        error_message: &str,
        usage: &RequestUsage,
    ) -> Result<(), ProxyError> {
        let conn = self.conn.lock().expect("repo mutex poisoned");
        conn.execute(
            "UPDATE requests SET \
                status = 'errored', \
                finished_at = ?2, \
                error_message = ?3, \
                input_tokens = ?4, \
                output_tokens = ?5, \
                cache_read_tokens = ?6, \
                cache_creation_tokens = ?7, \
                cost_usd = ?8 \
             WHERE id = ?1",
            params![
                id,
                finished_at,
                error_message,
                usage.input_tokens.map(|v| v as i64),
                usage.output_tokens.map(|v| v as i64),
                usage.cache_read_tokens.map(|v| v as i64),
                usage.cache_creation_tokens.map(|v| v as i64),
                usage.cost_usd,
            ],
        )?;
        Ok(())
    }
}

impl crate::application::ports::RequestLogReadPort for SqliteRequestLogRepository {
    fn total_count(&self) -> Result<u64, ProxyError> {
        let c = self.conn.lock().expect("repo mutex poisoned");
        let n: i64 = c.query_row("SELECT COUNT(*) FROM requests", [], |r| r.get(0))?;
        Ok(n.max(0) as u64)
    }

    fn count_by_provider(&self) -> Result<std::collections::BTreeMap<String, u64>, ProxyError> {
        count_grouped(&self.conn, "provider")
    }

    fn count_by_status(&self) -> Result<std::collections::BTreeMap<String, u64>, ProxyError> {
        count_grouped(&self.conn, "status")
    }

    fn recent(&self, limit: u32) -> Result<Vec<crate::domain::RequestRow>, ProxyError> {
        let c = self.conn.lock().expect("repo mutex poisoned");
        let mut stmt = c.prepare(
            "SELECT id, started_at, finished_at, provider, model, status,
                    input_tokens, output_tokens, cache_read_tokens,
                    cache_creation_tokens, cost_usd, error_message
             FROM requests
             ORDER BY started_at DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |r| {
            Ok(crate::domain::RequestRow {
                id: r.get(0)?,
                started_at_ms: r.get(1)?,
                finished_at_ms: r.get(2)?,
                provider: r.get(3)?,
                model: r.get(4)?,
                status: r.get(5)?,
                input_tokens: r.get(6)?,
                output_tokens: r.get(7)?,
                cache_read_tokens: r.get(8)?,
                cache_creation_tokens: r.get(9)?,
                cost_usd: r.get(10)?,
                error_message: r.get(11)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    fn summarize(&self, from_ms: i64, to_ms: i64) -> Result<UsageSummary, ProxyError> {
        let conn = self.conn.lock().expect("repo mutex poisoned");

        let daily = query_daily(&conn, from_ms, to_ms)?;
        let models = query_models(&conn, from_ms, to_ms)?;

        Ok(UsageSummary {
            from_ms,
            to_ms,
            daily,
            models,
        })
    }

    fn quota_seed(&self, cutoff_ms: i64) -> Result<Vec<QuotaSeedRow>, ProxyError> {
        let conn = self.conn.lock().expect("repo mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT provider, started_at, input_tokens, output_tokens \
             FROM requests \
             WHERE started_at > ?1 AND status = 'completed'",
        )?;
        let rows = stmt.query_map(params![cutoff_ms], |row| {
            Ok(QuotaSeedRow {
                provider: row.get::<_, String>(0)?,
                started_at_ms: row.get::<_, i64>(1)?,
                input_tokens: row.get::<_, Option<i64>>(2)?.map(|n| n.max(0) as u64),
                output_tokens: row.get::<_, Option<i64>>(3)?.map(|n| n.max(0) as u64),
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }
}

fn query_daily(
    conn: &rusqlite::Connection,
    from_ms: i64,
    to_ms: i64,
) -> Result<Vec<DailyTotal>, ProxyError> {
    let mut stmt = conn.prepare(
        r#"
        SELECT
            date(started_at / 1000, 'unixepoch', 'localtime') AS d,
            COUNT(*)                                             AS reqs,
            COALESCE(SUM(input_tokens),          0)              AS input_tokens,
            COALESCE(SUM(output_tokens),         0)              AS output_tokens,
            COALESCE(SUM(cache_read_tokens),     0)              AS cache_read_tokens,
            COALESCE(SUM(cache_creation_tokens), 0)              AS cache_creation_tokens,
            COALESCE(SUM(cost_usd),              0.0)            AS cost_usd
        FROM requests
        WHERE status = 'completed'
          AND started_at BETWEEN ?1 AND ?2
        GROUP BY d
        ORDER BY d ASC
        "#,
    )?;

    let rows = stmt
        .query_map([from_ms, to_ms], |row| {
            Ok(DailyTotal {
                date: row.get(0)?,
                requests: row.get::<_, i64>(1)? as u64,
                input_tokens: row.get::<_, i64>(2)? as u64,
                output_tokens: row.get::<_, i64>(3)? as u64,
                cache_read_tokens: row.get::<_, i64>(4)? as u64,
                cache_creation_tokens: row.get::<_, i64>(5)? as u64,
                cost_usd: row.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn query_models(
    conn: &rusqlite::Connection,
    from_ms: i64,
    to_ms: i64,
) -> Result<Vec<ModelTotal>, ProxyError> {
    let mut stmt = conn.prepare(
        r#"
        SELECT
            model,
            provider,
            COUNT(*)                                             AS reqs,
            COALESCE(SUM(input_tokens),          0)              AS input_tokens,
            COALESCE(SUM(output_tokens),         0)              AS output_tokens,
            COALESCE(SUM(cache_read_tokens),     0)              AS cache_read_tokens,
            COALESCE(SUM(cache_creation_tokens), 0)              AS cache_creation_tokens,
            COALESCE(SUM(cost_usd),              0.0)            AS cost_usd
        FROM requests
        WHERE status = 'completed'
          AND started_at BETWEEN ?1 AND ?2
        GROUP BY model, provider
        ORDER BY cost_usd DESC, model ASC
        "#,
    )?;

    let rows = stmt
        .query_map([from_ms, to_ms], |row| {
            Ok(ModelTotal {
                model: row.get(0)?,
                provider: row.get(1)?,
                requests: row.get::<_, i64>(2)? as u64,
                input_tokens: row.get::<_, i64>(3)? as u64,
                output_tokens: row.get::<_, i64>(4)? as u64,
                cache_read_tokens: row.get::<_, i64>(5)? as u64,
                cache_creation_tokens: row.get::<_, i64>(6)? as u64,
                cost_usd: row.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn count_grouped(
    conn: &Arc<Mutex<Connection>>,
    column: &str,
) -> Result<std::collections::BTreeMap<String, u64>, ProxyError> {
    // SAFETY: `column` is hard-coded by callers ("provider" | "status").
    // No user input flows here, so string interpolation into the query is fine.
    let sql = format!("SELECT {column}, COUNT(*) FROM requests GROUP BY {column}");
    let c = conn.lock().expect("repo mutex poisoned");
    let mut stmt = c.prepare(&sql)?;
    let rows = stmt.query_map([], |r| {
        let key: String = r.get(0)?;
        let n: i64 = r.get(1)?;
        Ok((key, n.max(0) as u64))
    })?;
    let mut out = std::collections::BTreeMap::new();
    for row in rows {
        let (k, v) = row?;
        out.insert(k, v);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::storage::schema::ensure_current;
    use crate::application::ports::RequestLogReadPort;

    fn open_in_memory() -> Connection {
        Connection::open_in_memory().unwrap()
    }

    fn fresh() -> SqliteRequestLogRepository {
        let conn = open_in_memory();
        ensure_current(&conn).unwrap();
        SqliteRequestLogRepository::new(Arc::new(Mutex::new(conn)))
    }

    #[allow(clippy::too_many_arguments)]
    fn seed_completed(
        conn: &rusqlite::Connection,
        id: &str,
        started_at_ms: i64,
        provider: &str,
        model: &str,
        input: i64,
        output: i64,
        cache_read: i64,
        cache_creation: i64,
        cost: f64,
    ) {
        conn.execute(
            r#"
            INSERT INTO requests
                (id, user_id, api_key_id, provider, model, status,
                 started_at, finished_at, error_message,
                 input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                 cost_usd)
            VALUES
                (?1, 1, NULL, ?2, ?3, 'completed',
                 ?4, ?4, NULL,
                 ?5, ?6, ?7, ?8,
                 ?9)
            "#,
            rusqlite::params![
                id,
                provider,
                model,
                started_at_ms,
                input,
                output,
                cache_read,
                cache_creation,
                cost,
            ],
        )
        .unwrap();
    }

    fn seed_errored(
        conn: &rusqlite::Connection,
        id: &str,
        started_at_ms: i64,
        provider: &str,
        model: &str,
    ) {
        conn.execute(
            r#"
            INSERT INTO requests
                (id, user_id, api_key_id, provider, model, status,
                 started_at, finished_at, error_message,
                 input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                 cost_usd)
            VALUES
                (?1, 1, NULL, ?2, ?3, 'errored',
                 ?4, ?4, 'boom',
                 NULL, NULL, NULL, NULL,
                 NULL)
            "#,
            rusqlite::params![id, provider, model, started_at_ms],
        )
        .unwrap();
    }

    fn local_user_id(repo: &SqliteRequestLogRepository) -> i64 {
        let conn = repo.conn.lock().unwrap();
        conn.query_row(
            "SELECT id FROM users WHERE external_id = 'local'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn insert_started_creates_row_with_status_started() {
        let repo = fresh();
        let user_id = local_user_id(&repo);
        repo.insert_started(&RequestStart {
            id: "req-1".into(),
            user_id,
            provider: "anthropic".into(),
            model: "claude-3-5-sonnet-20241022".into(),
            started_at: 1_700_000_000_000,
        })
        .unwrap();

        let conn = repo.conn.lock().unwrap();
        let (status, model): (String, String) = conn
            .query_row(
                "SELECT status, model FROM requests WHERE id = 'req-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "started");
        assert_eq!(model, "claude-3-5-sonnet-20241022");
    }

    #[test]
    fn complete_updates_status_and_tokens() {
        let repo = fresh();
        let user_id = local_user_id(&repo);
        repo.insert_started(&RequestStart {
            id: "req-2".into(),
            user_id,
            provider: "anthropic".into(),
            model: "m".into(),
            started_at: 1,
        })
        .unwrap();
        repo.complete(
            "req-2",
            2,
            &RequestUsage {
                input_tokens: Some(10),
                output_tokens: Some(20),
                cache_read_tokens: Some(0),
                cache_creation_tokens: Some(0),
                cost_usd: Some(0.0123),
            },
        )
        .unwrap();

        let conn = repo.conn.lock().unwrap();
        let (status, input, output, cost): (String, i64, i64, f64) = conn
            .query_row(
                "SELECT status, input_tokens, output_tokens, cost_usd \
                 FROM requests WHERE id = 'req-2'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(status, "completed");
        assert_eq!(input, 10);
        assert_eq!(output, 20);
        assert!((cost - 0.0123).abs() < 1e-9);
    }

    #[test]
    fn fail_records_error_message_and_partial_usage() {
        let repo = fresh();
        let user_id = local_user_id(&repo);
        repo.insert_started(&RequestStart {
            id: "req-3".into(),
            user_id,
            provider: "anthropic".into(),
            model: "m".into(),
            started_at: 1,
        })
        .unwrap();
        repo.fail(
            "req-3",
            5,
            "client disconnected",
            &RequestUsage {
                input_tokens: Some(10),
                ..Default::default()
            },
        )
        .unwrap();

        let conn = repo.conn.lock().unwrap();
        let (status, msg, input, output): (String, String, i64, Option<i64>) = conn
            .query_row(
                "SELECT status, error_message, input_tokens, output_tokens \
                 FROM requests WHERE id = 'req-3'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(status, "errored");
        assert_eq!(msg, "client disconnected");
        assert_eq!(input, 10);
        assert_eq!(output, None);
    }

    #[test]
    fn summarize_empty_db_returns_empty_arrays() {
        let conn = open_in_memory();
        ensure_current(&conn).unwrap();
        let repo = SqliteRequestLogRepository::new(Arc::new(Mutex::new(conn)));

        let summary = repo.summarize(0, i64::MAX).unwrap();

        assert_eq!(summary.from_ms, 0);
        assert_eq!(summary.to_ms, i64::MAX);
        assert!(summary.daily.is_empty());
        assert!(summary.models.is_empty());
    }

    #[test]
    fn summarize_single_completed_row_aggregates_into_one_daily_and_one_model() {
        let conn = open_in_memory();
        ensure_current(&conn).unwrap();
        seed_completed(
            &conn,
            "req-1",
            1_730_000_000_000, // 2024-10-27 in UTC; local tz may differ
            "anthropic",
            "claude-opus-4-5",
            1_000,
            200,
            5_000,
            0,
            0.42,
        );
        let repo = SqliteRequestLogRepository::new(Arc::new(Mutex::new(conn)));

        let summary = repo.summarize(0, i64::MAX).unwrap();

        assert_eq!(summary.daily.len(), 1, "expected one daily bucket");
        let d = &summary.daily[0];
        assert_eq!(d.requests, 1);
        assert_eq!(d.input_tokens, 1_000);
        assert_eq!(d.output_tokens, 200);
        assert_eq!(d.cache_read_tokens, 5_000);
        assert_eq!(d.cache_creation_tokens, 0);
        assert!((d.cost_usd - 0.42).abs() < 1e-9);

        assert_eq!(summary.models.len(), 1);
        let m = &summary.models[0];
        assert_eq!(m.model, "claude-opus-4-5");
        assert_eq!(m.provider, "anthropic");
        assert_eq!(m.requests, 1);
        assert!((m.cost_usd - 0.42).abs() < 1e-9);
    }

    #[test]
    fn summarize_excludes_errored_and_started_rows() {
        let conn = open_in_memory();
        ensure_current(&conn).unwrap();

        // 'started' row (no completion data)
        conn.execute(
            r#"INSERT INTO requests
               (id, user_id, provider, model, status, started_at)
               VALUES ('s1', 1, 'anthropic', 'claude-opus-4-5', 'started', 1730000000000)"#,
            [],
        )
        .unwrap();
        seed_errored(
            &conn,
            "e1",
            1_730_000_000_000,
            "anthropic",
            "claude-opus-4-5",
        );
        seed_completed(
            &conn,
            "c1",
            1_730_000_000_000,
            "anthropic",
            "claude-opus-4-5",
            100,
            50,
            0,
            0,
            0.10,
        );
        let repo = SqliteRequestLogRepository::new(Arc::new(Mutex::new(conn)));

        let summary = repo.summarize(0, i64::MAX).unwrap();

        assert_eq!(summary.daily.len(), 1);
        assert_eq!(
            summary.daily[0].requests, 1,
            "only the completed row should count"
        );
        assert_eq!(summary.models.len(), 1);
        assert_eq!(summary.models[0].requests, 1);
    }

    #[test]
    fn summarize_excludes_rows_outside_range() {
        let conn = open_in_memory();
        ensure_current(&conn).unwrap();

        seed_completed(&conn, "before", 1_000, "anthropic", "m", 1, 1, 0, 0, 0.01);
        seed_completed(&conn, "in-1", 2_000, "anthropic", "m", 1, 1, 0, 0, 0.01);
        seed_completed(&conn, "in-2", 3_000, "anthropic", "m", 1, 1, 0, 0, 0.01);
        seed_completed(&conn, "after", 4_000, "anthropic", "m", 1, 1, 0, 0, 0.01);
        let repo = SqliteRequestLogRepository::new(Arc::new(Mutex::new(conn)));

        let summary = repo.summarize(2_000, 3_000).unwrap();

        let total: u64 = summary.daily.iter().map(|d| d.requests).sum();
        assert_eq!(total, 2, "boundaries inclusive, outside excluded");
        assert_eq!(summary.models[0].requests, 2);
    }

    #[test]
    fn summarize_orders_models_by_cost_desc() {
        let conn = open_in_memory();
        ensure_current(&conn).unwrap();

        seed_completed(&conn, "a1", 1_000, "anthropic", "low", 1, 1, 0, 0, 0.10);
        seed_completed(&conn, "a2", 1_000, "anthropic", "high", 1, 1, 0, 0, 5.00);
        seed_completed(&conn, "a3", 1_000, "anthropic", "mid", 1, 1, 0, 0, 1.00);
        let repo = SqliteRequestLogRepository::new(Arc::new(Mutex::new(conn)));

        let summary = repo.summarize(0, i64::MAX).unwrap();

        let names: Vec<_> = summary.models.iter().map(|m| m.model.clone()).collect();
        assert_eq!(names, vec!["high", "mid", "low"]);
    }

    #[test]
    fn quota_seed_returns_only_completed_rows_after_cutoff() {
        let conn = open_in_memory();
        ensure_current(&conn).unwrap();

        let cutoff_ms: i64 = 5_000;

        // Before cutoff — completed; must be excluded.
        seed_completed(&conn, "before", 1_000, "anthropic", "m", 10, 20, 0, 0, 0.01);

        // After cutoff — completed; must be included.
        seed_completed(&conn, "after-ok", 10_000, "anthropic", "m", 50, 60, 0, 0, 0.05);

        // After cutoff — errored; must be excluded.
        seed_errored(&conn, "after-err", 10_000, "anthropic", "m");

        // After cutoff — still in 'started' state (no finished_at); must be excluded.
        conn.execute(
            r#"INSERT INTO requests
               (id, user_id, provider, model, status, started_at)
               VALUES ('after-started', 1, 'anthropic', 'm', 'started', 10000)"#,
            [],
        )
        .unwrap();

        let repo = SqliteRequestLogRepository::new(Arc::new(Mutex::new(conn)));
        let rows = repo.quota_seed(cutoff_ms).unwrap();

        assert_eq!(rows.len(), 1, "only the completed-after-cutoff row should appear");
        assert_eq!(rows[0].provider, "anthropic");
        assert_eq!(rows[0].started_at_ms, 10_000);
        assert_eq!(rows[0].input_tokens, Some(50));
        assert_eq!(rows[0].output_tokens, Some(60));
    }
}
