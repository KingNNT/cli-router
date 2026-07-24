use super::messages_protocol::AuthHeader;
use super::minimax_stream::ThinkingMode;
use crate::application::errors::ProxyError;
use crate::application::ports::{FormatSupport, Provider, UpstreamResponse, UsageParser};
use crate::config::ProviderKind;
use crate::domain::UsageRecord;
use async_trait::async_trait;
use axum::http::HeaderMap;
use bytes::Bytes;

/// Per-provider request/response behaviors on the forward path, as data.
#[derive(Debug, Clone, Default)]
pub struct Quirks {
    /// Inject `reasoning_split: true` into the OpenAI request body (MiniMax).
    pub reasoning_split: bool,
    /// Filter thinking content from the OpenAI response (MiniMax).
    pub strip_thinking: Option<ThinkingMode>,
    /// Reorder tool responses after assistant tool_calls in the OpenAI request
    /// (DeepSeek).
    pub reorder_tool_responses: bool,
    /// Sanitize empty-tool responses (Kimi).
    pub sanitize_empty_tools: bool,
    /// Inject reasoning effort into the Anthropic request output config
    /// (Anthropic).
    pub reasoning_effort: Option<String>,
}

impl Quirks {
    pub fn none() -> Self {
        Self::default()
    }
}

/// Build the `Quirks` for a kind, folding in the config-overridable fields.
pub fn quirks_for(
    kind: ProviderKind,
    thinking: ThinkingMode,
    reasoning_effort: Option<String>,
    sanitize_empty_tools: bool,
) -> Quirks {
    match kind {
        ProviderKind::Minimax => Quirks {
            reasoning_split: true,
            strip_thinking: Some(thinking),
            ..Quirks::none()
        },
        ProviderKind::DeepSeek => Quirks {
            reorder_tool_responses: true,
            ..Quirks::none()
        },
        ProviderKind::Kimi => Quirks {
            sanitize_empty_tools,
            ..Quirks::none()
        },
        ProviderKind::Anthropic => Quirks {
            reasoning_effort,
            ..Quirks::none()
        },
        ProviderKind::Zai | ProviderKind::OpenAi => Quirks::none(),
        // Codex is bespoke and never built as an UpstreamProvider.
        ProviderKind::Codex => Quirks::none(),
    }
}

#[allow(dead_code)]
pub struct UpstreamProvider {
    name: String,
    anthropic_base_url: Option<String>,
    openai_base_url: Option<String>,
    auth: AuthHeader,
    quirks: Quirks,
    http: reqwest::Client,
}

impl UpstreamProvider {
    pub fn new(
        name: String,
        anthropic_base_url: Option<String>,
        openai_base_url: Option<String>,
        auth: AuthHeader,
        quirks: Quirks,
        http: reqwest::Client,
    ) -> Self {
        Self {
            name,
            anthropic_base_url,
            openai_base_url,
            auth,
            quirks,
            http,
        }
    }
}

#[async_trait]
impl Provider for UpstreamProvider {
    fn name(&self) -> &str {
        self.name.as_str()
    }

    fn supported_formats(&self) -> FormatSupport {
        FormatSupport {
            anthropic: self
                .anthropic_base_url
                .as_deref()
                .is_some_and(|s| !s.is_empty()),
            openai: self
                .openai_base_url
                .as_deref()
                .is_some_and(|s| !s.is_empty()),
        }
    }

    // parse_model / usage parsers / forward / forward_openai — Task 4.
    fn parse_model(&self, body: &[u8]) -> Result<String, String> {
        super::messages_protocol::parse_model(body)
    }
    fn usage_parser(&self) -> Box<dyn UsageParser> {
        super::messages_protocol::usage_parser()
    }
    fn parse_usage_json(&self, body: &[u8]) -> Result<UsageRecord, String> {
        super::messages_protocol::parse_usage_json(body)
    }
    fn usage_parser_openai(&self) -> Box<dyn UsageParser> {
        super::messages_protocol::openai_usage_parser()
    }
    fn parse_usage_json_openai(&self, body: &[u8]) -> Result<UsageRecord, String> {
        super::messages_protocol::parse_openai_usage_json(body)
    }
    async fn forward(
        &self,
        path: &str,
        headers: &HeaderMap,
        body: Bytes,
        streaming: bool,
    ) -> Result<UpstreamResponse, ProxyError> {
        let _ = (path, headers, body, streaming);
        Err(ProxyError::BadRequest("not yet implemented".into()))
    }
    async fn forward_openai(
        &self,
        path: &str,
        headers: &HeaderMap,
        body: Bytes,
        streaming: bool,
    ) -> Result<UpstreamResponse, ProxyError> {
        let _ = (path, headers, body, streaming);
        Err(ProxyError::BadRequest("not yet implemented".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderKind;

    #[test]
    fn quirks_none_is_all_off() {
        let q = Quirks::none();
        assert!(!q.reasoning_split);
        assert!(q.strip_thinking.is_none());
        assert!(!q.reorder_tool_responses);
        assert!(!q.sanitize_empty_tools);
        assert!(q.reasoning_effort.is_none());
    }

    #[test]
    fn preset_minimax_enables_reasoning_split_and_strip() {
        let q = quirks_for(ProviderKind::Minimax, ThinkingMode::SplitOnly, None, false);
        assert!(q.reasoning_split);
        assert_eq!(q.strip_thinking, Some(ThinkingMode::SplitOnly));
    }

    #[test]
    fn preset_deepseek_reorders_tools() {
        let q = quirks_for(ProviderKind::DeepSeek, ThinkingMode::SplitOnly, None, false);
        assert!(q.reorder_tool_responses);
        assert!(!q.reasoning_split);
    }

    #[test]
    fn preset_kimi_sanitizes_when_configured() {
        let q = quirks_for(ProviderKind::Kimi, ThinkingMode::SplitOnly, None, true);
        assert!(q.sanitize_empty_tools);
    }

    #[test]
    fn preset_anthropic_carries_reasoning_effort() {
        let q = quirks_for(
            ProviderKind::Anthropic,
            ThinkingMode::SplitOnly,
            Some("high".into()),
            false,
        );
        assert_eq!(q.reasoning_effort.as_deref(), Some("high"));
    }

    #[test]
    fn preset_zai_and_openai_have_no_quirks() {
        for k in [ProviderKind::Zai, ProviderKind::OpenAi] {
            let q = quirks_for(k, ThinkingMode::SplitOnly, None, false);
            assert!(!q.reasoning_split && !q.reorder_tool_responses && !q.sanitize_empty_tools);
        }
    }

    use super::super::messages_protocol::AuthHeader;
    use crate::application::ports::{ApiFormat, Provider};

    fn up(anthropic: Option<&str>, openai: Option<&str>) -> UpstreamProvider {
        UpstreamProvider::new(
            "p".into(),
            anthropic.map(str::to_string),
            openai.map(str::to_string),
            AuthHeader::Passthrough,
            Quirks::none(),
            reqwest::Client::new(),
        )
    }

    #[test]
    fn supported_formats_reflect_configured_urls() {
        let both = up(Some("https://a"), Some("https://o"));
        assert!(both.supported_formats().has(ApiFormat::Anthropic));
        assert!(both.supported_formats().has(ApiFormat::OpenAI));

        let anth = up(Some("https://a"), None);
        assert!(anth.supported_formats().has(ApiFormat::Anthropic));
        assert!(!anth.supported_formats().has(ApiFormat::OpenAI));

        let oai = up(None, Some("https://o"));
        assert!(!oai.supported_formats().has(ApiFormat::Anthropic));
        assert!(oai.supported_formats().has(ApiFormat::OpenAI));
    }

    #[test]
    fn name_is_the_configured_name() {
        assert_eq!(up(Some("https://a"), None).name(), "p");
    }
}
