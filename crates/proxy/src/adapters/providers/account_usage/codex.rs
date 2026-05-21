//! Codex account usage adapter.
//!
//! Codex does not expose a documented standalone usage endpoint. The official
//! Codex CLI derives `/status` quota data from `x-codex-*` response headers
//! returned by the Codex backend. This module mirrors that header parsing for
//! cli-router's Account tab.

use std::time::Duration;

use crate::application::errors::ProxyError;
use crate::application::ports::AccountUsagePort;
use crate::config::AuthConfig;
use crate::domain::account_usage::{AccountUsageStatus, ProviderAccountUsage, UsageWindow};

const DEFAULT_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
const RESPONSES_PATH: &str = "/responses";
const MINUTES_PER_5_HOURS: i64 = 5 * 60;
const MINUTES_PER_DAY: i64 = 24 * 60;
const MINUTES_PER_WEEK: i64 = 7 * 24 * 60;
const MINUTES_PER_MONTH: i64 = 30 * 24 * 60;
const MINUTES_PER_YEAR: i64 = 365 * 24 * 60;

/// Codex account usage adapter. Queries Codex backend headers for ChatGPT plan limits.
pub struct CodexAccountUsage {
    provider_name: String,
    base_url: String,
    auth: AuthConfig,
    agent: ureq::Agent,
}

impl CodexAccountUsage {
    pub fn new(provider_name: String, base_url: Option<String>, auth: AuthConfig) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_read(Duration::from_secs(10))
            .timeout_write(Duration::from_secs(10))
            .build();
        Self {
            provider_name,
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            auth,
            agent,
        }
    }
}

fn resolve_token(auth: &AuthConfig) -> Result<Option<String>, ProxyError> {
    match auth {
        AuthConfig::CodexAuto => crate::adapters::oauth::openai::read_auth_json()
            .map(|tokens| Some(tokens.access_token))
            .map_err(|e| ProxyError::UpstreamUsage {
                provider: "codex".to_string(),
                message: format!(
                    "CodexAuto requires ~/.codex/auth.json. Run `codex login` first. ({e})"
                ),
            }),
        AuthConfig::OpenAiOAuth { access_token, .. }
        | AuthConfig::Bearer {
            value: access_token,
        } => Ok(Some(access_token.clone())),
        AuthConfig::ApiKey { .. } | AuthConfig::Passthrough => Ok(None),
        AuthConfig::AnthropicOAuth { .. } => Ok(None),
    }
}

impl AccountUsagePort for CodexAccountUsage {
    fn fetch_usage(&self) -> Option<Result<ProviderAccountUsage, ProxyError>> {
        let token = match resolve_token(&self.auth) {
            Ok(Some(token)) => token,
            Ok(None) => return None,
            Err(e) => return Some(Err(e)),
        };

        if token.trim().is_empty() {
            return None;
        }

        let url = probe_url(&self.base_url);
        let body = codex_probe_body();
        let response = self
            .agent
            .post(&url)
            .set("Authorization", &format!("Bearer {token}"))
            .set("Accept", "text/event-stream")
            .set("Content-Type", "application/json")
            .send_json(body);

        match response {
            Ok(resp) => {
                let headers = ureq_headers_to_http(resp.headers_names(), &resp);
                match provider_usage_from_headers(&self.provider_name, &headers) {
                    Some(usage) => Some(Ok(usage)),
                    None => Some(Err(ProxyError::UpstreamUsage {
                        provider: self.provider_name.clone(),
                        message: "Codex response did not include rate-limit headers".to_string(),
                    })),
                }
            }
            Err(ureq::Error::Status(_, resp)) => {
                let headers = ureq_headers_to_http(resp.headers_names(), &resp);
                if let Some(usage) = provider_usage_from_headers(&self.provider_name, &headers) {
                    return Some(Ok(usage));
                }

                Some(Err(ProxyError::UpstreamUsage {
                    provider: self.provider_name.clone(),
                    message: "Codex probe failed and response did not include rate-limit headers"
                        .to_string(),
                }))
            }
            Err(e) => Some(Err(ProxyError::UpstreamUsage {
                provider: self.provider_name.clone(),
                message: format!("Codex probe: {e}"),
            })),
        }
    }
}

fn provider_usage_from_headers(
    provider_name: &str,
    headers: &http::HeaderMap,
) -> Option<ProviderAccountUsage> {
    let windows = parse_windows_from_headers(headers);
    if windows.is_empty() {
        return None;
    }

    Some(ProviderAccountUsage {
        provider: provider_name.to_string(),
        status: AccountUsageStatus::Available,
        plan: None,
        windows,
        model_usage: None,
    })
}

fn probe_url(base_url: &str) -> String {
    format!("{}{}", base_url.trim_end_matches('/'), RESPONSES_PATH)
}

fn codex_probe_body() -> serde_json::Value {
    serde_json::json!({
        "model": "gpt-5.4-mini",
        "instructions": "",
        "input": [{
            "type": "message",
            "role": "user",
            "content": ""
        }],
        "tools": [],
        "tool_choice": "auto",
        "parallel_tool_calls": false,
        "store": false,
        "stream": true
    })
}

fn ureq_headers_to_http(names: Vec<String>, response: &ureq::Response) -> http::HeaderMap {
    let mut headers = http::HeaderMap::new();
    for name in names {
        let Some(value) = response.header(&name) else {
            continue;
        };
        let Ok(header_name) = http::HeaderName::from_bytes(name.as_bytes()) else {
            continue;
        };
        let Ok(header_value) = http::HeaderValue::from_str(value) else {
            continue;
        };
        headers.insert(header_name, header_value);
    }
    headers
}

fn parse_windows_from_headers(headers: &http::HeaderMap) -> Vec<UsageWindow> {
    let mut windows = Vec::new();

    if let Some(window) = parse_window(
        headers,
        "x-codex-primary-used-percent",
        "x-codex-primary-window-minutes",
        "x-codex-primary-reset-at",
        false,
    ) {
        windows.push(window);
    }

    if let Some(window) = parse_window(
        headers,
        "x-codex-secondary-used-percent",
        "x-codex-secondary-window-minutes",
        "x-codex-secondary-reset-at",
        true,
    ) {
        windows.push(window);
    }

    windows
}

fn parse_window(
    headers: &http::HeaderMap,
    used_percent_header: &str,
    window_minutes_header: &str,
    reset_at_header: &str,
    is_secondary: bool,
) -> Option<UsageWindow> {
    let used_pct = parse_header_f64(headers, used_percent_header)?;
    let window_minutes = parse_header_i64(headers, window_minutes_header);
    let reset_at_ms = parse_header_i64(headers, reset_at_header).map(|seconds| seconds * 1000);

    Some(UsageWindow {
        label: limit_label(window_minutes, is_secondary),
        used_pct,
        used: None,
        limit: None,
        resets_at_ms: reset_at_ms,
        sub_items: vec![],
    })
}

fn parse_header_f64(headers: &http::HeaderMap, name: &str) -> Option<f64> {
    headers
        .get(name)?
        .to_str()
        .ok()?
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
}

fn parse_header_i64(headers: &http::HeaderMap, name: &str) -> Option<i64> {
    headers.get(name)?.to_str().ok()?.parse::<i64>().ok()
}

fn limit_label(window_minutes: Option<i64>, is_secondary: bool) -> String {
    match window_minutes.and_then(limit_duration_label) {
        Some(duration) => format!("{duration} limit"),
        None if is_secondary => "Secondary usage limit".to_string(),
        None => "Usage limit".to_string(),
    }
}

fn limit_duration_label(window_minutes: i64) -> Option<&'static str> {
    let minutes = window_minutes.max(0);
    if is_approximate_window(minutes, MINUTES_PER_5_HOURS) {
        Some("5h")
    } else if is_approximate_window(minutes, MINUTES_PER_DAY) {
        Some("Daily")
    } else if is_approximate_window(minutes, MINUTES_PER_WEEK) {
        Some("Weekly")
    } else if is_approximate_window(minutes, MINUTES_PER_MONTH) {
        Some("Monthly")
    } else if is_approximate_window(minutes, MINUTES_PER_YEAR) {
        Some("Annual")
    } else {
        None
    }
}

fn is_approximate_window(minutes: i64, expected_minutes: i64) -> bool {
    let minutes = minutes as f64;
    let expected = expected_minutes as f64;
    minutes >= expected * 0.95 && minutes <= expected * 1.05
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderValue;

    #[test]
    fn parses_primary_5h_window() {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            "x-codex-primary-used-percent",
            HeaderValue::from_static("12.5"),
        );
        headers.insert(
            "x-codex-primary-window-minutes",
            HeaderValue::from_static("300"),
        );
        headers.insert(
            "x-codex-primary-reset-at",
            HeaderValue::from_static("1779333600"),
        );

        let windows = parse_windows_from_headers(&headers);

        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].label, "5h limit");
        assert_eq!(windows[0].used_pct, 12.5);
        assert_eq!(windows[0].resets_at_ms, Some(1_779_333_600_000));
        assert_eq!(windows[0].used, None);
        assert_eq!(windows[0].limit, None);
    }

    #[test]
    fn parses_secondary_weekly_window() {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            "x-codex-secondary-used-percent",
            HeaderValue::from_static("81"),
        );
        headers.insert(
            "x-codex-secondary-window-minutes",
            HeaderValue::from_static("10080"),
        );

        let windows = parse_windows_from_headers(&headers);

        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].label, "Weekly limit");
        assert_eq!(windows[0].used_pct, 81.0);
        assert_eq!(windows[0].resets_at_ms, None);
    }

    #[test]
    fn empty_headers_return_no_windows() {
        let headers = http::HeaderMap::new();

        let windows = parse_windows_from_headers(&headers);

        assert!(windows.is_empty());
    }

    #[test]
    fn malformed_optional_fields_do_not_drop_window() {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            "x-codex-primary-used-percent",
            HeaderValue::from_static("42"),
        );
        headers.insert(
            "x-codex-primary-window-minutes",
            HeaderValue::from_static("not-a-number"),
        );
        headers.insert(
            "x-codex-primary-reset-at",
            HeaderValue::from_static("also-bad"),
        );

        let windows = parse_windows_from_headers(&headers);

        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].label, "Usage limit");
        assert_eq!(windows[0].used_pct, 42.0);
        assert_eq!(windows[0].resets_at_ms, None);
    }

    #[test]
    fn malformed_required_percent_drops_window() {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            "x-codex-primary-used-percent",
            HeaderValue::from_static("NaN"),
        );
        headers.insert(
            "x-codex-primary-window-minutes",
            HeaderValue::from_static("300"),
        );

        let windows = parse_windows_from_headers(&headers);

        assert!(windows.is_empty());
    }

    #[test]
    fn bearer_auth_resolves_to_token() {
        let auth = crate::config::AuthConfig::Bearer {
            value: "token-123".to_string(),
        };

        let token = resolve_token(&auth).expect("bearer auth should resolve");

        assert_eq!(token, Some("token-123".to_string()));
    }

    #[test]
    fn openai_oauth_auth_resolves_to_access_token() {
        let auth = crate::config::AuthConfig::OpenAiOAuth {
            access_token: "access-123".to_string(),
            refresh_token: "refresh-123".to_string(),
            expires_at_ms: 1_779_333_600_000,
        };

        let token = resolve_token(&auth).expect("oauth auth should resolve");

        assert_eq!(token, Some("access-123".to_string()));
    }

    #[test]
    fn api_key_auth_is_not_supported_for_chatgpt_plan_limits() {
        let auth = crate::config::AuthConfig::ApiKey {
            value: "sk-test".to_string(),
        };

        let token = resolve_token(&auth).expect("api key should be cleanly unsupported");

        assert_eq!(token, None);
    }

    #[test]
    fn passthrough_auth_is_not_supported_for_account_refresh() {
        let auth = crate::config::AuthConfig::Passthrough;

        let token = resolve_token(&auth).expect("passthrough should be cleanly unsupported");

        assert_eq!(token, None);
    }

    #[test]
    fn codex_account_usage_new_defaults_base_url() {
        let adapter = CodexAccountUsage::new(
            "codex".to_string(),
            None,
            crate::config::AuthConfig::Bearer {
                value: "token-123".to_string(),
            },
        );

        assert_eq!(adapter.base_url, DEFAULT_BASE_URL);
    }

    #[test]
    fn response_headers_map_to_available_provider_usage() {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            "x-codex-primary-used-percent",
            HeaderValue::from_static("10"),
        );
        headers.insert(
            "x-codex-primary-window-minutes",
            HeaderValue::from_static("300"),
        );
        headers.insert(
            "x-codex-secondary-used-percent",
            HeaderValue::from_static("70"),
        );
        headers.insert(
            "x-codex-secondary-window-minutes",
            HeaderValue::from_static("10080"),
        );

        let usage = provider_usage_from_headers("codex-main", &headers)
            .expect("headers should produce usage");

        assert_eq!(usage.provider, "codex-main");
        assert_eq!(usage.status, AccountUsageStatus::Available);
        assert_eq!(usage.windows.len(), 2);
        assert_eq!(usage.windows[0].label, "5h limit");
        assert_eq!(usage.windows[0].used_pct, 10.0);
        assert_eq!(usage.windows[1].label, "Weekly limit");
        assert_eq!(usage.windows[1].used_pct, 70.0);
        assert!(usage.model_usage.is_none());
    }

    #[test]
    fn response_headers_without_limits_return_none() {
        let headers = http::HeaderMap::new();

        let usage = provider_usage_from_headers("codex-main", &headers);

        assert!(usage.is_none());
    }

    #[test]
    fn probe_url_joins_base_url_and_responses_path() {
        assert_eq!(
            probe_url("https://chatgpt.com/backend-api/codex"),
            "https://chatgpt.com/backend-api/codex/responses"
        );
        assert_eq!(
            probe_url("https://chatgpt.com/backend-api/codex/"),
            "https://chatgpt.com/backend-api/codex/responses"
        );
    }

    #[test]
    fn probe_body_is_minimal_streaming_responses_request() {
        let body = codex_probe_body();

        assert_eq!(body["model"], "gpt-5.4-mini");
        assert_eq!(body["instructions"], "");
        assert_eq!(body["stream"], true);
        assert_eq!(body["store"], false);
        assert_eq!(body["tools"].as_array().unwrap().len(), 0);

        let input = body["input"].as_array().unwrap();
        assert_eq!(input.len(), 1);
        assert_eq!(input[0]["type"], "message");
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[0]["content"], "");
    }
}
