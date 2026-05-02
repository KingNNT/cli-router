use std::collections::HashMap;

use chrono::NaiveDate;

use crate::application::errors::ApplicationError;
use crate::domain::entities::ModelPricing;

pub trait PricingRepository: Send + Sync {
    fn upsert_many(&self, rows: &[ModelPricing]) -> Result<usize, ApplicationError>;
    fn find_many(&self, keys: &[String])
        -> Result<HashMap<String, ModelPricing>, ApplicationError>;
    fn list(&self) -> Result<Vec<ModelPricing>, ApplicationError>;
    fn last_sync(&self) -> Result<Option<NaiveDate>, ApplicationError>;
}
