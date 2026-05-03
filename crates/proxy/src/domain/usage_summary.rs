//! Domain types for the proxy usage summary.
//!
//! Mirrors the wire DTOs in `proxy_admin_api` (`UsageSummaryResponse`,
//! `DailyUsageRow`, `ModelUsageRow`) but stays inside the proxy crate so the
//! application layer doesn't need to depend on `proxy_admin_api`. The
//! application boundary maps domain → DTO.

#[derive(Debug, Clone, PartialEq)]
pub struct UsageSummary {
    pub from_ms: i64,
    pub to_ms: i64,
    pub daily: Vec<DailyTotal>,
    pub models: Vec<ModelTotal>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DailyTotal {
    pub date: String, // YYYY-MM-DD, local tz
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelTotal {
    pub model: String,
    pub provider: String,
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cost_usd: f64,
}
