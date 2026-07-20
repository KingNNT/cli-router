//! Schema definition and migration runner. Hand-rolled, uses `PRAGMA user_version`.

use rusqlite::{Connection, Error};

const MIGRATIONS: &[(i32, &str)] = &[
    (1, MIGRATION_V1),
    (2, MIGRATION_V2),
    (3, MIGRATION_V3),
    (4, MIGRATION_V4),
    (5, MIGRATION_V5),
    (6, MIGRATION_V6),
    (7, MIGRATION_V7),
    (8, MIGRATION_V8),
];

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

const MIGRATION_V2: &str = r#"
ALTER TABLE requests ADD COLUMN translation_direction TEXT;
"#;

const MIGRATION_V3: &str = r#"
CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS providers (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    name              TEXT NOT NULL UNIQUE,
    kind              TEXT NOT NULL,
    base_url          TEXT,
    openai_base_url   TEXT,
    auth_type         TEXT NOT NULL,
    auth_api_key      TEXT,
    auth_bearer       TEXT,
    auth_access_token TEXT,
    auth_refresh_token TEXT,
    auth_expires_at_ms INTEGER,
    created_at        INTEGER NOT NULL DEFAULT (CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)),
    updated_at        INTEGER NOT NULL DEFAULT (CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER))
);

CREATE TABLE IF NOT EXISTS routing_rules (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    priority   INTEGER NOT NULL DEFAULT 0,
    provider   TEXT NOT NULL,
    model_glob TEXT NOT NULL,
    strategy   TEXT NOT NULL DEFAULT 'failover',
    fallback   TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL DEFAULT (CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER))
);

CREATE INDEX IF NOT EXISTS idx_routing_rules_priority ON routing_rules(priority);

CREATE TABLE IF NOT EXISTS quota_rules (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    provider          TEXT NOT NULL,
    window            TEXT NOT NULL,
    max_requests      INTEGER,
    max_input_tokens  INTEGER,
    max_output_tokens INTEGER,
    warn_pct          INTEGER NOT NULL DEFAULT 80
);
"#;

const MIGRATION_V4: &str = r#"
ALTER TABLE providers ADD COLUMN reasoning_effort TEXT;
"#;

const MIGRATION_V5: &str = r#"
ALTER TABLE providers ADD COLUMN thinking_mode TEXT NOT NULL DEFAULT 'split_only';
"#;

const MIGRATION_V6: &str = r#"
ALTER TABLE providers ADD COLUMN max_concurrent INTEGER;
"#;

const MIGRATION_V7: &str = r#"
ALTER TABLE providers ADD COLUMN sanitize_empty_tools INTEGER NOT NULL DEFAULT 0;
"#;

const MIGRATION_V8: &str = r#"
ALTER TABLE providers ADD COLUMN enabled INTEGER NOT NULL DEFAULT 1;
"#;

pub fn ensure_current(conn: &Connection) -> Result<(), Error> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    // WAL mode is safe with NORMAL — much faster writes without risking corruption.
    // The DB file may lose the last few transactions on power loss, but WAL itself
    // is durable and the proxy's request log is not mission-critical data.
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    // Wait up to 5 seconds if the DB is locked by another writer instead of
    // failing immediately with SQLITE_BUSY. Critical under high concurrency.
    conn.pragma_update(None, "busy_timeout", 5000)?;

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

/// Apply read-performance PRAGMAs to a read-only connection.
/// WAL mode allows concurrent reads while another connection is writing.
pub fn configure_readonly(conn: &Connection) -> Result<(), Error> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
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
    fn v2_adds_translation_direction_column() {
        let conn = open_in_memory();
        ensure_current(&conn).unwrap();
        let cols: Vec<String> = conn
            .prepare("PRAGMA table_info(requests)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert!(cols.contains(&"translation_direction".into()));
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

    #[test]
    fn v3_creates_config_tables() {
        let conn = open_in_memory();
        ensure_current(&conn).unwrap();
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert!(tables.contains(&"settings".into()));
        assert!(tables.contains(&"providers".into()));
        assert!(tables.contains(&"routing_rules".into()));
        assert!(tables.contains(&"quota_rules".into()));
    }

    #[test]
    fn v3_is_idempotent() {
        let conn = open_in_memory();
        ensure_current(&conn).unwrap();
        ensure_current(&conn).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM settings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "no seed data in settings");
    }

    #[test]
    fn v4_adds_provider_reasoning_effort_column() {
        let conn = open_in_memory();
        ensure_current(&conn).unwrap();
        let cols: Vec<String> = conn
            .prepare("PRAGMA table_info(providers)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert!(cols.contains(&"reasoning_effort".into()));
    }

    #[test]
    fn v5_adds_provider_thinking_mode_column() {
        let conn = open_in_memory();
        ensure_current(&conn).unwrap();
        let cols: Vec<String> = conn
            .prepare("PRAGMA table_info(providers)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert!(cols.contains(&"thinking_mode".into()));
    }

    #[test]
    fn v7_adds_provider_sanitize_empty_tools_column() {
        let conn = open_in_memory();
        ensure_current(&conn).unwrap();
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('providers')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(cols.contains(&"sanitize_empty_tools".into()));
    }
}
