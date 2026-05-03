# Proxy-TUI Usage Dashboard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a Usage tab to `proxy-tui` showing daily totals + per-model breakdown for a selected time range, fed by a new admin endpoint that aggregates the proxy's existing request log server-side.

**Architecture:** The proxy gains one new sync read method on `RequestLogReadPort` (`summarize(from_ms, to_ms)`) implemented in `SqliteRequestLogRepository`, exposed via a new `GET /admin/usage/summary` endpoint. `proxy-tui` gains a new `View::Usage` tab that calls the endpoint with preset ranges (Today / 7d / 30d / All) and renders an overview card + per-model table.

**Tech Stack:** Rust 2024, axum, rusqlite, ratatui, ureq, chrono, thiserror, wiremock (dev).

**Spec:** `docs/superpowers/specs/2026-05-03-proxy-tui-usage-dashboard-design.md`

---

## File Structure

### New files

- `crates/shared/src/adapters/presenters/mod.rs` — module declaration
- `crates/shared/src/adapters/presenters/formatting.rs` — relocated from `analysis`
- `crates/proxy/tests/usage_summary_api.rs` — integration test for the new endpoint
- `crates/proxy-tui/src/views/mod.rs` — module declaration
- `crates/proxy-tui/src/views/usage.rs` — Usage tab renderer

### Modified files

- `crates/proxy-admin-api/src/lib.rs` — add `UsageSummaryResponse`, `DailyUsageRow`, `ModelUsageRow`
- `crates/shared/src/adapters/mod.rs` — declare `presenters` module
- `crates/analysis/src/adapters/presenters/formatting.rs` — replace with re-export from `shared`
- `crates/proxy/src/domain/mod.rs` — add `UsageSummary`, `DailyTotal`, `ModelTotal`
- `crates/proxy/src/application/ports/request_log_read.rs` — add `summarize` method
- `crates/proxy/src/adapters/storage/sqlite_request_log.rs` — impl `summarize`
- `crates/proxy/src/application/use_cases/admin.rs` — add `GetUsageSummary`
- `crates/proxy/src/frameworks/admin.rs` — add `/admin/usage/summary` route + handler, extend `AdminState`
- `crates/proxy/src/main.rs` — construct `GetUsageSummary` and add it to `AdminState`
- `crates/proxy-tui/Cargo.toml` — add `shared` dep
- `crates/proxy-tui/src/client.rs` — add `get_usage_summary`
- `crates/proxy-tui/src/app.rs` — add `View::Usage`, `RangePreset`, `UsagePaneState`
- `crates/proxy-tui/src/main.rs` — declare `views` module, wire keys, initial fetch on first Usage entry
- `crates/proxy-tui/src/ui.rs` — dispatch `View::Usage` to `views::usage::draw`

---

## Task 1: Move formatting helpers from `analysis` to `shared`

**Files:**
- Create: `crates/shared/src/adapters/presenters/mod.rs`
- Create: `crates/shared/src/adapters/presenters/formatting.rs`
- Modify: `crates/shared/src/adapters/mod.rs`
- Modify: `crates/analysis/src/adapters/presenters/formatting.rs` (replace with re-export)

The `analysis` crate's `formatting.rs` is the canonical implementation. Move the entire module (including its existing tests) to `shared`, then have `analysis` re-export so existing callers keep working without touching imports.

- [ ] **Step 1: Create `crates/shared/src/adapters/presenters/mod.rs`**

```rust
//! User-facing formatting helpers shared by analysis and proxy-tui.

pub mod formatting;
```

- [ ] **Step 2: Copy `formatting.rs` from analysis to shared**

Run:
```bash
cp crates/analysis/src/adapters/presenters/formatting.rs \
   crates/shared/src/adapters/presenters/formatting.rs
```

- [ ] **Step 3: Declare `presenters` in `shared/src/adapters/mod.rs`**

Modify `crates/shared/src/adapters/mod.rs`. Append:

```rust
pub mod presenters;
```

(File currently declares `clock`, `errors`, `gateways`.)

- [ ] **Step 4: Replace `analysis/src/adapters/presenters/formatting.rs` with a re-export**

Overwrite the file contents with:

```rust
//! Formatting helpers — relocated to `shared::adapters::presenters::formatting`.
//! Re-exported here so existing call sites in `analysis` keep their imports.

pub use shared::adapters::presenters::formatting::{
    fmt_cost, fmt_num, fmt_num_compact, fmt_num_trunc, format_date_md, format_date_year,
    trunc_model,
};
```

- [ ] **Step 5: Verify the workspace still builds and tests pass**

Run:
```bash
cargo build --workspace
cargo test --workspace
```
Expected: clean build, all tests green. The relocated tests run from `shared` now.

- [ ] **Step 6: Commit**

```bash
git add crates/shared/src/adapters/mod.rs \
        crates/shared/src/adapters/presenters \
        crates/analysis/src/adapters/presenters/formatting.rs
git commit -m "refactor(shared): move formatting helpers from analysis to shared"
```

---

## Task 2: Add usage summary DTOs to `proxy-admin-api`

**Files:**
- Modify: `crates/proxy-admin-api/src/lib.rs`

Pure data + serde. No logic.

- [ ] **Step 1: Add the DTOs at the end of `lib.rs`**

Append to `crates/proxy-admin-api/src/lib.rs`:

```rust
/// `GET /admin/usage/summary?from=<ms>&to=<ms>`
///
/// Server-side aggregated view of the proxy's request log. `from`/`to` are
/// inclusive epoch milliseconds. Both arrays are empty when no rows match.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageSummaryResponse {
    pub from_ms: i64,
    pub to_ms: i64,
    /// One row per local-tz day. Sorted by `date` ascending.
    pub daily: Vec<DailyUsageRow>,
    /// One row per (model, provider) pair. Sorted by `cost_usd` descending.
    pub models: Vec<ModelUsageRow>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DailyUsageRow {
    /// Local-tz date as `YYYY-MM-DD`.
    pub date: String,
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelUsageRow {
    pub model: String,
    /// Value from `requests.provider` as logged — may be `"anthropic"`,
    /// `"zai"`, or `"router"` per the existing `StatusResponse` note.
    pub provider: String,
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cost_usd: f64,
}
```

- [ ] **Step 2: Add a serde round-trip test**

Append to `crates/proxy-admin-api/src/lib.rs`:

```rust
#[cfg(test)]
mod usage_summary_tests {
    use super::*;

    #[test]
    fn usage_summary_response_round_trips_through_json() {
        let original = UsageSummaryResponse {
            from_ms: 1_700_000_000_000,
            to_ms: 1_700_086_400_000,
            daily: vec![DailyUsageRow {
                date: "2026-05-03".into(),
                requests: 12,
                input_tokens: 1_000,
                output_tokens: 200,
                cache_read_tokens: 5_000,
                cache_creation_tokens: 0,
                cost_usd: 0.42,
            }],
            models: vec![ModelUsageRow {
                model: "claude-opus-4-5".into(),
                provider: "anthropic".into(),
                requests: 12,
                input_tokens: 1_000,
                output_tokens: 200,
                cache_read_tokens: 5_000,
                cache_creation_tokens: 0,
                cost_usd: 0.42,
            }],
        };

        let json = serde_json::to_string(&original).unwrap();
        let decoded: UsageSummaryResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, original);
    }
}
```

- [ ] **Step 3: Run the test**

Run:
```bash
cargo test -p proxy-admin-api usage_summary_response_round_trips_through_json
```
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/proxy-admin-api/src/lib.rs
git commit -m "feat(proxy-admin-api): add usage summary DTOs"
```

---

## Task 3: Add domain types for the usage summary in `proxy`

**Files:**
- Modify: `crates/proxy/src/domain/mod.rs` (or the appropriate domain submodule — match the existing pattern)

Domain-side mirror of the DTO. Keeps the application layer free of `proxy_admin_api` types.

- [ ] **Step 1: Inspect `proxy/src/domain/mod.rs` to see how existing types are organised**

Run:
```bash
cat crates/proxy/src/domain/mod.rs
```
Note where `RequestStart`, `RequestUsage`, `UsageRecord`, `RequestStatus` live, and follow that pattern.

- [ ] **Step 2: Add the domain types**

Add to `crates/proxy/src/domain/mod.rs` (or a new `domain/usage_summary.rs` re-exported from `mod.rs` if the existing types are file-split — match what's there):

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct UsageSummary {
    pub from_ms: i64,
    pub to_ms: i64,
    pub daily: Vec<DailyTotal>,
    pub models: Vec<ModelTotal>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DailyTotal {
    pub date: String, // YYYY-MM-DD, local tz
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelTotal {
    pub model: String,
    pub provider: String,
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cost_usd: f64,
}
```

- [ ] **Step 3: Verify it compiles**

Run:
```bash
cargo check -p proxy
```
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/proxy/src/domain
git commit -m "feat(proxy): add UsageSummary domain types"
```

---

## Task 4: Add `summarize` to `RequestLogReadPort`

**Files:**
- Modify: `crates/proxy/src/application/ports/request_log_read.rs`

Adding a method without an implementation breaks `cargo check` until Task 5 is done — that's expected. Tasks 4 and 5 are committed together if you'd rather, but keeping them separate makes the trait change reviewable on its own. Pick whichever you prefer; instructions below treat them as separate commits.

- [ ] **Step 1: Inspect the existing port shape**

Run:
```bash
cat crates/proxy/src/application/ports/request_log_read.rs
```
Note the existing method signatures and which domain types are imported.

- [ ] **Step 2: Add the `summarize` method to the trait**

Add this method to the `RequestLogReadPort` trait. Update the `use` line at the top of the file to import `UsageSummary` from the domain module if not already covered:

```rust
fn summarize(&self, from_ms: i64, to_ms: i64) -> Result<UsageSummary, ProxyError>;
```

(Use the same `Result` type alias / error type the existing methods use. If the existing methods return `Result<T, ProxyError>` directly, mirror that.)

- [ ] **Step 3: Verify it fails to compile because no impl yet**

Run:
```bash
cargo check -p proxy
```
Expected: error in `sqlite_request_log.rs`: "not all trait items implemented, missing: `summarize`". This confirms the trait extension reached the impl. Do NOT commit yet — Task 5 fixes the impl.

---

## Task 5: Implement `summarize` in `SqliteRequestLogRepository` (TDD)

**Files:**
- Modify: `crates/proxy/src/adapters/storage/sqlite_request_log.rs`

Server-side `GROUP BY` against the `requests` table. Use TDD: write tests against the impl one behavior at a time. The data-shape rules from `.claude/rules/data-shape.md` apply:

- `started_at` is epoch ms — divide by 1000 for `unixepoch`
- All `SUM(...)` wrapped in `COALESCE(..., 0)` (or `0.0` for `cost_usd`)
- Filter by `status = 'completed'`

- [ ] **Step 1: Inspect the existing impl block and test module**

Run:
```bash
grep -n "fn insert_started\|fn complete\|fn fail\|#\[cfg(test)\]\|fn open_in_memory\|fn seed" crates/proxy/src/adapters/storage/sqlite_request_log.rs
```
Note the test setup helpers (`open_in_memory`, any seeding helpers) so the new tests reuse them.

- [ ] **Step 2: Write the first failing test — empty DB**

Add to the `#[cfg(test)] mod tests` block in `crates/proxy/src/adapters/storage/sqlite_request_log.rs`:

```rust
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
```

(If the existing test module uses `Arc<Mutex<Connection>>` differently, match the pattern. `open_in_memory` and `ensure_current` are already imported in the test module per Task 5 Step 1.)

- [ ] **Step 3: Run the test — expected to fail**

Run:
```bash
cargo test -p proxy summarize_empty_db_returns_empty_arrays
```
Expected: compilation failure (`summarize` not implemented) **or** test failure if you've stubbed it out.

- [ ] **Step 4: Implement `summarize` minimally — empty results**

Add to the `impl RequestLogReadPort for SqliteRequestLogRepository` block:

```rust
fn summarize(&self, from_ms: i64, to_ms: i64) -> Result<UsageSummary, ProxyError> {
    let conn = self.conn.lock().expect("request log mutex poisoned");

    let daily = query_daily(&conn, from_ms, to_ms)?;
    let models = query_models(&conn, from_ms, to_ms)?;

    Ok(UsageSummary { from_ms, to_ms, daily, models })
}
```

Add the helpers (private functions in the same file, below the impl block):

```rust
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
```

Add the necessary imports at the top of the file:
```rust
use crate::domain::{DailyTotal, ModelTotal, UsageSummary};
```

(Match the path with how the domain types were exported in Task 3.)

- [ ] **Step 5: Run the empty-DB test — expected to pass**

Run:
```bash
cargo test -p proxy summarize_empty_db_returns_empty_arrays
```
Expected: PASS.

- [ ] **Step 6: Add a helper that seeds a completed request row**

Add to the test module:

```rust
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
            id, provider, model, started_at_ms,
            input, output, cache_read, cache_creation, cost,
        ],
    )
    .unwrap();
}
```

- [ ] **Step 7: Write a failing test for a single completed row**

Add to the test module:

```rust
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
        1_000, 200, 5_000, 0,
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
```

- [ ] **Step 8: Run the test — expected to pass**

Run:
```bash
cargo test -p proxy summarize_single_completed_row_aggregates_into_one_daily_and_one_model
```
Expected: PASS (impl from Step 4 already covers this).

- [ ] **Step 9: Add a failing test for status filtering**

Add to the test module. Reuse `seed_completed` and add a `seed_errored` helper (mirror it, just with `'errored'` and a non-NULL `error_message`):

```rust
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

#[test]
fn summarize_excludes_errored_and_started_rows() {
    let conn = open_in_memory();
    ensure_current(&conn).unwrap();

    // 'started' row (no completion data)
    conn.execute(
        r#"INSERT INTO requests
           (id, user_id, provider, model, status, started_at)
           VALUES ('s1', 1, 'anthropic', 'claude-opus-4-5', 'started', 1_730_000_000_000)"#,
        [],
    )
    .unwrap();
    seed_errored(&conn, "e1", 1_730_000_000_000, "anthropic", "claude-opus-4-5");
    seed_completed(
        &conn, "c1", 1_730_000_000_000, "anthropic", "claude-opus-4-5",
        100, 50, 0, 0, 0.10,
    );
    let repo = SqliteRequestLogRepository::new(Arc::new(Mutex::new(conn)));

    let summary = repo.summarize(0, i64::MAX).unwrap();

    assert_eq!(summary.daily.len(), 1);
    assert_eq!(summary.daily[0].requests, 1, "only the completed row should count");
    assert_eq!(summary.models.len(), 1);
    assert_eq!(summary.models[0].requests, 1);
}
```

- [ ] **Step 10: Run — expected to pass**

Run:
```bash
cargo test -p proxy summarize_excludes_errored_and_started_rows
```
Expected: PASS (impl filters `status = 'completed'`).

- [ ] **Step 11: Add a failing test for range filtering**

Add to the test module:

```rust
#[test]
fn summarize_excludes_rows_outside_range() {
    let conn = open_in_memory();
    ensure_current(&conn).unwrap();

    seed_completed(&conn, "before", 1_000, "anthropic", "m", 1, 1, 0, 0, 0.01);
    seed_completed(&conn, "in-1",   2_000, "anthropic", "m", 1, 1, 0, 0, 0.01);
    seed_completed(&conn, "in-2",   3_000, "anthropic", "m", 1, 1, 0, 0, 0.01);
    seed_completed(&conn, "after",  4_000, "anthropic", "m", 1, 1, 0, 0, 0.01);
    let repo = SqliteRequestLogRepository::new(Arc::new(Mutex::new(conn)));

    let summary = repo.summarize(2_000, 3_000).unwrap();

    let total: u64 = summary.daily.iter().map(|d| d.requests).sum();
    assert_eq!(total, 2, "boundaries inclusive, outside excluded");
    assert_eq!(summary.models[0].requests, 2);
}
```

- [ ] **Step 12: Run — expected to pass**

Run:
```bash
cargo test -p proxy summarize_excludes_rows_outside_range
```
Expected: PASS.

- [ ] **Step 13: Add a failing test for `models` ordering by cost desc**

Add to the test module:

```rust
#[test]
fn summarize_orders_models_by_cost_desc() {
    let conn = open_in_memory();
    ensure_current(&conn).unwrap();

    seed_completed(&conn, "a1", 1_000, "anthropic", "low",  1, 1, 0, 0, 0.10);
    seed_completed(&conn, "a2", 1_000, "anthropic", "high", 1, 1, 0, 0, 5.00);
    seed_completed(&conn, "a3", 1_000, "anthropic", "mid",  1, 1, 0, 0, 1.00);
    let repo = SqliteRequestLogRepository::new(Arc::new(Mutex::new(conn)));

    let summary = repo.summarize(0, i64::MAX).unwrap();

    let names: Vec<_> = summary.models.iter().map(|m| m.model.clone()).collect();
    assert_eq!(names, vec!["high", "mid", "low"]);
}
```

- [ ] **Step 14: Run — expected to pass**

Run:
```bash
cargo test -p proxy summarize_orders_models_by_cost_desc
```
Expected: PASS.

- [ ] **Step 15: Run the full proxy test suite to confirm nothing else broke**

Run:
```bash
cargo test -p proxy
cargo clippy -p proxy -- -D warnings
```
Expected: green clippy + tests.

- [ ] **Step 16: Commit Tasks 4 and 5 together**

```bash
git add crates/proxy/src/application/ports/request_log_read.rs \
        crates/proxy/src/adapters/storage/sqlite_request_log.rs
git commit -m "feat(proxy): aggregate request log by day and model"
```

---

## Task 6: Add `GetUsageSummary` use case

**Files:**
- Modify: `crates/proxy/src/application/use_cases/admin.rs`

Follow the pattern of existing structs in this file (`GetStatus`, `GetRecentRequests`).

- [ ] **Step 1: Add the use case struct + impl**

Append to `crates/proxy/src/application/use_cases/admin.rs`:

```rust
pub struct GetUsageSummary {
    read: Arc<dyn RequestLogReadPort>,
}

impl GetUsageSummary {
    pub fn new(read: Arc<dyn RequestLogReadPort>) -> Self {
        Self { read }
    }

    pub fn execute(
        &self,
        from_ms: i64,
        to_ms: i64,
    ) -> Result<proxy_admin_api::UsageSummaryResponse, ProxyError> {
        if from_ms > to_ms {
            return Err(ProxyError::BadRequest("from must be <= to".into()));
        }
        let s = self.read.summarize(from_ms, to_ms)?;
        Ok(proxy_admin_api::UsageSummaryResponse {
            from_ms: s.from_ms,
            to_ms: s.to_ms,
            daily: s
                .daily
                .into_iter()
                .map(|d| proxy_admin_api::DailyUsageRow {
                    date: d.date,
                    requests: d.requests,
                    input_tokens: d.input_tokens,
                    output_tokens: d.output_tokens,
                    cache_read_tokens: d.cache_read_tokens,
                    cache_creation_tokens: d.cache_creation_tokens,
                    cost_usd: d.cost_usd,
                })
                .collect(),
            models: s
                .models
                .into_iter()
                .map(|m| proxy_admin_api::ModelUsageRow {
                    model: m.model,
                    provider: m.provider,
                    requests: m.requests,
                    input_tokens: m.input_tokens,
                    output_tokens: m.output_tokens,
                    cache_read_tokens: m.cache_read_tokens,
                    cache_creation_tokens: m.cache_creation_tokens,
                    cost_usd: m.cost_usd,
                })
                .collect(),
        })
    }
}
```

If `proxy_admin_api` isn't already imported in this file, add `use proxy_admin_api;` at the top — match how the existing use cases import it.

- [ ] **Step 2: Add a stub-based unit test for the happy path and bad-request path**

The file already contains `mod tests` with a `StubRead` (per the grep in pre-work) — extend it. If `StubRead` doesn't already implement `summarize`, add it now (return a fixed `UsageSummary` with one daily row and one model row):

```rust
// inside the existing impl RequestLogReadPort for StubRead
fn summarize(&self, from_ms: i64, to_ms: i64) -> Result<UsageSummary, ProxyError> {
    Ok(UsageSummary {
        from_ms,
        to_ms,
        daily: vec![DailyTotal {
            date: "2026-05-03".into(),
            requests: 3,
            input_tokens: 100,
            output_tokens: 50,
            cache_read_tokens: 10,
            cache_creation_tokens: 5,
            cost_usd: 0.25,
        }],
        models: vec![ModelTotal {
            model: "claude-opus-4-5".into(),
            provider: "anthropic".into(),
            requests: 3,
            input_tokens: 100,
            output_tokens: 50,
            cache_read_tokens: 10,
            cache_creation_tokens: 5,
            cost_usd: 0.25,
        }],
    })
}
```

Then add the tests:

```rust
#[test]
fn get_usage_summary_maps_domain_to_dto() {
    let uc = GetUsageSummary::new(stub());
    let resp = uc.execute(0, i64::MAX).unwrap();
    assert_eq!(resp.from_ms, 0);
    assert_eq!(resp.to_ms, i64::MAX);
    assert_eq!(resp.daily.len(), 1);
    assert_eq!(resp.daily[0].requests, 3);
    assert_eq!(resp.models.len(), 1);
    assert_eq!(resp.models[0].model, "claude-opus-4-5");
}

#[test]
fn get_usage_summary_rejects_inverted_range() {
    let uc = GetUsageSummary::new(stub());
    let err = uc.execute(100, 50).unwrap_err();
    assert!(matches!(err, ProxyError::BadRequest(_)));
}
```

- [ ] **Step 3: Run the tests**

Run:
```bash
cargo test -p proxy get_usage_summary
```
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/proxy/src/application/use_cases/admin.rs
git commit -m "feat(proxy): add GetUsageSummary use case"
```

---

## Task 7: Wire the new endpoint into the admin router (with integration test)

**Files:**
- Modify: `crates/proxy/src/frameworks/admin.rs`
- Modify: `crates/proxy/src/main.rs`
- Create: `crates/proxy/tests/usage_summary_api.rs`

Existing `AdminState` shape (from `frameworks/admin.rs`) is:

```rust
pub struct AdminState {
    pub get_status: Arc<GetStatus>,
    pub get_config: Arc<GetConfig>,
    pub get_recent: Arc<GetRecentRequests>,
    pub update_config: Arc<UpdateConfig>,
    pub test_provider: Arc<TestProvider>,
    pub start_oauth: Arc<StartAnthropicOAuth>,
    pub complete_oauth: Arc<CompleteAnthropicOAuth>,
}
```

Building all of these in a test is overkill. We use axum's `FromRef` pattern so the new handler takes only `State<Arc<GetUsageSummary>>` (a substate). The integration test then mounts just the new route with that substate directly — no need to construct the full `AdminState`.

- [ ] **Step 1: Add `usage_summary` to `AdminState`**

In `crates/proxy/src/frameworks/admin.rs`, add the field:

```rust
pub usage_summary: Arc<GetUsageSummary>,
```

Add `GetUsageSummary` to the existing use-case imports.

- [ ] **Step 2: Add the route, query type, handler, and `FromRef` impl**

In the same file, add the imports `Query`, `FromRef` if missing:

```rust
use axum::extract::{FromRef, Query, State};
```

(Replace the existing `axum::extract::State` import — combine into one line.)

Add the query DTO, handler, and substate extractor:

```rust
#[derive(Debug, Deserialize)]
pub struct UsageSummaryQuery {
    pub from: i64,
    pub to: i64,
}

impl FromRef<AdminState> for Arc<GetUsageSummary> {
    fn from_ref(s: &AdminState) -> Self {
        s.usage_summary.clone()
    }
}

async fn usage_summary_handler(
    State(uc): State<Arc<GetUsageSummary>>,
    Query(q): Query<UsageSummaryQuery>,
) -> Result<Json<proxy_admin_api::UsageSummaryResponse>, ProxyError> {
    let resp = uc.execute(q.from, q.to)?;
    Ok(Json(resp))
}
```

Add `UsageSummaryResponse` to the `proxy_admin_api` import block at the top of the file.

In `build_admin_router`, append the route before `.with_state(state)`:

```rust
.route("/admin/usage/summary", get(usage_summary_handler))
```

(The handler's `State<Arc<GetUsageSummary>>` is satisfied automatically because `Arc<GetUsageSummary>` implements `FromRef<AdminState>`.)

- [ ] **Step 3: Construct `usage_summary` in `main.rs`**

In `crates/proxy/src/main.rs`, find the `let admin = AdminState { … }` block. Above it, add:

```rust
let usage_summary = Arc::new(GetUsageSummary::new(request_read.clone()));
```

Add `usage_summary,` to the `AdminState { ... }` struct literal. Update the use-case import line to include `GetUsageSummary`.

(Note: `request_read` already exists in `main.rs` and is currently passed to `GetStatus::new` and `GetRecentRequests::new`. Re-using it here is consistent.)

- [ ] **Step 4: Verify the proxy still builds**

Run:
```bash
cargo build -p proxy
```
Expected: clean.

- [ ] **Step 5: Write the integration test**

Create `crates/proxy/tests/usage_summary_api.rs`:

```rust
//! Integration test: GET /admin/usage/summary against an in-memory SQLite
//! seeded with a handful of completed requests.
//!
//! We mount only the new route directly on its own `Router` with
//! `Arc<GetUsageSummary>` as state — no need to assemble the full
//! `AdminState` for a one-route test.

use std::sync::{Arc, Mutex};

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use axum::extract::{Query, State};
use proxy::adapters::storage::{SqliteRequestLogRepository, ensure_current};
use proxy::application::ports::RequestLogReadPort;
use proxy::application::use_cases::admin::GetUsageSummary;
use proxy::application::errors::ProxyError;
use proxy_admin_api::UsageSummaryResponse;
use rusqlite::Connection;
use serde::Deserialize;
use tower::ServiceExt;

#[derive(Deserialize)]
struct UsageSummaryQuery {
    from: i64,
    to: i64,
}

async fn handler(
    State(uc): State<Arc<GetUsageSummary>>,
    Query(q): Query<UsageSummaryQuery>,
) -> Result<Json<UsageSummaryResponse>, ProxyError> {
    Ok(Json(uc.execute(q.from, q.to)?))
}

fn seed(conn: &Connection, id: &str, started_ms: i64, model: &str, cost: f64) {
    conn.execute(
        r#"INSERT INTO requests
           (id, user_id, provider, model, status,
            started_at, finished_at,
            input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
            cost_usd)
           VALUES (?1, 1, 'anthropic', ?2, 'completed', ?3, ?3, 1000, 200, 0, 0, ?4)"#,
        rusqlite::params![id, model, started_ms, cost],
    )
    .unwrap();
}

#[tokio::test]
async fn usage_summary_endpoint_returns_aggregated_rows() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();
    seed(&conn, "r1", 1_730_000_000_000, "claude-opus-4-5", 1.50);
    seed(&conn, "r2", 1_730_000_000_000, "claude-opus-4-5", 0.50);
    seed(&conn, "r3", 1_730_000_000_000, "claude-sonnet-4-6", 0.10);

    let repo = Arc::new(SqliteRequestLogRepository::new(Arc::new(Mutex::new(conn))));
    let read: Arc<dyn RequestLogReadPort> = repo;
    let usage_summary = Arc::new(GetUsageSummary::new(read));

    let app = Router::new()
        .route("/admin/usage/summary", get(handler))
        .with_state(usage_summary);

    let req = Request::builder()
        .uri("/admin/usage/summary?from=0&to=9999999999999")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let parsed: UsageSummaryResponse = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(parsed.daily.len(), 1);
    assert_eq!(parsed.daily[0].requests, 3);
    assert_eq!(parsed.models.len(), 2);
    assert_eq!(parsed.models[0].model, "claude-opus-4-5"); // higher cost
    assert!((parsed.models[0].cost_usd - 2.00).abs() < 1e-9);
}

#[tokio::test]
async fn usage_summary_endpoint_rejects_inverted_range() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();
    let repo = Arc::new(SqliteRequestLogRepository::new(Arc::new(Mutex::new(conn))));
    let read: Arc<dyn RequestLogReadPort> = repo;
    let usage_summary = Arc::new(GetUsageSummary::new(read));

    let app = Router::new()
        .route("/admin/usage/summary", get(handler))
        .with_state(usage_summary);

    let req = Request::builder()
        .uri("/admin/usage/summary?from=200&to=100")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
```

The test re-declares `UsageSummaryQuery` and `handler` locally so the test is self-contained — they're trivial wrappers around `GetUsageSummary::execute`. The point is to verify route wiring, query-string parsing, and JSON shape end-to-end.

The second test depends on `ProxyError::BadRequest` mapping to a 400 in axum. If it doesn't (because `IntoResponse` for `ProxyError` lives in `frameworks/error.rs` per the comment in `application/errors.rs`), assert against whatever status `IntoResponse` produces — adjust the expected status to match. Run the test once to confirm.

- [ ] **Step 6: Add tower as a dev-dep if it isn't already**

Run:
```bash
cargo test -p proxy --test usage_summary_api 2>&1 | head -40
```

If the build complains about `tower` or `tower::ServiceExt`, add it to `crates/proxy/Cargo.toml` `[dev-dependencies]`:

```toml
tower = { workspace = true }
```

(`tower` is the standard helper for `oneshot`-style axum testing.)

- [ ] **Step 7: Run the integration test**

Run:
```bash
cargo test -p proxy --test usage_summary_api
```
Expected: PASS for both tests (adjust the second test's expected status code if `ProxyError::BadRequest`'s `IntoResponse` differs from `400`).

- [ ] **Step 8: Confirm full proxy suite + clippy is green**

Run:
```bash
cargo test -p proxy
cargo clippy -p proxy -- -D warnings
```
Expected: green.

- [ ] **Step 9: Commit**

```bash
git add crates/proxy/src/frameworks/admin.rs \
        crates/proxy/src/main.rs \
        crates/proxy/Cargo.toml \
        crates/proxy/tests/usage_summary_api.rs
git commit -m "feat(proxy): expose GET /admin/usage/summary"
```

---

## Task 8: Add `shared` and `wiremock` dev-dep to `proxy-tui`, then `get_usage_summary` client method

**Files:**
- Modify: `crates/proxy-tui/Cargo.toml`
- Modify: `crates/proxy-tui/src/client.rs`

The proxy-tui client speaks `ureq` (blocking). `wiremock` is async-only — but the existing wiremock-based tests in the workspace run fine because `wiremock::MockServer` exposes a port and `ureq` calls it synchronously. We just need a `#[tokio::test]` to drive the mock.

- [ ] **Step 1: Add `shared` (path dep) and `wiremock` + `tokio` dev-deps to `proxy-tui/Cargo.toml`**

Edit `crates/proxy-tui/Cargo.toml`. Under `[dependencies]`, add:

```toml
shared = { path = "../shared" }
```

Under (or add) `[dev-dependencies]`:

```toml
wiremock = { workspace = true }
tokio    = { workspace = true }
```

(Both are already used elsewhere in the workspace per `cli-router/Cargo.toml`'s workspace deps.)

- [ ] **Step 2: Add `get_usage_summary` to `AdminClient`**

Edit `crates/proxy-tui/src/client.rs`. Add `UsageSummaryResponse` to the `proxy_admin_api` import line. Add this method inside `impl AdminClient`:

```rust
pub fn get_usage_summary(
    &self,
    from_ms: i64,
    to_ms: i64,
) -> Result<UsageSummaryResponse, ClientError> {
    get_json(&format!(
        "{}/admin/usage/summary?from={from_ms}&to={to_ms}",
        self.base_url
    ))
}
```

- [ ] **Step 3: Write a wiremock-backed test**

Append to `crates/proxy-tui/src/client.rs`:

```rust
#[cfg(test)]
mod usage_summary_client_tests {
    use super::*;
    use proxy_admin_api::{DailyUsageRow, ModelUsageRow, UsageSummaryResponse};
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn get_usage_summary_calls_endpoint_and_decodes_response() {
        let server = MockServer::start().await;
        let payload = UsageSummaryResponse {
            from_ms: 100,
            to_ms: 200,
            daily: vec![DailyUsageRow {
                date: "2026-05-03".into(),
                requests: 1,
                input_tokens: 1,
                output_tokens: 1,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
                cost_usd: 0.01,
            }],
            models: vec![ModelUsageRow {
                model: "m".into(),
                provider: "anthropic".into(),
                requests: 1,
                input_tokens: 1,
                output_tokens: 1,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
                cost_usd: 0.01,
            }],
        };

        Mock::given(method("GET"))
            .and(path("/admin/usage/summary"))
            .and(query_param("from", "100"))
            .and(query_param("to", "200"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&payload))
            .mount(&server)
            .await;

        let client = AdminClient::new(server.uri());
        let got = client.get_usage_summary(100, 200).unwrap();
        assert_eq!(got, payload);
    }
}
```

- [ ] **Step 4: Run the test**

Run:
```bash
cargo test -p proxy-tui get_usage_summary_calls_endpoint_and_decodes_response
```
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/proxy-tui/Cargo.toml crates/proxy-tui/src/client.rs
git commit -m "feat(proxy-tui): add usage summary admin client call"
```

---

## Task 9: Add `View::Usage`, `RangePreset`, and `UsagePaneState` to `app.rs`

**Files:**
- Modify: `crates/proxy-tui/src/app.rs`

Pure state shape + a small bit of preset → range math. No rendering or networking yet.

- [ ] **Step 1: Add `View::Usage` to the enum and `ALL_VIEWS`**

In `crates/proxy-tui/src/app.rs`:

```rust
pub enum View {
    Status,
    Providers,
    Routing,
    Requests,
    Usage,
}
```

Update the `label` impl:

```rust
View::Usage => "Usage",
```

Update `ALL_VIEWS`:

```rust
pub const ALL_VIEWS: &[View] =
    &[View::Status, View::Providers, View::Routing, View::Requests, View::Usage];
```

- [ ] **Step 2: Add the `RangePreset` enum and helpers**

Add to `app.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangePreset {
    Today,
    D7,
    D30,
    All,
}

impl RangePreset {
    pub fn label(self) -> &'static str {
        match self {
            RangePreset::Today => "Today",
            RangePreset::D7 => "7d",
            RangePreset::D30 => "30d",
            RangePreset::All => "All",
        }
    }

    /// Resolve the preset to an inclusive `(from_ms, to_ms)` window using the
    /// supplied "now" (epoch ms in local timezone). Splitting `now` out makes
    /// this trivially testable with deterministic times.
    pub fn to_range(self, now_ms: i64, local_midnight_ms: i64) -> (i64, i64) {
        const DAY_MS: i64 = 86_400_000;
        match self {
            RangePreset::Today => (local_midnight_ms, now_ms),
            RangePreset::D7 => (now_ms - 7 * DAY_MS, now_ms),
            RangePreset::D30 => (now_ms - 30 * DAY_MS, now_ms),
            RangePreset::All => (0, now_ms),
        }
    }
}
```

- [ ] **Step 3: Add `UsagePaneState`**

Add to `app.rs`:

```rust
use proxy_admin_api::UsageSummaryResponse; // extend the existing import line instead

#[derive(Debug, Clone, Default)]
pub struct UsagePaneState {
    pub range_preset: Option<RangePreset>, // None until first fetch
    pub summary: Option<UsageSummaryResponse>,
    pub table_offset: usize,
    pub last_error: Option<String>,
    pub loading: bool,
}
```

(Default value for `range_preset` is `None` so we can distinguish "never fetched" from "fetched with All". The first time the user enters the Usage tab, the controller picks `RangePreset::Today` and triggers a fetch.)

- [ ] **Step 4: Hold `UsagePaneState` inside `AppState`**

Find the `AppState` struct (same file) and add a field:

```rust
pub usage: UsagePaneState,
```

Add `usage: UsagePaneState::default()` to wherever `AppState::new()` constructs it.

- [ ] **Step 5: Write tests for preset resolution**

Append to `app.rs`:

```rust
#[cfg(test)]
mod range_preset_tests {
    use super::*;

    #[test]
    fn today_uses_local_midnight_to_now() {
        let now = 1_730_086_400_000;
        let midnight = 1_730_073_600_000;
        assert_eq!(
            RangePreset::Today.to_range(now, midnight),
            (midnight, now)
        );
    }

    #[test]
    fn d7_subtracts_seven_days() {
        let now = 1_730_086_400_000;
        let (from, to) = RangePreset::D7.to_range(now, 0);
        assert_eq!(to, now);
        assert_eq!(now - from, 7 * 86_400_000);
    }

    #[test]
    fn all_starts_at_epoch() {
        let now = 1_730_086_400_000;
        assert_eq!(RangePreset::All.to_range(now, 0), (0, now));
    }
}
```

- [ ] **Step 6: Run the tests**

Run:
```bash
cargo test -p proxy-tui range_preset_tests
```
Expected: PASS.

- [ ] **Step 7: Confirm proxy-tui still builds**

Run:
```bash
cargo build -p proxy-tui
```
Expected: clean (the new field is unused so far — that's fine).

- [ ] **Step 8: Commit**

```bash
git add crates/proxy-tui/src/app.rs
git commit -m "feat(proxy-tui): add Usage view state and range presets"
```

---

## Task 10: Wire the Usage view into the event loop and renderer

**Files:**
- Create: `crates/proxy-tui/src/views/mod.rs`
- Create: `crates/proxy-tui/src/views/usage.rs`
- Modify: `crates/proxy-tui/src/main.rs`
- Modify: `crates/proxy-tui/src/ui.rs`

Targeted split: the new view lives in `views/usage.rs`. Existing tabs stay in `ui.rs`.

- [ ] **Step 1: Create `views/mod.rs`**

```rust
//! Per-view renderers. Keeps `ui.rs` focused on shared chrome (tab bar,
//! status line). New views land here; existing views remain in `ui.rs` until
//! they're touched.

pub mod usage;
```

- [ ] **Step 2: Create `views/usage.rs` with the renderer**

```rust
//! Renderer for the Usage tab.

use crate::app::{RangePreset, UsagePaneState};
use proxy_admin_api::{DailyUsageRow, ModelUsageRow};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};
use shared::adapters::presenters::formatting::{fmt_cost, fmt_num, fmt_num_compact};

pub fn draw(f: &mut Frame<'_>, area: Rect, state: &UsagePaneState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // range tabs
            Constraint::Length(7), // overview card
            Constraint::Min(0),    // model table
            Constraint::Length(1), // help line
        ])
        .split(area);

    draw_range_tabs(f, chunks[0], state);
    draw_overview(f, chunks[1], state);
    draw_model_table(f, chunks[2], state);
    draw_help(f, chunks[3]);
}

fn draw_range_tabs(f: &mut Frame<'_>, area: Rect, state: &UsagePaneState) {
    let active = state.range_preset.unwrap_or(RangePreset::Today);
    let labels = [RangePreset::Today, RangePreset::D7, RangePreset::D30, RangePreset::All]
        .iter()
        .map(|p| {
            let lbl = format!(" {} ", p.label());
            if *p == active {
                format!("[{}]", lbl.trim())
            } else {
                lbl
            }
        })
        .collect::<Vec<_>>()
        .join("  ");
    let para = Paragraph::new(labels).block(Block::default().borders(Borders::BOTTOM));
    f.render_widget(para, area);
}

fn draw_overview(f: &mut Frame<'_>, area: Rect, state: &UsagePaneState) {
    let block = Block::default().borders(Borders::ALL).title(" Overview ");
    if let Some(err) = &state.last_error {
        let p = Paragraph::new(format!("Error: {err}\n(showing last good summary if any)"))
            .block(block);
        f.render_widget(p, area);
        return;
    }
    let Some(s) = &state.summary else {
        let msg = if state.loading { "Loading…" } else { "No data yet — press [r] to fetch." };
        f.render_widget(Paragraph::new(msg).block(block), area);
        return;
    };

    let totals = totals(&s.daily);
    let body = format!(
        "Range:    {} → {}     Requests:  {}\n\
         Input:    {} tokens                Cost: {}\n\
         Output:   {} tokens\n\
         Cache rd: {} tokens   Cache wr: {} tokens",
        first_date(&s.daily).unwrap_or("—"),
        last_date(&s.daily).unwrap_or("—"),
        fmt_num(totals.requests),
        fmt_num_compact(totals.input_tokens),
        fmt_cost(totals.cost_usd),
        fmt_num_compact(totals.output_tokens),
        fmt_num_compact(totals.cache_read_tokens),
        fmt_num_compact(totals.cache_creation_tokens),
    );
    f.render_widget(Paragraph::new(body).block(block), area);
}

fn draw_model_table(f: &mut Frame<'_>, area: Rect, state: &UsagePaneState) {
    let block = Block::default().borders(Borders::ALL).title(" By model ");
    let Some(s) = &state.summary else {
        f.render_widget(block, area);
        return;
    };
    if s.models.is_empty() {
        f.render_widget(
            Paragraph::new("No requests in this range.").block(block),
            area,
        );
        return;
    }

    let header = Row::new(vec![
        Cell::from("Model"),
        Cell::from("Provider"),
        Cell::from("Reqs"),
        Cell::from("Tokens"),
        Cell::from("Cost"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    let rows = s.models.iter().skip(state.table_offset).map(|m| {
        Row::new(vec![
            Cell::from(m.model.clone()),
            Cell::from(m.provider.clone()),
            Cell::from(fmt_num(m.requests)),
            Cell::from(fmt_num_compact(
                m.input_tokens
                    + m.output_tokens
                    + m.cache_read_tokens
                    + m.cache_creation_tokens,
            )),
            Cell::from(fmt_cost(m.cost_usd)),
        ])
    });

    let widths = [
        Constraint::Percentage(35),
        Constraint::Percentage(20),
        Constraint::Length(9),
        Constraint::Length(10),
        Constraint::Length(10),
    ];

    let table = Table::new(rows, widths).header(header).block(block);
    f.render_widget(table, area);
}

fn draw_help(f: &mut Frame<'_>, area: Rect) {
    let p = Paragraph::new(" [1] Today  [2] 7d  [3] 30d  [4] All   [r] Refresh   [↑/↓] Scroll ");
    f.render_widget(p, area);
}

#[derive(Default)]
struct Totals {
    requests: u64,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_creation_tokens: u64,
    cost_usd: f64,
}

fn totals(daily: &[DailyUsageRow]) -> Totals {
    let mut t = Totals::default();
    for d in daily {
        t.requests += d.requests;
        t.input_tokens += d.input_tokens;
        t.output_tokens += d.output_tokens;
        t.cache_read_tokens += d.cache_read_tokens;
        t.cache_creation_tokens += d.cache_creation_tokens;
        t.cost_usd += d.cost_usd;
    }
    t
}

fn first_date(daily: &[DailyUsageRow]) -> Option<&str> {
    daily.first().map(|d| d.date.as_str())
}

fn last_date(daily: &[DailyUsageRow]) -> Option<&str> {
    daily.last().map(|d| d.date.as_str())
}

// `ModelUsageRow` is referenced via `state.summary.models`; the unused-import
// warning would fire if we omitted this when reorganising — keep as-is.
#[allow(unused_imports)]
use proxy_admin_api::ModelUsageRow as _ModelUsageRow;
```

- [ ] **Step 3: Declare `views` and dispatch to it from `ui.rs`**

In `crates/proxy-tui/src/main.rs` (where `mod app; mod client; mod terminal; mod ui;` lives), add:

```rust
mod views;
```

In `crates/proxy-tui/src/ui.rs`, find the `match` on `View` (where each tab decides what to draw). Add the new arm:

```rust
View::Usage => crate::views::usage::draw(f, content_area, &state.usage),
```

(Use the same `content_area`/state-access shape as the existing arms — match the surrounding code.)

- [ ] **Step 4: Wire keys and the on-demand fetch in `main.rs`**

In `crates/proxy-tui/src/main.rs`, locate the `handle_key` function (the same one that handles tab cycling and existing hotkeys). Add Usage-tab key handling. The fetch helper goes near `refresh_all`.

```rust
fn fetch_usage(client: &AdminClient, state: &mut AppState) {
    use chrono::{Local, TimeZone};
    let preset = state.usage.range_preset.unwrap_or(RangePreset::Today);
    state.usage.range_preset = Some(preset);
    state.usage.loading = true;

    let now = Local::now();
    let now_ms = now.timestamp_millis();
    let local_midnight_ms = Local
        .with_ymd_and_hms(now.year(), now.month(), now.day(), 0, 0, 0)
        .single()
        .map(|d| d.timestamp_millis())
        .unwrap_or(now_ms);

    let (from_ms, to_ms) = preset.to_range(now_ms, local_midnight_ms);

    match client.get_usage_summary(from_ms, to_ms) {
        Ok(resp) => {
            state.usage.summary = Some(resp);
            state.usage.last_error = None;
            state.usage.table_offset = 0;
        }
        Err(e) => {
            state.usage.last_error = Some(format!("{e}"));
        }
    }
    state.usage.loading = false;
}
```

Add the imports `use chrono::Datelike;` and `use crate::app::RangePreset;` if they aren't already present (the chrono trait import is needed for `now.year()`, `.month()`, `.day()`).

In `handle_key`, add a branch when `state.view == View::Usage`:

```rust
if state.view == View::Usage {
    match k.code {
        KeyCode::Char('1') => { state.usage.range_preset = Some(RangePreset::Today); fetch_usage(client, state); }
        KeyCode::Char('2') => { state.usage.range_preset = Some(RangePreset::D7);    fetch_usage(client, state); }
        KeyCode::Char('3') => { state.usage.range_preset = Some(RangePreset::D30);   fetch_usage(client, state); }
        KeyCode::Char('4') => { state.usage.range_preset = Some(RangePreset::All);   fetch_usage(client, state); }
        KeyCode::Char('r') => { fetch_usage(client, state); }
        KeyCode::Up => {
            state.usage.table_offset = state.usage.table_offset.saturating_sub(1);
        }
        KeyCode::Down => {
            if let Some(s) = &state.usage.summary {
                if state.usage.table_offset + 1 < s.models.len() {
                    state.usage.table_offset += 1;
                }
            }
        }
        _ => {}
    }
}
```

Place this branch **after** the existing tab-cycling logic (so `Tab` still cycles even when on Usage), but **before** any default fall-through that might consume keys.

Trigger the first fetch when the user enters the Usage tab. After the line that updates `state.view` on Tab/BackTab, add:

```rust
if state.view == View::Usage && state.usage.summary.is_none() && state.usage.last_error.is_none() {
    fetch_usage(client, state);
}
```

- [ ] **Step 5: Build the workspace**

Run:
```bash
cargo build --workspace
```
Expected: clean.

- [ ] **Step 6: Run all tests**

Run:
```bash
cargo test --workspace
cargo clippy --workspace -- -D warnings
```
Expected: green.

- [ ] **Step 7: Smoke test by running the proxy + TUI manually**

Run (in two terminals):
```bash
# terminal 1
cargo run -p proxy

# terminal 2
cargo run -p proxy-tui
```
Tab to "Usage". Press `1`/`2`/`3`/`4`. Confirm the overview and model table populate when there's data, and "No requests in this range." when there isn't. Confirm `r` re-fetches. Confirm `↑`/`↓` scrolls the model table.

If the daemon isn't seeded with any requests, hit it with a real or curl'd request first to populate the log:

```bash
curl http://127.0.0.1:8787/v1/messages -H 'content-type: application/json' \
  -d '{"model":"…","messages":[…]}'
```

(use whatever provider config you have set up locally.)

- [ ] **Step 8: Commit**

```bash
git add crates/proxy-tui/src/main.rs \
        crates/proxy-tui/src/ui.rs \
        crates/proxy-tui/src/views
git commit -m "feat(proxy-tui): render usage dashboard tab"
```

---

## Task 11: Final verification

- [ ] **Step 1: Workspace gates**

Run:
```bash
cargo fmt --check
cargo clippy --workspace -- -D warnings
cargo test --workspace
```
Expected: all green.

- [ ] **Step 2: Confirm spec coverage**

Re-read the spec (`docs/superpowers/specs/2026-05-03-proxy-tui-usage-dashboard-design.md`). Walk each section:

- Endpoint exists: `GET /admin/usage/summary?from&to` ✓ Task 7
- DTO shape matches spec: `UsageSummaryResponse` / `DailyUsageRow` / `ModelUsageRow` ✓ Task 2
- SQL behaviour: `status='completed'`, `BETWEEN`, `localtime` bucketing, `COALESCE` ✓ Task 5
- `ProxyError::BadRequest` on `from > to` ✓ Task 6
- View added to `ALL_VIEWS`, tab cycling picks it up ✓ Task 9–10
- Range presets 1/2/3/4, refresh `r`, scroll ↑/↓ ✓ Task 10
- Auto-refresh limited to operational tabs (Usage stays on-demand) ✓ Task 10 (no timer wired)
- `fmt_num` / `fmt_cost` moved to `shared`, `analysis` re-exports ✓ Task 1
- Empty result renders "No requests in this range." ✓ Task 10 (`draw_model_table`)
- Last-good summary preserved on error ✓ Task 10 (`fetch_usage` doesn't blank `summary` on `Err`)

If any gap is found, add a tightly scoped follow-up task before declaring done.

---

## Notes for the executor

- **`AdminState` field name drift.** The plan assumes `usage_summary` is the field on `AdminState`. If other admin use cases use a different naming convention (e.g. `get_status` vs `status`), match the existing style instead of inventing one.
- **Module location for domain types.** Task 3 says "or a new submodule if `domain/mod.rs` is already split". Run the inspection step before adding code.
- **Don't widen the trait beyond `summarize`.** A `RequestLogReadPort::raw_rows` or any "list everything" method is out of scope per the spec.
- **No async on the new port method.** `RequestLogReadPort` is sync today; keep it that way.
- **Don't auto-poll the Usage tab.** The spec is explicit: on-demand only. Polling Usage every 2s like the operational tabs is a regression on this design.
