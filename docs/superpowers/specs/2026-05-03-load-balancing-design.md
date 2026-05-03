# Load Balancing Design Spec

## Goal

Add round-robin load balancing to the routing provider so multiple accounts for the same LLM provider can share traffic, with automatic cooldown on rate-limit (429) or 5xx errors.

## Config

New optional `strategy` field on routing rules. Defaults to `"failover"` (current behavior).

```toml
[[routing]]
match = { model = "*" }
strategy = "round_robin"
provider = "anthropic-a"
fallback = ["anthropic-b", "anthropic-c"]
```

Two strategies:
- **`failover`** (default) — always try `provider` first, then `fallback` in order. Current behavior, unchanged.
- **`round_robin`** — rotate across all providers in the pool (`provider` + `fallback`). On 429/5xx, skip that provider and try next. Track cooldown per provider.

## Algorithm

### Round-robin rotation

```
Pool: [A, B, C]     (provider + fallback flattened)
Counter: AtomicUsize (starts at 0, wraps around)

Request 1 → pool[counter % 3] = A, counter++ → 1
Request 2 → pool[counter % 3] = B, counter++ → 2
Request 3 → pool[counter % 3] = C, counter++ → 3
Request 4 → pool[counter % 3] = A, counter++ → 4
...
```

### Cooldown on 429/5xx

When a provider returns 429 or 5xx:
1. Mark that provider as "cooling down" with a timestamp (now + retry_after_ms)
2. Skip it in the rotation until cooldown expires
3. Try the next provider in the pool
4. If all providers are cooling down, return 429 to the client with the shortest remaining cooldown as `Retry-After`

### Cooldown data structure

```rust
struct ProviderHealth {
    cooldown_until: AtomicU64,  // epoch ms, 0 = healthy
}
```

One `ProviderHealth` per provider in the pool. Stored in the `Route` alongside the provider Arc.

### Retry-After extraction

On 429, read `retry-after` header from the upstream response. Default to 60 seconds if missing. Use this as the cooldown duration.

## Files to change

| File | Change |
|------|--------|
| `config.rs` | Add `strategy` field to `RoutingRule` |
| `proxy-admin-api/src/lib.rs` | Add `strategy` to `RoutingRulePayload` |
| `routing.rs` | Implement round-robin + cooldown in `RoutingProvider` |
| `builder.rs` | Pass strategy to `RoutingProviderBuilder` |
| `admin.rs` | Map strategy field in DTO conversions |

## What stays the same

- `failover` strategy is the default and behaves exactly as today
- Glob matching on model field (first-match-wins across rules)
- Streaming retry limitation stays (known tech debt)
- Config validation, TOML parsing, hot reload — all unchanged
