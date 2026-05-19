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
    docs_port: Option<u16>,
) -> Result<(), std::io::Error> {
    if let Some(port) = docs_port {
        let docs_addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        let docs_app = frameworks::openapi::build_docs_app();
        tracing::info!(address = %docs_addr, "docs server listening");
        tokio::spawn(async move {
            let listener = tokio::net::TcpListener::bind(docs_addr)
                .await
                .expect("failed to bind docs server");
            axum::serve(listener, docs_app).await.expect("docs server error");
        });
    }

    let app = frameworks::build_router(use_case, admin);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(address = %addr, "proxy listening");
    axum::serve(listener, app).await?;
    Ok(())
}
