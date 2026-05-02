use chrono::NaiveDate;

use crate::domain::value_objects::{Cost, ModelId, ProjectPath, TokenBreakdown};

#[derive(Debug, Clone)]
pub struct UsageRecord {
    pub date: NaiveDate,
    pub model: ModelId,
    pub project: ProjectPath,
    pub tokens: TokenBreakdown,
    pub cost: Cost,
    pub session_id: String,
}
