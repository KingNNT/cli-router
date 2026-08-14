//! OpenCode Go account usage adapter.
//!
//! `GET {openai_base}/usage` returns the Go subscription's three quota windows
//! (rolling 5h, weekly, monthly) as **percentages** — no dollar amounts and no
//! absolute used/limit counts, so the windows carry `used_pct` only.
//!
//! The endpoint is undocumented: it is not in the Go docs and the opencode CLI
//! never calls it (the CLI only learns about limits reactively, from a 429
//! `GoUsageLimitError` body). It answers `Authorization: Bearer <key>` and
//! rejects `x-api-key` with 401, which matches what `api_key` auth produces on
//! the OpenAI path. A 404 is treated as "nothing to show" rather than an error
//! so the account tab degrades quietly if OpenCode retires it.

use std::time::Duration;

use serde::Deserialize;

use crate::application::errors::ProxyError;
use crate::application::ports::AccountUsagePort;
use crate::domain::account_usage::{AccountUsageStatus, ProviderAccountUsage, UsageWindow};

const DEFAULT_USAGE_URL: &str = "https://opencode.ai/zen/go/v1/usage";

/// Derive the usage endpoint from the provider's configured base URLs. The
/// OpenAI base already carries the `/v1` segment; the Anthropic base does not.
fn resolve_usage_url(anthropic_base_url: Option<&str>, openai_base_url: Option<&str>) -> String {
    if let Some(base) = openai_base_url {
        return format!("{}/usage", base.trim_end_matches('/'));
    }
    match anthropic_base_url {
        Some(base) => format!("{}/v1/usage", base.trim_end_matches('/')),
        None => DEFAULT_USAGE_URL.to_string(),
    }
}

/// OpenCode Go account usage adapter.
pub struct OpencodeGoAccountUsage {
    provider_name: String,
    auth_token: String,
    usage_url: String,
    agent: ureq::Agent,
}

impl OpencodeGoAccountUsage {
    pub fn new(
        provider_name: String,
        auth_token: String,
        anthropic_base_url: Option<String>,
        openai_base_url: Option<String>,
    ) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_read(Duration::from_secs(10))
            .timeout_write(Duration::from_secs(10))
            .build();
        let usage_url =
            resolve_usage_url(anthropic_base_url.as_deref(), openai_base_url.as_deref());
        Self {
            provider_name,
            auth_token,
            usage_url,
            agent,
        }
    }
}

impl AccountUsagePort for OpencodeGoAccountUsage {
    fn fetch_usage(&self) -> Option<Result<ProviderAccountUsage, ProxyError>> {
        // No key configured → nothing to query.
        if self.auth_token.is_empty() {
            return None;
        }

        let resp = match self
            .agent
            .get(&self.usage_url)
            .set("Authorization", &format!("Bearer {}", self.auth_token))
            .set("Accept", "application/json")
            .call()
        {
            Ok(r) => r,
            // The endpoint is undocumented; if it disappears, show nothing
            // rather than an error row.
            Err(ureq::Error::Status(404, _)) => return None,
            Err(e) => {
                return Some(Err(ProxyError::UpstreamUsage {
                    provider: self.provider_name.clone(),
                    message: format!("usage API: {e}"),
                }));
            }
        };

        let payload: UsageResponse = match resp.into_json() {
            Ok(p) => p,
            Err(e) => {
                return Some(Err(ProxyError::UpstreamUsage {
                    provider: self.provider_name.clone(),
                    message: format!("usage decode: {e}"),
                }));
            }
        };

        Some(Ok(ProviderAccountUsage {
            provider: self.provider_name.clone(),
            status: AccountUsageStatus::Available,
            plan: Some("Go".to_string()),
            windows: payload
                .usage
                .map(UsageWindows::into_windows)
                .unwrap_or_default(),
            model_usage: None,
        }))
    }
}

// ---------------------------------------------------------------------------
// `/usage` response
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct UsageResponse {
    #[serde(default)]
    usage: Option<UsageWindows>,
}

#[derive(Debug, Deserialize)]
struct UsageWindows {
    #[serde(default)]
    rolling: Option<WindowDetail>,
    #[serde(default)]
    weekly: Option<WindowDetail>,
    #[serde(default)]
    monthly: Option<WindowDetail>,
}

#[derive(Debug, Deserialize)]
struct WindowDetail {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    percent: Option<f64>,
    #[serde(rename = "resetsAt", default)]
    resets_at: Option<String>,
}

/// Build a quota window. A non-`ok` status is folded into the label (the TUI
/// renders sub-items as `label: count`, which reads wrong for a status string).
fn to_window(label: &str, d: &WindowDetail) -> UsageWindow {
    let label = match d.status.as_deref() {
        Some(s) if !s.eq_ignore_ascii_case("ok") && !s.is_empty() => format!("{label} ({s})"),
        _ => label.to_string(),
    };
    UsageWindow {
        label,
        used_pct: d.percent.unwrap_or(0.0).clamp(0.0, 100.0),
        // The API reports percentages only — no dollar or token counts.
        used: None,
        limit: None,
        resets_at_ms: d
            .resets_at
            .as_deref()
            .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
            .map(|dt| dt.timestamp_millis()),
        sub_items: Vec::new(),
        is_balance_info: false,
    }
}

impl UsageWindows {
    fn into_windows(self) -> Vec<UsageWindow> {
        [
            ("Rolling (5h)", self.rolling),
            ("Weekly", self.weekly),
            ("Monthly", self.monthly),
        ]
        .iter()
        .filter_map(|(label, detail)| detail.as_ref().map(|d| to_window(label, d)))
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- endpoint resolution -----------------------------------------------

    #[test]
    fn preset_base_urls_resolve_to_go_usage_endpoint() {
        let url = resolve_usage_url(
            Some("https://opencode.ai/zen/go"),
            Some("https://opencode.ai/zen/go/v1"),
        );
        assert_eq!(url, "https://opencode.ai/zen/go/v1/usage");
    }

    #[test]
    fn anthropic_base_only_appends_v1_segment() {
        let url = resolve_usage_url(Some("https://opencode.ai/zen/go/"), None);
        assert_eq!(url, "https://opencode.ai/zen/go/v1/usage");
    }

    #[test]
    fn no_base_urls_fall_back_to_default() {
        assert_eq!(resolve_usage_url(None, None), DEFAULT_USAGE_URL);
    }

    #[test]
    fn empty_token_returns_none() {
        let a = OpencodeGoAccountUsage::new("opencode_go".into(), String::new(), None, None);
        assert!(a.fetch_usage().is_none());
    }

    // --- response parsing (real payload shape) ------------------------------

    #[test]
    fn parses_real_usage_response_into_three_windows() {
        let json = r#"{"usage":{
            "rolling":{"status":"ok","percent":0,"resetsAt":"2026-08-15T01:48:43.159Z"},
            "weekly":{"status":"ok","percent":80,"resetsAt":"2026-08-17T00:00:00.159Z"},
            "monthly":{"status":"ok","percent":40,"resetsAt":"2026-09-14T06:55:27.159Z"}
        }}"#;
        let payload: UsageResponse = serde_json::from_str(json).unwrap();
        let windows = payload.usage.unwrap().into_windows();
        assert_eq!(windows.len(), 3);

        assert_eq!(windows[0].label, "Rolling (5h)");
        assert!((windows[0].used_pct - 0.0).abs() < 1e-6);
        assert_eq!(
            windows[0].resets_at_ms,
            Some(
                chrono::DateTime::parse_from_rfc3339("2026-08-15T01:48:43.159Z")
                    .unwrap()
                    .timestamp_millis()
            )
        );

        assert_eq!(windows[1].label, "Weekly");
        assert!((windows[1].used_pct - 80.0).abs() < 1e-6);

        assert_eq!(windows[2].label, "Monthly");
        assert!((windows[2].used_pct - 40.0).abs() < 1e-6);
    }

    #[test]
    fn windows_carry_no_absolute_used_or_limit() {
        let d = WindowDetail {
            status: Some("ok".into()),
            percent: Some(40.0),
            resets_at: None,
        };
        let w = to_window("Monthly", &d);
        assert_eq!(w.used, None);
        assert_eq!(w.limit, None);
        assert!(!w.is_balance_info);
        assert!(w.sub_items.is_empty());
    }

    #[test]
    fn non_ok_status_is_appended_to_the_label() {
        let d = WindowDetail {
            status: Some("limited".into()),
            percent: Some(100.0),
            resets_at: None,
        };
        assert_eq!(to_window("Weekly", &d).label, "Weekly (limited)");
    }

    #[test]
    fn percent_is_clamped_to_the_bar_range() {
        let d = WindowDetail {
            status: None,
            percent: Some(150.0),
            resets_at: None,
        };
        assert!((to_window("Weekly", &d).used_pct - 100.0).abs() < 1e-6);
    }

    #[test]
    fn missing_window_is_skipped() {
        let json = r#"{"usage":{"weekly":{"status":"ok","percent":12,"resetsAt":null}}}"#;
        let payload: UsageResponse = serde_json::from_str(json).unwrap();
        let windows = payload.usage.unwrap().into_windows();
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].label, "Weekly");
        assert_eq!(windows[0].resets_at_ms, None);
    }
}
