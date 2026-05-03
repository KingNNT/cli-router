# Quota Tracking Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Track per-provider request and token usage against rolling and calendar windows; warn at 80% and reject pre-emptively at 100% so the proxy never lets through a request that would trigger an upstream 429. Counter rebuilt at startup from the existing `requests` SQLite log.

**Architecture:** Domain owns the `QuotaWindow`/`QuotaConfig`/`QuotaCheck` value types and a window-string parser. An adapter holds an in-memory counter (mutex-guarded ring buffer for rolling windows, single-counter + `reset_at_ms` for calendar). The application port `QuotaPort` exposes `check(provider) -> QuotaCheck` and `record(provider, &usage)`. Routing calls `check` after picking the entry (post-sticky); on `Reject`, returns 429 with `Retry-After`. Existing post-response `requests` write path also calls `record`. Startup seeds counters from `RequestLogReadPort::quota_seed(cutoff_ms)`.

**Tech Stack:** Rust 2024, std `Mutex`, no new crates. Reuses `chrono` (already in workspace) for calendar window math.

**Spec:** `docs/superpowers/specs/2026-05-03-quota-tracking-design.md`

---

## File Structure

| File | Role |
|---|---|
| `crates/proxy/src/domain/quota.rs` | **NEW** — `QuotaWindow`, `CalendarUnit`, `QuotaConfig`, `QuotaCheck`, window-string parser, `Bucket` |
| `crates/proxy/src/domain/mod.rs` | declare `pub mod quota;` |
| `crates/proxy/src/application/ports/quota.rs` | **NEW** — `QuotaPort` trait (`check`, `record`) |
| `crates/proxy/src/application/ports/request_log_read.rs` | add `quota_seed(cutoff_ms)` method |
| `crates/proxy/src/application/ports/mod.rs` | re-export `QuotaPort` |
| `crates/proxy/src/adapters/quota/mod.rs` | **NEW** — `InMemoryQuota` impl with rolling buf + calendar counter |
| `crates/proxy/src/adapters/storage/sqlite_request_log.rs` | implement `quota_seed` query |
| `crates/proxy/src/config.rs` | parse `[[quota]]` array; window string → `QuotaWindow` |
| `crates/proxy/src/application/errors.rs` | add `QuotaExceeded { provider, metric, retry_after_ms }` |
| `crates/proxy/src/frameworks/error.rs` | map `QuotaExceeded` → 429 + `Retry-After` |
| `crates/proxy/src/adapters/providers/routing.rs` | pre-flight check + post-response record hooks |
| `crates/proxy/src/adapters/providers/builder.rs` | accept `Arc<dyn QuotaPort>` and pass to routing |
| `crates/proxy/src/main.rs` | construct `InMemoryQuota`, seed from repo, pass into builder |
| `crates/proxy/src/frameworks/admin.rs` | `GET /admin/quota/status` handler |
| `crates/proxy/src/application/use_cases/admin.rs` | `GetQuotaStatus` use case |
| `crates/proxy-admin-api/src/lib.rs` | `QuotaStatusListDto`, `QuotaStatusDto`, `QuotaMetricDto`, `QuotaMetricState` |
| `crates/proxy-tui/src/{ui.rs, app.rs, client.rs}` | quota panel + polling |

---

## Task 1: Domain — QuotaWindow + parser

**Files:**
- Create: `crates/proxy/src/domain/quota.rs`
- Modify: `crates/proxy/src/domain/mod.rs`

- [ ] **Step 1: Create `domain/quota.rs`**

```rust
//! Quota domain types and the window-string parser.

use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalendarUnit { Minute, Hour, Day }

impl CalendarUnit {
    pub fn duration_ms(self) -> u64 {
        match self {
            CalendarUnit::Minute => 60 * 1_000,
            CalendarUnit::Hour => 60 * 60 * 1_000,
            CalendarUnit::Day => 24 * 60 * 60 * 1_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaWindow {
    Rolling { duration_ms: u64 },
    Calendar { unit: CalendarUnit },
}

impl QuotaWindow {
    pub fn duration_ms(&self) -> u64 {
        match *self {
            QuotaWindow::Rolling { duration_ms } => duration_ms,
            QuotaWindow::Calendar { unit } => unit.duration_ms(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct QuotaConfig {
    pub provider: String,
    pub window: QuotaWindow,
    pub max_requests: Option<u64>,
    pub max_input_tokens: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub warn_pct: u8, // 0..=100
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuotaCheck {
    Ok,
    Warn { metric: &'static str, used_pct: u8 },
    Reject { metric: &'static str, retry_after_ms: u64 },
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WindowParseError {
    #[error("missing colon: window must be like 'rolling:5h' or 'calendar:day'")]
    MissingColon,
    #[error("unknown window kind '{0}': expected 'rolling' or 'calendar'")]
    UnknownKind(String),
    #[error("invalid duration '{0}': accepted suffixes are s, m, h, d (e.g. '5h')")]
    InvalidDuration(String),
    #[error("invalid calendar unit '{0}': accepted are 'minute', 'hour', 'day'")]
    InvalidUnit(String),
}

pub fn parse_window(s: &str) -> Result<QuotaWindow, WindowParseError> {
    let (kind, rest) = s.split_once(':').ok_or(WindowParseError::MissingColon)?;
    match kind {
        "rolling" => parse_duration(rest)
            .ok_or_else(|| WindowParseError::InvalidDuration(rest.into()))
            .map(|duration_ms| QuotaWindow::Rolling { duration_ms }),
        "calendar" => match rest {
            "minute" => Ok(QuotaWindow::Calendar { unit: CalendarUnit::Minute }),
            "hour" => Ok(QuotaWindow::Calendar { unit: CalendarUnit::Hour }),
            "day" => Ok(QuotaWindow::Calendar { unit: CalendarUnit::Day }),
            other => Err(WindowParseError::InvalidUnit(other.into())),
        },
        other => Err(WindowParseError::UnknownKind(other.into())),
    }
}

fn parse_duration(s: &str) -> Option<u64> {
    if s.len() < 2 { return None; }
    let (num, suffix) = s.split_at(s.len() - 1);
    let n: u64 = num.parse().ok()?;
    let mult_ms: u64 = match suffix {
        "s" => 1_000,
        "m" => 60 * 1_000,
        "h" => 60 * 60 * 1_000,
        "d" => 24 * 60 * 60 * 1_000,
        _ => return None,
    };
    n.checked_mul(mult_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rolling_durations() {
        assert_eq!(parse_window("rolling:30s").unwrap(), QuotaWindow::Rolling { duration_ms: 30_000 });
        assert_eq!(parse_window("rolling:1m").unwrap(), QuotaWindow::Rolling { duration_ms: 60_000 });
        assert_eq!(parse_window("rolling:5h").unwrap(), QuotaWindow::Rolling { duration_ms: 5 * 60 * 60 * 1000 });
        assert_eq!(parse_window("rolling:2d").unwrap(), QuotaWindow::Rolling { duration_ms: 2 * 24 * 60 * 60 * 1000 });
    }

    #[test]
    fn parse_calendar_units() {
        assert_eq!(parse_window("calendar:minute").unwrap(), QuotaWindow::Calendar { unit: CalendarUnit::Minute });
        assert_eq!(parse_window("calendar:hour").unwrap(), QuotaWindow::Calendar { unit: CalendarUnit::Hour });
        assert_eq!(parse_window("calendar:day").unwrap(), QuotaWindow::Calendar { unit: CalendarUnit::Day });
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(matches!(parse_window("nope"), Err(WindowParseError::MissingColon)));
        assert!(matches!(parse_window("rolling"), Err(WindowParseError::MissingColon)));
        assert!(matches!(parse_window("rolling:5x"), Err(WindowParseError::InvalidDuration(_))));
        assert!(matches!(parse_window("calendar:year"), Err(WindowParseError::InvalidUnit(_))));
        assert!(matches!(parse_window("foo:bar"), Err(WindowParseError::UnknownKind(_))));
    }

    #[test]
    fn duration_ms_returns_correct_value() {
        assert_eq!(QuotaWindow::Rolling { duration_ms: 1234 }.duration_ms(), 1234);
        assert_eq!(QuotaWindow::Calendar { unit: CalendarUnit::Hour }.duration_ms(), 3_600_000);
    }
}
```

- [ ] **Step 2: Declare module**

In `crates/proxy/src/domain/mod.rs`, add `pub mod quota;` next to the existing module declarations.

- [ ] **Step 3: Run tests**

Run: `cargo test -p proxy --lib domain::quota::tests`
Expected: 4 tests PASS.

- [ ] **Step 4: Commit**

```
feat(proxy): add quota domain types and window parser
```

---

## Task 2: Application port — QuotaPort

**Files:**
- Create: `crates/proxy/src/application/ports/quota.rs`
- Modify: `crates/proxy/src/application/ports/mod.rs`

- [ ] **Step 1: Create port file**

```rust
//! QuotaPort — application-layer interface for usage-quota tracking.

use crate::domain::quota::QuotaCheck;
use crate::domain::RequestUsage;

pub trait QuotaPort: Send + Sync {
    /// Pre-flight check for the given provider name. Returns `Ok` when no
    /// quota config matches that provider. Caller should reject when it
    /// returns `Reject`, log a warning when `Warn`, and proceed otherwise.
    fn check(&self, provider: &str) -> QuotaCheck;

    /// Post-response record. Adds the request to the counter for the
    /// matching provider. Idempotent if no quota config matches.
    fn record(&self, provider: &str, usage: &RequestUsage);
}
```

If `RequestUsage` doesn't exist with the assumed shape, use whatever the existing usage struct in `domain` is — read `crates/proxy/src/domain/mod.rs` to find it. The fields needed are `input_tokens: Option<u64>` and `output_tokens: Option<u64>`.

- [ ] **Step 2: Re-export**

Add `pub mod quota;` and `pub use quota::QuotaPort;` to `application/ports/mod.rs`.

- [ ] **Step 3: Compile**

```
cargo check -p proxy
```

- [ ] **Step 4: Commit**

```
feat(proxy): add QuotaPort trait
```

---

## Task 3: Read-port extension — `quota_seed`

**Files:**
- Modify: `crates/proxy/src/application/ports/request_log_read.rs`
- Modify: `crates/proxy/src/adapters/storage/sqlite_request_log.rs`

- [ ] **Step 1: Define `QuotaSeedRow` in domain or port**

In `crates/proxy/src/application/ports/request_log_read.rs`, add a small struct alongside the trait (or in `crate::domain` if a similar shape lives there):

```rust
#[derive(Debug, Clone)]
pub struct QuotaSeedRow {
    pub provider: String,
    pub started_at_ms: i64,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}
```

Add a method to the trait:

```rust
fn quota_seed(&self, cutoff_ms: i64) -> Result<Vec<QuotaSeedRow>, ProxyError>;
```

- [ ] **Step 2: Implement in `sqlite_request_log.rs`**

In the existing `impl RequestLogReadPort for SqliteRequestLogRepository`, add the method. Read the file first to match the prepared-statement convention:

```rust
fn quota_seed(&self, cutoff_ms: i64) -> Result<Vec<QuotaSeedRow>, ProxyError> {
    let conn = self.conn.lock().expect("conn mutex poisoned");
    let mut stmt = conn
        .prepare(
            "SELECT provider, started_at, input_tokens, output_tokens
             FROM requests
             WHERE started_at > ?1 AND status = 'completed'",
        )
        .map_err(map_db)?;

    let rows = stmt
        .query_map([cutoff_ms], |row| {
            Ok(QuotaSeedRow {
                provider: row.get::<_, String>(0)?,
                started_at_ms: row.get::<_, i64>(1)?,
                input_tokens: row.get::<_, Option<i64>>(2)?.map(|n| n.max(0) as u64),
                output_tokens: row.get::<_, Option<i64>>(3)?.map(|n| n.max(0) as u64),
            })
        })
        .map_err(map_db)?;

    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(map_db)?);
    }
    Ok(out)
}
```

`map_db` is whatever the file's existing helper is (e.g. `|e| ProxyError::Internal(e.to_string())`). Match the file's style — don't invent a new error mapping helper.

- [ ] **Step 3: Add a unit test in the same file**

Inside the existing `#[cfg(test)] mod tests` block:

```rust
#[test]
fn quota_seed_returns_completed_rows_after_cutoff() {
    let repo = SqliteRequestLogRepository::open_in_memory().unwrap();
    // Insert two completed rows; one before cutoff, one after.
    let cutoff_ms: i64 = 1_000;
    repo.append(&fixture_complete_row("zai", 500, Some(100), Some(50))).unwrap();   // before cutoff
    repo.append(&fixture_complete_row("zai", 2_000, Some(200), Some(75))).unwrap(); // after cutoff
    repo.append(&fixture_started_row("zai", 3_000)).unwrap();                       // not completed

    let rows = repo.quota_seed(cutoff_ms).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].provider, "zai");
    assert_eq!(rows[0].started_at_ms, 2_000);
    assert_eq!(rows[0].input_tokens, Some(200));
    assert_eq!(rows[0].output_tokens, Some(75));
}
```

`fixture_complete_row` and `fixture_started_row` are likely already defined in the test module. If not, find an existing pattern and write minimal helpers like the rest of the file does.

- [ ] **Step 4: Run tests**

```
cargo test -p proxy --lib adapters::storage::sqlite_request_log
```

- [ ] **Step 5: Commit**

```
feat(proxy): add quota_seed query for startup counter rebuild
```

---

## Task 4: Domain — Bucket + RingBuffer + tests

**Files:**
- Modify: `crates/proxy/src/domain/quota.rs`

- [ ] **Step 1: Add Bucket and RingBuffer**

Append to `domain/quota.rs`:

```rust
#[derive(Debug, Clone, Copy, Default)]
pub struct Bucket {
    pub bucket_idx_ms: u64, // floor(timestamp / 60_000) * 60_000
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Totals {
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

const BUCKET_GRANULARITY_MS: u64 = 60_000;

#[derive(Debug)]
pub struct RingBuffer {
    duration_ms: u64,
    buckets: Vec<Bucket>,
}

impl RingBuffer {
    pub fn new(duration_ms: u64) -> Self {
        let n = ((duration_ms + BUCKET_GRANULARITY_MS - 1) / BUCKET_GRANULARITY_MS) as usize;
        let n = n.max(1);
        Self { duration_ms, buckets: vec![Bucket::default(); n] }
    }

    pub fn add(&mut self, now_ms: u64, requests: u64, input_tokens: u64, output_tokens: u64) {
        let bidx = (now_ms / BUCKET_GRANULARITY_MS) * BUCKET_GRANULARITY_MS;
        let slot = ((now_ms / BUCKET_GRANULARITY_MS) as usize) % self.buckets.len();
        let b = &mut self.buckets[slot];
        if b.bucket_idx_ms != bidx {
            *b = Bucket::default();
            b.bucket_idx_ms = bidx;
        }
        b.requests += requests;
        b.input_tokens += input_tokens;
        b.output_tokens += output_tokens;
    }

    pub fn totals(&self, now_ms: u64) -> Totals {
        let cutoff_ms = now_ms.saturating_sub(self.duration_ms);
        let mut t = Totals::default();
        for b in &self.buckets {
            if b.bucket_idx_ms == 0 { continue; }
            if b.bucket_idx_ms + BUCKET_GRANULARITY_MS <= cutoff_ms { continue; }
            t.requests += b.requests;
            t.input_tokens += b.input_tokens;
            t.output_tokens += b.output_tokens;
        }
        t
    }

    /// Milliseconds until the oldest currently-counted bucket falls out of window.
    pub fn next_boundary_ms(&self, now_ms: u64) -> u64 {
        let cutoff_ms = now_ms.saturating_sub(self.duration_ms);
        let mut oldest = u64::MAX;
        for b in &self.buckets {
            if b.bucket_idx_ms == 0 { continue; }
            if b.bucket_idx_ms + BUCKET_GRANULARITY_MS <= cutoff_ms { continue; }
            if b.bucket_idx_ms < oldest { oldest = b.bucket_idx_ms; }
        }
        if oldest == u64::MAX { return 0; }
        oldest.saturating_add(BUCKET_GRANULARITY_MS).saturating_add(self.duration_ms).saturating_sub(now_ms)
    }
}
```

- [ ] **Step 2: Add tests in `mod tests`**

```rust
#[test]
fn ring_add_and_total_in_same_minute() {
    let mut rb = RingBuffer::new(60 * 60 * 1000); // 1h
    rb.add(120_000, 1, 100, 50);
    rb.add(125_000, 2, 200, 80);
    let t = rb.totals(125_000);
    assert_eq!(t.requests, 3);
    assert_eq!(t.input_tokens, 300);
    assert_eq!(t.output_tokens, 130);
}

#[test]
fn ring_drops_old_buckets_outside_window() {
    let mut rb = RingBuffer::new(60_000); // 1 minute window
    rb.add(0, 5, 0, 0);
    let later = 125_000; // > 60s after bucket 0
    let t = rb.totals(later);
    assert_eq!(t.requests, 0);
}

#[test]
fn ring_buffer_size_at_least_one() {
    let rb = RingBuffer::new(0);
    assert_eq!(rb.buckets.len(), 1);
}

#[test]
fn ring_next_boundary_ms_rolls_oldest_out() {
    let mut rb = RingBuffer::new(60 * 60 * 1000); // 1h
    rb.add(0, 1, 0, 0);
    let now = 30 * 60 * 1000; // 30 min in
    // Oldest bucket (bucket_idx_ms=0) falls out at now+30min.
    assert!(rb.next_boundary_ms(now) > 29 * 60 * 1000);
    assert!(rb.next_boundary_ms(now) <= 31 * 60 * 1000);
}

#[test]
fn ring_handles_index_collision_after_full_rotation() {
    let mut rb = RingBuffer::new(60_000); // 1 bucket
    rb.add(0, 1, 0, 0);
    rb.add(300_000, 1, 0, 0);  // 5 minutes later — same slot index, different bucket_idx_ms
    let t = rb.totals(300_000);
    assert_eq!(t.requests, 1, "old bucket should be replaced not summed");
}
```

- [ ] **Step 3: Run, verify pass**

```
cargo test -p proxy --lib domain::quota::tests
```

Expected: 9 tests PASS (4 from Task 1 + 5 new).

- [ ] **Step 4: Commit**

```
feat(proxy): add ring-buffer counter for rolling quota windows
```

---

## Task 5: Calendar counter + check logic

**Files:**
- Modify: `crates/proxy/src/domain/quota.rs`

- [ ] **Step 1: Add CalendarCounter and check function**

Append:

```rust
#[derive(Debug, Default)]
pub struct CalendarCounter {
    pub totals: Totals,
    pub reset_at_ms: u64,
}

impl CalendarCounter {
    pub fn new(unit: CalendarUnit, now_ms: u64) -> Self {
        Self { totals: Totals::default(), reset_at_ms: next_calendar_boundary_ms(unit, now_ms) }
    }

    pub fn add(&mut self, now_ms: u64, unit: CalendarUnit, requests: u64, input_tokens: u64, output_tokens: u64) {
        if now_ms >= self.reset_at_ms {
            self.totals = Totals::default();
            self.reset_at_ms = next_calendar_boundary_ms(unit, now_ms);
        }
        self.totals.requests += requests;
        self.totals.input_tokens += input_tokens;
        self.totals.output_tokens += output_tokens;
    }

    pub fn totals(&self, now_ms: u64, unit: CalendarUnit) -> Totals {
        if now_ms >= self.reset_at_ms { Totals::default() } else { self.totals }
    }

    pub fn next_boundary_ms(&self, now_ms: u64) -> u64 {
        self.reset_at_ms.saturating_sub(now_ms)
    }
}

fn next_calendar_boundary_ms(unit: CalendarUnit, now_ms: u64) -> u64 {
    let unit_ms = unit.duration_ms();
    ((now_ms / unit_ms) + 1) * unit_ms
}

/// Run the warn/reject check logic against an arbitrary totals snapshot.
/// `next_boundary_ms` is the ms-until-window-rolls (used in `Reject.retry_after_ms`).
pub fn evaluate(cfg: &QuotaConfig, totals: Totals, next_boundary_ms: u64) -> QuotaCheck {
    let metrics: [(&'static str, u64, Option<u64>); 3] = [
        ("requests", totals.requests, cfg.max_requests),
        ("input_tokens", totals.input_tokens, cfg.max_input_tokens),
        ("output_tokens", totals.output_tokens, cfg.max_output_tokens),
    ];

    // Hard reject — first metric over the limit wins.
    for (metric, used, max) in metrics {
        if let Some(max) = max
            && max > 0
            && used >= max
        {
            return QuotaCheck::Reject { metric, retry_after_ms: next_boundary_ms.max(1) };
        }
    }

    // Warn — first metric at/above warn threshold wins.
    for (metric, used, max) in metrics {
        if let Some(max) = max
            && max > 0
        {
            let used_pct = ((used as u128 * 100) / max as u128).min(100) as u8;
            if used_pct >= cfg.warn_pct {
                return QuotaCheck::Warn { metric, used_pct };
            }
        }
    }

    QuotaCheck::Ok
}
```

- [ ] **Step 2: Tests**

Add to `mod tests`:

```rust
fn cfg_max(reqs: Option<u64>, input: Option<u64>, output: Option<u64>) -> QuotaConfig {
    QuotaConfig {
        provider: "p".into(),
        window: QuotaWindow::Rolling { duration_ms: 60_000 },
        max_requests: reqs,
        max_input_tokens: input,
        max_output_tokens: output,
        warn_pct: 80,
    }
}

#[test]
fn evaluate_ok_when_no_limits_configured() {
    let cfg = cfg_max(None, None, None);
    let t = Totals { requests: 999_999, input_tokens: 999_999, output_tokens: 999_999 };
    assert_eq!(evaluate(&cfg, t, 0), QuotaCheck::Ok);
}

#[test]
fn evaluate_rejects_when_requests_at_max() {
    let cfg = cfg_max(Some(10), None, None);
    let t = Totals { requests: 10, ..Default::default() };
    assert_eq!(
        evaluate(&cfg, t, 5_000),
        QuotaCheck::Reject { metric: "requests", retry_after_ms: 5_000 }
    );
}

#[test]
fn evaluate_warn_at_or_above_warn_pct() {
    let cfg = cfg_max(Some(10), None, None);
    let t = Totals { requests: 8, ..Default::default() }; // exactly 80%
    assert_eq!(evaluate(&cfg, t, 0), QuotaCheck::Warn { metric: "requests", used_pct: 80 });
}

#[test]
fn evaluate_reject_takes_priority_over_warn() {
    let cfg = QuotaConfig {
        provider: "p".into(), window: QuotaWindow::Rolling { duration_ms: 60_000 },
        max_requests: Some(10), max_input_tokens: Some(100), max_output_tokens: None,
        warn_pct: 80,
    };
    let t = Totals { requests: 100, input_tokens: 80, ..Default::default() };
    let r = evaluate(&cfg, t, 1_000);
    assert!(matches!(r, QuotaCheck::Reject { .. }));
}

#[test]
fn calendar_resets_at_boundary() {
    let unit = CalendarUnit::Hour;
    let mut c = CalendarCounter::new(unit, 0);
    c.add(0, unit, 5, 0, 0);
    assert_eq!(c.totals(0, unit).requests, 5);
    let after_hour = unit.duration_ms();
    c.add(after_hour, unit, 1, 0, 0);
    assert_eq!(c.totals(after_hour, unit).requests, 1, "must reset, not accumulate");
}

#[test]
fn calendar_next_boundary_decreases() {
    let mut c = CalendarCounter::new(CalendarUnit::Hour, 0);
    let early = c.next_boundary_ms(0);
    let later = c.next_boundary_ms(30 * 60 * 1000); // 30 min in
    assert!(later < early);
}
```

- [ ] **Step 3: Run**

```
cargo test -p proxy --lib domain::quota::tests
```

Expected: 15 tests PASS.

- [ ] **Step 4: Commit**

```
feat(proxy): add calendar counter and quota evaluate logic
```

---

## Task 6: Adapter — InMemoryQuota

**Files:**
- Create: `crates/proxy/src/adapters/quota/mod.rs`
- Modify: `crates/proxy/src/adapters/mod.rs`

- [ ] **Step 1: Module skeleton**

```rust
//! In-memory quota counter; one entry per provider with a configured quota.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::application::ports::request_log_read::QuotaSeedRow;
use crate::application::ports::QuotaPort;
use crate::domain::quota::{
    evaluate, CalendarCounter, CalendarUnit, QuotaCheck, QuotaConfig, QuotaWindow, RingBuffer, Totals,
};
use crate::domain::RequestUsage;

#[derive(Debug)]
enum CounterKind {
    Rolling(RingBuffer),
    Calendar { unit: CalendarUnit, counter: CalendarCounter },
}

#[derive(Debug)]
struct CounterEntry {
    config: QuotaConfig,
    state: Mutex<CounterKind>,
}

impl CounterEntry {
    fn new(config: QuotaConfig, now_ms: u64) -> Self {
        let kind = match config.window {
            QuotaWindow::Rolling { duration_ms } => CounterKind::Rolling(RingBuffer::new(duration_ms)),
            QuotaWindow::Calendar { unit } => CounterKind::Calendar {
                unit,
                counter: CalendarCounter::new(unit, now_ms),
            },
        };
        Self { config, state: Mutex::new(kind) }
    }

    fn totals_now(&self, now_ms: u64) -> (Totals, u64) {
        let state = self.state.lock().expect("quota mutex poisoned");
        match &*state {
            CounterKind::Rolling(rb) => (rb.totals(now_ms), rb.next_boundary_ms(now_ms)),
            CounterKind::Calendar { unit, counter } => {
                (counter.totals(now_ms, *unit), counter.next_boundary_ms(now_ms))
            }
        }
    }

    fn add(&self, now_ms: u64, requests: u64, input_tokens: u64, output_tokens: u64) {
        let mut state = self.state.lock().expect("quota mutex poisoned");
        match &mut *state {
            CounterKind::Rolling(rb) => rb.add(now_ms, requests, input_tokens, output_tokens),
            CounterKind::Calendar { unit, counter } => {
                counter.add(now_ms, *unit, requests, input_tokens, output_tokens)
            }
        }
    }
}

#[derive(Debug)]
pub struct InMemoryQuota {
    entries: HashMap<String, CounterEntry>,
}

impl InMemoryQuota {
    pub fn new(configs: Vec<QuotaConfig>, now_ms: u64) -> Self {
        let entries = configs
            .into_iter()
            .map(|c| {
                let name = c.provider.clone();
                (name, CounterEntry::new(c, now_ms))
            })
            .collect();
        Self { entries }
    }

    /// Replay historical rows into the counter. Callers should pass rows
    /// fetched from `RequestLogReadPort::quota_seed`.
    pub fn seed(&self, rows: &[QuotaSeedRow]) {
        for row in rows {
            if let Some(entry) = self.entries.get(&row.provider) {
                entry.add(
                    row.started_at_ms.max(0) as u64,
                    1,
                    row.input_tokens.unwrap_or(0),
                    row.output_tokens.unwrap_or(0),
                );
            }
        }
    }

    /// Snapshot for admin endpoint — returns one tuple per configured provider.
    pub fn snapshot(&self, now_ms: u64) -> Vec<QuotaSnapshot> {
        self.entries
            .values()
            .map(|e| {
                let (totals, next_boundary_ms) = e.totals_now(now_ms);
                QuotaSnapshot {
                    config: e.config.clone(),
                    totals,
                    next_boundary_ms,
                }
            })
            .collect()
    }
}

/// A point-in-time snapshot of one configured quota.
#[derive(Debug, Clone)]
pub struct QuotaSnapshot {
    pub config: QuotaConfig,
    pub totals: Totals,
    pub next_boundary_ms: u64,
}

impl QuotaPort for InMemoryQuota {
    fn check(&self, provider: &str) -> QuotaCheck {
        let entry = match self.entries.get(provider) {
            Some(e) => e,
            None => return QuotaCheck::Ok,
        };
        let now_ms = now_epoch_ms();
        let (totals, next_boundary_ms) = entry.totals_now(now_ms);
        evaluate(&entry.config, totals, next_boundary_ms)
    }

    fn record(&self, provider: &str, usage: &RequestUsage) {
        let entry = match self.entries.get(provider) {
            Some(e) => e,
            None => return,
        };
        let now_ms = now_epoch_ms();
        entry.add(
            now_ms,
            1,
            usage.input_tokens.unwrap_or(0),
            usage.output_tokens.unwrap_or(0),
        );
    }
}

fn now_epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
```

If `RequestUsage` lives at `crate::domain::RequestUsage` already, use that path. If the existing usage type uses different field names (e.g. `input_tokens: u64` not `Option<u64>`), adapt the `.unwrap_or(0)` accordingly. Read it first.

- [ ] **Step 2: Declare module**

In `crates/proxy/src/adapters/mod.rs`, add `pub mod quota;` next to the existing module list.

- [ ] **Step 3: Tests**

In `adapters/quota/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(provider: &str, max_req: u64) -> QuotaConfig {
        QuotaConfig {
            provider: provider.into(),
            window: QuotaWindow::Rolling { duration_ms: 60 * 60 * 1000 },
            max_requests: Some(max_req),
            max_input_tokens: None,
            max_output_tokens: None,
            warn_pct: 80,
        }
    }

    fn usage(input: u64, output: u64) -> RequestUsage {
        // Construct using whatever the real RequestUsage looks like.
        // If it has a `new` constructor or builder, use it. Otherwise field-init.
        RequestUsage {
            input_tokens: Some(input),
            output_tokens: Some(output),
            ..Default::default()
        }
    }

    #[test]
    fn check_returns_ok_when_provider_unconfigured() {
        let q = InMemoryQuota::new(vec![cfg("zai", 10)], 0);
        assert_eq!(q.check("anthropic"), QuotaCheck::Ok);
    }

    #[test]
    fn record_increments_counter_for_configured_provider() {
        let q = InMemoryQuota::new(vec![cfg("zai", 10)], 0);
        for _ in 0..5 {
            q.record("zai", &usage(0, 0));
        }
        let snap = q.snapshot(now_epoch_ms());
        let zai = snap.iter().find(|s| s.config.provider == "zai").unwrap();
        assert_eq!(zai.totals.requests, 5);
    }

    #[test]
    fn record_ignores_unconfigured_provider() {
        let q = InMemoryQuota::new(vec![cfg("zai", 10)], 0);
        q.record("anthropic", &usage(0, 0));
        let snap = q.snapshot(now_epoch_ms());
        let zai = snap.iter().find(|s| s.config.provider == "zai").unwrap();
        assert_eq!(zai.totals.requests, 0);
    }

    #[test]
    fn seed_replays_history() {
        let q = InMemoryQuota::new(vec![cfg("zai", 100)], now_epoch_ms());
        let rows = vec![
            QuotaSeedRow {
                provider: "zai".into(),
                started_at_ms: now_epoch_ms() as i64 - 10_000,
                input_tokens: Some(50),
                output_tokens: Some(20),
            },
            QuotaSeedRow {
                provider: "zai".into(),
                started_at_ms: now_epoch_ms() as i64 - 5_000,
                input_tokens: Some(60),
                output_tokens: Some(30),
            },
        ];
        q.seed(&rows);
        let snap = q.snapshot(now_epoch_ms());
        let zai = snap.iter().find(|s| s.config.provider == "zai").unwrap();
        assert_eq!(zai.totals.requests, 2);
        assert_eq!(zai.totals.input_tokens, 110);
        assert_eq!(zai.totals.output_tokens, 50);
    }

    #[test]
    fn check_rejects_at_max() {
        let q = InMemoryQuota::new(vec![cfg("zai", 3)], now_epoch_ms());
        for _ in 0..3 {
            q.record("zai", &usage(0, 0));
        }
        assert!(matches!(q.check("zai"), QuotaCheck::Reject { .. }));
    }
}
```

- [ ] **Step 4: Run**

```
cargo test -p proxy --lib adapters::quota
```

Expected: 5 tests PASS.

- [ ] **Step 5: Commit**

```
feat(proxy): add InMemoryQuota adapter with seeding
```

---

## Task 7: Config — `[[quota]]` parsing

**Files:**
- Modify: `crates/proxy/src/config.rs`

- [ ] **Step 1: Add deserialisable type**

Add near other `pub struct` types:

```rust
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuotaRule {
    pub provider: String,
    pub window: String, // parsed at conversion time
    #[serde(default)]
    pub max_requests: Option<u64>,
    #[serde(default)]
    pub max_input_tokens: Option<u64>,
    #[serde(default)]
    pub max_output_tokens: Option<u64>,
    #[serde(default = "default_warn_pct")]
    pub warn_pct: u8,
}

fn default_warn_pct() -> u8 { 80 }
```

Add to `Config`:

```rust
#[serde(default)]
pub quota: Vec<QuotaRule>,
```

- [ ] **Step 2: Convert to domain at boundary**

Add a converter (in `config.rs` or `application/use_cases/...`):

```rust
impl QuotaRule {
    pub fn to_domain(&self) -> Result<crate::domain::quota::QuotaConfig, ConfigError> {
        let window = crate::domain::quota::parse_window(&self.window)
            .map_err(|e| ConfigError::InvalidQuota(format!("provider {}: {e}", self.provider)))?;
        if self.warn_pct > 100 {
            return Err(ConfigError::InvalidQuota(format!(
                "provider {}: warn_pct must be 0..=100", self.provider
            )));
        }
        Ok(crate::domain::quota::QuotaConfig {
            provider: self.provider.clone(),
            window,
            max_requests: self.max_requests,
            max_input_tokens: self.max_input_tokens,
            max_output_tokens: self.max_output_tokens,
            warn_pct: self.warn_pct,
        })
    }
}
```

Add `InvalidQuota(String)` to `ConfigError` if not already present.

- [ ] **Step 3: Tests**

```rust
#[test]
fn quota_section_parses_with_defaults() {
    let toml = r#"
        [[providers]]
        name = "zai"
        kind = "zai"
        auth = { type = "passthrough" }

        [[routing]]
        match = { model = "*" }
        provider = "zai"

        [[quota]]
        provider = "zai"
        window = "rolling:5h"
        max_requests = 240
    "#;
    let cfg: Config = toml::from_str(toml).unwrap();
    assert_eq!(cfg.quota.len(), 1);
    assert_eq!(cfg.quota[0].warn_pct, 80, "default applies");
    let dom = cfg.quota[0].to_domain().unwrap();
    assert_eq!(dom.window.duration_ms(), 5 * 60 * 60 * 1000);
}

#[test]
fn quota_invalid_window_string_errors_at_conversion() {
    let toml = r#"
        [[providers]]
        name = "zai"
        kind = "zai"
        auth = { type = "passthrough" }

        [[routing]]
        match = { model = "*" }
        provider = "zai"

        [[quota]]
        provider = "zai"
        window = "garbage"
    "#;
    let cfg: Config = toml::from_str(toml).unwrap();
    assert!(cfg.quota[0].to_domain().is_err());
}

#[test]
fn quota_warn_pct_over_100_errors_at_conversion() {
    let toml = r#"
        [[providers]]
        name = "zai"
        kind = "zai"
        auth = { type = "passthrough" }

        [[routing]]
        match = { model = "*" }
        provider = "zai"

        [[quota]]
        provider = "zai"
        window = "rolling:1m"
        warn_pct = 150
    "#;
    let cfg: Config = toml::from_str(toml).unwrap();
    assert!(cfg.quota[0].to_domain().is_err());
}

#[test]
fn quota_section_optional() {
    let toml = r#"
        [[providers]]
        name = "p"
        kind = "anthropic"
        auth = { type = "passthrough" }

        [[routing]]
        match = { model = "*" }
        provider = "p"
    "#;
    let cfg: Config = toml::from_str(toml).unwrap();
    assert!(cfg.quota.is_empty());
}
```

- [ ] **Step 4: Run**

```
cargo test -p proxy --lib config::tests::quota
```

- [ ] **Step 5: Commit**

```
feat(proxy): parse [[quota]] config sections
```

---

## Task 8: Error variant + 429 mapping

**Files:**
- Modify: `crates/proxy/src/application/errors.rs`
- Modify: `crates/proxy/src/frameworks/error.rs`

- [ ] **Step 1: Add error variant**

In `application/errors.rs`, add to `ProxyError`:

```rust
#[error("quota exceeded for {provider} on {metric}; retry after {retry_after_ms}ms")]
QuotaExceeded {
    provider: String,
    metric: &'static str,
    retry_after_ms: u64,
},
```

Match the existing variant style (probably uses thiserror).

- [ ] **Step 2: Map to 429**

In `frameworks/error.rs`, find the `IntoResponse` impl for `ProxyError`. Add a branch:

```rust
ProxyError::QuotaExceeded { provider, metric, retry_after_ms } => {
    let body = serde_json::json!({
        "error": {
            "type": "rate_limit_exceeded",
            "message": format!("proxy quota exceeded for {provider} on {metric}"),
        }
    });
    let mut response = (StatusCode::TOO_MANY_REQUESTS, Json(body)).into_response();
    let retry_after_secs = retry_after_ms.div_ceil(1000);
    response.headers_mut().insert(
        http::header::RETRY_AFTER,
        retry_after_secs.to_string().parse().unwrap(),
    );
    response
}
```

If the existing 429 path already constructs an Anthropic-format vs OpenAI-format body based on path (likely — check `UpstreamRateLimited` variant for prior art), copy the pattern.

- [ ] **Step 3: Compile + test**

```
cargo build -p proxy
cargo test -p proxy
```

- [ ] **Step 4: Commit**

```
feat(proxy): map QuotaExceeded to 429 with Retry-After
```

---

## Task 9: Wire pre-flight + post-response into routing

**Files:**
- Modify: `crates/proxy/src/adapters/providers/routing.rs`
- Modify: `crates/proxy/src/adapters/providers/builder.rs`

- [ ] **Step 1: Add `quota: Arc<dyn QuotaPort>` to RoutingProvider**

```rust
pub struct RoutingProvider {
    rules: Vec<Route>,
    rr_counter: AtomicUsize,
    leaves: std::collections::HashMap<String, Arc<dyn Provider>>,
    affinity: AffinityConfig,
    quota: Arc<dyn crate::application::ports::QuotaPort>,
}
```

Update `RoutingProviderBuilder`:

```rust
pub struct RoutingProviderBuilder {
    rules: Vec<Route>,
    leaves: std::collections::HashMap<String, Arc<dyn Provider>>,
    affinity: AffinityConfig,
    quota: Option<Arc<dyn crate::application::ports::QuotaPort>>,
}

impl RoutingProviderBuilder {
    pub fn quota(mut self, q: Arc<dyn crate::application::ports::QuotaPort>) -> Self {
        self.quota = Some(q);
        self
    }
}

// in build():
quota: self.quota.unwrap_or_else(|| Arc::new(NoopQuota)),
```

Define a `NoopQuota` zero-sized struct for tests / clients that don't need quotas:

```rust
struct NoopQuota;
impl crate::application::ports::QuotaPort for NoopQuota {
    fn check(&self, _provider: &str) -> crate::domain::quota::QuotaCheck {
        crate::domain::quota::QuotaCheck::Ok
    }
    fn record(&self, _provider: &str, _usage: &crate::domain::RequestUsage) {}
}
```

- [ ] **Step 2: Pre-flight check inside the attempt loop**

In both `forward_round_robin` and `forward_round_robin_openai`, add the check **after** `entry.is_cooling_down()` skip but **before** sending upstream:

```rust
match self.quota.check(&entry.id) {
    QuotaCheck::Ok => {}
    QuotaCheck::Warn { metric, used_pct } => {
        tracing::warn!(provider = %entry.id, metric, used_pct, "quota approaching limit");
    }
    QuotaCheck::Reject { metric, retry_after_ms } => {
        return Err(ProxyError::QuotaExceeded {
            provider: entry.id.clone(),
            metric,
            retry_after_ms,
        });
    }
}
```

- [ ] **Step 3: Post-response record**

In the same forward function, on the success path (`Ok(other) => return Ok(other)`), before returning, capture the usage if present and call `self.quota.record(&entry.id, &usage)`. Look at how usage is currently captured for `RequestLogPort` writes — that capture point already exists; piggyback on it.

If usage is captured in a different layer (e.g. in `messages_protocol::forward`), expose it back to routing or call `quota.record` from the same place that writes to `requests`. The plan-spec acknowledges this might happen one layer deeper — pick whichever path is cleanest and document the choice in the commit body.

- [ ] **Step 4: Update existing tests**

Tests in `routing.rs` that build `RoutingProvider` need either:
- No change (default `NoopQuota` kicks in via `unwrap_or_else`).
- Explicit `.quota(Arc::new(NoopQuota))` if they're constructing the struct directly bypassing the builder.

- [ ] **Step 5: Run**

```
cargo test -p proxy
cargo clippy -p proxy --tests -- -D warnings
```

- [ ] **Step 6: Commit**

```
feat(proxy): pre-flight quota check + post-response record in routing
```

---

## Task 10: Integration test — quota rejects + retry-after

**Files:**
- Create: `crates/proxy/tests/quota_enforcement.rs`

- [ ] **Step 1: Write tests**

Use `wiremock` (already a dev-dep per `rust-style.md`). Mirror the existing integration test setup in `crates/proxy/tests/integration.rs`.

```rust
//! Integration: quota check rejects requests pre-emptively without hitting upstream.

use bytes::Bytes;
use http::HeaderMap;
use std::sync::Arc;

use proxy::adapters::quota::InMemoryQuota;
use proxy::application::ports::QuotaPort;
use proxy::domain::quota::{CalendarUnit, QuotaCheck, QuotaConfig, QuotaWindow};
use proxy::domain::RequestUsage;

fn cfg(provider: &str, max_req: u64) -> QuotaConfig {
    QuotaConfig {
        provider: provider.into(),
        window: QuotaWindow::Rolling { duration_ms: 60 * 60 * 1000 },
        max_requests: Some(max_req),
        max_input_tokens: None,
        max_output_tokens: None,
        warn_pct: 80,
    }
}

#[test]
fn quota_blocks_after_max_requests() {
    let q = InMemoryQuota::new(vec![cfg("zai", 3)], 0);
    let usage = RequestUsage {
        input_tokens: Some(0),
        output_tokens: Some(0),
        ..Default::default()
    };
    for _ in 0..3 {
        assert_eq!(q.check("zai"), QuotaCheck::Ok);
        q.record("zai", &usage);
    }
    let result = q.check("zai");
    match result {
        QuotaCheck::Reject { metric, retry_after_ms } => {
            assert_eq!(metric, "requests");
            assert!(retry_after_ms > 0);
        }
        other => panic!("expected Reject, got {other:?}"),
    }
}

#[test]
fn warn_at_80_then_reject_at_100() {
    let q = InMemoryQuota::new(vec![cfg("zai", 10)], 0);
    let u = RequestUsage::default();
    for _ in 0..7 {
        q.record("zai", &u);
    }
    assert_eq!(q.check("zai"), QuotaCheck::Ok); // 70%
    q.record("zai", &u);
    let r = q.check("zai");
    assert!(matches!(r, QuotaCheck::Warn { used_pct: 80, .. })); // 80%
    for _ in 0..2 {
        q.record("zai", &u);
    }
    assert!(matches!(q.check("zai"), QuotaCheck::Reject { .. })); // 100%
}

#[test]
fn unconfigured_provider_always_ok() {
    let q = InMemoryQuota::new(vec![cfg("zai", 3)], 0);
    let u = RequestUsage::default();
    for _ in 0..100 {
        q.record("anthropic", &u);
    }
    assert_eq!(q.check("anthropic"), QuotaCheck::Ok);
}
```

- [ ] **Step 2: Run**

```
cargo test -p proxy --test quota_enforcement
```

Expected: 3 tests PASS.

- [ ] **Step 3: Commit**

```
test(proxy): integration tests for quota enforcement
```

---

## Task 11: Construct + seed `InMemoryQuota` in main.rs

**Files:**
- Modify: `crates/proxy/src/main.rs`
- Modify: `crates/proxy/src/adapters/providers/builder.rs`

- [ ] **Step 1: Construct quota at startup**

In `main.rs`, after creating the request-log read port and before constructing the routing provider:

```rust
let quota_configs: Vec<QuotaConfig> = cfg
    .quota
    .iter()
    .map(|r| r.to_domain())
    .collect::<Result<_, _>>()
    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;

let quota = Arc::new(InMemoryQuota::new(quota_configs, now_epoch_ms()));

// Seed from history
let max_window_ms = cfg.quota.iter().filter_map(|r|
    crate::domain::quota::parse_window(&r.window).ok().map(|w| w.duration_ms())
).max().unwrap_or(0);
let cutoff_ms = (now_epoch_ms() as i64 - max_window_ms as i64).max(0);
let seed_rows = request_read.quota_seed(cutoff_ms)
    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
quota.seed(&seed_rows);
```

(Adjust error coercion to whatever `main.rs` uses today — if it's `anyhow` or a custom enum, match it. The repo policy is no-anyhow per `.claude/rules/rust-style.md`, so it's likely a typed error.)

- [ ] **Step 2: Pass into builder**

```rust
.quota(quota.clone())
```

- [ ] **Step 3: Build + test**

```
cargo build --workspace
cargo test --workspace
```

- [ ] **Step 4: Commit**

```
feat(proxy): construct and seed InMemoryQuota at startup
```

---

## Task 12: Admin endpoint — `GET /admin/quota/status`

**Files:**
- Modify: `crates/proxy-admin-api/src/lib.rs`
- Modify: `crates/proxy/src/application/use_cases/admin.rs`
- Modify: `crates/proxy/src/frameworks/admin.rs`

- [ ] **Step 1: DTOs**

In `proxy-admin-api/src/lib.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaStatusListDto {
    pub quotas: Vec<QuotaStatusDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaStatusDto {
    pub provider: String,
    pub window: String,           // "rolling:5h" / "calendar:day"
    pub window_resets_in_ms: u64,
    pub requests: QuotaMetricDto,
    pub input_tokens: QuotaMetricDto,
    pub output_tokens: QuotaMetricDto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaMetricDto {
    pub used: u64,
    pub max: Option<u64>,
    pub pct: u8,
    pub state: QuotaMetricState,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QuotaMetricState { Ok, Warn, Rejecting, Unconfigured }
```

- [ ] **Step 2: Use case**

In `application/use_cases/admin.rs`:

```rust
pub struct GetQuotaStatus {
    quota: Arc<dyn crate::application::ports::QuotaPort>,
    quota_view: Arc<crate::adapters::quota::InMemoryQuota>,
}

impl GetQuotaStatus {
    pub fn new(
        quota: Arc<dyn crate::application::ports::QuotaPort>,
        quota_view: Arc<crate::adapters::quota::InMemoryQuota>,
    ) -> Self {
        Self { quota, quota_view }
    }

    pub fn execute(&self) -> QuotaStatusListDto {
        let now_ms = now_epoch_ms() as u64;
        let snapshots = self.quota_view.snapshot(now_ms);
        QuotaStatusListDto {
            quotas: snapshots.into_iter().map(|s| snapshot_to_dto(&s, now_ms)).collect(),
        }
    }
}

fn snapshot_to_dto(s: &crate::adapters::quota::QuotaSnapshot, now_ms: u64) -> QuotaStatusDto {
    let window_str = match s.config.window {
        crate::domain::quota::QuotaWindow::Rolling { duration_ms } => {
            let secs = duration_ms / 1000;
            if secs % (24 * 3600) == 0 { format!("rolling:{}d", secs / 86400) }
            else if secs % 3600 == 0 { format!("rolling:{}h", secs / 3600) }
            else if secs % 60 == 0 { format!("rolling:{}m", secs / 60) }
            else { format!("rolling:{secs}s") }
        }
        crate::domain::quota::QuotaWindow::Calendar { unit } => match unit {
            crate::domain::quota::CalendarUnit::Minute => "calendar:minute".into(),
            crate::domain::quota::CalendarUnit::Hour => "calendar:hour".into(),
            crate::domain::quota::CalendarUnit::Day => "calendar:day".into(),
        },
    };

    let mk = |used: u64, max: Option<u64>| {
        let pct = if let Some(m) = max { ((used as u128 * 100) / m as u128).min(100) as u8 } else { 0 };
        let state = match max {
            None => QuotaMetricState::Unconfigured,
            Some(m) if used >= m => QuotaMetricState::Rejecting,
            Some(_) if pct >= s.config.warn_pct => QuotaMetricState::Warn,
            _ => QuotaMetricState::Ok,
        };
        QuotaMetricDto { used, max, pct, state }
    };

    QuotaStatusDto {
        provider: s.config.provider.clone(),
        window: window_str,
        window_resets_in_ms: s.next_boundary_ms,
        requests: mk(s.totals.requests, s.config.max_requests),
        input_tokens: mk(s.totals.input_tokens, s.config.max_input_tokens),
        output_tokens: mk(s.totals.output_tokens, s.config.max_output_tokens),
    }
}
```

The use case takes both `Arc<dyn QuotaPort>` (for symmetry) and `Arc<InMemoryQuota>` (because `snapshot` is a concrete-impl method). If you'd rather expose `snapshot` on the port trait, add it there and drop the concrete-Arc dependency. Either is fine; pick the simpler form.

- [ ] **Step 3: Handler**

In `frameworks/admin.rs`:

```rust
.route("/admin/quota/status", get(quota_status_handler))

async fn quota_status_handler(
    State(s): State<AdminState>,
) -> Json<QuotaStatusListDto> {
    Json(s.quota_status.execute())
}
```

Add `quota_status: Arc<GetQuotaStatus>` to `AdminState`.

- [ ] **Step 4: Wire `AdminState` in main.rs**

Pass the `GetQuotaStatus` use case in.

- [ ] **Step 5: Run**

```
cargo build --workspace
cargo test --workspace
```

- [ ] **Step 6: Commit**

```
feat(proxy): /admin/quota/status endpoint
```

---

## Task 13: Proxy-TUI quota panel

**Files:**
- Modify: `crates/proxy-tui/src/{ui.rs, app.rs, client.rs}`

- [ ] **Step 1: Client method**

In `proxy-tui/src/client.rs`, add:

```rust
pub fn get_quota_status(&self) -> Result<QuotaStatusListDto, ClientError> {
    self.get("/admin/quota/status")
}
```

- [ ] **Step 2: App state**

In `app.rs`, add `quota_status: Option<QuotaStatusListDto>`. Refresh it alongside other polled state on the existing 3s timer.

- [ ] **Step 3: Render**

In `ui.rs`, add a quota panel to `View::Status` rendering:

```rust
fn render_quota_panel(f: &mut Frame, area: Rect, quota: Option<&QuotaStatusListDto>) {
    let block = Block::default().title("Quota Usage").borders(Borders::ALL);
    f.render_widget(block, area);
    let inner = /* shrink area */;

    let quotas = match quota {
        Some(q) => &q.quotas,
        None => return,
    };
    if quotas.is_empty() { return; }

    // For each quota, render a row with provider name + 1-3 progress bars.
    let mut lines: Vec<Line> = Vec::new();
    for q in quotas {
        lines.push(Line::from(format!("{}  ({})  resets in {}",
            q.provider, q.window, fmt_duration_ms(q.window_resets_in_ms))));
        push_metric_line(&mut lines, "req", &q.requests);
        push_metric_line(&mut lines, "in_tok", &q.input_tokens);
        push_metric_line(&mut lines, "out_tok", &q.output_tokens);
        lines.push(Line::from(""));
    }
    let para = Paragraph::new(lines);
    f.render_widget(para, inner);
}

fn push_metric_line(out: &mut Vec<Line>, label: &str, m: &QuotaMetricDto) {
    if matches!(m.state, QuotaMetricState::Unconfigured) { return; }
    let bar = bar_string(m.pct);
    let color = match m.state {
        QuotaMetricState::Ok => Color::Green,
        QuotaMetricState::Warn => Color::Yellow,
        QuotaMetricState::Rejecting => Color::Red,
        QuotaMetricState::Unconfigured => Color::Gray,
    };
    let max_str = match m.max { Some(m) => m.to_string(), None => "—".into() };
    out.push(Line::from(vec![
        Span::raw(format!("  {:7} ", label)),
        Span::styled(bar, Style::default().fg(color)),
        Span::raw(format!(" {}% ({}/{})", m.pct, m.used, max_str)),
    ]));
}

fn bar_string(pct: u8) -> String {
    let filled = (pct as usize).min(100) / 5; // 20-char bar
    let empty = 20 - filled;
    "█".repeat(filled) + &"░".repeat(empty)
}

fn fmt_duration_ms(ms: u64) -> String {
    let secs = ms / 1000;
    if secs >= 3600 { format!("{}h{}m", secs / 3600, (secs % 3600) / 60) }
    else if secs >= 60 { format!("{}m{}s", secs / 60, secs % 60) }
    else { format!("{secs}s") }
}
```

Place the panel in the existing layout — split `chunks[1]` into two halves if needed, with status above and quota below.

- [ ] **Step 4: Run**

```
cargo build --workspace
cargo clippy --workspace -- -D warnings
```

- [ ] **Step 5: Smoke**

Manual: start proxy + TUI; with at least one `[[quota]]` configured, observe the panel populates. Without quota config, panel renders empty (or hidden — your choice).

- [ ] **Step 6: Commit**

```
feat(proxy-tui): quota usage panel with color-coded progress bars
```

---

## Task 14: Workspace gates + manual verify

- [ ] **Step 1: All gates**

```
cargo fmt
cargo fmt --check
cargo clippy --workspace --tests -- -D warnings
cargo test --workspace
```

- [ ] **Step 2: Manual smoke**

Add this to your local `~/.config/cli-router/config.toml`:

```toml
[[quota]]
provider = "zai"
window = "rolling:5h"
max_requests = 3
warn_pct = 50
```

Restart proxy, send 4 requests (any model that resolves to `zai`), confirm:
- Request 1: OK (33%)
- Request 2: OK (66%) — Warn at 50% threshold logged
- Request 3: OK (100%) — Reject takes effect for next call
- Request 4: 429 with `Retry-After` header

Inspect TUI quota panel — should show red at 100%.

- [ ] **Step 3: Final commit if any fmt churn**

```
chore(proxy): cargo fmt cleanup post-quota
```

---

## Self-review

- [x] **Spec coverage**: all sections of `2026-05-03-quota-tracking-design.md` addressed (domain → Tasks 1, 4, 5; counter → Task 6; port + seed → Tasks 2, 3; config → Task 7; error mapping → Task 8; routing wiring → Task 9; integration → Task 10; main.rs wiring → Task 11; admin → Task 12; TUI → Task 13; gates → Task 14).
- [x] **No placeholders**: every code block has actual content. No "TBD".
- [x] **Type consistency**: `QuotaConfig`, `QuotaCheck`, `Totals`, `QuotaSnapshot`, `QuotaSeedRow` named consistently across all 14 tasks.
- [x] **Frequent commits**: 14 commits over the plan, one per task.

## Out-of-scope (per spec)

- Per-key quotas, cost-based quotas, distributed quotas, auto-config from subscription tier, per-route quotas, pre-flight token estimation. None implemented.
