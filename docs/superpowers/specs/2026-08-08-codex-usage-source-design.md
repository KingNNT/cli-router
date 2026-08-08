# Codex Usage Source for the Analysis TUI

**Date:** 2026-08-08
**Status:** Design (approved by user, pending implementation plan)

## Overview

Add Codex CLI as a third usage data source in the `analysis` TUI, alongside
OpenCode and Claude Code. Codex writes one JSONL rollout file per session under
`~/.codex/sessions/YYYY/MM/DD/`, and every API request emits a `token_count`
event carrying that request's token usage. Parsing those files yields the same
`UsageRecord` shape the dashboard already consumes, so the entire presentation
layer works unchanged.

The new adapter mirrors `ClaudeCodeUsageRepository`: read-only, lazily loaded,
cached in memory, tolerant of malformed lines. The only genuinely new logic is
the token-field conversion (Codex reports cumulative-inclusive counts where the
domain expects disjoint buckets) and duplicate-event suppression.

`DataSource` grows from a two-state toggle to a three-state cycle; the `t` key
walks OpenCode → Claude Code → Codex → OpenCode.

## Data source shape

Verified against 61 rollout files on disk (2026-07-20 through 2026-08-08, Codex
CLI `0.147.0`). The shape described below was verified on the 11 files that
carry a `turn_context` line. The other 50 — everything under
`2026/06/18/`, written by `originator: "Codex Desktop"`,
`cli_version 0.140.0-alpha.19` — are a Desktop-format variant that omits
`turn_context` entirely; their `token_count` events have no model to attach to
and are skipped by the parser (see `parser.rs`'s module doc for the data-loss
analysis).

Each line is a JSON object with a top-level `type`. Three types matter:

**`session_meta`** — first line of the file.

```json
{"type":"session_meta","payload":{
  "session_id":"019fe150-9db5-7841-8ec5-ff36635eea31",
  "cwd":"/Users/kingnnt/Documents/workspaces/inviduality/blog-kingnnt-org",
  "cli_version":"0.147.0","model_provider":"openai"}}
```

**`turn_context`** — emitted once per turn, before that turn's requests.

```json
{"type":"turn_context","payload":{
  "turn_id":"019fe151-bbc6-7e72-9e44-b4e174c0bdc8",
  "cwd":"/Users/kingnnt/.../blog-kingnnt-org",
  "model":"gpt-5.6-sol"}}
```

**`event_msg`** with `payload.type == "token_count"` — one per API request.

```json
{"timestamp":"2026-08-08T12:27:24.560Z","type":"event_msg","payload":{
  "type":"token_count",
  "info":{
    "total_token_usage":{"input_tokens":476648,"cached_input_tokens":444055,
      "cache_write_input_tokens":32539,"output_tokens":6098,
      "reasoning_output_tokens":689,"total_tokens":482746},
    "last_token_usage":{"input_tokens":32542,"cached_input_tokens":32435,
      "cache_write_input_tokens":104,"output_tokens":174,
      "reasoning_output_tokens":18,"total_tokens":32716},
    "model_context_window":258400}}}
```

`last_token_usage` is the per-request delta; `total_token_usage` is the running
session total. The adapter reads `last_token_usage` and uses
`total_token_usage` only for duplicate detection.

Models observed: `gpt-5.6-sol`, `gpt-5.6-terra`, `gpt-5.5`. All three already
resolve in `pricing.db` under both the raw key and the `openai/`-prefixed key,
so `ModelId::lookup_keys()` finds rates with no alias-table changes.

### Field semantics that differ from Claude Code

Getting these wrong silently inflates every number on the dashboard, so they are
the core of this design.

1. **`input_tokens` is inclusive.** It counts fresh, cached, and cache-write
   tokens together. Sample: `32542 = 3 fresh + 32435 cached + 104 written`.
   Claude Code's `input_tokens` excludes both cache buckets, and
   `TokenBreakdown::total()` sums `input + output + reasoning + cache_read +
   cache_write`. Feeding Codex's inclusive value straight in would count cached
   tokens twice — in this sample roughly doubling the reported input.

2. **`reasoning_output_tokens` is inside `output_tokens`.** Adding it to the
   `reasoning` bucket would inflate `total()` the same way. Cost is unaffected
   either way — `calculate_cost` ignores `reasoning` — but the token totals are
   user-facing.

3. **`token_count` events can repeat.** 4 of 114 events across the 61 files
   repeat the previous event's `total_token_usage` verbatim (~3.5%). These are
   re-emissions, not new requests, and must not be counted twice.

## Architecture

New adapter slots into the existing dispatcher; nothing else changes shape.

```
tui (t key)
  └─ DataSourceCell ──────────────┐
                                  ▼
GetDashboard → DispatchingUsageRepository
                  ├─ SqliteUsageRepository      (OpenCode)
                  ├─ ClaudeCodeUsageRepository  (Claude Code)
                  └─ CodexUsageRepository       (Codex)   ← new
                        └─ ~/.codex/sessions/**/rollout-*.jsonl
```

`CodexUsageRepository` implements `UsageRepository` and lives in
`adapters/gateways/codex/`, matching the `claudecode/` module layout. It holds
`{ root: PathBuf, cache: Mutex<Option<Vec<UsageRecord>>> }` and loads every
rollout file once on first query, then serves from cache — identical to the
Claude Code adapter, and appropriate for the same reason: the TUI re-queries on
every filter change and the files are small.

Ring placement is unchanged: the adapter depends on `application` (the
`UsageRepository` port, `Filter`) and `domain` (`UsageRecord`, `TokenBreakdown`,
`ModelId`, `ProjectPath`), and nothing outward.

## Implementation

### Directory walk

`default_sessions_root()` returns `$HOME/.codex/sessions`. Unlike Claude Code's
single level of project directories, Codex nests `YYYY/MM/DD/`, so the walk
recurses. A missing root returns an empty record list, not an error — the same
contract Claude Code's adapter honours for users who don't have that CLI
installed.

Only files with a `.jsonl` extension are parsed.

### Per-file parse

One sequential pass per file, carrying state forward. `token_count` events
identify neither their turn nor their model, so the model must be taken from the
most recent `turn_context`.

| Line type | Action |
| --- | --- |
| `session_meta` | record `session_id` and `cwd` as the file-level fallback |
| `turn_context` | update `current_model` and `current_cwd` |
| `event_msg` / `token_count` | emit one `UsageRecord` |
| anything else | skip |

A `token_count` event arriving before any `turn_context` has no known model and
is skipped. Malformed JSON lines are skipped without aborting the file, matching
`parse_jsonl_into` in the Claude Code adapter.

Project path is `current_cwd`, falling back to the `session_meta` `cwd`. Date is
the event's `timestamp` parsed as RFC 3339 and taken as its **UTC** date, the
same convention the Claude Code adapter uses (`naive_utc().date()`).

### Token conversion

```
input       = input_tokens.saturating_sub(cached_input_tokens)
                          .saturating_sub(cache_write_input_tokens)
cache_read  = cached_input_tokens
cache_write = cache_write_input_tokens
output      = output_tokens          // already includes reasoning
reasoning   = 0                      // see field semantics, point 2
cost        = Cost::zero()           // priced later by GetDashboard
```

`saturating_sub` guards against a future Codex version making `input_tokens`
exclusive; the result degrades to zero fresh input rather than panicking or
wrapping.

### Duplicate suppression

Track the previous event's `total_token_usage` within the file. If the current
event's `total_token_usage` equals it field-for-field, skip the event. State
resets per file, since totals are per-session.

### `DataSource` becomes three-state

```rust
pub enum DataSource { OpenCode, ClaudeCode, Codex }
```

- `as_u8`: `Codex => 2`; `from_u8`: `2 => Codex`, unknown values still fall back
  to `OpenCode`.
- `label()`: `"Codex"`.
- `toggle()`: cycles `OpenCode → ClaudeCode → Codex → OpenCode`. The name stays
  `toggle` — the controller calls it and the semantics ("advance to the next
  source") are unchanged.

`DispatchingUsageRepository` gains a `codex: Arc<dyn UsageRepository>` field and
a third `active()` arm. `main.rs` constructs the repository and passes it in.

The renderer needs no change: the status bar already prints
`state.data_source.get().label()`.

## Error Handling

The adapter never fails a query because of bad data on disk. Missing root,
unreadable file, malformed line, unparseable timestamp, invalid `ModelId` or
`ProjectPath` — each is skipped at its own granularity, so one corrupt session
cannot hide the rest.

Genuine I/O failures while enumerating directories map to
`AdapterError::DataMapping("codex io: …")`, mirroring the Claude Code adapter's
`io_err`, and surface through `ApplicationError::Repository`.

## Testing

Unit tests in `adapters/gateways/codex/repository.rs`, following the `TestRoot`
fixture pattern already used by the Claude Code adapter (temp dir keyed by PID,
removed on `Drop`).

- missing root → zero overview
- empty root → zero overview
- one full session (`session_meta` + `turn_context` + `token_count`) → one
  record with the expected model, project, and session id
- **token conversion**: `input_tokens: 32542, cached: 32435, cache_write: 104`
  → `input 3, cache_read 32435, cache_write 104`; `output 174` with
  `reasoning 0` despite `reasoning_output_tokens: 18`
- **dedup**: two consecutive events with identical `total_token_usage` count
  once
- **model attribution**: two `turn_context` lines with different models, each
  followed by a `token_count`, produce two rows under the right models
- `token_count` before any `turn_context` is skipped
- malformed line mid-file does not abort the remaining lines
- nested `YYYY/MM/DD` directories are all discovered
- date-range filter excludes out-of-range records
- `daily_by_model` aggregates per day and model

Dispatcher and controller tests extend the existing ones:

- `DispatchingUsageRepository` routes to the Codex repo when the cell is set
- `DataSource::toggle()` cycles through all three and returns to `OpenCode`
- `from_u8`/`as_u8` round-trip for all three variants
- the `t`-key controller test walks all three states and invalidates cached view
  models at each step

## Files Changed

| File | Change |
| --- | --- |
| `crates/analysis/src/adapters/gateways/codex/mod.rs` | new — re-exports |
| `crates/analysis/src/adapters/gateways/codex/repository.rs` | new — adapter + tests |
| `crates/analysis/src/adapters/gateways/mod.rs` | declare and re-export `codex` |
| `crates/analysis/src/adapters/gateways/dispatching_usage_repository.rs` | third `DataSource` variant, third dispatch arm |
| `crates/analysis/src/main.rs` | construct and wire `CodexUsageRepository` |
| `crates/analysis/src/tui/controllers/tui_controller.rs` | extend `t`-key test to three states |
| `CLAUDE.md` | mention the Codex source in the `analysis` description |

## Assumptions

- **Cost is notional.** Codex here authenticates against a ChatGPT plan, so
  per-token cost is a list-price conversion rather than money billed. This
  matches how the dashboard already treats Claude Code usage, so it is reported
  the same way rather than suppressed.
- **UTC day buckets.** Days are bucketed by UTC date, consistent with the Claude
  Code adapter. For a user in `Asia/Ho_Chi_Minh` (UTC+7), a late-night session
  falls into the previous UTC day. Changing this is a separate, cross-source
  decision and is out of scope here.
- **`last_token_usage` is authoritative per request.** Deltas are read directly
  rather than reconstructed by differencing `total_token_usage`, which would be
  fragile across the compaction and re-emission the duplicate events hint at.
