//! Minimax provider — implements `Provider` against Minimax's Anthropic-compatible
//! endpoint at `https://api.minimaxi.com/anthropic`.
//!
//! Also supports OpenAI-format requests via `forward_openai()`, forwarding to
//! Minimax's OpenAI-compatible endpoint at `https://api.minimaxi.com/v1`.

use super::messages_protocol::{self, AuthHeader};
use crate::application::errors::ProxyError;
use crate::application::ports::{ApiFormat, Provider, UpstreamResponse, UsageParser};
use crate::domain::UsageRecord;
use async_trait::async_trait;
use axum::http::HeaderMap;
use bytes::Bytes;

const DEFAULT_BASE_URL: &str = "https://api.minimaxi.com/anthropic";
const DEFAULT_OPENAI_BASE_URL: &str = "https://api.minimaxi.com/v1";

pub struct MinimaxProvider {
    base_url: String,
    openai_base_url: Option<String>,
    http: reqwest::Client,
    auth: AuthHeader,
    thinking_mode: super::minimax_stream::ThinkingMode,
}

impl MinimaxProvider {
    pub fn new(http: reqwest::Client) -> Self {
        Self::build(
            http,
            DEFAULT_BASE_URL.into(),
            Some(DEFAULT_OPENAI_BASE_URL.into()),
            AuthHeader::Passthrough,
            super::minimax_stream::ThinkingMode::SplitOnly,
        )
    }

    pub fn with_base_url(http: reqwest::Client, base_url: impl Into<String>) -> Self {
        Self::build(
            http,
            base_url.into(),
            None,
            AuthHeader::Passthrough,
            super::minimax_stream::ThinkingMode::SplitOnly,
        )
    }

    pub fn with_auth(http: reqwest::Client, auth: AuthHeader) -> Self {
        Self::build(
            http,
            DEFAULT_BASE_URL.into(),
            Some(DEFAULT_OPENAI_BASE_URL.into()),
            auth,
            super::minimax_stream::ThinkingMode::SplitOnly,
        )
    }

    pub fn configure(
        http: reqwest::Client,
        base_url: Option<String>,
        openai_base_url: Option<String>,
        auth: AuthHeader,
        thinking_mode: super::minimax_stream::ThinkingMode,
    ) -> Self {
        Self::build(
            http,
            base_url.unwrap_or_else(|| DEFAULT_BASE_URL.into()),
            Some(openai_base_url.unwrap_or_else(|| DEFAULT_OPENAI_BASE_URL.into())),
            auth,
            thinking_mode,
        )
    }

    fn build(
        http: reqwest::Client,
        base_url: String,
        openai_base_url: Option<String>,
        auth: AuthHeader,
        thinking_mode: super::minimax_stream::ThinkingMode,
    ) -> Self {
        Self {
            base_url,
            openai_base_url,
            http,
            auth,
            thinking_mode,
        }
    }
}

#[async_trait]
impl Provider for MinimaxProvider {
    fn name(&self) -> &'static str {
        "minimax"
    }

    fn native_format(&self) -> ApiFormat {
        ApiFormat::OpenAI
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
                "provider 'minimax' does not support OpenAI chat completions format".into(),
            )
        })?;
        // Minimax uses Authorization: Bearer for both endpoints. Convert ApiKey → Bearer.
        let openai_auth = match &self.auth {
            AuthHeader::Passthrough => AuthHeader::Passthrough,
            AuthHeader::ApiKey(v) => AuthHeader::Bearer(v.clone()),
            other => other.clone(),
        };

        // Inject `reasoning_split: true` so MiniMax separates thinking content
        // into `reasoning_content` / `reasoning_details` fields instead of embedding
        // 思绪...半数 tags inside `content`. This makes it easier to strip cleanly.
        let outbound_body = inject_reasoning_split(&body);

        let resp = messages_protocol::forward(
            &self.http,
            openai_base,
            &openai_auth,
            path,
            headers,
            outbound_body,
            streaming,
            self.name(),
        )
        .await?;

        // Strip thinking content from the response according to configured mode.
        let mode = self.thinking_mode;
        match resp {
            UpstreamResponse::Buffered {
                status,
                headers,
                body,
                provider_id,
                translation_direction,
            } => {
                let cleaned =
                    super::minimax_stream::clean_thinking_buffered(&body, mode).unwrap_or(body);
                Ok(UpstreamResponse::Buffered {
                    status,
                    headers,
                    body: cleaned,
                    provider_id,
                    translation_direction,
                })
            }
            UpstreamResponse::Streaming {
                status,
                headers,
                body,
                provider_id,
                translation_direction,
            } => Ok(UpstreamResponse::Streaming {
                status,
                headers,
                body: super::minimax_stream::clean_thinking_stream(body, mode),
                provider_id,
                translation_direction,
            }),
        }
    }
}

/// Inject `reasoning_split: true` into an OpenAI-format request body.
/// Non-JSON or non-object bodies are returned verbatim.
fn inject_reasoning_split(body: &Bytes) -> Bytes {
    let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(body) else {
        return body.clone();
    };
    let Some(obj) = value.as_object_mut() else {
        return body.clone();
    };
    obj.insert("reasoning_split".to_string(), serde_json::Value::Bool(true));
    serde_json::to_vec(&value)
        .map(Bytes::from)
        .unwrap_or_else(|_| body.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> MinimaxProvider {
        MinimaxProvider::new(reqwest::Client::new())
    }

    #[test]
    fn name_is_minimax() {
        assert_eq!(provider().name(), "minimax");
    }

    #[test]
    fn default_base_url_points_to_minimax_anthropic_endpoint() {
        assert_eq!(provider().base_url, "https://api.minimaxi.com/anthropic");
    }

    #[test]
    fn default_openai_base_url_points_to_minimax_v1() {
        assert_eq!(
            provider().openai_base_url,
            Some("https://api.minimaxi.com/v1".into())
        );
    }

    #[test]
    fn with_base_url_overrides_default() {
        let p = MinimaxProvider::with_base_url(reqwest::Client::new(), "http://localhost:1234");
        assert_eq!(p.base_url, "http://localhost:1234");
        assert!(p.openai_base_url.is_none());
    }

    #[test]
    fn parses_model_id_from_body() {
        let body = br#"{"model":"MiniMax-M3","messages":[]}"#;
        assert_eq!(provider().parse_model(body).unwrap(), "MiniMax-M3");
    }

    #[test]
    fn default_auth_is_passthrough() {
        assert!(matches!(provider().auth, AuthHeader::Passthrough));
    }

    #[test]
    fn default_thinking_mode_is_split_only() {
        assert_eq!(
            provider().thinking_mode,
            super::super::minimax_stream::ThinkingMode::SplitOnly
        );
    }

    #[test]
    fn configure_sets_base_url_openai_base_url_and_auth() {
        let p = MinimaxProvider::configure(
            reqwest::Client::new(),
            Some("http://localhost:1234".into()),
            Some("http://localhost:9999".into()),
            AuthHeader::ApiKey("minimax-test".into()),
            super::super::minimax_stream::ThinkingMode::SplitOnly,
        );
        assert_eq!(p.base_url, "http://localhost:1234");
        assert_eq!(p.openai_base_url, Some("http://localhost:9999".into()));
        assert!(matches!(p.auth, AuthHeader::ApiKey(_)));
    }

    #[test]
    fn configure_uses_default_urls_when_none() {
        let p = MinimaxProvider::configure(
            reqwest::Client::new(),
            None,
            None,
            AuthHeader::Passthrough,
            super::super::minimax_stream::ThinkingMode::SplitOnly,
        );
        assert_eq!(p.base_url, "https://api.minimaxi.com/anthropic");
        assert_eq!(
            p.openai_base_url,
            Some("https://api.minimaxi.com/v1".into())
        );
    }

    #[test]
    fn inject_reasoning_split_adds_field() {
        let body = Bytes::from(r#"{"model":"MiniMax-M3","messages":[]}"#);
        let result = inject_reasoning_split(&body);
        let parsed: serde_json::Value = serde_json::from_slice(&result).unwrap();
        assert_eq!(parsed["reasoning_split"], true);
    }

    #[test]
    fn inject_reasoning_split_preserves_existing_fields() {
        let body = Bytes::from(r#"{"model":"MiniMax-M3","messages":[],"stream":true}"#);
        let result = inject_reasoning_split(&body);
        let parsed: serde_json::Value = serde_json::from_slice(&result).unwrap();
        assert_eq!(parsed["model"], "MiniMax-M3");
        assert_eq!(parsed["stream"], true);
        assert_eq!(parsed["reasoning_split"], true);
    }

    #[test]
    fn inject_reasoning_split_overwrites_false() {
        let body = Bytes::from(r#"{"model":"MiniMax-M3","reasoning_split":false}"#);
        let result = inject_reasoning_split(&body);
        let parsed: serde_json::Value = serde_json::from_slice(&result).unwrap();
        assert_eq!(parsed["reasoning_split"], true);
    }

    #[test]
    fn inject_reasoning_split_returns_non_json_unchanged() {
        let body = Bytes::from("not json");
        let result = inject_reasoning_split(&body);
        assert_eq!(result, body);
    }

    #[test]
    fn inject_reasoning_split_returns_json_non_object_unchanged() {
        let body = Bytes::from("[1, 2, 3]");
        let result = inject_reasoning_split(&body);
        assert_eq!(result, body);
    }

    #[test]
    fn forward_openai_strips_thinking_from_buffered_response() {
        // Simulate what MiniMax returns with reasoning_split: true
        let fake_response = r#"{"id":"chatcmpl-1","object":"chat.completion","model":"MiniMax-M3","choices":[{"index":0,"message":{"role":"assistant","content":"Hello!","reasoning_content":"The user said hi.","reasoning_details":[{"type":"text","text":"The user said hi."}]},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#;
        let body = Bytes::from(fake_response);
        let cleaned =
            crate::adapters::providers::minimax_stream::strip_thinking_buffered(&body).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&cleaned).unwrap();
        assert_eq!(parsed["choices"][0]["message"]["content"], "Hello!");
        assert!(
            parsed["choices"][0]["message"]
                .get("reasoning_content")
                .is_none()
        );
        assert!(
            parsed["choices"][0]["message"]
                .get("reasoning_details")
                .is_none()
        );
        assert_eq!(parsed["model"], "MiniMax-M3");
        assert_eq!(parsed["usage"]["total_tokens"], 15);
    }
}
