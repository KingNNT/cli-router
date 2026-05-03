# Quota Tracking Design Spec

## Goal

Track per-provider request and token usage against configurable rolling and calendar windows, warn at a soft threshold, and pre-emptively reject at a hard threshold so the proxy never lets a request through that would trigger an upstream 429. Counter is rebuilt at startup from the existing `requests` SQLite log so quotas are accurate after restart.

## Why

Z.ai Coding Plan and similar subscription tiers enforce rolling-window quotas (e.g., Lite tier ~120 prompts per 5 hours). Hitting upstream 429 mid-conversation is disruptive — Claude Code/OpenCode show ugly errors, retries hammer the same key. Pre-flight rejection at the proxy with a correct `Retry-After` lets the client back off cleanly.

Anthropic API key has TPM/RPM rate limits; same protection useful there.

## Counter unit

Track three independent metrics per provider:

- `requests` — count of requests sent upstream (counted on success or failure, both consume quota at upstream).
- `input_tokens` — sum of `input_tokens` from upstream usage response.
- `output_tokens` — sum of `output_tokens` from upstream usage response.

Quota config sets `Option<u64>` max for each — `None` means that metric is not enforced.

## Window types

```rust
pub enum QuotaWindow {
    Rolling { duration_ms: u64 },        // "rolling:5h", "rolling:1m", "rolling:24h"
    Calendar { unit: CalendarUnit },     // "calendar:day", "calendar:hour", "calendar:minute"
}

pub enum CalendarUnit { Minute, Hour, Day }
```

**Rolling**: ring buffer of N buckets at 1-minute granularity. `N = ceil(duration_ms / 60_000)`. Bucket index = `(now_ms / 60_000) % N`. `total = Σ buckets where bucket_age_ms ≤ duration_ms`. Memory: `N * 24 bytes` per provider (3× u64 per bucket). 5h window = 300 buckets ≈ 7 KB.

**Calendar**: single counter + `reset_at_ms`. On `record(...)`, if `now_ms ≥ reset_at_ms`: zero counter, recompute next boundary. Boundary is in UTC (`floor(now / unit) + unit`).

## Counter implementation

```rust
pub struct QuotaCounter {
    config: QuotaConfig,
    state: Mutex<QuotaState>,           // small enough; sync mutex is fine
}

enum QuotaState {
    Rolling(RingBuffer),                // Vec<Bucket> + last_bucket_idx
    Calendar { counter: Counter, reset_at_ms: u64 },
}

struct Bucket { requests: u64, input_tokens: u64, output_tokens: u64, bucket_idx_ms: u64 }
struct Counter { requests: u64, input_tokens: u64, output_tokens: u64 }
```

`Mutex` not `RwLock`: every record + check is a write. Critical section is microseconds; contention only if QPS in the thousands per provider, which is well above target.

## Pre-flight check

```rust
pub enum QuotaCheck {
    Ok,
    Warn { metric: &'static str, used_pct: u8 },
    Reject { metric: &'static str, retry_after_ms: u64 },
}

fn check(&self, provider: &str) -> QuotaCheck {
    let counter = match self.counters.get(provider) { Some(c) => c, None => return QuotaCheck::Ok };
    let totals = counter.totals_now();

    // Hard reject (any metric ≥ max)
    for (metric, used, max) in [
        ("requests", totals.requests, counter.config.max_requests),
        ("input_tokens", totals.input_tokens, counter.config.max_input_tokens),
        ("output_tokens", totals.output_tokens, counter.config.max_output_tokens),
    ] {
        if let Some(max) = max && used >= max {
            return QuotaCheck::Reject {
                metric,
                retry_after_ms: counter.next_window_boundary_in_ms(),
            };
        }
    }

    // Warn (any metric ≥ max * warn_pct / 100)
    for (metric, used, max) in [...] {
        if let Some(max) = max {
            let used_pct = ((used as u128 * 100) / max as u128) as u8;
            if used_pct >= counter.config.warn_pct {
                return QuotaCheck::Warn { metric, used_pct };
            }
        }
    }

    QuotaCheck::Ok
}
```

`next_window_boundary_in_ms` for rolling = time until oldest bucket falls out (`now - oldest_bucket_idx_ms - duration_ms`); for calendar = `reset_at_ms - now`.

## Post-response record

After successful upstream response (in the same place that today writes `requests` table row):

```rust
fn record(&self, provider: &str, usage: &RequestUsage) {
    if let Some(counter) = self.counters.get(provider) {
        counter.add(usage);
    }
}
```

Counter updates are post-response, so a request that pushes counter past `max` overshoots by 1 — accepted trade-off. User configures `max_*` slightly under upstream's hard limit to give buffer.

## Startup seeding

On `QuotaService::new(config, repo)`:

```rust
let max_window_ms = config.quotas.iter()
    .filter_map(|q| match q.window {
        QuotaWindow::Rolling { duration_ms } => Some(duration_ms),
        QuotaWindow::Calendar { unit } => Some(unit.duration_ms()),
    })
    .max()
    .unwrap_or(0);

let cutoff_ms = now_ms.saturating_sub(max_window_ms);

let rows = repo.query_completed_since(cutoff_ms);
for row in rows {
    if let Some(counter) = self.counters.get(&row.provider) {
        counter.add_historical(row.started_at, row.input_tokens, row.output_tokens);
    }
}
```

Result: counter is accurate within `max_window` of restart. No new tables, no new writes.

## Config

Add `[[quota]]` table array to `config.toml`:

```toml
[[quota]]
provider = "zai"
window = "rolling:5h"
max_requests = 240
max_input_tokens = 5_000_000
warn_pct = 80           # default 80; 0..=100

[[quota]]
provider = "anthropic"
window = "rolling:1m"
max_requests = 50
warn_pct = 75
```

Window string parser: `"rolling:<duration>"` or `"calendar:<unit>"`. Duration accepts `s`, `m`, `h`, `d` suffix. Unit one of `minute|hour|day`.

Provider with no `[[quota]]` block → not enforced (backward compatible).

## Wire-up

**Pre-flight** lives in `RoutingProvider::forward_*` after `pick_sticky_entry` selected an entry, before sending upstream:

```rust
match self.quota.check(&entry.id) {
    QuotaCheck::Ok => {},
    QuotaCheck::Warn { metric, used_pct } => {
        warn!(provider=%entry.id, metric, used_pct, "quota approaching limit");
        // tracing event; TUI status will reflect on next poll
    },
    QuotaCheck::Reject { metric, retry_after_ms } => {
        return Err(quota_429(entry.id.clone(), metric, retry_after_ms));
    },
}
```

`quota_429` constructs `ProxyError::QuotaExceeded` → handler maps to HTTP 429 + `Retry-After: <secs>` header + JSON body matching upstream 429 shape (Anthropic-format on `/v1/messages`, OpenAI-format on `/v1/chat/completions`).

**Post-response** in same path that today writes `requests`:

```rust
self.quota.record(&entry.id, &usage);
```

## Admin API

New endpoint `GET /admin/quota/status`:

```rust
pub struct QuotaStatusListDto {
    pub quotas: Vec<QuotaStatusDto>,
}

pub struct QuotaStatusDto {
    pub provider: String,
    pub window: String,                 // "rolling:5h"
    pub window_resets_in_ms: u64,
    pub requests: QuotaMetricDto,
    pub input_tokens: QuotaMetricDto,
    pub output_tokens: QuotaMetricDto,
}

pub struct QuotaMetricDto {
    pub used: u64,
    pub max: Option<u64>,
    pub pct: u8,                        // 0 if max is None
    pub state: QuotaMetricState,        // Ok | Warn | Rejecting | Unconfigured
}
```

DTOs added to `crates/proxy-admin-api/src/lib.rs`.

## TUI surface

New "Quota Usage" panel in proxy-tui (after status, before recent requests):

```
Quota Usage
zai     ████████░░░░ 65% (156/240 req)        rolling 5h, resets 1h22m
        ██░░░░░░░░░░ 18% (920k/5M in_tok)
anthropic ░░░░░░░░░░ 8%  (4/50 req)           rolling 1m, resets 38s
```

- Color: green `< warn_pct`, yellow `warn_pct .. 99`, red `≥ 100`.
- Poll `/admin/quota/status` every 3 seconds while panel is visible.
- If no quota configured for any provider → panel hidden.

Files: `crates/proxy-tui/src/{ui.rs, app.rs, client.rs}`.

## Error type

```rust
// crates/proxy/src/application/errors.rs
pub enum ApplicationError {
    ...,
    QuotaExceeded { provider: String, metric: &'static str, retry_after_ms: u64 },
}
```

Mapped in `frameworks/error.rs` to HTTP 429 with `Retry-After` header.

## Files to change

| File | Change |
|------|--------|
| `crates/proxy/src/domain/quota.rs` | **NEW** — `QuotaWindow`, `QuotaConfig`, `QuotaCheck`, `CalendarUnit` |
| `crates/proxy/src/application/quota.rs` | **NEW** — `QuotaPort` trait, `QuotaService` |
| `crates/proxy/src/adapters/quota/mod.rs` | **NEW** — `InMemoryQuota`, ring buffer, mutex state |
| `crates/proxy/src/adapters/quota/seed.rs` | **NEW** — startup seeding from `requests` table |
| `crates/proxy/src/adapters/storage/mod.rs` | Add `query_completed_since(cutoff_ms)` query helper |
| `crates/proxy/src/config.rs` | Parse `[[quota]]` array; window/duration string parsing |
| `crates/proxy/src/adapters/providers/routing.rs` | Pre-flight check + post-response record hooks |
| `crates/proxy/src/adapters/providers/builder.rs` | Wire `QuotaService` into routing |
| `crates/proxy/src/application/errors.rs` | Add `QuotaExceeded` variant |
| `crates/proxy/src/frameworks/error.rs` | Map `QuotaExceeded` → 429 + `Retry-After` |
| `crates/proxy/src/frameworks/admin.rs` | `GET /admin/quota/status` |
| `crates/proxy/src/main.rs` | Construct `QuotaService` at startup, seed from repo |
| `crates/proxy-admin-api/src/lib.rs` | `QuotaStatusListDto`, `QuotaStatusDto`, `QuotaMetricDto`, `QuotaMetricState` |
| `crates/proxy-tui/src/{ui.rs, app.rs, client.rs}` | Quota panel rendering + polling |

## Tests

**Domain (`quota.rs`):**
- `parse_window_rolling_duration_units`
- `parse_window_calendar_units`
- `parse_window_rejects_garbage`
- `bucket_rotation_drops_old_entries`
- `bucket_total_sums_only_within_window`
- `calendar_resets_at_boundary_utc`
- `check_returns_ok_when_no_metric_set`
- `check_returns_warn_at_threshold`
- `check_returns_reject_at_max`
- `check_uses_lowest_metric_priority` (highest severity wins: Reject > Warn > Ok)

**Adapter (`quota/mod.rs`):**
- `seed_from_repo_replays_history_into_buckets`
- `seed_ignores_rows_outside_window`
- `record_after_seed_continues_correctly`
- `concurrent_record_is_safe` (spawn 100 tasks)

**Integration (`routing.rs`):**
- `quota_reject_blocks_request_before_upstream` (use wiremock; verify upstream hit count)
- `quota_warn_does_not_block` (verify upstream still called)
- `retry_after_header_present_on_quota_reject`

## What stays the same

- `requests` SQLite schema (no new columns).
- Routing/sticky-auth selection — quota check happens after pick.
- Hot reload of config — quota service rebuilds counters; existing buckets reset (acceptable: after reload, seed re-runs).
- All non-quota provider behavior.

## Out of scope (YAGNI)

- Per-key quotas (only per-provider).
- Cost-based quota (`max_cost_usd`).
- Distributed quota across multiple proxy instances.
- Auto-config from subscription tier inference.
- Pre-flight token estimation (we record post-response only; pre-flight checks current totals against max).
- Quota per route/glob match.

## Risk and rollout

- Backward compatible: no `[[quota]]` block → no enforcement.
- Reject mode is an addition, not a behavior change for existing flows.
- After restart, seeded counters may briefly under-count requests in flight at restart moment — drifts back to accurate within minutes.
- No SQLite migration; no destructive operations.
