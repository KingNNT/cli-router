//! QuotaPort — application-layer interface for usage-quota tracking.

use crate::domain::RequestUsage;
use crate::domain::quota::QuotaCheck;

pub trait QuotaPort: Send + Sync {
    /// Pre-flight check for the given provider name. Returns `Ok` when no
    /// quota config matches that provider. Caller should reject when it
    /// returns `Reject`, log a warning when `Warn`, and proceed otherwise.
    fn check(&self, provider: &str) -> QuotaCheck;

    /// Post-response record. Adds the request to the counter for the
    /// matching provider. Idempotent if no quota config matches.
    fn record(&self, provider: &str, usage: &RequestUsage);
}
