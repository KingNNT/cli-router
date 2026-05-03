#[derive(Debug, Clone, Default)]
pub struct PricingViewModel {
    pub rows: Vec<PricingRowVM>,
    pub last_sync_label: String,
    pub empty: bool,
    pub query: String,
}

#[derive(Debug, Clone)]
pub struct PricingRowVM {
    pub model: String,
    pub provider: String,
    pub input: String,
    pub output: String,
    pub cache_read: String,
    pub cache_write: String,
}
