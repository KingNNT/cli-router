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
}
