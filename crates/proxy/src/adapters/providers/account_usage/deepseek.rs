//! DeepSeek account usage adapter.
//!
//! Queries DeepSeek's `/user/balance` endpoint to show account balance
//! as a usage window. DeepSeek does not expose a public token-usage API.

use std::time::Duration;

use serde::Deserialize;

use crate::application::errors::ProxyError;
use crate::application::ports::AccountUsagePort;
use crate::domain::account_usage::{
    AccountUsageStatus, ProviderAccountUsage, UsageWindow,
};

/// DeepSeek account usage adapter. Queries balance API.
pub struct DeepSeekAccountUsage {
    provider_name: String,
    auth_token: String,
    agent: ureq::Agent,
}

impl DeepSeekAccountUsage {
    pub fn new(provider_name: String, auth_token: String) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_read(Duration::from_secs(10))
            .timeout_write(Duration::from_secs(10))
            .build();
        Self {
            provider_name,
            auth_token,
            agent,
        }
    }
}

impl AccountUsagePort for DeepSeekAccountUsage {
    fn fetch_usage(&self) -> Option<Result<ProviderAccountUsage, ProxyError>> {
        // If no auth token configured, this provider can't query usage.
        if self.auth_token.is_empty() {
            return None;
        }

        let url = "https://api.deepseek.com/user/balance";

        let resp = match self
            .agent
            .get(url)
            .set("Authorization", &format!("Bearer {}", self.auth_token))
            .set("Accept", "application/json")
            .call()
        {
            Ok(r) => r,
            Err(e) => {
                return Some(Err(ProxyError::UpstreamUsage {
                    provider: self.provider_name.clone(),
                    message: format!("balance API: {e}"),
                }));
            }
        };

        let envelope: BalanceResponse = match resp.into_json() {
            Ok(e) => e,
            Err(e) => {
                return Some(Err(ProxyError::UpstreamUsage {
                    provider: self.provider_name.clone(),
                    message: format!("balance decode: {e}"),
                }));
            }
        };

        let mut windows = Vec::new();

        for info in &envelope.balance_infos {
            let total: f64 = info.total_balance.parse().unwrap_or(0.0);
            let topped_up: f64 = info.topped_up_balance.parse().unwrap_or(0.0);
            let used = ((total - topped_up) * 100.0).max(0.0) as u64;
            let limit = (total * 100.0).max(0.0) as u64;
            let used_pct = if limit > 0 {
                (used as f64 / limit as f64) * 100.0
            } else {
                0.0
            };

            windows.push(UsageWindow {
                label: format!("Top-up Balance ({})", info.currency),
                used_pct,
                used: Some(used),
                limit: Some(limit),
                resets_at_ms: None,
                sub_items: vec![],
            });
        }

        Some(Ok(ProviderAccountUsage {
            provider: self.provider_name.clone(),
            status: AccountUsageStatus::Available,
            plan: None,
            windows,
            model_usage: None,
        }))
    }
}

// ---------------------------------------------------------------------------
// DeepSeek response JSON shapes
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct BalanceResponse {
    #[allow(dead_code)]
    is_available: bool,
    #[serde(default)]
    balance_infos: Vec<BalanceInfo>,
}

#[derive(Debug, Deserialize)]
struct BalanceInfo {
    currency: String,
    total_balance: String,
    #[serde(default)]
    #[allow(dead_code)]
    granted_balance: String,
    #[serde(default)]
    topped_up_balance: String,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_balance_response() {
        let json = r#"{
            "is_available": true,
            "balance_infos": [
                {
                    "currency": "CNY",
                    "total_balance": "110.00",
                    "granted_balance": "10.00",
                    "topped_up_balance": "100.00"
                }
            ]
        }"#;
        let resp: BalanceResponse = serde_json::from_str(json).unwrap();
        assert!(resp.is_available);
        assert_eq!(resp.balance_infos.len(), 1);
        assert_eq!(resp.balance_infos[0].currency, "CNY");
        assert_eq!(resp.balance_infos[0].total_balance, "110.00");
        assert_eq!(resp.balance_infos[0].topped_up_balance, "100.00");
    }

    #[test]
    fn deserialize_balance_no_balances() {
        let json = r#"{"is_available": true}"#;
        let resp: BalanceResponse = serde_json::from_str(json).unwrap();
        assert!(resp.is_available);
        assert!(resp.balance_infos.is_empty());
    }

    #[test]
    fn deserialize_balance_zero_values() {
        let json = r#"{
            "is_available": false,
            "balance_infos": [
                {
                    "currency": "USD",
                    "total_balance": "0.00",
                    "granted_balance": "0.00",
                    "topped_up_balance": "0.00"
                }
            ]
        }"#;
        let resp: BalanceResponse = serde_json::from_str(json).unwrap();
        assert!(!resp.is_available);
        assert_eq!(resp.balance_infos[0].total_balance, "0.00");
    }
}
