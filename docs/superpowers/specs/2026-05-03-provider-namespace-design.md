# Provider Namespace Routing

Date: 2026-05-03

## Summary

Allow clients to explicitly target a specific provider by prefixing the model
name with the provider's name, separated by `/`. Example: `zai/glm-5` routes
directly to the provider named `"zai"` and sends `glm-5` to the upstream.

## Motivation

Currently, routing is rule-based (glob matching on model names). Clients cannot
override routing to target a specific provider — especially useful when multiple
providers of the same kind exist (e.g. `anthropic-work`, `anthropic-personal`).

## Model Format

| Client sends        | Namespace | Model forwarded upstream |
|---------------------|-----------|--------------------------|
| `zai/glm-5`         | `zai`     | `glm-5`                  |
| `anthropic-work/claude-sonnet-4` | `anthropic-work` | `claude-sonnet-4` |
| `glm-5`             | *(none)*  | `glm-5` (routed by rules)|

**Parsing rule:** split on the first `/`. Everything before is the provider
name; everything after is the actual model name. Only the first `/` is
treated as a namespace delimiter.

## Routing Flow

```
parse model from body
  ├─ has namespace? → lookup provider by name → rewrite body → forward
  └─ no namespace   → existing glob-based routing rules (unchanged)
```

Namespace **always wins** over routing rules. If a namespace is present, routing
rules are bypassed entirely.

## Implementation (Approach A: extend RoutingProvider)

### Changes to `RoutingProvider` (`routing.rs`)

1. **New field:** `leaves: HashMap<String, Arc<dyn Provider>>` — the name-to-provider map.
   Already built in `builder.rs` by `build_leaves()`.

2. **Modified `RoutingProviderBuilder`:** accept and store the leaves map.
   `build_from_config()` in `builder.rs` already has access to it.

3. **Modified `forward()`:**
   - Parse model from body
   - If model contains `/`, extract namespace and bare model
   - Lookup namespace in `leaves` map
   - If found: rewrite body (replace `"model"` value with bare model), forward
   - If not found: return 400 error
   - If no namespace: existing glob-based routing (unchanged)

4. **New helper function:** `split_namespace(model: &str) -> Option<(&str, &str)>`
   - Returns `Some((namespace, bare_model))` if model contains `/`
   - Returns `None` otherwise

### Body rewrite

When namespace is found, patch the JSON body's `"model"` field:

```rust
let mut value: serde_json::Value = serde_json::from_slice(&body)?;
value["model"] = serde_json::Value::String(bare_model.to_string());
let rewritten = serde_json::to_vec(&value)?;
```

### Error handling

| Scenario | Response |
|----------|----------|
| `nonexistent/glm-5` (unknown provider) | `400 Bad Request: unknown provider namespace 'nonexistent'` |
| `zai/` (empty model after namespace) | Forward to upstream as-is; upstream returns its own error |

### Config

No changes. Namespace maps to existing `providers[].name`.

## Files touched

- `crates/proxy/src/adapters/providers/routing.rs` — namespace parsing, routing logic, body rewrite
- `crates/proxy/src/adapters/providers/builder.rs` — pass leaves map to `RoutingProviderBuilder`

## Tests

- Namespace routes to correct provider, body has prefix stripped
- Unknown namespace returns 400
- Model without `/` uses existing routing rules (regression)
- Namespace takes priority over matching glob rule
- Model with multiple `/` splits on first only
