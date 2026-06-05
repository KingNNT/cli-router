//! DeepSeek provider — implements `Provider` against DeepSeek's
//! OpenAI-compatible endpoint at `https://api.deepseek.com/v1`.
//!
//! DeepSeek only speaks the OpenAI chat-completions format, so
//! `forward_openai()` delegates to the shared `messages_protocol::forward()`
//! helper and `forward()` (Anthropic path) returns an error.
//!
//! # Reasoner models and `tool_choice`
//!
//! DeepSeek's reasoner models (e.g. `deepseek-reasoner`, `deepseek-v4-pro`)
//! do **not** support the `tool_choice` parameter.  When the incoming request
//! includes `tool_choice`, we strip it from the body before forwarding so the
//! upstream does not return a 400 error.

use super::messages_protocol::{self, AuthHeader};
use crate::application::errors::ProxyError;
use crate::application::ports::{ApiFormat, Provider, UpstreamResponse, UsageParser};
use crate::domain::UsageRecord;
use async_trait::async_trait;
use axum::http::HeaderMap;
use bytes::Bytes;
use serde_json::Value;

const DEFAULT_BASE_URL: &str = "https://api.deepseek.com/v1";

/// Models that support `tool_choice`.  Only `deepseek-chat` (V3) is known to
/// support it; all other models (reasoner, V4-Pro, etc.) reject it with 400.
const TOOL_CHOICE_SUPPORTED_MODELS: &[&str] = &["deepseek-chat"];

pub struct DeepSeekProvider {
    base_url: String,
    http: reqwest::Client,
    auth: AuthHeader,
}

impl DeepSeekProvider {
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

    /// Strip `tool_choice` from the request body when the target model is a
    /// DeepSeek reasoner model that does not support it.  Returns the
    /// (possibly modified) body bytes.
    fn strip_tool_choice_if_unsupported(body: Bytes) -> Bytes {
        // Fast path: if body can't be parsed as JSON, forward as-is.
        let Ok(mut v) = serde_json::from_slice::<Value>(&body) else {
            return body;
        };

        let model = v
            .get("model")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .to_string();

        // Only `deepseek-chat` supports `tool_choice`.  For everything else
        // (deepseek-reasoner, deepseek-v4-pro, etc.) strip it.
        if TOOL_CHOICE_SUPPORTED_MODELS.contains(&model.as_str()) {
            return body;
        }

        let Some(obj) = v.as_object_mut() else {
            return body;
        };

        if obj.remove("tool_choice").is_some() {
            tracing::debug!(
                model = %model,
                "stripped unsupported 'tool_choice' from DeepSeek reasoner request"
            );
            Bytes::from(serde_json::to_vec(obj).unwrap_or_else(|e| {
                tracing::warn!(error = %e, "failed to re-serialize body after stripping tool_choice");
                body.to_vec()
            }))
        } else {
            body
        }
    }
}

#[async_trait]
impl Provider for DeepSeekProvider {
    fn name(&self) -> &'static str {
        "deepseek"
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
            "provider 'deepseek' does not support Anthropic messages format; use the OpenAI-compatible endpoint".into(),
        ))
    }

    async fn forward_openai(
        &self,
        path: &str,
        headers: &HeaderMap,
        body: Bytes,
        streaming: bool,
    ) -> Result<UpstreamResponse, ProxyError> {
        let body = Self::strip_tool_choice_if_unsupported(body);
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
    fn name_is_deepseek() {
        let p = DeepSeekProvider::new(reqwest::Client::new());
        assert_eq!(p.name(), "deepseek");
    }

    #[test]
    fn native_format_is_openai() {
        let p = DeepSeekProvider::new(reqwest::Client::new());
        assert_eq!(p.native_format(), ApiFormat::OpenAI);
    }

    #[test]
    fn default_base_url_points_to_deepseek() {
        let p = DeepSeekProvider::new(reqwest::Client::new());
        assert_eq!(p.base_url, "https://api.deepseek.com/v1");
    }

    #[test]
    fn with_base_url_overrides_default() {
        let p = DeepSeekProvider::with_base_url(reqwest::Client::new(), "http://localhost:1234");
        assert_eq!(p.base_url, "http://localhost:1234");
    }

    #[test]
    fn default_auth_is_passthrough() {
        let p = DeepSeekProvider::new(reqwest::Client::new());
        assert!(matches!(p.auth, AuthHeader::Passthrough));
    }

    #[test]
    fn configure_sets_base_url_and_auth() {
        let p = DeepSeekProvider::configure(
            reqwest::Client::new(),
            Some("http://localhost:1234".into()),
            AuthHeader::Bearer("sk-test".into()),
        );
        assert_eq!(p.base_url, "http://localhost:1234");
        assert!(matches!(p.auth, AuthHeader::Bearer(_)));
    }

    #[test]
    fn configure_uses_default_base_url_when_none() {
        let p = DeepSeekProvider::configure(reqwest::Client::new(), None, AuthHeader::Passthrough);
        assert_eq!(p.base_url, "https://api.deepseek.com/v1");
    }

    #[test]
    fn parses_model_from_body() {
        let body = br#"{"model":"deepseek-chat","messages":[]}"#;
        assert_eq!(
            DeepSeekProvider::new(reqwest::Client::new())
                .parse_model(body)
                .unwrap(),
            "deepseek-chat"
        );
    }

    #[test]
    fn strip_tool_choice_removes_from_reasoner_model() {
        let body = Bytes::from(r#"{"model":"deepseek-v4-pro","messages":[],"tool_choice":"auto"}"#);
        let stripped = DeepSeekProvider::strip_tool_choice_if_unsupported(body);
        let v: Value = serde_json::from_slice(&stripped).unwrap();
        assert!(
            v.get("tool_choice").is_none(),
            "tool_choice should be removed"
        );
        assert_eq!(v["model"], "deepseek-v4-pro");
    }

    #[test]
    fn strip_tool_choice_removes_from_deepseek_reasoner() {
        let body = Bytes::from(
            r#"{"model":"deepseek-reasoner","messages":[],"tool_choice":{"type":"function","function":{"name":"my_tool"}}}"#,
        );
        let stripped = DeepSeekProvider::strip_tool_choice_if_unsupported(body);
        let v: Value = serde_json::from_slice(&stripped).unwrap();
        assert!(v.get("tool_choice").is_none());
    }

    #[test]
    fn strip_tool_choice_keeps_for_deepseek_chat() {
        let body = Bytes::from(r#"{"model":"deepseek-chat","messages":[],"tool_choice":"auto"}"#);
        let stripped = DeepSeekProvider::strip_tool_choice_if_unsupported(body);
        let v: Value = serde_json::from_slice(&stripped).unwrap();
        assert_eq!(
            v["tool_choice"], "auto",
            "tool_choice should be kept for deepseek-chat"
        );
    }

    #[test]
    fn strip_tool_choice_noop_when_absent() {
        let body = Bytes::from(r#"{"model":"deepseek-v4-pro","messages":[]}"#);
        let stripped = DeepSeekProvider::strip_tool_choice_if_unsupported(body);
        let v: Value = serde_json::from_slice(&stripped).unwrap();
        assert!(v.get("tool_choice").is_none());
        assert_eq!(v["model"], "deepseek-v4-pro");
    }

    #[test]
    fn strip_tool_choice_noop_for_invalid_json() {
        let body = Bytes::from_static(b"not json");
        let stripped = DeepSeekProvider::strip_tool_choice_if_unsupported(body);
        assert_eq!(&stripped[..], b"not json");
    }
}
