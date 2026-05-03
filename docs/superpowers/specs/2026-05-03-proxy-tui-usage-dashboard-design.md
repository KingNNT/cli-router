# Proxy-TUI Usage Dashboard

Date: 2026-05-03

## Summary

Add a usage dashboard to `proxy-tui` that surfaces what's already being logged
to the proxy's request-log SQLite. The TUI gains one new top-level tab —
**Usage** — with a daily-totals overview and a per-model breakdown for the
selected time range. Existing `Status` / `Providers` / `Routing` / `Requests`
tabs already cover the live operational view, so this spec focuses on the
analytics half.

## Motivation

The proxy already writes one row per request to `requests` in its local SQLite
(provider, model, started_at, tokens, `cost_usd`). Today there is no UI for
that data — a user has to query the DB by hand or run the unrelated `analysis`
crate (which reads OpenCode/Claude Code data, not the proxy's log). Surfacing
the proxy's own usage in `proxy-tui` is the natural home and reuses the
existing admin API plumbing.

## Approach: New admin endpoint + new TUI tab

The proxy gains one new admin endpoint that returns server-aggregated daily
totals + per-model breakdown for an arbitrary date range. `proxy-tui` gains
one new tab that calls it with preset ranges (Today / 7d / 30d / All) and
renders the result.

## Architecture

```
proxy-tui                          proxy daemon
─────────                          ─────────
Status   ─────────────────────►    GET /admin/status              (exists)
Providers ────────────────────►    GET /admin/config              (exists)
Routing                            PUT /admin/config              (exists)
Requests ─────────────────────►    GET /admin/requests/recent     (exists)
Usage    ─────────────────────►    GET /admin/usage/summary       (NEW)
                                       ?from=<ms>&to=<ms>
                                                ▼
                                       SqliteRequestLogRepository
                                       (server-side GROUP BY)
                                                ▼
                                       requests table (existing)
```

Server-side aggregation: the TUI never sees raw rows for analytics. Payload is
small regardless of range.

## Endpoint

`GET /admin/usage/summary?from=<epoch_ms>&to=<epoch_ms>` → 200 JSON.

Both `from` and `to` are required, inclusive, in epoch milliseconds. `from > to`
is a 400 Bad Request.

### Response shape (DTOs in `proxy-admin-api`)

```rust
pub struct UsageSummaryResponse {
    pub from_ms: i64,
    pub to_ms: i64,
    pub daily: Vec<DailyUsageRow>,    // sorted by date asc
    pub models: Vec<ModelUsageRow>,   // sorted by cost_usd desc
}

pub struct DailyUsageRow {
    pub date: String,                 // "YYYY-MM-DD" in local tz
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cost_usd: f64,
}

pub struct ModelUsageRow {
    pub model: String,
    pub provider: String,             // value from `requests.provider`, e.g.
                                      // "anthropic", "zai", or "router" — see note below
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cost_usd: f64,
}
```

Empty result (valid range, no rows): 200 with empty `daily` and `models`. The
TUI renders "No requests in this range."

## SQL behavior

The aggregation runs in `SqliteRequestLogRepository::summarize(from_ms, to_ms)`.

- Filters to `status = 'completed'`. `started` rows are in-flight; `errored`
  rows have NULL tokens. Including either skews totals.
- Range filter: `started_at BETWEEN :from AND :to`, inclusive.
- Date bucketing: `date(started_at / 1000, 'unixepoch', 'localtime')`. Local
  timezone matches what the user already sees in the `analysis` crate.
- All `SUM(...)` wrapped in `COALESCE(..., 0)` for integers, `COALESCE(..., 0.0)`
  for `cost_usd`. Old rows may have NULL token columns; `SUM` of all-NULL groups
  is NULL in SQLite.
- Two queries: one `GROUP BY date(...)` for `daily`, one `GROUP BY model, provider`
  for `models`. Both run in a single `prepare`-once-per-call block.

### Note on the `provider` column

`requests.provider` reflects what the proxy wrote at log-time, which is
currently a mix of upstream types (`"anthropic"`, `"zai"`) and the routing
sentinel `"router"` (per the `StatusResponse` comment in `proxy-admin-api`).
The summary endpoint surfaces these values as-is. Normalising routing
sentinels to the resolved upstream is a separate concern — out of scope here.

## Internal port and use case (proxy ring)

Extend the existing `RequestLogPort` (single trait, write + read together).
The existing methods are sync (`fn insert_started`, `fn complete`, `fn fail`)
and return `Result<(), ProxyError>` — the new method follows that pattern:

```rust
fn summarize(
    &self,
    from_ms: i64,
    to_ms: i64,
) -> Result<UsageSummary, ProxyError>;
```

`UsageSummary` is a domain-side struct with the same fields as the DTO. The
admin handler maps `UsageSummary` → `UsageSummaryResponse`.

New use case: `GetUsageSummary` in `crates/proxy/src/application/use_cases/`,
following the same pattern as `GetStatus`, `GetConfig`, etc. Validates
`from <= to` (returns `ProxyError::BadRequest("from must be <= to")` if not)
and delegates to the port.

## TUI changes (`proxy-tui`)

### `app.rs`

- Append `Usage` to `View` enum and `ALL_VIEWS` so existing `Tab` cycling picks
  it up automatically.
- Add `UsagePaneState`:

```rust
pub struct UsagePaneState {
    pub range_preset: RangePreset,        // Today | D7 | D30 | All
    pub summary: Option<UsageSummaryResponse>,
    pub table_offset: usize,              // model table scroll
    pub last_error: Option<String>,
}

pub enum RangePreset { Today, D7, D30, All }
```

- Preset → `(from_ms, to_ms)` is computed in `app.rs` against the system clock
  (start of local day for `Today`, `now - N*86_400_000` for D7/D30, `0` for All).

### `client.rs`

Add `get_usage_summary(from_ms, to_ms) -> Result<UsageSummaryResponse, ClientError>`.

### Rendering

`ui.rs` is currently the single rendering file. Use this addition to start the
overdue split: keep tab bar / status line in `ui.rs`, move per-view renderers
to `views/{status,providers,routing,requests,usage}.rs`. Targeted improvement,
not a wholesale refactor — the empty `views/` directory already exists.

### Layout (Usage tab)

```
┌─ Usage ───────────────────────── [Today] [7d] [30d] [All] ─┐
│ ╭─ Overview ─────────────────────────────────────────────╮ │
│ │ Range:    2026-04-26 → 2026-05-03     Requests:  4,217 │ │
│ │ Input:    12.4M tokens                Cost: $48.92     │ │
│ │ Output:   1.8M tokens                                  │ │
│ │ Cache rd: 92.1M tokens   Cache wr: 0.7M tokens         │ │
│ ╰────────────────────────────────────────────────────────╯ │
│ ╭─ By model ─────────────────────────────────────────────╮ │
│ │ Model                Provider   Reqs    Tokens    Cost │ │
│ │ claude-opus-4-5      anthropic  1,832   58.2M  $32.10  │ │
│ │ claude-sonnet-4-6    anthropic  1,901   34.8M  $12.44  │ │
│ │ glm-4.6              zai          484    13.4M  $4.38  │ │
│ ╰────────────────────────────────────────────────────────╯ │
│  [1] Today  [2] 7d  [3] 30d  [4] All   [r] Refresh         │
└────────────────────────────────────────────────────────────┘
```

### Keys (Usage tab only)

- `1` / `2` / `3` / `4` — switch range preset (re-fetch)
- `r` — refresh current range
- `↑` / `↓` — scroll model table when rows exceed pane height
- `Tab` — next view (already wired)

### Refresh model

- Operational tabs (`Status`, `Requests`) keep existing auto-refresh (~2s).
- Usage tab is **on-demand**: fetched once on first entry to the tab, refetched
  only on preset change or `r`. Daily/model summaries don't change second-to-second
  so polling them is wasted work.

## Formatting helpers

`fmt_num` / `fmt_cost` currently live in
`crates/analysis/src/adapters/presenters/formatting.rs`. Both `analysis` and
`proxy-tui` need them. Move to `crates/shared/src/adapters/presenters/formatting.rs`
(new module) and re-export. The existing tests move with the code. One-time
relocation, no behavior change.

This crosses a ring boundary in the right direction — `shared::adapters` is
where cross-app adapter helpers belong per the workspace layout.

## Error handling

| Case | Handling |
|---|---|
| `from > to` | 400 Bad Request — `ProxyError::BadRequest("from must be <= to")` |
| Missing or malformed query params | 400 — axum `Query<UsageSummaryQuery>` extractor rejects automatically |
| SQLite error | 500 — `ProxyError::Storage(rusqlite::Error)` via existing `#[from]` |
| Empty result (valid range, no rows) | 200 with empty arrays; TUI shows "No requests in this range." |
| TUI client → daemon network error | Show inline error banner above the Usage panes; keep last good summary cached so the screen doesn't blank |

## Testing

1. **`proxy` adapter tests** (`sqlite_request_log.rs`):
   - empty DB → empty `daily` and `models`
   - multi-day rows → correct daily bucketing in local tz
   - rows outside `from..=to` → excluded
   - `status = 'started'` and `status = 'errored'` rows → excluded
   - NULL token columns → `COALESCE` returns 0, no panic
   - same-day multiple models → correctly grouped in `models`, summed in `daily`
   - `models` sorted by `cost_usd` desc, `daily` sorted by date asc

2. **`proxy` use case test** (`get_usage_summary.rs`):
   - happy path with fake repo
   - `from > to` → `ProxyError::BadRequest`

3. **`proxy` integration test** (new `crates/proxy/tests/usage_summary_api.rs`):
   - boot admin router with in-memory SQLite seeded with `requests` rows, hit
     `GET /admin/usage/summary?from=…&to=…`, assert JSON shape + values

4. **`proxy-admin-api`**: serde round-trip test on `UsageSummaryResponse` (the
   wire contract).

5. **`proxy-tui` client test** (`client.rs`): wiremock stub for the new
   endpoint, assert query params and deserialisation.

6. **`proxy-tui` state test** (`app.rs`): preset transitions recompute
   `from`/`to` correctly; error states preserve last-good summary.

7. **`shared`**: existing `analysis` formatting tests move with the code, no new
   coverage needed.

No new dev-deps. `wiremock` is already used in the workspace.

## Out of scope (v1)

- Custom date pickers in the TUI (API supports it, UI doesn't expose it).
- Provider-only breakdown (separate from model breakdown).
- Per-`api_key` breakdown — only the seeded `local` user exists today.
- Charts / sparklines for the daily series.
- Export (CSV / JSON dump).

These are easy to layer on once the foundation is in place.
