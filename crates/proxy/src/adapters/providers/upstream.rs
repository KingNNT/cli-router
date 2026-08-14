use super::messages_protocol::{self, AuthHeader};
use super::minimax_stream::ThinkingMode;
use super::model_formats::ModelFormatTable;
use crate::application::errors::ProxyError;
use crate::application::ports::{FormatSupport, Provider, UpstreamResponse, UsageParser};
use crate::config::{FormatMode, ProviderKind, WireFormat};
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
    /// Rename `max_tokens` → `max_completion_tokens` in the OpenAI request
    /// body (OpenAI).
    pub rename_max_tokens: bool,
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
        ProviderKind::OpenAi => Quirks {
            rename_max_tokens: true,
            ..Quirks::none()
        },
        ProviderKind::Anthropic | ProviderKind::Zai | ProviderKind::OpencodeGo => Quirks::none(),
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
        ProviderKind::OpencodeGo => (
            Some("https://opencode.ai/zen/go"),
            Some("https://opencode.ai/zen/go/v1"),
        ),
    }
}

/// Seed value for `ProviderConfig::model_formats` when a provider of this kind
/// is created. Prefill only — once stored it is plain config the user owns.
///
/// OpenCode Go serves MiniMax and Qwen on its Anthropic endpoint, Grok 4.5 and
/// GPT 5.6 Luna on the Responses endpoint, and everything else (GLM, Kimi,
/// DeepSeek, MiMo, Hy3) on Chat Completions, which is the fallthrough.
pub fn preset_model_formats(kind: ProviderKind) -> Option<&'static str> {
    match kind {
        ProviderKind::OpencodeGo => {
            Some("minimax-*=anthropic,qwen3.*=anthropic,grok-4.5=responses,gpt-5.6-luna=responses")
        }
        _ => None,
    }
}

pub struct UpstreamProvider {
    name: String,
    anthropic_base_url: Option<String>,
    openai_base_url: Option<String>,
    auth: AuthHeader,
    quirks: Quirks,
    format_mode: FormatMode,
    model_formats: ModelFormatTable,
    thinking_anthropic: Option<super::thinking::ThinkingInjection>,
    thinking_openai: Option<super::thinking::ThinkingInjection>,
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
            format_mode: FormatMode::Both,
            model_formats: ModelFormatTable::empty(),
            thinking_anthropic: None,
            thinking_openai: None,
            http,
        }
    }

    /// Restrict which configured endpoints this provider may serve. See
    /// [`FormatMode`].
    pub fn with_format_mode(mut self, mode: FormatMode) -> Self {
        self.format_mode = mode;
        self
    }

    /// Attach compiled per-model format rules. Empty means "decide from the
    /// configured URLs", i.e. the behavior of every other provider.
    pub fn with_model_formats(mut self, table: ModelFormatTable) -> Self {
        self.model_formats = table;
        self
    }

    /// Attach the provider's resolved thinking patch, one branch per wire
    /// format. Each branch is applied only on requests actually sent in that
    /// format, after `format_mode` and any translation have run.
    pub fn with_thinking(
        mut self,
        anthropic: Option<super::thinking::ThinkingInjection>,
        openai: Option<super::thinking::ThinkingInjection>,
    ) -> Self {
        self.thinking_anthropic = anthropic;
        self.thinking_openai = openai;
        self
    }

    /// Apply Anthropic-format request quirks: merge the resolved thinking
    /// patch, if any.
    fn apply_anthropic_request_quirks(&self, body: Bytes) -> Bytes {
        let mut body = body;
        if let Some(injection) = &self.thinking_anthropic {
            body = injection.apply(body);
        }
        body
    }

    /// Apply OpenAI-format request quirks: reasoning_split injection
    /// (MiniMax), tool_choice stripping for unsupported models (DeepSeek), and
    /// the `max_tokens` rename (OpenAI).
    fn apply_openai_request_quirks(&self, body: Bytes) -> Bytes {
        let mut body = body;
        if self.quirks.reasoning_split {
            body = inject_reasoning_split(&body);
        }
        if self.quirks.strip_tool_choice {
            body = strip_tool_choice_if_unsupported(body);
        }
        if self.quirks.rename_max_tokens {
            body = rename_max_tokens(body);
        }
        if let Some(injection) = &self.thinking_openai {
            body = injection.apply(body);
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

/// Rename `max_tokens` → `max_completion_tokens` in an OpenAI-format request
/// body.
///
/// OpenAI deprecated `max_tokens` on `/v1/chat/completions` and the gpt-5 and
/// o-series models reject it with a 400 ("Unsupported parameter: 'max_tokens'
/// is not supported with this model. Use 'max_completion_tokens' instead"),
/// while `max_completion_tokens` is accepted by every current model. Clients
/// like OpenCode still send the legacy field, so the rename happens here
/// rather than at the caller.
///
/// The quirk is scoped to `ProviderKind::OpenAi` — vendors with their own kind
/// still only understand `max_tokens`. An already-present
/// `max_completion_tokens` wins; the legacy field is then merely dropped.
/// Non-JSON or non-object bodies are returned verbatim.
fn rename_max_tokens(body: Bytes) -> Bytes {
    let Ok(mut v) = serde_json::from_slice::<Value>(&body) else {
        return body;
    };
    let Some(obj) = v.as_object_mut() else {
        return body;
    };
    let Some(max_tokens) = obj.remove("max_tokens") else {
        return body;
    };
    obj.entry("max_completion_tokens").or_insert(max_tokens);

    serde_json::to_vec(obj)
        .map(Bytes::from)
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "failed to re-serialize body after renaming max_tokens");
            body
        })
}

#[async_trait]
impl Provider for UpstreamProvider {
    fn name(&self) -> &str {
        self.name.as_str()
    }

    fn supported_formats(&self) -> FormatSupport {
        let has_anthropic = self
            .anthropic_base_url
            .as_deref()
            .is_some_and(|s| !s.is_empty());
        let has_openai = self
            .openai_base_url
            .as_deref()
            .is_some_and(|s| !s.is_empty());
        // The configured mode narrows what the URLs offer; it never invents an
        // endpoint that isn't configured. A mode pinned to a format with no URL
        // leaves the other one advertised, so requests still reach the upstream
        // instead of failing with "no endpoint".
        match self.format_mode {
            FormatMode::Both => FormatSupport {
                anthropic: has_anthropic,
                openai: has_openai,
            },
            FormatMode::Anthropic if has_anthropic => FormatSupport {
                anthropic: true,
                openai: false,
            },
            FormatMode::OpenAi if has_openai => FormatSupport {
                anthropic: false,
                openai: true,
            },
            _ => FormatSupport {
                anthropic: has_anthropic,
                openai: has_openai,
            },
        }
    }

    fn supported_formats_for(&self, model: &str) -> FormatSupport {
        match self.model_formats.resolve(model) {
            // Responses is an upstream detail of the OpenAI path: routing only
            // needs to know the request goes out as OpenAI.
            Some(WireFormat::OpenAi) | Some(WireFormat::Responses) => FormatSupport {
                anthropic: false,
                openai: true,
            },
            Some(WireFormat::Anthropic) => FormatSupport {
                anthropic: true,
                openai: false,
            },
            None => self.supported_formats(),
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
        assert!(!q.rename_max_tokens);
    }

    fn dual_provider(mode: FormatMode) -> UpstreamProvider {
        UpstreamProvider::new(
            "minimax".to_string(),
            Some("https://api.minimax.io/anthropic".to_string()),
            Some("https://api.minimax.io/v1".to_string()),
            AuthHeader::Bearer("k".to_string()),
            Quirks::none(),
            reqwest::Client::new(),
        )
        .with_format_mode(mode)
    }

    #[test]
    fn format_mode_both_advertises_every_configured_endpoint() {
        let s = dual_provider(FormatMode::Both).supported_formats();
        assert!(s.anthropic);
        assert!(s.openai);
    }

    /// Pinning to Anthropic is what pulls OpenAI clients off MiniMax's OpenAI
    /// endpoint (it loses the head of the answer into `reasoning_content`).
    #[test]
    fn format_mode_anthropic_hides_the_openai_endpoint() {
        let s = dual_provider(FormatMode::Anthropic).supported_formats();
        assert!(s.anthropic);
        assert!(!s.openai);
    }

    #[test]
    fn format_mode_openai_hides_the_anthropic_endpoint() {
        let s = dual_provider(FormatMode::OpenAi).supported_formats();
        assert!(!s.anthropic);
        assert!(s.openai);
    }

    /// A mode pinned to a format with no URL must not black-hole the provider.
    #[test]
    fn format_mode_without_matching_url_falls_back_to_configured_endpoints() {
        let p = UpstreamProvider::new(
            "deepseek".to_string(),
            None,
            Some("https://api.deepseek.com/v1".to_string()),
            AuthHeader::Bearer("k".to_string()),
            Quirks::none(),
            reqwest::Client::new(),
        )
        .with_format_mode(FormatMode::Anthropic);
        let s = p.supported_formats();
        assert!(!s.anthropic);
        assert!(s.openai);
    }

    #[test]
    fn model_formats_narrow_supported_formats_per_model() {
        let p = UpstreamProvider::new(
            "go".to_string(),
            Some("https://opencode.ai/zen/go".to_string()),
            Some("https://opencode.ai/zen/go/v1".to_string()),
            AuthHeader::ApiKey("k".to_string()),
            Quirks::none(),
            reqwest::Client::new(),
        )
        .with_model_formats(
            ModelFormatTable::parse("qwen3.*=anthropic,grok-4.5=responses").unwrap(),
        );

        // Anthropic-only model.
        let qwen = p.supported_formats_for("qwen3.7-max");
        assert!(qwen.anthropic && !qwen.openai);

        // Responses models ride the OpenAI path; the split happens inside
        // forward_openai.
        let grok = p.supported_formats_for("grok-4.5");
        assert!(grok.openai && !grok.anthropic);

        // No rule → unchanged capability (both URLs configured).
        let kimi = p.supported_formats_for("kimi-k3");
        assert!(kimi.anthropic && kimi.openai);
    }

    #[test]
    fn preset_minimax_enables_reasoning_split_and_strip() {
        let q = quirks_for(ProviderKind::Minimax, ThinkingMode::SplitOnly, false);
        assert!(q.reasoning_split);
        assert_eq!(q.strip_thinking, Some(ThinkingMode::SplitOnly));
    }

    #[test]
    fn preset_deepseek_strips_tool_choice() {
        let q = quirks_for(ProviderKind::DeepSeek, ThinkingMode::SplitOnly, false);
        assert!(q.strip_tool_choice);
        assert!(!q.reasoning_split);
    }

    #[test]
    fn preset_kimi_sanitizes_when_configured() {
        let q = quirks_for(ProviderKind::Kimi, ThinkingMode::SplitOnly, true);
        assert!(q.sanitize_empty_tools);
    }

    #[test]
    fn preset_anthropic_and_zai_have_no_quirks() {
        for k in [ProviderKind::Anthropic, ProviderKind::Zai] {
            let q = quirks_for(k, ThinkingMode::SplitOnly, false);
            assert!(
                !q.reasoning_split
                    && !q.strip_tool_choice
                    && !q.sanitize_empty_tools
                    && !q.rename_max_tokens
            );
        }
    }

    #[test]
    fn preset_openai_renames_max_tokens() {
        let q = quirks_for(ProviderKind::OpenAi, ThinkingMode::SplitOnly, false);
        assert!(q.rename_max_tokens);
        assert!(!q.reasoning_split && !q.strip_tool_choice && !q.sanitize_empty_tools);
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
            quirks_for(ProviderKind::Minimax, ThinkingMode::SplitOnly, false),
            reqwest::Client::new(),
        );
        let out = p.apply_openai_request_quirks(Bytes::from(r#"{"model":"m","messages":[]}"#));
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["reasoning_split"], true);
    }

    #[test]
    fn openai_request_quirks_strip_tool_choice_for_deepseek() {
        let p = UpstreamProvider::new(
            "ds".into(),
            None,
            Some("https://o".into()),
            AuthHeader::Bearer("k".into()),
            quirks_for(ProviderKind::DeepSeek, ThinkingMode::SplitOnly, false),
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

    fn openai_kind_provider() -> UpstreamProvider {
        UpstreamProvider::new(
            "openai".into(),
            None,
            Some("https://api.openai.com/v1".into()),
            AuthHeader::Bearer("k".into()),
            quirks_for(ProviderKind::OpenAi, ThinkingMode::SplitOnly, false),
            reqwest::Client::new(),
        )
    }

    /// OpenCode and other OpenAI-format clients still send `max_tokens`, which
    /// the gpt-5 family rejects outright ("Use 'max_completion_tokens'").
    #[test]
    fn openai_request_quirks_rename_max_tokens() {
        let out = openai_kind_provider().apply_openai_request_quirks(Bytes::from(
            r#"{"model":"gpt-5.6-terra","max_tokens":32000,"messages":[]}"#,
        ));
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert!(v.get("max_tokens").is_none());
        assert_eq!(v["max_completion_tokens"], 32000);
    }

    /// A client that already speaks the new field wins; the legacy one is only
    /// dropped, never allowed to clobber it.
    #[test]
    fn rename_max_tokens_keeps_an_existing_max_completion_tokens() {
        let out = openai_kind_provider().apply_openai_request_quirks(Bytes::from(
            r#"{"model":"gpt-5.6-terra","max_tokens":32000,"max_completion_tokens":8000}"#,
        ));
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert!(v.get("max_tokens").is_none());
        assert_eq!(v["max_completion_tokens"], 8000);
    }

    #[test]
    fn rename_max_tokens_is_a_noop_without_the_field() {
        let body = Bytes::from(r#"{"model":"gpt-5.6-terra","messages":[]}"#);
        let out = openai_kind_provider().apply_openai_request_quirks(body.clone());
        assert_eq!(out, body);
    }

    #[test]
    fn rename_max_tokens_passes_through_a_non_json_body() {
        let body = Bytes::from("not json");
        let out = openai_kind_provider().apply_openai_request_quirks(body.clone());
        assert_eq!(out, body);
    }

    /// The quirk is scoped to the OpenAI kind — vendors whose endpoints only
    /// understand `max_tokens` must keep it.
    #[test]
    fn other_kinds_keep_max_tokens() {
        let p = UpstreamProvider::new(
            "ds".into(),
            None,
            Some("https://api.deepseek.com/v1".into()),
            AuthHeader::Bearer("k".into()),
            quirks_for(ProviderKind::DeepSeek, ThinkingMode::SplitOnly, false),
            reqwest::Client::new(),
        );
        let out = p.apply_openai_request_quirks(Bytes::from(
            r#"{"model":"deepseek-chat","max_tokens":100}"#,
        ));
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["max_tokens"], 100);
        assert!(v.get("max_completion_tokens").is_none());
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

    #[test]
    fn opencode_go_preset_serves_both_bases_and_seeds_rules() {
        let (anthropic, openai) = default_urls(ProviderKind::OpencodeGo);
        assert_eq!(anthropic, Some("https://opencode.ai/zen/go"));
        assert_eq!(openai, Some("https://opencode.ai/zen/go/v1"));
        assert_eq!(
            preset_model_formats(ProviderKind::OpencodeGo),
            Some("minimax-*=anthropic,qwen3.*=anthropic,grok-4.5=responses,gpt-5.6-luna=responses")
        );
        assert!(preset_model_formats(ProviderKind::Zai).is_none());
    }

    #[test]
    fn opencode_go_preset_has_no_quirks() {
        let q = quirks_for(ProviderKind::OpencodeGo, ThinkingMode::SplitOnly, false);
        assert!(!q.reasoning_split);
        assert!(!q.strip_tool_choice);
        assert!(!q.rename_max_tokens);
        assert!(!q.sanitize_empty_tools);
        assert!(q.strip_thinking.is_none());
    }

    #[test]
    fn anthropic_path_merges_the_anthropic_thinking_branch() {
        let p = UpstreamProvider::new(
            "anthropic".to_string(),
            Some("https://api.anthropic.com".to_string()),
            None,
            AuthHeader::Bearer("k".to_string()),
            Quirks::none(),
            reqwest::Client::new(),
        )
        .with_thinking(
            Some(super::super::thinking::ThinkingInjection::new(
                serde_json::json!({"output_config": {"effort": "high"}}),
                true,
            )),
            None,
        );
        let out = p.apply_anthropic_request_quirks(Bytes::from(r#"{"model":"claude-opus-5"}"#));
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["output_config"]["effort"], "high");
    }

    #[test]
    fn openai_path_merges_the_openai_thinking_branch() {
        let p = UpstreamProvider::new(
            "deepseek".to_string(),
            None,
            Some("https://api.deepseek.com/v1".to_string()),
            AuthHeader::Bearer("k".to_string()),
            Quirks::none(),
            reqwest::Client::new(),
        )
        .with_thinking(
            None,
            Some(super::super::thinking::ThinkingInjection::new(
                serde_json::json!({"reasoning_effort": "max"}),
                true,
            )),
        );
        let out = p.apply_openai_request_quirks(Bytes::from(r#"{"model":"deepseek-v4-pro"}"#));
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["reasoning_effort"], "max");
    }

    /// The two branches are independent: an Anthropic-only injection must not
    /// leak into an OpenAI-format body.
    #[test]
    fn each_format_only_sees_its_own_branch() {
        let p = UpstreamProvider::new(
            "zai".to_string(),
            Some("https://api.z.ai/api/anthropic".to_string()),
            Some("https://api.z.ai/api/paas/v4".to_string()),
            AuthHeader::Bearer("k".to_string()),
            Quirks::none(),
            reqwest::Client::new(),
        )
        .with_thinking(
            Some(super::super::thinking::ThinkingInjection::new(
                serde_json::json!({"anthropic_only": true}),
                true,
            )),
            None,
        );
        let out = p.apply_openai_request_quirks(Bytes::from(r#"{"model":"glm-5.2"}"#));
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert!(v.get("anthropic_only").is_none());
    }

    #[test]
    fn no_injection_leaves_the_body_byte_identical() {
        let p = UpstreamProvider::new(
            "zai".to_string(),
            Some("https://api.z.ai/api/anthropic".to_string()),
            None,
            AuthHeader::Bearer("k".to_string()),
            Quirks::none(),
            reqwest::Client::new(),
        );
        let body = Bytes::from(r#"{"model":"glm-5.2"}"#);
        assert_eq!(p.apply_anthropic_request_quirks(body.clone()), body);
    }
}
