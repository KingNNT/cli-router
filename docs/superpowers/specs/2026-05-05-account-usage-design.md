# Account Usage — Upstream Provider Quota Display

Date: 2026-05-05

## Summary

Add a new **Account** tab to `proxy-tui` that shows live upstream provider
account-level quota and usage data (plan tier, quota windows with reset
timers, model usage stats, MCP tool breakdowns). The proxy daemon fetches
from each provider's usage API and exposes a single merged endpoint. Only
Z.ai has a public usage API today; Anthropic returns `NotSupported`. The
architecture is provider-generic so adding future providers requires only a
new adapter.

## Motivation

Users on the GLM Coding Plan need to see how much of their quota they've
consumed and when it resets — the same information `claude.ai/settings/usage`
shows for Anthropic. Today there is no way to see this inside `cli-router`.
Z.ai exposes three monitoring endpoints that return exactly this data. The
proxy already holds auth credentials and runs as a long-lived daemon, making
it the natural place to query these APIs on the TUI's behalf.

## Approach: Provider-generic port + per-provider adapter + single admin endpoint

A new application port `AccountUsagePort` with one method
`fetch_usage() -> Option<Result<ProviderAccountUsage, ProxyError>>`.
Each provider kind gets an adapter: `ZaiAccountUsage` (hits Z.ai's three
monitoring endpoints), `AnthropicAccountUsage` (returns `None`). The builder
constructs the right adapter per provider. A new use case
`GetAccountUsage` iterates all adapters and produces a merged DTO. One new
admin endpoint `GET /admin/account/usage` serves it. `proxy-tui` gains a new
`View::Account` tab that calls it on-demand.

## Z.ai Usage API Reference

Three endpoints, all `GET`, auth via `Authorization: <token>` (no "Bearer"
prefix).

| Endpoint | Purpose | Query Params |
|---|---|---|
| `/api/monitor/usage/quota/limit` | Quota windows (5h token, weekly, monthly MCP) + plan tier | None |
| `/api/monitor/usage/model-usage` | Model token/call usage (24h rolling) | `startTime`, `endTime` (epoch ms) |
| `/api/monitor/usage/tool-usage` | MCP tool usage counts (24h rolling) | `startTime`, `endTime` (epoch ms) |

Base URL: `https://api.z.ai` (global) or `https://open.bigmodel.cn` (CN).

### Response shapes (observed)

**`/api/monitor/usage/quota/limit`**

```json
{
  "level": "pro",
  "limits": [
    {
      "type": "TOKENS_LIMIT",
      "percentage": 40.5,
      "total": 40000000,
      "nextResetTime": 1746300000000
    },
    {
      "type": "TOKENS_LIMIT",
      "percentage": 52.0,
      "nextResetTime": 1746600000000
    },
    {
      "type": "TIME_LIMIT",
      "percentage": 12.3,
      "currentValue": 123,
      "usage": 1000,
      "usageDetails": [
        { "modelCode": "search-prime", "usage": 5678 },
        { "modelCode": "web-reader", "usage": 2345 },
        { "modelCode": "zread", "usage": 890 }
      ]
    }
  ]
}
```

The first `TOKENS_LIMIT` is the 5-hour rolling window. The second is the
weekly window. `TIME_LIMIT` is the monthly MCP allowance. Distinguishing the
two token limits is by index (first = 5h, second = weekly) or by comparing
`nextResetTime` deltas.

**`/api/monitor/usage/model-usage`**

```json
{
  "totalUsage": {
    "totalTokensUsage": 12500000,
    "totalModelCallCount": 1234
  }
}
```

**`/api/monitor/usage/tool-usage`**

```json
{
  "totalUsage": {
    "totalNetworkSearchCount": 5678,
    "totalWebReadMcpCount": 2345,
    "totalZreadMcpCount": 890
  }
}
```

## Architecture

```
proxy-tui                          proxy daemon
─────────                          ─────────
Account  ─────────────────────►    GET /admin/account/usage       (NEW)
                                         ▼
                                   GetAccountUsage (use case)
                                         ▼
                                   HashMap<String, AccountUsagePort>
                                      ├── "zai" → ZaiAccountUsage
                                      │            ├── /api/monitor/usage/quota/limit
                                      │            ├── /api/monitor/usage/model-usage
                                      │            └── /api/monitor/usage/tool-usage
                                      └── "anthropic" → AnthropicAccountUsage (None)
```

Server-side aggregation: TUI makes one call, daemon fans out to all
providers. Partial failure is fine — one provider error doesn't block
others.

## Domain types

New file: `crates/proxy/src/domain/account_usage.rs`.

```rust
pub struct ProviderAccountUsage {
    pub provider: String,
    pub status: AccountUsageStatus,
    pub plan: Option<String>,
    pub windows: Vec<UsageWindow>,
    pub model_usage: Option<ModelUsageSnapshot>,
}

pub enum AccountUsageStatus {
    Available,
    NotSupported,
    Error(String),
}

pub struct UsageWindow {
    pub label: String,              // "5h Token", "Weekly", "MCP (1 Month)"
    pub used_pct: f64,              // 0.0–100.0
    pub used: Option<u64>,
    pub limit: Option<u64>,
    pub resets_at_ms: Option<i64>,
    pub sub_items: Vec<UsageSubItem>,
}

pub struct UsageSubItem {
    pub label: String,              // "Network Searches", "Web Reads", "ZRead Calls"
    pub used: u64,
}

pub struct ModelUsageSnapshot {
    pub total_tokens: u64,
    pub total_calls: u64,
    pub period_start_ms: i64,
    pub period_end_ms: i64,
}
```

Pure data, `std` + `chrono` only. Re-exported from `domain/mod.rs`.

## Application port

New file: `crates/proxy/src/application/ports/account_usage.rs`.

```rust
pub trait AccountUsagePort: Send + Sync {
    /// Returns `None` if this provider kind does not support usage queries.
    /// Returns `Some(Ok(..))` with the snapshot on success.
    /// Returns `Some(Err(..))` on upstream failure.
    fn fetch_usage(&self) -> Option<Result<ProviderAccountUsage, ProxyError>>;
}
```

Re-exported from `application/ports/mod.rs`.

## Adapters

### Z.ai

New file: `crates/proxy/src/adapters/providers/account_usage/zai.rs`.

Constructs with `provider_name`, resolved auth token, and base URL.
Implements `AccountUsagePort::fetch_usage` by hitting the three Z.ai
monitoring endpoints via `ureq` (sync, already in workspace).

Auth header: `Authorization: <token>` (no "Bearer" prefix, per Z.ai docs).

Response mapping:

- `level` → `plan`
- `limits[0]` (TOKENS_LIMIT) → 5h token window
- `limits[1]` (TOKENS_LIMIT) → weekly token window
- `limits[2]` (TIME_LIMIT) → MCP monthly window, with `usageDetails` mapped to `sub_items`
- `model-usage` response → `ModelUsageSnapshot` (period = requested 24h window)
- Tool usage totals folded into the MCP window's `sub_items` (or as additional
  sub-items — whichever is cleaner at impl time; the data overlaps)

Handles: HTTP errors → `Some(Err(...))`, JSON parse errors → `Some(Err(...))`.
Partial success (quota works, model-usage fails) returns what it has with
`model_usage = None`.

### Anthropic

New file: `crates/proxy/src/adapters/providers/account_usage/anthropic.rs`.

Zero-sized struct. `fetch_usage()` returns `None`.

### Module structure

```
crates/proxy/src/adapters/providers/account_usage/
├── mod.rs          // declares zai + anthropic modules
├── zai.rs
└── anthropic.rs
```

### Builder changes

`adapters/providers/builder.rs` gains a method:

```rust
pub fn build_account_usage(&self) -> HashMap<String, Arc<dyn AccountUsagePort>>
```

Iterates `self.providers`, picks adapter by `kind` (`"zai"` → `ZaiAccountUsage`,
everything else → `AnthropicAccountUsage`). Auth resolution reuses the same
logic `build()` uses — extracts the resolved token value from the config's
`AuthConfig` variant.

## DTOs (proxy-admin-api)

Appended to `crates/proxy-admin-api/src/lib.rs`.

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountUsageResponse {
    pub providers: Vec<ProviderAccountUsageDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderAccountUsageDto {
    pub provider: String,
    pub status: ProviderUsageStatus,
    pub plan: Option<String>,
    pub windows: Vec<UsageWindowDto>,
    pub model_usage: Option<ModelUsageDto>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderUsageStatus {
    Available,
    NotSupported,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageWindowDto {
    pub label: String,
    pub used_pct: f64,
    pub used: Option<u64>,
    pub limit: Option<u64>,
    pub resets_at_ms: Option<i64>,
    pub sub_items: Vec<UsageSubItemDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageSubItemDto {
    pub label: String,
    pub used: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelUsageDto {
    pub total_tokens: u64,
    pub total_calls: u64,
    pub period_start_ms: i64,
    pub period_end_ms: i64,
}
```

## Endpoint

`GET /admin/account/usage` → 200 JSON `AccountUsageResponse`.

No query params. Returns all providers. Providers are sorted by name
(ascending, alphabetical) for deterministic ordering.

Handler is sync (matches existing pattern for `GetUsageSummary`). The
upstream Z.ai calls (ureq, ~200ms each) run in the handler. Axum serves
this via `spawn_blocking` or the existing sync-in-axum pattern.

## Use case

New struct appended to `crates/proxy/src/application/use_cases/admin.rs`.

```rust
pub struct GetAccountUsage {
    adapters: HashMap<String, Arc<dyn AccountUsagePort>>,
}

impl GetAccountUsage {
    pub fn new(adapters: HashMap<String, Arc<dyn AccountUsagePort>>) -> Self { ... }

    pub fn execute(&self) -> AccountUsageResponse {
        // Iterate adapters sorted by name.
        // Each: match on fetch_usage() → Available | NotSupported | Error.
        // Map domain → DTO.
        // Partial failure is fine — one provider error doesn't affect others.
    }
}
```

## main.rs wiring

After existing provider building, before `AdminState` construction:

```rust
let account_usage_map = builder.build_account_usage();
let get_account_usage = Arc::new(GetAccountUsage::new(account_usage_map));
```

`AdminState` gains `pub account_usage: Arc<GetAccountUsage>`.
Route registered in `build_admin_router`:
`.route("/admin/account/usage", get(account_usage_handler))`.

## TUI changes (proxy-tui)

### app.rs

- `View::Account` appended to `View` enum and `ALL_VIEWS`.
- `View::Account => "Account"` in the label impl.
- New `AccountPaneState`:

```rust
#[derive(Debug, Clone, Default)]
pub struct AccountPaneState {
    pub usage: Option<AccountUsageResponse>,
    pub last_error: Option<String>,
    pub loading: bool,
    pub scroll_offset: usize,
}
```

- `AppState` gains `pub account: AccountPaneState`.

### client.rs

```rust
pub fn get_account_usage(&self) -> Result<AccountUsageResponse, ClientError> {
    get_json(&format!("{}/admin/account/usage", self.base_url))
}
```

### views/account.rs

New file: `crates/proxy-tui/src/views/account.rs`.

Layout (one provider block per configured provider):

```
┌─ Account ────────────────────────────────────────────────────────┐
│ ╭─ zai (Pro) ──────────────────────────────────────────────────╮ │
│ │  ⏱ 5h Token   ████████░░░░  67.3%   resets in 2h 14m       │ │
│ │     26.9M / 40.0M tokens                                     │ │
│ │  📅 Weekly    ████░░░░░░░░  38.1%   resets in 4d 8h         │ │
│ │  🔌 MCP (1M)  █░░░░░░░░░░  12.3%                            │ │
│ │     123 / 1,000   🔍 5,678  🌐 2,345  📖 890               │ │
│ │                                                              │ │
│ │  Model usage (24h)                                           │ │
│ │  Tokens: 12,500,000   Calls: 1,234                          │ │
│ ╰──────────────────────────────────────────────────────────────╯ │
│ ╭─ anthropic ──────────────────────────────────────────────────╮ │
│ │  No account usage API available for this provider.           │ │
│ ╰──────────────────────────────────────────────────────────────╯ │
│  [r] Refresh   [↑/↓] Scroll                                      │
└──────────────────────────────────────────────────────────────────┘
```

Each provider block:
- Title: provider name + plan tier (if available)
- Quota windows: progress bar (20-char `█░`), percentage, reset countdown
- Sub-items: inline on a single line below the parent window
- Model usage: summary line with tokens + calls
- `NotSupported`: one-liner explanation
- `Error`: error message displayed

Progress bar color: green ≤ 50%, yellow 50–80%, red ≥ 80%.

Uses `shared::adapters::presenters::formatting` for number formatting
(`fmt_num`, `fmt_num_compact`, `fmt_cost`).

### Keys (Account tab only)

- `r` — refresh (re-fetch from daemon)
- `↑` / `↓` — scroll when content overflows
- `Tab` — next view (existing wiring)

### Refresh model

On-demand, identical to the existing Usage tab:
- Fetched once on first tab entry.
- Refetched on `r` key or when switching back to the tab (if data is stale
  or never fetched).
- No polling — quota data changes on 5h/weekly/monthly windows.

### main.rs (proxy-tui)

- Declare `views::account` module.
- Wire `View::Account` in the event loop: key handling (`r`, `↑`, `↓`).
- Wire `View::Account` in renderer → `crate::views::account::draw`.

## Error handling

| Case | Handling |
|---|---|
| Z.ai upstream returns non-2xx | Provider marked `Error` in response; TUI shows error message |
| Z.ai upstream times out | Same — `ureq` timeout → `Error` |
| One provider fails, others succeed | Partial response; failed provider shows error, others render normally |
| All providers `NotSupported` | TUI renders "No providers support account usage queries." |
| TUI → daemon network error | Show inline error banner; keep last good data cached |
| Missing/malformed JSON from upstream | `Error` status; TUI shows "Failed to parse provider response" |

## Testing

### Adapter tests (`zai.rs`)

- Unit test with wiremock returning fixed Z.ai JSON → correct domain mapping
- Auth header test: verify `Authorization: <token>` (no Bearer prefix)
- Network error test: wiremock returns 500 → `Some(Err(...))`
- Partial data test: quota succeeds, model-usage fails → returns what it has

### Use case test (`GetAccountUsage`)

- Happy path: two stubs (one `Some(Ok)`, one `None`) → correct DTOs
- Error path: stub returns `Some(Err(...))` → `Error` status
- Empty map: no adapters → empty providers vec
- Sorting: adapters inserted in reverse order → response sorted alphabetically

### DTO serde test (`proxy-admin-api`)

- Round-trip `AccountUsageResponse` through JSON

### Integration test (`proxy/tests/`)

- Wire handler with stub adapters → hit `GET /admin/account/usage` → 200 + JSON shape

### TUI client test

- wiremock stub → client deserializes `AccountUsageResponse` correctly

### TUI state test

- `AccountPaneState` transitions: loading → loaded, loading → error with cached data preserved

No new dev-deps. `wiremock`, `tokio`, `tower` already in workspace.

## File structure

### New files

- `crates/proxy/src/domain/account_usage.rs` — domain types
- `crates/proxy/src/application/ports/account_usage.rs` — `AccountUsagePort` trait
- `crates/proxy/src/adapters/providers/account_usage/mod.rs` — module declaration
- `crates/proxy/src/adapters/providers/account_usage/zai.rs` — Z.ai adapter
- `crates/proxy/src/adapters/providers/account_usage/anthropic.rs` — no-op adapter
- `crates/proxy-tui/src/views/account.rs` — Account tab renderer
- `crates/proxy/tests/account_usage_api.rs` — integration test

### Modified files

- `crates/proxy/src/domain/mod.rs` — declare `account_usage`
- `crates/proxy/src/application/ports/mod.rs` — declare + re-export `account_usage`
- `crates/proxy/src/application/use_cases/admin.rs` — add `GetAccountUsage`
- `crates/proxy/src/adapters/providers/builder.rs` — add `build_account_usage`
- `crates/proxy/src/frameworks/admin.rs` — add route + handler + `AdminState` field
- `crates/proxy/src/main.rs` — construct use case, add to `AdminState`
- `crates/proxy-admin-api/src/lib.rs` — add DTOs + serde test
- `crates/proxy-tui/src/app.rs` — add `View::Account`, `AccountPaneState`
- `crates/proxy-tui/src/client.rs` — add `get_account_usage`
- `crates/proxy-tui/src/main.rs` — wire keys + initial fetch
- `crates/proxy-tui/src/ui.rs` — dispatch `View::Account` to `views::account`

## Out of scope (v1)

- Background polling / cached snapshots (on-demand is fine; add later if stale data is annoying)
- Per-provider endpoint (`GET /admin/providers/:name/account-usage`) — single merged endpoint is simpler
- Anthropic usage API (none exists publicly)
- CN platform (`open.bigmodel.cn`) support — the adapter takes a base URL so adding it is config-only
- Historical usage charts / sparklines
- Export (CSV / JSON dump)
- Config-based opt-in/opt-out per provider (all providers are queried; `NotSupported` is determined by adapter)
