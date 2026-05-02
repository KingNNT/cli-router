//! SQLite-backed RequestLogPort adapter.

use crate::application::errors::ProxyError;
use crate::application::ports::RequestLogPort;
use crate::domain::{RequestStart, RequestUsage};
use rusqlite::{params, Connection};
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

    fn fresh() -> SqliteRequestLogRepository {
        let conn = Connection::open_in_memory().unwrap();
        ensure_current(&conn).unwrap();
        SqliteRequestLogRepository::new(Arc::new(Mutex::new(conn)))
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
}
