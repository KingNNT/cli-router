//! RequestLogReadPort — read-only view over the request log used by the
//! admin API. Splits cleanly from `RequestLogPort` (write-side) so the
//! admin reader can be implemented separately, including by a read-only
//! SQLite handle if we ever care to enforce that at the type level.

use crate::application::errors::ProxyError;
use crate::domain::{RequestRow, UsageSummary};
use std::collections::BTreeMap;

pub trait RequestLogReadPort: Send + Sync {
    fn total_count(&self) -> Result<u64, ProxyError>;
    fn count_by_provider(&self) -> Result<BTreeMap<String, u64>, ProxyError>;
    fn count_by_status(&self) -> Result<BTreeMap<String, u64>, ProxyError>;
    fn recent(&self, limit: u32) -> Result<Vec<RequestRow>, ProxyError>;
    fn summarize(&self, from_ms: i64, to_ms: i64) -> Result<UsageSummary, ProxyError>;
}
