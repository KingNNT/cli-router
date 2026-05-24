# Pricing Lookup Correction Design

## Summary

Pricing should be accurate for exact model names, resilient to known model-name variants, and safe for unknown names. The current implementation is mostly exact-string based and has inconsistent cache-token fallback behavior between the shared pricing service and the proxy request-cost path.

This design uses a hybrid approach: exact pricing lookup first, explicit canonical/family fallback second, and `NULL` cost for unknown models. It avoids open-ended fuzzy matching because guessing prices can silently overcharge or undercharge usage.

## Current Problems

1. `shared::domain::services::pricing::calculate_cost` treats missing cache rates as the input-token rate.
2. `proxy::application::use_cases::handle_messages::compute_cost` treats missing cache rates as zero.
3. `ModelId::lookup_keys()` only tries the original id and the suffix after the first `/`.
4. `aliases::canonicalize()` only exact-matches source names, so date, `latest`, preview, or provider-prefixed variants miss unless listed exactly.
5. `CompositePricingRepository` checks built-in constant aliases before SQLite/LiteLLM. This is acceptable for exact aliases but would be risky for broad family fallbacks, especially where aliases document placeholder pricing.

## Goals

- Use one cost formula everywhere.
- Preserve exact pricing priority.
- Support new/similar names only through explicit, deterministic rules.
- Make fallback use visible in logs.
- Keep unknown model prices as `NULL` rather than guessed.

## Non-Goals

- No fuzzy string-distance matching.
- No automatic web lookup during request handling.
- No pricing UI changes.
- No large repository or schema redesign.

## Recommended Behavior

For a request model id, generate lookup candidates in priority order:

1. Original model id.
2. Existing suffix key after the first provider slash, when present.
3. Exact canonical alias for any exact known source.
4. Explicit family fallback keys for reviewed variants.

Examples:

```text
openai/gpt-5.1-codex-latest
→ openai/gpt-5.1-codex-latest
→ gpt-5.1-codex-latest
→ gpt5.1-codex

anthropic.claude-opus-4-6-v1
→ anthropic.claude-opus-4-6-v1
→ opus4.6

zai/glm-5.1
→ zai/glm-5.1
→ glm-5.1
→ glm5
```

If none of these candidates has a price, the proxy records `cost_usd = NULL` and logs a warning.

## Architecture

### Pricing key resolution

Add a small shared-domain resolver near alias logic. It should expose a function such as:

```rust
pub fn pricing_lookup_keys(source: &str) -> Vec<String>
```

The function should be deterministic and deduplicate keys while preserving priority order. It should combine the current `ModelId::lookup_keys()` behavior with exact alias canonicalization and explicit family fallback rules.

### Family fallback rules

Family fallback rules must be conservative and manually reviewed. Initial rules should cover families already represented in `ALIASES`:

- GPT-5 Codex variants ending with `-latest`, `-preview`, or a date-like suffix map to `gpt5-codex` or `gpt5.1-codex` only when the family prefix is clear.
- Anthropic source names already listed exactly continue to map through `canonicalize`; do not infer Sonnet/Opus pricing beyond explicitly defined version families.
- GLM 5 variants already listed map to `glm5`; do not infer unrelated GLM models.

Fallbacks should only map from a reviewed pattern to an existing alias canonical key.

### Cost calculation

Change proxy `compute_cost` to build a `TokenBreakdown` and call `shared::domain::services::pricing::calculate_cost` instead of duplicating the formula.

The shared formula remains:

- input tokens use `input_rate`
- output tokens use `output_rate`
- cache-read tokens use `cache_read_rate` or `input_rate`
- cache-write tokens use `cache_write_rate` or `input_rate`

This ensures cache fallback is consistent and conservative.

### Repository priority

Keep the repository implementation simple. Do not add broad fallback behavior inside `CompositePricingRepository`. The lookup resolver controls candidate order and `compute_cost` tries candidates in order. Exact DB/LiteLLM entries still win when they appear earlier in the candidate list than family fallback aliases.

Because the existing composite repository prioritizes const aliases for a matching key, fallback aliases must be separate candidate keys after exact original/suffix keys. This keeps exact official keys preferred when present.

### Logging

When a price is found, log debug-level metadata:

```text
model="..." pricing_key="..." pricing_match="exact|alias|family_alias"
```

When no price is found, keep warning-level logging:

```text
model="..." pricing_match="none" cost=NULL
```

The match kind can be derived by the resolver or by comparing the winning key with the generated candidates.

## Testing

Add or update tests for:

- shared cost formula with missing cache rates.
- proxy `compute_cost` uses the shared cache-rate fallback.
- exact pricing match works.
- provider-prefixed exact match works.
- exact alias match works.
- family fallback match works for a reviewed new/similar name.
- unknown model returns `None` cost.
- lookup candidate ordering preserves exact keys before fallback keys.

## Acceptance Criteria

- There is one effective cost formula for proxy request accounting.
- New/similar names are priced only when they match explicit resolver rules.
- Unknown names still produce `NULL` cost.
- Tests document lookup order and cache fallback behavior.
- No fuzzy pricing guesses are introduced.
