//! OpenAI provider adapter.
//!
//! Supports both API key (Bearer) and Codex OAuth authentication.
//! Speaks the OpenAI `/v1/chat/completions` protocol natively.

use super::messages_protocol::{self, AuthHeader};
use crate::application::errors::ProxyError;
use crate::application::ports::{ApiFormat, Provider, UpstreamResponse, UsageParser};
use crate::domain::UsageRecord;
use async_trait::async_trait;
use axum::http::HeaderMap;
use bytes::Bytes;

const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

pub struct OpenAiProvider {
    pub(crate) base_url: String,
    pub(crate) http: reqwest::Client,
    pub(crate) auth: AuthHeader,
}

impl OpenAiProvider {
    pub fn new(http: reqwest::Client) -> Self {
        Self::build(http, DEFAULT_BASE_URL.into(), AuthHeader::Passthrough)
    }

    pub fn with_base_url(http: reqwest::Client, base_url: impl Into<String>) -> Self {
        Self::build(http, base_url.into(), AuthHeader::Passthrough)
    }

    pub fn with_auth(http: reqwest::Client, auth: AuthHeader) -> Self {
        Self::build(http, DEFAULT_BASE_URL.into(), auth)
    }

    pub fn configure(http: reqwest::Client, base_url: Option<String>, auth: AuthHeader) -> Self {
        Self::build(
            http,
            base_url.unwrap_or_else(|| DEFAULT_BASE_URL.into()),
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
impl Provider for OpenAiProvider {
    fn name(&self) -> &str {
        "openai"
    }

    fn native_format(&self) -> ApiFormat {
        ApiFormat::OpenAI
    }

    fn parse_model(&self, body: &[u8]) -> Result<String, String> {
        messages_protocol::parse_model(body)
    }

    fn usage_parser(&self) -> Box<dyn UsageParser> {
        messages_protocol::openai_usage_parser()
    }

    fn parse_usage_json(&self, body: &[u8]) -> Result<UsageRecord, String> {
        messages_protocol::parse_openai_usage_json(body)
    }

    fn usage_parser_openai(&self) -> Box<dyn UsageParser> {
        messages_protocol::openai_usage_parser()
    }

    fn parse_usage_json_openai(&self, body: &[u8]) -> Result<UsageRecord, String> {
        messages_protocol::parse_openai_usage_json(body)
    }

    async fn forward(
        &self,
        path: &str,
        headers: &HeaderMap,
        body: Bytes,
        streaming: bool,
    ) -> Result<UpstreamResponse, ProxyError> {
        let _ = (path, headers, body, streaming);
        Err(ProxyError::BadRequest(
            "provider 'openai' does not support Anthropic messages format; use the OpenAI-compatible endpoint".into(),
        ))
    }

    async fn forward_openai(
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

    #[test]
    fn name_is_openai() {
        let p = OpenAiProvider::new(reqwest::Client::new());
        assert_eq!(p.name(), "openai");
    }

    #[test]
    fn native_format_is_openai() {
        let p = OpenAiProvider::new(reqwest::Client::new());
        assert_eq!(p.native_format(), ApiFormat::OpenAI);
    }

    #[test]
    fn default_base_url_points_to_openai_api() {
        let p = OpenAiProvider::new(reqwest::Client::new());
        assert_eq!(p.base_url, "https://api.openai.com/v1");
    }

    #[test]
    fn with_base_url_overrides_default() {
        let p = OpenAiProvider::with_base_url(reqwest::Client::new(), "http://localhost:1234");
        assert_eq!(p.base_url, "http://localhost:1234");
    }

    #[test]
    fn default_auth_is_passthrough() {
        let p = OpenAiProvider::new(reqwest::Client::new());
        assert!(matches!(p.auth, AuthHeader::Passthrough));
    }

    #[test]
    fn configure_sets_base_url_and_auth() {
        let p = OpenAiProvider::configure(
            reqwest::Client::new(),
            Some("http://localhost:1234".into()),
            AuthHeader::Bearer("sk-test".into()),
        );
        assert_eq!(p.base_url, "http://localhost:1234");
        assert!(matches!(p.auth, AuthHeader::Bearer(_)));
    }

    #[test]
    fn configure_uses_default_base_url_when_none() {
        let p = OpenAiProvider::configure(reqwest::Client::new(), None, AuthHeader::Passthrough);
        assert_eq!(p.base_url, "https://api.openai.com/v1");
    }

    #[test]
    fn parses_model_from_body() {
        let body = br#"{"model":"gpt-4o","messages":[]}"#;
        assert_eq!(
            OpenAiProvider::new(reqwest::Client::new())
                .parse_model(body)
                .unwrap(),
            "gpt-4o"
        );
    }
}
