use std::collections::HashSet;

use chrono::NaiveDate;

use crate::application::dto::Filter;
use shared::domain::entities::ModelUsage;
use shared::domain::value_objects::ModelId;

#[derive(Debug, Clone, Default)]
pub struct GetModelsBreakdownInput {
    pub filter: Option<Filter>,
}

#[derive(Debug, Clone)]
pub struct GetModelsBreakdownOutput {
    pub filter_applied: Filter,
    pub models: Vec<ModelUsage>,
    pub missing_pricing_count: usize,
    pub unpriced_models: HashSet<ModelId>,
    pub last_pricing_sync: Option<NaiveDate>,
}
