# Unified provider with dual-format (URL-driven) routing — design

- **Date:** 2026-07-24
- **Status:** Approved (brainstorm), pending implementation plan
- **Approach:** C — collapse the per-kind provider structs into one generic
  `UpstreamProvider`; move per-kind differences into data (`Quirks` + a
  `preset(kind)` table).

## Problem

Format handling is currently decided by a hardcoded `Provider::native_format()`
that returns a single `ApiFormat` per provider struct. Consequences:

- A provider that physically exposes **both** an Anthropic and an OpenAI
  endpoint (MiniMax, Kimi) is still pinned to one. Whichever client format
  doesn't match is translated through the lossy Anthropic↔OpenAI layer, even
  though the matching upstream endpoint exists and sits idle.
- Each provider is a separate struct (`MinimaxProvider`, `DeepSeekProvider`,
  `ZaiProvider`, `KimiProvider`, `OpenAiProvider`, `CodexProvider`,
  `AnthropicProvider`) that differs only in auth, endpoint URLs, and a couple
  of request/response quirks. Adding a provider means writing a struct.

## Goals

1. Each provider can configure a URL for **both** formats (Anthropic and
   OpenAI), each optional.
2. The endpoint the client calls determines the **client format**
   (`/v1/messages` → Anthropic, `/v1/chat/completions` → OpenAI). Unchanged.
3. Format capability is **URL-driven**, not hardcoded:
   - both URLs configured → **passthrough** to the endpoint matching the
     client format (no translation for either client);
   - exactly one URL configured → matching client = passthrough, other client
     = translate to the configured endpoint;
   - zero URLs → configuration error at build time.
4. Collapse the seven provider structs into one generic `UpstreamProvider`.
   Per-kind behavior becomes data.

## Non-goals

- Changing the translation algorithms themselves (`translation/*`,
  `minimax_stream`). Only *where* they are invoked changes.
- Changing the account-usage or OAuth token-refresh subsystems. They remain
  keyed by `ProviderKind` and are wired separately in the builder.
- Per-endpoint auth. A provider has one `AuthConfig`, applied to whichever
  endpoint is used. (OAuth kinds are Anthropic-only in practice; configuring an
  OpenAI URL for an OAuth-only kind is unsupported.)

## Core concepts

Two independent axes, unchanged in spirit:

- **Client format** — per request, from the endpoint. Dynamic.
- **Provider capability** — per provider, from which base URLs are configured.
  Static for a given config.

Translation decision:

```
sup = provider.supported_formats()            // { anthropic: bool, openai: bool }
upstream_format =
    if sup.has(client_format) { client_format }   // passthrough
    else { sup.the_only_one() }                    // translate to the one it has
direction = Direction::from_pair(client_format, upstream_format)
```

`Direction::Passthrough` when the two formats match; otherwise the existing
`AnthropicToOpenAI` / `OpenAIToAnthropic` translation applies.

## Architecture

### `UpstreamProvider` (adapters/providers)

Replaces all seven per-kind structs.

```rust
pub struct UpstreamProvider {
    name: String,                        // the configured provider name (unique)
    anthropic_base_url: Option<String>,  // Anthropic endpoint, if configured
    openai_base_url: Option<String>,     // OpenAI endpoint, if configured
    auth: AuthHeader,
    quirks: Quirks,
}
```

Implements `Provider`:

- `supported_formats()` — derived from which base URL is `Some`.
- `forward(path, headers, body, streaming)` — Anthropic upstream. Requires
  `anthropic_base_url`; applies Anthropic-format request quirks, calls
  `messages_protocol::forward`, applies Anthropic-format response quirks.
- `forward_openai(path, headers, body, streaming)` — OpenAI upstream. Requires
  `openai_base_url`; applies OpenAI-format request quirks (reasoning_split,
  reasoning_effort, tool reorder, empty-tool sanitize), calls
  `messages_protocol::forward`, applies OpenAI-format response quirks
  (thinking strip).
- If a `forward*` method is entered without its URL configured, return a
  `ProxyError::BadRequest`. Routing guarantees this never happens (it checks
  `supported_formats()` first), so this is a defensive guard.
- `parse_model` / `parse_model_and_stream` / usage parsers delegate to
  `messages_protocol` (format-appropriate variant), unchanged.

`name()` returns the configured provider name. **Identity note:** today several
provider structs return the *kind* string (e.g. `"minimax"`) from `name()`,
and quota/affinity/cooldown key off `provider.name()`. Switching to the config
name is more correct (two providers of the same kind become distinct) but is a
behavior change; the implementation must add tests confirming quota, affinity
rendezvous hashing, and 429-cooldown identity still line up, and the migration
must not split what used to be one identity in a way that surprises the user.

### `Quirks`

The complete set of per-provider request/response differences on the forward
path, as plain data:

```rust
pub struct Quirks {
    reasoning_split: bool,                 // MiniMax: inject reasoning_split:true (OpenAI req)
    strip_thinking: Option<ThinkingMode>,  // MiniMax: filter thinking (OpenAI resp)
    reasoning_effort: Option<String>,      // Codex / Anthropic: inject into req
    reorder_tool_responses: bool,          // DeepSeek: reorder tool responses after tool_calls
    sanitize_empty_tools: bool,            // Kimi
}
```

Quirks are applied inside `forward` / `forward_openai`. The existing quirk
implementations move verbatim; only their call site changes. `minimax_stream`
and `translation/*` are reused as-is.

### `preset(kind) -> ProviderPreset`

`ProviderKind` stays in config and remains user-facing. At build time a preset
table maps each kind to its defaults:

| kind | default URL(s) | auth style | quirks enabled | account-usage adapter |
|---|---|---|---|---|
| anthropic | anthropic | OAuth | reasoning_effort | AnthropicAccountUsage |
| minimax | anthropic + openai | Bearer | reasoning_split, strip_thinking | MinimaxAccountUsage |
| kimi | anthropic + openai | Bearer | sanitize_empty_tools | KimiAccountUsage |
| zai | openai | Bearer | — | ZaiAccountUsage |
| deepseek | openai | Bearer | reorder_tool_responses | DeepSeekAccountUsage |
| openai | openai | ApiKey/OAuth | — | NoopAccountUsage |
| codex | openai | CodexAuto | reasoning_effort | CodexAccountUsage |

Config values override the preset: explicit `anthropic_base_url` /
`openai_base_url`, `thinking_mode`, `reasoning_effort`, `sanitize_empty_tools`.
The default URLs are used only as **prefill at creation time** (migration and
the TUI wizard). They are **not** a runtime fallback — the stored columns are
the single source of truth for capability, so omitting a URL genuinely makes a
provider single-format.

Adding a future provider = one preset row, no new struct.

## Config schema and migration

### `ProviderConfig` (config.rs)

- Rename `base_url` → `anthropic_base_url`.
- Keep `openai_base_url`.
- `kind`, `auth`, `reasoning_effort`, `thinking_mode`, `max_concurrent`,
  `sanitize_empty_tools`, `enabled` unchanged.

### SQLite `providers` table (adapters/storage)

- Add column `anthropic_base_url TEXT NULL`.
- Backfill from the existing `base_url` **by kind**, because `base_url`'s
  meaning currently depends on kind:

  | kind | old `base_url` meant | migrated into |
  |---|---|---|
  | anthropic, minimax, zai, kimi | Anthropic URL | `anthropic_base_url` |
  | deepseek, openai, codex | OpenAI URL | `openai_base_url` |

- `openai_base_url` (currently populated only for zai/minimax/kimi) is kept as
  is.
- After backfill, drop `base_url` (or leave it unused and stop reading it —
  decided in the plan; dropping is cleaner but SQLite column drop needs a table
  rebuild, so the plan may keep it nullable and ignored).
- `db_config.rs` reads/writes the two explicit columns.

### Effect on the current prod DB

| provider | after migration | behavior |
|---|---|---|
| minimax (both URLs) | anthropic + openai | **dual passthrough** — Claude Code and OpenCode both passthrough |
| kimi (both URLs) | anthropic + openai | **dual passthrough** — Claude Code → Kimi stops being translated |
| zai (openai only) | openai | unchanged (Anthropic client translated) |
| deepseek (openai only) | openai | unchanged |
| anthropic (anthropic only) | anthropic | unchanged |

This dual-for-all outcome is intended.

## Ports change (application/ports/provider.rs)

- Remove `fn native_format(&self) -> ApiFormat`.
- Add `fn supported_formats(&self) -> FormatSupport`, where

  ```rust
  pub struct FormatSupport { pub anthropic: bool, pub openai: bool }
  impl FormatSupport {
      pub fn has(&self, f: ApiFormat) -> bool { … }
      // the single supported format; only called when !has(client_format),
      // which implies exactly one is true.
      pub fn sole(&self) -> ApiFormat { … }
  }
  ```

- `Direction` and `ApiFormat` are unchanged.

## Routing change (adapters/providers/routing.rs)

Every `match provider.native_format()` site (namespace `forward`,
`forward_openai`, failover, round-robin) is replaced by the capability
selection above:

```rust
let sup = provider.supported_formats();
let upstream_format = if sup.has(client_format) { client_format } else { sup.sole() };
let direction = Direction::from_pair(client_format, upstream_format);
let native_path = Self::translate_path(path, direction);
let send_body = Self::translate_request(&body, direction)?;
let raw = match upstream_format {
    ApiFormat::Anthropic => provider.forward(native_path, headers, send_body, streaming).await,
    ApiFormat::OpenAI    => provider.forward_openai(native_path, headers, send_body, streaming).await,
}?;
Self::translate_upstream_response(raw, direction)
```

`translate_path`, `translate_request`, `translate_upstream_response` are
unchanged. Pool selection, 429 cooldown, affinity, quota are unchanged.

## Builder / composition (adapters/providers/builder.rs)

- `build_leaf` constructs one `UpstreamProvider` from `ProviderConfig`:
  resolve `AuthConfig → AuthHeader` (unchanged), look up `preset(kind)`, merge
  config overrides, produce `anthropic_base_url` / `openai_base_url` / `quirks`.
- Validate: at least one base URL is present after merge, else `BuildError`.
- `build_account_usage` stays keyed by `kind` (unchanged).
- OAuth `token_refresh` wiring stays keyed by auth type (unchanged).

## Admin API + TUI

- `proxy-admin-api` DTOs (`ConfigPayload` / provider payload): replace
  `base_url` with `anthropic_base_url`, keep `openai_base_url`.
- `proxy-tui` config wizard and edit modal: two optional URL fields, prefilled
  from `preset(kind)` defaults when the user picks a kind. Validation mirrors
  the builder: at least one URL required.

## Testing strategy

- **Unit — capability:** `supported_formats()` for all three states
  (both / anthropic-only / openai-only).
- **Unit — routing selection:** for the 2 client formats × 3 capability states,
  assert the chosen `upstream_format`, `Direction`, and whether the translator
  is invoked (passthrough must not touch the translator).
- **Unit — quirks:** each quirk still fires on the right format path
  (reasoning_split/strip_thinking on OpenAI; reorder/sanitize on OpenAI;
  reasoning_effort injection).
- **Unit — preset:** each kind maps to the expected URLs/auth/quirks/account
  adapter.
- **Integration (wiremock):** Claude Code (`/v1/messages`) and OpenCode
  (`/v1/chat/completions`) against a dual provider (both passthrough, no
  translation artifacts) and a single provider (matching passthrough, other
  translated and correctly terminated).
- **Migration test:** an old-schema DB row per kind migrates to the correct
  explicit columns.
- **Identity test:** quota, affinity, and cooldown keys behave correctly with
  `name()` returning the config name.

## Phased rollout (strangler; plan will detail)

Each phase keeps `cargo test --workspace` green and is committed separately.

1. Introduce `FormatSupport`, `Quirks`, `UpstreamProvider`, `preset(kind)`.
   No deletions; not yet wired.
2. Switch `build_leaf` to `UpstreamProvider`; delete the old provider structs
   one kind at a time, keeping tests green each step.
3. Schema migration + `db_config.rs` two-column read/write + `ProviderConfig`
   rename.
4. Ports: remove `native_format`, add `supported_formats`; rewrite routing
   selection sites.
5. Admin API DTO + TUI wizard two-URL fields. Remove now-dead code
   (`native_format`, per-struct remnants).

## Risks and mitigations

- **Large blast radius.** Mitigated by the strangler order and per-phase green
  tests.
- **`name()` identity shift** (kind → config name) affecting quota/affinity —
  covered by the identity test; if the current kind-as-name behavior must be
  preserved, the preset can carry an explicit identity, decided in the plan.
- **`base_url` semantic backfill** must be exactly right per kind — covered by
  the migration test with a row per kind.
- **Dual passthrough for Kimi** is a real behavior change (accepted): Claude
  Code → Kimi now hits Kimi's Anthropic endpoint directly. If Kimi's Anthropic
  endpoint misbehaves, the mitigation is to remove its `anthropic_base_url`
  (making it OpenAI-only again) — now a config change, not a code change.
