//! DeepSeek account usage adapter.
//!
//! Queries DeepSeek's `/user/balance` endpoint to show account balance
//! as a usage window. DeepSeek does not expose a public token-usage API.

use std::time::Duration;

use serde::Deserialize;

use crate::application::errors::ProxyError;
use crate::application::ports::AccountUsagePort;
use crate::domain::account_usage::{
    AccountUsageStatus, ProviderAccountUsage, UsageSubItem, UsageWindow,
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

/// Converts a DeepSeek balance info into a usage window showing the remaining
/// balance breakdown. DeepSeek does not expose spending history, so we display
/// the raw balance values as sub-items instead of a progress bar.
fn balance_to_window(info: &BalanceInfo) -> UsageWindow {
    let total: f64 = info.total_balance.parse().unwrap_or(0.0);
    let granted: f64 = info.granted_balance.parse().unwrap_or(0.0);
    let topped_up: f64 = info.topped_up_balance.parse().unwrap_or(0.0);

    UsageWindow {
        label: format!("Balance ({})", info.currency),
        used_pct: 0.0, // DeepSeek only exposes remaining balance, not spending
        used: None,
        limit: None,
        resets_at_ms: None,
        sub_items: vec![
            UsageSubItem {
                label: format!("total: {:.2}", total),
                used: 0,
            },
            UsageSubItem {
                label: format!("topped-up: {:.2}", topped_up),
                used: 0,
            },
            UsageSubItem {
                label: format!("granted: {:.2}", granted),
                used: 0,
            },
        ],
        is_balance_info: true,
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

        let windows: Vec<UsageWindow> = envelope
            .balance_infos
            .iter()
            .map(balance_to_window)
            .collect();

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

    // ── balance_to_window tests ──

    #[test]
    fn balance_to_window_shows_total_topped_up_granted() {
        let info = BalanceInfo {
            currency: "CNY".to_string(),
            total_balance: "110.00".to_string(),
            granted_balance: "10.00".to_string(),
            topped_up_balance: "100.00".to_string(),
        };
        let w = balance_to_window(&info);
        assert_eq!(w.label, "Balance (CNY)");
        assert_eq!(w.used_pct, 0.0);
        assert!(w.used.is_none());
        assert!(w.limit.is_none());
        assert!(w.resets_at_ms.is_none());
        assert_eq!(w.sub_items.len(), 3);
        assert_eq!(w.sub_items[0].label, "total: 110.00");
        assert_eq!(w.sub_items[1].label, "topped-up: 100.00");
        assert_eq!(w.sub_items[2].label, "granted: 10.00");
        assert!(w.is_balance_info);
    }

    #[test]
    fn balance_to_window_only_topped_up() {
        // Most common case: user only topped up, no granted balance.
        let info = BalanceInfo {
            currency: "CNY".to_string(),
            total_balance: "50.00".to_string(),
            granted_balance: "0.00".to_string(),
            topped_up_balance: "50.00".to_string(),
        };
        let w = balance_to_window(&info);
        assert_eq!(w.sub_items[0].label, "total: 50.00");
        assert_eq!(w.sub_items[1].label, "topped-up: 50.00");
        assert_eq!(w.sub_items[2].label, "granted: 0.00");
        // used_pct should be 0 — we cannot derive spending from balance alone.
        assert_eq!(w.used_pct, 0.0);
        assert!(w.is_balance_info);
    }

    #[test]
    fn balance_to_window_handles_zero_balance() {
        let info = BalanceInfo {
            currency: "USD".to_string(),
            total_balance: "0.00".to_string(),
            granted_balance: "0.00".to_string(),
            topped_up_balance: "0.00".to_string(),
        };
        let w = balance_to_window(&info);
        assert_eq!(w.label, "Balance (USD)");
        assert_eq!(w.sub_items[0].label, "total: 0.00");
        assert!(w.is_balance_info);
    }
}
