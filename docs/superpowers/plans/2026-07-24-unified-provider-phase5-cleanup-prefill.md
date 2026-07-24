# Unified Provider — Phase 5: Remove `native_format`, add TUI preset prefill

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Retire the now-unused `native_format()` from the `Provider` trait (capability fully lives in `supported_formats()`), and make the TUI wizard prefill sensible default endpoint URLs when the user picks a kind.

**Architecture:** After Phase 4 only `CodexProvider`, `RoutingProvider`,
`LiveProvider`, and test doubles still carry `native_format()`. Make
`supported_formats()` a **required** trait method (no default), give each
remaining impl an explicit one, then delete `native_format()` everywhere. Then
add `default_urls(kind)` to the preset and call it from the TUI wizard.

**Tech Stack:** Rust (edition 2024), ratatui, `cargo test`.

## Global Constraints

- Edition 2024; no new deps. `cargo test --workspace` +
  `cargo clippy --workspace -- -D warnings` green before every commit.
- Conventional Commits, no attribution. Branch, not `develop`.

## File Structure

- `crates/proxy/src/application/ports/provider.rs` — remove `native_format`,
  make `supported_formats` required.
- `crates/proxy/src/adapters/providers/codex.rs`, `routing.rs`, `live.rs` (and
  any test fakes) — explicit `supported_formats`.
- `crates/proxy/src/adapters/providers/upstream.rs` — `default_urls(kind)`.
- `crates/proxy-tui/src/*` — prefill URL fields on kind change.

---

### Task 1: Make `supported_formats` required; give remaining impls explicit ones

**Files:**
- Modify: `crates/proxy/src/application/ports/provider.rs`
- Modify: `crates/proxy/src/adapters/providers/codex.rs`
- Modify: `crates/proxy/src/adapters/providers/routing.rs`
- Modify: `crates/proxy/src/adapters/providers/live.rs` (if it implements
  `Provider`)
- Modify: any `Provider` test double (search below)

**Interfaces:**
- Produces: `supported_formats(&self) -> FormatSupport` with **no** default;
  `native_format` removed from the trait.

- [ ] **Step 1: Find every `Provider` implementor and every `native_format`**

Run: `grep -rn "impl Provider for\|native_format" crates/proxy/src --include=*.rs`
Record each `impl Provider for T` — every `T` needs an explicit
`supported_formats` after this task.

- [ ] **Step 2: Add explicit `supported_formats` to each remaining impl**

- `CodexProvider` (codex.rs): `fn supported_formats(&self) -> FormatSupport { FormatSupport::single(ApiFormat::OpenAI) }`
- `RoutingProvider` (routing.rs): it dispatches to leaves and accepts both
  client formats at the top, so `FormatSupport::both()`.
- `LiveProvider` (live.rs): delegate to the inner provider —
  `self.current().supported_formats()` (use whatever accessor it already uses
  to reach the inner `Arc<dyn Provider>`; mirror how its `forward` reaches it).
- Any test fake: return whatever its existing `native_format` implied
  (`FormatSupport::single(<that format>)`).

Ensure `FormatSupport` and `ApiFormat` are imported in each file.

- [ ] **Step 3: Remove `native_format` from the trait and all impls**

- In `provider.rs`, delete the `native_format` method (both the signature and,
  if present, the default body), and delete the default body of
  `supported_formats` so it becomes a required method:

```rust
    /// Which formats this provider can serve natively.
    fn supported_formats(&self) -> FormatSupport;
```

- Delete every `fn native_format(&self) -> ApiFormat { … }` override the grep
  from Step 1 found (codex, and any remaining struct/fake).

- [ ] **Step 4: Build + test**

Run: `cargo test --workspace`
Expected: PASS. The compiler will flag any `Provider` impl missing
`supported_formats` — add it (single/both as appropriate). Remove any test that
asserted `native_format()` directly (e.g. `native_format_is_openai` in
`codex.rs` tests) or convert it to assert `supported_formats()`.

Run: `cargo clippy --workspace -- -D warnings`
Expected: no warnings.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor(proxy): remove native_format; supported_formats is the sole capability"
```

---

### Task 2: `default_urls(kind)` preset helper

**Files:**
- Modify: `crates/proxy/src/adapters/providers/upstream.rs`

**Interfaces:**
- Produces: `pub fn default_urls(kind: ProviderKind) -> (Option<&'static str>,
  Option<&'static str>)` returning `(anthropic_default, openai_default)`.

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn default_urls_minimax_has_both() {
        let (a, o) = default_urls(ProviderKind::Minimax);
        assert_eq!(a, Some("https://api.minimaxi.com/anthropic"));
        assert_eq!(o, Some("https://api.minimaxi.com/v1"));
    }

    #[test]
    fn default_urls_deepseek_openai_only() {
        let (a, o) = default_urls(ProviderKind::DeepSeek);
        assert!(a.is_none());
        assert!(o.is_some());
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p proxy --lib default_urls`
Expected: FAIL — function not found.

- [ ] **Step 3: Implement**

Port the default endpoint constants from the (now-deleted) struct files' git
history, or hardcode the known defaults:

```rust
pub fn default_urls(kind: ProviderKind) -> (Option<&'static str>, Option<&'static str>) {
    match kind {
        ProviderKind::Anthropic => (Some("https://api.anthropic.com"), None),
        ProviderKind::Minimax => (
            Some("https://api.minimaxi.com/anthropic"),
            Some("https://api.minimaxi.com/v1"),
        ),
        ProviderKind::Zai => (None, Some("https://api.z.ai/api/coding/paas/v4")),
        ProviderKind::DeepSeek => (None, Some("https://api.deepseek.com")),
        ProviderKind::OpenAi => (None, Some("https://api.openai.com/v1")),
        ProviderKind::Kimi => (
            Some("https://api.moonshot.ai/anthropic"),
            Some("https://api.moonshot.ai/v1"),
        ),
        ProviderKind::Codex => (None, Some("https://chatgpt.com/backend-api/codex")),
    }
}
```

Verify each default against the deleted structs' `DEFAULT_*` consts in git
(`git show <phase4-commit>^:crates/proxy/src/adapters/providers/<kind>.rs`) and
correct any that differ — the exact strings matter.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p proxy --lib default_urls`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/proxy/src/adapters/providers/upstream.rs
git commit -m "feat(proxy): default_urls preset for TUI prefill"
```

---

### Task 3: TUI prefills URL fields when kind changes

**Files:**
- Modify: `crates/proxy-tui/src/*` (the provider create/edit modal + kind
  selector)

**Interfaces:**
- Consumes: `default_urls(kind)`. If the TUI cannot depend on the proxy crate
  directly, duplicate the small `default_urls` table in the TUI (it is static
  data) — check how the TUI currently obtains kind-specific defaults, if at all,
  and follow that pattern.

- [ ] **Step 1: Locate the kind selector and URL fields**

Run: `grep -rn "ProviderKind\|kind\|anthropic_base_url\|openai_base_url" crates/proxy-tui/src`
Find where the wizard changes the selected kind and where the two URL fields
hold their current input.

- [ ] **Step 2: Prefill on kind change**

When the user selects/changes the kind in the create flow (not when editing an
existing provider with already-set URLs), set the Anthropic-URL and OpenAI-URL
input buffers to the `default_urls(kind)` values **only if those buffers are
currently empty**, so a user's manual entry is never overwritten.

- [ ] **Step 3: Build + test**

Run: `cargo test -p proxy-tui && cargo build -p proxy-tui`
Expected: PASS / builds. If validation forbids saving a provider with both URLs
empty, keep that consistent with the builder rule (at least one URL).

- [ ] **Step 4: Commit**

```bash
git add crates/proxy-tui/src
git commit -m "feat(proxy-tui): prefill endpoint URLs from provider kind defaults"
```

---

### Task 4: Final workspace verification

- [ ] **Step 1: Full gates**

Run: `cargo test --workspace`
Run: `cargo clippy --workspace -- -D warnings`
Run: `cargo fmt --check`
Expected: all green.

- [ ] **Step 2: Confirm the surface is clean**

Run: `grep -rn "native_format" crates/ --include=*.rs`
Expected: no matches anywhere.

Run: `grep -rn "MinimaxProvider\|DeepSeekProvider\|ZaiProvider\|KimiProvider\|OpenAiProvider\|AnthropicProvider" crates/ --include=*.rs`
Expected: no matches (all folded into `UpstreamProvider`). `CodexProvider`
remains.

- [ ] **Step 3: Live end-to-end (recommended)**

Repeat the Phase 4 live smoke test on a throwaway DB; additionally confirm a
single-URL provider (e.g. a DeepSeek route) still answers an Anthropic client
via translation.

- [ ] **Step 4: Commit any fmt fixes**

```bash
git add -A
git commit -m "style(proxy): fmt after provider unification" # only if fmt changed files
```

---

## Self-Review

- **Spec coverage:** spec §"Ports change" removal of `native_format` (Task 1),
  §"Admin API + TUI" prefill (Tasks 2–3), §"phased rollout" step 5. Codex keeps
  an explicit OpenAI-only `supported_formats`.
- **Placeholder scan:** `default_urls` values are given in full with a
  git-verification step (exact strings matter); TUI steps name the grep to
  locate the exact sites since the modal internals are read in-repo.
- **Type consistency:** `supported_formats` (required), `FormatSupport::single`
  / `both`, `default_urls` used consistently with Phases 1–4.

## Redesign complete

After this phase: one `UpstreamProvider` serves every HTTP-uniform provider with
URL-driven dual-format capability; Codex remains bespoke; capability flows from
two config columns through routing with no hardcoded format; the TUI edits and
prefills both URLs. The five phases together implement
`docs/superpowers/specs/2026-07-24-unified-provider-dual-format-design.md`.
