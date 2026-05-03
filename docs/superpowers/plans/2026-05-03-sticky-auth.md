# Sticky Auth Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace round-robin key rotation with rendezvous-hash conversation affinity, so the same conversation pins to the same upstream key and benefits from per-account caches (Z.ai context cache, Anthropic prompt cache).

**Architecture:** Pure functions in a new `affinity` module compute a stable `u64` hash from the request (header lookup with body fallback). The existing `forward_round_robin` and `forward_round_robin_openai` in `routing.rs` swap their `start = counter % pool_size` initialization for `pick_sticky_entry(pool, hash)` (rendezvous max-by-key over healthy entries). Soft sticky falls out for free: `is_cooling_down()` filter routes around hot keys, and when the original returns to healthy the next request with the same hash picks it again. Stateless — no map, no TTL, no cleanup.

**Tech Stack:** Rust 2024, axum + reqwest (already in repo), `siphasher = "1"` (new dep).

**Spec:** `docs/superpowers/specs/2026-05-03-sticky-auth-design.md`

---

## File Structure

| File | Role |
|---|---|
| `crates/proxy/Cargo.toml` | add `siphasher` dep |
| `crates/proxy/src/adapters/providers/affinity.rs` | **NEW** — `affinity_hash`, `pick_sticky_entry`, helpers (pure) |
| `crates/proxy/src/adapters/providers/mod.rs` | declare `mod affinity;` |
| `crates/proxy/src/adapters/providers/routing.rs` | `PoolEntry::id` field; rewrite start-index logic in both `forward_round_robin*` |
| `crates/proxy/src/adapters/providers/builder.rs` | accept `AffinityConfig`; pass `provider_name` into `PoolEntry::new` |
| `crates/proxy/src/config.rs` | parse `[affinity]` TOML section |
| `crates/proxy-admin-api/src/lib.rs` | `AffinityStatus` field on `StatusResponse` |
| `crates/proxy/src/frameworks/admin.rs` | populate affinity status in `GET /admin/status` |
| `crates/proxy-tui/src/ui.rs` | render affinity line in status panel |

---

## Task 1: Add siphasher dependency

**Files:**
- Modify: `crates/proxy/Cargo.toml`

- [ ] **Step 1: Add dep**

In `[dependencies]`:

```toml
siphasher = "1"
```

- [ ] **Step 2: Verify it resolves**

Run: `cargo check -p proxy`
Expected: PASS (no compile errors yet, just dep resolution).

- [ ] **Step 3: Commit**

```bash
git add crates/proxy/Cargo.toml crates/proxy/Cargo.lock 2>/dev/null; git add Cargo.lock
git commit -m "build(proxy): add siphasher dependency for affinity hashing"
```

---

## Task 2: Module skeleton — `affinity.rs`

**Files:**
- Create: `crates/proxy/src/adapters/providers/affinity.rs`
- Modify: `crates/proxy/src/adapters/providers/mod.rs`

- [ ] **Step 1: Create module file**

Write `crates/proxy/src/adapters/providers/affinity.rs`:

```rust
//! Conversation-affinity hashing and rendezvous picking.
//!
//! [`affinity_hash`] derives a stable `u64` from a request: prefer one of the
//! configured headers, fall back to the first 1024 chars of normalized
//! system + first two messages. Returns `None` when the request has neither
//! signal (caller should fall back to round-robin).
//!
//! [`pick_sticky_entry`] uses rendezvous (highest random weight) hashing over
//! the healthy entries of a pool, so the same affinity always picks the same
//! entry while a removed/cooling-down entry only re-routes its share (1/N).

use std::hash::{Hash, Hasher};

use http::HeaderMap;
use siphasher::sip::SipHasher13;

/// Compute the affinity hash for a request.
pub fn affinity_hash(headers: &HeaderMap, body: &[u8], header_names: &[String]) -> Option<u64> {
    if let Some(h) = header_lookup(headers, header_names) {
        return Some(siphash_str(&h));
    }
    let extracted = extract_body_signal(body)?;
    let normalized = normalize(&extracted);
    if normalized.is_empty() {
        return None;
    }
    let truncated: String = normalized.chars().take(1024).collect();
    Some(siphash_str(&truncated))
}

fn header_lookup(headers: &HeaderMap, names: &[String]) -> Option<String> {
    for name in names {
        if let Some(v) = headers.get(name) {
            if let Ok(s) = v.to_str() {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

/// Extract system + first two messages text content as one string.
/// Returns `None` if the body is not parseable JSON or has no signal.
fn extract_body_signal(body: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    let mut buf = String::new();

    // Anthropic format: top-level `system`
    if let Some(sys) = v.get("system") {
        append_value_text(sys, &mut buf);
    }

    if let Some(msgs) = v.get("messages").and_then(|m| m.as_array()) {
        for msg in msgs.iter().take(2) {
            if let Some(content) = msg.get("content") {
                append_value_text(content, &mut buf);
            }
        }
    }

    if buf.is_empty() { None } else { Some(buf) }
}

/// Append text from a JSON value:
/// - string: the string itself
/// - array: each element's `.text` field if present, else recurse into the element
/// - object with `.text`: that text
/// Other shapes are ignored.
fn append_value_text(v: &serde_json::Value, out: &mut String) {
    match v {
        serde_json::Value::String(s) => {
            out.push(' ');
            out.push_str(s);
        }
        serde_json::Value::Array(items) => {
            for item in items {
                append_value_text(item, out);
            }
        }
        serde_json::Value::Object(map) => {
            if let Some(t) = map.get("text").and_then(|x| x.as_str()) {
                out.push(' ');
                out.push_str(t);
            }
        }
        _ => {}
    }
}

/// Lowercase + collapse whitespace runs to a single space + trim.
fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_ws = true; // start as ws so we trim leading
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !prev_ws {
                out.push(' ');
                prev_ws = true;
            }
        } else {
            for lc in ch.to_lowercase() {
                out.push(lc);
            }
            prev_ws = false;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

/// Pool member trait so the picker stays decoupled from the concrete `PoolEntry`.
pub trait PoolMember {
    /// Stable identifier (provider name).
    fn id(&self) -> &str;
    /// `false` when the member is in cooldown.
    fn healthy(&self) -> bool;
}

/// Rendezvous-hash picker over healthy pool members. Returns `None` when
/// the pool is empty or every member is unhealthy.
pub fn pick_sticky_entry<'a, T: PoolMember>(pool: &'a [T], affinity: u64) -> Option<&'a T> {
    pool.iter()
        .filter(|e| e.healthy())
        .max_by_key(|e| score(e.id(), affinity))
}

fn score(id: &str, affinity: u64) -> u64 {
    let mut h = SipHasher13::new();
    id.hash(&mut h);
    affinity.hash(&mut h);
    h.finish()
}

fn siphash_str(s: &str) -> u64 {
    let mut h = SipHasher13::new();
    s.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    // Tests added in subsequent tasks.
}
```

- [ ] **Step 2: Declare module**

Edit `crates/proxy/src/adapters/providers/mod.rs` and add:

```rust
pub mod affinity;
```

(Adjacent to existing `pub mod routing;`, `pub mod live;`, etc. — match the style used in the file.)

- [ ] **Step 3: Compile-check**

Run: `cargo check -p proxy`
Expected: PASS (warnings about unused `pub` items are fine for now).

- [ ] **Step 4: Commit**

```bash
git add crates/proxy/src/adapters/providers/affinity.rs crates/proxy/src/adapters/providers/mod.rs
git commit -m "feat(proxy): scaffold affinity module"
```

---

## Task 3: Test — body extraction + normalization (TDD)

**Files:**
- Test: `crates/proxy/src/adapters/providers/affinity.rs` (in-file `mod tests`)

- [ ] **Step 1: Add tests**

Replace the empty `mod tests` block in `affinity.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderMap;

    fn no_headers() -> HeaderMap {
        HeaderMap::new()
    }

    #[test]
    fn hash_deterministic_for_same_body() {
        let body = br#"{"system":"You are helpful","messages":[{"role":"user","content":"hi"}]}"#;
        let a = affinity_hash(&no_headers(), body, &[]).unwrap();
        let b = affinity_hash(&no_headers(), body, &[]).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn hash_differs_for_different_systems() {
        let b1 = br#"{"system":"You are A","messages":[{"role":"user","content":"hi"}]}"#;
        let b2 = br#"{"system":"You are B","messages":[{"role":"user","content":"hi"}]}"#;
        let h1 = affinity_hash(&no_headers(), b1, &[]).unwrap();
        let h2 = affinity_hash(&no_headers(), b2, &[]).unwrap();
        assert_ne!(h1, h2);
    }

    #[test]
    fn hash_normalizes_whitespace_variations() {
        let b1 = br#"{"system":"hello   world","messages":[]}"#;
        let b2 = br#"{"system":"hello\tworld","messages":[]}"#;
        let b3 = br#"{"system":" HELLO world ","messages":[]}"#;
        let h1 = affinity_hash(&no_headers(), b1, &[]).unwrap();
        let h2 = affinity_hash(&no_headers(), b2, &[]).unwrap();
        let h3 = affinity_hash(&no_headers(), b3, &[]).unwrap();
        assert_eq!(h1, h2, "tab vs spaces should normalize");
        assert_eq!(h1, h3, "case + leading/trailing should normalize");
    }

    #[test]
    fn hash_returns_none_on_invalid_json() {
        let body = b"this is not json";
        assert!(affinity_hash(&no_headers(), body, &[]).is_none());
    }

    #[test]
    fn hash_returns_none_on_empty_messages_and_no_system() {
        let body = br#"{"messages":[]}"#;
        assert!(affinity_hash(&no_headers(), body, &[]).is_none());
    }

    #[test]
    fn hash_handles_anthropic_array_system() {
        let body = br#"{"system":[{"type":"text","text":"part one"},{"type":"text","text":"part two"}],"messages":[]}"#;
        let h = affinity_hash(&no_headers(), body, &[]).unwrap();
        // Different array → different hash
        let body2 = br#"{"system":[{"type":"text","text":"part one"}],"messages":[]}"#;
        let h2 = affinity_hash(&no_headers(), body2, &[]).unwrap();
        assert_ne!(h, h2);
    }

    #[test]
    fn hash_uses_only_first_two_messages() {
        let b1 = br#"{"messages":[{"role":"user","content":"a"},{"role":"assistant","content":"b"},{"role":"user","content":"c"}]}"#;
        let b2 = br#"{"messages":[{"role":"user","content":"a"},{"role":"assistant","content":"b"},{"role":"user","content":"DIFFERENT"}]}"#;
        let h1 = affinity_hash(&no_headers(), b1, &[]).unwrap();
        let h2 = affinity_hash(&no_headers(), b2, &[]).unwrap();
        assert_eq!(h1, h2, "third message should not affect hash");
    }

    #[test]
    fn hash_handles_openai_array_content_parts() {
        let body = br#"{"messages":[{"role":"system","content":[{"type":"text","text":"sys text"}]},{"role":"user","content":"hi"}]}"#;
        assert!(affinity_hash(&no_headers(), body, &[]).is_some());
    }
}
```

- [ ] **Step 2: Run tests, verify pass**

Run: `cargo test -p proxy --lib adapters::providers::affinity::tests`
Expected: **all 7 tests PASS** (the implementation in Task 2 already covers these).

- [ ] **Step 3: Commit**

```bash
git add crates/proxy/src/adapters/providers/affinity.rs
git commit -m "test(proxy): cover affinity body extraction + normalization"
```

---

## Task 4: Test — header priority (TDD)

**Files:**
- Test: `crates/proxy/src/adapters/providers/affinity.rs`

- [ ] **Step 1: Add tests**

Add inside `mod tests`:

```rust
#[test]
fn hash_priority_header_over_body() {
    let mut h = HeaderMap::new();
    h.insert("x-session-id", "abc123".parse().unwrap());

    let b1 = br#"{"system":"A","messages":[]}"#;
    let b2 = br#"{"system":"B","messages":[]}"#;

    let names = vec!["x-session-id".to_string()];
    let h1 = affinity_hash(&h, b1, &names).unwrap();
    let h2 = affinity_hash(&h, b2, &names).unwrap();
    assert_eq!(h1, h2, "header should override body");
}

#[test]
fn hash_skips_empty_header_value() {
    let mut h = HeaderMap::new();
    h.insert("x-session-id", "   ".parse().unwrap());

    let body = br#"{"system":"hi","messages":[]}"#;
    let names = vec!["x-session-id".to_string()];
    // empty/whitespace header → fall through to body
    let with_empty = affinity_hash(&h, body, &names).unwrap();
    let body_only = affinity_hash(&HeaderMap::new(), body, &[]).unwrap();
    assert_eq!(with_empty, body_only);
}

#[test]
fn hash_walks_header_list_in_order() {
    let mut h = HeaderMap::new();
    h.insert("anthropic-session-id", "from-anthropic".parse().unwrap());

    let body = br#"{"system":"hi","messages":[]}"#;
    let names = vec![
        "x-session-id".to_string(),         // not present
        "anthropic-session-id".to_string(), // present
    ];
    let result = affinity_hash(&h, body, &names).unwrap();

    // Construct the same hash a different way to confirm we matched the right header.
    let mut h2 = HeaderMap::new();
    h2.insert("x-session-id", "from-anthropic".parse().unwrap());
    let names2 = vec!["x-session-id".to_string()];
    let parallel = affinity_hash(&h2, b"{}", &names2).unwrap();
    assert_eq!(result, parallel, "same value through different header name yields same hash");
}
```

- [ ] **Step 2: Run, verify pass**

Run: `cargo test -p proxy --lib adapters::providers::affinity::tests`
Expected: 10 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/proxy/src/adapters/providers/affinity.rs
git commit -m "test(proxy): cover affinity header priority"
```

---

## Task 5: Test — rendezvous picker (TDD)

**Files:**
- Test: `crates/proxy/src/adapters/providers/affinity.rs`

- [ ] **Step 1: Add tests**

Add inside `mod tests`:

```rust
struct MockEntry {
    id: String,
    healthy: bool,
}
impl PoolMember for MockEntry {
    fn id(&self) -> &str { &self.id }
    fn healthy(&self) -> bool { self.healthy }
}
fn entry(id: &str, healthy: bool) -> MockEntry {
    MockEntry { id: id.into(), healthy }
}

#[test]
fn pick_returns_none_when_pool_empty() {
    let pool: Vec<MockEntry> = vec![];
    assert!(pick_sticky_entry(&pool, 12345).is_none());
}

#[test]
fn pick_returns_none_when_all_unhealthy() {
    let pool = vec![entry("a", false), entry("b", false)];
    assert!(pick_sticky_entry(&pool, 12345).is_none());
}

#[test]
fn pick_is_deterministic_for_same_inputs() {
    let pool = vec![entry("a", true), entry("b", true), entry("c", true)];
    let r1 = pick_sticky_entry(&pool, 42).unwrap().id().to_string();
    let r2 = pick_sticky_entry(&pool, 42).unwrap().id().to_string();
    assert_eq!(r1, r2);
}

#[test]
fn pick_skips_unhealthy_entries() {
    // Find an affinity for which "a" is the natural winner, then mark "a"
    // unhealthy and confirm the picker switches.
    let healthy_pool = vec![entry("a", true), entry("b", true), entry("c", true)];
    let mut affinity = 0u64;
    let mut chose_a = String::new();
    for cand in 0..1000u64 {
        if pick_sticky_entry(&healthy_pool, cand).unwrap().id() == "a" {
            affinity = cand;
            chose_a = "a".into();
            break;
        }
    }
    assert_eq!(chose_a, "a", "should find an affinity that picks 'a'");

    let pool_a_down = vec![entry("a", false), entry("b", true), entry("c", true)];
    let alt = pick_sticky_entry(&pool_a_down, affinity).unwrap().id().to_string();
    assert_ne!(alt, "a", "must skip unhealthy 'a'");
}

#[test]
fn pick_redistributes_only_affected_when_one_entry_removed() {
    // HRW property: removing one entry should leave the picks for other
    // affinities mostly stable. We assert weakly: < 50% of trials change.
    let before = vec![entry("a", true), entry("b", true), entry("c", true), entry("d", true)];
    let after = vec![entry("a", true), entry("b", true), entry("c", true)]; // dropped "d"

    let mut affected = 0;
    let mut total = 0;
    for affinity in 0..200u64 {
        let pre = pick_sticky_entry(&before, affinity).unwrap().id().to_string();
        if pre == "d" {
            // Affinity that mapped to "d" *must* re-route — not counted in stability.
            continue;
        }
        total += 1;
        let post = pick_sticky_entry(&after, affinity).unwrap().id().to_string();
        if pre != post {
            affected += 1;
        }
    }
    assert!(total > 100, "sanity: most affinities don't pick 'd'");
    assert_eq!(affected, 0, "non-'d' picks must be unchanged when 'd' is removed");
}
```

- [ ] **Step 2: Run, verify pass**

Run: `cargo test -p proxy --lib adapters::providers::affinity::tests`
Expected: 15 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/proxy/src/adapters/providers/affinity.rs
git commit -m "test(proxy): cover rendezvous picker properties"
```

---

## Task 6: Add `id` and `healthy()` to `PoolEntry`

**Files:**
- Modify: `crates/proxy/src/adapters/providers/routing.rs`
- Modify: `crates/proxy/src/adapters/providers/builder.rs`

- [ ] **Step 1: Update `PoolEntry`**

In `routing.rs`, replace the `PoolEntry` struct (lines ~40-91) with:

```rust
pub(super) struct PoolEntry {
    provider: Arc<dyn Provider>,
    /// Stable identifier — provider name from config. Used for affinity scoring.
    id: String,
    /// Epoch millis when cooldown expires. 0 = healthy.
    cooldown_until: AtomicU64,
}

impl PoolEntry {
    fn new(provider: Arc<dyn Provider>, id: String) -> Self {
        Self {
            provider,
            id,
            cooldown_until: AtomicU64::new(0),
        }
    }

    fn is_cooling_down(&self) -> bool {
        let until = self.cooldown_until.load(Ordering::Relaxed);
        if until == 0 {
            return false;
        }
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        if now_ms >= until {
            self.cooldown_until.store(0, Ordering::Relaxed);
            return false;
        }
        true
    }

    fn set_cooldown(&self, duration_ms: u64) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.cooldown_until
            .store(now_ms + duration_ms, Ordering::Relaxed);
    }

    fn remaining_cooldown_ms(&self) -> u64 {
        let until = self.cooldown_until.load(Ordering::Relaxed);
        if until == 0 {
            return 0;
        }
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        until.saturating_sub(now_ms)
    }
}

impl super::affinity::PoolMember for PoolEntry {
    fn id(&self) -> &str { &self.id }
    fn healthy(&self) -> bool { !self.is_cooling_down() }
}
```

- [ ] **Step 2: Update construction sites**

Find every `PoolEntry::new(...)` call in `routing.rs` and `builder.rs`. There is one in the builder's `rule()` method that pushes `primary` and `fallback`. Replace those calls, threading the provider name:

In `routing.rs`'s `rule()` (or wherever `PoolEntry::new(provider)` is called now in builder):

```rust
pool.push(PoolEntry::new(primary.clone(), primary.name().to_string()));
for fb in fallback {
    let id = fb.name().to_string();
    pool.push(PoolEntry::new(fb, id));
}
```

(`Provider::name()` already exists on the trait — it returns `&'static str`. Confirm by reading `crates/proxy/src/adapters/providers/mod.rs` if uncertain.)

If any unit tests in `routing.rs` construct `PoolEntry::new(arc)` directly, update them with a name string (use `"test-a"`, `"test-b"`, etc.).

- [ ] **Step 3: Run all proxy tests**

Run: `cargo test -p proxy`
Expected: all PASS. If any test fails because `Provider::name()` collides (two providers with same `"anthropic"` static name), promote the test ID to a unique string in the test setup.

- [ ] **Step 4: Commit**

```bash
git add crates/proxy/src/adapters/providers/routing.rs crates/proxy/src/adapters/providers/builder.rs
git commit -m "refactor(proxy): give PoolEntry a stable id field"
```

---

## Task 7: AffinityConfig in `config.rs`

**Files:**
- Modify: `crates/proxy/src/config.rs`

- [ ] **Step 1: Add the type**

Near the other `pub struct` definitions, add:

```rust
/// Affinity hashing config — controls how requests are pinned to upstream keys.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AffinityConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_affinity_headers")]
    pub headers: Vec<String>,
}

impl Default for AffinityConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            headers: default_affinity_headers(),
        }
    }
}

fn default_true() -> bool { true }

fn default_affinity_headers() -> Vec<String> {
    vec!["x-session-id".to_string(), "anthropic-session-id".to_string()]
}
```

- [ ] **Step 2: Add to `Config`**

Find the `pub struct Config { ... }` block and add the field:

```rust
pub struct Config {
    // ...existing fields...
    pub providers: Vec<ProviderConfig>,
    pub routing: Vec<RoutingRule>,

    /// Conversation-affinity hashing. Defaults to enabled with sensible header list.
    #[serde(default)]
    pub affinity: AffinityConfig,
}
```

- [ ] **Step 3: Add a parse test**

In the `#[cfg(test)] mod tests` block at the bottom of `config.rs`, add:

```rust
#[test]
fn affinity_defaults_when_section_missing() {
    let toml = r#"
        [[providers]]
        name = "p"
        kind = "anthropic"
        auth = { type = "passthrough" }

        [[routing]]
        match = { model = "*" }
        provider = "p"
    "#;
    let cfg: Config = toml::from_str(toml).unwrap();
    assert!(cfg.affinity.enabled);
    assert_eq!(cfg.affinity.headers.len(), 2);
}

#[test]
fn affinity_custom_headers_parses() {
    let toml = r#"
        [[providers]]
        name = "p"
        kind = "anthropic"
        auth = { type = "passthrough" }

        [[routing]]
        match = { model = "*" }
        provider = "p"

        [affinity]
        enabled = false
        headers = ["x-trace-id"]
    "#;
    let cfg: Config = toml::from_str(toml).unwrap();
    assert!(!cfg.affinity.enabled);
    assert_eq!(cfg.affinity.headers, vec!["x-trace-id".to_string()]);
}
```

- [ ] **Step 4: Run, verify pass**

Run: `cargo test -p proxy --lib config::tests::affinity`
Expected: 2 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/proxy/src/config.rs
git commit -m "feat(proxy): parse [affinity] config section"
```

---

## Task 8: Wire affinity into `forward_round_robin` (Anthropic path)

**Files:**
- Modify: `crates/proxy/src/adapters/providers/routing.rs`

- [ ] **Step 1: Add affinity field to `RoutingProvider`**

Update the struct and builder:

```rust
pub struct RoutingProvider {
    rules: Vec<Route>,
    rr_counter: AtomicUsize,
    leaves: std::collections::HashMap<String, Arc<dyn Provider>>,
    affinity: AffinityConfig,
}
```

(Add `use crate::config::AffinityConfig;` at top of file.)

In `RoutingProviderBuilder`, add `affinity: AffinityConfig` field with `Default::default()` and a setter:

```rust
pub fn affinity(mut self, affinity: AffinityConfig) -> Self {
    self.affinity = affinity;
    self
}
```

And include it in `build()`:

```rust
pub fn build(self) -> RoutingProvider {
    RoutingProvider {
        rules: self.rules,
        rr_counter: AtomicUsize::new(0),
        leaves: self.leaves,
        affinity: self.affinity,
    }
}
```

- [ ] **Step 2: Replace the round-robin start index**

Find `forward_round_robin`. Replace the starting-index block:

```rust
// OLD
let start = self.rr_counter.fetch_add(1, Ordering::Relaxed) % pool_size;

for offset in 0..pool_size {
    let idx = (start + offset) % pool_size;
    let entry = &route.pool[idx];
```

with:

```rust
// NEW
let order = self.compute_attempt_order(&route.pool, headers, &body);

for &idx in &order {
    let entry = &route.pool[idx];
```

Then add a helper method on `RoutingProvider` (place it near the other helpers, e.g. above `forward_round_robin`):

```rust
fn compute_attempt_order(
    &self,
    pool: &[PoolEntry],
    headers: &HeaderMap,
    body: &[u8],
) -> Vec<usize> {
    let pool_size = pool.len();

    // Sticky path: hash request, sort indices by rendezvous score (highest first).
    if self.affinity.enabled {
        if let Some(hash) = super::affinity::affinity_hash(headers, body, &self.affinity.headers) {
            let mut scored: Vec<(usize, u64)> = pool
                .iter()
                .enumerate()
                .map(|(i, e)| (i, super::affinity::score_for(e.id(), hash)))
                .collect();
            scored.sort_by(|a, b| b.1.cmp(&a.1));
            return scored.into_iter().map(|(i, _)| i).collect();
        }
    }

    // Fallback: round-robin (original behavior).
    let start = self.rr_counter.fetch_add(1, Ordering::Relaxed) % pool_size;
    (0..pool_size).map(|offset| (start + offset) % pool_size).collect()
}
```

Add a public `score_for` to `affinity.rs` that wraps the private `score` (so we don't expose the hasher internals):

```rust
// in affinity.rs, replace `fn score(...)` with `pub fn score_for(...)`
pub fn score_for(id: &str, affinity: u64) -> u64 {
    let mut h = SipHasher13::new();
    id.hash(&mut h);
    affinity.hash(&mut h);
    h.finish()
}
```

Update the internal call site in `pick_sticky_entry`:

```rust
.max_by_key(|e| score_for(e.id(), affinity))
```

- [ ] **Step 3: Run existing tests**

Run: `cargo test -p proxy`
Expected: all PASS. The behavior for tests that don't set affinity (default `AffinityConfig` is `enabled=true`) may differ — see step 4.

- [ ] **Step 4: If round-robin order tests fail**

If a test asserts strict round-robin order with a single body that produces an affinity hash, add `.affinity(AffinityConfig { enabled: false, ..Default::default() })` to the test's builder so it falls back to round-robin. Justify in commit: tests that exercise round-robin rotation specifically must opt out of sticky.

- [ ] **Step 5: Commit**

```bash
git add crates/proxy/src/adapters/providers/routing.rs crates/proxy/src/adapters/providers/affinity.rs
git commit -m "feat(proxy): use sticky rendezvous order in forward_round_robin"
```

---

## Task 9: Wire affinity into `forward_round_robin_openai`

**Files:**
- Modify: `crates/proxy/src/adapters/providers/routing.rs`

- [ ] **Step 1: Apply the same edit**

In `forward_round_robin_openai`, replace the same `let start = ...; for offset ... let idx = (start+offset)%pool_size;` block with:

```rust
let order = self.compute_attempt_order(&route.pool, headers, &body);

for &idx in &order {
    let entry = &route.pool[idx];
```

(same shape as Task 8 step 2, calling the same helper).

- [ ] **Step 2: Run all proxy tests**

Run: `cargo test -p proxy`
Expected: all PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/proxy/src/adapters/providers/routing.rs
git commit -m "feat(proxy): use sticky rendezvous order in forward_round_robin_openai"
```

---

## Task 10: Pass `AffinityConfig` from `main.rs` through builder

**Files:**
- Modify: `crates/proxy/src/adapters/providers/builder.rs`
- Modify: `crates/proxy/src/main.rs`

- [ ] **Step 1: Wire builder**

Find the function in `builder.rs` that constructs the `RoutingProvider` (likely `build_routing_provider(cfg, ...)`). Add a parameter (or pull from `cfg.affinity`) and pass through:

```rust
let routing = RoutingProvider::builder()
    .leaves(leaves)
    .affinity(cfg.affinity.clone())  // ← NEW
    .rule(...)?
    .rule(...)?
    .build();
```

Confirm `AffinityConfig: Clone` (it is — `#[derive(Clone)]`).

- [ ] **Step 2: Verify `main.rs` builds**

Run: `cargo build --workspace`
Expected: PASS.

- [ ] **Step 3: Run full test suite**

Run: `cargo test --workspace`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/proxy/src/adapters/providers/builder.rs crates/proxy/src/main.rs
git commit -m "feat(proxy): thread affinity config through builder to routing provider"
```

---

## Task 11: Integration test — same body pins to same key

**Files:**
- Create: `crates/proxy/tests/sticky_affinity.rs`

- [ ] **Step 1: Write the test**

Reference an existing integration test in `crates/proxy/tests/` for the wiremock setup pattern (e.g., look for `tests/load_balancing.rs` or similar — read it to copy fixtures). Then write `tests/sticky_affinity.rs`:

```rust
//! Integration: sticky-affinity pins a conversation to a single upstream
//! across multiple calls, with two upstreams in a round-robin pool.

use bytes::Bytes;
use http::HeaderMap;
use std::sync::Arc;

use cli_router_proxy::adapters::providers::affinity::{affinity_hash, score_for, PoolMember};

#[test]
fn same_affinity_picks_same_id_in_two_key_pool() {
    struct E { id: String, healthy: bool }
    impl PoolMember for E {
        fn id(&self) -> &str { &self.id }
        fn healthy(&self) -> bool { self.healthy }
    }

    let pool: Vec<E> = vec![
        E { id: "key-a".into(), healthy: true },
        E { id: "key-b".into(), healthy: true },
    ];

    let body = br#"{"system":"long enough","messages":[{"role":"user","content":"hello world"}]}"#;
    let h1 = affinity_hash(&HeaderMap::new(), body, &[]).unwrap();
    let h2 = affinity_hash(&HeaderMap::new(), body, &[]).unwrap();

    let pick1 = pool.iter().filter(|e| e.healthy()).max_by_key(|e| score_for(e.id(), h1)).unwrap();
    let pick2 = pool.iter().filter(|e| e.healthy()).max_by_key(|e| score_for(e.id(), h2)).unwrap();
    assert_eq!(pick1.id(), pick2.id());
}

#[test]
fn distributes_across_keys_for_different_bodies() {
    struct E { id: String, healthy: bool }
    impl PoolMember for E {
        fn id(&self) -> &str { &self.id }
        fn healthy(&self) -> bool { self.healthy }
    }
    let pool: Vec<E> = vec![
        E { id: "key-a".into(), healthy: true },
        E { id: "key-b".into(), healthy: true },
    ];
    let mut a_count = 0;
    let mut b_count = 0;
    for i in 0..200 {
        let body = format!(r#"{{"system":"sys-{}","messages":[]}}"#, i);
        let hash = affinity_hash(&HeaderMap::new(), body.as_bytes(), &[]).unwrap();
        let p = pool.iter().filter(|e| e.healthy()).max_by_key(|e| score_for(e.id(), hash)).unwrap();
        match p.id() {
            "key-a" => a_count += 1,
            "key-b" => b_count += 1,
            _ => unreachable!(),
        }
    }
    assert!(a_count > 50 && b_count > 50, "expected rough balance, got a={a_count} b={b_count}");
}
```

The lib name `cli_router_proxy` should match the proxy crate's `lib.rs` exports. Verify with `grep -n 'pub use' crates/proxy/src/lib.rs` and adjust if needed; if `affinity` isn't re-exported, add `pub use crate::adapters::providers::affinity;` in `lib.rs` first.

- [ ] **Step 2: Run the integration test**

Run: `cargo test -p proxy --test sticky_affinity`
Expected: 2 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/proxy/tests/sticky_affinity.rs crates/proxy/src/lib.rs 2>/dev/null
git commit -m "test(proxy): integration test for sticky affinity over a 2-key pool"
```

---

## Task 12: Surface affinity status in admin DTO

**Files:**
- Modify: `crates/proxy-admin-api/src/lib.rs`
- Modify: `crates/proxy/src/frameworks/admin.rs`

- [ ] **Step 1: Add the DTO**

In `crates/proxy-admin-api/src/lib.rs`, add:

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct AffinityStatus {
    pub enabled: bool,
    pub headers: Vec<String>,
}
```

Add the field to `StatusResponse`:

```rust
pub struct StatusResponse {
    // ...existing fields...
    pub affinity: AffinityStatus,
}
```

- [ ] **Step 2: Populate it in the handler**

In `crates/proxy/src/frameworks/admin.rs`, find `status_handler` and where the `StatusResponse` is constructed, add:

```rust
affinity: AffinityStatus {
    enabled: state.config.affinity.enabled,
    headers: state.config.affinity.headers.clone(),
},
```

(`state.config` exposure depends on existing `AdminState` shape; if config isn't on state, plumb it through — read `AdminState` definition first and add a `config: Arc<Config>` field if missing, mirroring how other fields flow.)

- [ ] **Step 3: Run tests**

Run: `cargo test --workspace`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/proxy-admin-api/src/lib.rs crates/proxy/src/frameworks/admin.rs crates/proxy/src/frameworks/mod.rs 2>/dev/null
git commit -m "feat(proxy): expose affinity status via /admin/status"
```

---

## Task 13: Render affinity in proxy-tui status

**Files:**
- Modify: `crates/proxy-tui/src/ui.rs`
- Modify: `crates/proxy-tui/src/app.rs` (if a state field is needed)

- [ ] **Step 1: Locate status panel rendering**

Find the function that renders the status panel — search for an existing status/info text block that prints provider names or version. Match its style.

- [ ] **Step 2: Add the affinity line**

Add a line after the existing status fields:

```rust
let affinity_line = if status.affinity.enabled {
    format!("Affinity: enabled ({} headers + body fallback)", status.affinity.headers.len())
} else {
    "Affinity: disabled (round-robin)".to_string()
};
lines.push(Line::from(affinity_line));
```

(Adjust `lines.push(...)` to whatever the surrounding code uses — `Spans`, `vec!` builder, etc.)

- [ ] **Step 3: Compile**

Run: `cargo build -p proxy-tui`
Expected: PASS.

- [ ] **Step 4: Manual smoke test**

Start proxy in one terminal: `cargo run -p proxy`
Start TUI in another: `cargo run -p proxy-tui`
Confirm the status panel shows `Affinity: enabled (2 headers + body fallback)`.

- [ ] **Step 5: Commit**

```bash
git add crates/proxy-tui/src/ui.rs crates/proxy-tui/src/app.rs 2>/dev/null
git commit -m "feat(proxy-tui): show affinity status line"
```

---

## Task 14: Workspace-wide gate

**Files:** none

- [ ] **Step 1: Run all gates**

```bash
cargo fmt
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

Expected: all PASS.

- [ ] **Step 2: Verify with manual run**

```bash
ZAI_API_KEY=$ZAI_API_KEY cargo run -p proxy &
PROXY_PID=$!
sleep 1
python3 /tmp/zai_cache_test.py | grep -A3 "proxy call"   # from earlier session
kill $PROXY_PID
```

Expected: cache_read_tokens > 0 on call 2, identical key chosen across both calls (visible in `~/.local/share/cli-router/proxy.db`).

- [ ] **Step 3: Final commit (if any fmt churn)**

```bash
git status
# only commit if there are stragglers
git add -A && git commit -m "chore(proxy): cargo fmt cleanup" || true
```

---

## Self-review

- [x] **Spec coverage**: all 7 sections of the spec touched (affinity input → Tasks 2-4; rendezvous picker → Task 5; PoolEntry id → Task 6; fallback chain → Tasks 8-9 keep existing fallback iteration; config → Task 7; files-to-change → Tasks 6-13; tests → Tasks 3-5, 11).
- [x] **No placeholders**: every code block has actual content. No "TBD" or "similar to above".
- [x] **Type consistency**: `PoolMember` trait used in both `affinity.rs` (Task 2/5) and `routing.rs` impl (Task 6); `AffinityConfig` constructed once in Task 7 and threaded in Tasks 8/10.
- [x] **Frequent commits**: every TDD cycle ends with `git commit`; total ~13 commits over the plan.

## Out-of-scope (per spec)

- Load-aware pinning, persistent affinity, per-route affinity config, affinity stats endpoint, failover-strategy stickiness — none implemented; spec marked YAGNI.
