//! First-run wizard: Welcome → Add Provider → Done.
//!
//! The wizard activates when no config file exists and the proxy daemon is
//! unreachable. It renders as a centered overlay on a cleared screen and
//! guides the user through adding their first LLM provider.

use crate::app::{ProviderFormModal, WizardState, WizardStep};
use proxy_admin_api::{
    AffinityPayload, AuthPayload, ConfigPayload, ProviderPayload, RoutingStrategyPayload,
};

/// Create a fresh wizard starting at the Welcome step.
pub fn new_wizard() -> WizardState {
    WizardState {
        step: WizardStep::Welcome,
        form: ProviderFormModal::new_for_add(),
        saved_path: None,
    }
}

/// Build a default `ConfigPayload` from the wizard form.
///
/// Uses sensible defaults: port 8787, affinity enabled with common session
/// headers, no proxy_db / pricing_db (let the proxy use its built-in defaults).
pub fn build_config_from_form(form: &ProviderFormModal) -> ConfigPayload {
    let auth = match form.auth_kind {
        crate::app::AuthInputKind::Passthrough => AuthPayload::Passthrough,
        crate::app::AuthInputKind::ApiKey => AuthPayload::ApiKey {
            value: form.auth_value.clone(),
        },
        crate::app::AuthInputKind::Bearer => AuthPayload::Bearer {
            value: form.auth_value.clone(),
        },
        crate::app::AuthInputKind::OAuthAnthropic | crate::app::AuthInputKind::OAuthOpenAi => AuthPayload::Passthrough,
    };

    let provider = if form.name.trim().is_empty() {
        None
    } else {
        Some(ProviderPayload {
            name: form.name.trim().to_string(),
            kind: form.kind.label().to_string(),
            auth,
            base_url: Some(form.base_url.trim().to_string()).filter(|s| !s.is_empty()),
            openai_base_url: Some(form.openai_base_url.trim().to_string())
                .filter(|s| !s.is_empty()),
        })
    };

    ConfigPayload {
        port: 8787,
        providers: provider.into_iter().collect(),
        routing: vec![],
        quota: vec![],
        affinity: AffinityPayload {
            enabled: true,
            headers: vec![
                "x-session-id".into(),
                "x-request-id".into(),
                "x-api-key".into(),
            ],
        },
        proxy_db: None,
        pricing_db: None,
    }
}

/// Build a minimal empty config (used when the user skips the Add Provider step).
pub fn build_minimal_config() -> ConfigPayload {
    ConfigPayload {
        port: 8787,
        providers: vec![],
        routing: vec![],
        quota: vec![],
        affinity: AffinityPayload {
            enabled: true,
            headers: vec![
                "x-session-id".into(),
                "x-request-id".into(),
                "x-api-key".into(),
            ],
        },
        proxy_db: None,
        pricing_db: None,
    }
}

/// Label for [`RoutingStrategyPayload`] (used in the routing form).
pub fn strategy_label(s: &RoutingStrategyPayload) -> &'static str {
    match s {
        RoutingStrategyPayload::Failover => "failover",
        RoutingStrategyPayload::RoundRobin => "round_robin",
    }
}

/// Cycle to the next strategy variant.
pub fn strategy_cycle(s: &RoutingStrategyPayload) -> RoutingStrategyPayload {
    match s {
        RoutingStrategyPayload::Failover => RoutingStrategyPayload::RoundRobin,
        RoutingStrategyPayload::RoundRobin => RoutingStrategyPayload::Failover,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{AuthInputKind, ProviderKind};

    #[test]
    fn new_wizard_starts_at_welcome() {
        let w = new_wizard();
        assert_eq!(w.step, WizardStep::Welcome);
    }

    #[test]
    fn build_config_from_form_with_provider() {
        let mut form = ProviderFormModal::new_for_add();
        form.name = "my-provider".into();
        form.kind = ProviderKind::Anthropic;
        form.auth_kind = AuthInputKind::ApiKey;
        form.auth_value = "sk-test".into();
        form.base_url = "https://api.example.com".into();

        let cfg = build_config_from_form(&form);
        assert_eq!(cfg.port, 8787);
        assert_eq!(cfg.providers.len(), 1);
        assert_eq!(cfg.providers[0].name, "my-provider");
        assert_eq!(cfg.providers[0].kind, "anthropic");
        assert!(cfg.affinity.enabled);
        assert_eq!(cfg.affinity.headers.len(), 3);
    }

    #[test]
    fn build_config_from_form_empty_name_yields_no_providers() {
        let form = ProviderFormModal::new_for_add();
        let cfg = build_config_from_form(&form);
        assert!(cfg.providers.is_empty());
    }

    #[test]
    fn minimal_config_is_valid() {
        let cfg = build_minimal_config();
        assert_eq!(cfg.port, 8787);
        assert!(cfg.providers.is_empty());
        assert!(cfg.routing.is_empty());
        assert!(cfg.quota.is_empty());
        assert!(cfg.affinity.enabled);
    }

    #[test]
    fn strategy_cycle_toggles() {
        let failover = RoutingStrategyPayload::Failover;
        let round_robin = RoutingStrategyPayload::RoundRobin;
        assert_eq!(strategy_label(&strategy_cycle(&failover)), "round_robin");
        assert_eq!(strategy_label(&strategy_cycle(&round_robin)), "failover");
    }

    #[test]
    fn strategy_label_matches() {
        assert_eq!(strategy_label(&RoutingStrategyPayload::Failover), "failover");
        assert_eq!(strategy_label(&RoutingStrategyPayload::RoundRobin), "round_robin");
    }
}
