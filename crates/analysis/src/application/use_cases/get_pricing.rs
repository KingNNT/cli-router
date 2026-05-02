use std::sync::Arc;

use crate::application::dto::{GetPricingInput, GetPricingOutput};
use shared::application::errors::ApplicationError;
use shared::application::ports::PricingRepository;

pub struct GetPricing {
    repo: Arc<dyn PricingRepository>,
}

impl GetPricing {
    pub fn new(repo: Arc<dyn PricingRepository>) -> Self {
        Self { repo }
    }

    pub fn execute(&self, _input: GetPricingInput) -> Result<GetPricingOutput, ApplicationError> {
        let rows = self.repo.list()?;
        let last_sync = self.repo.last_sync()?;
        Ok(GetPricingOutput { rows, last_sync })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shared::application::test_support::FakePricingRepository;
    use chrono::NaiveDate;
    use shared::domain::entities::ModelPricing;
    use shared::domain::value_objects::{ModelId, PricePerToken};

    #[test]
    fn lists_rows_and_reports_last_sync() {
        let repo = Arc::new(FakePricingRepository::default());
        let row = ModelPricing {
            lookup_key: "m1".into(),
            model: ModelId::new("m1").unwrap(),
            provider_id: "p".into(),
            input_rate: PricePerToken::new(0.00001).unwrap(),
            output_rate: PricePerToken::new(0.00003).unwrap(),
            cache_read_rate: None,
            cache_write_rate: None,
            last_synced: NaiveDate::from_ymd_opt(2026, 4, 23).unwrap(),
            alias: None,
        };
        repo.upsert_many(&[row]).unwrap();
        let repo_dyn: Arc<dyn PricingRepository> = repo;
        let uc = GetPricing::new(repo_dyn);
        let out = uc.execute(GetPricingInput).unwrap();
        assert_eq!(out.rows.len(), 1);
        assert_eq!(
            out.last_sync,
            Some(NaiveDate::from_ymd_opt(2026, 4, 23).unwrap())
        );
    }

    #[test]
    fn empty_repo_reports_no_sync() {
        let repo = Arc::new(FakePricingRepository::default());
        let repo_dyn: Arc<dyn PricingRepository> = repo;
        let uc = GetPricing::new(repo_dyn);
        let out = uc.execute(GetPricingInput).unwrap();
        assert!(out.rows.is_empty());
        assert_eq!(out.last_sync, None);
    }
}
