//! Z.ai provider — implements `Provider` against Z.ai's Anthropic-compatible
//! endpoint at `https://api.z.ai/api/anthropic`.
//!
//! Also supports OpenAI-format requests via `forward_openai()`, forwarding to
//! Z.ai's OpenAI-compatible endpoint at `https://api.z.ai/api/paas/v4`.

use super::messages_protocol::{self, AuthHeader};
use crate::application::errors::ProxyError;
use crate::application::ports::{Provider, UpstreamResponse, UsageParser};
use crate::domain::UsageRecord;
use async_trait::async_trait;
use axum::http::HeaderMap;
use bytes::Bytes;

const DEFAULT_BASE_URL: &str = "https://api.z.ai/api/anthropic";
const DEFAULT_OPENAI_BASE_URL: &str = "https://api.z.ai/api/paas/v4";

pub struct ZaiProvider {
    base_url: String,
    openai_base_url: Option<String>,
    http: reqwest::Client,
    auth: AuthHeader,
}

impl ZaiProvider {
    pub fn new(http: reqwest::Client) -> Self {
        Self::build(
            http,
            DEFAULT_BASE_URL.into(),
            Some(DEFAULT_OPENAI_BASE_URL.into()),
            AuthHeader::Passthrough,
        )
    }

    pub fn with_base_url(http: reqwest::Client, base_url: impl Into<String>) -> Self {
        Self::build(http, base_url.into(), None, AuthHeader::Passthrough)
    }

    pub fn with_auth(http: reqwest::Client, auth: AuthHeader) -> Self {
        Self::build(
            http,
            DEFAULT_BASE_URL.into(),
            Some(DEFAULT_OPENAI_BASE_URL.into()),
            auth,
        )
    }

    pub fn configure(
        http: reqwest::Client,
        base_url: Option<String>,
        openai_base_url: Option<String>,
        auth: AuthHeader,
    ) -> Self {
        Self::build(
            http,
            base_url.unwrap_or_else(|| DEFAULT_BASE_URL.into()),
            Some(openai_base_url.unwrap_or_else(|| DEFAULT_OPENAI_BASE_URL.into())),
            auth,
        )
    }

    fn build(
        http: reqwest::Client,
        base_url: String,
        openai_base_url: Option<String>,
        auth: AuthHeader,
    ) -> Self {
        Self {
            base_url,
            openai_base_url,
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

    async fn forward_openai(
        &self,
        path: &str,
        headers: &HeaderMap,
        body: Bytes,
        streaming: bool,
    ) -> Result<UpstreamResponse, ProxyError> {
        let openai_base = self.openai_base_url.as_deref().ok_or_else(|| {
            ProxyError::BadRequest(
                "provider 'zai' does not support OpenAI chat completions format".into(),
            )
        })?;
        // OpenAI format uses Authorization: Bearer. Convert ApiKey → Bearer.
        let openai_auth = match &self.auth {
            AuthHeader::Passthrough => AuthHeader::Passthrough,
            AuthHeader::ApiKey(v) => AuthHeader::Bearer(v.clone()),
            other => other.clone(),
        };
        messages_protocol::forward(
            &self.http,
            openai_base,
            &openai_auth,
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
    fn default_openai_base_url_points_to_zai_paas() {
        assert_eq!(
            provider().openai_base_url,
            Some("https://api.z.ai/api/paas/v4".into())
        );
    }

    #[test]
    fn with_base_url_overrides_default() {
        let p = ZaiProvider::with_base_url(reqwest::Client::new(), "http://localhost:1234");
        assert_eq!(p.base_url, "http://localhost:1234");
        assert!(p.openai_base_url.is_none());
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
    fn configure_sets_base_url_openai_base_url_and_auth() {
        let p = ZaiProvider::configure(
            reqwest::Client::new(),
            Some("http://localhost:1234".into()),
            Some("http://localhost:9999".into()),
            AuthHeader::ApiKey("zai-test".into()),
        );
        assert_eq!(p.base_url, "http://localhost:1234");
        assert_eq!(p.openai_base_url, Some("http://localhost:9999".into()));
        assert!(matches!(p.auth, AuthHeader::ApiKey(_)));
    }

    #[test]
    fn configure_uses_default_openai_base_url_when_none() {
        let p = ZaiProvider::configure(reqwest::Client::new(), None, None, AuthHeader::Passthrough);
        assert_eq!(p.base_url, "https://api.z.ai/api/anthropic");
        assert_eq!(
            p.openai_base_url,
            Some("https://api.z.ai/api/paas/v4".into())
        );
    }
}
