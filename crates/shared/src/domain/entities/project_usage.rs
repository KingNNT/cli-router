use crate::domain::value_objects::{Cost, ProjectPath, TokenBreakdown};

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectUsage {
    pub project: ProjectPath,
    pub message_count: u64,
    pub tokens: TokenBreakdown,
    pub cost: Cost,
}
