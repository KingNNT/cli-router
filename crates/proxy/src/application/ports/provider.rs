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
