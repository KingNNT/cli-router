//! Kimi (Moonshot AI) provider — implements `Provider` against Moonshot's
//! Anthropic-compatible endpoint at `https://api.moonshot.ai/anthropic` and its
//! OpenAI-compatible endpoint at `https://api.moonshot.ai/v1`.
//!
//! Follows the dual-endpoint shape of `ZaiProvider`: `native_format()` is
//! `OpenAI`, so incoming Anthropic requests are translated to OpenAI and sent to
//! `/v1`; `forward()` still offers the Anthropic passthrough path. The usage
//! parsers use the OpenAI variants (matching `native_format()`, as `DeepSeek`
//! does) rather than Zai's Anthropic variants.

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
    sanitize_empty_tools: bool,
}

impl KimiProvider {
    pub fn new(http: reqwest::Client) -> Self {
        Self::build(
            http,
            DEFAULT_BASE_URL.into(),
            Some(DEFAULT_OPENAI_BASE_URL.into()),
            AuthHeader::Passthrough,
            false,
        )
    }

    pub fn with_auth(http: reqwest::Client, auth: AuthHeader) -> Self {
        Self::build(
            http,
            DEFAULT_BASE_URL.into(),
            Some(DEFAULT_OPENAI_BASE_URL.into()),
            auth,
            false,
        )
    }

    pub fn configure(
        http: reqwest::Client,
        base_url: Option<String>,
        openai_base_url: Option<String>,
        auth: AuthHeader,
        sanitize_empty_tools: bool,
    ) -> Self {
        Self::build(
            http,
            base_url.unwrap_or_else(|| DEFAULT_BASE_URL.into()),
            Some(openai_base_url.unwrap_or_else(|| DEFAULT_OPENAI_BASE_URL.into())),
            auth,
            sanitize_empty_tools,
        )
    }

    fn build(
        http: reqwest::Client,
        base_url: String,
        openai_base_url: Option<String>,
        auth: AuthHeader,
        sanitize_empty_tools: bool,
    ) -> Self {
        Self {
            base_url,
            openai_base_url,
            http,
            auth,
            sanitize_empty_tools,
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
        let resp = messages_protocol::forward(
            &self.http,
            &self.base_url,
            &self.auth,
            path,
            headers,
            body,
            streaming,
            self.name(),
        )
        .await?;

        if !self.sanitize_empty_tools {
            return Ok(resp);
        }
        // Only sanitize successful responses; error bodies pass through.
        Ok(match resp {
            UpstreamResponse::Buffered {
                status,
                headers,
                body,
                provider_id,
                translation_direction,
            } if (200..300).contains(&status) => UpstreamResponse::Buffered {
                status,
                headers,
                body: super::tool_sanitizer::sanitize_buffered(&body),
                provider_id,
                translation_direction,
            },
            UpstreamResponse::Streaming {
                status,
                headers,
                body,
                provider_id,
                translation_direction,
            } if (200..300).contains(&status) => UpstreamResponse::Streaming {
                status,
                headers,
                body: super::tool_sanitizer::sanitize_stream(body),
                provider_id,
                translation_direction,
            },
            other => other,
        })
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
    use crate::application::ports::UpstreamResponse;
    use futures::StreamExt;

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
        let p = KimiProvider::configure(
            reqwest::Client::new(),
            None,
            None,
            AuthHeader::Passthrough,
            false,
        );
        assert_eq!(p.base_url, "https://api.moonshot.ai/anthropic");
        assert_eq!(
            p.openai_base_url.as_deref(),
            Some("https://api.moonshot.ai/v1")
        );
    }

    #[test]
    fn configure_sets_sanitize_flag() {
        let p = KimiProvider::configure(
            reqwest::Client::new(),
            None,
            None,
            AuthHeader::Passthrough,
            true,
        );
        assert!(p.sanitize_empty_tools);
    }

    #[tokio::test]
    async fn forward_streaming_drops_noop_tool_and_forces_end_turn() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let sse = concat!(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"model\":\"kimi\",\"content\":[]}}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"t1\",\"name\":\"bash\",\"input\":{}}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"command\\\":\\\":\\\"}\"}}\n\n",
            "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":3}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        );
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(sse),
            )
            .mount(&server)
            .await;

        let provider = KimiProvider::configure(
            reqwest::Client::new(),
            Some(server.uri()),
            None,
            AuthHeader::Passthrough,
            true, // sanitize ON
        );
        let resp = provider
            .forward("/v1/messages", &HeaderMap::new(), Bytes::from("{}"), true)
            .await
            .unwrap();

        let collected = collect_streaming_body(resp).await;
        assert!(
            !collected.contains("tool_use"),
            "no-op tool_use must be dropped: {collected}"
        );
        assert!(
            collected.contains("\"stop_reason\":\"end_turn\""),
            "stop_reason must be rewritten: {collected}"
        );
    }

    #[tokio::test]
    async fn forward_streaming_passthrough_when_flag_off() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let sse = concat!(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"model\":\"kimi\",\"content\":[]}}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"t1\",\"name\":\"bash\",\"input\":{}}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"command\\\":\\\":\\\"}\"}}\n\n",
            "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":3}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        );
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(sse),
            )
            .mount(&server)
            .await;

        let provider = KimiProvider::configure(
            reqwest::Client::new(),
            Some(server.uri()),
            None,
            AuthHeader::Passthrough,
            false, // sanitize OFF
        );
        let resp = provider
            .forward("/v1/messages", &HeaderMap::new(), Bytes::from("{}"), true)
            .await
            .unwrap();

        let collected = collect_streaming_body(resp).await;
        assert!(
            collected.contains("tool_use"),
            "flag off must pass tool_use through"
        );
        assert!(
            collected.contains("\"stop_reason\":\"tool_use\""),
            "flag off must keep stop_reason"
        );
    }

    // Helper: drain a Streaming UpstreamResponse into a String.
    async fn collect_streaming_body(resp: UpstreamResponse) -> String {
        match resp {
            UpstreamResponse::Streaming { body, .. } => {
                let mut s = String::new();
                let mut body = body;
                while let Some(chunk) = body.next().await {
                    s.push_str(&String::from_utf8_lossy(&chunk.unwrap()));
                }
                s
            }
            UpstreamResponse::Buffered { body, .. } => String::from_utf8_lossy(&body).to_string(),
        }
    }
}
