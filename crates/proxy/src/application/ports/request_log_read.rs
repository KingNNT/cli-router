//! RequestLogReadPort — read-only view over the request log used by the
//! admin API. Splits cleanly from `RequestLogPort` (write-side) so the
//! admin reader can be implemented separately, including by a read-only
//! SQLite handle if we ever care to enforce that at the type level.

use crate::application::errors::ProxyError;
use crate::domain::{RequestRow, UsageSummary};
use std::collections::BTreeMap;

/// A single completed request, projected for quota counter seeding.
#[derive(Debug, Clone)]
pub struct QuotaSeedRow {
    pub provider: String,
    pub started_at_ms: i64,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

/// Aggregated translation counts returned by [`RequestLogReadPort::count_translations`].
#[derive(Debug, Clone, Default)]
pub struct TranslationCounts {
    pub completed: u64,
    pub failed: u64,
    pub by_direction: BTreeMap<String, u64>,
}


/// One row of per-model usage breakdown.
#[derive(Debug, Clone)]
pub struct ModelBreakdownRow {
    pub model: String,
    pub tokens: u64,
    pub calls: u64,
    pub cost_usd: f64,
}

pub trait RequestLogReadPort: Send + Sync {
    fn total_count(&self) -> Result<u64, ProxyError>;
    fn count_by_provider(&self) -> Result<BTreeMap<String, u64>, ProxyError>;
    fn count_by_status(&self) -> Result<BTreeMap<String, u64>, ProxyError>;
    fn recent(&self, limit: u32, offset: u32) -> Result<Vec<RequestRow>, ProxyError>;
    fn summarize(&self, from_ms: i64, to_ms: i64) -> Result<UsageSummary, ProxyError>;
    fn quota_seed(&self, cutoff_ms: i64) -> Result<Vec<QuotaSeedRow>, ProxyError>;
    fn count_translations(&self) -> Result<TranslationCounts, ProxyError>;

    /// Per-model token+calls breakdown for a provider in the given time range.
    fn model_breakdown(
        &self,
        provider: &str,
        from_ms: i64,
        to_ms: i64,
    ) -> Result<Vec<ModelBreakdownRow>, ProxyError>;

    /// Total cost in USD for a provider over the last 30 days.
    fn monthly_cost(&self, provider: &str) -> Result<f64, ProxyError>;
}
