use std::collections::HashSet;
use std::sync::Arc;

use crate::application::dto::{Filter, GetModelsBreakdownInput, GetModelsBreakdownOutput};
use crate::application::ports::UsageRepository;
use shared::application::errors::ApplicationError;
use shared::application::ports::{Clock, PricingRepository};
use shared::domain::services::aggregation::aggregate_model_usage_by_alias;
use shared::domain::services::pricing as pricing_service;
use shared::domain::value_objects::{DateRange, ModelId};

pub struct GetModelsBreakdown {
    usage_repo: Arc<dyn UsageRepository>,
    pricing_repo: Arc<dyn PricingRepository>,
    clock: Arc<dyn Clock>,
}

impl GetModelsBreakdown {
    pub fn new(
        usage_repo: Arc<dyn UsageRepository>,
        pricing_repo: Arc<dyn PricingRepository>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            usage_repo,
            pricing_repo,
            clock,
        }
    }

    pub fn execute(
        &self,
        input: GetModelsBreakdownInput,
    ) -> Result<GetModelsBreakdownOutput, ApplicationError> {
        let filter = input.filter.unwrap_or_else(|| Filter {
            date_range: Some(DateRange::last_n_days(self.clock.today(), 30)),
            ..Filter::default()
        });

        let raw = self.usage_repo.by_model(&filter)?;
        // Collapse by alias before pricing lookup.
        let mut models = aggregate_model_usage_by_alias(raw);

        let mut candidates: HashSet<String> = HashSet::new();
        for m in &models {
            for k in m.model.lookup_keys() {
                candidates.insert(k);
            }
        }
        let candidate_vec: Vec<String> = candidates.into_iter().collect();
        let pricing_map = self.pricing_repo.find_many(&candidate_vec)?;

        let mut missing = 0usize;
        let mut unpriced_models: HashSet<ModelId> = HashSet::new();
        for m in models.iter_mut() {
            let mut matched = false;
            for k in m.model.lookup_keys() {
                if let Some(p) = pricing_map.get(&k) {
                    m.cost = pricing_service::calculate_cost(&m.tokens, p);
                    matched = true;
                    break;
                }
            }
            if !matched {
                missing += 1;
                unpriced_models.insert(m.model.clone());
            }
        }

        // Sort reconciled rows by cost descending.
        models.sort_by(|a, b| {
            b.cost
                .partial_cmp(&a.cost)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let last_pricing_sync = self.pricing_repo.last_sync()?;

        Ok(GetModelsBreakdownOutput {
            filter_applied: filter,
            models,
            missing_pricing_count: missing,
            unpriced_models,
            last_pricing_sync,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::test_support::FakeUsageRepository;
    use chrono::NaiveDate;
    use shared::application::test_support::{FakePricingRepository, FixedClock};
    use shared::domain::entities::{ModelPricing, ModelUsage};
    use shared::domain::value_objects::{Cost, ModelId, PricePerToken, TokenBreakdown, TokenCount};

    fn pricing_for(key: &str, input_rate: f64) -> ModelPricing {
        ModelPricing {
            lookup_key: key.into(),
            model: ModelId::new(key).unwrap(),
            provider_id: "p".into(),
            input_rate: PricePerToken::new(input_rate).unwrap(),
            output_rate: PricePerToken::new(0.0).unwrap(),
            cache_read_rate: None,
            cache_write_rate: None,
            last_synced: NaiveDate::from_ymd_opt(2026, 4, 23).unwrap(),
            alias: None,
        }
    }

    #[test]
    fn pricing_hit_replaces_model_cost_and_tracks_missing() {
        let usage = Arc::new(FakeUsageRepository {
            by_model: vec![
                ModelUsage {
                    model: ModelId::new("anthropic/opus").unwrap(),
                    message_count: 1,
                    tokens: TokenBreakdown {
                        input: TokenCount::new(2000),
                        ..Default::default()
                    },
                    cost: Cost::new(99.0).unwrap(),
                },
                ModelUsage {
                    model: ModelId::new("unknown/one").unwrap(),
                    message_count: 1,
                    tokens: TokenBreakdown::default(),
                    cost: Cost::new(5.0).unwrap(),
                },
            ],
            ..Default::default()
        });
        let pricing = Arc::new(FakePricingRepository::default());
        pricing
            .upsert_many(&[pricing_for("anthropic/opus", 0.00001)])
            .unwrap();
        let clock = Arc::new(FixedClock::new(2026, 4, 23));
        let usage_dyn: Arc<dyn UsageRepository> = usage;
        let pricing_dyn: Arc<dyn PricingRepository> = pricing;
        let uc = GetModelsBreakdown::new(usage_dyn, pricing_dyn, clock);

        let out = uc.execute(GetModelsBreakdownInput::default()).unwrap();
        assert_eq!(out.models.len(), 2);
        assert_eq!(out.missing_pricing_count, 1);
        assert!(
            out.unpriced_models
                .contains(&ModelId::new("unknown/one").unwrap())
        );
        assert!(
            !out.unpriced_models
                .contains(&ModelId::new("anthropic/opus").unwrap())
        );
        // opus reconciled: 2000 * 0.00001 = 0.02
        let opus = out
            .models
            .iter()
            .find(|m| m.model.as_str() == "anthropic/opus")
            .unwrap();
        assert!((opus.cost.value() - 0.02).abs() < 1e-9);
        // unknown kept OpenCode's cost
        let unknown = out
            .models
            .iter()
            .find(|m| m.model.as_str() == "unknown/one")
            .unwrap();
        assert!((unknown.cost.value() - 5.0).abs() < 1e-9);
    }

    #[test]
    fn aggregates_aliased_models_and_sorts_by_cost_desc() {
        let usage = Arc::new(FakeUsageRepository {
            by_model: vec![
                ModelUsage {
                    model: ModelId::new("anthropic.claude-opus-4-6-v1").unwrap(),
                    message_count: 10,
                    tokens: TokenBreakdown {
                        input: TokenCount::new(1000),
                        ..Default::default()
                    },
                    cost: Cost::new(99.0).unwrap(),
                },
                ModelUsage {
                    model: ModelId::new("us.anthropic.claude-opus-4-6-v1").unwrap(),
                    message_count: 7,
                    tokens: TokenBreakdown {
                        input: TokenCount::new(2000),
                        ..Default::default()
                    },
                    cost: Cost::new(99.0).unwrap(),
                },
                ModelUsage {
                    model: ModelId::new("unknown/one").unwrap(),
                    message_count: 1,
                    tokens: TokenBreakdown::default(),
                    cost: Cost::new(0.5).unwrap(),
                },
            ],
            ..Default::default()
        });
        let pricing = Arc::new(FakePricingRepository::default());
        pricing
            .upsert_many(&[ModelPricing {
                lookup_key: "opus4.6".into(),
                model: ModelId::new("opus4.6").unwrap(),
                provider_id: "alias".into(),
                input_rate: PricePerToken::new(5e-6).unwrap(),
                output_rate: PricePerToken::new(0.0).unwrap(),
                cache_read_rate: None,
                cache_write_rate: None,
                last_synced: NaiveDate::from_ymd_opt(2026, 4, 23).unwrap(),
                alias: None,
            }])
            .unwrap();

        let clock = Arc::new(FixedClock::new(2026, 4, 23));
        let usage_dyn: Arc<dyn UsageRepository> = usage;
        let pricing_dyn: Arc<dyn PricingRepository> = pricing;
        let uc = GetModelsBreakdown::new(usage_dyn, pricing_dyn, clock);

        let out = uc.execute(GetModelsBreakdownInput::default()).unwrap();
        assert_eq!(
            out.models.len(),
            2,
            "two source variants collapsed into one opus4.6 row"
        );
        // opus4.6 total input = 3000 → 3000 * 5e-6 = 0.015; unknown kept OpenCode's 0.5.
        // Sort desc: unknown (0.5), opus4.6 (0.015).
        assert_eq!(out.models[0].model.as_str(), "unknown/one");
        assert_eq!(out.models[1].model.as_str(), "opus4.6");
        assert_eq!(out.models[1].message_count, 17);
        assert_eq!(out.missing_pricing_count, 1); // unknown/one
    }
}
