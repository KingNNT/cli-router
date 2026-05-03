# Cross-format Translation Design Spec

## Goal

Allow any client to call any upstream regardless of native API format. Proxy translates request body, response body, and SSE stream events between Anthropic and OpenAI formats when client format and provider native format differ. Scope is bidirectional Anthropic ↔ OpenAI for text + tools + streaming. Multimodal (images, PDFs), Gemini, and other formats are out of scope.

## Why

Currently `cli-router` accepts both `/v1/messages` (Anthropic) and `/v1/chat/completions` (OpenAI) but forwards same-format only. This blocks two real workflows:

- **Claude Code (Anthropic) → Z.ai (OpenAI-compat)**: lets Claude Code use GLM Coding Plan with full cache benefits.
- **OpenCode (OpenAI) → Anthropic (Anthropic)**: lets OpenCode hit Anthropic API key without a separate adapter.

CLIProxyAPI has a full N×M translator matrix; this spec covers the two cells we actually need.

## Trigger — auto-detect from path + provider kind

```rust
let client_fmt = match request_path {
    "/v1/messages"          => ApiFormat::Anthropic,
    "/v1/chat/completions"  => ApiFormat::OpenAI,
};

let provider_fmt = provider_kind.native_format();
//   ProviderKind::Anthropic => ApiFormat::Anthropic
//   ProviderKind::Zai       => ApiFormat::OpenAI

let direction = match (client_fmt, provider_fmt) {
    (a, b) if a == b              => Direction::Passthrough,
    (Anthropic, OpenAI)           => Direction::AnthropicToOpenAI,
    (OpenAI, Anthropic)           => Direction::OpenAIToAnthropic,
};
```

`Passthrough` = today's path, no overhead. Future provider kinds add a `native_format()` impl; no new config knobs.

## Request translation

### Anthropic → OpenAI request

| Anthropic field | OpenAI field | Mapping |
|---|---|---|
| `system` (string) | `messages[0]` `{role:"system", content: <string>}` | direct |
| `system` (array of `{type:"text", text}`) | `messages[0]` `{role:"system", content: text-blocks-joined-with-newline}` | concat text blocks; drop `cache_control` markers |
| `messages[].role` (`user` \| `assistant`) | `messages[].role` | identical |
| `messages[].content` (string) | `messages[].content` (string) | direct |
| `messages[].content` (array of blocks) | mapped per block (see below) | per-block |
| Block `{type:"text", text}` | string concat into `content` | drop `cache_control` |
| Block `{type:"tool_use", id, name, input}` (assistant) | move to `messages[i].tool_calls[]` `{id, type:"function", function:{name, arguments: JSON.stringify(input)}}`; `content` stays text or `null` | flatten tool_use blocks into separate `tool_calls` array |
| Block `{type:"tool_result", tool_use_id, content}` (user) | new `messages[].role = "tool"` message with `{tool_call_id: tool_use_id, content: <flattened content>}` | tool_result content (which may itself be array) → flat text |
| Block `{type:"image", ...}` | **drop with warning** (multimodal out of scope) | log warning, don't fail |
| `tools[].input_schema` | `tools[].function.{name, description, parameters: input_schema}` | wrap with `function` envelope |
| `tool_choice = "auto"` \| `"any"` \| `"none"` | `tool_choice = "auto"` \| `"required"` \| `"none"` | rename `"any"` → `"required"` |
| `tool_choice = {type:"tool", name}` | `tool_choice = {type:"function", function:{name}}` | rewrap |
| `max_tokens` | `max_tokens` | identical (required by Anthropic, optional by OpenAI) |
| `temperature`, `top_p`, `top_k` | `temperature`, `top_p` (drop `top_k`) | OpenAI has no top_k |
| `stop_sequences: [...]` | `stop: [...]` | rename |
| `stream: bool` | `stream: bool` | identical |
| `metadata.user_id` | `user` | rename |
| `cache_control` markers anywhere | **drop** | Z.ai content-based auto-cache; markers ignored |

### OpenAI → Anthropic request

| OpenAI field | Anthropic field | Mapping |
|---|---|---|
| `messages[0]` with `role:"system"` | `system` (string) | extract first system message; later system messages flattened into preceding user message |
| `messages[].role` (`user` \| `assistant`) | `messages[].role` | identical |
| `messages[].content` (string) | `messages[].content` (string) | direct |
| `messages[].content` (array of `{type:"text" \| "image_url"}`) | array of `{type:"text"}` blocks (drop image with warning) | image out of scope |
| `messages[].tool_calls[]` (assistant) | append `{type:"tool_use", id, name, input: JSON.parse(arguments)}` to `content` array | merge with existing text into block array |
| `messages[].role = "tool"` | move to next user message as `{type:"tool_result", tool_use_id: tool_call_id, content}` | OpenAI tool messages become Anthropic tool_result blocks within next user turn |
| `tools[].function.{name, description, parameters}` | `tools[].{name, description, input_schema: parameters}` | unwrap function envelope |
| `tool_choice` mapping | reverse of above |  |
| `max_tokens` (optional) | `max_tokens` (default to 4096 if absent) | required by Anthropic |
| `temperature`, `top_p` | identical |  |
| `stop` | `stop_sequences` | rename |
| `user` | `metadata.user_id` | rename |
| `stream: bool` | `stream: bool` | identical |

## Response translation (non-streaming)

### OpenAI → Anthropic response

| OpenAI | Anthropic |
|---|---|
| `choices[0].message.content` (string) | `content: [{type:"text", text}]` |
| `choices[0].message.tool_calls[]` | `content[]` `{type:"tool_use", id, name, input: JSON.parse(arguments)}` (appended after text) |
| `choices[0].finish_reason: "stop"` | `stop_reason: "end_turn"` |
| `choices[0].finish_reason: "tool_calls"` | `stop_reason: "tool_use"` |
| `choices[0].finish_reason: "length"` | `stop_reason: "max_tokens"` |
| `choices[0].finish_reason: "content_filter"` | `stop_reason: "end_turn"` (closest fit; log warning) |
| `usage.prompt_tokens` | `usage.input_tokens` |
| `usage.completion_tokens` | `usage.output_tokens` |
| `usage.prompt_tokens_details.cached_tokens` | `usage.cache_read_input_tokens` |
| `model` | `model` (identical) |
| `id` | `id` (prefix `msg_` if not already) |

### Anthropic → OpenAI response

Reverse of above. Notable:

- Anthropic `content[]` array → flatten text blocks into single string for `message.content`; tool_use blocks → `message.tool_calls[]`.
- `stop_reason: "tool_use"` → `finish_reason: "tool_calls"`.
- `usage.cache_read_input_tokens` → `prompt_tokens_details.cached_tokens`.
- `usage.cache_creation_input_tokens` → no OpenAI equivalent; folded into `prompt_tokens` total (already counted).

## Streaming translation

### OpenAI → Anthropic stream

State machine fed by upstream OpenAI SSE chunks (`event: data: {...}`), emits Anthropic SSE events.

```
struct State {
    message_id: String,                                 // generated `msg_<uuid>`
    model: String,                                      // captured from first chunk
    text_block_open: bool,                              // index 0 reserved for text
    tool_blocks: HashMap<usize, ToolBlockState>,        // by tool_call index
    next_block_index: usize,
    input_tokens: u64, output_tokens: u64,              // accumulated for message_delta
    cached_tokens: u64,
    stop_reason: Option<&'static str>,
}

struct ToolBlockState { id: String, name: String, anth_block_index: usize, args_buf: String }
```

Event mapping:

| OpenAI chunk | Emit Anthropic |
|---|---|
| First chunk (any) | `event: message_start` with `message: {id, role:"assistant", model, content:[], usage:{input_tokens:0, output_tokens:0}, stop_reason:null, stop_sequence:null}` |
| `delta.content` (text) — first text delta | `event: content_block_start` `{index:0, content_block:{type:"text", text:""}}`; mark `text_block_open=true` |
| `delta.content` (text) — subsequent | `event: content_block_delta` `{index:0, delta:{type:"text_delta", text:<delta>}}` |
| `delta.tool_calls[i]` — first chunk for index `i` | if `text_block_open`: emit `content_block_stop {index:0}`, set `text_block_open=false`; assign new `anth_block_index = next_block_index++`; emit `content_block_start {index:anth_block_index, content_block:{type:"tool_use", id, name, input:{}}}` |
| `delta.tool_calls[i].function.arguments` — subsequent | emit `content_block_delta {index:anth_block_index, delta:{type:"input_json_delta", partial_json:<delta>}}`; append to `args_buf` |
| Final chunk with `usage` | capture for message_delta |
| Final chunk with `finish_reason` | emit `content_block_stop` for each open block (text + every tool); emit `event: message_delta {delta:{stop_reason, stop_sequence:null}, usage:{output_tokens, cache_read_input_tokens}}`; emit `event: message_stop` |
| OpenAI `[DONE]` sentinel | (no-op; message_stop already emitted) |

Edge cases:
- Multiple tool_calls interleaved → each gets its own block index, args buffered independently.
- finish_reason without prior delta → still emit message_start + message_delta + message_stop with empty content.
- Upstream error mid-stream → emit Anthropic `event: error` with body, then close.

### Anthropic → OpenAI stream

Reverse direction. Anthropic emits 6 event types; OpenAI uses one (`chat.completion.chunk`) plus `[DONE]`.

```
struct State {
    chunk_id: String,           // = anthropic message id
    model: String,
    sent_role: bool,
    open_tool_idx: Option<usize>,   // current tool index emitted
    next_tool_idx: usize,
    finish_reason: Option<&'static str>,
}
```

Event mapping:

| Anthropic event | Emit OpenAI |
|---|---|
| `message_start` | first chunk: `{id, model, choices:[{index:0, delta:{role:"assistant"}}], finish_reason:null}`; mark `sent_role=true` |
| `content_block_start {type:"text"}` | (no emit; text deltas come next) |
| `content_block_delta {type:"text_delta", text}` | `{choices:[{delta:{content:text}}]}` |
| `content_block_start {type:"tool_use", id, name}` | assign `tool_idx = next_tool_idx++`; emit `{choices:[{delta:{tool_calls:[{index:tool_idx, id, type:"function", function:{name, arguments:""}}]}}]}` |
| `content_block_delta {type:"input_json_delta", partial_json}` | `{choices:[{delta:{tool_calls:[{index:open_tool_idx, function:{arguments:partial_json}}]}}]}` |
| `content_block_stop` | (no emit; track open tool closed) |
| `message_delta {stop_reason, usage}` | capture for final chunk |
| `message_stop` | emit final chunk: `{choices:[{delta:{}, finish_reason:<mapped>}], usage:{prompt_tokens, completion_tokens, prompt_tokens_details:{cached_tokens}}}`; then `data: [DONE]\n\n` |

stop_reason mapping inverse of non-streaming.

## Architecture

```
crates/proxy/src/adapters/translation/
├── mod.rs                          # NEW: Direction enum, dispatch entry, ApiFormat
├── usage.rs                        # NEW: bidirectional usage stat mapper
├── anthropic_to_openai/
│   ├── mod.rs
│   ├── request.rs                  # NEW: AnthropicRequest -> OpenAiRequest
│   ├── response.rs                 # NEW: OpenAiResponse -> AnthropicResponse (note: this dir is named by client direction)
│   └── stream.rs                   # NEW: OpenAI stream chunks -> Anthropic SSE events
└── openai_to_anthropic/
    ├── mod.rs
    ├── request.rs                  # NEW
    ├── response.rs                 # NEW: AnthropicResponse -> OpenAiResponse
    └── stream.rs                   # NEW: Anthropic SSE events -> OpenAI chunks

crates/proxy/src/adapters/providers/messages_protocol.rs
                                    # edit: take Direction, wrap forward fn
crates/proxy/src/frameworks/handler.rs
                                    # edit: derive Direction, pass into HandleMessages
crates/proxy/src/application/handle_messages.rs
                                    # edit: thread Direction through use case
```

Naming: the directory `anthropic_to_openai/` holds code for the client direction "client speaks Anthropic, upstream speaks OpenAI" — so `request.rs` translates Anthropic → OpenAI for outbound, `response.rs` translates OpenAI → Anthropic for inbound. Symmetric for `openai_to_anthropic/`.

## Type-driven translation

Use `serde_json::Value` for request/response bodies. Translators take `&Value` → produce `Value`. Don't model full schemas as Rust structs — too brittle, OpenAI/Anthropic fields churn. Strategy:

- **Whitelist key fields**: known fields are remapped per the tables above.
- **Best-effort passthrough**: unknown fields at top level are dropped with a debug log (we don't know if they're safe to forward to a different provider).
- **Validate minimally**: ensure `messages` is array, role is known, etc. Reject with 400 on structural failure.

Streaming: parse SSE chunks line-by-line (`event:` + `data:` pairs), feed to FSM as `serde_json::Value`. Emit translated chunks as raw bytes to existing `TeedStream`.

## Errors

Add to `adapters::AdapterError`:

```rust
pub enum AdapterError {
    ...,
    Translation(TranslationError),
}

pub enum TranslationError {
    InvalidRequest { field: &'static str, reason: String },
    UnsupportedFeature { feature: &'static str },         // e.g. images
    StreamProtocol { reason: String },
}
```

Mapping in `frameworks/error.rs`:
- `InvalidRequest` → 400 with body matching client format (Anthropic-style or OpenAI-style error envelope based on path).
- `UnsupportedFeature` → 200 with warning logged, drop unsupported parts (don't fail the request).
- `StreamProtocol` mid-stream → emit `error` event in client format, close stream.

## Usage capture

Existing `AnthropicSseParser` and `OpenAiSseParser` (in `adapters/usage/`) read upstream SSE events. With translation, the streaming translator IS the path through which usage flows — capture usage inside the stream FSM (`message_delta` for Anthropic, last chunk for OpenAI) and write to DB as today.

`requests` table already has `cache_read_tokens`, `cache_creation_tokens`. No schema change.

## TUI surface

Add to recent requests view a `xform` column:
- `—` for passthrough
- `A→O` for Anthropic-to-OpenAI
- `O→A` for OpenAI-to-Anthropic

Requires nullable `translation_direction TEXT` column on `requests` table (one-line schema migration in `adapters/storage/`).

Status panel adds:
```
Translation: bidirectional ANT↔OAI | done: 1247 | failed: 3
```

`/admin/status` returns translation stats; TUI polls.

## Files to change

| File | Change |
|------|--------|
| `crates/proxy/src/adapters/translation/...` | **NEW** (8 files) — see Architecture |
| `crates/proxy/src/adapters/providers/messages_protocol.rs` | Wrap forward functions to apply translation |
| `crates/proxy/src/application/handle_messages.rs` | Thread `Direction` through |
| `crates/proxy/src/frameworks/handler.rs` | Derive `Direction` from path + provider; pass into use case |
| `crates/proxy/src/adapters/providers/{anthropic,zai}.rs` | Implement `native_format()` |
| `crates/proxy/src/adapters/storage/mod.rs` | Schema add `translation_direction` column; migration |
| `crates/proxy/src/adapters/usage/{anthropic_sse, openai_sse}.rs` | Hook usage capture into stream FSM (or share parser via composition) |
| `crates/proxy/src/application/errors.rs` + `frameworks/error.rs` | New `Translation` error variant + 400/streaming-error mapping |
| `crates/proxy-admin-api/src/lib.rs` | Add `translations_completed`, `translations_failed`, `translation_directions` to `StatusResponse` |
| `crates/proxy-tui/src/{ui.rs, app.rs}` | `xform` column + status line |

## Tests

**Per-translator unit tests (`*_request.rs`, `*_response.rs`):**

For each of the 4 files, ≥ 8 fixture-pair tests covering:
- Plain text round-trip
- System as string vs array (Anthropic side)
- Multi-message conversation
- Tool definitions translate (schema mapping)
- Single tool_use call
- Multi tool_use calls in one message
- Tool_result with text content
- Tool_result with array content
- `tool_choice` modes (auto/required/none/named)
- `cache_control` dropped on Anthropic→OpenAI
- `top_k` dropped on Anthropic→OpenAI
- `image` content blocks dropped with warning

**Stream FSM tests (`*_stream.rs`):**

Driver harness: feed a recorded SSE chunk sequence (text fixture in `tests/fixtures/`), assert emit sequence matches expected. Cover:
- Text-only response
- Tool call with args split across 5 chunks
- Two parallel tool calls in one message
- Text + tool_use mixed
- finish_reason at last chunk
- Mid-stream upstream error
- Empty content (zero deltas, just message_start + message_stop)

**Integration (wiremock):**
- POST `/v1/messages` → upstream Z.ai (mocked) → response translated back to Anthropic
- POST `/v1/chat/completions` → upstream Anthropic (mocked) → response translated back to OpenAI
- Streaming variants of both
- Verify `requests` table row has correct `translation_direction`

## What stays the same

- Sticky-auth picker, quota check, cooldown lifecycle — all run before translation kicks in.
- Provider-side auth header injection.
- Wire format: HTTP + SSE; no protocol invention.
- Hot reload, env var interpolation.
- SQLite usage logging columns (one column added; no replays).

## Out of scope (YAGNI)

- Multimodal: images, PDFs, audio. Drop with warning on input; out of scope on output (upstreams may emit but we don't translate).
- Gemini, Bedrock, Vertex formats.
- Files API, Batch API endpoints.
- OpenAI Responses API (different shape from chat-completions).
- Anthropic `cache_control` synthesis when going OpenAI→Anthropic (we don't fabricate markers; user can add explicitly via Anthropic format).
- Function-calling extensions (JSON mode, structured outputs schemas) — basic tools only.
- Token counting / pre-flight budget translation.

## Risk and rollout

- Translation is opt-in by **routing**, not config: if no provider has different native format than client path, no translation runs. Existing setups untouched.
- `translation_direction` column default NULL → backward compatible queries.
- Translation errors fail closed: 400 to client (don't silently corrupt requests). Streaming errors emit error event then close.
- Performance: serde_json parse + re-serialize on every request, plus per-chunk parse on streaming. Estimated overhead: ~1-2 ms per non-streaming request, ~50-200 µs per chunk. Acceptable for IDE/CLI workloads.
- Largest unknown: tool_calling edge cases. Plan: ship with extensive fixture tests for known shapes, expect to patch as new edge cases appear (Claude Code / OpenCode emit slightly different patterns).
