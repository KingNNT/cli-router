# CodexAuto Auth — Design Spec

**Date:** 2026-05-18
**Status:** Approved

## Problem

Using OpenAI with a Codex subscription through cli-router requires manually copying
OAuth tokens from `~/.codex/auth.json` into `config.toml`. This is fragile — tokens
expire, the user must re-copy after `codex login`, and long JWTs clutter the config.

## Solution

New `AuthConfig::CodexAuto` auth type. The proxy reads `~/.codex/auth.json` automatically
at startup and during background refresh. No tokens in config.toml. No file mutation.

## Config Syntax

```toml
[[providers]]
name = "openai"
kind = "open_ai"
auth = { type = "codex_auto" }
```

## Architecture

### Startup — `builder.rs`

In `build_leaf()`, when auth is `CodexAuto`:

1. Call `oauth::openai::read_auth_json()` to read `~/.codex/auth.json`
2. Decode the JWT `exp` claim to compute `expires_at_ms`
3. Convert to `AuthHeader::OAuth { access_token, refresh_token, expires_at_ms }`
4. If file missing or invalid → return `BuildError` with clear message:
   `"CodexAuto auth requires ~/.codex/auth.json. Run 'codex login' first."`

### Background Refresh — `token_refresh.rs`

The existing `refresh_expiring` task runs every 5 minutes. For `CodexAuto` providers:

1. **Re-read `~/.codex/auth.json`** first — if Codex CLI refreshed the file
   since last check, use the fresh file tokens
2. If file tokens are fresh (`expires_at_ms` > now + 5 min) → update in-memory
   state from file, skip OAuth refresh
3. If file tokens are also expired → do OAuth refresh via
   `openai::refresh_token()`, update in-memory state only
4. **Never writes to `~/.codex/auth.json`** — Codex CLI owns that file
5. **Never writes tokens to config.toml** — stays clean with just `codex_auto`

### Auth Header — `messages_protocol.rs`

No changes needed. `AuthHeader::OAuth` already sends
`Authorization: Bearer <access_token>`. The existing code handles this correctly.

### TUI / Admin API — `admin.rs`

- `auth_to_payload()`: map `CodexAuto` → `AuthPayload::CodexAuto`
- `payload_to_auth()`: map `AuthPayload::CodexAuto` → `AuthConfig::CodexAuto`
- `kind_to_str` / `str_to_kind`: no new provider kind needed

## Files to Change

| Action | File | What |
|--------|------|------|
| Modify | `crates/proxy/src/config.rs` | +`AuthConfig::CodexAuto` variant, serde, Debug |
| Modify | `crates/proxy/src/adapters/providers/builder.rs` | +`CodexAuto` arm in `build_leaf()` |
| Modify | `crates/proxy/src/adapters/providers/token_refresh.rs` | +`CodexAuto` handling in `refresh_expiring` |
| Modify | `crates/proxy/src/application/use_cases/admin.rs` | +mapping arms for `CodexAuto` |
| Modify | `crates/proxy-admin-api/src/lib.rs` | +`AuthPayload::CodexAuto` variant |

## Error Handling

| Scenario | Behavior |
|----------|----------|
| `~/.codex/auth.json` missing at startup | `BuildError` — proxy won't start for this provider. Message: "CodexAuto auth requires ~/.codex/auth.json. Run 'codex login' first." |
| Token expired and refresh fails | Provider returns 401 to client, logged as error |
| File disappears while running | Next refresh cycle logs warning, provider keeps last-known tokens in memory |

## Scope Exclusions

- No new admin API endpoint
- No TUI changes (user edits config.toml directly)
- No writes to `~/.codex/auth.json`
- No config.toml mutation
- No changes to `openai_base_url` handling
