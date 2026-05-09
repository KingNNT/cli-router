//! Wire types for the proxy admin API. Pure data + serde, zero logic — both
//! the proxy daemon (server side) and the upcoming `proxy-tui` (client side)
//! depend on this crate so the on-the-wire shape stays in sync.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// `GET /admin/status`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusResponse {
    /// Epoch milliseconds when the daemon started.
    pub started_at_ms: i64,
    /// Seconds since `started_at_ms`.
    pub uptime_seconds: u64,
    /// Total rows in the `requests` table.
    pub total_requests: u64,
    /// Per-provider counts. Key is the provider name as written to the DB
    /// (currently `"router"`, `"anthropic"`, `"zai"` — Phase 1 limitation).
    pub requests_by_provider: BTreeMap<String, u64>,
    /// Per-status counts (`started`, `completed`, `errored`).
    pub requests_by_status: BTreeMap<String, u64>,
    /// Current affinity (sticky-auth) configuration.
    pub affinity: AffinityStatus,
    /// Count of requests that involved cross-format translation and completed.
    pub translations_completed: u64,
    /// Count of requests that involved cross-format translation and errored.
    pub translations_failed: u64,
    /// Per-direction translation counts (e.g. `"anthropic→openai"` → 3).
    pub translation_directions: BTreeMap<String, u64>,
}

/// Affinity (sticky-auth) status reported by `GET /admin/status`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AffinityStatus {
    pub enabled: bool,
    pub headers: Vec<String>,
}

/// Affinity config for conversation-affinity hashing (editable via `ConfigPayload`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AffinityPayload {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_affinity_headers")]
    pub headers: Vec<String>,
}

impl Default for AffinityPayload {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            headers: default_affinity_headers(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_affinity_headers() -> Vec<String> {
    vec![
        "x-session-id".into(),
        "x-request-id".into(),
        "x-api-key".into(),
    ]
}

/// A single quota rule (editable via `ConfigPayload`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QuotaPayload {
    pub provider: String,
    pub window: String,
    #[serde(default)]
    pub max_requests: Option<u64>,
    #[serde(default)]
    pub max_input_tokens: Option<u64>,
    #[serde(default)]
    pub max_output_tokens: Option<u64>,
    #[serde(default = "default_warn_pct")]
    pub warn_pct: u8,
}

fn default_warn_pct() -> u8 {
    80
}

/// `GET /admin/config` and `PUT /admin/config` body.
///
/// Mirrors `proxy::config::Config` but lives outside the proxy crate so the
/// TUI can deserialise it without depending on the daemon.
///
/// Auth secrets are NOT redacted — the admin API is bound to `127.0.0.1` and
/// presumed trusted. If you expose it beyond loopback, add a bearer token in
/// front (Phase 2 does not).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigPayload {
    pub port: u16,
    #[serde(default)]
    pub providers: Vec<ProviderPayload>,
    #[serde(default)]
    pub routing: Vec<RoutingRulePayload>,
    #[serde(default)]
    pub quota: Vec<QuotaPayload>,
    #[serde(default)]
    pub affinity: AffinityPayload,
    #[serde(default)]
    pub proxy_db: Option<String>,
    #[serde(default)]
    pub pricing_db: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderPayload {
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub auth: AuthPayload,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub openai_base_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthPayload {
    #[default]
    Passthrough,
    ApiKey {
        value: String,
    },
    Bearer {
        value: String,
    },
    #[serde(rename = "anthropic_oauth")]
    AnthropicOAuth {
        access_token: String,
        refresh_token: String,
        expires_at_ms: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RoutingStrategyPayload {
    #[default]
    Failover,
    RoundRobin,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingRulePayload {
    pub r#match: MatchPayload,
    pub provider: String,
    #[serde(default)]
    pub fallback: Vec<String>,
    #[serde(default)]
    pub strategy: RoutingStrategyPayload,
    #[serde(default)]
    pub priority: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchPayload {
    #[serde(default)]
    pub model: Option<String>,
}

/// `GET /admin/requests/recent?limit=N&offset=M`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentRequestsResponse {
    pub items: Vec<RecentRequestItem>,
    pub total_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentRequestItem {
    pub id: String,
    pub started_at_ms: i64,
    pub finished_at_ms: Option<i64>,
    pub provider: String,
    pub model: String,
    pub status: String,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_creation_tokens: Option<i64>,
    pub cost_usd: Option<f64>,
    pub error_message: Option<String>,
    /// Cross-format translation direction, e.g. `"anthropic→openai"`.
    /// `None` when the request was a passthrough (no translation).
    #[serde(default)]
    pub translation_direction: Option<String>,
}

/// `POST /admin/providers/:name/test` request body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestProviderRequest {
    /// Model id to ping with. The TUI reads this from the provider config
    /// (e.g. `"claude-3-5-haiku-latest"` for an Anthropic provider, `"glm-4.5-air"`
    /// for Z.ai). Server returns 400 if missing.
    pub model: String,
}

/// `POST /admin/providers/:name/test`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestProviderResponse {
    pub success: bool,
    pub status_code: Option<u16>,
    pub latency_ms: u64,
    pub error: Option<String>,
}

/// Minimal error envelope returned on failure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiError {
    pub error: String,
}

// ---- OAuth (Anthropic, paste flow) ----

/// `POST /admin/oauth/anthropic/start` request body — names the provider
/// whose auth will be replaced once the flow completes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartOAuthRequest {
    pub provider_name: String,
}

/// `POST /admin/oauth/anthropic/start` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartOAuthResponse {
    /// Open this in a browser. Anthropic redirects to a manual-callback
    /// page that displays `code#state` for the user to copy.
    pub authorization_url: String,
    /// Echoed in `CompleteOAuthRequest` so the daemon can match the
    /// pasted code back to the right PKCE verifier.
    pub state_id: String,
}

/// `POST /admin/oauth/anthropic/complete` request body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompleteOAuthRequest {
    pub state_id: String,
    /// The `code` value from the redirect URL (everything before `#`).
    pub code: String,
    pub provider_name: String,
}

/// `POST /admin/oauth/anthropic/complete` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompleteOAuthResponse {
    pub success: bool,
    pub error: Option<String>,
    /// Updated config snapshot if `success` — the named provider now has
    /// `auth = bearer` with the access token from Anthropic.
    pub config: Option<ConfigPayload>,
}

/// `GET /admin/usage/summary?from=<ms>&to=<ms>`
///
/// Server-side aggregated view of the proxy's request log. `from`/`to` are
/// inclusive epoch milliseconds. Both arrays are empty when no rows match.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageSummaryResponse {
    pub from_ms: i64,
    pub to_ms: i64,
    /// One row per local-tz day. Sorted by `date` ascending.
    pub daily: Vec<DailyUsageRow>,
    /// One row per (model, provider) pair. Sorted by `cost_usd` descending.
    pub models: Vec<ModelUsageRow>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DailyUsageRow {
    /// Local-tz date as `YYYY-MM-DD`.
    pub date: String,
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelUsageRow {
    pub model: String,
    /// Value from `requests.provider` as logged — may be `"anthropic"`,
    /// `"zai"`, or `"router"` per the existing `StatusResponse` note.
    pub provider: String,
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cost_usd: f64,
}

// ---- Quota status ----

/// `GET /admin/quota/status`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaStatusListDto {
    pub quotas: Vec<QuotaStatusDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaStatusDto {
    pub provider: String,
    pub window: String,
    pub window_resets_in_ms: u64,
    pub requests: QuotaMetricDto,
    pub input_tokens: QuotaMetricDto,
    pub output_tokens: QuotaMetricDto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaMetricDto {
    pub used: u64,
    pub max: Option<u64>,
    pub pct: u8,
    pub state: QuotaMetricState,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QuotaMetricState {
    Ok,
    Warn,
    Rejecting,
    Unconfigured,
}

// ---------------------------------------------------------------------------
// Account usage DTOs
// ---------------------------------------------------------------------------

/// `GET /admin/account/usage`
///
/// Merged account-level usage from all configured providers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountUsageResponse {
    pub providers: Vec<ProviderAccountUsageDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderAccountUsageDto {
    pub provider: String,
    pub status: ProviderUsageStatus,
    pub plan: Option<String>,
    pub windows: Vec<UsageWindowDto>,
    pub model_usage: Option<ModelUsageDto>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderUsageStatus {
    Available,
    NotSupported,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageWindowDto {
    pub label: String,
    pub used_pct: f64,
    pub used: Option<u64>,
    pub limit: Option<u64>,
    pub resets_at_ms: Option<i64>,
    pub sub_items: Vec<UsageSubItemDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageSubItemDto {
    pub label: String,
    pub used: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelUsageDto {
    pub total_tokens: u64,
    pub total_calls: u64,
    pub period_start_ms: i64,
    pub period_end_ms: i64,
}

#[cfg(test)]
mod usage_summary_tests {
    use super::*;

    #[test]
    fn usage_summary_response_round_trips_through_json() {
        let original = UsageSummaryResponse {
            from_ms: 1_700_000_000_000,
            to_ms: 1_700_086_400_000,
            daily: vec![DailyUsageRow {
                date: "2026-05-03".into(),
                requests: 12,
                input_tokens: 1_000,
                output_tokens: 200,
                cache_read_tokens: 5_000,
                cache_creation_tokens: 0,
                cost_usd: 0.42,
            }],
            models: vec![ModelUsageRow {
                model: "claude-opus-4-5".into(),
                provider: "anthropic".into(),
                requests: 12,
                input_tokens: 1_000,
                output_tokens: 200,
                cache_read_tokens: 5_000,
                cache_creation_tokens: 0,
                cost_usd: 0.42,
            }],
        };

        let json = serde_json::to_string(&original).unwrap();
        let decoded: UsageSummaryResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, original);
    }
}

#[cfg(test)]
mod account_usage_tests {
    use super::*;

    #[test]
    fn account_usage_response_round_trips_through_json() {
        let original = AccountUsageResponse {
            providers: vec![
                ProviderAccountUsageDto {
                    provider: "zai".into(),
                    status: ProviderUsageStatus::Available,
                    plan: Some("pro".into()),
                    windows: vec![UsageWindowDto {
                        label: "5h Token".into(),
                        used_pct: 40.5,
                        used: Some(16_200_000),
                        limit: Some(40_000_000),
                        resets_at_ms: Some(1_746_300_000_000),
                        sub_items: vec![],
                    }],
                    model_usage: Some(ModelUsageDto {
                        total_tokens: 12_500_000,
                        total_calls: 1_234,
                        period_start_ms: 1_746_220_800_000,
                        period_end_ms: 1_746_292_800_000,
                    }),
                },
                ProviderAccountUsageDto {
                    provider: "anthropic".into(),
                    status: ProviderUsageStatus::NotSupported,
                    plan: None,
                    windows: vec![],
                    model_usage: None,
                },
            ],
        };

        let json = serde_json::to_string(&original).unwrap();
        let decoded: AccountUsageResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn provider_usage_status_serializes_as_snake_case() {
        let s = serde_json::to_string(&ProviderUsageStatus::NotSupported).unwrap();
        assert_eq!(s, "\"not_supported\"");
    }
}

#[cfg(test)]
mod config_payload_toml_tests {
    use super::*;

    #[test]
    fn config_payload_round_trips_through_toml() {
        let payload = ConfigPayload {
            port: 8787,
            providers: vec![ProviderPayload {
                name: "anthropic".into(),
                kind: "anthropic".into(),
                auth: AuthPayload::Passthrough,
                base_url: None,
                openai_base_url: None,
            }],
            routing: vec![],
            quota: vec![QuotaPayload {
                provider: "zai".into(),
                window: "rolling:1h".into(),
                max_requests: Some(100),
                max_input_tokens: None,
                max_output_tokens: None,
                warn_pct: 80,
            }],
            affinity: AffinityPayload {
                enabled: true,
                headers: vec!["x-session-id".into()],
            },
            proxy_db: None,
            pricing_db: None,
        };
        let toml_str = toml::to_string_pretty(&payload).unwrap();
        let parsed: ConfigPayload = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.port, 8787);
        assert_eq!(parsed.providers.len(), 1);
        assert_eq!(parsed.quota.len(), 1);
        assert_eq!(parsed.quota[0].provider, "zai");
        assert!(parsed.affinity.enabled);
    }

    #[test]
    fn config_payload_defaults_when_empty_toml() {
        let toml_str = "port = 8787\n";
        let parsed: ConfigPayload = toml::from_str(toml_str).unwrap();
        assert!(parsed.providers.is_empty());
        assert!(parsed.routing.is_empty());
        assert!(parsed.quota.is_empty());
        assert!(parsed.affinity.enabled);
    }
}
