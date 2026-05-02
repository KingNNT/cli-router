//! Schema definition and migration runner. Hand-rolled, uses `PRAGMA user_version`.

use rusqlite::{Connection, Error};

const MIGRATIONS: &[(i32, &str)] = &[(1, MIGRATION_V1)];

const MIGRATION_V1: &str = r#"
CREATE TABLE users (
    id          INTEGER PRIMARY KEY,
    external_id TEXT UNIQUE NOT NULL,
    created_at  INTEGER NOT NULL
);

INSERT INTO users (external_id, created_at)
VALUES ('local', CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER));

CREATE TABLE api_keys (
    id           INTEGER PRIMARY KEY,
    user_id      INTEGER NOT NULL REFERENCES users(id),
    name         TEXT NOT NULL,
    key_hash     TEXT NOT NULL UNIQUE,
    created_at   INTEGER NOT NULL,
    last_used_at INTEGER
);

CREATE TABLE requests (
    id                       TEXT PRIMARY KEY,
    user_id                  INTEGER NOT NULL REFERENCES users(id),
    api_key_id               INTEGER REFERENCES api_keys(id),
    provider                 TEXT NOT NULL,
    model                    TEXT NOT NULL,
    status                   TEXT NOT NULL,
    started_at               INTEGER NOT NULL,
    finished_at              INTEGER,
    error_message            TEXT,
    input_tokens             INTEGER,
    output_tokens            INTEGER,
    cache_read_tokens        INTEGER,
    cache_creation_tokens    INTEGER,
    cost_usd                 REAL
);

CREATE INDEX idx_requests_user_started ON requests(user_id, started_at DESC);
CREATE INDEX idx_requests_model        ON requests(model);
"#;

pub fn ensure_current(conn: &Connection) -> Result<(), Error> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;

    let mut current: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for (target, sql) in MIGRATIONS {
        if *target > current {
            conn.execute_batch(sql)?;
            conn.pragma_update(None, "user_version", target)?;
            current = *target;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_in_memory() -> Connection {
        Connection::open_in_memory().expect("open in-memory db")
    }

    #[test]
    fn fresh_db_has_required_tables_after_migrate() {
        let conn = open_in_memory();
        ensure_current(&conn).unwrap();

        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();

        assert!(tables.contains(&"users".into()));
        assert!(tables.contains(&"api_keys".into()));
        assert!(tables.contains(&"requests".into()));
    }

    #[test]
    fn fresh_db_has_local_user() {
        let conn = open_in_memory();
        ensure_current(&conn).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM users WHERE external_id = 'local'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn migrate_is_idempotent() {
        let conn = open_in_memory();
        ensure_current(&conn).unwrap();
        ensure_current(&conn).unwrap();
        ensure_current(&conn).unwrap();
        let user_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            user_count, 1,
            "rerunning migrations must not duplicate seed rows"
        );
    }
}
