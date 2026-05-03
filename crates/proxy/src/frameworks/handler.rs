//! /v1/messages handler — thin axum glue over HandleMessages.

use crate::application::use_cases::{ApiFormat, HandleMessages, HandleMessagesInput, HandleMessagesOutput};
use crate::frameworks::error::ProxyError;
use crate::frameworks::stream::TeedStream;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderName, StatusCode};
use axum::response::Response;
use std::sync::Arc;

const BODY_LIMIT: usize = 10 * 1024 * 1024;

const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "host",
    "content-length",
];

pub async fn messages(
    State(use_case): State<Arc<HandleMessages>>,
    req: Request,
) -> Result<Response, ProxyError> {
    let (parts, body) = req.into_parts();
    let body_bytes = axum::body::to_bytes(body, BODY_LIMIT)
        .await
        .map_err(|e| ProxyError::BadRequest(format!("body read: {e}")))?;

    let output = use_case
        .execute(HandleMessagesInput {
            headers: parts.headers,
            body: body_bytes,
            api_format: ApiFormat::Anthropic,
        })
        .await?;

    Ok(match output {
        HandleMessagesOutput::Buffered {
            status,
            headers,
            body,
        } => build_response(status, headers, Body::from(body)),
        HandleMessagesOutput::Streaming {
            status,
            headers,
            body,
            usage_parser,
            on_finish,
        } => {
            let teed = TeedStream::new(body, usage_parser, on_finish);
            build_response(status, headers, Body::from_stream(teed))
        }
    })
}

fn build_response(status: u16, upstream_headers: axum::http::HeaderMap, body: Body) -> Response {
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut response = Response::new(body);
    *response.status_mut() = status;
    let headers = response.headers_mut();
    for (k, v) in upstream_headers.iter() {
        if HOP_BY_HOP.contains(&k.as_str()) {
            continue;
        }
        if let (Ok(name), Ok(val)) = (
            HeaderName::from_bytes(k.as_str().as_bytes()),
            axum::http::HeaderValue::from_bytes(v.as_bytes()),
        ) {
            headers.insert(name, val);
        }
    }
    response
}
