//! Provider trait — what every upstream LLM API offers to the use case.

use crate::application::errors::ProxyError;
use crate::application::ports::{UpstreamResponse, UsageParser};
use crate::domain::UsageRecord;
use async_trait::async_trait;
use bytes::Bytes;
use http::HeaderMap;

/// The wire format a client or provider speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiFormat {
    Anthropic,
    OpenAI,
}

/// Translation direction derived from a (client format, upstream format) pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Passthrough,
    AnthropicToOpenAI,
    OpenAIToAnthropic,
}

impl Direction {
    pub fn from_pair(client: ApiFormat, upstream: ApiFormat) -> Self {
        match (client, upstream) {
            (ApiFormat::Anthropic, ApiFormat::Anthropic) | (ApiFormat::OpenAI, ApiFormat::OpenAI) => Self::Passthrough,
            (ApiFormat::Anthropic, ApiFormat::OpenAI) => Self::AnthropicToOpenAI,
            (ApiFormat::OpenAI, ApiFormat::Anthropic) => Self::OpenAIToAnthropic,
        }
    }

    pub fn as_label(&self) -> Option<&'static str> {
        match self {
            Self::Passthrough => None,
            Self::AnthropicToOpenAI => Some("anthropic→openai"),
            Self::OpenAIToAnthropic => Some("openai→anthropic"),
        }
    }
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> &'static str;

    /// Native API format this provider speaks. Used by the translation layer
    /// to decide whether to translate between Anthropic and OpenAI shapes.
    /// Defaults to Anthropic for compatibility with pre-translation tests.
    fn native_format(&self) -> ApiFormat {
        ApiFormat::Anthropic
    }
    fn parse_model(&self, body: &[u8]) -> Result<String, String>;
    fn usage_parser(&self) -> Box<dyn UsageParser>;
    fn parse_usage_json(&self, body: &[u8]) -> Result<UsageRecord, String>;

    /// SSE usage parser for OpenAI-format streams. Default falls back to the
    /// Anthropic parser; providers that speak OpenAI-flavoured endpoints
    /// (e.g. `/v1/chat/completions`) should override this.
    fn usage_parser_openai(&self) -> Box<dyn UsageParser> {
        self.usage_parser()
    }

    /// Parse a buffered OpenAI-format response body for token usage. Default
    /// falls back to the Anthropic parser; providers that speak OpenAI-flavoured
    /// endpoints should override this.
    fn parse_usage_json_openai(&self, body: &[u8]) -> Result<UsageRecord, String> {
        self.parse_usage_json(body)
    }

    async fn forward(
        &self,
        path: &str,
        headers: &HeaderMap,
        body: Bytes,
        streaming: bool,
    ) -> Result<UpstreamResponse, ProxyError>;

    /// Forward an OpenAI-format request (e.g. `/v1/chat/completions`).
    /// Default implementation returns an error — only providers with an
    /// OpenAI-compatible endpoint should override this.
    async fn forward_openai(
        &self,
        path: &str,
        headers: &HeaderMap,
        body: Bytes,
        streaming: bool,
    ) -> Result<UpstreamResponse, ProxyError> {
        let _ = (path, headers, body, streaming);
        Err(ProxyError::BadRequest(format!(
            "provider '{}' does not support OpenAI chat completions format",
            self.name()
        )))
    }
}

#[cfg(test)]
mod direction_tests {
    use super::*;

    #[test]
    fn direction_passthrough_when_formats_match() {
        assert_eq!(
            Direction::from_pair(ApiFormat::Anthropic, ApiFormat::Anthropic),
            Direction::Passthrough
        );
        assert_eq!(
            Direction::from_pair(ApiFormat::OpenAI, ApiFormat::OpenAI),
            Direction::Passthrough
        );
    }

    #[test]
    fn direction_picks_translator_when_formats_differ() {
        assert_eq!(
            Direction::from_pair(ApiFormat::Anthropic, ApiFormat::OpenAI),
            Direction::AnthropicToOpenAI
        );
        assert_eq!(
            Direction::from_pair(ApiFormat::OpenAI, ApiFormat::Anthropic),
            Direction::OpenAIToAnthropic
        );
    }

    #[test]
    fn direction_as_label() {
        assert_eq!(Direction::Passthrough.as_label(), None);
        assert_eq!(
            Direction::AnthropicToOpenAI.as_label(),
            Some("anthropic→openai")
        );
        assert_eq!(
            Direction::OpenAIToAnthropic.as_label(),
            Some("openai→anthropic")
        );
    }
}
