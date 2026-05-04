//! ProxyError — the boundary error returned from use cases.
//!
//! `IntoResponse` impl lives in `frameworks/error.rs` (or `server/error.rs`
//! pre-Task-3) since it depends on axum.
//!
//! Note: pricing lookup failures do NOT bubble up as ProxyError. They're
//! logged and result in cost_usd = NULL on the request log row, which is
//! the desired behavior — the request itself succeeded; cost is just
//! best-effort metadata.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ProxyError {
    #[error("storage: {0}")]
    Storage(#[from] rusqlite::Error),
    #[error("upstream transport: {0}")]
    UpstreamTransport(#[from] reqwest::Error),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("rate limited: {message}")]
    UpstreamRateLimited {
        retry_after_secs: u64,
        message: String,
    },
    #[error("quota exceeded for {provider} on {metric}; retry after {retry_after_ms}ms")]
    QuotaExceeded {
        provider: String,
        metric: &'static str,
        retry_after_ms: u64,
    },
    #[error("translation: invalid request: {field}: {reason}")]
    TranslationInvalidRequest { field: &'static str, reason: String },
    #[error("upstream usage query failed for {provider}: {message}")]
    UpstreamUsage {
        provider: String,
        message: String,
    },
}
