//! Pure validation logic for the provider form modal.
//!
//! No I/O, no ratatui, no async — just functions on `ConfigPayload` /
//! `ProviderPayload` shapes. Easy to unit-test.

use proxy_admin_api::{AuthPayload, ConfigPayload, ProviderPayload};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormError {
    EmptyName,
    InvalidNameChars,
    DuplicateName,
    EmptyAuthValue,
    RenameBlockedBy(Vec<String>),
}

impl std::fmt::Display for FormError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FormError::EmptyName => write!(f, "name is required"),
            FormError::InvalidNameChars => {
                write!(f, "name may contain only letters, digits, '-' and '_'")
            }
            FormError::DuplicateName => write!(f, "a provider with this name already exists"),
            FormError::EmptyAuthValue => write!(f, "auth value is required for api_key and bearer"),
            FormError::RenameBlockedBy(rules) => {
                write!(
                    f,
                    "cannot rename: referenced by {} routing rule(s):\n  - {}",
                    rules.len(),
                    rules.join("\n  - ")
                )
            }
        }
    }
}

/// Inputs needed to validate. The caller owns a richer modal struct, but
/// validation only needs these.
pub struct FormInputs<'a> {
    pub name: &'a str,
    pub kind: &'a str,
    pub base_url: Option<&'a str>,
    pub openai_base_url: Option<&'a str>,
    pub reasoning_effort: Option<&'a str>,
    pub thinking_mode: Option<&'a str>,
    pub sanitize_empty_tools: bool,
    /// Whether the provider is active and eligible for routing.
    pub enabled: bool,
    /// Preserved per-provider concurrency cap (not yet editable in the form).
    pub max_concurrent: Option<usize>,
    pub auth: &'a AuthPayload,
    /// `None` for Add, `Some(original_index)` for Edit.
    pub editing_index: Option<usize>,
    /// `None` for Add, `Some(original_name)` for Edit.
    pub original_name: Option<&'a str>,
}

/// Validate the form against the current config and return a freshly built
/// `ProviderPayload` if everything checks out. Caller is responsible for
/// inserting/replacing it in the cached config.
pub fn validate_provider_form(
    input: &FormInputs<'_>,
    cfg: &ConfigPayload,
) -> Result<ProviderPayload, FormError> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err(FormError::EmptyName);
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(FormError::InvalidNameChars);
    }

    // Duplicate name check.
    let dup = cfg
        .providers
        .iter()
        .enumerate()
        .any(|(i, p)| p.name == name && Some(i) != input.editing_index);
    if dup {
        return Err(FormError::DuplicateName);
    }

    // Auth value presence.
    match input.auth {
        AuthPayload::ApiKey { value } | AuthPayload::Bearer { value }
            if value.trim().is_empty() =>
        {
            return Err(FormError::EmptyAuthValue);
        }
        _ => {}
    }

    // Rename blocked by routing references.
    if let (Some(orig), Some(_)) = (input.original_name, input.editing_index)
        && orig != name
    {
        let refs = rules_referencing(orig, cfg);
        if !refs.is_empty() {
            return Err(FormError::RenameBlockedBy(refs));
        }
    }

    Ok(ProviderPayload {
        name: name.to_string(),
        kind: input.kind.to_string(),
        enabled: input.enabled,
        auth: input.auth.clone(),
        base_url: input
            .base_url
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        openai_base_url: input
            .openai_base_url
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        reasoning_effort: if matches!(input.kind, "codex" | "anthropic") {
            input
                .reasoning_effort
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        } else {
            None
        },
        thinking_mode: if input.kind == "minimax" {
            input.thinking_mode.map(str::to_string)
        } else {
            None
        },
        max_concurrent: input.max_concurrent,
        sanitize_empty_tools: if input.kind == "kimi" {
            Some(input.sanitize_empty_tools)
        } else {
            None
        },
    })
}

/// Returns human-readable descriptions of routing rules that reference
/// `provider_name` (either as primary `provider` or in `fallback`).
pub fn rules_referencing(provider_name: &str, cfg: &ConfigPayload) -> Vec<String> {
    cfg.routing
        .iter()
        .enumerate()
        .filter_map(|(idx, rule)| {
            let role = if rule.provider == provider_name {
                Some("provider")
            } else if rule.fallback.iter().any(|f| f == provider_name) {
                Some("fallback")
            } else {
                None
            }?;
            let model = rule.r#match.model.as_deref().unwrap_or("*");
            Some(format!(
                "rule #{}: match={} {}={}",
                idx + 1,
                model,
                role,
                provider_name
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proxy_admin_api::{
        AffinityPayload, MatchPayload, RoutingRulePayload, RoutingStrategyPayload,
    };

    fn empty_cfg() -> ConfigPayload {
        ConfigPayload {
            port: 8787,
            providers: vec![],
            routing: vec![],
            quota: vec![],
            affinity: AffinityPayload {
                enabled: true,
                headers: vec![],
            },
            proxy_db: None,
            pricing_db: None,
        }
    }

    fn provider(name: &str) -> ProviderPayload {
        ProviderPayload {
            name: name.into(),
            kind: "anthropic".into(),
            enabled: true,
            auth: AuthPayload::Passthrough,
            base_url: None,
            openai_base_url: None,
            reasoning_effort: None,
            thinking_mode: None,
            max_concurrent: None,
            sanitize_empty_tools: None,
        }
    }

    fn rule(model: &str, primary: &str, fallback: &[&str]) -> RoutingRulePayload {
        RoutingRulePayload {
            r#match: MatchPayload {
                model: Some(model.into()),
            },
            provider: primary.into(),
            fallback: fallback.iter().map(|s| s.to_string()).collect(),
            strategy: RoutingStrategyPayload::default(),
            priority: None,
        }
    }

    fn inputs<'a>(name: &'a str, auth: &'a AuthPayload) -> FormInputs<'a> {
        FormInputs {
            name,
            kind: "anthropic",
            base_url: None,
            openai_base_url: None,
            reasoning_effort: None,
            thinking_mode: None,
            max_concurrent: None,
            auth,
            editing_index: None,
            original_name: None,
            sanitize_empty_tools: false,
            enabled: true,
        }
    }

    #[test]
    fn empty_name_rejected() {
        let cfg = empty_cfg();
        let auth = AuthPayload::Passthrough;
        let r = validate_provider_form(&inputs("", &auth), &cfg);
        assert_eq!(r, Err(FormError::EmptyName));
    }

    #[test]
    fn whitespace_only_name_rejected() {
        let cfg = empty_cfg();
        let auth = AuthPayload::Passthrough;
        let r = validate_provider_form(&inputs("   ", &auth), &cfg);
        assert_eq!(r, Err(FormError::EmptyName));
    }

    #[test]
    fn invalid_name_chars_rejected() {
        let cfg = empty_cfg();
        let auth = AuthPayload::Passthrough;
        for bad in &["a/b", "a b", "a.b", "a!b"] {
            let r = validate_provider_form(&inputs(bad, &auth), &cfg);
            assert_eq!(r, Err(FormError::InvalidNameChars), "input: {bad}");
        }
    }

    #[test]
    fn duplicate_name_on_add_rejected() {
        let mut cfg = empty_cfg();
        cfg.providers.push(provider("foo"));
        let auth = AuthPayload::Passthrough;
        let r = validate_provider_form(&inputs("foo", &auth), &cfg);
        assert_eq!(r, Err(FormError::DuplicateName));
    }

    #[test]
    fn duplicate_name_on_edit_at_different_index_rejected() {
        let mut cfg = empty_cfg();
        cfg.providers.push(provider("foo"));
        cfg.providers.push(provider("bar"));
        let auth = AuthPayload::Passthrough;
        let mut input = inputs("foo", &auth);
        input.editing_index = Some(1); // editing "bar", trying to rename to "foo"
        input.original_name = Some("bar");
        let r = validate_provider_form(&input, &cfg);
        assert_eq!(r, Err(FormError::DuplicateName));
    }

    #[test]
    fn edit_keeping_same_name_allowed() {
        let mut cfg = empty_cfg();
        cfg.providers.push(provider("foo"));
        let auth = AuthPayload::ApiKey { value: "k".into() };
        let mut input = inputs("foo", &auth);
        input.editing_index = Some(0);
        input.original_name = Some("foo");
        let p = validate_provider_form(&input, &cfg).unwrap();
        assert_eq!(p.name, "foo");
    }

    #[test]
    fn empty_api_key_value_rejected() {
        let cfg = empty_cfg();
        let auth = AuthPayload::ApiKey {
            value: "   ".into(),
        };
        let r = validate_provider_form(&inputs("foo", &auth), &cfg);
        assert_eq!(r, Err(FormError::EmptyAuthValue));
    }

    #[test]
    fn empty_bearer_value_rejected() {
        let cfg = empty_cfg();
        let auth = AuthPayload::Bearer { value: "".into() };
        let r = validate_provider_form(&inputs("foo", &auth), &cfg);
        assert_eq!(r, Err(FormError::EmptyAuthValue));
    }

    #[test]
    fn passthrough_with_no_value_ok() {
        let cfg = empty_cfg();
        let auth = AuthPayload::Passthrough;
        let p = validate_provider_form(&inputs("foo", &auth), &cfg).unwrap();
        assert!(matches!(p.auth, AuthPayload::Passthrough));
    }

    #[test]
    fn codex_provider_preserves_reasoning_effort() {
        let auth = AuthPayload::CodexAuto;
        let cfg = empty_cfg();
        let input = FormInputs {
            name: "codex-main",
            kind: "codex",
            base_url: None,
            openai_base_url: None,
            reasoning_effort: Some("high"),
            thinking_mode: None,
            max_concurrent: None,
            auth: &auth,
            editing_index: None,
            original_name: None,
            sanitize_empty_tools: false,
            enabled: true,
        };

        let provider = validate_provider_form(&input, &cfg).unwrap();
        assert_eq!(provider.reasoning_effort.as_deref(), Some("high"));
    }

    #[test]
    fn non_codex_provider_drops_reasoning_effort() {
        let auth = AuthPayload::Passthrough;
        let cfg = empty_cfg();
        let input = FormInputs {
            name: "zai-main",
            kind: "zai",
            base_url: None,
            openai_base_url: None,
            reasoning_effort: Some("high"),
            thinking_mode: None,
            max_concurrent: None,
            auth: &auth,
            editing_index: None,
            original_name: None,
            sanitize_empty_tools: false,
            enabled: true,
        };

        let provider = validate_provider_form(&input, &cfg).unwrap();
        assert_eq!(provider.reasoning_effort, None);
    }

    #[test]
    fn anthropic_provider_preserves_reasoning_effort() {
        let auth = AuthPayload::Passthrough;
        let cfg = empty_cfg();
        let input = FormInputs {
            name: "anthropic-main",
            kind: "anthropic",
            base_url: None,
            openai_base_url: None,
            reasoning_effort: Some("high"),
            thinking_mode: None,
            max_concurrent: None,
            auth: &auth,
            editing_index: None,
            original_name: None,
            sanitize_empty_tools: false,
            enabled: true,
        };

        let provider = validate_provider_form(&input, &cfg).unwrap();
        assert_eq!(provider.reasoning_effort.as_deref(), Some("high"));
    }

    #[test]
    fn kimi_provider_preserves_sanitize_empty_tools() {
        let auth = AuthPayload::Passthrough;
        let cfg = empty_cfg();
        let input = FormInputs {
            name: "kimi-main",
            kind: "kimi",
            base_url: None,
            openai_base_url: None,
            reasoning_effort: None,
            thinking_mode: None,
            max_concurrent: None,
            auth: &auth,
            editing_index: None,
            original_name: None,
            sanitize_empty_tools: true,
            enabled: true,
        };

        let provider = validate_provider_form(&input, &cfg).unwrap();
        assert_eq!(provider.sanitize_empty_tools, Some(true));
    }

    #[test]
    fn non_kimi_provider_drops_sanitize_empty_tools() {
        let auth = AuthPayload::Passthrough;
        let cfg = empty_cfg();
        let input = FormInputs {
            name: "zai-main",
            kind: "zai",
            base_url: None,
            openai_base_url: None,
            reasoning_effort: None,
            thinking_mode: None,
            max_concurrent: None,
            auth: &auth,
            editing_index: None,
            original_name: None,
            sanitize_empty_tools: true,
            enabled: true,
        };

        let provider = validate_provider_form(&input, &cfg).unwrap();
        assert_eq!(provider.sanitize_empty_tools, None);
    }

    #[test]
    fn rename_blocked_by_routing_reference() {
        let mut cfg = empty_cfg();
        cfg.providers.push(provider("foo"));
        cfg.routing.push(rule("*", "foo", &[]));
        let auth = AuthPayload::Passthrough;
        let mut input = inputs("bar", &auth);
        input.editing_index = Some(0);
        input.original_name = Some("foo");
        let r = validate_provider_form(&input, &cfg);
        match r {
            Err(FormError::RenameBlockedBy(rules)) => assert_eq!(rules.len(), 1),
            other => panic!("expected RenameBlockedBy, got {:?}", other),
        }
    }

    #[test]
    fn rules_referencing_finds_primary_and_fallback() {
        let mut cfg = empty_cfg();
        cfg.routing.push(rule("opus-*", "a", &["b"]));
        cfg.routing.push(rule("*", "b", &["a", "c"]));
        let refs = rules_referencing("a", &cfg);
        assert_eq!(refs.len(), 2);
        assert!(refs[0].contains("provider=a"));
        assert!(refs[1].contains("fallback=a"));
    }

    #[test]
    fn rules_referencing_returns_empty_when_unreferenced() {
        let mut cfg = empty_cfg();
        cfg.routing.push(rule("*", "x", &["y"]));
        assert!(rules_referencing("z", &cfg).is_empty());
    }

    #[test]
    fn base_urls_normalized_and_optional() {
        let cfg = empty_cfg();
        let auth = AuthPayload::Passthrough;
        let mut input = inputs("foo", &auth);
        input.base_url = Some("  https://api.example  ");
        input.openai_base_url = Some("");
        let p = validate_provider_form(&input, &cfg).unwrap();
        assert_eq!(p.base_url.as_deref(), Some("https://api.example"));
        assert_eq!(p.openai_base_url, None);
    }
}
