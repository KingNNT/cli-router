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
