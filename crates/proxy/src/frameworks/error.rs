//! IntoResponse mapping for ProxyError. Enum lives in application::errors.
//!
//! Note: pricing lookup failures do NOT bubble up as ProxyError. They're
//! logged and result in cost_usd = NULL on the request log row, which is
//! the desired behavior — the request itself succeeded; cost is just
//! best-effort metadata.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

pub use crate::application::errors::ProxyError;

impl IntoResponse for ProxyError {
    fn into_response(self) -> Response {
        match self {
            ProxyError::BadRequest(msg) => (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error":{"type":"bad_request","message":msg}})),
            )
                .into_response(),
            ProxyError::UpstreamTransport(e) => {
                tracing::error!(error = %e, "upstream transport failure");
                (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({"error":{"type":"upstream_unavailable"}})),
                )
                    .into_response()
            }
            other => {
                tracing::error!(error = %other, "internal proxy error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error":{"type":"internal"}})),
                )
                    .into_response()
            }
        }
    }
}
