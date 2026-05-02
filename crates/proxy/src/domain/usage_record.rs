//! Token usage value type, provider-agnostic.

#[derive(Debug, Clone, Default, PartialEq)]
pub struct UsageRecord {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_creation_tokens: Option<u64>,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct UsageState {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_creation_tokens: Option<u64>,
}

impl From<UsageState> for UsageRecord {
    fn from(s: UsageState) -> Self {
        UsageRecord {
            input_tokens: s.input_tokens,
            output_tokens: s.output_tokens,
            cache_read_tokens: s.cache_read_tokens,
            cache_creation_tokens: s.cache_creation_tokens,
        }
    }
}
