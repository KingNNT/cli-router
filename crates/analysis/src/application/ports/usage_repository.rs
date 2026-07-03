use crate::application::dto::Filter;
use shared::application::errors::ApplicationError;
use shared::domain::entities::{DayModelRow, Overview};

pub trait UsageRepository: Send + Sync {
    fn overview(&self, filter: &Filter) -> Result<Overview, ApplicationError>;
    fn daily_by_model(&self, filter: &Filter) -> Result<Vec<DayModelRow>, ApplicationError>;
}
