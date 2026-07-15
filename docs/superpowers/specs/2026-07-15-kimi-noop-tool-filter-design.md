# Kimi no-op tool-call filter — design

**Date:** 2026-07-15
**Status:** Approved (pending spec review)

## Problem

When driving `moonshot/kimi-k2.7-code` from opencode through the proxy, tasks
frequently end in an infinite loop: opencode renders `$ :` with `(no output)`
and repeats forever.

Investigation (live requests through the running proxy):

- The Kimi path is a **pure passthrough**. opencode speaks Anthropic
  (`POST /v1/messages`), and `KimiProvider::forward` sends it verbatim to
  Moonshot's Anthropic-native endpoint (`translation_direction: None`). No
  cross-format translation runs. The proxy does not corrupt content.
- The loop is a **model-level behavior**: Kimi K2 over-eagerly emits `tool_use`
  blocks instead of ending the turn, and under real opencode conditions
  degrades into emitting a `bash` tool call with an empty / no-op `command`.
  opencode runs `:` (shell no-op) → `(no output)` → the model sees an empty
  result and calls again → loop. This matches known opencode+Kimi issues
  (anomalyco/opencode #14343, #11707, #20650, #3743, #8957).

The proxy cannot change the model, but it sits in the response path and can
**sanitize** these no-op tool calls before they reach opencode.

## Goal

Add an **opt-in, per-provider** filter that drops no-op `bash`/shell tool calls
from Kimi's Anthropic responses and, when that leaves nothing actionable, forces
`stop_reason = end_turn` so opencode stops instead of looping. The feature must
be **easy to disable later** — a config flag, default OFF, flippable at runtime.

## Scope

- **In scope:** `KimiProvider`, the **Anthropic** response path (`forward`),
  both **streaming** (opencode's mode) and **buffered** responses.
- **Out of scope:** the OpenAI-native path (`forward_openai` → Moonshot `/v1`).
  opencode uses Anthropic; the OpenAI SSE shape differs and is not the observed
  failure. Explicitly not filtered.
- **Out of scope:** other providers. The sanitizer module is written
  provider-agnostic (operates on Anthropic response format) so it *can* be
  reused later, but only Kimi is wired to it now.

## Decisions (confirmed)

1. **Toggle:** per-provider config field `sanitize_empty_tools: bool`, default
   `false`. Consistent with existing `thinking_mode` / `reasoning_effort`
   fields. Disable later = set back to `false` via admin API / TUI (hot reload,
   no rebuild). Removing the feature entirely is safe because the default is
   `false`.
2. **Filter action:** drop the no-op `tool_use` block(s); if no valid `tool_use`
   block remains in the message, rewrite `stop_reason` → `end_turn`. Text blocks
   are preserved untouched.
3. **No-op definition:** a `tool_use` block whose `input.command` is a string and
   `command.trim()` ∈ `{"", ":", "true"}`. Blocks without a string `command`
   field are never touched.

## Architecture

Four pieces, dependencies pointing inward per the ring rules.

### 1. Config — `config.rs` + `proxy-admin-api`

- `ProviderConfig` gains `sanitize_empty_tools: bool` with
  `#[serde(default)]` → deserializes to `false` when absent.
- `proxy-admin-api::ProviderPayload` gains
  `pub sanitize_empty_tools: Option<bool>` (mirrors how `thinking_mode` /
  `max_concurrent` are optional on the wire).

### 2. Storage — `storage/schema.rs` + `storage/db_config.rs`

- New migration `MIGRATION_V7`:
  ```sql
  ALTER TABLE providers ADD COLUMN sanitize_empty_tools INTEGER NOT NULL DEFAULT 0;
  ```
  Registered as `(7, MIGRATION_V7)` in `MIGRATIONS`.
- `DbConfigRepository` write path: include `sanitize_empty_tools` in the
  `INSERT`, binding `p.sanitize_empty_tools as i64`.
- `DbConfigRepository` read path: `SELECT ... sanitize_empty_tools`, map
  `i64 != 0` → `bool`.

### 3. Sanitizer — `adapters/providers/tool_sanitizer.rs` (new)

Pure logic, unit-testable in isolation. No provider/HTTP types.

- `pub fn is_noop_tool_use(block: &Value) -> bool`
  — true iff `block.type == "tool_use"` and `block.input.command` is a string
  whose `.trim()` ∈ `{"", ":", "true"}`.
- **Buffered:** `pub fn sanitize_buffered(body: &[u8]) -> Bytes`
  — parse the Anthropic message JSON; retain `content[]` entries that are not
  no-op `tool_use`; if the result contains no `tool_use` block at all, set
  `stop_reason = "end_turn"`; re-serialize. On parse failure, return the body
  unchanged (fail open — never break a response we can't understand).
- **Streaming:** `pub struct AnthropicNoopFilter` — an SSE state machine fed one
  `(event_name, data)` at a time, emitting zero or more re-serialized Anthropic
  SSE strings:
  - `message_start`, `message_delta`(usage), text `content_block_*`: pass
    through immediately.
  - On `content_block_start` with `type == "tool_use"`: **buffer** the block
    (index, id, name) and its subsequent `input_json_delta` fragments instead of
    emitting them. Do not emit yet.
  - On `content_block_stop` for a buffered tool block: concatenate the
    `partial_json` fragments, parse, and check `is_noop`. If no-op → **swallow**
    (emit nothing for this block). If valid → **flush** the buffered
    start + deltas + stop verbatim, and record that a real tool_use was emitted.
    (Block indices are preserved as received; opencode keys on `index`
    per-event, and dropping a block does not require re-indexing survivors.)
  - On `message_delta`: **buffer** the whole event (it carries both
    `stop_reason` and `usage`). Defer emitting it.
  - On `message_stop`: emit the buffered `message_delta`, then `message_stop`.
    If the buffered `stop_reason == "tool_use"` but **no** valid tool_use block
    was flushed this message, rewrite only its `stop_reason` field to
    `"end_turn"` first, preserving `usage` and every other field untouched.
  - Parse failure on a fragment → flush the buffered block as-is (fail open).

  Precedent: `minimax_stream.rs` already implements a buffering SSE filter for
  thinking content — same shape.

### 4. Wiring — `KimiProvider` + `builder.rs`

- `KimiProvider` gains `sanitize_empty_tools: bool` (threaded through
  `build` / `configure`). `builder.rs` passes `p.sanitize_empty_tools`.
- In `KimiProvider::forward` only (the Anthropic path): after
  `messages_protocol::forward` returns, if the flag is `false` return the
  `UpstreamResponse` unchanged (current behavior, zero overhead). If `true`:
  - `Buffered` → replace `body` with `tool_sanitizer::sanitize_buffered(&body)`.
  - `Streaming` → wrap the byte stream in an adapter that parses SSE, feeds
    `AnthropicNoopFilter`, and re-emits the filtered SSE bytes.
- `forward_openai` is unchanged.

## Data flow (streaming, flag ON)

```
opencode --Anthropic /v1/messages (stream)--> proxy
  HandleMessages (ApiFormat::Anthropic)
    KimiProvider::forward
      messages_protocol::forward --> Moonshot /coding/v1/messages (Anthropic SSE)
      [flag ON] wrap stream: SSE parse -> AnthropicNoopFilter -> SSE re-emit
  <-- filtered Anthropic SSE (no-op tool_use dropped, stop_reason fixed)
```

## Error handling

- **Fail open everywhere.** Any JSON/SSE parse error → forward the original
  bytes unchanged. The filter must never turn a working response into a broken
  one; the worst acceptable outcome is "filter didn't fire this time".
- Non-2xx upstream responses are already buffered upstream and are **not**
  sanitized (nothing to filter; error bodies pass through).

## Testing

- `tool_sanitizer` unit tests:
  - `is_noop_tool_use`: `""`, `"  "`, `":"`, `"true"` → true; `"ls"`, missing
    `command`, non-string `command`, non-`tool_use` block → false.
  - `sanitize_buffered`: drop 1 of 2 tool_use (keep `stop_reason=tool_use`);
    drop the only tool_use (→ `end_turn`); text + no-op tool_use (keep text,
    → `end_turn`); valid tool_use untouched; malformed JSON returned as-is.
  - `AnthropicNoopFilter`: same cases over SSE, including a `command` delivered
    across multiple `input_json_delta` chunks; a valid tool_use flushed verbatim;
    parallel valid + no-op tool blocks in one message.
- Integration test (`wiremock`): stub upstream returning a no-op tool_use
  streaming response; assert the client receives `stop_reason=end_turn` and no
  tool_use block. One test with the flag OFF asserting passthrough is unchanged.
- Config round-trip test: `DbConfigRepository` write→read preserves
  `sanitize_empty_tools`; migration test asserts the new column exists (mirrors
  `v5_adds_provider_thinking_mode_column`).

## Rollback / disable

- Runtime: set the provider's `sanitize_empty_tools` to `false` (admin API /
  TUI) → hot reload → passthrough restored, no restart.
- Code: the flag defaults `false`, so the sanitizer is dead weight when unused;
  the module + wiring can be deleted in one commit without touching behavior of
  any provider that never enabled it.
