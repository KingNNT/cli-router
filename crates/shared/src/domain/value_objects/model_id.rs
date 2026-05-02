use crate::domain::DomainError;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModelId(String);

impl ModelId {
    /// Rejects empty strings.
    ///
    /// ```
    /// use shared::domain::value_objects::ModelId;
    /// assert_eq!(ModelId::new("claude-opus-4-7").unwrap().as_str(), "claude-opus-4-7");
    /// assert!(ModelId::new("").is_err());
    /// ```
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        let s = value.into();
        if s.is_empty() {
            Err(DomainError::InvalidModelId)
        } else {
            Ok(Self(s))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Candidate lookup keys for matching this model against a pricing row.
    ///
    /// For `"anthropic/claude-opus-4-7"` returns `["anthropic/claude-opus-4-7", "claude-opus-4-7"]`.
    /// For `"claude-opus-4"` (no `/`) returns `["claude-opus-4"]`.
    ///
    /// The caller tries each in order and picks the first hit.
    ///
    /// ```
    /// use shared::domain::value_objects::ModelId;
    /// let id = ModelId::new("anthropic/claude-opus-4-7").unwrap();
    /// assert_eq!(
    ///     id.lookup_keys(),
    ///     vec!["anthropic/claude-opus-4-7".to_string(), "claude-opus-4-7".to_string()],
    /// );
    /// ```
    pub fn lookup_keys(&self) -> Vec<String> {
        let mut keys = vec![self.0.clone()];
        if let Some(pos) = self.0.find('/') {
            let suffix = &self.0[pos + 1..];
            if !suffix.is_empty() {
                keys.push(suffix.to_string());
            }
        }
        keys
    }
}

impl std::fmt::Display for ModelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_accepts_non_empty() {
        let m = ModelId::new("anthropic/claude-opus-4-7").unwrap();
        assert_eq!(m.as_str(), "anthropic/claude-opus-4-7");
    }

    #[test]
    fn new_rejects_empty() {
        assert_eq!(ModelId::new(""), Err(DomainError::InvalidModelId));
    }

    #[test]
    fn lookup_keys_for_provider_prefixed_returns_two() {
        let m = ModelId::new("anthropic/claude-opus-4-7").unwrap();
        assert_eq!(
            m.lookup_keys(),
            vec![
                "anthropic/claude-opus-4-7".to_string(),
                "claude-opus-4-7".to_string()
            ]
        );
    }

    #[test]
    fn lookup_keys_for_bare_returns_one() {
        let m = ModelId::new("claude-opus-4").unwrap();
        assert_eq!(m.lookup_keys(), vec!["claude-opus-4".to_string()]);
    }

    #[test]
    fn lookup_keys_with_multiple_slashes_splits_at_first() {
        let m = ModelId::new("openrouter/anthropic/claude-3-opus").unwrap();
        assert_eq!(
            m.lookup_keys(),
            vec![
                "openrouter/anthropic/claude-3-opus".to_string(),
                "anthropic/claude-3-opus".to_string()
            ]
        );
    }

    use proptest::prelude::*;

    proptest! {
        #[test]
        fn lookup_keys_first_is_always_full_id(s in "[a-zA-Z0-9/_-]{1,40}") {
            let m = ModelId::new(&*s).unwrap();
            let keys = m.lookup_keys();
            prop_assert!(!keys.is_empty());
            prop_assert_eq!(&keys[0], m.as_str());
        }

        #[test]
        fn lookup_keys_has_one_or_two_entries(s in "[a-zA-Z0-9/_-]{1,40}") {
            let keys = ModelId::new(&*s).unwrap().lookup_keys();
            prop_assert!(keys.len() == 1 || keys.len() == 2);
        }

        #[test]
        fn second_key_is_suffix_after_first_slash(s in "[a-zA-Z0-9_-]{1,15}/[a-zA-Z0-9/_-]{1,15}") {
            let m = ModelId::new(&*s).unwrap();
            let keys = m.lookup_keys();
            prop_assert_eq!(keys.len(), 2);
            let pos = m.as_str().find('/').unwrap();
            prop_assert_eq!(&keys[1], &m.as_str()[pos + 1..]);
        }

        #[test]
        fn new_rejects_empty_always(_d in 0u8..1) {
            prop_assert!(ModelId::new("").is_err());
        }
    }
}
