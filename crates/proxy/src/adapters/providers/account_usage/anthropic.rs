//! Anthropic has no public usage API. This adapter always returns `None`.

use crate::application::errors::ProxyError;
use crate::application::ports::AccountUsagePort;
use crate::domain::account_usage::ProviderAccountUsage;

/// No-op account usage adapter for Anthropic providers.
/// Always returns `None` because Anthropic has no public usage/quota API.
pub struct AnthropicAccountUsage;

impl AccountUsagePort for AnthropicAccountUsage {
    fn fetch_usage(&self) -> Option<Result<ProviderAccountUsage, ProxyError>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_returns_none() {
        let adapter = AnthropicAccountUsage;
        assert!(adapter.fetch_usage().is_none());
    }
}
