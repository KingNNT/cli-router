//! Admin control-plane HTTP handlers under `/admin/*`. Wraps the admin use
//! cases with axum extractors and returns JSON shaped per `proxy_admin_api`.
//!
//! Bound to `127.0.0.1` only by `serve()` — there is no auth on these
//! endpoints. Don't expose the proxy beyond loopback without putting a
//! bearer-token middleware in front.

use crate::application::errors::ProxyError;
use crate::application::use_cases::{
    CompleteAnthropicOAuth, CompleteOpenAiOAuth, GetAccountUsage, GetConfig, GetQuotaStatus,
    GetRecentRequests, GetStatus, GetUsageSummary, StartAnthropicOAuth, StartOpenAiOAuth,
    TestProvider, UpdateConfig,
};
use axum::extract::{FromRef, Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use proxy_admin_api::{
    AccountUsageResponse, CompleteOAuthRequest, CompleteOAuthResponse, ConfigPayload,
    QuotaStatusListDto, RecentRequestsResponse, StartOAuthRequest, StartOAuthResponse,
    StatusResponse, TestProviderRequest, TestProviderResponse, UsageSummaryResponse,
};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Clone)]
pub struct AdminState {
    pub get_status: Arc<GetStatus>,
    pub get_config: Arc<GetConfig>,
    pub get_recent: Arc<GetRecentRequests>,
    pub update_config: Arc<UpdateConfig>,
    pub test_provider: Arc<TestProvider>,
    pub start_oauth: Arc<StartAnthropicOAuth>,
    pub complete_oauth: Arc<CompleteAnthropicOAuth>,
    pub start_openai_oauth: Arc<StartOpenAiOAuth>,
    pub complete_openai_oauth: Arc<CompleteOpenAiOAuth>,
    pub usage_summary: Arc<GetUsageSummary>,
    pub quota_status: Arc<GetQuotaStatus>,
    pub account_usage: Arc<GetAccountUsage>,
}

impl FromRef<AdminState> for Arc<GetUsageSummary> {
    fn from_ref(s: &AdminState) -> Self {
        s.usage_summary.clone()
    }
}

impl FromRef<AdminState> for Arc<GetAccountUsage> {
    fn from_ref(s: &AdminState) -> Self {
        s.account_usage.clone()
    }
}

pub fn build_admin_router(state: AdminState) -> Router {
    Router::new()
        .route("/admin/status", get(status_handler))
        .route(
            "/admin/config",
            get(get_config_handler).put(update_config_handler),
        )
        .route("/admin/requests/recent", get(recent_handler))
        .route("/admin/usage/summary", get(usage_summary_handler))
        .route("/admin/account/usage", get(account_usage_handler))
        .route("/admin/quota/status", get(quota_status_handler))
        .route("/admin/providers/:name/test", post(test_provider_handler))
        .route("/admin/oauth/anthropic/start", post(oauth_start_handler))
        .route(
            "/admin/oauth/anthropic/complete",
            post(oauth_complete_handler),
        )
        .route(
            "/admin/oauth/openai/start",
            post(openai_oauth_start_handler),
        )
        .route(
            "/admin/oauth/openai/complete",
            post(openai_oauth_complete_handler),
        )
        .with_state(state)
}

#[utoipa::path(
    get,
    path = "/admin/status",
    responses(
        (status = 200, description = "Proxy uptime and request counts", body = StatusResponse),
        (status = 500, description = "Internal error"),
    ),
    tag = "Admin"
)]
async fn status_handler(State(s): State<AdminState>) -> Result<Json<StatusResponse>, ProxyError> {
    Ok(Json(s.get_status.execute()?))
}

#[utoipa::path(
    get,
    path = "/admin/config",
    responses(
        (status = 200, description = "Current proxy configuration", body = ConfigPayload),
        (status = 500, description = "Internal error"),
    ),
    tag = "Admin"
)]
async fn get_config_handler(
    State(s): State<AdminState>,
) -> Result<Json<ConfigPayload>, ProxyError> {
    Ok(Json(s.get_config.execute()?))
}

#[utoipa::path(
    put,
    path = "/admin/config",
    request_body = ConfigPayload,
    responses(
        (status = 200, description = "Config updated, returns new config", body = ConfigPayload),
        (status = 400, description = "Invalid config payload"),
        (status = 500, description = "Internal error"),
    ),
    tag = "Admin"
)]
async fn update_config_handler(
    State(s): State<AdminState>,
    Json(payload): Json<ConfigPayload>,
) -> Result<Json<ConfigPayload>, ProxyError> {
    s.update_config.execute(payload)?;
    Ok(Json(s.get_config.execute()?))
}

#[derive(Deserialize)]
struct RecentParams {
    limit: Option<u32>,
    offset: Option<u32>,
}

#[utoipa::path(
    get,
    path = "/admin/requests/recent",
    params(
        ("limit" = Option<u32>, Query, description = "Max rows to return"),
        ("offset" = Option<u32>, Query, description = "Row offset for pagination"),
    ),
    responses(
        (status = 200, description = "Recent request log entries", body = RecentRequestsResponse),
        (status = 500, description = "Internal error"),
    ),
    tag = "Admin"
)]
async fn recent_handler(
    State(s): State<AdminState>,
    Query(params): Query<RecentParams>,
) -> Result<Json<RecentRequestsResponse>, ProxyError> {
    Ok(Json(s.get_recent.execute(params.limit, params.offset)?))
}

#[derive(Debug, Deserialize)]
pub struct UsageSummaryQuery {
    pub from: i64,
    pub to: i64,
}

#[utoipa::path(
    get,
    path = "/admin/usage/summary",
    params(
        ("from" = i64, Query, description = "Start timestamp (epoch ms)"),
        ("to" = i64, Query, description = "End timestamp (epoch ms)"),
    ),
    responses(
        (status = 200, description = "Aggregate usage summary", body = UsageSummaryResponse),
        (status = 500, description = "Internal error"),
    ),
    tag = "Usage"
)]
async fn usage_summary_handler(
    State(uc): State<Arc<GetUsageSummary>>,
    Query(q): Query<UsageSummaryQuery>,
) -> Result<Json<UsageSummaryResponse>, ProxyError> {
    Ok(Json(uc.execute(q.from, q.to)?))
}

#[utoipa::path(
    post,
    path = "/admin/providers/{name}/test",
    params(
        ("name" = String, Path, description = "Provider name"),
    ),
    request_body = TestProviderRequest,
    responses(
        (status = 200, description = "Provider test result", body = TestProviderResponse),
    ),
    tag = "Providers"
)]
async fn test_provider_handler(
    State(s): State<AdminState>,
    Path(name): Path<String>,
    Json(req): Json<TestProviderRequest>,
) -> Json<TestProviderResponse> {
    Json(s.test_provider.execute(&name, &req.model).await)
}

#[utoipa::path(
    post,
    path = "/admin/oauth/anthropic/start",
    request_body = StartOAuthRequest,
    responses(
        (status = 200, description = "OAuth authorization URL", body = StartOAuthResponse),
    ),
    tag = "OAuth"
)]
async fn oauth_start_handler(
    State(s): State<AdminState>,
    Json(_req): Json<StartOAuthRequest>,
) -> Json<StartOAuthResponse> {
    Json(s.start_oauth.execute())
}

#[utoipa::path(
    post,
    path = "/admin/oauth/anthropic/complete",
    request_body = CompleteOAuthRequest,
    responses(
        (status = 200, description = "OAuth completion result", body = CompleteOAuthResponse),
    ),
    tag = "OAuth"
)]
async fn oauth_complete_handler(
    State(s): State<AdminState>,
    Json(req): Json<CompleteOAuthRequest>,
) -> Json<CompleteOAuthResponse> {
    Json(s.complete_oauth.execute(req).await)
}

#[utoipa::path(
    post,
    path = "/admin/oauth/openai/start",
    request_body = StartOAuthRequest,
    responses(
        (status = 200, description = "OAuth authorization URL", body = StartOAuthResponse),
    ),
    tag = "OAuth"
)]
async fn openai_oauth_start_handler(
    State(s): State<AdminState>,
    Json(_req): Json<StartOAuthRequest>,
) -> Json<StartOAuthResponse> {
    Json(s.start_openai_oauth.execute())
}

#[utoipa::path(
    post,
    path = "/admin/oauth/openai/complete",
    request_body = CompleteOAuthRequest,
    responses(
        (status = 200, description = "OAuth completion result", body = CompleteOAuthResponse),
    ),
    tag = "OAuth"
)]
async fn openai_oauth_complete_handler(
    State(s): State<AdminState>,
    Json(req): Json<CompleteOAuthRequest>,
) -> Json<CompleteOAuthResponse> {
    Json(s.complete_openai_oauth.execute(req).await)
}

#[utoipa::path(
    get,
    path = "/admin/quota/status",
    responses(
        (status = 200, description = "Per-provider quota health", body = QuotaStatusListDto),
    ),
    tag = "Usage"
)]
async fn quota_status_handler(State(s): State<AdminState>) -> Json<QuotaStatusListDto> {
    Json(s.quota_status.execute())
}

#[utoipa::path(
    get,
    path = "/admin/account/usage",
    responses(
        (status = 200, description = "Provider account balances and quota", body = AccountUsageResponse),
    ),
    tag = "Usage"
)]
async fn account_usage_handler(
    State(uc): State<Arc<GetAccountUsage>>,
) -> Json<AccountUsageResponse> {
    Json(uc.execute())
}
