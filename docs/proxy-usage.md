# Proxy Usage Guide

Complete guide to configuring and running the `cli-router` proxy daemon.

## Quick Start

```bash
# Set your API key and run
export ANTHROPIC_API_KEY="sk-ant-your-key"
cargo run -p proxy

# Point your tools at it
ANTHROPIC_BASE_URL=http://127.0.0.1:8787 claude
```

This uses a default config: single Anthropic provider, passthrough auth (forwards the client's key).

The proxy accepts both **Anthropic** (`/v1/messages`) and **OpenAI** (`/v1/chat/completions`) request formats. Point any compatible tool at it — Claude Code, OpenCode, Cursor, etc.

---

## Configuration

The proxy reads `~/.config/cli-router/config.toml` on startup. If the file doesn't exist, it falls back to environment variables.

### Environment variable override:

```bash
export CLI_ROUTER_CONFIG="/path/to/custom-config.toml"
cargo run -p proxy
```

---

## Auth Types

### API Key (permanent, never expires)

```toml
[[providers]]
name = "anthropic"
kind = "anthropic"
auth = { type = "api_key", value = "sk-ant-your-key" }
```

Or use env var interpolation:

```toml
[[providers]]
name = "anthropic"
kind = "anthropic"
auth = { type = "api_key", value = "${ANTHROPIC_API_KEY}" }
```

### Bearer Token (manual, no auto-refresh)

If you have a long-lived token from `claude setup-token`:

```toml
[[providers]]
name = "anthropic"
kind = "anthropic"
auth = { type = "bearer", value = "sk-ant-oat-..." }
```

### Anthropic OAuth (auto-refresh)

Set up via the TUI (recommended) — see [OAuth Setup](#oauth-setup) below. The resulting config:

```toml
[[providers]]
name = "anthropic"
kind = "anthropic"
auth = { type = "anthropic_oauth", access_token = "sk-ant-oat-...", refresh_token = "sk-ant-oar-...", expires_at_ms = 1746300000000 }
```

The daemon automatically refreshes tokens when they're within 5 minutes of expiry.

### Passthrough (forward client's key)

```toml
[[providers]]
name = "anthropic"
kind = "anthropic"
auth = { type = "passthrough" }
```

The proxy forwards whatever `x-api-key` or `Authorization` header the client sends.

---

## Multi-Provider Setup

```toml
[[providers]]
name = "anthropic"
kind = "anthropic"
auth = { type = "api_key", value = "${ANTHROPIC_API_KEY}" }

[[providers]]
name = "zai"
kind = "zai"
auth = { type = "api_key", value = "${ZAI_API_KEY}" }
openai_base_url = "https://api.z.ai/api/paas/v4"

[[routing]]
match = { model = "glm-*" }
provider = "zai"

[[routing]]
match = { model = "*" }
provider = "anthropic"
fallback = ["zai"]
```

- `glm-*` models → Z.ai
- Everything else → Anthropic, with Z.ai as fallback on 5xx

### Z.ai Coding Plan vs Pay-As-You-Go

Z.ai routes requests to different billing ledgers based on the auth header:

| Auth header sent | Billed against |
|---|---|
| `Authorization: Bearer <token>` | Coding Plan quota |
| `x-api-key: <token>` | Pay-as-you-go balance |

If you're on the **GLM Coding Plan**, you need *both* of:

1. `type = "bearer"` — not `type = "api_key"`. Z.ai routes the two headers to different ledgers.
2. `openai_base_url = "https://api.z.ai/api/coding/paas/v4"` — the `coding/` prefix is what selects the Coding Plan ledger for OpenAI-format requests. Without it, requests hit the PAYG ledger.

```toml
[[providers]]
name = "zai"
kind = "zai"
auth = { type = "bearer", value = "${ZAI_CODING_PLAN_TOKEN}" }
openai_base_url = "https://api.z.ai/api/coding/paas/v4"
```

The default Anthropic-compatible URL (`https://api.z.ai/api/anthropic`) already routes to the Coding Plan when authenticated with Bearer, so no override is needed for clients that speak Anthropic format.

| Endpoint | Coding Plan | PAYG |
|---|---|---|
| `https://api.z.ai/api/anthropic/v1/messages` | ✓ | ✓ |
| `https://api.z.ai/api/coding/paas/v4/chat/completions` | ✓ | — |
| `https://api.z.ai/api/paas/v4/chat/completions` | — | ✓ |

A Coding Plan request that lands on the PAYG endpoint surfaces as `1113 Insufficient balance or no resource package` even when the plan has quota.

Pay-as-you-go users can use either header on `/api/paas/v4`; `type = "api_key"` matches the Anthropic SDK default.

### DeepSeek

DeepSeek provides an OpenAI-compatible endpoint. Configure it with `kind = "deepseek"`:

```toml
[[providers]]
name = "deepseek"
kind = "deepseek"
auth = { type = "bearer", value = "${DEEPSEEK_API_KEY}" }

[[routing]]
match = { model = "deepseek-*" }
provider = "deepseek"
```

DeepSeek only speaks the OpenAI chat-completions format — requests via `/v1/messages` (Anthropic format) will return an error. Route all DeepSeek traffic through `/v1/chat/completions`.

The default base URL is `https://api.deepseek.com/v1`. Override it with `base_url` if needed.

| Auth config | OpenAI format (`/v1/chat/completions`) |
|---|---|
| `type = "bearer"` | `Authorization: Bearer <key>` |
| `type = "api_key"` | auto-converted to `Bearer` |

The `bearer` auth type is recommended. Use `api_key` only if your key comes from a source (e.g. the Anthropic SDK) that defaults to `x-api-key` headers.

Available models: `deepseek-v4-pro`, `deepseek-v4-flash`.

---

## Multiple Accounts

Define multiple providers with the same `kind` but different names:

```toml
[[providers]]
name = "anthropic-work"
kind = "anthropic"
auth = { type = "api_key", value = "${ANTHROPIC_WORK_KEY}" }

[[providers]]
name = "anthropic-personal"
kind = "anthropic"
auth = { type = "api_key", value = "${ANTHROPIC_PERSONAL_KEY}" }

[[providers]]
name = "anthropic-oauth"
kind = "anthropic"
auth = { type = "anthropic_oauth", access_token = "...", refresh_token = "...", expires_at_ms = 1746300000000 }
```

---

## Load Balancing

### Strategy: `round_robin`

Rotate requests across all providers in the pool. On 429/5xx, skip that provider and try the next.

```toml
[[routing]]
match = { model = "*" }
strategy = "round_robin"
provider = "anthropic-work"
fallback = ["anthropic-personal", "anthropic-oauth"]
```

Behavior:

```
Request 1 → anthropic-work
Request 2 → anthropic-personal
Request 3 → anthropic-oauth
Request 4 → anthropic-work    ← rotates back
...
```

When a provider returns 429 (rate limited):
1. That provider is marked as "cooling down" (uses `Retry-After` header, defaults to 60s)
2. The next provider in the pool handles the request
3. When cooldown expires, the provider rejoins rotation

When **all** providers are rate limited:
```
HTTP 429 Too Many Requests
Retry-After: 23
{"error":{"type":"rate_limit_error","message":"all 3 providers in round-robin pool are rate-limited","retry_after":23}}
```

### Strategy: `failover` (default)

Always try `provider` first, then `fallback` in order on 5xx/error. This is the default if you don't specify `strategy`.

```toml
[[routing]]
match = { model = "*" }
strategy = "failover"     # optional, this is the default
provider = "anthropic"
fallback = ["zai"]
```

---

## Priority

Control rule evaluation order independent of TOML position. Lower number = higher priority = checked first.

```toml
# Checked SECOND (priority=10)
[[routing]]
match = { model = "glm-*" }
priority = 10
provider = "zai"

# Checked FIRST (priority=1)
[[routing]]
match = { model = "*" }
priority = 1
strategy = "round_robin"
provider = "anthropic-a"
fallback = ["anthropic-b"]
```

Without `priority`, rules are checked in TOML order.

---

## Namespace Routing

Override routing rules by prefixing the model name with a provider name and `/`:

```
<provider-name>/<model>
```

| Client sends | Routed to | Upstream receives |
|---|---|---|
| `zai/glm-5` | provider `zai` | `{"model":"glm-5"}` |
| `anthropic-work/claude-sonnet-4` | provider `anthropic-work` | `{"model":"claude-sonnet-4"}` |
| `glm-5` | normal routing rules | `{"model":"glm-5"}` |

**Namespace always wins** — it bypasses glob-based routing rules entirely. The proxy strips the prefix before forwarding, so the upstream only sees the bare model name.

If the namespace doesn't match any configured provider name, the proxy returns:

```
400 Bad Request: unknown provider namespace 'nonexistent'
```

This is useful when you have multiple providers of the same kind and want explicit control over which one handles a request, or when testing a specific provider without changing routing rules.

---

## OAuth Setup

### Via TUI (recommended)

```bash
# Terminal 1: start the proxy
cargo run -p proxy

# Terminal 2: start the admin TUI
cargo run -p proxy-tui
```

In the TUI:
1. Select a provider → **Edit Auth**
2. Choose **OAuth (Anthropic)**
3. A browser opens to `claude.ai/oauth/authorize`
4. Log in with your Anthropic account
5. Anthropic shows a `code#state` value
6. Copy the code part (before `#`) and paste into the TUI
7. Done — your config is updated with `anthropic_oauth` auth

The daemon handles refresh automatically.

### What happens under the hood

1. Proxy generates PKCE codes (verifier + challenge + state)
2. Browser opens the Anthropic authorize URL
3. After login, Anthropic redirects to a manual-callback page showing the code
4. You paste the code back
5. Proxy exchanges the code for tokens at Anthropic's token endpoint
6. Tokens are saved to config as `anthropic_oauth`
7. Background task refreshes tokens every 60 seconds when within 5 minutes of expiry
8. If a request gets 401, the proxy refreshes and retries once

---

## Admin API

The proxy serves admin endpoints on the same port (`127.0.0.1:8787`):

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/admin/status` | GET | Uptime, request counts by provider/status |
| `/admin/config` | GET | Current config (secrets included — localhost only) |
| `/admin/config` | PUT | Update config + hot reload |
| `/admin/requests/recent?limit=N` | GET | Last N request rows |
| `/admin/usage/summary` | GET | Aggregate usage (daily totals, per-model breakdowns) |
| `/admin/account/usage` | GET | Provider account balances and quota status |
| `/admin/quota/status` | GET | Per-provider quota health (remaining, reset time) |
| `/admin/providers/:name/test` | POST | Ping a provider with a minimal request |
| `/admin/oauth/anthropic/start` | POST | Start OAuth PKCE flow |
| `/admin/oauth/anthropic/complete` | POST | Complete OAuth with pasted code |

### Quick test:

```bash
# Status
curl http://127.0.0.1:8787/admin/status | jq

# Test a provider
curl -X POST http://127.0.0.1:8787/admin/providers/anthropic/test \
  -H "content-type: application/json" \
  -d '{"model":"claude-sonnet-4-20250514"}'
```

---

## Full Config Example

```toml
# ~/.config/cli-router/config.toml

[[providers]]
name = "anthropic-a"
kind = "anthropic"
auth = { type = "anthropic_oauth", access_token = "sk-ant-oat-aaa", refresh_token = "sk-ant-oar-aaa", expires_at_ms = 1746300000000 }

[[providers]]
name = "anthropic-b"
kind = "anthropic"
auth = { type = "api_key", value = "${ANTHROPIC_B_KEY}" }

[[providers]]
name = "anthropic-c"
kind = "anthropic"
auth = { type = "api_key", value = "${ANTHROPIC_C_KEY}" }

[[providers]]
name = "zai"
kind = "zai"
auth = { type = "api_key", value = "${ZAI_API_KEY}" }
openai_base_url = "https://api.z.ai/api/paas/v4"

# Opus models: round-robin across 3 Anthropic accounts
[[routing]]
match = { model = "claude-opus-*" }
priority = 1
strategy = "round_robin"
provider = "anthropic-a"
fallback = ["anthropic-b", "anthropic-c"]

# GLM models: send to Z.ai
[[routing]]
match = { model = "glm-*" }
priority = 2
provider = "zai"

# Everything else: failover from A → B → Z.ai
[[routing]]
match = { model = "*" }
priority = 10
strategy = "failover"
provider = "anthropic-a"
fallback = ["anthropic-b", "zai"]
```

---

## Running

```bash
# Development
cargo run -p proxy

# With debug logging
RUST_LOG=debug cargo run -p proxy

# Release build
cargo build --release --workspace
./target/release/cli-router-proxy
```

The proxy binds to `127.0.0.1:8787` by default. Configure the port:

```toml
port = 9000
```

---

## Verifying It Works

After starting the proxy, here are three ways to confirm it's routing requests correctly.

### 1. Quick health check

```bash
# Is the proxy responding?
curl -s http://127.0.0.1:8787/admin/status | jq
```

A `200` response with uptime and request counts means the proxy is running.

### 2. Test a provider via admin API

```bash
# Sends a minimal request through to the upstream provider
curl -s -X POST http://127.0.0.1:8787/admin/providers/zai/test \
  -H "content-type: application/json" \
  -d '{"model":"glm-5"}' | jq
```

Replace `zai` with your provider name. A successful response confirms the proxy can reach the upstream and authenticate.

### 3. Send a real request through the proxy

The proxy speaks the **Anthropic Messages API** (`/v1/messages`). Both Anthropic and Z.ai providers use this format.

```bash
curl http://127.0.0.1:8787/v1/messages \
  -H "content-type: application/json" \
  -H "x-api-key: dummy" \
  -H "anthropic-version: 2023-06-01" \
  -d '{
    "model": "glm-5",
    "max_tokens": 50,
    "messages": [{"role": "user", "content": "Say hello"}]
  }'
```

> **Note:** The `x-api-key` header is required by the Anthropic API format, but if your provider uses `api_key` auth in the config, the proxy replaces it with your configured key. For `passthrough` auth, the client's key is forwarded as-is.

A successful response looks like:

```json
{
  "id": "msg_...",
  "type": "message",
  "role": "assistant",
  "model": "glm-5.1",
  "content": [{"type": "text", "text": "Hello! How can I help you today?"}],
  "stop_reason": "end_turn",
  "usage": {"input_tokens": 7, "output_tokens": 10}
}
```

#### Point your tools at it

```bash
# Claude Code (Anthropic format)
ANTHROPIC_BASE_URL=http://127.0.0.1:8787 claude

# OpenCode (OpenAI format — use @ai-sdk/openai-compatible provider)
# See "Configure OpenCode" below
```

### 4. Test the OpenAI-compatible endpoint

```bash
curl http://127.0.0.1:8787/v1/chat/completions \
  -H "content-type: application/json" \
  -H "Authorization: Bearer dummy" \
  -d '{
    "model": "zai/glm-5",
    "max_tokens": 50,
    "messages": [{"role": "user", "content": "Say hello"}]
  }'
```

#### Configure OpenCode

Add this to your `opencode.json`:

```json
{
  "provider": {
    "cli-router": {
      "npm": "@ai-sdk/openai-compatible",
      "name": "CLI Router",
      "options": {
        "baseURL": "http://127.0.0.1:8787/v1"
      },
      "models": {
        "zai/glm-5.1": { "name": "z.ai/GLM 5.1" },
        "zai/glm-5": { "name": "z.ai/GLM 5" }
      }
    }
  }
}
```

Run `/connect` in OpenCode, select the `cli-router` provider, and enter any non-empty string as the API key. The proxy replaces it with your configured key.
