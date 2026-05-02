#[derive(Debug, Clone, Default)]
pub struct ModelsViewModel {
    pub rows: Vec<ModelRowVM>,
    pub pricing_note: Option<String>,
    pub empty: bool,
}

#[derive(Debug, Clone)]
pub struct ModelRowVM {
    pub model: String,
    pub messages: String,
    pub input: String,
    pub output: String,
    pub reasoning: String,
    pub cost: String,
}
