//! Proxy domain: value types and entities, ring zero.
//! Imports: std, serde, chrono only.

pub mod request_log;
pub mod usage_record;

pub use request_log::{RequestStart, RequestStatus, RequestUsage};
pub use usage_record::{UsageRecord, UsageState};
