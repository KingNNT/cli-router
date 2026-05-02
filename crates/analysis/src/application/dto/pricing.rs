use chrono::NaiveDate;

use shared::domain::entities::ModelPricing;

#[derive(Debug, Clone, Default)]
pub struct GetPricingInput;

#[derive(Debug, Clone)]
pub struct GetPricingOutput {
    pub rows: Vec<ModelPricing>,
    pub last_sync: Option<NaiveDate>,
}
