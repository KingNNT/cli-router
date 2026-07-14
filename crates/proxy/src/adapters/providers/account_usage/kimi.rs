//! Kimi (Moonshot) account usage adapter.
//!
//! Kimi runs two independent platforms whose API keys are NOT interchangeable,
//! and which expose usage information differently:
//!
//! - **Kimi For Coding** (`api.kimi.com/coding`, subscription, `sk-kimi-…` keys)
//!   exposes `GET {openai_base}/usages` returning quota windows: a weekly summary
//!   plus one or more rolling rate windows (e.g. 5-hour). No cash balance.
//! - **Open platform** (`api.moonshot.ai`, pay-as-you-go, `sk-…` keys) exposes
//!   `GET /v1/users/me/balance` returning a cash balance. No quota windows.
//!
//! The adapter picks the endpoint from the provider's configured base URLs so a
//! `.com` coding key never hits the `.ai` host (which would 401). When the
//! selected endpoint is absent (404), `fetch_usage` returns `None` (nothing to
//! show) instead of surfacing a spurious error.

use std::time::Duration;

use serde::Deserialize;

use crate::application::errors::ProxyError;
use crate::application::ports::AccountUsagePort;
use crate::domain::account_usage::{
    AccountUsageStatus, ProviderAccountUsage, UsageSubItem, UsageWindow,
};

const DEFAULT_BASE_URL: &str = "https://api.moonshot.ai/anthropic";
/// Matches the header the official Kimi CLI sends; some coding-plan deployments
/// gate the usage endpoint on it.
const KIMI_USER_AGENT: &str = "KimiCLI/1.6";

/// Which usage endpoint this provider is configured to talk to.
enum Endpoint {
    /// Kimi For Coding quota windows: `{openai_base}/usages`.
    CodingUsages(String),
    /// Open-platform cash balance: `{host}/v1/users/me/balance`.
    Balance(String),
}

/// Derive `scheme://host` (no path) from a URL, defaulting to the open platform.
fn derive_host(base_url: Option<&str>) -> String {
    let raw = base_url.unwrap_or(DEFAULT_BASE_URL);
    let after_scheme = raw.find("://").map(|i| i + 3).unwrap_or(0);
    let host_end = raw[after_scheme..]
        .find('/')
        .map(|i| after_scheme + i)
        .unwrap_or(raw.len());
    raw[..host_end].to_string()
}

/// Choose the usage endpoint from the provider's base URLs. The Kimi For Coding
/// plan is identified by its `api.kimi.com` host or a `/coding` path segment.
fn resolve_endpoint(base_url: Option<&str>, openai_base_url: Option<&str>) -> Endpoint {
    let signal = openai_base_url.or(base_url).unwrap_or("");
    let is_coding = signal.contains("kimi.com") || signal.contains("/coding");
    if is_coding {
        // The `/usages` endpoint lives under the OpenAI-style base (has `/v1`).
        // Fall back to `base_url + /v1` when only the Anthropic base is set.
        let base = match openai_base_url {
            Some(u) => u.trim_end_matches('/').to_string(),
            None => format!(
                "{}/v1",
                base_url.unwrap_or("https://api.kimi.com/coding").trim_end_matches('/')
            ),
        };
        Endpoint::CodingUsages(format!("{base}/usages"))
    } else {
        Endpoint::Balance(format!("{}/v1/users/me/balance", derive_host(base_url)))
    }
}

/// Kimi account usage adapter.
pub struct KimiAccountUsage {
    provider_name: String,
    auth_token: String,
    endpoint: Endpoint,
    agent: ureq::Agent,
}

impl KimiAccountUsage {
    pub fn new(
        provider_name: String,
        auth_token: String,
        base_url: Option<String>,
        openai_base_url: Option<String>,
    ) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_read(Duration::from_secs(10))
            .timeout_write(Duration::from_secs(10))
            .build();
        let endpoint = resolve_endpoint(base_url.as_deref(), openai_base_url.as_deref());
        Self {
            provider_name,
            auth_token,
            endpoint,
            agent,
        }
    }

    /// Build an authenticated GET request. Callers invoke `.call()` inline so the
    /// large `ureq::Error` never crosses a function boundary (clippy::result_large_err).
    fn request(&self, url: &str) -> ureq::Request {
        self.agent
            .get(url)
            .set("Authorization", &format!("Bearer {}", self.auth_token))
            .set("User-Agent", KIMI_USER_AGENT)
            .set("Accept", "application/json")
    }

    fn err(&self, message: String) -> Option<Result<ProviderAccountUsage, ProxyError>> {
        Some(Err(ProxyError::UpstreamUsage {
            provider: self.provider_name.clone(),
            message,
        }))
    }

    fn ok(&self, windows: Vec<UsageWindow>) -> Option<Result<ProviderAccountUsage, ProxyError>> {
        Some(Ok(ProviderAccountUsage {
            provider: self.provider_name.clone(),
            status: AccountUsageStatus::Available,
            plan: None,
            windows,
            model_usage: None,
        }))
    }
}

impl AccountUsagePort for KimiAccountUsage {
    fn fetch_usage(&self) -> Option<Result<ProviderAccountUsage, ProxyError>> {
        if self.auth_token.is_empty() {
            return None;
        }
        match &self.endpoint {
            Endpoint::CodingUsages(url) => self.fetch_coding_usages(url),
            Endpoint::Balance(url) => self.fetch_balance(url),
        }
    }
}

impl KimiAccountUsage {
    /// Kimi For Coding: `GET /usages` → weekly summary + rolling windows.
    fn fetch_coding_usages(&self, url: &str) -> Option<Result<ProviderAccountUsage, ProxyError>> {
        let resp = match self.request(url).call() {
            Ok(r) => r,
            // Some deployments expose the singular `/usage`; try it once.
            Err(ureq::Error::Status(404, _)) => {
                let fallback = url.trim_end_matches('s'); // /usages -> /usage
                match self.request(fallback).call() {
                    Ok(r) => r,
                    Err(ureq::Error::Status(404, _)) => return None,
                    Err(e) => return self.err(format!("usages API: {e}")),
                }
            }
            Err(e) => return self.err(format!("usages API: {e}")),
        };

        let payload: UsagesResponse = match resp.into_json() {
            Ok(p) => p,
            Err(e) => return self.err(format!("usages decode: {e}")),
        };
        self.ok(payload.into_windows())
    }

    /// Open platform: `GET /v1/users/me/balance` → cash balance snapshot.
    fn fetch_balance(&self, url: &str) -> Option<Result<ProviderAccountUsage, ProxyError>> {
        let resp = match self.request(url).call() {
            Ok(r) => r,
            Err(ureq::Error::Status(404, _)) => return None,
            Err(e) => return self.err(format!("balance API: {e}")),
        };
        let envelope: BalanceResponse = match resp.into_json() {
            Ok(e) => e,
            Err(e) => return self.err(format!("balance decode: {e}")),
        };
        let windows = envelope
            .data
            .as_ref()
            .map(|d| vec![balance_to_window(d)])
            .unwrap_or_default();
        self.ok(windows)
    }
}

// ---------------------------------------------------------------------------
// Coding-plan `/usages` response
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct UsagesResponse {
    /// Weekly quota summary.
    #[serde(default)]
    usage: Option<QuotaDetail>,
    /// Rolling rate windows (e.g. 5-hour).
    #[serde(default)]
    limits: Vec<LimitEntry>,
}

#[derive(Debug, Deserialize)]
struct LimitEntry {
    #[serde(default)]
    window: Option<Window>,
    #[serde(default)]
    detail: Option<QuotaDetail>,
}

#[derive(Debug, Deserialize)]
struct Window {
    #[serde(default)]
    duration: Option<i64>,
    #[serde(rename = "timeUnit", default)]
    time_unit: Option<String>,
}

/// Numeric fields arrive as JSON strings (e.g. `"100"`).
#[derive(Debug, Deserialize)]
struct QuotaDetail {
    #[serde(default)]
    limit: Option<String>,
    #[serde(default)]
    used: Option<String>,
    #[serde(default)]
    remaining: Option<String>,
    #[serde(rename = "resetTime", default)]
    reset_time: Option<String>,
}

fn parse_u64(s: Option<&String>) -> Option<u64> {
    s.and_then(|v| v.parse::<u64>().ok())
}

/// Parse an RFC 3339 timestamp to epoch milliseconds. `None` on failure.
fn parse_rfc3339_to_ms(s: Option<&String>) -> Option<i64> {
    s.and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
        .map(|dt| dt.timestamp_millis())
}

/// Human-readable window label from its duration, e.g. 300 MINUTE → `"5h"`.
fn window_label(w: Option<&Window>) -> String {
    let Some(w) = w else {
        return "Window".to_string();
    };
    let Some(dur) = w.duration else {
        return "Window".to_string();
    };
    let unit = w.time_unit.as_deref().unwrap_or("").to_ascii_uppercase();
    if unit.contains("MINUTE") {
        if dur >= 60 && dur % 60 == 0 {
            format!("{}h", dur / 60)
        } else {
            format!("{dur}m")
        }
    } else if unit.contains("HOUR") {
        format!("{dur}h")
    } else if unit.contains("DAY") {
        format!("{dur}d")
    } else {
        format!("{dur}")
    }
}

/// Build a quota window from a `{limit, used, remaining, resetTime}` detail.
/// `used` is inferred from `limit - remaining` when the API omits it.
fn quota_window(label: String, d: &QuotaDetail) -> UsageWindow {
    let limit = parse_u64(d.limit.as_ref());
    let remaining = parse_u64(d.remaining.as_ref());
    let used = parse_u64(d.used.as_ref())
        .or_else(|| match (limit, remaining) {
            (Some(l), Some(r)) => Some(l.saturating_sub(r)),
            _ => None,
        });
    let used_pct = match (used, limit) {
        (Some(u), Some(l)) if l > 0 => (u as f64 / l as f64) * 100.0,
        _ => 0.0,
    };
    let sub_items = match (used, limit) {
        (Some(u), Some(l)) => vec![UsageSubItem {
            label: format!("{u} used / {l}"),
            used: u,
        }],
        _ => Vec::new(),
    };
    UsageWindow {
        label,
        used_pct,
        used,
        limit,
        resets_at_ms: parse_rfc3339_to_ms(d.reset_time.as_ref()),
        sub_items,
        is_balance_info: false,
    }
}

impl UsagesResponse {
    fn into_windows(self) -> Vec<UsageWindow> {
        let mut windows = Vec::new();
        if let Some(weekly) = &self.usage {
            windows.push(quota_window("Weekly".to_string(), weekly));
        }
        for entry in &self.limits {
            if let Some(detail) = &entry.detail {
                windows.push(quota_window(window_label(entry.window.as_ref()), detail));
            }
        }
        windows
    }
}

// ---------------------------------------------------------------------------
// Open-platform `/v1/users/me/balance` response
// ---------------------------------------------------------------------------

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

    // --- endpoint selection -------------------------------------------------

    #[test]
    fn coding_base_url_selects_usages_endpoint() {
        let e = resolve_endpoint(Some("https://api.kimi.com/coding"), None);
        assert!(matches!(e, Endpoint::CodingUsages(u) if u == "https://api.kimi.com/coding/v1/usages"));
    }

    #[test]
    fn coding_openai_base_url_selects_usages_endpoint() {
        let e = resolve_endpoint(
            Some("https://api.kimi.com/coding"),
            Some("https://api.kimi.com/coding/v1"),
        );
        assert!(matches!(e, Endpoint::CodingUsages(u) if u == "https://api.kimi.com/coding/v1/usages"));
    }

    #[test]
    fn moonshot_base_url_selects_balance_endpoint() {
        let e = resolve_endpoint(Some("https://api.moonshot.ai/anthropic"), None);
        assert!(
            matches!(e, Endpoint::Balance(u) if u == "https://api.moonshot.ai/v1/users/me/balance")
        );
    }

    #[test]
    fn default_selects_balance_endpoint() {
        let e = resolve_endpoint(None, None);
        assert!(
            matches!(e, Endpoint::Balance(u) if u == "https://api.moonshot.ai/v1/users/me/balance")
        );
    }

    #[test]
    fn empty_token_returns_none() {
        let a = KimiAccountUsage::new("kimi".into(), String::new(), None, None);
        assert!(a.fetch_usage().is_none());
    }

    // --- coding `/usages` parsing (real response shape) ---------------------

    #[test]
    fn parses_real_usages_response_into_weekly_and_5h_windows() {
        let json = r#"{
            "user": {"userId":"x","membership":{"level":"LEVEL_BASIC"}},
            "usage": {"limit":"100","remaining":"100","resetTime":"2026-07-21T03:33:27.860191Z"},
            "limits": [
                {"window":{"duration":300,"timeUnit":"TIME_UNIT_MINUTE"},
                 "detail":{"limit":"100","used":"1","remaining":"99","resetTime":"2026-07-14T08:33:27.860191Z"}}
            ],
            "totalQuota":{"limit":"100","remaining":"99"}
        }"#;
        let payload: UsagesResponse = serde_json::from_str(json).unwrap();
        let windows = payload.into_windows();
        assert_eq!(windows.len(), 2);

        let weekly = &windows[0];
        assert_eq!(weekly.label, "Weekly");
        assert_eq!(weekly.limit, Some(100));
        assert_eq!(weekly.used, Some(0)); // 100 - 100 remaining
        assert!((weekly.used_pct - 0.0).abs() < 1e-6);
        assert!(weekly.resets_at_ms.is_some());
        assert!(!weekly.is_balance_info);

        let five_h = &windows[1];
        assert_eq!(five_h.label, "5h"); // 300 minutes
        assert_eq!(five_h.limit, Some(100));
        assert_eq!(five_h.used, Some(1));
        assert!((five_h.used_pct - 1.0).abs() < 1e-6);
        assert!(five_h.resets_at_ms.is_some());
    }

    #[test]
    fn window_label_maps_minutes_to_hours() {
        let w = Window {
            duration: Some(300),
            time_unit: Some("TIME_UNIT_MINUTE".into()),
        };
        assert_eq!(window_label(Some(&w)), "5h");
    }

    #[test]
    fn quota_window_infers_used_from_remaining() {
        let d = QuotaDetail {
            limit: Some("100".into()),
            used: None,
            remaining: Some("40".into()),
            reset_time: None,
        };
        let w = quota_window("Weekly".into(), &d);
        assert_eq!(w.used, Some(60));
        assert!((w.used_pct - 60.0).abs() < 1e-6);
    }

    // --- balance parsing (open platform) ------------------------------------

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
    }

    #[test]
    fn window_maps_available_and_is_balance_info() {
        let data = BalanceData {
            available_balance: 49.58,
            voucher_balance: 46.58,
            cash_balance: 3.0,
        };
        let w = balance_to_window(&data);
        assert!(w.is_balance_info);
        assert_eq!(w.label, "Balance");
        assert_eq!(w.sub_items[0].label, "available: 49.58");
    }
}
