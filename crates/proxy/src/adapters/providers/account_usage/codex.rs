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

impl CodexAccountUsage {
    /// Send the probe request and return the parsed usage or error.
    fn probe_once(&self, token: &str) -> ProbeOutcome {
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
                    Some(usage) => ProbeOutcome::Success(usage),
                    None => ProbeOutcome::NoRateLimitHeaders,
                }
            }
            Err(ureq::Error::Status(_, resp)) => {
                let headers = ureq_headers_to_http(resp.headers_names(), &resp);
                if let Some(usage) = provider_usage_from_headers(&self.provider_name, &headers) {
                    return ProbeOutcome::Success(usage);
                }
                ProbeOutcome::HttpErrorNoHeaders
            }
            Err(e) => ProbeOutcome::TransportError(e),
        }
    }
}

/// Outcome of a single probe attempt.
enum ProbeOutcome {
    Success(ProviderAccountUsage),
    /// 200 OK but no x-codex-* headers — retrying won't help.
    NoRateLimitHeaders,
    /// HTTP error with no parseable rate-limit headers — worth retrying with a fresh token.
    HttpErrorNoHeaders,
    /// Network/transport error — retrying won't help.
    TransportError(ureq::Error),
}

/// Whether a probe outcome is worth retrying on a transient failure.
///
/// `HttpErrorNoHeaders` covers 4xx/5xx responses without rate-limit headers
/// (could be a backend hiccup, a 401 from a rotated token, or a 502 from a
/// Cloudflare blip). `TransportError` covers network-level failures (DNS,
/// connection reset, timeout). The 200 OK cases are deterministic and
/// should not be retried.
fn is_retriable(outcome: &ProbeOutcome) -> bool {
    matches!(
        outcome,
        ProbeOutcome::HttpErrorNoHeaders | ProbeOutcome::TransportError(_)
    )
}

/// A single iteration of a retry loop.
enum RetryStep<T> {
    /// Stop and return this value (success or non-retriable failure).
    Done(T),
    /// Keep trying — sleep for the next backoff, then re-run.
    Retry(T),
}

/// Run `f` up to `max_attempts` times, sleeping for `backoffs[i]` between
/// attempt `i` and attempt `i + 1`.
///
/// `f` receives the 0-based attempt index. If it returns `Done`, the value
/// is returned immediately. If it returns `Retry`, the loop sleeps for the
/// matching backoff (or skips the sleep on the last attempt) and re-runs.
/// When the budget is exhausted, the last `Retry` value is returned.
fn run_with_retry<T, F>(max_attempts: usize, backoffs: &[Duration], mut f: F) -> T
where
    F: FnMut(usize) -> RetryStep<T>,
{
    debug_assert!(max_attempts >= 1, "max_attempts must be at least 1");

    let mut last: Option<T> = None;
    for attempt in 0..max_attempts {
        let step = f(attempt);
        match step {
            RetryStep::Done(value) => return value,
            RetryStep::Retry(value) => {
                last = Some(value);
                if let Some(backoff) = backoffs.get(attempt) {
                    std::thread::sleep(*backoff);
                }
            }
        }
    }
    last.expect("max_attempts >= 1 guarantees at least one iteration")
}

/// Maximum number of probe attempts (initial + retries) before giving up.
const MAX_PROBE_ATTEMPTS: usize = 3;
/// Backoff between probe attempts. The probe is user-facing (TUI refresh),
/// so total worst-case wait is ~2s — fast enough to feel responsive while
/// giving the Codex backend room to recover from a transient blip.
const PROBE_BACKOFFS: [Duration; MAX_PROBE_ATTEMPTS - 1] =
    [Duration::from_millis(500), Duration::from_millis(1500)];

impl AccountUsagePort for CodexAccountUsage {
    fn fetch_usage(&self) -> Option<Result<ProviderAccountUsage, ProxyError>> {
        let mut token = match resolve_token(&self.auth) {
            Ok(Some(token)) => token,
            Ok(None) => return None,
            Err(e) => return Some(Err(e)),
        };

        if token.trim().is_empty() {
            return None;
        }

        // Retry transient backend failures (network blips, Cloudflare
        // hiccups, 5xx without rate-limit headers) with backoff before
        // giving up. The token is the same on every attempt; if it is
        // genuinely stale, the CodexAuto branch below re-reads it.
        let mut outcome = run_with_retry(MAX_PROBE_ATTEMPTS, &PROBE_BACKOFFS, |attempt| {
            if attempt > 0 {
                tracing::info!(
                    provider = %self.provider_name,
                    attempt = attempt + 1,
                    max_attempts = MAX_PROBE_ATTEMPTS,
                    "Codex account probe retrying after transient failure"
                );
            }
            let result = self.probe_once(&token);
            if is_retriable(&result) {
                RetryStep::Retry(result)
            } else {
                RetryStep::Done(result)
            }
        });

        // If all retries failed with HttpErrorNoHeaders and the user is
        // using CodexAuto, the token may be stale — re-read ~/.codex/auth.json
        // and probe one last time. OpenAiOAuth tokens are managed by the
        // background refresh task, so we do not touch them here.
        if matches!(outcome, ProbeOutcome::HttpErrorNoHeaders)
            && matches!(self.auth, AuthConfig::CodexAuto)
        {
            tracing::info!(
                provider = %self.provider_name,
                "Codex account probe still failing; re-reading ~/.codex/auth.json and retrying"
            );
            if let Ok(Some(fresh_token)) = resolve_token(&self.auth) {
                if fresh_token != token {
                    token = fresh_token;
                    outcome = self.probe_once(&token);
                }
            }
        }

        Some(match outcome {
            ProbeOutcome::Success(usage) => Ok(usage),
            ProbeOutcome::NoRateLimitHeaders => Err(ProxyError::UpstreamUsage {
                provider: self.provider_name.clone(),
                message: "Codex response did not include rate-limit headers".to_string(),
            }),
            ProbeOutcome::HttpErrorNoHeaders => Err(ProxyError::UpstreamUsage {
                provider: self.provider_name.clone(),
                message: "Codex probe failed and response did not include rate-limit headers"
                    .to_string(),
            }),
            ProbeOutcome::TransportError(e) => Err(ProxyError::UpstreamUsage {
                provider: self.provider_name.clone(),
                message: format!("Codex probe: {e}"),
            }),
        })
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
        is_balance_info: false,
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

    // -- Retry helper tests --

    #[test]
    fn http_error_no_headers_is_retriable() {
        assert!(is_retriable(&ProbeOutcome::HttpErrorNoHeaders));
    }

    #[test]
    fn success_is_not_retriable() {
        let usage = ProviderAccountUsage {
            provider: "codex".into(),
            status: AccountUsageStatus::Available,
            plan: None,
            windows: vec![],
            model_usage: None,
        };
        assert!(!is_retriable(&ProbeOutcome::Success(usage)));
    }

    #[test]
    fn no_rate_limit_headers_is_not_retriable() {
        assert!(!is_retriable(&ProbeOutcome::NoRateLimitHeaders));
    }

    #[test]
    fn retry_returns_immediately_on_done() {
        let mut calls = 0;
        let result = run_with_retry(3, &[], |attempt| {
            calls += 1;
            assert_eq!(attempt, 0);
            RetryStep::Done("ok")
        });
        assert_eq!(result, "ok");
        assert_eq!(calls, 1);
    }

    #[test]
    fn retry_continues_until_done() {
        let backoffs = [Duration::from_millis(0), Duration::from_millis(0)];
        let mut calls = 0;
        let result = run_with_retry(3, &backoffs, |attempt| {
            calls += 1;
            if attempt < 2 {
                RetryStep::Retry(format!("attempt {attempt}"))
            } else {
                RetryStep::Done(format!("final {attempt}"))
            }
        });
        assert_eq!(result, "final 2");
        assert_eq!(calls, 3);
    }

    #[test]
    fn retry_returns_last_value_when_exhausted() {
        let backoffs = [Duration::from_millis(0), Duration::from_millis(0)];
        let mut calls = 0;
        let result = run_with_retry(3, &backoffs, |attempt| {
            calls += 1;
            RetryStep::Retry(format!("attempt {attempt}"))
        });
        assert_eq!(result, "attempt 2");
        assert_eq!(calls, 3);
    }

    #[test]
    fn retry_stops_early_on_non_retriable_done() {
        let backoffs = [Duration::from_millis(0), Duration::from_millis(0)];
        let mut calls = 0;
        let result = run_with_retry(3, &backoffs, |_| {
            calls += 1;
            if calls == 1 {
                RetryStep::Retry("first")
            } else {
                RetryStep::Done("second")
            }
        });
        assert_eq!(result, "second");
        assert_eq!(calls, 2);
    }
}
