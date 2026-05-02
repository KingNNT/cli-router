use std::collections::HashSet;
use std::sync::Arc;

use crate::application::dto::{Filter, GetDashboardInput, GetDashboardOutput};
use shared::application::errors::ApplicationError;
use crate::application::ports::UsageRepository;
use shared::application::ports::{Clock, PricingRepository};
use shared::domain::entities::Overview;
use shared::domain::services::aggregation::aggregate_day_model_rows_by_alias;
use shared::domain::services::pricing as pricing_service;
use shared::domain::value_objects::{Cost, DateRange, ModelId, TokenBreakdown};

pub struct GetDashboard {
    usage_repo: Arc<dyn UsageRepository>,
    pricing_repo: Arc<dyn PricingRepository>,
    clock: Arc<dyn Clock>,
}

impl GetDashboard {
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
        input: GetDashboardInput,
    ) -> Result<GetDashboardOutput, ApplicationError> {
        let filter = input.filter.unwrap_or_else(|| Filter {
            date_range: Some(DateRange::last_n_days(self.clock.today(), 30)),
            ..Filter::default()
        });

        let raw = self.usage_repo.daily_by_model(&filter)?;
        // Collapse by alias before pricing lookup.
        let mut rows = aggregate_day_model_rows_by_alias(raw);
        let overview_seed = self.usage_repo.overview(&filter)?;

        // Collect lookup candidates across all rows.
        let mut candidates: HashSet<String> = HashSet::new();
        for row in &rows {
            for key in row.model.lookup_keys() {
                candidates.insert(key);
            }
        }
        let candidate_vec: Vec<String> = candidates.into_iter().collect();
        let pricing_map = self.pricing_repo.find_many(&candidate_vec)?;

        // Reconcile each row's cost.
        let mut missing = 0usize;
        let mut unpriced_models: HashSet<ModelId> = HashSet::new();
        for row in rows.iter_mut() {
            let mut matched = false;
            for key in row.model.lookup_keys() {
                if let Some(p) = pricing_map.get(&key) {
                    row.cost = pricing_service::calculate_cost(&row.tokens, p);
                    matched = true;
                    break;
                }
            }
            if !matched {
                missing += 1;
                unpriced_models.insert(row.model.clone());
            }
        }

        // Aggregation disturbs SQL's ORDER BY; re-sort here.
        rows.sort_by(|a, b| {
            b.date.cmp(&a.date).then_with(|| {
                b.cost
                    .partial_cmp(&a.cost)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
        });

        // Derive overview totals from reconciled rows.
        let tokens_total: TokenBreakdown = rows.iter().map(|r| r.tokens).sum();
        let cost_total: Cost = rows.iter().map(|r| r.cost).sum();
        let overview = Overview {
            range: overview_seed.range,
            session_count: overview_seed.session_count,
            message_count: overview_seed.message_count,
            tokens: tokens_total,
            cost: cost_total,
        };

        let last_pricing_sync = self.pricing_repo.last_sync()?;

        Ok(GetDashboardOutput {
            filter_applied: filter,
            overview,
            rows,
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
    use shared::application::test_support::{FakePricingRepository, FixedClock};
    use chrono::NaiveDate;
    use shared::domain::entities::{DayModelRow, ModelPricing, Overview};
    use shared::domain::value_objects::{ModelId, PricePerToken, TokenCount};

    fn row(model: &str, input: u64, cost: f64) -> DayModelRow {
        DayModelRow {
            date: NaiveDate::from_ymd_opt(2026, 4, 23).unwrap(),
            model: ModelId::new(model).unwrap(),
            tokens: TokenBreakdown {
                input: TokenCount::new(input),
                ..Default::default()
            },
            cost: Cost::new(cost).unwrap(),
        }
    }

    fn setup(
        usage_rows: Vec<DayModelRow>,
        pricing_rows: Vec<ModelPricing>,
    ) -> (GetDashboard, Arc<FakePricingRepository>) {
        let usage = Arc::new(FakeUsageRepository {
            daily: usage_rows,
            overview: Overview {
                session_count: 1,
                message_count: 1,
                ..Overview::default()
            },
            ..Default::default()
        });
        let pricing = Arc::new(FakePricingRepository::default());
        pricing.upsert_many(&pricing_rows).unwrap();
        let clock = Arc::new(FixedClock::new(2026, 4, 23));
        let usage_dyn: Arc<dyn UsageRepository> = usage;
        let pricing_dyn: Arc<dyn PricingRepository> = pricing.clone();
        let uc = GetDashboard::new(usage_dyn, pricing_dyn, clock);
        (uc, pricing)
    }

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
    fn pricing_hit_replaces_cost() {
        let (uc, _) = setup(
            vec![row("anthropic/opus", 1000, 999.99)],
            vec![pricing_for("anthropic/opus", 0.00001)],
        );
        let out = uc.execute(GetDashboardInput::default()).unwrap();
        assert_eq!(out.missing_pricing_count, 0);
        assert!(out.unpriced_models.is_empty());
        // 1000 * 0.00001 = 0.01
        assert!((out.rows[0].cost.value() - 0.01).abs() < 1e-9);
    }

    #[test]
    fn missing_pricing_preserves_opencode_cost() {
        let (uc, _) = setup(vec![row("unknown/model", 1000, 7.77)], vec![]);
        let out = uc.execute(GetDashboardInput::default()).unwrap();
        assert_eq!(out.missing_pricing_count, 1);
        assert!(out
            .unpriced_models
            .contains(&ModelId::new("unknown/model").unwrap()));
        assert!((out.rows[0].cost.value() - 7.77).abs() < 1e-9);
    }

    #[test]
    fn suffix_lookup_matches_bare_key() {
        // OpenCode id "anthropic/claude-opus-4-7"; pricing stored only as bare "claude-opus-4-7".
        let (uc, _) = setup(
            vec![row("anthropic/claude-opus-4-7", 1000, 999.99)],
            vec![pricing_for("claude-opus-4-7", 0.00002)],
        );
        let out = uc.execute(GetDashboardInput::default()).unwrap();
        assert_eq!(out.missing_pricing_count, 0);
        assert!((out.rows[0].cost.value() - 0.02).abs() < 1e-9);
    }

    #[test]
    fn overview_totals_equal_row_sum_after_reconciliation() {
        let (uc, _) = setup(
            vec![
                row("anthropic/opus", 1000, 100.0),
                row("anthropic/sonnet", 500, 100.0),
            ],
            vec![
                pricing_for("anthropic/opus", 0.00001),
                pricing_for("anthropic/sonnet", 0.00001),
            ],
        );
        let out = uc.execute(GetDashboardInput::default()).unwrap();
        let expected_cost = 1000.0 * 0.00001 + 500.0 * 0.00001;
        assert!((out.overview.cost.value() - expected_cost).abs() < 1e-9);
        assert_eq!(out.overview.tokens.input.value(), 1500);
    }

    #[test]
    fn last_pricing_sync_propagates_from_repo() {
        let (uc, _) = setup(vec![row("m", 1, 1.0)], vec![pricing_for("m", 0.0)]);
        let out = uc.execute(GetDashboardInput::default()).unwrap();
        assert_eq!(
            out.last_pricing_sync,
            Some(NaiveDate::from_ymd_opt(2026, 4, 23).unwrap())
        );
    }

    #[test]
    fn aggregates_aliased_rows_before_pricing_reconciliation() {
        let d = NaiveDate::from_ymd_opt(2026, 4, 23).unwrap();
        let usage = Arc::new(FakeUsageRepository {
            daily: vec![
                DayModelRow {
                    date: d,
                    model: ModelId::new("anthropic.claude-opus-4-6-v1").unwrap(),
                    tokens: TokenBreakdown {
                        input: TokenCount::new(1000),
                        ..Default::default()
                    },
                    cost: Cost::new(5.0).unwrap(),
                },
                DayModelRow {
                    date: d,
                    model: ModelId::new("us.anthropic.claude-opus-4-6-v1").unwrap(),
                    tokens: TokenBreakdown {
                        input: TokenCount::new(2000),
                        ..Default::default()
                    },
                    cost: Cost::new(10.0).unwrap(),
                },
            ],
            overview: Overview {
                session_count: 1,
                message_count: 2,
                ..Overview::default()
            },
            ..Default::default()
        });
        let pricing = Arc::new(FakePricingRepository::default());
        // Const-style alias pricing: opus4.6 at $0.000005 per input token.
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

        let clock: Arc<dyn Clock> = Arc::new(FixedClock::new(2026, 4, 23));
        let usage_dyn: Arc<dyn UsageRepository> = usage;
        let pricing_dyn: Arc<dyn PricingRepository> = pricing;
        let uc = GetDashboard::new(usage_dyn, pricing_dyn, clock);

        let out = uc.execute(GetDashboardInput::default()).unwrap();
        // Two source variants collapse into one row with canonical name.
        assert_eq!(out.rows.len(), 1);
        assert_eq!(out.rows[0].model.as_str(), "opus4.6");
        assert_eq!(out.rows[0].tokens.input.value(), 3000);
        // Pricing came from the aliased entry: 3000 * 5e-6 = 0.015
        assert!((out.rows[0].cost.value() - 0.015).abs() < 1e-9);
        assert_eq!(out.missing_pricing_count, 0);
    }

    #[test]
    fn rows_are_sorted_by_date_desc_and_cost_desc_after_aggregation() {
        let d1 = NaiveDate::from_ymd_opt(2026, 4, 22).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2026, 4, 23).unwrap();
        let usage = Arc::new(FakeUsageRepository {
            daily: vec![
                DayModelRow {
                    date: d1,
                    model: ModelId::new("a-model").unwrap(),
                    tokens: TokenBreakdown::default(),
                    cost: Cost::new(1.0).unwrap(),
                },
                DayModelRow {
                    date: d2,
                    model: ModelId::new("b-model").unwrap(),
                    tokens: TokenBreakdown::default(),
                    cost: Cost::new(2.0).unwrap(),
                },
                DayModelRow {
                    date: d2,
                    model: ModelId::new("c-model").unwrap(),
                    tokens: TokenBreakdown::default(),
                    cost: Cost::new(5.0).unwrap(),
                },
            ],
            overview: Overview::default(),
            ..Default::default()
        });
        let pricing = Arc::new(FakePricingRepository::default());
        let clock: Arc<dyn Clock> = Arc::new(FixedClock::new(2026, 4, 23));
        let usage_dyn: Arc<dyn UsageRepository> = usage;
        let pricing_dyn: Arc<dyn PricingRepository> = pricing;
        let uc = GetDashboard::new(usage_dyn, pricing_dyn, clock);

        let out = uc.execute(GetDashboardInput::default()).unwrap();
        // Expected order: d2/c-model (5.0), d2/b-model (2.0), d1/a-model (1.0).
        assert_eq!(out.rows[0].date, d2);
        assert_eq!(out.rows[0].model.as_str(), "c-model");
        assert_eq!(out.rows[1].date, d2);
        assert_eq!(out.rows[1].model.as_str(), "b-model");
        assert_eq!(out.rows[2].date, d1);
    }
}
