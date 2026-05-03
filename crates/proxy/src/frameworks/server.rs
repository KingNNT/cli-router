//! Build the axum Router with all proxy routes.
//!
//! Two route groups live behind the same listener:
//! - `POST /v1/messages` — data path (proxied to upstream LLM provider)
//! - `/admin/*` — control plane (status, config CRUD, recent requests, ping)
//!
//! Each group has its own state — `Arc<HandleMessages>` for the data path,
//! `AdminState` for admin — merged into one `Router<()>`.

use crate::application::use_cases::HandleMessages;
use crate::frameworks::admin::{AdminState, build_admin_router};
use crate::frameworks::handler::{chat_completions, messages};
use axum::{Router, routing::post};
use std::sync::Arc;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::TraceLayer;

pub fn build_router(use_case: Arc<HandleMessages>, admin: AdminState) -> Router {
    let id_layer = SetRequestIdLayer::new(
        axum::http::HeaderName::from_static("x-request-id"),
        MakeRequestUuid,
    );
    let data = Router::new()
        .route("/v1/messages", post(messages))
        .route("/v1/chat/completions", post(chat_completions))
        .with_state(use_case);
    data.merge(build_admin_router(admin))
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(id_layer)
        .layer(TraceLayer::new_for_http())
}
