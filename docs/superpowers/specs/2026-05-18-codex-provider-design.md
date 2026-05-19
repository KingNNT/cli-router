# Codex Provider Design

Date: 2026-05-18

## Problem

The proxy's `OpenAi` provider sends requests to `api.openai.com/v1/chat/completions`
(the OpenAI Platform API), which requires separate pay-per-token billing. Users with
a ChatGPT Plus/Codex subscription have access to the same models through the Codex
backend at `chatgpt.com/backend-api/codex/responses`, but the proxy cannot reach it.

The Codex backend uses the **Responses API** format (not Chat Completions), requires
Codex OAuth tokens from `~/.codex/auth.json`, and needs an `instructions` field.

## Decision

Add a new `ProviderKind::Codex` with its own `CodexProvider` adapter that:

1. Accepts Chat Completions requests from clients (unchanged client interface)
2. Translates them to Responses API format internally
3. Sends to `chatgpt.com/backend-api/codex/responses` via HTTP SSE
4. Translates Responses API SSE events back to Chat Completions SSE chunks
5. Reuses existing `CodexAuto` auth (`~/.codex/auth.json`)

## Architecture

```
Client (Chat Completions format)
    |
    v
RoutingProvider
    |
    v
CodexProvider (ProviderKind::Codex)
    |  native_format() = ApiFormat::OpenAI
    |  auth = CodexAuto -> AuthHeader::OAuth
    |
    +-- forward_openai()
    |      1. Parse Chat Completions JSON body
    |      2. Translate to Responses API payload
    |      3. POST to {base_url}/responses
    |      4. Receive Responses API SSE stream
    |      5. Translate SSE events to Chat Completions SSE chunks
    |      6. Return UpstreamResponse::Streaming
    v
chatgpt.com/backend-api/codex/responses
```

## New Types

### `ProviderKind::Codex`

```rust
// config.rs
pub enum ProviderKind {
    Anthropic,
    Zai,
    DeepSeek,
    OpenAi,
    Codex,  // NEW
}
```

Aliases: `"codex"`. DB column `kind = "codex"`.

### `CodexProvider`

```rust
// adapters/providers/codex.rs (new file)
pub struct CodexProvider {
    pub(crate) base_url: String,   // "https://chatgpt.com/backend-api/codex"
    pub(crate) http: reqwest::Client,
    pub(crate) auth: AuthHeader,
}
```

Default base URL: `https://chatgpt.com/backend-api/codex`.

Constructors mirror existing providers: `new()`, `with_base_url()`, `with_auth()`,
`configure()`.

### Provider Trait Implementation

| Method | Return / Behavior |
|--------|-------------------|
| `name()` | `"codex"` |
| `native_format()` | `ApiFormat::OpenAI` |
| `forward()` | Returns error — Codex doesn't speak Anthropic format |
| `forward_openai()` | Translates Chat Completions -> Responses API, forwards, translates back |
| `usage_parser_openai()` | Standard OpenAI SSE usage parser (reused from `messages_protocol`) |
| `parse_usage_json_openai()` | Standard OpenAI usage JSON parser (reused) |

## Request Translation: Chat Completions -> Responses API

The `forward_openai()` method receives a Chat Completions JSON body and translates it:

| Chat Completions field | Responses API field | Notes |
|------------------------|---------------------|-------|
| `messages` | `input` | Same message objects, wrapped as `{"type":"message","role":...,"content":[...]}` |
| `model` | `model` | Passthrough |
| `stream` | `stream` | Passthrough |
| `tools` | `tools` | Passthrough |
| `tool_choice` | `tool_choice` | Passthrough |
| `temperature` | `temperature` | Passthrough |
| `max_tokens` | `max_output_tokens` | Renamed |
| `reasoning_effort` | `reasoning.effort` | Nested under `reasoning` |
| (n/a) | `include` | Always `["reasoning.encrypted_content"]` |
| (n/a) | `store` | Always `false` |
| (n/a) | `instructions` | Required by Codex backend; minimal default |

### Instructions Requirement

The Codex backend returns `400 "Instructions are not valid"` if `instructions` is
missing. The provider sends a minimal default:

```json
"instructions": "You are a helpful assistant."
```

If the Chat Completions request contains a `system` or `developer` message, it is
extracted from the `messages` array and placed into `instructions` instead.

### Message Translation

Chat Completions messages are translated to Responses API `input` items:

```
{"role": "system", "content": "..."}     -> extracted to "instructions"
{"role": "developer","content": "..."}  -> extracted to "instructions" (merged if both present)
{"role": "user", "content": "..."}      -> {"type":"message","role":"user","content":[{"type":"input_text","text":"..."}]}
{"role": "assistant", "content": "..."} -> {"type":"message","role":"assistant","content":[{"type":"output_text","text":"..."}]}
{"role": "assistant","tool_calls":[...]} -> {"type":"message","role":"assistant","content":[{"type":"function_call",...}]}
{"role": "tool","content":"..."}         -> {"type":"message","role":"tool","content":[{"type":"function_call_output",...}]}
```

## Response Translation: Responses API SSE -> Chat Completions SSE

A streaming adapter wraps the upstream `Bytes` stream, parses Responses API SSE
events line-by-line, and yields Chat Completions SSE chunks.

### Event Mapping

| Responses API event | Action |
|---------------------|--------|
| `response.created` | Skip |
| `response.in_progress` | Skip |
| `response.output_item.added` | Skip |
| `response.content_part.added` | Skip |
| `response.output_text.delta` `{"delta":"hi"}` | Emit `{"choices":[{"index":0,"delta":{"content":"hi"}}]}` |
| `response.output_text.done` | Skip |
| `response.output_text.added` | Skip |
| `response.reasoning.delta` | Skip (not exposed in Chat Completions) |
| `response.reasoning.done` | Skip |
| `response.function_call_arguments.delta` | Emit tool_calls delta chunk |
| `response.function_call_arguments.done` | Skip |
| `response.completed` with `usage` | Emit final chunk with usage stats |
| `response.done` | Emit `[DONE]` |
| Error events | Emit as Chat Completions error |

### Usage Mapping

Responses API `usage` object -> Chat Completions `usage`:

| Responses API | Chat Completions |
|---------------|-----------------|
| `input_tokens` | `prompt_tokens` |
| `output_tokens` | `completion_tokens` |
| `total_tokens` | `total_tokens` |
| `input_tokens_details.cached_tokens` | `prompt_tokens_details.cached_tokens` |

### Buffered (non-streaming) Responses

For non-streaming requests (`stream: false` or omitted):
1. Send request without `stream: true`
2. Receive full Responses API JSON body
3. Extract text output, tool calls, usage
4. Construct Chat Completions JSON response
5. Return `UpstreamResponse::Buffered`

## Auth & Token Refresh

No new auth type needed. The existing `CodexAuto` auth config already:
- Reads `~/.codex/auth.json`
- Constructs `AuthHeader::OAuth { access_token, refresh_token, expires_at_ms }`
- Background refresh in `token_refresh.rs` re-reads `auth.json` every 60s

The `CodexProvider` uses the same `AuthHeader` for `Authorization: Bearer <token>`.

The builder (`build_leaf`) maps `ProviderKind::Codex` + `CodexAuto` the same way it
does for `ProviderKind::OpenAi`.

## HTTP Headers

The Codex backend expects:
- `Authorization: Bearer <access_token>`
- `Content-Type: application/json`

No special headers like `anthropic-beta` (unlike Anthropic OAuth). The provider
sends standard bearer auth, stripping any incoming auth headers from the client
(consistent with existing provider behavior).

## Config & DB Schema

### DB providers table

| Column | Value for Codex |
|--------|----------------|
| `kind` | `"codex"` |
| `base_url` | `NULL` or custom (default: `https://chatgpt.com/backend-api/codex`) |
| `openai_base_url` | `NULL` (not used by Codex provider) |
| `auth_type` | `"codex_auto"` |
| `auth_access_token` etc. | `NULL` (CodexAuto reads from file) |

### Routing

Existing routing rules work unchanged. Example DB config:

```sql
INSERT INTO routing_rules (priority, provider, model_glob, strategy, fallback)
VALUES (1, 'codex', 'openai/*', 'failover', '');
```

## Files Changed

| File | Change |
|------|--------|
| `adapters/providers/codex.rs` | **NEW** — `CodexProvider`, request/response translation (~250-350 lines) |
| `adapters/providers/mod.rs` | Add `mod codex;` and re-export |
| `config.rs` | Add `ProviderKind::Codex` variant, alias `"codex"` in parser |
| `adapters/providers/builder.rs` | Handle `ProviderKind::Codex` in `build_leaf()` and `build_account_usage()` |
| `adapters/storage/db_config.rs` | Parse `"codex"` kind in `parse_kind()`, serialize in `kind_to_str()` |
| `proxy-tui/src/app.rs` | Display "codex" as provider kind option |
| `proxy-tui/src/ui.rs` | Show codex provider info in detail view |

### Files NOT Changed

- `adapters/providers/token_refresh.rs` — CodexAuto handling already works
- `adapters/oauth/openai.rs` — `read_auth_json()` already works
- `application/ports/provider.rs` — No trait changes needed
- `frameworks/handler.rs` — No changes, routing dispatches via Provider trait

## Testing Strategy

### Unit Tests (in `codex.rs`)

1. **Request translation** — Chat Completions JSON -> Responses API JSON
   - Simple user message
   - System message extraction to `instructions`
   - Multi-turn conversation
   - Tool calls / function calling
   - `reasoning_effort` mapping
   - `max_tokens` -> `max_output_tokens` rename

2. **Response translation** — Responses API SSE lines -> Chat Completions SSE chunks
   - Text delta events
   - Tool call delta events
   - Usage in `response.completed`
   - `[DONE]` terminator
   - Unknown event types (graceful skip)

3. **Buffered response translation** — Full Responses API JSON -> Chat Completions JSON

4. **Provider construction** — Default base URL, auth configuration

### Integration Tests

Test via `TestProvider` use case (existing pattern):
1. Mock Codex backend HTTP server returning Responses API format
2. Send Chat Completions request through proxy
3. Verify Chat Completions response comes back

## Out of Scope

- **Responses API passthrough** — Only Chat Completions is exposed to clients. Direct
  Responses API proxying is not in scope.
- **WebSocket transport** — HTTP SSE only. The Codex backend supports both, but SSE
  is sufficient for a proxy and matches existing infrastructure.
- **Image generation** — The Codex backend supports image generation via hosted tools,
  but this is not in the initial scope.
- **Audio transcription** — Not in scope.
- **Model listing** — Not in scope. Clients should know which models are available.

## Risks and Mitigations

| Risk | Mitigation |
|------|-----------|
| Codex backend API is undocumented and may change | Translation layer is isolated; changes only affect `codex.rs` |
| `instructions` requirement may change | Minimal default instructions; logged clearly if 400 occurs |
| ChatGPT subscription limits may cause errors | Errors pass through as standard OpenAI error responses |
| Token refresh relies on `codex` CLI updating `auth.json` | Already working; background task re-reads every 60s |
