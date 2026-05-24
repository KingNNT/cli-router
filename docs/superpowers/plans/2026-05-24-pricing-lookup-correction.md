# Pricing Lookup Correction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Correct model pricing lookup so exact prices win, reviewed similar-name fallbacks work, and proxy request costs use one consistent formula.

**Architecture:** Add deterministic pricing-key resolution in the shared alias service, then use that resolver from the proxy cost path. Replace the proxy's duplicated cost arithmetic with the shared `calculate_cost` service so cache-token pricing behavior is consistent.

**Tech Stack:** Rust workspace, shared domain services, proxy application use case tests, Cargo test runner.

---

## Files

- Modify: `crates/shared/src/domain/services/aliases.rs` — add lookup-key resolution and conservative family fallback tests.
- Modify: `crates/proxy/src/application/use_cases/handle_messages.rs` — use shared resolver and shared cost calculation.
- Modify: `docs/superpowers/specs/2026-05-24-pricing-lookup-correction-design.md` only if implementation discovers a necessary design correction.

---

### Task 1: Add deterministic pricing lookup keys

**Files:**
- Modify: `crates/shared/src/domain/services/aliases.rs`

- [ ] **Step 1: Write failing tests for lookup-key order and fallback behavior**

Add these tests inside the existing `#[cfg(test)] mod tests` in `crates/shared/src/domain/services/aliases.rs`:

```rust
    #[test]
    fn pricing_lookup_keys_preserves_exact_keys_before_aliases() {
        assert_eq!(
            pricing_lookup_keys("openai/gpt-5.1-codex-latest"),
            vec![
                "openai/gpt-5.1-codex-latest".to_string(),
                "gpt-5.1-codex-latest".to_string(),
                "gpt5.1-codex".to_string(),
            ]
        );
    }

    #[test]
    fn pricing_lookup_keys_adds_exact_canonical_alias() {
        assert_eq!(
            pricing_lookup_keys("anthropic.claude-opus-4-6-v1"),
            vec![
                "anthropic.claude-opus-4-6-v1".to_string(),
                "opus4.6".to_string(),
            ]
        );
    }

    #[test]
    fn pricing_lookup_keys_deduplicates_without_reordering() {
        assert_eq!(
            pricing_lookup_keys("zai/glm-5.1"),
            vec![
                "zai/glm-5.1".to_string(),
                "glm-5.1".to_string(),
                "glm5".to_string(),
            ]
        );
    }

    #[test]
    fn pricing_lookup_keys_does_not_guess_unreviewed_models() {
        assert_eq!(
            pricing_lookup_keys("some-provider/unknown-model-latest"),
            vec![
                "some-provider/unknown-model-latest".to_string(),
                "unknown-model-latest".to_string(),
            ]
        );
    }
```

- [ ] **Step 2: Run the shared alias tests to verify failure**

Run:

```bash
cargo test -p shared domain::services::aliases::tests::pricing_lookup_keys -- --nocapture
```

Expected: FAIL because `pricing_lookup_keys` does not exist.

- [ ] **Step 3: Implement lookup-key resolver**

In `crates/shared/src/domain/services/aliases.rs`, add this code after `canonicalize`:

```rust
/// Candidate lookup keys for pricing a raw model id.
///
/// Ordering matters: exact request keys come first, then exact canonical aliases,
/// then conservative family fallbacks. Keys are deduplicated while preserving the
/// first occurrence.
pub fn pricing_lookup_keys(source: &str) -> Vec<String> {
    let mut keys = Vec::new();
    push_unique(&mut keys, source);

    if let Some(pos) = source.find('/') {
        let suffix = &source[pos + 1..];
        if !suffix.is_empty() {
            push_unique(&mut keys, suffix);
        }
    }

    for candidate in keys.clone() {
        if let Some(canonical) = canonicalize(candidate.as_str()) {
            push_unique(&mut keys, canonical);
        }
        if let Some(family) = family_fallback(candidate.as_str()) {
            push_unique(&mut keys, family);
        }
    }

    keys
}

fn push_unique(keys: &mut Vec<String>, key: &str) {
    if !key.is_empty() && !keys.iter().any(|existing| existing == key) {
        keys.push(key.to_string());
    }
}

fn family_fallback(source: &str) -> Option<&'static str> {
    if is_gpt_51_codex_variant(source) {
        return Some("gpt5.1-codex");
    }
    if is_gpt_5_codex_variant(source) {
        return Some("gpt5-codex");
    }
    if matches!(
        source,
        "glm-5.1"
            | "glm-5-turbo"
            | "glm-5v-turbo"
            | "zai/glm-5.1"
            | "zai/glm-5-turbo"
            | "zai/glm-5v-turbo"
            | "z-ai/glm-5.1"
            | "z-ai/glm-5-turbo"
            | "z-ai/glm-5v-turbo"
    ) {
        return Some("glm5");
    }
    None
}

fn is_gpt_51_codex_variant(source: &str) -> bool {
    source == "gpt-5.1-codex-latest"
        || source == "gpt-5.1-codex-preview"
        || has_date_suffix(source, "gpt-5.1-codex")
}

fn is_gpt_5_codex_variant(source: &str) -> bool {
    source == "gpt-5-codex-latest"
        || source == "gpt-5-codex-preview"
        || has_date_suffix(source, "gpt-5-codex")
}

fn has_date_suffix(source: &str, prefix: &str) -> bool {
    let Some(rest) = source.strip_prefix(prefix) else {
        return false;
    };
    let Some(date) = rest.strip_prefix('-') else {
        return false;
    };
    let bytes = date.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(idx, b)| idx == 4 || idx == 7 || b.is_ascii_digit())
}
```

- [ ] **Step 4: Run shared alias tests to verify pass**

Run:

```bash
cargo test -p shared domain::services::aliases::tests::pricing_lookup_keys -- --nocapture
```

Expected: PASS for the four new tests.

- [ ] **Step 5: Commit Task 1**

Only commit if the user explicitly requested commits. Otherwise skip this step and leave changes unstaged.

```bash
git add crates/shared/src/domain/services/aliases.rs
git commit -m "feat(pricing): add deterministic pricing lookup keys"
```

---

### Task 2: Use shared cost calculation in the proxy

**Files:**
- Modify: `crates/proxy/src/application/use_cases/handle_messages.rs`

- [ ] **Step 1: Write failing tests for proxy cost behavior**

Add these imports in the existing proxy test module in `crates/proxy/src/application/use_cases/handle_messages.rs`:

```rust
    use chrono::NaiveDate;
    use shared::domain::entities::ModelPricing;
    use shared::domain::value_objects::{ModelId, PricePerToken};
```

Add this helper near `fn fake_pricing()` in the same test module:

```rust
    fn pricing_row(key: &str, input: f64, output: f64) -> ModelPricing {
        ModelPricing {
            lookup_key: key.into(),
            model: ModelId::new(key).unwrap(),
            provider_id: "test".into(),
            input_rate: PricePerToken::new(input).unwrap(),
            output_rate: PricePerToken::new(output).unwrap(),
            cache_read_rate: None,
            cache_write_rate: None,
            last_synced: NaiveDate::from_ymd_opt(2026, 5, 24).unwrap(),
        }
    }
```

Add these tests near the existing unit tests:

```rust
    #[test]
    fn compute_cost_uses_input_rate_for_missing_cache_rates() {
        let repo = Arc::new(FakePricingRepository::default());
        repo.upsert_many(&[pricing_row("m", 0.00001, 0.00003)])
            .unwrap();
        let repo_dyn: Arc<dyn PricingRepository> = repo;

        let usage = UsageRecord {
            input_tokens: Some(100),
            output_tokens: Some(10),
            cache_read_tokens: Some(20),
            cache_creation_tokens: Some(30),
        };

        let cost = compute_cost(&repo_dyn, "m", &usage).unwrap();
        // input 100 * .00001 + output 10 * .00003 + cache 50 * .00001
        assert!((cost - 0.0018).abs() < 1e-12);
    }

    #[test]
    fn compute_cost_uses_family_fallback_after_exact_miss() {
        let repo = Arc::new(FakePricingRepository::default());
        repo.upsert_many(&[pricing_row("gpt5.1-codex", 0.00000125, 0.00001)])
            .unwrap();
        let repo_dyn: Arc<dyn PricingRepository> = repo;

        let usage = UsageRecord {
            input_tokens: Some(1000),
            output_tokens: Some(100),
            cache_read_tokens: None,
            cache_creation_tokens: None,
        };

        let cost = compute_cost(&repo_dyn, "openai/gpt-5.1-codex-latest", &usage).unwrap();
        assert!((cost - 0.00225).abs() < 1e-12);
    }

    #[test]
    fn compute_cost_returns_none_for_unknown_model() {
        let repo_dyn: Arc<dyn PricingRepository> = Arc::new(FakePricingRepository::default());
        let usage = UsageRecord {
            input_tokens: Some(1000),
            output_tokens: Some(100),
            cache_read_tokens: None,
            cache_creation_tokens: None,
        };

        assert_eq!(compute_cost(&repo_dyn, "unknown-model-latest", &usage), None);
    }
```

- [ ] **Step 2: Run proxy cost tests to verify failure**

Run:

```bash
cargo test -p proxy compute_cost_ -- --nocapture
```

Expected: FAIL. The cache-rate test should show the old proxy path treats missing cache rates as zero, and the fallback test should show the old lookup keys do not find `gpt5.1-codex`.

- [ ] **Step 3: Update proxy imports**

In `crates/proxy/src/application/use_cases/handle_messages.rs`, replace this import:

```rust
use shared::domain::value_objects::{ModelId, PricePerToken};
```

with:

```rust
use shared::domain::services::aliases::pricing_lookup_keys;
use shared::domain::services::pricing::calculate_cost;
use shared::domain::value_objects::{ModelId, TokenBreakdown, TokenCount};
```

- [ ] **Step 4: Replace `compute_cost` with resolver plus shared formula**

Replace the existing `compute_cost` function in `crates/proxy/src/application/use_cases/handle_messages.rs` with:

```rust
fn compute_cost(
    pricing: &Arc<dyn PricingRepository>,
    model: &str,
    usage: &UsageRecord,
) -> Option<f64> {
    if let Err(e) = ModelId::new(model.to_string()) {
        tracing::warn!(model = %model, error = %e, "invalid model id; cost will be NULL");
        return None;
    }

    let keys = pricing_lookup_keys(model);
    let map = match pricing.find_many(&keys) {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, "pricing lookup failed; cost will be NULL");
            return None;
        }
    };
    let Some((pricing_key, p)) = keys
        .iter()
        .find_map(|k| map.get(k).map(|pricing| (k, pricing)))
    else {
        tracing::warn!(model = %model, pricing_match = "none", "no pricing entry; cost will be NULL");
        return None;
    };

    let match_kind = if pricing_key == model {
        "exact"
    } else if pricing_key.as_str() == "gpt5.1-codex"
        || pricing_key.as_str() == "gpt5-codex"
        || pricing_key.as_str() == "glm5"
    {
        "family_alias"
    } else {
        "alias"
    };
    tracing::debug!(
        model = %model,
        pricing_key = %pricing_key,
        pricing_match = match_kind,
        "pricing entry selected"
    );

    let tokens = TokenBreakdown {
        input: TokenCount::new(usage.input_tokens.unwrap_or(0) as u64),
        output: TokenCount::new(usage.output_tokens.unwrap_or(0) as u64),
        reasoning: TokenCount::new(0),
        cache_read: TokenCount::new(usage.cache_read_tokens.unwrap_or(0) as u64),
        cache_write: TokenCount::new(usage.cache_creation_tokens.unwrap_or(0) as u64),
    };
    Some(calculate_cost(&tokens, p).value())
}
```

- [ ] **Step 5: Run proxy cost tests to verify pass**

Run:

```bash
cargo test -p proxy compute_cost_ -- --nocapture
```

Expected: PASS for the three new tests.

- [ ] **Step 6: Commit Task 2**

Only commit if the user explicitly requested commits. Otherwise skip this step and leave changes unstaged.

```bash
git add crates/proxy/src/application/use_cases/handle_messages.rs
git commit -m "fix(proxy): use shared pricing calculation"
```

---

### Task 3: Verify the full relevant test surface

**Files:**
- No code changes expected.

- [ ] **Step 1: Run shared pricing service tests**

Run:

```bash
cargo test -p shared domain::services::pricing -- --nocapture
```

Expected: PASS. Existing shared cost formula tests still pass.

- [ ] **Step 2: Run shared alias service tests**

Run:

```bash
cargo test -p shared domain::services::aliases -- --nocapture
```

Expected: PASS. Existing alias tests and new lookup-key tests pass.

- [ ] **Step 3: Run proxy handle message tests**

Run:

```bash
cargo test -p proxy application::use_cases::handle_messages::tests -- --nocapture
```

Expected: PASS. Existing request logging tests and new cost tests pass.

- [ ] **Step 4: Run formatting**

Run:

```bash
cargo fmt --all -- --check
```

Expected: PASS. If it fails, run `cargo fmt --all`, inspect the formatting-only diff, then rerun `cargo fmt --all -- --check`.

- [ ] **Step 5: Run workspace tests if time allows**

Run:

```bash
cargo test --workspace
```

Expected: PASS. If unrelated tests fail, record the exact failing command and failure output before deciding whether to fix or defer.

- [ ] **Step 6: Final commit**

Only commit if the user explicitly requested commits. Otherwise skip this step and leave changes unstaged.

```bash
git add crates/shared/src/domain/services/aliases.rs crates/proxy/src/application/use_cases/handle_messages.rs
git commit -m "fix(pricing): handle reviewed model name variants"
```

---

## Plan Self-Review

- Spec coverage: The plan centralizes cost calculation, adds deterministic lookup keys, keeps unknown models as `None`, and tests exact/fallback behavior.
- Placeholder scan: No TBD/TODO placeholders are present.
- Type consistency: The plan uses existing `ModelPricing`, `PricingRepository`, `UsageRecord`, `TokenBreakdown`, `TokenCount`, and `calculate_cost` names from the codebase.
