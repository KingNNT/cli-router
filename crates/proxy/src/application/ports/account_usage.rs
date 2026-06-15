//! AccountUsagePort — application-layer interface for querying upstream
//! provider account-level usage and quota.

use std::collections::HashMap;
use std::sync::Arc;

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

/// Supplies the current per-provider account-usage adapter map.
///
/// `GetAccountUsage` asks the registry for the live map on every call instead
/// of capturing it once at startup, so providers added at runtime (via the
/// admin API) appear in the account tab without a daemon restart.
///
/// The live implementation rebuilds only when the configured provider set
/// changes and reuses adapter instances otherwise, preserving each adapter's
/// internal response cache (e.g. Anthropic's 5-minute / 429-protection cache).
pub trait AccountUsageRegistry: Send + Sync {
    fn adapters(&self) -> HashMap<String, Arc<dyn AccountUsagePort>>;
}

/// A static map is itself a trivial registry — the provider set never changes.
/// Used in tests and any composition where hot reload isn't needed.
impl AccountUsageRegistry for HashMap<String, Arc<dyn AccountUsagePort>> {
    fn adapters(&self) -> HashMap<String, Arc<dyn AccountUsagePort>> {
        self.clone()
    }
}
