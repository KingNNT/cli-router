//! Kimi (Moonshot) account usage adapter.
//!
//! Queries Moonshot's `/v1/users/me/balance` endpoint to show account balance
//! as a usage window. Moonshot does not expose a public token-usage API.

use std::time::Duration;

use serde::Deserialize;

use crate::application::errors::ProxyError;
use crate::application::ports::AccountUsagePort;
use crate::domain::account_usage::{
    AccountUsageStatus, ProviderAccountUsage, UsageSubItem, UsageWindow,
};

const BALANCE_URL: &str = "https://api.moonshot.ai/v1/users/me/balance";

/// Kimi account usage adapter. Queries the Moonshot balance API.
pub struct KimiAccountUsage {
    provider_name: String,
    auth_token: String,
    agent: ureq::Agent,
}

impl KimiAccountUsage {
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

/// Converts Moonshot balance data into a balance usage window. Moonshot exposes
/// only remaining balance (no spend history), so values render as sub-items
/// rather than a progress bar.
fn balance_to_window(d: &BalanceData) -> UsageWindow {
    UsageWindow {
        label: "Balance".to_string(),
        used_pct: 0.0,
        used: None,
        limit: None,
        resets_at_ms: None,
        sub_items: vec![
            UsageSubItem {
                label: format!("available: {:.2}", d.available_balance),
                used: 0,
            },
            UsageSubItem {
                label: format!("cash: {:.2}", d.cash_balance),
                used: 0,
            },
            UsageSubItem {
                label: format!("voucher: {:.2}", d.voucher_balance),
                used: 0,
            },
        ],
        is_balance_info: true,
    }
}

impl AccountUsagePort for KimiAccountUsage {
    fn fetch_usage(&self) -> Option<Result<ProviderAccountUsage, ProxyError>> {
        if self.auth_token.is_empty() {
            return None;
        }

        let resp = match self
            .agent
            .get(BALANCE_URL)
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
            .data
            .as_ref()
            .map(|d| vec![balance_to_window(d)])
            .unwrap_or_default();

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
// Moonshot response JSON shapes
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct BalanceResponse {
    #[serde(default)]
    data: Option<BalanceData>,
}

#[derive(Debug, Deserialize)]
struct BalanceData {
    #[serde(default)]
    available_balance: f64,
    #[serde(default)]
    voucher_balance: f64,
    #[serde(default)]
    cash_balance: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_balance_response() {
        let json = r#"{
            "code": 0,
            "data": { "available_balance": 49.58, "voucher_balance": 46.58, "cash_balance": 3.00 },
            "scode": "0x0",
            "status": true
        }"#;
        let resp: BalanceResponse = serde_json::from_str(json).unwrap();
        let data = resp.data.unwrap();
        assert!((data.available_balance - 49.58).abs() < 1e-6);
        assert!((data.cash_balance - 3.00).abs() < 1e-6);
        assert!((data.voucher_balance - 46.58).abs() < 1e-6);
    }

    #[test]
    fn deserialize_balance_missing_data() {
        let json = r#"{"code": 0, "status": true}"#;
        let resp: BalanceResponse = serde_json::from_str(json).unwrap();
        assert!(resp.data.is_none());
    }

    #[test]
    fn window_maps_available_and_is_balance_info() {
        let data = BalanceData { available_balance: 49.58, voucher_balance: 46.58, cash_balance: 3.0 };
        let w = balance_to_window(&data);
        assert!(w.is_balance_info);
        assert_eq!(w.label, "Balance");
        assert_eq!(w.sub_items[0].label, "available: 49.58");
    }

    #[test]
    fn empty_token_returns_none() {
        let a = KimiAccountUsage::new("kimi".into(), String::new());
        assert!(a.fetch_usage().is_none());
    }
}
