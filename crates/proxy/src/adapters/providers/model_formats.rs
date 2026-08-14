//! Compiled per-model wire-format rules. The config string is the source of
//! truth; this is its runtime form.

use crate::config::{WireFormat, parse_model_formats};
use globset::{Glob, GlobMatcher};

#[derive(Debug, Default)]
pub struct ModelFormatTable {
    rules: Vec<(GlobMatcher, WireFormat)>,
}

impl ModelFormatTable {
    pub fn empty() -> Self {
        Self::default()
    }

    /// Compile the compact rule string. Syntax errors carry the offending
    /// entry so the message is actionable in the TUI.
    pub fn parse(rules: &str) -> Result<Self, String> {
        let parsed = parse_model_formats(rules).map_err(|e| e.to_string())?;
        let mut compiled = Vec::with_capacity(parsed.len());
        for (glob, format) in parsed {
            let matcher = Glob::new(&glob)
                .map_err(|e| format!("invalid model_formats glob '{glob}': {e}"))?
                .compile_matcher();
            compiled.push((matcher, format));
        }
        Ok(Self { rules: compiled })
    }

    /// First matching rule wins. `None` means "no opinion" — the caller falls
    /// back to the provider's URL-derived capability.
    pub fn resolve(&self, model: &str) -> Option<WireFormat> {
        self.rules
            .iter()
            .find(|(matcher, _)| matcher.is_match(model))
            .map(|(_, format)| *format)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WireFormat;

    #[test]
    fn resolves_first_match_in_written_order() {
        let t = ModelFormatTable::parse("qwen3.7-max=responses,qwen3.*=anthropic").unwrap();
        assert_eq!(t.resolve("qwen3.7-max"), Some(WireFormat::Responses));
        assert_eq!(t.resolve("qwen3.6-plus"), Some(WireFormat::Anthropic));
    }

    #[test]
    fn unmatched_model_resolves_to_none() {
        let t = ModelFormatTable::parse("minimax-*=anthropic").unwrap();
        assert_eq!(t.resolve("kimi-k3"), None);
    }

    #[test]
    fn empty_table_never_matches() {
        assert_eq!(ModelFormatTable::empty().resolve("anything"), None);
    }

    #[test]
    fn dot_is_literal_not_a_wildcard() {
        let t = ModelFormatTable::parse("qwen3.*=anthropic").unwrap();
        assert_eq!(t.resolve("qwen3.8-max"), Some(WireFormat::Anthropic));
        assert_eq!(t.resolve("qwen38-max"), None);
    }

    #[test]
    fn parse_error_names_the_bad_entry() {
        let err = ModelFormatTable::parse("glm-*=grpc").unwrap_err();
        assert!(err.contains("glm-*=grpc"), "{err}");
    }
}
