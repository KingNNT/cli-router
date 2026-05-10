# DeepSeek Provider + Account Usage Display

**Date:** 2026-05-11
**Status:** Design (approved by user, pending implementation plan)

## Overview

Add DeepSeek as a fully supported upstream LLM provider in the cli-router proxy,
then display DeepSeek's account-level usage (balance + per-model breakdown) on
the proxy-tui Account tab (tab 6).

DeepSeek's chat/completions API is OpenAI-compatible, so the proxy integration
is a thin wrapper following the same pattern as ZaiProvider. DeepSeek's account
API is limited to `GET /user/balance` (no token-usage statistics endpoint like
Zai has), so per-model breakdowns are sourced from the proxy's own SQLite
request log.

## Architecture

```
DeepSeek API (balance) ──→ DeepSeekAccountUsage adapter ──→ ProviderAccountUsage
                                                                  │
Proxy SQLite request log ──→ per-model query (admin use case) ────┤
                                                                  ▼
                                            /admin/account/usage response
                                                                  │
                                                                  ▼
                                                    proxy-tui Account tab
```

Two data sources merge at the admin use case layer:
1. **Account-level** (balance): fetched from DeepSeek's `/user/balance` API by the adapter
2. **Per-model breakdown**: queried from the proxy's own `requests` table for the last 24h

## Implementation

### 1. Config: ProviderKind::DeepSeek (`proxy/src/config.rs`)

- Add `DeepSeek` variant to `ProviderKind` enum
- Wire `parse_kind` to accept `"deepseek"` (case-insensitive)
- `legacy_default` unchanged (DeepSeek needs explicit config)

### 2. DeepSeek Provider Adapter (`proxy/src/adapters/providers/deepseek.rs`)

New file. Thin wrapper around OpenAI-compatible HTTP client. Same pattern as
`ZaiProvider`:
- Accepts `base_url` (default: `https://api.deepseek.com`)
- Auth: Bearer token from API key config
- No `openai_base_url` needed (native OpenAI format)
- Implements `Provider` trait

### 3. DeepSeek Account Usage Adapter (`proxy/src/adapters/providers/account_usage/deepseek.rs`)

New file. Calls DeepSeek balance API:
- `GET https://api.deepseek.com/user/balance` with `Authorization: Bearer {token}`
- Response shape (serde-deserialized):
  ```rust
  struct BalanceResponse {
      is_available: bool,
      balance_infos: Vec<BalanceInfo>,
  }
  struct BalanceInfo {
      currency: String,       // "CNY" or "USD"
      total_balance: String,  // e.g. "110.00"
      granted_balance: String,
      topped_up_balance: String,
  }
  ```
- Maps to `UsageWindow` per currency with label `"Top-up Balance (CNY)"` etc.
- Balance values are decimal strings (e.g. `"110.00"`). Parse to f64, multiply
  by 100, truncate to u64 for the DTO's `used`/`limit` fields (integer-only).
  `used` = `(total - topped_up) * 100`, `limit` = `total * 100`.
  `used_pct` = `used / limit * 100`.
- `model_usage` returned as `None` (no per-model API available)
- `status`: `Available` on success, `Error(msg)` on failure
- Network timeout: 10s read/write

### 4. Builder Wiring (`proxy/src/adapters/providers/builder.rs`)

- `build_leaf`: add `ProviderKind::DeepSeek` arm → `Arc::new(DeepSeekProvider::configure(...))`
- `build_account_usage`: add `ProviderKind::DeepSeek` arm → `Arc::new(DeepSeekAccountUsage::new(...))`
- Auth token extraction for DeepSeek: `resolve_auth_token` from ApiKey or Bearer
- No monitor base URL derivation needed (balance API is always at base_url)

### 5. Per-Model Breakdown Domain Type (`proxy/src/domain/account_usage.rs`)

Add to `ModelUsageSnapshot`:
```rust
pub model_breakdown: Vec<ModelBreakdownItem>,
```
```rust
pub struct ModelBreakdownItem {
    pub model: String,
    pub tokens: u64,
    pub calls: u64,
}
```

### 6. Wire Type (`proxy-admin-api/src/lib.rs`)

Add to `ModelUsageDto`:
```rust
pub model_breakdown: Vec<ModelBreakdownItemDto>,
```
```rust
pub struct ModelBreakdownItemDto {
    pub model: String,
    pub tokens: u64,
    pub calls: u64,
}
```

### 7. Admin Use Case (`proxy/src/application/use_cases/admin.rs`)

In the account usage handler, after collecting adapter results:
- Query the SQLite `requests` table for the last 24h
- Filter by each configured provider name (the `provider` column stores the
  config name as a string, e.g. `"deepseek"`, `"zai"`, `"anthropic"`)
- Group by `model` field
- Sum `input_tokens + output_tokens` as tokens, count rows as calls
- Sort by tokens descending
- Attach as `model_breakdown` on the `ModelUsageSnapshot` (creating one if needed)
- Same logic also applies to other providers (Zai) as a bonus — the data is already available

### 8. TUI Provider Kind (`proxy-tui/src/app.rs`)

- Add `ProviderKind::DeepSeek` with label `"deepseek"`
- Wire `cycle_next`/`cycle_prev` to include the new variant
- Add default test model suggestion: `"deepseek-chat"` in `open_test_modal`
- `from_str_or_default` maps `"deepseek"` to `ProviderKind::DeepSeek`

### 9. TUI Account View (`proxy-tui/src/views/account.rs`)

In `render_provider`:
- For balance windows: render `used_pct` as `(topped_up / total * 100)` with progress bar
- Display currency label + used/limit as strings (balance values are string-encoded floats)
- Render per-model breakdown lines below the aggregate model_usage line:
  ```
  │  Model usage (24h):  Tokens: 1.2M   Calls: 500
  │    deepseek-chat           800K tokens  300 calls
  │    deepseek-reasoner       400K tokens  200 calls
  ```
- Each per-model line: 2-space indent + model name (right-padded to 24 chars) + tokens (compact) + calls (compact)
- Skip breakdown rendering if `model_breakdown` is empty
- Sorted by tokens descending (already sorted from the backend)

### 10. Main.rs Key Bindings

No change needed. The Account tab already uses key `5` with `[r]` for refresh.

## Error Handling

- **Balance API unreachable**: `status: Error(msg)`, shown as error block in TUI
- **Balance API returns empty/zero**: still rendered (shows 0 balance)
- **Proxy request log empty for DeepSeek**: `model_breakdown` is empty vec, only aggregate line shown (or none if no model_usage)
- **No DeepSeek provider configured**: provider simply absent from account usage response

## Testing

- Unit tests for `BalanceResponse` deserialization
- Unit tests for balance-to-window mapping
- Unit tests for `ModelBreakdownItemDto` round-trip through JSON
- Unit tests for `ProviderKind::DeepSeek` cycle/parse
- Integration tests for account usage endpoint (wiremock for balance API + in-memory SQLite for request log)
- No TUI rendering tests (visual, covered by existing pattern consistency)

## Files Changed

| Crate | File | Change |
|---|---|---|
| proxy | `config.rs` | Add `ProviderKind::DeepSeek`, wire `parse_kind` |
| proxy | `adapters/providers/deepseek.rs` | **New**: `DeepSeekProvider` adapter |
| proxy | `adapters/providers/account_usage/deepseek.rs` | **New**: `DeepSeekAccountUsage` adapter |
| proxy | `adapters/providers/account_usage/mod.rs` | Add `pub mod deepseek`, re-export |
| proxy | `adapters/providers/builder.rs` | Wire DeepSeek in `build_leaf` and `build_account_usage` |
| proxy | `adapters/providers/mod.rs` | Add `mod deepseek`, re-export in provider list |
| proxy | `domain/account_usage.rs` | Add `ModelBreakdownItem` struct and field |
| proxy | `application/use_cases/admin.rs` | Query request log for per-model breakdown |
| proxy-admin-api | `src/lib.rs` | Add `ModelBreakdownItemDto`, field on `ModelUsageDto` |
| proxy-tui | `app.rs` | Add `ProviderKind::DeepSeek`, default model, cycle logic |
| proxy-tui | `views/account.rs` | Render per-model breakdown, handle balance window |
