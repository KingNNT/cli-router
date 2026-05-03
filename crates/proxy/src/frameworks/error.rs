//! IntoResponse mapping for ProxyError. Enum lives in application::errors.
//!
//! Note: pricing lookup failures do NOT bubble up as ProxyError. They're
//! logged and result in cost_usd = NULL on the request log row, which is
//! the desired behavior — the request itself succeeded; cost is just
//! best-effort metadata.

use axum::Json;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

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
            ProxyError::UpstreamRateLimited {
                retry_after_secs,
                message,
            } => {
                tracing::warn!(retry_after = retry_after_secs, %message, "all providers rate limited");
                let mut headers = HeaderMap::new();
                if let Ok(val) = HeaderValue::from_str(&retry_after_secs.to_string()) {
                    headers.insert("retry-after", val);
                }
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    headers,
                    Json(serde_json::json!({
                        "error": {
                            "type": "rate_limit_error",
                            "message": message,
                            "retry_after": retry_after_secs
                        }
                    })),
                )
                    .into_response()
            }
            ProxyError::QuotaExceeded {
                provider,
                metric,
                retry_after_ms,
            } => {
                tracing::warn!(provider=%provider, metric=%metric, retry_after_ms, "proxy quota exceeded");
                let mut headers = HeaderMap::new();
                let retry_after_secs = retry_after_ms.div_ceil(1000);
                if let Ok(val) = HeaderValue::from_str(&retry_after_secs.to_string()) {
                    headers.insert("retry-after", val);
                }
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    headers,
                    Json(serde_json::json!({
                        "error": {
                            "type": "rate_limit_error",
                            "message": format!("proxy quota exceeded for {provider} on {metric}"),
                            "retry_after": retry_after_secs,
                        }
                    })),
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
