# Technical debt — proxy / TUI work (Phases 1–5)

Snapshot taken 2026-05-03 after shipping Phases 1–5 (multi-provider routing, admin
API, Ratatui admin client, Anthropic OAuth paste flow, hot reload). Each item:
**what**, **why it's debt**, **suggested fix**, **priority**.

Priority legend:
- 🔴 high — affects correctness, security, or core UX
- 🟡 medium — limits a documented feature or risks future breakage
- 🟢 low — polish, nice-to-have, or workspace hygiene

---

## OAuth

### 🔴 No refresh-token logic for Anthropic OAuth (Phase 4 v1)
**What**: `CompleteAnthropicOAuth` saves only `access_token` as `Bearer` auth.
Refresh tokens returned by `console.anthropic.com/v1/oauth/token` are discarded
in `OAuthTokens.refresh_token`.

**Why debt**: Access tokens expire in ~8 hours. After expiry every request 401s
and the user must re-run the full OAuth dance.

**Fix**: New auth variant `AuthConfig::AnthropicOAuth { access_token,
refresh_token, expires_at_ms }`. New port `TokenRefresher`. Provider
middleware (`messages_protocol::forward`) checks expiry before each request
and refreshes proactively, plus retries once on 401. Persist refreshed
tokens back to config (refresh tokens rotate on use).

**Files**: `crates/proxy/src/config.rs`, `crates/proxy/src/adapters/oauth/anthropic.rs`,
`crates/proxy/src/adapters/providers/messages_protocol.rs`,
`crates/proxy/src/application/use_cases/admin.rs`.

### 🟡 No bootstrap → permanent API key (OpenCode mode #2)
**What**: We do PKCE OAuth and save the bearer token directly. OpenCode also
supports using the OAuth access token *once* to call Anthropic's
`POST /v1/organizations/api_keys` and save the resulting permanent API key.

**Why debt**: Permanent keys avoid the refresh problem entirely and aren't
grey-zone (Anthropic explicitly supports `org:create_api_key` scope). This is
the safer long-term default per our research notes.

**Fix**: After successful `exchange_code`, optionally call the API-key
creation endpoint with the access token, save the result as
`AuthConfig::ApiKey`, discard tokens. Surface as a TUI choice: "Use OAuth
session (8h, refresh)" vs "Bootstrap permanent key (recommended)".

**Files**: same as above + new TUI modal step.

### 🟡 Anthropic OAuth client_id is grey-zone
**What**: We reuse Claude Code's public `client_id` (`9d1c250a-…`).

**Why debt**: Anthropic may rotate or restrict it without notice. If they
add User-Agent checks or device attestation, the flow breaks silently.

**Fix**: No clean fix without official Anthropic OAuth integration. Mitigations:
(a) document the risk in `proxy-tui` Edit modal (already done in Phase 4),
(b) prefer the bootstrap → permanent key path so a single working OAuth gives
us a long-lived API key, (c) keep manual `claude setup-token` paste working
as a fallback.

### 🟢 PKCE session store has no TTL
**What**: `OAuthSessionStore` only evicts when capped at 32 entries or when
`take()` consumes one. Abandoned flows hang around indefinitely.

**Why debt**: Memory is bounded by the cap, but a stale `state_id` from
yesterday could (theoretically, given collision risk) match a new request.
In practice UUIDv4 makes this near-impossible.

**Fix**: Add `created_at: Instant` to each `PkceCodes`, evict any entry older
than 10 minutes on every `insert`/`take`.

**Files**: `crates/proxy/src/adapters/oauth/anthropic.rs`.

---

## Security / admin API

### 🔴 Admin API has no auth — relies on 127.0.0.1 binding only
**What**: `/admin/*` routes accept any localhost connection. No bearer token,
no per-tenant identity. Documented in `frameworks/admin.rs` doc comment.

**Why debt**: Anyone who can open a TCP connection to `127.0.0.1:8787` can
read all credentials, mint OAuth flows, swap routing, etc. On shared servers
or compromised dev machines, that's everything. Also blocks SSH-tunnel /
Tailscale exposure use cases.

**Fix**: Optional bearer token. Generate on first daemon start, save to
config, require in `Authorization: Bearer …` for all `/admin/*` routes.
TUI reads from config (or env var). Skip if `bind=127.0.0.1` and a config
flag opts out.

**Files**: `crates/proxy/src/frameworks/admin.rs` (axum middleware layer),
`crates/proxy/src/config.rs` (admin token field), TUI client.

### 🟡 GET /admin/config returns secrets unredacted
**What**: API keys, OAuth tokens, bearer values come back in plain text.
TUI shows them with simple display redaction (`sk-ant-12…cdef`) but the
wire payload is raw.

**Why debt**: A network log or curl history captures secrets. Also blocks
multi-user admin scenarios where one admin shouldn't see another's keys.

**Fix**: Redact in `GetConfig::execute`, return `AuthPayload::ApiKey {
masked: "sk-ant-12…cdef" }` instead. Add `GET /admin/config/raw` for the TUI
edit flow with explicit `?reveal=true` query that requires admin token
(once we have one). Or — simpler — use a separate dedicated `PUT` payload
that supports `value: KeepExisting | Replace(String)` so the TUI never
needs to read raw values.

**Files**: `crates/proxy-admin-api/src/lib.rs`, `crates/proxy/src/application/use_cases/admin.rs`,
`crates/proxy-tui/src/main.rs`.

### 🟢 Config TOML stored in plaintext on disk
**What**: `~/.config/cli-router/config.toml` contains raw API keys and OAuth
tokens. User accepted this in Phase 1 design.

**Why debt**: A backup, dotfile sync, or accidental git commit leaks creds.

**Fix**: Optional integration with `keyring` crate (macOS Keychain / Linux
Secret Service / Windows Credential Manager). Config keeps a symbolic
reference (`auth = { type = "api_key", source = "keyring:cli-router/anthropic" }`),
adapters resolve at load.

**Files**: `crates/proxy/src/config.rs`, all use cases that touch auth, TUI.

---

## Routing & observability

### 🟡 `provider` column in `proxy.db` reads `"router"` for all routed requests
**What**: `RoutingProvider::name()` returns the static `"router"` string.
`HandleMessages` logs that to the DB. The actual leaf provider that handled
each request is invisible downstream.

**Why debt**: The TUI Status view (`/admin/status` `requests_by_provider`)
just shows `router: N`, not `anthropic: M, zai: K`. Defeats the point of
multi-provider routing for cost analysis.

**Fix**: Two paths.
- (a) Have `RoutingProvider::forward` return the leaf name alongside the
  `UpstreamResponse` (new variant or tuple). `HandleMessages` updates the
  DB row with the actual provider after forward returns.
- (b) Add a thread-local / request-scoped `OnceCell<&'static str>` that
  `RoutingProvider::forward` writes and `HandleMessages` reads.

(a) is cleaner. Requires changing the `Provider::forward` return type
across the trait — touches 4 impls.

**Files**: `crates/proxy/src/application/ports/upstream.rs`,
`crates/proxy/src/adapters/providers/*.rs`,
`crates/proxy/src/application/use_cases/handle_messages.rs`.

### 🟡 Streaming responses can't fall back on 5xx
**What**: `RoutingProvider::forward` only retries fallback chain when the
buffered response status ≥ 500 or transport error. Streaming responses
commit to the chosen leaf even if it returns `Streaming { status: 503 }`.

**Why debt**: User-visible: a flaky upstream serving a streaming 503 won't
be retried. Mitigation today: a tiny client retry usually re-rolls into
buffered fallback path.

**Fix**: When `streaming=true` and we get back `Streaming { status: 5xx }`,
drop the body stream and try next fallback. Document the cancellation
guarantees on `BoxedByteStream` first — they're not strong.

**Files**: `crates/proxy/src/adapters/providers/routing.rs`.

### 🟢 No client cancellation propagation
**What**: When a client drops mid-stream, the upstream connection stays open
until upstream finishes or times out.

**Why debt**: Wasted upstream tokens (we billed for output the user didn't
consume). At low volume this is noise.

**Fix**: `TeedStream` should carry a cancellation token; on stream drop,
abort the upstream future. Standard reqwest streams do this when the
`Response` is dropped, so the gap may be in axum's `Body::from_stream`
behavior. Audit and add a unit test.

**Files**: `crates/proxy/src/frameworks/stream.rs`.

---

## Hot reload (Phase 5)

### 🟡 Disk and in-memory can briefly diverge if rebuild fails post-write
**What**: `UpdateConfig::execute` writes the file before calling
`live.reload()`. If reload errors (it shouldn't post `validate()`, but is
defensive), the file holds the new config but the daemon serves the old
provider tree until restart.

**Why debt**: User sees "saved" but their actual routing didn't change.
Subtle, hard to debug.

**Fix**: Reorder — build in memory first, swap atomically, only then
persist. Or: rollback the file on reload failure.

**Files**: `crates/proxy/src/application/use_cases/admin.rs`
(`UpdateConfig`, `CompleteAnthropicOAuth`).

### 🟢 `LiveProvider::reload` is not concurrency-tested
**What**: Two simultaneous PUTs could interleave: read old config, write
config A, build A, read config B, write config B, build B but swap A.

**Why debt**: Admin is single-user in practice, low risk. But could surprise
during testing or scripted ops.

**Fix**: Add a `reload_lock: Mutex<()>` held across the read-write-build-swap
sequence. Or use a single-writer actor pattern.

**Files**: `crates/proxy/src/application/use_cases/admin.rs`.

---

## TUI

### 🟡 No integration tests against a running daemon
**What**: `proxy-tui` has zero test coverage. Phase 3 shipped without any
tests that exercise the full client.

**Why debt**: Refactors break silently. Specifically: the AdminClient HTTP
shapes can drift from server-side handlers if proxy-admin-api types ever
get partially synced.

**Fix**: Add `crates/proxy-tui/tests/integration.rs` that spins up a real
proxy daemon (using `proxy::serve` like the existing proxy integration
tests), then exercises `AdminClient` against it. ~5 happy-path tests
covering status, config get/put, recent, test-provider, oauth start.

### 🟡 Edit modal has no key-masking while typing
**What**: User pastes a 100-char API key and it shows in plaintext in the
modal. Onlooking shoulder-surfers see it.

**Why debt**: Common UX expectation for credential input. Also TTY scrollback
captures it.

**Fix**: Mask input as `*` while typing, with a `Ctrl+R` reveal toggle.
Display existing values already-redacted; add explicit "reveal" key in
view mode for power users.

**Files**: `crates/proxy-tui/src/ui.rs` (Edit modal),
`crates/proxy-tui/src/main.rs` (keymap).

### 🟢 No "claude setup-token" helper in TUI
**What**: Anthropic's official long-lived OAuth token CLI is supported only
via manual paste into Bearer auth. TUI doesn't acknowledge or guide.

**Why debt**: Users who don't trust the grey-zone PKCE flow have to know
about `claude setup-token` separately.

**Fix**: When user picks Bearer in Edit modal, show help: "Run `claude
setup-token` for a 1-year Anthropic OAuth token, paste here." Optionally
detect prefix `sk-ant-oat…` and warn if it doesn't match.

### 🟢 No concurrent-admin protection (ETag-style)
**What**: Two TUI instances editing simultaneously: last write wins, no
warning.

**Why debt**: Lost updates.

**Fix**: `GET /admin/config` returns an `ETag` (hash of current config).
`PUT` checks `If-Match`. TUI surfaces conflicts and prompts re-fetch.

---

## Workspace / docs

### 🟡 Specs out of date
**What**: `docs/superpowers/specs/2026-05-02-workspace-and-proxy-mvp-design.md`
and `2026-05-02-proxy-clean-architecture-design.md` describe a 3-crate
workspace with single-upstream proxy. We now have 5 crates and four major
proxy responsibilities (data path, admin API, OAuth, hot reload).

**Why debt**: Future contributors (or future-you) read stale docs and make
wrong assumptions about boundaries.

**Fix**: Either update the existing specs in place with a "Phase 1–5
amendment" section, or write a new spec
`docs/superpowers/specs/2026-05-03-proxy-admin-and-oauth-design.md` that
supersedes the relevant sections. CLAUDE.md needs a pointer update either
way.

### 🟢 No README update for the TUI binary
**What**: Top-level CLAUDE.md mentions `analysis` and `proxy` binaries but
not `proxy-tui`. No README at all currently.

**Fix**: Add a minimal README with: what each crate does, common commands,
link to specs, link to this debt doc.

### 🟢 No usage docs / examples for `proxy-admin-api` consumers
**What**: A third-party tool wanting to talk to the admin API has to read
the `lib.rs` file directly.

**Fix**: Inline doc comment with one curl example per endpoint.

---

## Cross-cutting / future features

### 🟡 No per-key quota or rate limiting
**What**: We don't enforce any per-credential rate limits. LiteLLM-style
"virtual keys" would let a user hand out scoped credentials with budgets.

**Why debt**: Limits the proxy's usefulness for shared dev environments.
Single-user (current target) doesn't need this.

**Fix**: New domain entity `Quota`, new `RateLimiter` adapter (token bucket
in `requests` table or in-memory), new admin endpoints to issue and
inspect virtual keys. Big feature — Phase 6 candidate.

### 🟢 Pricing aliases for GLM-5 family are placeholder rates
**What**: `crates/shared/src/domain/services/aliases.rs` GLM-5 entries use
glm-4.6 rates because LiteLLM doesn't carry real Z.AI rates. Comment
acknowledges this.

**Fix**: Pull rates from Z.AI's pricing page when they publish, or accept
user-provided overrides via a config table.

---

## How to use this doc

When picking debt to address:
1. Pick highest-priority item that doesn't conflict with current feature work.
2. Spin off a phase document for non-trivial items (anything 🟡 or 🔴 in
   OAuth/security categories typically touches multiple crates).
3. After shipping, delete the entry from this file.

When adding new debt:
- Append to the relevant category, don't reorder.
- Set priority based on **user-observable impact**, not code aesthetics.
- Always include "Files" so future-you knows where to start.
