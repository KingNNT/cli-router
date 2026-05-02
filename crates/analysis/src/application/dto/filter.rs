use shared::domain::value_objects::{DateRange, ModelId, ProjectPath};

#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub date_range: Option<DateRange>,
    pub project: Option<ProjectPath>,
    pub model: Option<ModelId>,
    pub provider: Option<String>,
    pub session_id: Option<String>,
}
