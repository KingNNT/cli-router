//! Minimax account usage adapter.
//!
//! Queries Minimax's `/v1/api/openplatform/coding_plan/remains` endpoint to
//! show Token Plan quota windows (5h rolling, weekly) with remaining counts
//! and reset timers.

use std::time::Duration;

use serde::Deserialize;

use crate::application::errors::ProxyError;
use crate::application::ports::AccountUsagePort;
use crate::domain::account_usage::{
    AccountUsageStatus, ProviderAccountUsage, UsageSubItem, UsageWindow,
};

const USAGE_PATH: &str = "/v1/token_plan/remains";
const SUBSCRIPTION_PATH: &str = "/v1/api/openplatform/charge/combo/cycle_audio_resource_package";

/// Derive the usage host from the provider's configured base_url.
/// - `api.minimaxi.com` (CN) → `https://www.minimaxi.com`
/// - `api.minimax.io` (international) → `https://api.minimax.io`
/// Falls back to CN if no base_url is set.
fn derive_usage_host(base_url: Option<&str>) -> String {
    let raw = base_url.unwrap_or("https://api.minimaxi.com/anthropic");
    let idx = raw.find("://").unwrap_or(0);
    let rest = &raw[idx..];
    let host_end = rest[3..].find('/').unwrap_or(rest.len() - 3);
    let host = &rest[3..3 + host_end];
    // api.minimaxi.com → www.minimaxi.com (CN usage endpoint)
    let host = if host == "api.minimaxi.com" {
        "www.minimaxi.com"
    } else {
        host
    };
    format!("https://{}", host)
}

/// Minimax account usage adapter. Queries Token Plan quota API.
pub struct MinimaxAccountUsage {
    provider_name: String,
    auth_token: String,
    usage_host: String,
    agent: ureq::Agent,
}

impl MinimaxAccountUsage {
    pub fn new(
        provider_name: String,
        auth_token: String,
        provider_base_url: Option<String>,
    ) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_read(Duration::from_secs(10))
            .timeout_write(Duration::from_secs(10))
            .build();
        let usage_host = derive_usage_host(provider_base_url.as_deref());
        Self {
            provider_name,
            auth_token,
            usage_host,
            agent,
        }
    }
}

/// Build a usage window from one model's 5h rolling quota.
fn five_hour_window(m: &ModelRemains) -> UsageWindow {
    let total = m.current_interval_total_count;
    let resets_at_ms = m.remains_time.map(|ms| now_ms() + ms);

    // The API uses two modes:
    //   - Count-based: total > 0, usage_count = used requests
    //   - Percent-based: total == 0, remaining_percent = remaining quota %
    let (used_count, used_pct) = if total > 0 {
        let used = m.current_interval_usage_count.unwrap_or(0).min(total);
        let pct = (used as f64 / total as f64) * 100.0;
        (used, pct)
    } else {
        let remaining_pct = m.current_interval_remaining_percent.unwrap_or(100.0);
        let pct = 100.0 - remaining_pct;
        (0, pct)
    };

    UsageWindow {
        label: format!("{} (5h)", m.model_name),
        used_pct,
        used: Some(used_count),
        limit: Some(total),
        resets_at_ms,
        sub_items: vec![UsageSubItem {
            label: if total > 0 {
                format!("{} used / {} total", used_count, total)
            } else {
                format!("{:.0}% used", used_pct)
            },
            used: used_count,
        }],
        is_balance_info: false,
    }
}

/// Build a usage window from one model's weekly quota.
fn weekly_window(m: &ModelRemains) -> UsageWindow {
    let total = m.current_weekly_total_count.unwrap_or(0);
    let resets_at_ms = m.weekly_remains_time.map(|ms| now_ms() + ms);

    let (used_count, used_pct) = if total > 0 {
        let used = m.current_weekly_usage_count.unwrap_or(0).min(total);
        let pct = (used as f64 / total as f64) * 100.0;
        (used, pct)
    } else {
        let remaining_pct = m.current_weekly_remaining_percent.unwrap_or(100.0);
        let pct = 100.0 - remaining_pct;
        (0, pct)
    };

    UsageWindow {
        label: format!("{} (weekly)", m.model_name),
        used_pct,
        used: Some(used_count),
        limit: if total > 0 { Some(total) } else { None },
        resets_at_ms,
        sub_items: if total > 0 {
            vec![UsageSubItem {
                label: format!("{} used / {} total", used_count, total),
                used: used_count,
            }]
        } else {
            vec![UsageSubItem {
                label: format!("{:.0}% used", used_pct),
                used: used_count,
            }]
        },
        is_balance_info: false,
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

impl AccountUsagePort for MinimaxAccountUsage {
    fn fetch_usage(&self) -> Option<Result<ProviderAccountUsage, ProxyError>> {
        if self.auth_token.is_empty() {
            return None;
        }

        // ── 1. Fetch quota data ──

        let usage_url = format!("{}{}", self.usage_host, USAGE_PATH);

        let resp = match self
            .agent
            .get(&usage_url)
            .set("Authorization", &format!("Bearer {}", self.auth_token))
            .set("Accept", "application/json")
            .call()
        {
            Ok(r) => r,
            Err(e) => {
                return Some(Err(ProxyError::UpstreamUsage {
                    provider: self.provider_name.clone(),
                    message: format!("Token Plan API: {e}"),
                }));
            }
        };

        let envelope: RemainsResponse = match resp.into_json() {
            Ok(e) => e,
            Err(e) => {
                return Some(Err(ProxyError::UpstreamUsage {
                    provider: self.provider_name.clone(),
                    message: format!("Token Plan decode: {e}"),
                }));
            }
        };

        // ── 2. Fetch subscription details (plan name) ──

        let plan = self.fetch_plan_name();

        // ── 3. Build windows ──

        let mut windows: Vec<UsageWindow> = Vec::new();

        // Take the first model entry (usually "general" covering all models).
        if let Some(m) = envelope.model_remains.first() {
            windows.push(five_hour_window(m));
            windows.push(weekly_window(m));

            // Additional model entries (video, hailuo, etc.) as 5h-only windows.
            for extra in envelope.model_remains.iter().skip(1) {
                windows.push(five_hour_window(extra));
            }
        }

        Some(Ok(ProviderAccountUsage {
            provider: self.provider_name.clone(),
            status: AccountUsageStatus::Available,
            plan,
            windows,
            model_usage: None,
        }))
    }
}

impl MinimaxAccountUsage {
    /// Query the subscription API and return the plan tier name (e.g. "Plus", "Max", "Ultra").
    /// Returns `None` silently on failure — plan display is cosmetic.
    fn fetch_plan_name(&self) -> Option<String> {
        let sub_url = format!("{}{}", self.usage_host, SUBSCRIPTION_PATH);
        let resp = self
            .agent
            .get(&sub_url)
            .query("biz_line", "2")
            .query("cycle_type", "1")
            .query("resource_package_type", "7")
            .set("Authorization", &format!("Bearer {}", self.auth_token))
            .set("Accept", "application/json")
            .call()
            .ok()?;

        let sub: SubscriptionResponse = resp.into_json().ok()?;
        sub.current_subscribe
            .as_ref()
            .map(|cs| cs.plan_name.clone())
    }
}

// ---------------------------------------------------------------------------
// Minimax subscription JSON shapes
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct SubscriptionResponse {
    #[serde(default)]
    current_subscribe: Option<CurrentSubscribe>,
}

#[derive(Debug, Deserialize)]
struct CurrentSubscribe {
    /// Plan tier name, e.g. "Plus", "Max", "Ultra".
    #[serde(default, rename = "plan_name")]
    plan_name: String,
}

// ---------------------------------------------------------------------------
// Minimax quota response JSON shapes
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct RemainsResponse {
    #[serde(default)]
    #[allow(dead_code)]
    base_resp: Option<BaseResp>,
    #[serde(default)]
    #[allow(dead_code)]
    status_code: Option<i64>,
    #[serde(default)]
    model_remains: Vec<ModelRemains>,
}

#[derive(Debug, Deserialize)]
struct BaseResp {
    #[serde(default)]
    #[allow(dead_code)]
    status_code: Option<i64>,
    #[serde(default)]
    #[allow(dead_code)]
    status_msg: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ModelRemains {
    /// Model name, e.g. "general", "video".
    #[serde(default)]
    model_name: String,
    /// Total requests allowed in the current 5h window (0 = percent-based).
    #[serde(default)]
    current_interval_total_count: u64,
    /// Used requests in the current 5h window (when total > 0).
    #[serde(default)]
    current_interval_usage_count: Option<u64>,
    /// Remaining percentage (0–100) in the current 5h window (when total == 0).
    #[serde(default)]
    current_interval_remaining_percent: Option<f64>,
    /// Status of current interval (0=normal, 1=active, 3=no quota).
    #[serde(default)]
    #[allow(dead_code)]
    current_interval_status: Option<u64>,
    /// Milliseconds until the 5h window resets.
    #[serde(default)]
    remains_time: Option<i64>,
    /// Window start time (epoch ms).
    #[serde(default)]
    #[allow(dead_code)]
    start_time: Option<i64>,
    /// Window end time (epoch ms).
    #[serde(default)]
    #[allow(dead_code)]
    end_time: Option<i64>,
    /// Total requests allowed in the current weekly window (0 = percent-based).
    #[serde(default)]
    current_weekly_total_count: Option<u64>,
    /// Used requests in the current weekly window (when total > 0).
    #[serde(default)]
    current_weekly_usage_count: Option<u64>,
    /// Remaining percentage (0–100) in the current weekly window (when total == 0).
    #[serde(default)]
    current_weekly_remaining_percent: Option<f64>,
    /// Status of current weekly window.
    #[serde(default)]
    #[allow(dead_code)]
    current_weekly_status: Option<u64>,
    /// Milliseconds until the weekly window resets.
    #[serde(default)]
    weekly_remains_time: Option<i64>,
    /// Weekly window start time (epoch ms).
    #[serde(default)]
    #[allow(dead_code)]
    weekly_start_time: Option<i64>,
    /// Weekly window end time (epoch ms).
    #[serde(default)]
    #[allow(dead_code)]
    weekly_end_time: Option<i64>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_subscription_response() {
        let json = r#"{
            "current_subscribe": {
                "plan_name": "Max",
                "current_subscribe_end_time": "2027-03-19"
            }
        }"#;
        let resp: SubscriptionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(
            resp.current_subscribe.as_ref().map(|c| &c.plan_name),
            Some(&"Max".to_string())
        );
    }

    #[test]
    fn deserialize_subscription_empty() {
        let json = r#"{}"#;
        let resp: SubscriptionResponse = serde_json::from_str(json).unwrap();
        assert!(resp.current_subscribe.is_none());
    }

    fn model_general() -> ModelRemains {
        ModelRemains {
            model_name: "general".into(),
            current_interval_total_count: 0,
            current_interval_usage_count: Some(0),
            current_interval_remaining_percent: Some(99.0),
            current_interval_status: Some(1),
            remains_time: Some(2_059_592),
            start_time: Some(1780549200000),
            end_time: Some(1780567200000),
            current_weekly_total_count: Some(0),
            current_weekly_usage_count: Some(0),
            current_weekly_remaining_percent: Some(99.0),
            current_weekly_status: Some(1),
            weekly_remains_time: Some(311_659_592),
            weekly_start_time: Some(1780272000000),
            weekly_end_time: Some(1780876800000),
        }
    }

    fn model_count_based() -> ModelRemains {
        ModelRemains {
            model_name: "general".into(),
            current_interval_total_count: 1500,
            current_interval_usage_count: Some(300),
            current_interval_remaining_percent: None,
            current_interval_status: Some(1),
            remains_time: Some(14_400_000),
            start_time: Some(1749008400000),
            end_time: Some(1749026400000),
            current_weekly_total_count: Some(15000),
            current_weekly_usage_count: Some(900),
            current_weekly_remaining_percent: None,
            current_weekly_status: Some(1),
            weekly_remains_time: Some(518_400_000),
            weekly_start_time: Some(1749888000000),
            weekly_end_time: Some(1750492800000),
        }
    }

    #[test]
    fn derive_usage_host_cn() {
        assert_eq!(
            derive_usage_host(Some("https://api.minimaxi.com/anthropic")),
            "https://www.minimaxi.com"
        );
    }

    #[test]
    fn derive_usage_host_international() {
        assert_eq!(
            derive_usage_host(Some("https://api.minimax.io/anthropic")),
            "https://api.minimax.io"
        );
    }

    #[test]
    fn derive_usage_host_default() {
        assert_eq!(derive_usage_host(None), "https://www.minimaxi.com");
    }

    #[test]
    fn deserialize_real_api_response() {
        let json = r#"{
            "base_resp": {"status_code": 0, "status_msg": "success"},
            "model_remains": [
                {
                    "start_time": 1780549200000,
                    "end_time": 1780567200000,
                    "remains_time": 2059592,
                    "current_interval_total_count": 0,
                    "current_interval_usage_count": 0,
                    "model_name": "general",
                    "current_weekly_total_count": 0,
                    "current_weekly_usage_count": 0,
                    "weekly_start_time": 1780272000000,
                    "weekly_end_time": 1780876800000,
                    "weekly_remains_time": 311659592,
                    "current_interval_status": 1,
                    "current_interval_remaining_percent": 99,
                    "current_weekly_status": 1,
                    "current_weekly_remaining_percent": 99
                },
                {
                    "start_time": 1780531200000,
                    "end_time": 1780617600000,
                    "remains_time": 52459592,
                    "current_interval_total_count": 0,
                    "current_interval_usage_count": 0,
                    "model_name": "video",
                    "current_weekly_total_count": 0,
                    "current_weekly_usage_count": 0,
                    "weekly_start_time": 1780272000000,
                    "weekly_end_time": 1780876800000,
                    "weekly_remains_time": 311659592,
                    "current_interval_status": 3,
                    "current_interval_remaining_percent": 100,
                    "current_weekly_status": 3,
                    "current_weekly_remaining_percent": 100
                }
            ]
        }"#;
        let resp: RemainsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.model_remains.len(), 2);

        let m = &resp.model_remains[0];
        assert_eq!(m.model_name, "general");
        assert_eq!(m.current_interval_total_count, 0);
        assert_eq!(m.current_interval_remaining_percent, Some(99.0));
    }

    #[test]
    fn five_hour_window_percent_based() {
        let w = five_hour_window(&model_general());
        assert_eq!(w.label, "general (5h)");
        assert!((w.used_pct - 1.0).abs() < 0.01); // 100 - 99
        assert_eq!(w.used, Some(0)); // no count available
        assert_eq!(w.limit, Some(0)); // percent-based
        assert!(w.resets_at_ms.is_some());
    }

    #[test]
    fn weekly_window_percent_based() {
        let w = weekly_window(&model_general());
        assert_eq!(w.label, "general (weekly)");
        assert!((w.used_pct - 1.0).abs() < 0.01); // 100 - 99
        assert!(w.limit.is_none()); // percent-based = no limit
        assert!(w.resets_at_ms.is_some());
    }

    #[test]
    fn five_hour_window_count_based() {
        let w = five_hour_window(&model_count_based());
        assert_eq!(w.label, "general (5h)");
        assert!((w.used_pct - 20.0).abs() < 0.01); // 300/1500
        assert_eq!(w.used, Some(300));
        assert_eq!(w.limit, Some(1500));
    }

    #[test]
    fn weekly_window_count_based() {
        let w = weekly_window(&model_count_based());
        assert_eq!(w.label, "general (weekly)");
        assert!((w.used_pct - 6.0).abs() < 0.01); // 900/15000
        assert_eq!(w.used, Some(900));
        assert_eq!(w.limit, Some(15000));
    }
}
