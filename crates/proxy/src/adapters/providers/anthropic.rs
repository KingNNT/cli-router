//! Anthropic provider — implements `Provider` against `https://api.anthropic.com`.
//!
//! Protocol-level logic (parse_model, parse_usage_json, usage_parser, forward)
//! is delegated to `super::messages_protocol`, which is shared with `ZaiProvider`.

use super::messages_protocol::{self, AuthHeader};
use crate::application::errors::ProxyError;
use crate::application::ports::{ApiFormat, Provider, UpstreamResponse, UsageParser};
use crate::domain::UsageRecord;
use async_trait::async_trait;
use axum::http::HeaderMap;
use bytes::Bytes;
use serde_json::Value;

pub struct AnthropicProvider {
    base_url: String,
    http: reqwest::Client,
    auth: AuthHeader,
    default_effort: Option<String>,
}

impl AnthropicProvider {
    pub fn new(http: reqwest::Client) -> Self {
        Self::build(
            http,
            "https://api.anthropic.com".into(),
            AuthHeader::Passthrough,
            None,
        )
    }

    pub fn with_base_url(http: reqwest::Client, base_url: impl Into<String>) -> Self {
        Self::build(http, base_url.into(), AuthHeader::Passthrough, None)
    }

    pub fn with_auth(http: reqwest::Client, auth: AuthHeader) -> Self {
        Self::build(http, "https://api.anthropic.com".into(), auth, None)
    }

    pub fn configure(http: reqwest::Client, base_url: Option<String>, auth: AuthHeader) -> Self {
        Self::build(
            http,
            base_url.unwrap_or_else(|| "https://api.anthropic.com".into()),
            auth,
            None,
        )
    }

    pub fn configure_with_effort(
        http: reqwest::Client,
        base_url: Option<String>,
        auth: AuthHeader,
        default_effort: Option<String>,
    ) -> Self {
        Self::build(
            http,
            base_url.unwrap_or_else(|| "https://api.anthropic.com".into()),
            auth,
            default_effort,
        )
    }

    fn build(
        http: reqwest::Client,
        base_url: String,
        auth: AuthHeader,
        default_effort: Option<String>,
    ) -> Self {
        Self {
            base_url,
            http,
            auth,
            default_effort,
        }
    }
}

/// Inject `output_config.effort` into an Anthropic Messages API request body
/// if the provider has a default effort configured AND the request doesn't
/// already specify one. The Anthropic effort parameter lives at:
/// ```json
/// { "output_config": { "effort": "medium" } }
/// ```
///
/// When both are present the request's explicit value wins.
fn inject_effort(body: &mut Bytes, default_effort: Option<&str>) {
    let Some(effort) = default_effort else {
        return;
    };

    let Ok(mut parsed) = serde_json::from_slice::<Value>(body) else {
        return;
    };

    let already_set = parsed
        .get("output_config")
        .and_then(|oc| oc.get("effort"))
        .and_then(|e| e.as_str())
        .is_some_and(|s| !s.is_empty());

    if already_set {
        return;
    }

    // Merge effort into existing output_config or create a new one.
    let oc = parsed
        .as_object_mut()
        .unwrap()
        .entry("output_config")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));

    if let Some(oc_obj) = oc.as_object_mut() {
        oc_obj.insert("effort".into(), Value::String(effort.to_string()));
    }

    if let Ok(reencoded) = serde_json::to_vec(&parsed) {
        *body = Bytes::from(reencoded);
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    fn native_format(&self) -> ApiFormat {
        ApiFormat::Anthropic
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
        mut body: Bytes,
        streaming: bool,
    ) -> Result<UpstreamResponse, ProxyError> {
        inject_effort(&mut body, self.default_effort.as_deref());
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

    #[test]
    fn configure_with_effort_stores_default() {
        let p = AnthropicProvider::configure_with_effort(
            reqwest::Client::new(),
            None,
            AuthHeader::Passthrough,
            Some("high".into()),
        );
        assert_eq!(p.default_effort.as_deref(), Some("high"));
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

    // -- inject_effort tests --

    #[test]
    fn inject_effort_adds_output_config_when_absent() {
        let mut body = Bytes::from(r#"{"model":"claude-opus-4-8","messages":[]}"#);
        inject_effort(&mut body, Some("medium"));
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["output_config"]["effort"], "medium");
    }

    #[test]
    fn inject_effort_merges_into_existing_output_config() {
        let mut body =
            Bytes::from(r#"{"model":"claude-opus-4-8","output_config":{"other_key":true}}"#);
        inject_effort(&mut body, Some("high"));
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["output_config"]["effort"], "high");
        assert_eq!(parsed["output_config"]["other_key"], true);
    }

    #[test]
    fn inject_effort_does_not_override_existing_effort() {
        let mut body =
            Bytes::from(r#"{"model":"claude-opus-4-8","output_config":{"effort":"low"}}"#);
        inject_effort(&mut body, Some("high"));
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["output_config"]["effort"], "low");
    }

    #[test]
    fn inject_effort_noop_when_no_default() {
        let original = r#"{"model":"claude-opus-4-8","messages":[]}"#;
        let mut body = Bytes::from(original);
        inject_effort(&mut body, None);
        assert_eq!(&body[..], original.as_bytes());
    }

    #[test]
    fn inject_effort_handles_invalid_json_gracefully() {
        let original = r#"not json"#;
        let mut body = Bytes::from(original);
        inject_effort(&mut body, Some("high"));
        assert_eq!(&body[..], original.as_bytes());
    }
}
