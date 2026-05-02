use std::collections::HashSet;

use chrono::NaiveDate;

use crate::application::dto::Filter;
use shared::domain::entities::{DayModelRow, Overview};
use shared::domain::value_objects::ModelId;

#[derive(Debug, Clone, Default)]
pub struct GetDashboardInput {
    pub filter: Option<Filter>,
}

#[derive(Debug, Clone)]
pub struct GetDashboardOutput {
    pub filter_applied: Filter,
    pub overview: Overview,
    pub rows: Vec<DayModelRow>,
    pub missing_pricing_count: usize,
    pub unpriced_models: HashSet<ModelId>,
    pub last_pricing_sync: Option<NaiveDate>,
}
