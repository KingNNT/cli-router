//! RequestLogPort — abstracts the request-log writer.

use crate::application::errors::ProxyError;
use crate::domain::{RequestStart, RequestUsage};

pub trait RequestLogPort: Send + Sync {
    fn insert_started(&self, start: &RequestStart) -> Result<(), ProxyError>;
    fn complete(&self, id: &str, finished_at: i64, usage: &RequestUsage) -> Result<(), ProxyError>;
    fn fail(
        &self,
        id: &str,
        finished_at: i64,
        error_message: &str,
        usage: &RequestUsage,
    ) -> Result<(), ProxyError>;

    /// Mark all rows still in `status = 'started'` that are older than
    /// `cutoff_ms` (epoch millis) as `errored` with a timed-out message.
    /// Returns the number of rows swept.
    fn sweep_stale(&self, cutoff_ms: i64) -> Result<u64, ProxyError>;
}
