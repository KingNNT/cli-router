use chrono::NaiveDate;

use crate::domain::value_objects::{Cost, TokenBreakdown};

#[derive(Debug, Clone, PartialEq)]
pub struct DailyUsage {
    pub date: NaiveDate,
    pub message_count: u64,
    pub tokens: TokenBreakdown,
    pub cost: Cost,
}
