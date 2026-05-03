//! Anthropic provider — implements `Provider` against `https://api.anthropic.com`.
//!
//! Protocol-level logic (parse_model, parse_usage_json, usage_parser, forward)
//! is delegated to `super::messages_protocol`, which is shared with `ZaiProvider`.

use super::messages_protocol::{self, AuthHeader};
use crate::application::errors::ProxyError;
use crate::application::ports::{Provider, UpstreamResponse, UsageParser};
use crate::domain::UsageRecord;
use async_trait::async_trait;
use axum::http::HeaderMap;
use bytes::Bytes;

pub struct AnthropicProvider {
    base_url: String,
    http: reqwest::Client,
    auth: AuthHeader,
}

impl AnthropicProvider {
    pub fn new(http: reqwest::Client) -> Self {
        Self::build(
            http,
            "https://api.anthropic.com".into(),
            AuthHeader::Passthrough,
        )
    }

    pub fn with_base_url(http: reqwest::Client, base_url: impl Into<String>) -> Self {
        Self::build(http, base_url.into(), AuthHeader::Passthrough)
    }

    pub fn with_auth(http: reqwest::Client, auth: AuthHeader) -> Self {
        Self::build(http, "https://api.anthropic.com".into(), auth)
    }

    pub fn configure(http: reqwest::Client, base_url: Option<String>, auth: AuthHeader) -> Self {
        Self::build(
            http,
            base_url.unwrap_or_else(|| "https://api.anthropic.com".into()),
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
impl Provider for AnthropicProvider {
    fn name(&self) -> &'static str {
        "anthropic"
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
            self.name(),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> AnthropicProvider {
        AnthropicProvider::new(reqwest::Client::new())
    }

    #[test]
    fn name_is_anthropic() {
        assert_eq!(provider().name(), "anthropic");
    }

    #[test]
    fn default_base_url_points_to_anthropic_api() {
        assert_eq!(provider().base_url, "https://api.anthropic.com");
    }

    #[test]
    fn with_base_url_overrides_default() {
        let p = AnthropicProvider::with_base_url(reqwest::Client::new(), "http://localhost:1234");
        assert_eq!(p.base_url, "http://localhost:1234");
    }

    #[test]
    fn default_auth_is_passthrough() {
        assert!(matches!(provider().auth, AuthHeader::Passthrough));
    }

    #[test]
    fn configure_sets_both_base_url_and_auth() {
        let p = AnthropicProvider::configure(
            reqwest::Client::new(),
            Some("http://x".into()),
            AuthHeader::ApiKey("sk-ant-test".into()),
        );
        assert_eq!(p.base_url, "http://x");
        assert!(matches!(p.auth, AuthHeader::ApiKey(_)));
    }

    #[tokio::test]
    async fn forward_openai_returns_error() {
        let p = AnthropicProvider::new(reqwest::Client::new());
        let result = p
            .forward_openai(
                "/v1/chat/completions",
                &HeaderMap::new(),
                Bytes::from_static(br#"{"model":"test"}"#),
                false,
            )
            .await;
        match result {
            Err(ProxyError::BadRequest(msg)) => {
                assert!(msg.contains("does not support OpenAI"), "got: {msg}");
            }
            _other => panic!("expected BadRequest"),
        }
    }
}
