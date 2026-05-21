//! Codex account usage adapter.
//!
//! Codex does not expose a documented standalone usage endpoint. The official
//! Codex CLI derives `/status` quota data from `x-codex-*` response headers
//! returned by the Codex backend. This module mirrors that header parsing for
//! cli-router's Account tab.

use crate::domain::account_usage::UsageWindow;

const MINUTES_PER_5_HOURS: i64 = 5 * 60;
const MINUTES_PER_DAY: i64 = 24 * 60;
const MINUTES_PER_WEEK: i64 = 7 * 24 * 60;
const MINUTES_PER_MONTH: i64 = 30 * 24 * 60;
const MINUTES_PER_YEAR: i64 = 365 * 24 * 60;

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
}
