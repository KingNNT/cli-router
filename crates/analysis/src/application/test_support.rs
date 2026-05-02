//! Test-only fakes for the analysis-specific application ports
//! (UsageRepository + PricingSource).

use std::sync::Mutex;

use shared::application::errors::ApplicationError;
use shared::domain::entities::ModelPricing;
use shared::domain::entities::{DayModelRow, ModelUsage, Overview, ProjectUsage};

use crate::application::dto::Filter;
use crate::application::ports::{PricingSource, UsageRepository};

#[derive(Default)]
pub struct FakeUsageRepository {
    pub overview: Overview,
    pub daily: Vec<DayModelRow>,
    pub by_model: Vec<ModelUsage>,
    pub by_project: Vec<ProjectUsage>,
    pub last_filter: Mutex<Option<Filter>>,
}

impl UsageRepository for FakeUsageRepository {
    fn overview(&self, filter: &Filter) -> Result<Overview, ApplicationError> {
        *self.last_filter.lock().unwrap() = Some(filter.clone());
        Ok(self.overview.clone())
    }

    fn daily_by_model(&self, filter: &Filter) -> Result<Vec<DayModelRow>, ApplicationError> {
        *self.last_filter.lock().unwrap() = Some(filter.clone());
        Ok(self.daily.clone())
    }

    fn by_model(&self, filter: &Filter) -> Result<Vec<ModelUsage>, ApplicationError> {
        *self.last_filter.lock().unwrap() = Some(filter.clone());
        Ok(self.by_model.clone())
    }

    fn by_project(&self, filter: &Filter) -> Result<Vec<ProjectUsage>, ApplicationError> {
        *self.last_filter.lock().unwrap() = Some(filter.clone());
        Ok(self.by_project.clone())
    }
}

#[derive(Default)]
pub struct FakePricingSource {
    pub rows: Vec<ModelPricing>,
    pub err: Option<String>,
}

impl PricingSource for FakePricingSource {
    fn fetch_all(&self) -> Result<Vec<ModelPricing>, ApplicationError> {
        if let Some(e) = &self.err {
            return Err(ApplicationError::InvalidInput(e.clone()));
        }
        Ok(self.rows.clone())
    }
}
