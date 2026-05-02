//! proxy — HTTP proxy for LLM providers with usage capture.
//!
//! See docs/superpowers/specs/2026-05-02-workspace-and-proxy-mvp-design.md
//! for the design contract.

pub mod adapters;
pub mod application;
pub mod config;
pub mod domain;
pub mod frameworks;

use crate::application::use_cases::HandleMessages;
use std::net::SocketAddr;
use std::sync::Arc;

pub async fn serve(
    addr: SocketAddr,
    use_case: Arc<HandleMessages>,
    admin: frameworks::AdminState,
) -> Result<(), std::io::Error> {
    let app = frameworks::build_router(use_case, admin);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(address = %addr, "proxy listening");
    axum::serve(listener, app).await?;
    Ok(())
}
