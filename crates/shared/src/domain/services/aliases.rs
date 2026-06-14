use chrono::NaiveDate;

use crate::domain::entities::ModelPricing;
use crate::domain::value_objects::{ModelId, PricePerToken};

pub struct ModelAlias {
    pub canonical: &'static str,
    pub sources: &'static [&'static str],
    pub input_per_token: f64,
    pub output_per_token: f64,
    pub cache_read_per_token: Option<f64>,
    pub cache_write_per_token: Option<f64>,
}

/// Baked-in aliases. Edit this slice and rebuild to change them.
///
/// Matching is exact-string on each source. If a source appears in two
/// entries the first one wins; duplicates are silently ignored.
pub const ALIASES: &[ModelAlias] = &[
    // === Anthropic Claude ===
    ModelAlias {
        canonical: "opus4.6",
        sources: &[
            "anthropic.claude-opus-4-6-v1",
            "us.anthropic.claude-opus-4-6-v1",
        ],
        input_per_token: 5.0e-6,
        output_per_token: 25.0e-6,
        cache_read_per_token: Some(5.0e-7),
        cache_write_per_token: Some(6.25e-6),
    },
    ModelAlias {
        canonical: "sonnet4.6",
        sources: &["anthropic.claude-sonnet-4-6"],
        input_per_token: 3.0e-6,
        output_per_token: 15.0e-6,
        cache_read_per_token: Some(3.0e-7),
        cache_write_per_token: Some(3.75e-6),
    },
    // === OpenAI GPT-5 family ===
    ModelAlias {
        canonical: "gpt5-codex",
        sources: &["gpt-5-codex", "openai/gpt-5-codex"],
        input_per_token: 1.25e-6,
        output_per_token: 10.0e-6,
        cache_read_per_token: Some(1.25e-7),
        cache_write_per_token: None,
    },
    ModelAlias {
        canonical: "gpt5.1-codex",
        sources: &["gpt-5.1-codex", "openai/gpt-5.1-codex"],
        input_per_token: 1.25e-6,
        output_per_token: 10.0e-6,
        cache_read_per_token: Some(1.25e-7),
        cache_write_per_token: None,
    },
    ModelAlias {
        canonical: "gpt5",
        sources: &["gpt-5", "openai/gpt-5"],
        input_per_token: 1.25e-6,
        output_per_token: 10.0e-6,
        cache_read_per_token: Some(1.25e-7),
        cache_write_per_token: None,
    },
    ModelAlias {
        canonical: "gpt5.2-pro",
        sources: &["gpt-5.2-pro", "openai/gpt-5.2-pro"],
        input_per_token: 21.0e-6,
        output_per_token: 168.0e-6,
        cache_read_per_token: None,
        cache_write_per_token: None,
    },
    // === OpenAI GPT-4 family ===
    ModelAlias {
        canonical: "gpt4o",
        sources: &["gpt-4o", "openai/gpt-4o"],
        input_per_token: 2.5e-6,
        output_per_token: 10.0e-6,
        cache_read_per_token: Some(1.25e-6),
        cache_write_per_token: None,
    },
    ModelAlias {
        canonical: "gpt4.1-nano",
        sources: &["gpt-4.1-nano", "openai/gpt-4.1-nano"],
        input_per_token: 1.0e-7,
        output_per_token: 4.0e-7,
        cache_read_per_token: Some(2.5e-8),
        cache_write_per_token: None,
    },
    // === Google Gemini ===
    ModelAlias {
        canonical: "gemini3-pro",
        sources: &["gemini-3-pro-preview", "google/gemini-3-pro-preview"],
        input_per_token: 2.0e-6,
        output_per_token: 12.0e-6,
        cache_read_per_token: Some(2.0e-7),
        cache_write_per_token: None,
    },
    // === Z.AI GLM ===
    // Raw forms (e.g. "glm-4.6") are what the proxy stores when forwarding
    // through Z.ai's Anthropic-compatible endpoint — the body's `model`
    // field round-trips verbatim. Prefixed forms ("zai/...") cover the
    // analysis crate looking up by qualified id.
    ModelAlias {
        canonical: "glm4.6",
        sources: &["glm-4.6", "zai/glm-4.6", "z-ai/glm-4.6"],
        input_per_token: 6.0e-7,
        output_per_token: 2.2e-6,
        cache_read_per_token: Some(1.1e-7),
        cache_write_per_token: None,
    },
    ModelAlias {
        canonical: "glm4.7",
        sources: &["glm-4.7", "zai/glm-4.7", "z-ai/glm-4.7"],
        input_per_token: 6.0e-7,
        output_per_token: 2.2e-6,
        cache_read_per_token: Some(1.1e-7),
        cache_write_per_token: None,
    },
    ModelAlias {
        canonical: "glm4.5-air",
        sources: &["glm-4.5-air", "zai/glm-4.5-air", "z-ai/glm-4.5-air"],
        input_per_token: 2.0e-7,
        output_per_token: 1.1e-6,
        cache_read_per_token: Some(5.0e-8),
        cache_write_per_token: None,
    },
    // GLM-5 family — LiteLLM doesn't carry rates for these yet, so using
    // glm-4.6 rates as a placeholder estimate. Adjust if you know the real
    // Z.AI rates; turbo variants likely run cheaper than the 5.1 flagship.
    ModelAlias {
        canonical: "glm5",
        sources: &[
            "glm-5.1",
            "glm-5-turbo",
            "glm-5v-turbo",
            "zai/glm-5.1",
            "zai/glm-5-turbo",
            "zai/glm-5v-turbo",
        ],
        input_per_token: 6.0e-7,
        output_per_token: 2.2e-6,
        cache_read_per_token: Some(1.1e-7),
        cache_write_per_token: None,
    },
    // === Free / open-weights tiers (zero cost) ===
    ModelAlias {
        canonical: "minimax2.5",
        sources: &["minimax-m2.5-free"],
        input_per_token: 0.0,
        output_per_token: 0.0,
        cache_read_per_token: None,
        cache_write_per_token: None,
    },
    ModelAlias {
        canonical: "qwen3.6-plus",
        sources: &["qwen/qwen3.6-plus:free", "dashscope/qwen3.6-plus"],
        input_per_token: 0.0,
        output_per_token: 0.0,
        cache_read_per_token: None,
        cache_write_per_token: None,
    },
    ModelAlias {
        canonical: "gemma4-31b",
        sources: &["google/gemma-4-31b-it:free"],
        input_per_token: 0.0,
        output_per_token: 0.0,
        cache_read_per_token: None,
        cache_write_per_token: None,
    },
];

/// Return the canonical name for a raw source model id, or `None` if unaliased.
pub fn canonicalize(source: &str) -> Option<&'static str> {
    for entry in ALIASES {
        if entry.sources.contains(&source) {
            return Some(entry.canonical);
        }
    }
    None
}

/// Candidate lookup keys for pricing a raw model id.
///
/// Ordering matters: exact request keys come first, then exact canonical aliases,
/// then conservative family fallbacks. Keys are deduplicated while preserving the
/// first occurrence. All keys are lowercased on the way out so the SQL lookup
/// (which compares with `LOWER(lookup_key)`) and the result-map keys line up
/// regardless of whether the source id was upper, lower, or mixed case.
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

    let mut lower = Vec::with_capacity(keys.len());
    for k in keys {
        let lk = k.to_lowercase();
        push_unique(&mut lower, &lk);
    }
    lower
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

/// Build `ModelPricing` entries from the const list, ready for the
/// `PricingRepository` port. `today` is stamped into `last_synced`.
pub fn canonical_pricing(today: NaiveDate) -> Vec<ModelPricing> {
    ALIASES
        .iter()
        .map(|a| ModelPricing {
            lookup_key: a.canonical.to_string(),
            model: ModelId::new(a.canonical)
                .expect("alias canonical must be a valid non-empty model id"),
            provider_id: "alias".to_string(),
            input_rate: PricePerToken::new(a.input_per_token)
                .expect("alias input rate must be finite and non-negative"),
            output_rate: PricePerToken::new(a.output_per_token)
                .expect("alias output rate must be finite and non-negative"),
            cache_read_rate: a
                .cache_read_per_token
                .map(|v| PricePerToken::new(v).expect("alias cache_read rate must be valid")),
            cache_write_rate: a
                .cache_write_per_token
                .map(|v| PricePerToken::new(v).expect("alias cache_write rate must be valid")),
            last_synced: today,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalize_returns_some_for_known_source() {
        assert_eq!(
            canonicalize("anthropic.claude-opus-4-6-v1"),
            Some("opus4.6")
        );
        assert_eq!(
            canonicalize("us.anthropic.claude-opus-4-6-v1"),
            Some("opus4.6")
        );
    }

    #[test]
    fn canonicalize_returns_none_for_unknown_source() {
        assert_eq!(canonicalize("some-unknown-model-xyz"), None);
        assert_eq!(canonicalize(""), None);
    }

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

    #[test]
    fn pricing_lookup_keys_lowercases_candidates() {
        // Mixed-case upstream ids (e.g. LiteLLM's "minimax/MiniMax-M3") and
        // mixed-case client ids must produce lowercase candidates so the SQL
        // lookup (which compares with LOWER()) and the result-map keys line up.
        assert_eq!(
            pricing_lookup_keys("MiniMax-M3"),
            vec!["minimax-m3".to_string()]
        );
        assert_eq!(
            pricing_lookup_keys("minimax/MiniMax-M3"),
            vec!["minimax/minimax-m3".to_string(), "minimax-m3".to_string(),]
        );
    }

    #[test]
    fn canonical_pricing_produces_one_entry_per_alias_with_matching_keys() {
        let today = NaiveDate::from_ymd_opt(2026, 4, 23).unwrap();
        let rows = canonical_pricing(today);
        assert_eq!(rows.len(), ALIASES.len());
        for (entry, row) in ALIASES.iter().zip(rows.iter()) {
            assert_eq!(row.lookup_key, entry.canonical);
            assert_eq!(row.model.as_str(), entry.canonical);
            assert_eq!(row.provider_id, "alias");
            assert_eq!(row.last_synced, today);
            assert!((row.input_rate.value() - entry.input_per_token).abs() < 1e-12);
            assert!((row.output_rate.value() - entry.output_per_token).abs() < 1e-12);
        }
    }
}
