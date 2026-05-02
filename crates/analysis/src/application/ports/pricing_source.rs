use shared::application::errors::ApplicationError;
use shared::domain::entities::ModelPricing;

pub trait PricingSource: Send + Sync {
    fn fetch_all(&self) -> Result<Vec<ModelPricing>, ApplicationError>;
}
