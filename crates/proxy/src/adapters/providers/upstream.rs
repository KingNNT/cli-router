use super::minimax_stream::ThinkingMode;
use crate::config::ProviderKind;

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
        let q = quirks_for(ProviderKind::Anthropic, ThinkingMode::SplitOnly, Some("high".into()), false);
        assert_eq!(q.reasoning_effort.as_deref(), Some("high"));
    }

    #[test]
    fn preset_zai_and_openai_have_no_quirks() {
        for k in [ProviderKind::Zai, ProviderKind::OpenAi] {
            let q = quirks_for(k, ThinkingMode::SplitOnly, None, false);
            assert!(!q.reasoning_split && !q.reorder_tool_responses && !q.sanitize_empty_tools);
        }
    }
}
