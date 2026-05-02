//! Build the axum Router with all proxy routes.

use crate::application::use_cases::HandleMessages;
use crate::frameworks::handler::messages;
use axum::{routing::post, Router};
use std::sync::Arc;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::TraceLayer;

pub fn build_router(use_case: Arc<HandleMessages>) -> Router {
    let id_layer = SetRequestIdLayer::new(
        axum::http::HeaderName::from_static("x-request-id"),
        MakeRequestUuid,
    );
    Router::new()
        .route("/v1/messages", post(messages))
        .with_state(use_case)
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(id_layer)
        .layer(TraceLayer::new_for_http())
}
