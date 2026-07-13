//! Kimi (Moonshot AI) provider — implements `Provider` against Moonshot's
//! Anthropic-compatible endpoint at `https://api.moonshot.ai/anthropic` and its
//! OpenAI-compatible endpoint at `https://api.moonshot.ai/v1`.
//!
//! Mirrors `ZaiProvider`: `native_format()` is `OpenAI`, so incoming Anthropic
//! requests are translated to OpenAI and sent to `/v1`; `forward()` still offers
//! the Anthropic passthrough path.

use super::messages_protocol::{self, AuthHeader};
use crate::application::errors::ProxyError;
use crate::application::ports::{ApiFormat, Provider, UpstreamResponse, UsageParser};
use crate::domain::UsageRecord;
use async_trait::async_trait;
use axum::http::HeaderMap;
use bytes::Bytes;

const DEFAULT_BASE_URL: &str = "https://api.moonshot.ai/anthropic";
const DEFAULT_OPENAI_BASE_URL: &str = "https://api.moonshot.ai/v1";

pub struct KimiProvider {
    base_url: String,
    openai_base_url: Option<String>,
    http: reqwest::Client,
    auth: AuthHeader,
}

impl KimiProvider {
    pub fn new(http: reqwest::Client) -> Self {
        Self::build(
            http,
            DEFAULT_BASE_URL.into(),
            Some(DEFAULT_OPENAI_BASE_URL.into()),
            AuthHeader::Passthrough,
        )
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
impl Provider for KimiProvider {
    fn name(&self) -> &'static str {
        "kimi"
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

    async fn forward_openai(
        &self,
        path: &str,
        headers: &HeaderMap,
        body: Bytes,
        streaming: bool,
    ) -> Result<UpstreamResponse, ProxyError> {
        let openai_base = self.openai_base_url.as_deref().ok_or_else(|| {
            ProxyError::BadRequest(
                "provider 'kimi' does not support OpenAI chat completions format".into(),
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
            self.name(),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_is_kimi() {
        let p = KimiProvider::new(reqwest::Client::new());
        assert_eq!(p.name(), "kimi");
    }

    #[test]
    fn native_format_is_openai() {
        let p = KimiProvider::new(reqwest::Client::new());
        assert_eq!(p.native_format(), ApiFormat::OpenAI);
    }

    #[test]
    fn configure_defaults_to_moonshot_ai() {
        let p = KimiProvider::configure(reqwest::Client::new(), None, None, AuthHeader::Passthrough);
        assert_eq!(p.base_url, "https://api.moonshot.ai/anthropic");
        assert_eq!(p.openai_base_url.as_deref(), Some("https://api.moonshot.ai/v1"));
    }
}
