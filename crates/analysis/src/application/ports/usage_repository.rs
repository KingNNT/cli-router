use crate::application::dto::Filter;
use shared::application::errors::ApplicationError;
use shared::domain::entities::{DayModelRow, ModelUsage, Overview, ProjectUsage};

pub trait UsageRepository: Send + Sync {
    fn overview(&self, filter: &Filter) -> Result<Overview, ApplicationError>;
    fn daily_by_model(&self, filter: &Filter) -> Result<Vec<DayModelRow>, ApplicationError>;
    fn by_model(&self, filter: &Filter) -> Result<Vec<ModelUsage>, ApplicationError>;
    fn by_project(&self, filter: &Filter) -> Result<Vec<ProjectUsage>, ApplicationError>;
}
