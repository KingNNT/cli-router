# OpenAI Chat Completions Endpoint

Date: 2026-05-03

## Summary

Add a `/v1/chat/completions` route to the proxy so that OpenAI-compatible clients
(OpenCode, Cursor, etc.) can use the proxy without needing to speak the Anthropic
Messages API format.

## Motivation

The proxy currently only handles `/v1/messages` (Anthropic format). OpenCode uses
`@ai-sdk/openai-compatible` which sends OpenAI-format requests to
`/v1/chat/completions`. Without this route, OpenCode gets a 404 and reports
"Unauthorized: Missing API key".

## Approach: Per-provider OpenAI base URL (pass-through)

The proxy does **not** translate between formats. Instead, it passes the OpenAI-format
request through to the provider's OpenAI-compatible endpoint. Each provider config
gains an optional `openai_base_url` field that specifies the base URL for OpenAI-format
requests.

## Routing

Two routes share the same routing logic (namespace + glob rules):

| Incoming route | Upstream base URL | Example |
|---|---|---|
| `POST /v1/messages` | `base_url` (Anthropic) | `https://api.z.ai/api/anthropic/v1/messages` |
| `POST /v1/chat/completions` | `openai_base_url` (OpenAI) | `https://api.z.ai/api/paas/v4/chat/completions` |

Namespace routing and body rewriting work identically for both routes.

## Config change

Add `openai_base_url` to `ProviderConfig`:

```toml
[[providers]]
name = "zai"
kind = "zai"
auth = { type = "api_key", value = "${ZAI_API_KEY}" }
openai_base_url = "https://api.z.ai/api/paas/v4"
```

- `openai_base_url` is optional (`Option<String>`).
- If not set, `/v1/chat/completions` requests to that provider return `400`.
- `base_url` remains the Anthropic-format default (unchanged).

## Default `openai_base_url` per provider kind

| Provider Kind | `base_url` (Anthropic) | `openai_base_url` (OpenAI) |
|---|---|---|
| `zai` | `https://api.z.ai/api/anthropic` | `https://api.z.ai/api/paas/v4` |
| `anthropic` | `https://api.anthropic.com` | *(none — not supported)* |

When `openai_base_url` is not explicitly configured, the provider falls back to its
kind-specific default (or `None` if the provider doesn't support OpenAI format).

## Provider trait change

Add a `forward_openai()` method to the `Provider` trait:

```rust
fn forward_openai(
    &self,
    path: &str,
    headers: &HeaderMap,
    body: Bytes,
    streaming: bool,
) -> Pin<Box<dyn Future<Output = Result<UpstreamResponse, ProxyError>> + Send + '_>> {
    Box::pin(async {
        Err(ProxyError::BadRequest(format!(
            "provider '{}' does not support OpenAI chat completions format",
            self.name()
        )))
    })
}
```

Default implementation returns an error. Providers that support OpenAI format
override it.

## Auth for OpenAI-format requests

OpenAI format uses `Authorization: Bearer <key>`. Anthropic format uses
`x-api-key: <key>`. Z.AI's OpenAI endpoint expects `Authorization: Bearer`.

When forwarding to an OpenAI base URL, the provider must convert `ApiKey` auth to
`Bearer` auth so the correct header is sent. This conversion happens in the
`forward_openai()` implementation.

## Handler and use case

Add a new handler `chat_completions` alongside the existing `messages` handler.
Both share the same `HandleMessages` use case. The use case needs to know which
path format to use — simplest approach: the handler passes the path through, and
the routing/provider layer decides based on which `forward_*` method to call.

## Error handling

| Scenario | Error |
|---|---|
| `/v1/chat/completions` → provider without `openai_base_url` | `400: provider 'name' does not support OpenAI chat completions format` |
| `/v1/chat/completions` with no matching provider/rule | `400: no routing rule matches model '...'` (same as existing) |

## Files touched

- `crates/proxy/src/config.rs` — add `openai_base_url` field to `ProviderConfig`
- `crates/proxy/src/adapters/providers/messages_protocol.rs` — add `forward_openai` function
- `crates/proxy/src/adapters/providers/zai.rs` — implement `forward_openai` with `openai_base_url`
- `crates/proxy/src/adapters/providers/anthropic.rs` — use default (returns error)
- `crates/proxy/src/adapters/providers/routing.rs` — implement `forward_openai` for `RoutingProvider`
- `crates/proxy/src/adapters/providers/live.rs` — delegate `forward_openai`
- `crates/proxy/src/adapters/providers/builder.rs` — pass `openai_base_url` to provider constructors
- `crates/proxy/src/application/ports/provider.rs` — add `forward_openai` to `Provider` trait
- `crates/proxy/src/application/use_cases/handle_messages.rs` — handle OpenAI path
- `crates/proxy/src/frameworks/handler.rs` — add `chat_completions` handler
- `crates/proxy/src/frameworks/server.rs` — add route

## Tests

- `chat_completions` handler routes to correct provider with namespace
- `chat_completions` to Anthropic provider returns 400
- Body rewrite (namespace stripping) works for OpenAI path
- Auth conversion: `ApiKey` becomes `Bearer` for OpenAI forwarding
- ZaiProvider uses `openai_base_url` for `forward_openai`
- Default `openai_base_url` for ZAI kind
