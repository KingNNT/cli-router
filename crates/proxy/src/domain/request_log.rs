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
    /// e.g. "anthropic→openai" or "openai→anthropic"; None when passthrough.
    pub translation_direction: Option<String>,
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

/// Snapshot of a row in the `requests` table — what the read port returns.
#[derive(Debug, Clone)]
pub struct RequestRow {
    pub id: String,
    pub started_at_ms: i64,
    pub finished_at_ms: Option<i64>,
    pub provider: String,
    pub model: String,
    pub status: String,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_creation_tokens: Option<i64>,
    pub cost_usd: Option<f64>,
    pub error_message: Option<String>,
    pub translation_direction: Option<String>,
}
