//! Admin control-plane HTTP handlers under `/admin/*`. Wraps the admin use
//! cases with axum extractors and returns JSON shaped per `proxy_admin_api`.
//!
//! Bound to `127.0.0.1` only by `serve()` — there is no auth on these
//! endpoints. Don't expose the proxy beyond loopback without putting a
//! bearer-token middleware in front.

use crate::application::errors::ProxyError;
use crate::application::use_cases::{
    CompleteAnthropicOAuth, GetConfig, GetRecentRequests, GetStatus, StartAnthropicOAuth,
    TestProvider, UpdateConfig,
};
use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use proxy_admin_api::{
    CompleteOAuthRequest, CompleteOAuthResponse, ConfigPayload, RecentRequestsResponse,
    StartOAuthRequest, StartOAuthResponse, StatusResponse, TestProviderRequest,
    TestProviderResponse,
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
}

pub fn build_admin_router(state: AdminState) -> Router {
    Router::new()
        .route("/admin/status", get(status_handler))
        .route(
            "/admin/config",
            get(get_config_handler).put(update_config_handler),
        )
        .route("/admin/requests/recent", get(recent_handler))
        .route("/admin/providers/:name/test", post(test_provider_handler))
        .route("/admin/oauth/anthropic/start", post(oauth_start_handler))
        .route(
            "/admin/oauth/anthropic/complete",
            post(oauth_complete_handler),
        )
        .with_state(state)
}

async fn status_handler(State(s): State<AdminState>) -> Result<Json<StatusResponse>, ProxyError> {
    Ok(Json(s.get_status.execute()?))
}

async fn get_config_handler(
    State(s): State<AdminState>,
) -> Result<Json<ConfigPayload>, ProxyError> {
    Ok(Json(s.get_config.execute()?))
}

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
}

async fn recent_handler(
    State(s): State<AdminState>,
    Query(params): Query<RecentParams>,
) -> Result<Json<RecentRequestsResponse>, ProxyError> {
    Ok(Json(s.get_recent.execute(params.limit)?))
}

async fn test_provider_handler(
    State(s): State<AdminState>,
    Path(name): Path<String>,
    Json(req): Json<TestProviderRequest>,
) -> Json<TestProviderResponse> {
    Json(s.test_provider.execute(&name, &req.model).await)
}

async fn oauth_start_handler(
    State(s): State<AdminState>,
    Json(_req): Json<StartOAuthRequest>,
) -> Json<StartOAuthResponse> {
    Json(s.start_oauth.execute())
}

async fn oauth_complete_handler(
    State(s): State<AdminState>,
    Json(req): Json<CompleteOAuthRequest>,
) -> Json<CompleteOAuthResponse> {
    Json(s.complete_oauth.execute(req).await)
}
