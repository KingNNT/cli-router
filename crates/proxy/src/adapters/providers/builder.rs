//! Compose `Provider` adapters from a `Config` snapshot. Lives here so both
//! the daemon's composition root (`main.rs`) and the hot-reload path
//! (`LiveProvider::reload`) call the same code.

use super::{AnthropicProvider, AuthHeader, RoutingProvider, ZaiProvider};
use crate::application::ports::Provider;
use crate::config::{AuthConfig, Config, ProviderConfig, ProviderKind};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("provider '{0}' referenced by routing rule but not in providers list")]
    UnknownProvider(String),
    #[error("invalid glob pattern '{0}': {1}")]
    BadPattern(String, String),
}

/// Build a single leaf provider from one `ProviderConfig` row.
pub fn build_leaf(p: &ProviderConfig, http: reqwest::Client) -> Arc<dyn Provider> {
    let auth = match &p.auth {
        AuthConfig::Passthrough => AuthHeader::Passthrough,
        AuthConfig::ApiKey { value } => AuthHeader::ApiKey(value.clone()),
        AuthConfig::Bearer { value } => AuthHeader::Bearer(value.clone()),
        AuthConfig::AnthropicOAuth {
            access_token,
            refresh_token,
            expires_at_ms,
        } => AuthHeader::OAuth {
            access_token: access_token.clone(),
            refresh_token: refresh_token.clone(),
            expires_at_ms: *expires_at_ms,
        },
    };
    match p.kind {
        ProviderKind::Anthropic => {
            Arc::new(AnthropicProvider::configure(http, p.base_url.clone(), auth))
        }
        ProviderKind::Zai => Arc::new(ZaiProvider::configure(http, p.base_url.clone(), auth)),
    }
}

/// Build the per-name leaf map.
pub fn build_leaves(
    providers: &[ProviderConfig],
    http: reqwest::Client,
) -> HashMap<String, Arc<dyn Provider>> {
    providers
        .iter()
        .map(|p| (p.name.clone(), build_leaf(p, http.clone())))
        .collect()
}

/// Build the top-level `RoutingProvider` from the config + a pre-built leaf
/// map. Returns the routing provider as `Arc<dyn Provider>`.
pub fn build_routing_provider(
    cfg: &Config,
    leaves: &HashMap<String, Arc<dyn Provider>>,
) -> Result<Arc<dyn Provider>, BuildError> {
    let mut builder = RoutingProvider::builder();
    for rule in &cfg.routing {
        let pattern = rule.match_spec.model.as_deref().unwrap_or("*");
        let primary = leaves
            .get(&rule.provider)
            .ok_or_else(|| BuildError::UnknownProvider(rule.provider.clone()))?
            .clone();
        let fallback: Vec<Arc<dyn Provider>> = rule
            .fallback
            .iter()
            .map(|n| {
                leaves
                    .get(n)
                    .cloned()
                    .ok_or_else(|| BuildError::UnknownProvider(n.clone()))
            })
            .collect::<Result<_, _>>()?;
        builder = builder
            .rule(pattern, rule.strategy, primary, fallback)
            .map_err(|e| BuildError::BadPattern(pattern.into(), e.to_string()))?;
    }
    Ok(Arc::new(builder.build()))
}

/// Convenience: leaves + routing in one shot.
pub fn build_from_config(
    cfg: &Config,
    http: reqwest::Client,
) -> Result<Arc<dyn Provider>, BuildError> {
    let leaves = build_leaves(&cfg.providers, http);
    build_routing_provider(cfg, &leaves)
}
