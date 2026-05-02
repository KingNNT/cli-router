#[derive(Debug, Clone, Default)]
pub struct ProjectsViewModel {
    pub rows: Vec<ProjectRowVM>,
    pub empty: bool,
}

#[derive(Debug, Clone)]
pub struct ProjectRowVM {
    pub project: String,
    pub messages: String,
    pub input: String,
    pub output: String,
    pub cost: String,
}
