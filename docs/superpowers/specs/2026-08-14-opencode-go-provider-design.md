# OpenCode Go provider — design

- **Date:** 2026-08-14
- **Status:** Approved (brainstorm), pending implementation plan
- **Approach:** A — one provider entry whose wire format is selected **per
  model** from a config-stored glob table, plus a third upstream dialect
  (OpenAI Responses) extracted from `CodexProvider` and shared.

## Problem

[OpenCode Go](https://opencode.ai/docs/go/) is a $10/month subscription
gateway serving 19 open coding models. Unlike every provider the proxy speaks
to today, **the wire format depends on the model, not on the provider**:

| upstream endpoint | models |
|---|---|
| `https://opencode.ai/zen/go/v1/chat/completions` | `glm-5.1`, `glm-5.2`, `glm-5.3`, `kimi-k3`, `kimi-k2.7-code`, `kimi-k2.6`, `deepseek-v4-pro`, `deepseek-v4-flash`, `mimo-v2.5`, `mimo-v2.5-pro`, `hy3` |
| `https://opencode.ai/zen/go/v1/messages` (Anthropic) | `minimax-m3`, `minimax-m2.7`, `minimax-m2.5`, `qwen3.8-max`, `qwen3.7-max`, `qwen3.7-plus`, `qwen3.6-plus` |
| `https://opencode.ai/zen/go/v1/responses` (OpenAI Responses) | `grok-4.5`, `gpt-5.6-luna` |

The current model picks the upstream format from two axes — the client's
endpoint (`/v1/messages` vs `/v1/chat/completions`) and which base URLs the
provider has configured (`2026-07-24-unified-provider-dual-format-design.md`).
Neither axis is the model, so a single OpenCode Go provider entry would send
`kimi-k3` to `/v1/messages` whenever the client speaks Anthropic.

`GET /zen/go/v1/models` returns `{id, object, created, owned_by}` only — the
endpoint class is **not** discoverable at runtime, so it has to be a table.

The Responses endpoint is a second gap: only `CodexProvider` can translate
Chat Completions → Responses, and its translation hardcodes ChatGPT-backend
quirks.

## Goals

1. One provider entry (`opencode-go`) serves all 19 models, so the namespace
   form documented by OpenCode (`opencode-go/kimi-k3`) works unchanged.
2. The model→format table is **data in the config DB**, not a compile-time
   const — OpenCode adds and retires models without a rebuild.
3. Both client formats keep working for every model: an Anthropic client
   asking for `kimi-k3` is translated A→O; an OpenAI client asking for
   `qwen3.7-max` is translated O→A.
4. Reuse the Responses translation instead of copying it; `CodexProvider`
   behavior does not change.

## Non-goals

- Changing the Anthropic↔OpenAI translation algorithms.
- Dollar-denominated quota. OpenCode Go limits usage in dollars ($12/5h,
  $30/week, $60/month); the proxy's `QuotaRule` counts requests and tokens.
  No mapping is attempted.
- Account-usage reporting for Go (see *Deferred* below).
- A `Responses` variant on `ApiFormat`. Responses stays an upstream detail
  **inside** the provider adapter, exactly where Codex handles it today, so the
  translation layer and routing keep a two-valued format axis.

## Architecture

### 1. `ProviderKind::OpencodeGo`

Serialized as `opencode_go`, with `#[serde(alias = "opencode-go")]`.

Preset (`upstream.rs::default_urls`, `quirks_for`, `builder.rs`):

| field | value |
|---|---|
| `anthropic_base_url` | `https://opencode.ai/zen/go` (+ client path `/v1/messages`) |
| `openai_base_url` | `https://opencode.ai/zen/go/v1` (+ translated path `/chat/completions`) |
| `responses_base_url` | defaults to `openai_base_url` (+ `/responses`) |
| `auth` | `api_key` |
| quirks | none |
| account usage | `NoopAccountUsage` |
| thinking levels | none offered |

`api_key` is chosen because the existing auth plumbing already produces the
right header on each path: `forward` sends `x-api-key`, and `forward_openai`
converts `AuthHeader::ApiKey → Bearer`. If the probe shows `/v1/messages`
requires `Bearer`, the preset changes to `bearer` — one data row, no code.

Base URLs follow the existing `format!("{base_url}{path}")` join, where the
Anthropic path is `/v1/messages` and the OpenAI path is `/chat/completions`
(see `routing.rs::translate_path`). Hence the two bases differ by the `/v1`
segment; both are prefill defaults and remain user-overridable.

### 2. Model → format table

`ProviderConfig` gains one field, stored and edited as a **compact string**:

```
minimax-*=anthropic,qwen3.*=anthropic,grok-4.5=responses,gpt-5.6-luna=responses
```

- Grammar: comma-separated `glob=format`; `format` ∈ `anthropic | openai |
  responses`; whitespace around tokens ignored. **Divergence during review:**
  unlike the provider `kind` vocabulary (§1), there is no `open_ai` alias for
  the `openai` format — `parse_model_formats` rejects it. Removed on purpose
  so the two vocabularies don't silently drift.
- Matching is **first-match-wins**, in written order. No match → the provider's
  normal capability rules (unchanged behavior).
- Empty / absent → today's behavior exactly, so no existing provider is
  affected.
- Globs use the same matcher as routing rules (`*` wildcard; `.` is literal).
- Parsing and validation live in `config.rs`
  (`ModelFormatRules::parse`), called from `Config::validate()`, so a malformed
  string is rejected by the admin API at save time rather than at forward time.

The preset seeds the string above when an `opencode_go` provider is created
(same prefill mechanism as `default_urls`), after which it is plain config.
`glm-*`, `kimi-*`, `deepseek-v4-*`, `mimo-*` and `hy3` need no entry — they
fall through to the OpenAI default.

### 3. Port and routing change

`application/ports/provider.rs`:

```rust
/// Formats this provider can serve for a specific model. Defaults to
/// `supported_formats()` for providers whose capability is model-independent.
fn supported_formats_for(&self, model: &str) -> FormatSupport {
    let _ = model;
    self.supported_formats()
}
```

`UpstreamProvider` overrides it: a model matching an `anthropic` rule reports
Anthropic-only; `openai` **and** `responses` rules both report OpenAI-only
(the Responses split happens later, inside `forward_openai`); no match falls
back to `supported_formats()`, which already folds `format_mode`.

**Ruling during review (corrects the paragraph above):** a rule may only
*narrow* within the endpoints the provider actually has a URL for — the same
invariant `supported_formats()` already keeps for `format_mode`. If a rule
names a format whose base URL is empty on that provider (e.g. someone hand-writes
an `=anthropic` rule on a provider with no `anthropic_base_url`), the rule is
ignored for that model and capability falls back to
`supported_formats()`/the URL-derived default, rather than the request
failing with "no endpoint for this provider." The implementation
(`UpstreamProvider::supported_formats_for` in `upstream.rs`) checks
`has_anthropic`/`has_openai` before honoring a rule's format, matching this
fallback exactly.

`routing.rs` swaps `provider.supported_formats()` for
`provider.supported_formats_for(model)` at the six `select_direction` call
sites — namespace ×2, failover ×2, round-robin ×2. All six already have the
parsed model in scope. On the namespace path the lookup uses the **bare**
model (after `split_namespace` strips the `opencode-go/` prefix), because that
is what is sent upstream.

`select_direction` itself, `Direction`, `translate_path`, `translate_request`
and `translate_upstream_response` are unchanged.

### 4. Responses dialect

Move out of `codex.rs` into `adapters/translation/openai_to_responses.rs`:
`translate_request`, `translate_tool`, `text_content_to_string`,
`translate_buffered_response`, `ResponsesSseTranslator`.

The Codex-specific behavior currently hardcoded there becomes data:

```rust
pub struct ResponsesDialect {
    /// Codex: true (the backend rejects non-streaming). Go: false.
    pub force_stream: bool,
    /// Codex: Some(false). Go: None (field omitted).
    pub store: Option<bool>,
    /// Codex: true → include: ["reasoning.encrypted_content"]. Go: false.
    pub include_encrypted_reasoning: bool,
    /// Codex: true (backend rejects max_output_tokens).
    /// Go: false → map Chat `max_tokens` → `max_output_tokens`.
    pub drop_max_output_tokens: bool,
    /// Fallback when no system/developer message is present.
    pub default_instructions: Option<&'static str>,
}
```

`CodexProvider` passes its own dialect and keeps every current behavior; its
existing tests become the regression guard for the extraction.

`UpstreamProvider::forward_openai` gains one branch: if the request's model
resolves to `responses`, translate the Chat Completions body with the Go
dialect, POST to `{responses_base_url}/responses`, and wrap the SSE stream in
`ResponsesSseTranslator` (or `translate_buffered_response` when the client did
not ask for streaming). Otherwise the existing chat path runs untouched.

**Usage logging needs no change:** the translator emits
`chat.completion.chunk` events, so the existing OpenAI usage parser reads them
correctly — this is how Codex already reports usage.

## Config schema and migration

- `schema.rs`: `ALTER TABLE providers ADD COLUMN model_formats TEXT;`
  NULL means "no rules", i.e. current behavior.
- `db_config.rs`: read/write the column; map kind string `opencode_go` ↔
  `ProviderKind::OpencodeGo`.
- `admin.rs`: the same kind mapping in both directions.
- `proxy-admin-api`: `ProviderPayload.model_formats: Option<String>`.

## Admin API and TUI

- `proxy-tui/src/app.rs`: mirror the new kind — label, cycle order, default
  URLs, offered thinking levels — and add a `model_formats` string to the
  provider form state.
- `FormField` gains one variant; `main.rs` maps it to the field, `ui.rs`
  renders one more text row, `validate.rs` rejects a malformed string with the
  same message the daemon would return.
- The preset value is prefilled when the user selects the kind, using the
  existing autofill rule (overwrite only while the field still holds the
  previous kind's default).

## Deferred

- **Account usage.** OpenCode Go exposes its dollar limits in the Zen console;
  no documented API. `NoopAccountUsage` for now. If the probe finds an
  endpoint under `opencode.ai/zen/`, add an adapter in a follow-up.
- **Pricing.** Go model ids (`kimi-k3`, `glm-5.2`, …) are unlikely to exist in
  the LiteLLM-synced `pricing.db` under those exact keys, so logged requests
  will show cost 0. Per-model prices are published in the Go docs and could be
  seeded under `opencode-go/<model>` lookup keys later. **Assumption for this
  spec: cost 0 is accepted; no pricing work in scope.**
- **Thinking levels.** Each Go model reasons differently; the kind offers no
  levels and passes the client's `reasoning_effort` through (the Responses
  dialect already maps it to `reasoning.effort`).

## Testing strategy

- **Config:** kind parses from `opencode_go` and `opencode-go`; round-trips
  through serde and `db_config`; `model_formats` parse errors are rejected by
  `Config::validate()`.
- **Resolution:** first-match-wins ordering, fallthrough to default, and
  namespace-stripped model lookup.
- **Routing:** for each of the three model classes × two client formats,
  assert the chosen upstream format, the `Direction`, and that passthrough
  never invokes the translator.
- **Dialect:** the Codex dialect produces the byte-identical request body it
  produces today (the existing `codex.rs` tests, retargeted at the shared
  module); the Go dialect omits `store`/`include`, preserves the client's
  `stream` flag, and maps `max_tokens` → `max_output_tokens`.
- **Integration (wiremock):** six cases — `/chat/completions`, `/v1/messages`,
  `/responses` upstream × Anthropic and OpenAI client — asserting the URL
  path, the auth header, and the absence of translation artifacts on
  passthrough.
- **Migration:** a pre-migration provider row loads with empty
  `model_formats` and behaves as before.
- **TUI:** form round-trip of the new field and its validation error.

## Phased rollout

Each phase keeps `cargo test --workspace` green and is committed separately.

1. Extract the Responses translation into
   `adapters/translation/openai_to_responses.rs` behind `ResponsesDialect`;
   `CodexProvider` delegates. No behavior change.
2. Add `ProviderKind::OpencodeGo`, its preset, the `model_formats` config
   field, the schema migration, and the admin/TUI plumbing. The
   `chat/completions` and `messages` model groups work end to end.
3. Add `supported_formats_for(model)` to the port and switch the six routing
   call sites.
4. Wire the `responses` class into `UpstreamProvider::forward_openai`.
5. Document the kind in `docs/specs/provider-config.md` and verify against the
   live API with a real key.

## Assumptions and risks

1. **Auth header** — assumed `x-api-key` on `/v1/messages` and
   `Authorization: Bearer` on the OpenAI paths, which the `api_key` preset
   produces automatically. Unverified until a key is available.
2. **Gateway strictness** — the endpoint table above is from the docs; whether
   the gateway actually rejects a model on the "wrong" endpoint is unverified.
   If it is permissive, the seeded rule string shrinks to the `responses`
   group only, with no code change.
3. **Model list churn** — models are added and removed by OpenCode; because
   the table is config, a stale entry is a config edit, not a release. A model
   with no rule falls through to `chat/completions`, which is the largest and
   most stable group.
4. **Goal 3 is not enforced for unmatched models** — Goal 3 says "an
   Anthropic client asking for `kimi-k3` is translated A→O." In the shipped
   implementation this only happens for models that have an explicit
   `model_formats` rule. A model with no rule (the entire `chat/completions`
   group, by design) reports unchanged, both-URLs-configured capability from
   `supported_formats_for`, so `select_direction` passes it straight through
   on whichever endpoint the client used — an Anthropic client asking for
   `kimi-k3` reaches `/zen/go/v1/messages` with no translation, not
   `/zen/go/v1/chat/completions`. Whether that is safe depends on the same
   unverified gateway-strictness assumption as #2: if the gateway accepts
   `chat/completions`-group models on `/messages`, Goal 3 holds anyway via
   passthrough; if it rejects them, Goal 3 is broken for that group until
   explicit `=openai` rules are added.
5. **Extraction blast radius** — the Responses code is Codex's hot path.
   Mitigated by phase 1 being pure refactor with the existing test suite as
   the guard.
