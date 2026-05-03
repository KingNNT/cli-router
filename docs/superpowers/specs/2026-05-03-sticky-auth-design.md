# Sticky Auth Selection Design Spec

## Goal

Replace pure round-robin key selection inside a routing pool with **conversation-affinity rendezvous hashing**, so successive requests in the same conversation are pinned to the same upstream key. Upstreams that cache per-account (Z.ai context cache, Anthropic prompt cache) then keep hitting cache instead of re-warming on every key rotation.

## Why

Z.ai context cache is per-account: API key A and key B have separate cache pools. Round-robin across N keys for a multi-turn conversation wastes the cache, since every other turn lands on a cold key. Pinning a conversation to one key recovers ~80% prompt-token cost savings (verified: cached input is $0.11/M vs $0.6/M on GLM-4.6/4.7).

Soft sticky (fallback to another healthy key on cooldown without re-pinning) keeps availability high; cooldown lifts → next request resumes original key, cache rewarm not needed because Z.ai cache hasn't expired.

## Affinity input

Hash function `affinity_hash(headers, body) -> Option<u64>`:

1. **Header lookup** (priority): walk `config.affinity.headers` in order. First header present whose value is non-empty → siphash that value, return.
2. **Body fallback**: parse JSON body, extract:
   - **Anthropic format** (`/v1/messages`): `system` field (string or array of `{type:"text", text}` blocks concatenated) + `messages[0..2]` content (text portions only).
   - **OpenAI format** (`/v1/chat/completions`): `messages[0..2]` content (covers system + first user).
3. **Normalize**: trim, collapse whitespace runs to single space, lowercase.
4. **Truncate**: take first **1024 chars** of concatenated string. (Approximation of Z.ai's 1024-token cache trigger threshold; cheap to compute, sufficient entropy.)
5. **Hash**: `siphasher::sip::SipHasher13` with fixed key → `u64`.

Edge cases:
- JSON parse fails → return `None` → caller falls back to round-robin (current behavior).
- No system + no messages[0] → return `None`.
- Empty/whitespace-only after normalize → return `None`.

## Rendezvous picker

```rust
fn pick_sticky_entry<'a>(pool: &'a [PoolEntry], affinity: u64) -> Option<&'a PoolEntry> {
    pool.iter()
        .filter(|e| e.healthy())              // soft sticky: skip cooldown
        .max_by_key(|e| siphash_u64(&(e.id.as_str(), affinity)))
}
```

Properties:
- **Deterministic**: same `(pool, affinity)` → same winner.
- **Stable**: removing one key only re-shuffles ~1/N of conversations (HRW property).
- **Soft sticky**: `healthy()` filter routes around cooldown. When the original winner returns to healthy, next request with same affinity picks it again automatically — no state to clear.
- **Stateless**: no map, no TTL, no cleanup.

## PoolEntry stable ID

`PoolEntry` currently has `Arc<dyn Provider>` + `cooldown_until`. Add:

```rust
pub struct PoolEntry {
    provider: Arc<dyn Provider>,
    id: String,                  // = provider.name(), copied at construction
    cooldown_until: AtomicU64,
}
```

`id` derived from `ProviderConfig::name` (already unique within config). Hot-reload that renames a provider → re-shuffle of conversations hitting that provider (acceptable; renames are rare).

## Fallback chain on transport failure

When the picked entry returns 429 / 5xx / network error:

1. Mark that entry's cooldown (existing logic).
2. Re-run `pick_sticky_entry(pool, affinity)` — picks next-best healthy.
3. Retry up to `pool.len()` times before returning final error to client.

Identical to today's failover behavior, only the iteration order changes (rendezvous order, not next-index rotation).

## Config

Add optional `[affinity]` section to `config.toml`:

```toml
[affinity]
enabled = true                                          # default true
headers = ["x-session-id", "anthropic-session-id"]      # default
```

Defaults applied when section is missing (`enabled = true`, default headers).

To disable globally: `enabled = false` → skip affinity hash entirely, behave like today's round-robin.

No per-route override — affinity is global. (Per-route can be added later if needed; YAGNI.)

## Files to change

| File | Change |
|------|--------|
| `crates/proxy/src/adapters/providers/affinity.rs` | **NEW** — `affinity_hash`, `pick_sticky_entry`, helpers |
| `crates/proxy/src/adapters/providers/routing.rs` | Use `pick_sticky_entry` in `forward_round_robin` and `forward_round_robin_openai`; add `id` field to `PoolEntry` |
| `crates/proxy/src/config.rs` | Add `AffinityConfig`, parse `[affinity]` section |
| `crates/proxy/src/adapters/providers/builder.rs` | Pass `AffinityConfig` into routing provider |
| `crates/proxy-admin-api/src/lib.rs` | Add affinity status to `StatusResponse` (enabled, headers list) |
| `crates/proxy/src/frameworks/admin.rs` | Surface affinity status in `/admin/status` |
| `crates/proxy-tui/src/ui.rs` | Render `Affinity: enabled (3 headers + body fallback)` line in status panel |

Add dep: `siphasher = "1"` to `crates/proxy/Cargo.toml`.

## Tests

**`affinity.rs` unit (pure functions):**
- `hash_deterministic_for_same_body`
- `hash_differs_for_different_systems`
- `hash_normalizes_whitespace_variations`
- `hash_returns_none_on_invalid_json`
- `hash_returns_none_on_empty_messages`
- `hash_priority_header_over_body`
- `hash_skips_empty_header_value`
- `hash_uses_first_2_messages_only`
- `pick_returns_same_entry_for_same_affinity`
- `pick_returns_none_when_pool_empty`
- `pick_returns_none_when_all_cooldown`
- `pick_skips_cooldown_entries`
- `pick_redistributes_only_affected_when_one_entry_removed` (HRW property)

**`routing.rs` integration:**
- 2-key pool, 10 requests with same body → all hit same key
- 10 different bodies → distribute across keys (loose statistical assertion: each key gets ≥ 1 over 100 trials)
- Sticky entry on cooldown → request routes to other key; after cooldown expires, next request returns to sticky entry

## What stays the same

- 429/5xx cooldown lifecycle and `Retry-After` extraction.
- Glob matching, namespace routing, model resolution.
- Failover strategy (when `strategy = "failover"`, sticky is bypassed — rendezvous only kicks in for `round_robin`).
- Hot reload, config TOML parsing, env var interpolation.
- Streaming/non-streaming forward paths.

## Out of scope (YAGNI)

- Load-aware pinning (track per-key conversation count).
- Persistent affinity across restart (rendezvous is stateless, restart-proof).
- Affinity stats endpoint (`/admin/affinity/stats`).
- Per-route affinity config.
- Affinity for failover strategy (only `round_robin` benefits — failover always picks first healthy anyway).

## Risk and rollout

- Default-on, but disable knob exists (`enabled = false`).
- Behavior change is observable: same conversation now consistently hits same key. If a buggy key shadows the conversation, user notices fast.
- No data migration. No SQLite schema change.
- Reversible by toggling config; no persistent state to clean up.
