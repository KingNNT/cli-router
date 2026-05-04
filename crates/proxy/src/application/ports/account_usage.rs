//! AccountUsagePort — application-layer interface for querying upstream
//! provider account-level usage and quota.

use crate::application::errors::ProxyError;
use crate::domain::account_usage::ProviderAccountUsage;

/// Each configured provider gets one adapter implementing this trait.
/// The use case iterates all adapters and merges the results.
pub trait AccountUsagePort: Send + Sync {
    /// Fetch account-level usage/quota from the upstream provider.
    ///
    /// Returns `None` if this provider kind does not support usage queries
    /// (e.g. Anthropic has no public usage API).
    /// Returns `Some(Ok(..))` with the snapshot on success.
    /// Returns `Some(Err(..))` on upstream failure.
    fn fetch_usage(&self) -> Option<Result<ProviderAccountUsage, ProxyError>>;
}
