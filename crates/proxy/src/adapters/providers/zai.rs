//! Z.ai provider — implements `Provider` against Z.ai's Anthropic-compatible
//! endpoint at `https://api.z.ai/api/anthropic`.
//!
//! Z.ai exposes the same Messages API as Anthropic (path `/v1/messages`,
//! `x-api-key` header, identical body schema, identical SSE event names),
//! so all protocol-level logic is delegated to `super::messages_protocol`.
//! Only `name()` and `base_url` differ from `AnthropicProvider`.

use super::messages_protocol::{self, AuthHeader};
use crate::application::errors::ProxyError;
use crate::application::ports::{Provider, UpstreamResponse, UsageParser};
use crate::domain::UsageRecord;
use async_trait::async_trait;
use axum::http::HeaderMap;
use bytes::Bytes;

pub struct ZaiProvider {
    base_url: String,
    http: reqwest::Client,
    auth: AuthHeader,
}

impl ZaiProvider {
    pub fn new(http: reqwest::Client) -> Self {
        Self::build(
            http,
            "https://api.z.ai/api/anthropic".into(),
            AuthHeader::Passthrough,
        )
    }

    pub fn with_base_url(http: reqwest::Client, base_url: impl Into<String>) -> Self {
        Self::build(http, base_url.into(), AuthHeader::Passthrough)
    }

    pub fn with_auth(http: reqwest::Client, auth: AuthHeader) -> Self {
        Self::build(http, "https://api.z.ai/api/anthropic".into(), auth)
    }

    pub fn configure(http: reqwest::Client, base_url: Option<String>, auth: AuthHeader) -> Self {
        Self::build(
            http,
            base_url.unwrap_or_else(|| "https://api.z.ai/api/anthropic".into()),
            auth,
        )
    }

    fn build(http: reqwest::Client, base_url: String, auth: AuthHeader) -> Self {
        Self {
            base_url,
            http,
            auth,
        }
    }
}

#[async_trait]
impl Provider for ZaiProvider {
    fn name(&self) -> &'static str {
        "zai"
    }

    fn parse_model(&self, body: &[u8]) -> Result<String, String> {
        messages_protocol::parse_model(body)
    }

    fn usage_parser(&self) -> Box<dyn UsageParser> {
        messages_protocol::usage_parser()
    }

    fn parse_usage_json(&self, body: &[u8]) -> Result<UsageRecord, String> {
        messages_protocol::parse_usage_json(body)
    }

    async fn forward(
        &self,
        path: &str,
        headers: &HeaderMap,
        body: Bytes,
        streaming: bool,
    ) -> Result<UpstreamResponse, ProxyError> {
        messages_protocol::forward(
            &self.http,
            &self.base_url,
            &self.auth,
            path,
            headers,
            body,
            streaming,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> ZaiProvider {
        ZaiProvider::new(reqwest::Client::new())
    }

    #[test]
    fn name_is_zai() {
        assert_eq!(provider().name(), "zai");
    }

    #[test]
    fn default_base_url_points_to_zai_anthropic_endpoint() {
        assert_eq!(provider().base_url, "https://api.z.ai/api/anthropic");
    }

    #[test]
    fn with_base_url_overrides_default() {
        let p = ZaiProvider::with_base_url(reqwest::Client::new(), "http://localhost:1234");
        assert_eq!(p.base_url, "http://localhost:1234");
    }

    #[test]
    fn parses_glm_model_id_from_body() {
        let body = br#"{"model":"glm-4.6","messages":[]}"#;
        assert_eq!(provider().parse_model(body).unwrap(), "glm-4.6");
    }

    #[test]
    fn default_auth_is_passthrough() {
        assert!(matches!(provider().auth, AuthHeader::Passthrough));
    }

    #[test]
    fn configure_sets_both_base_url_and_auth() {
        let p = ZaiProvider::configure(
            reqwest::Client::new(),
            Some("http://localhost:1234".into()),
            AuthHeader::ApiKey("zai-test".into()),
        );
        assert_eq!(p.base_url, "http://localhost:1234");
        assert!(matches!(p.auth, AuthHeader::ApiKey(_)));
    }
}
