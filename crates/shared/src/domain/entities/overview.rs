use crate::domain::value_objects::{Cost, DateRange, TokenBreakdown};

#[derive(Debug, Clone, PartialEq)]
pub struct Overview {
    pub range: DateRange,
    pub session_count: u64,
    pub message_count: u64,
    pub tokens: TokenBreakdown,
    pub cost: Cost,
}

impl Default for Overview {
    fn default() -> Self {
        Self {
            range: DateRange::unbounded(),
            session_count: 0,
            message_count: 0,
            tokens: TokenBreakdown::default(),
            cost: Cost::zero(),
        }
    }
}
