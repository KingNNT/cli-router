//! Domain types for upstream provider account-level usage and quota.

/// One provider's account-level usage snapshot.
#[derive(Debug, Clone)]
pub struct ProviderAccountUsage {
    pub provider: String,
    pub status: AccountUsageStatus,
    pub plan: Option<String>,
    pub windows: Vec<UsageWindow>,
    pub model_usage: Option<ModelUsageSnapshot>,
}

/// Whether the adapter could retrieve usage data.
#[derive(Debug, Clone, PartialEq)]
pub enum AccountUsageStatus {
    /// Data successfully retrieved.
    Available,
    /// This provider kind has no public usage API (e.g. Anthropic).
    NotSupported,
    /// Upstream call failed.
    Error(String),
}

/// One quota window (e.g. "5h Token", "Weekly", "MCP (1 Month)").
#[derive(Debug, Clone)]
pub struct UsageWindow {
    /// Human-readable label, e.g. "5h Token", "Weekly", "MCP (1 Month)".
    pub label: String,
    /// Percentage used, 0.0–100.0.
    pub used_pct: f64,
    /// Absolute usage count, if the API returns it.
    pub used: Option<u64>,
    /// Absolute limit, if the API returns it.
    pub limit: Option<u64>,
    /// When this window resets, in epoch milliseconds.
    pub resets_at_ms: Option<i64>,
    /// Sub-items (e.g. individual MCP tool counts under the MCP window).
    pub sub_items: Vec<UsageSubItem>,
}

/// A sub-item within a quota window (e.g. "Network Searches: 5678").
#[derive(Debug, Clone)]
pub struct UsageSubItem {
    pub label: String,
    pub used: u64,
}

/// Model usage snapshot over a time window (typically 24h).
#[derive(Debug, Clone)]
pub struct ModelUsageSnapshot {
    pub total_tokens: u64,
    pub total_calls: u64,
    pub period_start_ms: i64,
    pub period_end_ms: i64,
    pub model_breakdown: Vec<ModelBreakdownItem>,
}

/// Per-model usage breakdown within a snapshot period.
#[derive(Debug, Clone)]
pub struct ModelBreakdownItem {
    pub model: String,
    pub tokens: u64,
    pub calls: u64,
}
