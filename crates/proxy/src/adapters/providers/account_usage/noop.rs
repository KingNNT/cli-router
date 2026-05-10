//! No-op account-usage adapter for providers that don't have a usage API.

use crate::application::ports::AccountUsagePort;
use crate::domain::account_usage::ProviderAccountUsage;
use crate::frameworks::ProxyError;

/// Returns `None` from `fetch_usage()`, indicating this provider kind
/// does not support account-level usage queries.
pub struct NoopAccountUsage;

impl AccountUsagePort for NoopAccountUsage {
    fn fetch_usage(&self) -> Option<Result<ProviderAccountUsage, ProxyError>> {
        None
    }
}
