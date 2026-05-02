use crate::application::dto::Filter;
use shared::domain::entities::ProjectUsage;

#[derive(Debug, Clone, Default)]
pub struct GetProjectsBreakdownInput {
    pub filter: Option<Filter>,
}

#[derive(Debug, Clone)]
pub struct GetProjectsBreakdownOutput {
    pub filter_applied: Filter,
    pub projects: Vec<ProjectUsage>,
}
