# Unified Provider — Phase 4: Cut the builder over, delete the six structs

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build every non-Codex provider as an `UpstreamProvider`, then delete the six now-unused provider structs one at a time. This is where **dual-for-all turns on** — providers with both URLs (MiniMax, Kimi) become passthrough in both directions.

**Architecture:** `build_leaf` gains one generic arm for the six kinds
(`quirks_for(kind, …)` + `UpstreamProvider::new(…)`) and keeps the bespoke
`Codex` arm. With capability now sourced from the two URL columns (Phase 2) and
routing already honoring `supported_formats()` (Phase 1), the behavior change
lands automatically. The seven struct files shrink to one (`codex.rs`).

**Tech Stack:** Rust (edition 2024), reqwest, wiremock (dev), `cargo test`.

## Global Constraints

- Edition 2024; no new deps. `ProxyError` at the provider surface.
- Validate at build time: a generic provider must have at least one non-empty
  URL, else `BuildError`.
- Delete structs one per commit so a failed step is bisectable.
- `cargo test --workspace` + `cargo clippy --workspace -- -D warnings` green
  before every commit. Conventional Commits, no attribution. Branch, not
  `develop`.

## File Structure

- `crates/proxy/src/adapters/providers/builder.rs` — generic arm + Codex arm.
- Delete: `minimax.rs`, `deepseek.rs`, `kimi.rs`, `zai.rs`, `openai.rs`,
  `anthropic.rs` (one per task). Keep `codex.rs`, `messages_protocol.rs`,
  `minimax_stream.rs`, `tool_sanitizer.rs`, `upstream.rs`, `routing.rs`,
  `account_usage/*`, `token_refresh.rs`.
- `crates/proxy/tests/` — new integration tests for dual + single providers.

---

### Task 1: `build_leaf` constructs `UpstreamProvider` for the six kinds

**Files:**
- Modify: `crates/proxy/src/adapters/providers/builder.rs`

**Interfaces:**
- Consumes: `UpstreamProvider::new`, `quirks_for` (Phase 3);
  `ProviderConfig.anthropic_base_url` / `openai_base_url` (Phase 2).
- Produces: unchanged `build_leaf` signature returning `Arc<dyn Provider>`.

- [ ] **Step 1: Write the failing test**

Add a unit test in `builder.rs` asserting a MiniMax config yields a provider
that supports both formats (proving the generic arm is wired):

```rust
#[test]
fn minimax_config_builds_dual_capable_provider() {
    let p = build_leaf(&ProviderConfig {
        name: "mm".into(),
        kind: ProviderKind::Minimax,
        enabled: true,
        auth: AuthConfig::Bearer { value: "k".into() },
        anthropic_base_url: Some("https://api.minimax.io/anthropic".into()),
        openai_base_url: Some("https://api.minimax.io/v1".into()),
        reasoning_effort: None,
        thinking_mode: crate::config::ThinkingMode::SplitOnly,
        max_concurrent: None,
        sanitize_empty_tools: false,
    }, reqwest::Client::new()).unwrap();
    let sup = p.supported_formats();
    assert!(sup.anthropic && sup.openai);
}

#[test]
fn provider_with_no_urls_is_rejected() {
    let err = build_leaf(&ProviderConfig {
        name: "bad".into(),
        kind: ProviderKind::DeepSeek,
        enabled: true,
        auth: AuthConfig::Bearer { value: "k".into() },
        anthropic_base_url: None,
        openai_base_url: None,
        reasoning_effort: None,
        thinking_mode: crate::config::ThinkingMode::SplitOnly,
        max_concurrent: None,
        sanitize_empty_tools: false,
    }, reqwest::Client::new());
    assert!(err.is_err());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p proxy --lib minimax_config_builds_dual_capable`
Expected: FAIL — the old `MinimaxProvider` arm returns a provider whose
`supported_formats()` defaults from `native_format()` (single), so the dual
assertion fails (or the no-URL case does not error yet).

- [ ] **Step 3: Replace the six arms with one generic arm; keep Codex**

In `build_leaf`, after resolving `auth`, replace the `match p.kind { … }` with:

```rust
    // Codex is bespoke: translates to the OpenAI Responses API.
    if let ProviderKind::Codex = p.kind {
        return Ok(Arc::new(CodexProvider::configure_with_reasoning_effort(
            http, p.openai_base_url.clone(), auth, p.reasoning_effort.clone(),
        )));
    }

    // Validate at least one endpoint is configured.
    let has_anthropic = p.anthropic_base_url.as_deref().is_some_and(|s| !s.is_empty());
    let has_openai = p.openai_base_url.as_deref().is_some_and(|s| !s.is_empty());
    if !has_anthropic && !has_openai {
        return Err(BuildError::AuthResolve(format!(
            "provider '{}': at least one of anthropic_base_url / openai_base_url is required",
            p.name
        )));
    }

    let thinking = match p.thinking_mode {
        crate::config::ThinkingMode::SplitOnly => minimax_stream::ThinkingMode::SplitOnly,
        crate::config::ThinkingMode::StripAll => minimax_stream::ThinkingMode::StripAll,
    };
    let quirks = quirks_for(p.kind, thinking, p.reasoning_effort.clone(), p.sanitize_empty_tools);
    Ok(Arc::new(UpstreamProvider::new(
        p.name.clone(),
        p.anthropic_base_url.clone(),
        p.openai_base_url.clone(),
        auth,
        quirks,
        http,
    )))
```

Add the needed `use` lines: `use super::upstream::{quirks_for, UpstreamProvider};`
and `use super::minimax_stream;` (if not already imported). Use the correct
`BuildError` variant that already exists for config errors (mirror the
`AuthResolve` usage in the Codex-auth path).

- [ ] **Step 4: Run tests**

Run: `cargo test -p proxy`
Expected: the two new tests PASS. Some existing per-struct unit tests still
compile because the structs still exist (deleted in later tasks). Behavior of
MiniMax/Kimi now flips to dual — if an integration test asserted the old
single-format behavior, update it in Task 8 (integration).

- [ ] **Step 5: Commit**

```bash
git add crates/proxy/src/adapters/providers/builder.rs
git commit -m "feat(proxy): build non-Codex providers as UpstreamProvider (dual-for-all)"
```

---

### Tasks 2–7: Delete the six structs, one per task

For each of `minimax`, `deepseek`, `kimi`, `zai`, `openai`, `anthropic`, do the
following (one commit each). This is mechanical: the builder no longer
references them; only their `mod` declaration, re-exports, and any helper the
generic path still needs must be handled.

**Per-struct steps (repeat, substituting `<X>`):**

- [ ] **Step 1: Confirm no non-test references remain**

Run: `grep -rn "<X>Provider" crates/proxy/src --include=*.rs`
Expected: references only in `<X>.rs` itself, `mod.rs` (module + re-export), and
possibly a stale `use` in `builder.rs`. If a **helper function** defined in
`<X>.rs` is still called from `upstream.rs` (e.g. `inject_reasoning_split`,
`inject_effort`, the DeepSeek reorder, the Kimi sanitize wrappers), that helper
must already have been **moved** into `upstream.rs`/`tool_sanitizer` in Phase 3.
If any remain in `<X>.rs`, move them now (in this task) before deleting.

- [ ] **Step 2: Delete the file and its wiring**

- Delete `crates/proxy/src/adapters/providers/<X>.rs`.
- Remove `mod <X>;` and the `pub use <X>::<X>Provider;` (and any other
  re-exports) from `crates/proxy/src/adapters/providers/mod.rs`.
- Remove the now-unused `use` of `<X>Provider` in `builder.rs`.

- [ ] **Step 3: Build + test**

Run: `cargo test -p proxy && cargo clippy -p proxy -- -D warnings`
Expected: PASS, no warnings. Any test that lived in `<X>.rs` is gone with it;
its coverage is replaced by `upstream.rs` unit tests (Phase 3) and the
integration tests (Task 8). If a test elsewhere referenced `<X>Provider`
directly, port it to construct an `UpstreamProvider` or delete it if redundant.

- [ ] **Step 4: Commit**

```bash
git rm crates/proxy/src/adapters/providers/<X>.rs
git add crates/proxy/src/adapters/providers/mod.rs crates/proxy/src/adapters/providers/builder.rs
git commit -m "refactor(proxy): remove <X>Provider, folded into UpstreamProvider"
```

Order: `zai` → `openai` → `deepseek` → `kimi` → `anthropic` → `minimax`
(simplest first; MiniMax last since it exercises the most quirks).

---

### Task 8: Integration tests — dual passthrough and single translate

**Files:**
- Create: `crates/proxy/tests/dual_format_routing.rs`

**Interfaces:**
- Consumes: the public proxy test harness used by existing tests in
  `crates/proxy/tests/translation.rs` (reuse its app/server setup helpers).

- [ ] **Step 1: Write the tests**

Mirror the wiremock setup in `crates/proxy/tests/translation.rs`. Two cases:

```rust
// Dual provider: both endpoints mocked. Anthropic client hits the Anthropic
// mock with NO translation; OpenAI client hits the OpenAI mock with NO
// translation.
#[tokio::test]
async fn dual_provider_passes_through_both_client_formats() {
    // mount two wiremock endpoints: /v1/messages (anthropic) and
    // /chat/completions (openai). Configure a provider with BOTH base urls
    // pointing at them. Send an Anthropic request to the proxy's /v1/messages;
    // assert the anthropic mock received it verbatim (no openai fields) and the
    // response reached the client unchanged. Repeat for OpenAI.
}

// Single provider (openai only): Anthropic client is translated to the OpenAI
// endpoint and the response is translated back.
#[tokio::test]
async fn single_openai_provider_translates_anthropic_client() {
    // mount only /chat/completions. Configure a provider with openai_base_url
    // only. Send an Anthropic /v1/messages request; assert the openai mock
    // received an OpenAI-shaped body and the client got an Anthropic-shaped,
    // terminated response.
}
```

Fill in the bodies using the exact harness helpers from `translation.rs`
(server spawn, config injection, request helpers). Assert on the mock's
received request shape (translated vs verbatim) and on the client-visible
response shape.

- [ ] **Step 2: Run**

Run: `cargo test -p proxy --test dual_format_routing`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/proxy/tests/dual_format_routing.rs
git commit -m "test(proxy): dual-format passthrough and single-format translation"
```

---

### Task 9: Live smoke test (optional, recommended)

- [ ] Build release, run against a throwaway DB copy on a spare port, and
  confirm both client paths against MiniMax (dual) end in a terminated stream
  and — for the Anthropic path — include a native `thinking` block:

```bash
cargo build --release -p proxy
cp ~/.local/share/cli-router/proxy.db /tmp/p4.db
sqlite3 /tmp/p4.db "update settings set value='18791' where key='port'; update settings set value='/tmp/p4.db' where key='proxy_db';"
./target/release/cli-router-proxy --db /tmp/p4.db &  PID=$!
sleep 4
echo "--- anthropic client ---"
curl -sS -N http://127.0.0.1:18791/v1/messages -H 'content-type: application/json' \
  -d '{"model":"MiniMax-M2","max_tokens":300,"stream":true,"messages":[{"role":"user","content":"Say: ok"}]}' | tail -3
echo "--- openai client ---"
curl -sS -N http://127.0.0.1:18791/v1/chat/completions -H 'content-type: application/json' \
  -d '{"model":"MiniMax-M2","max_tokens":300,"stream":true,"messages":[{"role":"user","content":"Say: ok"}]}' | tail -3
kill $PID; rm -f /tmp/p4.db*
```
Expected: Anthropic path ends `message_stop`; OpenAI path ends `data: [DONE]`.

---

## Self-Review

- **Spec coverage:** spec §"Builder / composition" (Task 1), §"phased rollout"
  step 2 (Tasks 2–7), §"Testing strategy" integration (Task 8). Codex arm
  preserved. Dual-for-all behavior change realized and asserted.
- **Placeholder scan:** the integration test bodies are described with exact
  harness source (`translation.rs`) to copy from — acceptable, as they depend on
  private test helpers that must be read in-repo. The builder arm is shown in
  full.
- **Type consistency:** `UpstreamProvider::new`, `quirks_for`,
  `supported_formats`, `BuildError` used consistently with Phases 2–3.

## After this phase

Phase 5 removes `native_format()` from the trait and the remaining providers
(`CodexProvider` and any test doubles), then adds preset-default URL prefill to
the TUI wizard so picking a kind fills sensible endpoints.
