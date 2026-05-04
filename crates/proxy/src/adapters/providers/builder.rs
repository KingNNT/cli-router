//! Compose `Provider` adapters from a `Config` snapshot. Lives here so both
//! the daemon's composition root (`main.rs`) and the hot-reload path
//! (`LiveProvider::reload`) call the same code.

use super::{AnthropicProvider, AuthHeader, RoutingProvider, ZaiProvider};
use super::account_usage::{AnthropicAccountUsage, ZaiAccountUsage};
use crate::application::ports::{AccountUsagePort, Provider, QuotaPort};
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
        ProviderKind::Zai => Arc::new(ZaiProvider::configure(
            http,
            p.base_url.clone(),
            p.openai_base_url.clone(),
            auth,
        )),
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
    quota: Arc<dyn QuotaPort>,
) -> Result<Arc<dyn Provider>, BuildError> {
    // Collect rules with their original index as default priority.
    let mut indexed: Vec<(u32, &crate::config::RoutingRule)> = cfg
        .routing
        .iter()
        .enumerate()
        .map(|(i, r)| (r.priority.unwrap_or(i as u32), r))
        .collect();
    // Sort by priority (lower = higher priority = checked first).
    indexed.sort_by_key(|(pri, _)| *pri);

    let mut builder = RoutingProvider::builder();
    for (_pri, rule) in indexed {
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
    Ok(Arc::new(
        builder
            .leaves(leaves.clone())
            .affinity(cfg.affinity.clone())
            .quota(quota)
            .build(),
    ))
}

/// Build leaves + routing in one shot, wiring in the given quota handle.
/// The caller is responsible for supplying the live quota so that enforcement
/// survives hot-reload. Pass `Arc::new(NoopQuota)` in tests or static builds
/// that do not need enforcement.
pub fn build_from_config(
    cfg: &Config,
    http: reqwest::Client,
    quota: Arc<dyn QuotaPort>,
) -> Result<Arc<dyn Provider>, BuildError> {
    let leaves = build_leaves(&cfg.providers, http);
    build_routing_provider(cfg, &leaves, quota)
}

/// Build the per-name account usage adapter map.
///
/// Z.ai providers get a `ZaiAccountUsage` adapter (hits Z.ai's monitoring API).
/// All other kinds get `AnthropicAccountUsage` (returns `None` — no public API).
pub fn build_account_usage(
    providers: &[ProviderConfig],
) -> HashMap<String, Arc<dyn AccountUsagePort>> {
    providers
        .iter()
        .map(|p| {
            let adapter: Arc<dyn AccountUsagePort> = match p.kind {
                ProviderKind::Zai => {
                    // Derive the monitoring base URL from the provider config.
                    // The Z.ai monitoring API lives at the scheme+host level,
                    // e.g. "https://api.z.ai" regardless of the openai_base_url path.
                    let base_url = derive_monitor_base_url(p);
                    let token = resolve_auth_token(&p.auth);
                    Arc::new(ZaiAccountUsage::new(
                        p.name.clone(),
                        token,
                        base_url,
                    ))
                }
                ProviderKind::Anthropic => Arc::new(AnthropicAccountUsage),
            };
            (p.name.clone(), adapter)
        })
        .collect()
}

/// Extract the bearer token value from an auth config for use in
/// the Z.ai monitoring API (`Authorization: <token>` header).
fn resolve_auth_token(auth: &AuthConfig) -> String {
    match auth {
        AuthConfig::ApiKey { value } => value.clone(),
        AuthConfig::Bearer { value } => value.clone(),
        AuthConfig::AnthropicOAuth { access_token, .. } => access_token.clone(),
        AuthConfig::Passthrough => String::new(),
    }
}

/// Derive the monitoring base URL from a provider config.
/// The Z.ai monitoring API lives at scheme+host (e.g. "https://api.z.ai"),
/// not at the full openai_base_url path.
fn derive_monitor_base_url(p: &ProviderConfig) -> String {
    // Try to extract scheme+host from openai_base_url first, then base_url.
    p.openai_base_url
        .as_deref()
        .or(p.base_url.as_deref())
        .and_then(|u| {
            let idx = u.find("://")?;
            let rest = &u[idx + 3..];
            let end = rest.find('/').unwrap_or(rest.len());
            Some(format!("{}://{}", &u[..idx + 3], &rest[..end]))
        })
        .unwrap_or_else(|| "https://api.z.ai".to_string())
}
