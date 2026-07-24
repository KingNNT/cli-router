# Unified Provider — Phase 1: Routing reads capability, not `native_format`

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Introduce a `FormatSupport` capability type and a `supported_formats()` provider method, then make `RoutingProvider` choose the upstream format from capability instead of the hardcoded `native_format()` — with **zero behavior change**.

**Architecture:** `supported_formats()` is added to the `Provider` trait with a default implementation derived from the existing `native_format()`, so every current provider keeps behaving identically. A small `select_direction` helper computes `(Direction, upstream_format)` from `(client_format, FormatSupport)`. Routing's format branches call it instead of matching on `native_format()`. This is the seam later phases build on: once providers override `supported_formats()` from configured URLs, routing already honors it.

**Tech Stack:** Rust (edition 2024), axum, async-trait, `cargo test`, `cargo clippy`.

## Global Constraints

- All crates target **edition 2024**; prefer let-chains over nested `if let`.
- One error enum per ring; cross-ring conversions via `#[from]`. No `anyhow`.
- Ports use `Arc<dyn Port + Send + Sync>`. No new dependencies.
- Do not remove `native_format()` in this phase — later phases still read it until Phase 5.
- `cargo test --workspace` and `cargo clippy --workspace -- -D warnings` must pass before every commit. The pre-commit hook runs the full test suite (allow ~5 min).
- Conventional Commits; **never** add `Co-Authored-By` or attribution lines.
- Work on a branch, not `develop`.

---

## File Structure

- `crates/proxy/src/application/ports/provider.rs` — home of `ApiFormat`,
  `Direction`, the `Provider` trait. Add `FormatSupport` + the
  `supported_formats()` trait method here.
- `crates/proxy/src/adapters/providers/routing.rs` — add `select_direction`
  and swap the `native_format()` call sites.

No new files. No new crates.

---

### Task 1: `FormatSupport` capability type + trait method

**Files:**
- Modify: `crates/proxy/src/application/ports/provider.rs`

**Interfaces:**
- Produces:
  - `pub struct FormatSupport { pub anthropic: bool, pub openai: bool }`
  - `FormatSupport::single(ApiFormat) -> FormatSupport`
  - `FormatSupport::both() -> FormatSupport`
  - `FormatSupport::has(&self, ApiFormat) -> bool`
  - `FormatSupport::sole(&self) -> ApiFormat`
  - `Provider::supported_formats(&self) -> FormatSupport` (default impl derived
    from `native_format()`)

- [ ] **Step 1: Write the failing test**

Add to the existing `#[cfg(test)] mod tests` at the bottom of
`crates/proxy/src/application/ports/provider.rs` (create the module if none
exists):

```rust
#[cfg(test)]
mod format_support_tests {
    use super::*;

    #[test]
    fn single_anthropic_supports_only_anthropic() {
        let s = FormatSupport::single(ApiFormat::Anthropic);
        assert!(s.has(ApiFormat::Anthropic));
        assert!(!s.has(ApiFormat::OpenAI));
        assert_eq!(s.sole(), ApiFormat::Anthropic);
    }

    #[test]
    fn single_openai_supports_only_openai() {
        let s = FormatSupport::single(ApiFormat::OpenAI);
        assert!(!s.has(ApiFormat::Anthropic));
        assert!(s.has(ApiFormat::OpenAI));
        assert_eq!(s.sole(), ApiFormat::OpenAI);
    }

    #[test]
    fn both_supports_both() {
        let s = FormatSupport::both();
        assert!(s.has(ApiFormat::Anthropic));
        assert!(s.has(ApiFormat::OpenAI));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p proxy --lib format_support_tests`
Expected: FAIL — `cannot find type FormatSupport in this scope`.

- [ ] **Step 3: Write minimal implementation**

In `crates/proxy/src/application/ports/provider.rs`, immediately after the
`Direction` `impl` block, add:

```rust
/// Which wire formats a provider can serve natively.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatSupport {
    pub anthropic: bool,
    pub openai: bool,
}

impl FormatSupport {
    /// Support exactly one format.
    pub fn single(format: ApiFormat) -> Self {
        match format {
            ApiFormat::Anthropic => Self { anthropic: true, openai: false },
            ApiFormat::OpenAI => Self { anthropic: false, openai: true },
        }
    }

    /// Support both formats (dual-endpoint provider).
    pub fn both() -> Self {
        Self { anthropic: true, openai: true }
    }

    /// Whether the given format is supported.
    pub fn has(&self, format: ApiFormat) -> bool {
        match format {
            ApiFormat::Anthropic => self.anthropic,
            ApiFormat::OpenAI => self.openai,
        }
    }

    /// The single supported format. Caller must ensure exactly one is
    /// supported (i.e. call only when `!has(client_format)`); prefers
    /// Anthropic if — through misconfiguration — both were false.
    pub fn sole(&self) -> ApiFormat {
        if self.openai && !self.anthropic {
            ApiFormat::OpenAI
        } else {
            ApiFormat::Anthropic
        }
    }
}
```

Then, inside the `pub trait Provider` definition, right below the existing
`native_format` method, add the defaulted capability method:

```rust
    /// Which formats this provider can serve natively. Default derives from
    /// `native_format()`; providers that speak both override this.
    fn supported_formats(&self) -> FormatSupport {
        FormatSupport::single(self.native_format())
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p proxy --lib format_support_tests`
Expected: PASS (3 tests).

- [ ] **Step 5: Confirm the workspace still builds and lints**

Run: `cargo clippy -p proxy -- -D warnings`
Expected: Finished with no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/proxy/src/application/ports/provider.rs
git commit -m "feat(proxy): add FormatSupport capability and supported_formats() port method"
```

---

### Task 2: `select_direction` helper in routing

**Files:**
- Modify: `crates/proxy/src/adapters/providers/routing.rs`

**Interfaces:**
- Consumes: `FormatSupport`, `ApiFormat`, `Direction` from
  `crate::application::ports` (Task 1).
- Produces: an associated function
  `RoutingProvider::select_direction(client: ApiFormat, sup: FormatSupport) -> (Direction, ApiFormat)`
  returning `(direction, upstream_format)`.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in
`crates/proxy/src/adapters/providers/routing.rs`:

```rust
#[test]
fn select_direction_passthrough_when_client_format_supported() {
    // dual provider — every client format is a passthrough
    let (dir, up) =
        RoutingProvider::select_direction(ApiFormat::Anthropic, FormatSupport::both());
    assert_eq!(dir, Direction::Passthrough);
    assert_eq!(up, ApiFormat::Anthropic);

    let (dir, up) =
        RoutingProvider::select_direction(ApiFormat::OpenAI, FormatSupport::both());
    assert_eq!(dir, Direction::Passthrough);
    assert_eq!(up, ApiFormat::OpenAI);
}

#[test]
fn select_direction_translates_when_client_format_unsupported() {
    // provider only speaks OpenAI; an Anthropic client must be translated
    let (dir, up) = RoutingProvider::select_direction(
        ApiFormat::Anthropic,
        FormatSupport::single(ApiFormat::OpenAI),
    );
    assert_eq!(dir, Direction::AnthropicToOpenAI);
    assert_eq!(up, ApiFormat::OpenAI);

    // provider only speaks Anthropic; an OpenAI client must be translated
    let (dir, up) = RoutingProvider::select_direction(
        ApiFormat::OpenAI,
        FormatSupport::single(ApiFormat::Anthropic),
    );
    assert_eq!(dir, Direction::OpenAIToAnthropic);
    assert_eq!(up, ApiFormat::Anthropic);
}
```

If the test module does not already import these types, add
`use crate::application::ports::{ApiFormat, Direction, FormatSupport};` to the
test module (or reference them via their existing import path in the file).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p proxy --lib select_direction`
Expected: FAIL — `no function or associated item named select_direction`.

- [ ] **Step 3: Write minimal implementation**

Add this associated function inside `impl RoutingProvider { … }` (near
`translate_path`, which is already an associated `fn` in that impl):

```rust
    /// Given the client's format and a provider's capability, pick the
    /// upstream format (passthrough when supported, otherwise the provider's
    /// sole supported format) and the translation direction to apply.
    fn select_direction(
        client_format: ApiFormat,
        sup: FormatSupport,
    ) -> (Direction, ApiFormat) {
        let upstream_format = if sup.has(client_format) {
            client_format
        } else {
            sup.sole()
        };
        (Direction::from_pair(client_format, upstream_format), upstream_format)
    }
```

Ensure `FormatSupport` is in scope at the top of `routing.rs` (extend the
existing `use crate::application::ports::{…}` line to include `FormatSupport`).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p proxy --lib select_direction`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/proxy/src/adapters/providers/routing.rs
git commit -m "feat(proxy): add RoutingProvider::select_direction capability helper"
```

---

### Task 3: Route through capability at every dispatch site

**Files:**
- Modify: `crates/proxy/src/adapters/providers/routing.rs`

**Interfaces:**
- Consumes: `RoutingProvider::select_direction` (Task 2),
  `Provider::supported_formats()` (Task 1).
- Produces: no new public interface — replaces internal `native_format()`
  dispatch. Behavior is identical because `supported_formats()` still defaults
  from `native_format()`.

There are **six** dispatch sites in `routing.rs`, each an instance of the same
two-part pattern. Find them all first:

Run: `grep -n "native_format()" crates/proxy/src/adapters/providers/routing.rs`

Expected: matches inside `forward` (namespace branch), `forward_openai`
(namespace branch), `forward_failover`, and `forward_round_robin` — paired as a
`Direction::from_pair(...)` line and a `match ....native_format() { … }` block.
Ignore any match inside `#[cfg(test)]` (there should be none in production
paths besides these).

- [ ] **Step 1: Establish the safety net (record current behavior)**

Run the existing translation + routing suites and confirm they pass **before**
editing, so any post-edit failure is attributable:

Run: `cargo test -p proxy --lib routing && cargo test -p proxy --test translation`
Expected: PASS.

- [ ] **Step 2: Replace each site — the mechanical transform**

For **every** occurrence, replace this shape:

```rust
            let direction =
                Direction::from_pair(<CLIENT_FORMAT>, <PROVIDER>.native_format());
            let native_path = Self::translate_path(path, direction);
            let send_body = Self::translate_request(&<BODY>, direction)?;

            let raw_resp = match <PROVIDER>.native_format() {
                ApiFormat::Anthropic => {
                    <PROVIDER>.forward(native_path, headers, send_body, streaming).await
                }
                ApiFormat::OpenAI => {
                    <PROVIDER>.forward_openai(native_path, headers, send_body, streaming).await
                }
            };
```

with this shape (identical surrounding names; only the format decision moves
to `select_direction` and the `match` keys off `upstream_format`):

```rust
            let (direction, upstream_format) =
                Self::select_direction(<CLIENT_FORMAT>, <PROVIDER>.supported_formats());
            let native_path = Self::translate_path(path, direction);
            let send_body = Self::translate_request(&<BODY>, direction)?;

            let raw_resp = match upstream_format {
                ApiFormat::Anthropic => {
                    <PROVIDER>.forward(native_path, headers, send_body, streaming).await
                }
                ApiFormat::OpenAI => {
                    <PROVIDER>.forward_openai(native_path, headers, send_body, streaming).await
                }
            };
```

Concrete substitutions per site:
- `forward` namespace branch: `<CLIENT_FORMAT>` = `ApiFormat::Anthropic`,
  `<PROVIDER>` = `provider`, `<BODY>` = `rewritten_body`.
- `forward_openai` namespace branch: `<CLIENT_FORMAT>` = `ApiFormat::OpenAI`,
  `<PROVIDER>` = `provider`, `<BODY>` = `rewritten_body`.
- `forward_failover`: `<CLIENT_FORMAT>` = `client_format`,
  `<PROVIDER>` = `entry.provider`, `<BODY>` = `body`.
- `forward_round_robin`: `<CLIENT_FORMAT>` = `client_format`,
  `<PROVIDER>` = `entry.provider`, `<BODY>` = `body`.

Some functions contain the pattern twice (e.g. a primary attempt and a retry,
or buffered vs streaming branches). Apply the transform to **every** match the
grep found. Where a site currently calls `.native_format()` a second time only
to build a log field, reuse the `upstream_format` binding instead of calling
`.supported_formats()` again.

- [ ] **Step 3: Verify no `native_format()` remains in dispatch**

Run: `grep -n "native_format()" crates/proxy/src/adapters/providers/routing.rs`
Expected: no matches in the four dispatch functions. (Matches may remain only
in `#[cfg(test)]` assertions, if any — leave those.)

- [ ] **Step 4: Full workspace test + lint**

Run: `cargo test --workspace`
Expected: PASS — identical results to Step 1, plus the new unit tests.

Run: `cargo clippy --workspace -- -D warnings`
Expected: no warnings.

- [ ] **Step 5: Behavioral spot-check against a live provider (optional but recommended)**

If a MiniMax key is configured, run the proxy against a throwaway DB copy on a
spare port and confirm `/v1/messages` still streams to completion (proves the
capability path is wired identically to before):

```bash
cargo build --release -p proxy
cp ~/.local/share/cli-router/proxy.db /tmp/p1.db
sqlite3 /tmp/p1.db "update settings set value='18790' where key='port'; update settings set value='/tmp/p1.db' where key='proxy_db';"
./target/release/cli-router-proxy --db /tmp/p1.db &  PID=$!
sleep 4
curl -sS -N http://127.0.0.1:18790/v1/messages -H 'content-type: application/json' \
  -d '{"model":"MiniMax-M2","max_tokens":200,"stream":true,"messages":[{"role":"user","content":"Say: ok"}]}' | tail -5
kill $PID; rm -f /tmp/p1.db*
```
Expected: a terminating stream ending in `message_stop`.

- [ ] **Step 6: Commit**

```bash
git add crates/proxy/src/adapters/providers/routing.rs
git commit -m "refactor(proxy): route by supported_formats() instead of native_format()"
```

---

## Self-Review

- **Spec coverage (Phase 1 slice):** the spec's "Ports change" introduces
  `FormatSupport` + `supported_formats()` (Task 1) and "Routing change"
  replaces `match native_format()` with capability selection (Tasks 2–3). This
  phase intentionally keeps `native_format()` (spec Phase 5 removes it) and
  does not change behavior — dual-for-all only lands in Phase 4 when providers
  override `supported_formats()` from URLs.
- **Placeholder scan:** no `TBD`/`TODO`; every code step shows full code; the
  repeated routing edit is expressed once with explicit per-site substitutions
  rather than "similar to above".
- **Type consistency:** `FormatSupport`, `has`, `sole`, `single`, `both`,
  `select_direction(ApiFormat, FormatSupport) -> (Direction, ApiFormat)`, and
  `supported_formats()` are named identically across Tasks 1–3.

## After this phase

Phase 2 (separate plan) adds `anthropic_base_url` + `openai_base_url` to
`ProviderConfig`, the SQLite schema, and `db_config.rs`, with a per-kind
backfill migration — still no behavior change. Phases 3–5 introduce
`UpstreamProvider`/`Quirks`/`preset`, cut the builder over and delete the old
structs (dual-for-all turns on here), then remove `native_format()` and add the
two-URL admin API + TUI wizard.
