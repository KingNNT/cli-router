//! Provider trait — what every upstream LLM API offers to the use case.

use crate::application::errors::ProxyError;
use crate::application::ports::{UpstreamResponse, UsageParser};
use crate::domain::UsageRecord;
use async_trait::async_trait;
use bytes::Bytes;
use http::HeaderMap;

#[async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> &'static str;
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
