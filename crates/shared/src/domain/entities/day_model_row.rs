use chrono::NaiveDate;

use crate::domain::value_objects::{Cost, ModelId, TokenBreakdown};

#[derive(Debug, Clone, PartialEq)]
pub struct DayModelRow {
    pub date: NaiveDate,
    pub model: ModelId,
    pub tokens: TokenBreakdown,
    pub cost: Cost,
}
