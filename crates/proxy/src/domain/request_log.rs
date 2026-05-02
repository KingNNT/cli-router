//! Request-log domain types.

#[derive(Debug, Clone)]
pub struct RequestStart {
    pub id: String, // uuid v4
    pub user_id: i64,
    pub provider: String, // 'anthropic'
    pub model: String,
    pub started_at: i64, // epoch ms
}

#[derive(Debug, Clone, Default)]
pub struct RequestUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_creation_tokens: Option<u64>,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestStatus {
    Started,
    Completed,
    Errored,
}

impl RequestStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            RequestStatus::Started => "started",
            RequestStatus::Completed => "completed",
            RequestStatus::Errored => "errored",
        }
    }
}
