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
./target/release/proxy
```

The proxy binds to `127.0.0.1:8787` by default. Configure the port:

```toml
port = 9000
```
