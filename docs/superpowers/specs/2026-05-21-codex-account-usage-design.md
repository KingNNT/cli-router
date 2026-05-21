# Codex Account Usage — Real Quota Display

Date: 2026-05-21

## Summary

Show real Codex quota remaining in the `proxy-tui` Account tab for Codex providers. Replace Codex's current `NoopAccountUsage` adapter with a native `CodexAccountUsage` adapter that authenticates with the same Codex/ChatGPT credentials used by `CodexAuto`, probes the Codex backend, parses the Codex rate-limit response headers, and maps them to existing account-usage windows.

## Motivation

Codex users need to see remaining plan usage, especially 5-hour and weekly windows, from inside `cli-router` instead of opening `https://chatgpt.com/codex/settings/usage` or running Codex CLI `/status`. The proxy already supports a provider-generic Account tab and stores enough Codex auth information to query the Codex backend. Codex currently reports `NotSupported`, even though OpenAI's Codex CLI now exposes rate-limit information by parsing Codex backend headers.

## External research

OpenAI documents Codex usage limits as plan-based, shared across Codex surfaces, and visible from the Codex usage dashboard or `/status`. The public documentation does not describe a stable standalone usage endpoint.

Current `openai/codex` source shows the CLI parses rate-limit data from backend response headers, including:

- `x-codex-primary-used-percent`
- `x-codex-primary-window-minutes`
- `x-codex-primary-reset-at`
- `x-codex-secondary-used-percent`
- `x-codex-secondary-window-minutes`
- `x-codex-secondary-reset-at`
- `x-codex-credits-has-credits`
- `x-codex-credits-unlimited`
- `x-codex-credits-balance`
- additional metered-limit families such as `x-codex-<limit>-primary-used-percent`

The Codex CLI maps window lengths to labels such as `5h`, `daily`, `weekly`, `monthly`, and displays percent remaining plus reset time. We will mirror this behavior with a small local parser instead of depending on Codex CLI output.

## Recommended approach

Add a `CodexAccountUsage` adapter under `crates/proxy/src/adapters/providers/account_usage/` and wire it from `build_account_usage()` for `ProviderKind::Codex`.

The adapter will:

1. Resolve credentials from provider auth config.
2. Send a minimal Codex backend probe request to the provider `base_url`, defaulting to `https://chatgpt.com/backend-api/codex`.
3. Parse Codex rate-limit headers from the response.
4. Return `ProviderAccountUsage` with `status = Available`, `plan = None`, and windows for primary/secondary limits and optional credits.

This keeps the TUI largely unchanged because `GetAccountUsage` already merges upstream windows with local proxy-log model/cost usage, and `views/account.rs` already renders upstream windows.

## Architecture

```text
proxy-tui Account view
        │
        ▼
GET /admin/account/usage
        │
        ▼
GetAccountUsage
        │
        ├── ZaiAccountUsage
        ├── AnthropicAccountUsage
        ├── DeepSeekAccountUsage
        └── CodexAccountUsage   (new)
                │
                ├── read ~/.codex/auth.json for CodexAuto
                ├── probe Codex backend
                └── parse x-codex-* rate-limit headers
```

### Adapter location

New file:

```text
crates/proxy/src/adapters/providers/account_usage/codex.rs
```

Existing module file updates:

```text
crates/proxy/src/adapters/providers/account_usage/mod.rs
crates/proxy/src/adapters/providers/builder.rs
```

## Authentication

`CodexAccountUsage` should accept the provider name, optional base URL, auth config, and an HTTP client.

Supported auth modes:

- `AuthConfig::CodexAuto`: read `~/.codex/auth.json` via the existing `crate::adapters::oauth::openai::read_auth_json()` helper.
- `AuthConfig::OpenAiOAuth`: use the configured access token.
- `AuthConfig::Bearer`: use the configured bearer token.

Unsupported auth modes:

- `ApiKey`: return `None` because API-key usage is token-billed API usage, not ChatGPT/Codex plan quota.
- `Passthrough`: return `None` because Account refresh has no client request credentials to pass through.

If `CodexAuto` is configured but `~/.codex/auth.json` is missing or invalid, return `Some(Err(...))` so the Account tab shows a clear provider-level error.

## Probe request

The adapter should call the Codex backend in a way that surfaces rate-limit headers. The preferred probe is a minimal `POST /responses` request to the configured Codex base URL, matching the existing Codex provider endpoint family.

The request must include:

- `Authorization: Bearer <access_token>`
- `Accept: text/event-stream`
- `Content-Type: application/json`

The body should be the smallest safe request that causes the backend to attach rate-limit headers. The implementation plan should verify the exact body with tests/mocks and, if possible, by comparing Codex CLI behavior. Candidate shape:

```json
{
  "model": "gpt-5.4-mini",
  "instructions": "",
  "input": [],
  "tools": [],
  "tool_choice": "auto",
  "parallel_tool_calls": false,
  "store": false,
  "stream": true
}
```

If a safer non-generating request shape is confirmed before implementation, use that. The design requirement is that Account refresh should avoid meaningful quota consumption. If the backend rejects the probe but still returns rate-limit headers, parse the headers and treat the snapshot as available.

## Header parsing

Implement a small Codex-specific parser in `codex.rs`.

Primary headers map to a primary window:

- `x-codex-primary-used-percent` → `used_pct`
- `x-codex-primary-window-minutes` → label derivation and/or metadata
- `x-codex-primary-reset-at` → `resets_at_ms`

Secondary headers map to a secondary window:

- `x-codex-secondary-used-percent`
- `x-codex-secondary-window-minutes`
- `x-codex-secondary-reset-at`

Reset headers are epoch seconds in Codex source; convert to epoch milliseconds for `UsageWindow.resets_at_ms`.

Label mapping should mirror Codex CLI behavior:

- approximately 300 minutes → `5h limit`
- approximately 1 day → `Daily limit`
- approximately 7 days → `Weekly limit`
- approximately 30 days → `Monthly limit`
- approximately 365 days → `Annual limit`
- unknown primary → `Usage limit`
- unknown secondary → `Secondary usage limit`

For account-usage windows, `used_pct` stores percent used, not percent remaining. The existing Account TUI progress bars already use percent used.

Credits headers should become a separate window only if credits are meaningful:

- `x-codex-credits-has-credits=true` and `x-codex-credits-unlimited=true` → `Credits`, one sub-item or displayable value indicating unlimited.
- `x-codex-credits-has-credits=true` and positive `x-codex-credits-balance` → `Credits`, `used = None`, `limit = None`, sub-item `Balance: <n> credits` or another representation compatible with existing rendering.

If credits do not fit the existing `UsageWindow` display cleanly, defer credits display and implement only primary/secondary quota windows in the first pass.

## DTO and TUI impact

No new DTO is required for the first pass. Existing `UsageWindowDto` already supports:

- label
- used percentage
- optional used/limit values
- reset timestamp
- sub-items

Existing Account TUI will render the new Codex upstream windows under the `── upstream API ──` section.

Optional small TUI polish:

- For Codex windows with no numeric used/limit, render just the progress bar, percentage, and reset time.
- Keep existing proxy-log estimated usage below the upstream API section when present.

## Error handling

Behavior should match other account-usage adapters:

- Unsupported auth → `None` so status is `NotSupported`.
- Missing/invalid `~/.codex/auth.json` for `CodexAuto` → `Some(Err(...))` with actionable text: run `codex login`.
- HTTP/network error with no rate-limit headers → `Some(Err(...))`.
- HTTP error with parseable rate-limit headers → `Some(Ok(...))`; the quota data is useful even if the probe body was rejected.
- Missing rate-limit headers → `Some(Err("Codex response did not include rate-limit headers"))`.
- Malformed individual headers should not fail the whole snapshot if at least one valid window exists.

## Testing

Unit tests should cover:

- Header parser maps primary 300-minute window to `5h limit` with epoch-ms reset.
- Header parser maps secondary 10080-minute window to `Weekly limit`.
- Percent values are preserved as percent used.
- Empty headers return no usable snapshot.
- Malformed optional fields are ignored without panicking.
- `build_account_usage()` maps Codex providers to `CodexAccountUsage` instead of `NoopAccountUsage`.
- Unsupported Codex auth modes produce `NotSupported` behavior.

Where practical, add a mock HTTP test using an in-process HTTP server or existing test helper to verify the adapter parses headers from a probe response without relying on live OpenAI services.

## Scope boundaries

In scope:

- Codex real quota windows in Account tab.
- Codex auth resolution for account usage.
- Codex rate-limit header parsing.
- Tests for parser and adapter wiring.

Out of scope:

- A new public admin endpoint; reuse `GET /admin/account/usage`.
- Persisting Codex quota snapshots in SQLite.
- Scraping `chatgpt.com/codex/settings/usage`.
- Calling or parsing interactive Codex CLI `/status` output.
- Full support for every future metered-limit header family beyond the default `codex` primary/secondary windows. The parser can be structured to allow this later.

## Risks

The Codex rate-limit headers and probe behavior are undocumented. To reduce breakage:

- Keep the parser permissive.
- Treat missing headers as a clear adapter error, not a crash.
- Keep local proxy-log usage enrichment working even when upstream Codex quota fetch fails.
- Document the dependency on Codex backend headers in code comments.

## Success criteria

- A Codex provider configured with `CodexAuto` no longer shows “No account usage API available” in Account.
- Account shows Codex 5-hour and weekly quota windows when the backend returns those headers.
- Refresh failures show actionable errors without breaking other providers.
- Existing local 24h model usage and 30d cost enrichment still appears for Codex when proxy logs contain data.
