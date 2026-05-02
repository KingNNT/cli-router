//! Anthropic provider adapter — implements the Provider port using reqwest.

use crate::adapters::usage::anthropic_sse::AnthropicSseParser;
use crate::application::errors::ProxyError;
use crate::application::ports::{BoxedByteStream, Provider, UpstreamResponse, UsageParser};
use crate::domain::UsageRecord;
use async_trait::async_trait;
use axum::http::HeaderMap;
use bytes::Bytes;
use futures::StreamExt;

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

pub struct AnthropicProvider {
    base_url: String,
    http: reqwest::Client,
}

impl AnthropicProvider {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            base_url: "https://api.anthropic.com".into(),
            http,
        }
    }

    pub fn with_base_url(http: reqwest::Client, base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            http,
        }
    }
}

struct AnthropicUsageParser(AnthropicSseParser);

impl UsageParser for AnthropicUsageParser {
    fn feed(&mut self, chunk: &[u8]) {
        self.0.feed(chunk);
    }
    fn finish(self: Box<Self>) -> UsageRecord {
        UsageRecord::from(self.0.state)
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    fn parse_model(&self, body: &[u8]) -> Result<String, String> {
        let value: serde_json::Value =
            serde_json::from_slice(body).map_err(|e| format!("invalid json: {e}"))?;
        value
            .get("model")
            .and_then(|m| m.as_str())
            .map(str::to_string)
            .ok_or_else(|| "missing 'model' field".to_string())
    }

    fn usage_parser(&self) -> Box<dyn UsageParser> {
        Box::new(AnthropicUsageParser(AnthropicSseParser::new()))
    }

    fn parse_usage_json(&self, body: &[u8]) -> Result<UsageRecord, String> {
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

    async fn forward(
        &self,
        path: &str,
        headers: &HeaderMap,
        body: Bytes,
        streaming: bool,
    ) -> Result<UpstreamResponse, ProxyError> {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.http.post(&url).body(body);
        for (k, v) in headers {
            if HOP_BY_HOP.contains(&k.as_str()) {
                continue;
            }
            req = req.header(k, v);
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> AnthropicProvider {
        AnthropicProvider::new(reqwest::Client::new())
    }

    #[test]
    fn parse_model_extracts_name_from_body() {
        let body = br#"{"model":"claude-3-5-sonnet-20241022","messages":[]}"#;
        assert_eq!(
            provider().parse_model(body).unwrap(),
            "claude-3-5-sonnet-20241022"
        );
    }

    #[test]
    fn parse_model_errors_when_missing() {
        let body = br#"{"messages":[]}"#;
        assert!(provider().parse_model(body).is_err());
    }

    #[test]
    fn parse_model_errors_on_invalid_json() {
        let body = b"not json";
        assert!(provider().parse_model(body).is_err());
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
        let u = provider().parse_usage_json(body).unwrap();
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
        let u = provider().parse_usage_json(body).unwrap();
        assert_eq!(u.input_tokens, Some(1));
        assert_eq!(u.output_tokens, Some(2));
        assert_eq!(u.cache_read_tokens, None);
        assert_eq!(u.cache_creation_tokens, None);
    }

    #[test]
    fn usage_parser_collects_streaming_usage() {
        let mut p = provider().usage_parser();
        p.feed(b"event: message_start\ndata: {\"message\":{\"usage\":{\"input_tokens\":7,\"output_tokens\":1}}}\n\n");
        p.feed(b"event: message_delta\ndata: {\"usage\":{\"output_tokens\":13}}\n\n");
        let rec = p.finish();
        assert_eq!(rec.input_tokens, Some(7));
        assert_eq!(rec.output_tokens, Some(13));
    }
}
