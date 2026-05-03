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
    pub providers: Vec<ProviderPayload>,
    pub routing: Vec<RoutingRulePayload>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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

/// `GET /admin/requests/recent?limit=N`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentRequestsResponse {
    pub items: Vec<RecentRequestItem>,
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
