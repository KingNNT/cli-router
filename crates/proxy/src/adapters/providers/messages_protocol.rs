//! Shared implementation of the Anthropic Messages API protocol
//! (`POST /v1/messages` with SSE streaming and `usage` JSON shape).
//!
//! Both `AnthropicProvider` and `ZaiProvider` speak this protocol; their
//! `Provider` impls delegate the protocol-level logic into the free
//! functions below, and only differ in `name()` and `base_url`.

use crate::adapters::usage::anthropic_sse::AnthropicSseParser;
use crate::application::errors::ProxyError;
use crate::application::ports::{BoxedByteStream, UpstreamResponse, UsageParser};
use crate::domain::UsageRecord;
use axum::http::HeaderMap;
use bytes::Bytes;
use futures::StreamExt;

pub(super) const HOP_BY_HOP: &[&str] = &[
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

/// Headers the proxy may inject — stripped from the incoming request when the
/// configured auth mode overrides them, so the client can't smuggle a stale
/// or wrong credential through.
const AUTH_HEADERS: &[&str] = &["x-api-key", "authorization"];

/// How the proxy authenticates to the upstream when forwarding a request.
#[derive(Debug, Clone)]
pub enum AuthHeader {
    /// Forward whatever auth headers the client sent (Phase 0 behaviour).
    Passthrough,
    /// Strip incoming auth, inject `x-api-key: <value>`.
    ApiKey(String),
    /// Strip incoming auth, inject `authorization: Bearer <value>`.
    Bearer(String),
}

pub(super) fn parse_model(body: &[u8]) -> Result<String, String> {
    let value: serde_json::Value =
        serde_json::from_slice(body).map_err(|e| format!("invalid json: {e}"))?;
    value
        .get("model")
        .and_then(|m| m.as_str())
        .map(str::to_string)
        .ok_or_else(|| "missing 'model' field".to_string())
}

pub(super) fn parse_usage_json(body: &[u8]) -> Result<UsageRecord, String> {
    let value: serde_json::Value =
        serde_json::from_slice(body).map_err(|e| format!("invalid json: {e}"))?;
    let usage = value.get("usage").ok_or("missing 'usage' field")?;
    let get = |k: &str| usage.get(k).and_then(|v| v.as_u64());
    Ok(UsageRecord {
        input_tokens: get("input_tokens"),
        output_tokens: get("output_tokens"),
        cache_creation_tokens: get("cache_creation_input_tokens"),
        cache_read_tokens: get("cache_read_input_tokens"),
    })
}

pub(super) fn usage_parser() -> Box<dyn UsageParser> {
    Box::new(SseUsageParser(AnthropicSseParser::new()))
}

struct SseUsageParser(AnthropicSseParser);

impl UsageParser for SseUsageParser {
    fn feed(&mut self, chunk: &[u8]) {
        self.0.feed(chunk);
    }
    fn finish(self: Box<Self>) -> UsageRecord {
        UsageRecord::from(self.0.state)
    }
}

pub(super) async fn forward(
    http: &reqwest::Client,
    base_url: &str,
    auth: &AuthHeader,
    path: &str,
    headers: &HeaderMap,
    body: Bytes,
    streaming: bool,
) -> Result<UpstreamResponse, ProxyError> {
    let url = format!("{base_url}{path}");
    let mut req = http.post(&url).body(body);
    let strip_auth = !matches!(auth, AuthHeader::Passthrough);
    for (k, v) in headers {
        if HOP_BY_HOP.contains(&k.as_str()) {
            continue;
        }
        if strip_auth && AUTH_HEADERS.contains(&k.as_str()) {
            continue;
        }
        req = req.header(k, v);
    }
    match auth {
        AuthHeader::Passthrough => {}
        AuthHeader::ApiKey(v) => {
            req = req.header("x-api-key", v);
        }
        AuthHeader::Bearer(v) => {
            req = req.header("authorization", format!("Bearer {v}"));
        }
    }
    let resp = req.send().await?;
    let status = resp.status().as_u16();
    let mut headers_out = HeaderMap::new();
    for (k, v) in resp.headers() {
        if HOP_BY_HOP.contains(&k.as_str()) {
            continue;
        }
        headers_out.insert(k.clone(), v.clone());
    }
    if streaming {
        let stream = resp.bytes_stream().map(|res| {
            res.map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })
        });
        let body: BoxedByteStream = Box::pin(stream);
        Ok(UpstreamResponse::Streaming {
            status,
            headers: headers_out,
            body,
        })
    } else {
        let body = resp.bytes().await?;
        Ok(UpstreamResponse::Buffered {
            status,
            headers: headers_out,
            body,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_model_extracts_name_from_body() {
        let body = br#"{"model":"claude-3-5-sonnet-20241022","messages":[]}"#;
        assert_eq!(parse_model(body).unwrap(), "claude-3-5-sonnet-20241022");
    }

    #[test]
    fn parse_model_errors_when_missing() {
        let body = br#"{"messages":[]}"#;
        assert!(parse_model(body).is_err());
    }

    #[test]
    fn parse_model_errors_on_invalid_json() {
        let body = b"not json";
        assert!(parse_model(body).is_err());
    }

    #[test]
    fn parse_usage_json_extracts_all_fields() {
        let body = br#"{
            "id":"msg_1","type":"message","role":"assistant","content":[],
            "model":"claude-3-5-sonnet-20241022","stop_reason":"end_turn",
            "usage":{
                "input_tokens": 100,
                "output_tokens": 200,
                "cache_creation_input_tokens": 50,
                "cache_read_input_tokens": 25
            }
        }"#;
        let u = parse_usage_json(body).unwrap();
        assert_eq!(u.input_tokens, Some(100));
        assert_eq!(u.output_tokens, Some(200));
        assert_eq!(u.cache_creation_tokens, Some(50));
        assert_eq!(u.cache_read_tokens, Some(25));
    }

    #[test]
    fn parse_usage_json_handles_missing_cache_fields() {
        let body = br#"{
            "usage":{"input_tokens":1, "output_tokens":2}
        }"#;
        let u = parse_usage_json(body).unwrap();
        assert_eq!(u.input_tokens, Some(1));
        assert_eq!(u.output_tokens, Some(2));
        assert_eq!(u.cache_read_tokens, None);
        assert_eq!(u.cache_creation_tokens, None);
    }

    #[test]
    fn usage_parser_collects_streaming_usage() {
        let mut p = usage_parser();
        p.feed(b"event: message_start\ndata: {\"message\":{\"usage\":{\"input_tokens\":7,\"output_tokens\":1}}}\n\n");
        p.feed(b"event: message_delta\ndata: {\"usage\":{\"output_tokens\":13}}\n\n");
        let rec = p.finish();
        assert_eq!(rec.input_tokens, Some(7));
        assert_eq!(rec.output_tokens, Some(13));
    }
}
