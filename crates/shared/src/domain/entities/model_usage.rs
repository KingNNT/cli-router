use crate::domain::value_objects::{Cost, ModelId, TokenBreakdown};

#[derive(Debug, Clone, PartialEq)]
pub struct ModelUsage {
    pub model: ModelId,
    pub message_count: u64,
    pub tokens: TokenBreakdown,
    pub cost: Cost,
}
