use std::sync::Arc;

use crate::application::dto::{SyncPricingInput, SyncPricingOutput};
use shared::application::errors::ApplicationError;
use crate::application::ports::PricingSource;
use shared::application::ports::{Clock, PricingRepository};

pub struct SyncPricing {
    source: Arc<dyn PricingSource>,
    repo: Arc<dyn PricingRepository>,
    clock: Arc<dyn Clock>,
}

impl SyncPricing {
    pub fn new(
        source: Arc<dyn PricingSource>,
        repo: Arc<dyn PricingRepository>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            source,
            repo,
            clock,
        }
    }

    pub fn execute(&self, _input: SyncPricingInput) -> Result<SyncPricingOutput, ApplicationError> {
        let rows = self.source.fetch_all()?;
        let count = self.repo.upsert_many(&rows)?;
        Ok(SyncPricingOutput {
            synced_count: count,
            last_synced_at: self.clock.today(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::test_support::FakePricingSource;
    use shared::application::test_support::{FakePricingRepository, FixedClock};
    use chrono::NaiveDate;
    use shared::domain::entities::ModelPricing;
    use shared::domain::value_objects::{ModelId, PricePerToken};

    fn make_pricing(key: &str, provider: &str) -> ModelPricing {
        ModelPricing {
            lookup_key: key.into(),
            model: ModelId::new(key).unwrap(),
            provider_id: provider.into(),
            input_rate: PricePerToken::new(0.00001).unwrap(),
            output_rate: PricePerToken::new(0.00003).unwrap(),
            cache_read_rate: None,
            cache_write_rate: None,
            last_synced: NaiveDate::from_ymd_opt(2026, 4, 22).unwrap(),
            alias: None,
        }
    }

    #[test]
    fn happy_path_syncs_and_reports_clock_today() {
        let source = Arc::new(FakePricingSource {
            rows: vec![make_pricing("m1", "p"), make_pricing("m2", "p")],
            err: None,
        });
        let repo = Arc::new(FakePricingRepository::default());
        let clock = Arc::new(FixedClock::new(2026, 4, 23));
        let source_dyn: Arc<dyn PricingSource> = source;
        let repo_dyn: Arc<dyn PricingRepository> = repo.clone();
        let uc = SyncPricing::new(source_dyn, repo_dyn, clock);
        let out = uc.execute(SyncPricingInput).unwrap();
        assert_eq!(out.synced_count, 2);
        assert_eq!(
            out.last_synced_at,
            NaiveDate::from_ymd_opt(2026, 4, 23).unwrap()
        );
        assert_eq!(repo.rows.lock().unwrap().len(), 2);
    }

    #[test]
    fn empty_source_syncs_zero() {
        let source = Arc::new(FakePricingSource::default());
        let repo = Arc::new(FakePricingRepository::default());
        let clock = Arc::new(FixedClock::new(2026, 4, 23));
        let source_dyn: Arc<dyn PricingSource> = source;
        let repo_dyn: Arc<dyn PricingRepository> = repo;
        let uc = SyncPricing::new(source_dyn, repo_dyn, clock);
        let out = uc.execute(SyncPricingInput).unwrap();
        assert_eq!(out.synced_count, 0);
    }

    #[test]
    fn source_error_propagates() {
        let source = Arc::new(FakePricingSource {
            err: Some("network".into()),
            ..Default::default()
        });
        let repo = Arc::new(FakePricingRepository::default());
        let clock = Arc::new(FixedClock::new(2026, 4, 23));
        let source_dyn: Arc<dyn PricingSource> = source;
        let repo_dyn: Arc<dyn PricingRepository> = repo;
        let uc = SyncPricing::new(source_dyn, repo_dyn, clock);
        assert!(uc.execute(SyncPricingInput).is_err());
    }
}
