use super::messages_protocol::{self, AuthHeader};
use super::minimax_stream::ThinkingMode;
use crate::application::errors::ProxyError;
use crate::application::ports::{FormatSupport, Provider, UpstreamResponse, UsageParser};
use crate::config::ProviderKind;
use crate::domain::UsageRecord;
use async_trait::async_trait;
use axum::http::HeaderMap;
use bytes::Bytes;
use serde_json::Value;

/// Per-provider request/response behaviors on the forward path, as data.
#[derive(Debug, Clone, Default)]
pub struct Quirks {
    /// Inject `reasoning_split: true` into the OpenAI request body (MiniMax).
    pub reasoning_split: bool,
    /// Filter thinking content from the OpenAI response (MiniMax).
    pub strip_thinking: Option<ThinkingMode>,
    /// Strip `tool_choice` from the OpenAI request when the target model
    /// doesn't support it (DeepSeek).
    pub strip_tool_choice: bool,
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
            strip_tool_choice: true,
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

/// Return default endpoint URLs (Anthropic format, OpenAI format) for TUI prefill.
pub fn default_urls(kind: ProviderKind) -> (Option<&'static str>, Option<&'static str>) {
    match kind {
        ProviderKind::Anthropic => (Some("https://api.anthropic.com"), None),
        ProviderKind::Minimax => (
            Some("https://api.minimaxi.com/anthropic"),
            Some("https://api.minimaxi.com/v1"),
        ),
        ProviderKind::Zai => (
            Some("https://api.z.ai/api/anthropic"),
            Some("https://api.z.ai/api/paas/v4"),
        ),
        ProviderKind::DeepSeek => (None, Some("https://api.deepseek.com/v1")),
        ProviderKind::OpenAi => (None, Some("https://api.openai.com/v1")),
        ProviderKind::Kimi => (
            Some("https://api.moonshot.ai/anthropic"),
            Some("https://api.moonshot.ai/v1"),
        ),
        ProviderKind::Codex => (None, Some("https://chatgpt.com/backend-api/codex")),
    }
}

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

    /// Apply Anthropic-format request quirks. Currently: inject a default
    /// reasoning effort into `output_config.effort` (Anthropic).
    fn apply_anthropic_request_quirks(&self, body: Bytes) -> Bytes {
        let mut body = body;
        if let Some(effort) = self.quirks.reasoning_effort.as_deref() {
            inject_effort(&mut body, Some(effort));
        }
        body
    }

    /// Apply OpenAI-format request quirks: reasoning_split injection
    /// (MiniMax) and tool_choice stripping for unsupported models (DeepSeek).
    fn apply_openai_request_quirks(&self, body: Bytes) -> Bytes {
        let mut body = body;
        if self.quirks.reasoning_split {
            body = inject_reasoning_split(&body);
        }
        if self.quirks.strip_tool_choice {
            body = strip_tool_choice_if_unsupported(body);
        }
        body
    }

    /// Apply Anthropic-format response quirks: sanitize empty-tool responses
    /// on 2xx bodies only (Kimi). Non-2xx responses pass through unchanged.
    fn apply_anthropic_response_quirks(&self, resp: UpstreamResponse) -> UpstreamResponse {
        if !self.quirks.sanitize_empty_tools {
            return resp;
        }
        match resp {
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
        }
    }

    /// Apply OpenAI-format response quirks: strip thinking content according
    /// to the configured mode (MiniMax).
    fn apply_openai_response_quirks(&self, resp: UpstreamResponse) -> UpstreamResponse {
        let Some(mode) = self.quirks.strip_thinking else {
            return resp;
        };
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
                UpstreamResponse::Buffered {
                    status,
                    headers,
                    body: cleaned,
                    provider_id,
                    translation_direction,
                }
            }
            UpstreamResponse::Streaming {
                status,
                headers,
                body,
                provider_id,
                translation_direction,
            } => UpstreamResponse::Streaming {
                status,
                headers,
                body: super::minimax_stream::clean_thinking_stream(body, mode),
                provider_id,
                translation_direction,
            },
        }
    }
}

/// Inject `reasoning_split: true` into an OpenAI-format request body.
/// Non-JSON or non-object bodies are returned verbatim.
fn inject_reasoning_split(body: &Bytes) -> Bytes {
    let Ok(mut value) = serde_json::from_slice::<Value>(body) else {
        return body.clone();
    };
    let Some(obj) = value.as_object_mut() else {
        return body.clone();
    };
    obj.insert("reasoning_split".to_string(), Value::Bool(true));
    serde_json::to_vec(&value)
        .map(Bytes::from)
        .unwrap_or_else(|_| body.clone())
}

/// Inject `output_config.effort` into an Anthropic Messages API request body
/// if the provider has a default effort configured AND the request doesn't
/// already specify one.
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

/// Models that support `tool_choice`. Only `deepseek-chat` (V3) is known to
/// support it; all other models (reasoner, V4-Pro, etc.) reject it with 400.
const TOOL_CHOICE_SUPPORTED_MODELS: &[&str] = &["deepseek-chat"];

/// Strip `tool_choice` from the request body when the target model is a
/// DeepSeek reasoner model that does not support it.
fn strip_tool_choice_if_unsupported(body: Bytes) -> Bytes {
    let Ok(mut v) = serde_json::from_slice::<Value>(&body) else {
        return body;
    };

    let model = v
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .to_string();

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
        let base = self.anthropic_base_url.as_deref().ok_or_else(|| {
            ProxyError::BadRequest(format!(
                "provider '{}' has no Anthropic endpoint",
                self.name
            ))
        })?;
        let body = self.apply_anthropic_request_quirks(body);
        let resp = messages_protocol::forward(
            &self.http,
            base,
            &self.auth,
            path,
            headers,
            body,
            streaming,
            self.name(),
        )
        .await?;
        Ok(self.apply_anthropic_response_quirks(resp))
    }
    async fn forward_openai(
        &self,
        path: &str,
        headers: &HeaderMap,
        body: Bytes,
        streaming: bool,
    ) -> Result<UpstreamResponse, ProxyError> {
        let base = self.openai_base_url.as_deref().ok_or_else(|| {
            ProxyError::BadRequest(format!("provider '{}' has no OpenAI endpoint", self.name))
        })?;
        // OpenAI-compatible endpoints authenticate with Authorization: Bearer;
        // convert ApiKey → Bearer as the individual providers do today.
        let auth = match &self.auth {
            AuthHeader::ApiKey(v) => AuthHeader::Bearer(v.clone()),
            other => other.clone(),
        };
        let body = self.apply_openai_request_quirks(body);
        let resp = messages_protocol::forward(
            &self.http,
            base,
            &auth,
            path,
            headers,
            body,
            streaming,
            self.name(),
        )
        .await?;
        Ok(self.apply_openai_response_quirks(resp))
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
        assert!(!q.strip_tool_choice);
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
    fn preset_deepseek_strips_tool_choice() {
        let q = quirks_for(ProviderKind::DeepSeek, ThinkingMode::SplitOnly, None, false);
        assert!(q.strip_tool_choice);
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
            assert!(!q.reasoning_split && !q.strip_tool_choice && !q.sanitize_empty_tools);
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

    #[test]
    fn openai_request_quirks_inject_reasoning_split_for_minimax() {
        let p = UpstreamProvider::new(
            "mm".into(),
            None,
            Some("https://o".into()),
            AuthHeader::Bearer("k".into()),
            quirks_for(ProviderKind::Minimax, ThinkingMode::SplitOnly, None, false),
            reqwest::Client::new(),
        );
        let out = p.apply_openai_request_quirks(Bytes::from(r#"{"model":"m","messages":[]}"#));
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["reasoning_split"], true);
    }

    #[test]
    fn anthropic_request_quirks_inject_effort_for_anthropic() {
        let p = UpstreamProvider::new(
            "an".into(),
            Some("https://a".into()),
            None,
            AuthHeader::Passthrough,
            quirks_for(
                ProviderKind::Anthropic,
                ThinkingMode::SplitOnly,
                Some("high".into()),
                false,
            ),
            reqwest::Client::new(),
        );
        let out = p.apply_anthropic_request_quirks(Bytes::from(r#"{"model":"m","messages":[]}"#));
        assert_ne!(out, Bytes::from(r#"{"model":"m","messages":[]}"#));
    }

    #[test]
    fn openai_request_quirks_strip_tool_choice_for_deepseek() {
        let p = UpstreamProvider::new(
            "ds".into(),
            None,
            Some("https://o".into()),
            AuthHeader::Bearer("k".into()),
            quirks_for(ProviderKind::DeepSeek, ThinkingMode::SplitOnly, None, false),
            reqwest::Client::new(),
        );

        let reasoner =
            Bytes::from(r#"{"model":"deepseek-reasoner","tool_choice":"auto","messages":[]}"#);
        let out = p.apply_openai_request_quirks(reasoner);
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert!(v.get("tool_choice").is_none());

        let chat = Bytes::from(r#"{"model":"deepseek-chat","tool_choice":"auto","messages":[]}"#);
        let out = p.apply_openai_request_quirks(chat);
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["tool_choice"], "auto");
    }

    #[test]
    fn default_urls_minimax_has_both() {
        let (a, o) = default_urls(ProviderKind::Minimax);
        assert_eq!(a, Some("https://api.minimaxi.com/anthropic"));
        assert_eq!(o, Some("https://api.minimaxi.com/v1"));
    }

    #[test]
    fn default_urls_deepseek_openai_only() {
        let (a, o) = default_urls(ProviderKind::DeepSeek);
        assert!(a.is_none());
        assert!(o.is_some());
    }
}
