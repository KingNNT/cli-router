use super::minimax_stream::ThinkingMode;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quirks_none_is_all_off() {
        let q = Quirks::none();
        assert!(!q.reasoning_split);
        assert!(q.strip_thinking.is_none());
        assert!(!q.reorder_tool_responses);
        assert!(!q.sanitize_empty_tools);
        assert!(q.reasoning_effort.is_none());
    }
}
