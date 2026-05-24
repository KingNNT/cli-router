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
    /// Full OAuth with auto-refresh. Carries the current tokens.
    OAuth {
        access_token: String,
        refresh_token: String,
        expires_at_ms: u64,
    },
}

impl AuthHeader {
    /// Returns `true` if the access token is expired or expires within
    /// `buffer_secs` seconds from now. Only meaningful for `OAuth` variant.
    pub fn is_expired(&self, buffer_secs: u64) -> bool {
        match self {
            AuthHeader::OAuth { expires_at_ms, .. } => {
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                now_ms + (buffer_secs * 1000) >= *expires_at_ms
            }
            _ => false,
        }
    }
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

/// Parse model and stream flag from the body in a single JSON pass.
/// Returns `(model, is_streaming)`.
pub fn parse_model_and_stream(body: &[u8]) -> Result<(String, bool), String> {
    let value: serde_json::Value =
        serde_json::from_slice(body).map_err(|e| format!("invalid json: {e}"))?;
    let model = value
        .get("model")
        .and_then(|m| m.as_str())
        .map(str::to_string)
        .ok_or_else(|| "missing 'model' field".to_string())?;
    let streaming = value
        .get("stream")
        .and_then(|s| s.as_bool())
        .unwrap_or(false);
    Ok((model, streaming))
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

pub(super) fn parse_openai_usage_json(body: &[u8]) -> Result<UsageRecord, String> {
    let value: serde_json::Value =
        serde_json::from_slice(body).map_err(|e| format!("invalid json: {e}"))?;
    let usage = value.get("usage").ok_or("missing 'usage' field")?;
    let get = |k: &str| usage.get(k).and_then(|v| v.as_u64());
    let cached = usage
        .get("prompt_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .and_then(|v| v.as_u64());
    Ok(UsageRecord {
        input_tokens: get("prompt_tokens"),
        output_tokens: get("completion_tokens"),
        cache_creation_tokens: None,
        cache_read_tokens: cached,
    })
}

pub(super) fn openai_usage_parser() -> Box<dyn UsageParser> {
    Box::new(OpenAiUsageParser(
        crate::adapters::usage::openai_sse::OpenAiSseParser::new(),
    ))
}

struct OpenAiUsageParser(crate::adapters::usage::openai_sse::OpenAiSseParser);

impl UsageParser for OpenAiUsageParser {
    fn feed(&mut self, chunk: &[u8]) {
        self.0.feed(chunk);
    }
    fn finish(self: Box<Self>) -> UsageRecord {
        UsageRecord::from(self.0.state)
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn forward(
    http: &reqwest::Client,
    base_url: &str,
    auth: &AuthHeader,
    path: &str,
    headers: &HeaderMap,
    body: Bytes,
    streaming: bool,
    provider_id: &str,
) -> Result<UpstreamResponse, ProxyError> {
    // Send the request using the current auth.
    let resp = send_request(
        http,
        base_url,
        auth,
        path,
        headers,
        &body,
        streaming,
        provider_id,
    )
    .await?;

    // On 401 with OAuth, refresh and retry once. This handles both Buffered
    // and Streaming response shapes — a 401 can come as either.
    // Proactive refresh is handled by the background token_refresh task
    // so we don't race with it on refresh-token rotation.
    let status = match &resp {
        UpstreamResponse::Buffered { status, .. } => *status,
        UpstreamResponse::Streaming { status, .. } => *status,
    };
    if status == 401
        && let AuthHeader::OAuth { refresh_token, .. } = auth
    {
        tracing::info!("401 from upstream, attempting OAuth refresh + retry");
        match crate::adapters::oauth::refresh_token(http, refresh_token).await {
            Ok(tokens) => {
                let refreshed = oauth_tokens_to_auth_header(&tokens);
                return send_request(
                    http,
                    base_url,
                    &refreshed,
                    path,
                    headers,
                    &body,
                    streaming,
                    provider_id,
                )
                .await;
            }
            Err(e) => {
                tracing::warn!(error = %e, "OAuth refresh on 401 failed");
            }
        }
    }

    Ok(resp)
}

fn oauth_tokens_to_auth_header(tokens: &crate::adapters::oauth::OAuthTokens) -> AuthHeader {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    // Default to 1 hour if expires_in is missing.
    let expires_in_ms = tokens.expires_in.unwrap_or(3600) * 1000;
    let refresh = tokens.refresh_token.clone().unwrap_or_default();
    if refresh.is_empty() {
        tracing::warn!(
            "OAuth token response contained no refresh_token; auto-refresh will not be possible"
        );
    }
    AuthHeader::OAuth {
        access_token: tokens.access_token.clone(),
        refresh_token: refresh,
        expires_at_ms: now_ms + expires_in_ms,
    }
}

#[allow(clippy::too_many_arguments)]
async fn send_request(
    http: &reqwest::Client,
    base_url: &str,
    auth: &AuthHeader,
    path: &str,
    headers: &HeaderMap,
    body: &[u8],
    streaming: bool,
    provider_id: &str,
) -> Result<UpstreamResponse, ProxyError> {
    let url = format!("{base_url}{path}");
    let mut req = http.post(&url).body(body.to_vec());
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
        AuthHeader::OAuth { access_token, .. } => {
            req = req.header("authorization", format!("Bearer {access_token}"));
            // Required for OAuth-authenticated requests.
            req = req.header("anthropic-beta", "oauth-2025-04-20");
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
            provider_id: provider_id.to_string(),
            translation_direction: None,
        })
    } else {
        let body = resp.bytes().await?;
        Ok(UpstreamResponse::Buffered {
            status,
            headers: headers_out,
            body,
            provider_id: provider_id.to_string(),
            translation_direction: None,
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

    #[test]
    fn auth_header_oauth_is_expired_checks_expiry() {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let expired = AuthHeader::OAuth {
            access_token: "at".into(),
            refresh_token: "rt".into(),
            expires_at_ms: now_ms - 1000, // already expired
        };
        assert!(expired.is_expired(300));

        let fresh = AuthHeader::OAuth {
            access_token: "at".into(),
            refresh_token: "rt".into(),
            expires_at_ms: now_ms + 600_000, // expires in 10 min
        };
        assert!(!fresh.is_expired(300));

        let almost_expired = AuthHeader::OAuth {
            access_token: "at".into(),
            refresh_token: "rt".into(),
            expires_at_ms: now_ms + 200_000, // expires in 200s, within 300s buffer
        };
        assert!(almost_expired.is_expired(300));
    }

    #[test]
    fn non_oauth_auth_header_is_never_expired() {
        assert!(!AuthHeader::Passthrough.is_expired(300));
        assert!(!AuthHeader::ApiKey("k".into()).is_expired(300));
        assert!(!AuthHeader::Bearer("b".into()).is_expired(300));
    }

    #[test]
    fn parse_openai_usage_json_extracts_all_fields() {
        let body = br#"{
            "id":"chatcmpl-1","object":"chat.completion","choices":[],
            "usage":{
                "prompt_tokens": 6,
                "completion_tokens": 10,
                "total_tokens": 16,
                "prompt_tokens_details": { "cached_tokens": 2 }
            }
        }"#;
        let u = parse_openai_usage_json(body).unwrap();
        assert_eq!(u.input_tokens, Some(6));
        assert_eq!(u.output_tokens, Some(10));
        assert_eq!(u.cache_read_tokens, Some(2));
        assert_eq!(u.cache_creation_tokens, None);
    }

    #[test]
    fn parse_openai_usage_json_handles_missing_prompt_tokens_details() {
        let body = br#"{"usage":{"prompt_tokens":1,"completion_tokens":2,"total_tokens":3}}"#;
        let u = parse_openai_usage_json(body).unwrap();
        assert_eq!(u.input_tokens, Some(1));
        assert_eq!(u.output_tokens, Some(2));
        assert_eq!(u.cache_read_tokens, None);
        assert_eq!(u.cache_creation_tokens, None);
    }

    #[test]
    fn parse_openai_usage_json_errors_when_usage_missing() {
        let body = br#"{"id":"chatcmpl-1","choices":[]}"#;
        assert!(parse_openai_usage_json(body).is_err());
    }

    #[test]
    fn openai_usage_parser_collects_streaming_usage() {
        let mut p = openai_usage_parser();
        p.feed(b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n");
        p.feed(b"data: {\"choices\":[],\"usage\":{\"prompt_tokens\":4,\"completion_tokens\":8,\"prompt_tokens_details\":{\"cached_tokens\":1}}}\n");
        p.feed(b"data: [DONE]\n");
        let rec = p.finish();
        assert_eq!(rec.input_tokens, Some(4));
        assert_eq!(rec.output_tokens, Some(8));
        assert_eq!(rec.cache_read_tokens, Some(1));
        assert_eq!(rec.cache_creation_tokens, None);
    }
}
