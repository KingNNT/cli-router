use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;

use crate::application::ports::PricingSource;
use shared::adapters::AdapterError;
use shared::application::errors::ApplicationError;
use shared::application::ports::Clock;
use shared::domain::entities::ModelPricing;
use shared::domain::services::aliases::canonicalize;
use shared::domain::value_objects::{ModelId, PricePerToken};

pub const LITELLM_JSON_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";

#[derive(Debug, Deserialize)]
struct RawEntry {
    #[serde(default)]
    input_cost_per_token: Option<f64>,
    #[serde(default)]
    output_cost_per_token: Option<f64>,
    #[serde(default)]
    cache_read_input_token_cost: Option<f64>,
    #[serde(default)]
    cache_creation_input_token_cost: Option<f64>,
    #[serde(default)]
    litellm_provider: Option<String>,
}

pub struct LiteLlmPricingSource {
    url: String,
    clock: Arc<dyn Clock>,
    agent: ureq::Agent,
}

impl LiteLlmPricingSource {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self::with_url(clock, LITELLM_JSON_URL.to_string())
    }

    pub fn with_url(clock: Arc<dyn Clock>, url: String) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(10))
            .timeout_read(Duration::from_secs(30))
            .build();
        Self { url, clock, agent }
    }

    pub(crate) fn parse(
        body: &str,
        today: chrono::NaiveDate,
    ) -> Result<Vec<ModelPricing>, AdapterError> {
        let raw: std::collections::HashMap<String, RawEntry> =
            serde_json::from_str(body).map_err(|e| AdapterError::DataMapping(e.to_string()))?;
        let mut out = Vec::new();
        for (key, entry) in raw {
            if key == "sample_spec" {
                continue;
            }
            let (Some(input), Some(output)) =
                (entry.input_cost_per_token, entry.output_cost_per_token)
            else {
                continue;
            };
            let provider = match entry.litellm_provider {
                Some(p) if !p.is_empty() => p,
                _ => continue,
            };
            let input_rate = match PricePerToken::new(input) {
                Ok(p) => p,
                Err(_) => continue,
            };
            let output_rate = match PricePerToken::new(output) {
                Ok(p) => p,
                Err(_) => continue,
            };
            let cache_read_rate = entry
                .cache_read_input_token_cost
                .and_then(|v| PricePerToken::new(v).ok());
            let cache_write_rate = entry
                .cache_creation_input_token_cost
                .and_then(|v| PricePerToken::new(v).ok());
            let model = match ModelId::new(key.clone()) {
                Ok(m) => m,
                Err(_) => continue,
            };

            let raw_alias = canonicalize(&key).map(String::from);
            // Raw key row.
            out.push(ModelPricing {
                lookup_key: key.clone(),
                model: model.clone(),
                provider_id: provider.clone(),
                input_rate,
                output_rate,
                cache_read_rate,
                cache_write_rate,
                last_synced: today,
                alias: raw_alias,
            });
            // Provider-composed key row, unless raw already has '/'.
            if !key.contains('/') {
                let composed_key = format!("{}/{}", provider, key);
                let composed_alias = canonicalize(&composed_key).map(String::from);
                out.push(ModelPricing {
                    lookup_key: composed_key,
                    model,
                    provider_id: provider,
                    input_rate,
                    output_rate,
                    cache_read_rate,
                    cache_write_rate,
                    last_synced: today,
                    alias: composed_alias,
                });
            }
        }
        Ok(out)
    }
}

impl PricingSource for LiteLlmPricingSource {
    fn fetch_all(&self) -> Result<Vec<ModelPricing>, ApplicationError> {
        let resp = self
            .agent
            .get(&self.url)
            .call()
            .map_err(|e| ApplicationError::from(AdapterError::Http(Box::new(e))))?;
        let mut body = String::new();
        resp.into_reader()
            .read_to_string(&mut body)
            .map_err(|e| ApplicationError::from(AdapterError::DataMapping(e.to_string())))?;
        Self::parse(&body, self.clock.today()).map_err(ApplicationError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 4, 23).unwrap()
    }

    fn sample_body() -> String {
        std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/litellm_sample.json"
        ))
        .unwrap()
    }

    #[test]
    fn parse_yields_two_rows_per_valid_bare_key() {
        let rows = LiteLlmPricingSource::parse(&sample_body(), today()).unwrap();
        // claude-opus-4 → 2 rows, gpt-4o → 2 rows, openrouter/... → 1 row
        // (sample_spec skipped, missing-costs-model skipped)
        assert_eq!(rows.len(), 5);
    }

    #[test]
    fn parse_skips_sample_spec() {
        let rows = LiteLlmPricingSource::parse(&sample_body(), today()).unwrap();
        assert!(rows.iter().all(|r| r.model.as_str() != "sample_spec"));
    }

    #[test]
    fn parse_skips_entries_missing_costs() {
        let rows = LiteLlmPricingSource::parse(&sample_body(), today()).unwrap();
        assert!(rows
            .iter()
            .all(|r| r.model.as_str() != "missing-costs-model"));
    }

    #[test]
    fn parse_emits_raw_and_composed_for_bare_keys() {
        let rows = LiteLlmPricingSource::parse(&sample_body(), today()).unwrap();
        let keys: Vec<&str> = rows.iter().map(|r| r.lookup_key.as_str()).collect();
        assert!(keys.contains(&"claude-opus-4"));
        assert!(keys.contains(&"anthropic/claude-opus-4"));
    }

    #[test]
    fn parse_emits_only_raw_for_keys_with_slash() {
        let rows = LiteLlmPricingSource::parse(&sample_body(), today()).unwrap();
        let count = rows
            .iter()
            .filter(|r| r.model.as_str() == "openrouter/anthropic/claude-3-opus")
            .count();
        assert_eq!(count, 1);
    }

    #[test]
    fn parse_sets_last_synced_to_today() {
        let rows = LiteLlmPricingSource::parse(&sample_body(), today()).unwrap();
        assert!(rows.iter().all(|r| r.last_synced == today()));
    }

    #[test]
    fn parse_cache_rates_when_present() {
        let rows = LiteLlmPricingSource::parse(&sample_body(), today()).unwrap();
        let opus = rows
            .iter()
            .find(|r| r.lookup_key == "claude-opus-4")
            .unwrap();
        assert!(opus.cache_read_rate.is_some());
        assert!(opus.cache_write_rate.is_some());
    }

    #[test]
    fn parse_malformed_json_errors() {
        let err = LiteLlmPricingSource::parse("not json", today()).unwrap_err();
        assert!(matches!(err, AdapterError::DataMapping(_)));
    }

    #[test]
    fn parse_populates_alias_for_sources_matching_const() {
        // "anthropic.claude-opus-4-6-v1" is a literal source entry in ALIASES.
        let body = r#"{
            "anthropic.claude-opus-4-6-v1": {
                "input_cost_per_token": 5e-6,
                "output_cost_per_token": 2.5e-5,
                "litellm_provider": "anthropic",
                "mode": "chat"
            }
        }"#;
        let rows = LiteLlmPricingSource::parse(body, today()).unwrap();
        // Raw key is in ALIASES → alias = Some("opus4.6")
        let raw = rows
            .iter()
            .find(|r| r.lookup_key == "anthropic.claude-opus-4-6-v1")
            .unwrap();
        assert_eq!(raw.alias.as_deref(), Some("opus4.6"));
        // Composed key "anthropic/anthropic.claude-opus-4-6-v1" is NOT in ALIASES → alias = None
        let composed = rows
            .iter()
            .find(|r| r.lookup_key == "anthropic/anthropic.claude-opus-4-6-v1")
            .unwrap();
        assert_eq!(composed.alias, None);
    }

    #[test]
    fn parse_alias_is_none_for_unrecognised_keys() {
        let rows = LiteLlmPricingSource::parse(&sample_body(), today()).unwrap();
        // Rows whose lookup_key is NOT in ALIASES must carry alias = None.
        // (`gpt-4o` and similar may be in ALIASES; check only known-unaliased fixture keys.)
        let unaliased = ["claude-opus-4", "openrouter/anthropic/claude-3-opus"];
        for key in unaliased {
            let row = rows
                .iter()
                .find(|r| r.lookup_key == key)
                .unwrap_or_else(|| panic!("missing row for {}", key));
            assert_eq!(row.alias, None, "row {} should be unaliased", key);
        }
    }
}
