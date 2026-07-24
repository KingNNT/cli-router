//! Compose `Provider` adapters from a `Config` snapshot. Lives here so both
//! the daemon's composition root (`main.rs`) and the hot-reload path
//! (`LiveProvider::reload`) call the same code.

use super::account_usage::{
    AnthropicAccountUsage, CodexAccountUsage, DeepSeekAccountUsage, KimiAccountUsage,
    MinimaxAccountUsage, ZaiAccountUsage,
};
use super::minimax_stream;
use super::upstream::{UpstreamProvider, quirks_for};
use super::{AuthHeader, CodexProvider, RoutingProvider};
use crate::application::ports::{AccountUsagePort, AccountUsageRegistry, Provider, QuotaPort};
use crate::config::{AuthConfig, Config, ProviderConfig, ProviderKind};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("provider '{0}' referenced by routing rule but not in providers list")]
    UnknownProvider(String),
    #[error("invalid glob pattern '{0}': {1}")]
    BadPattern(String, String),
    #[error("{0}")]
    AuthResolve(String),
}

/// Build a single leaf provider from one `ProviderConfig` row.
pub fn build_leaf(
    p: &ProviderConfig,
    http: reqwest::Client,
) -> Result<Arc<dyn Provider>, BuildError> {
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
        AuthConfig::OpenAiOAuth {
            access_token,
            refresh_token,
            expires_at_ms,
        } => AuthHeader::OAuth {
            access_token: access_token.clone(),
            refresh_token: refresh_token.clone(),
            expires_at_ms: *expires_at_ms,
        },
        AuthConfig::CodexAuto => {
            let tokens = crate::adapters::oauth::openai::read_auth_json()
                .map_err(|e| BuildError::AuthResolve(format!(
                    "provider '{}': CodexAuto requires ~/.codex/auth.json. Run 'codex login' first. ({e})",
                    p.name
                )))?;
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let expires_at_ms = now_ms + tokens.expires_in.unwrap_or(3600) * 1000;
            AuthHeader::OAuth {
                access_token: tokens.access_token,
                refresh_token: tokens.refresh_token.unwrap_or_default(),
                expires_at_ms,
            }
        }
    };
    // Codex is bespoke: translates to the OpenAI Responses API.
    if let ProviderKind::Codex = p.kind {
        return Ok(Arc::new(CodexProvider::configure_with_reasoning_effort(
            http,
            p.openai_base_url.clone(),
            auth,
            p.reasoning_effort.clone(),
        )));
    }

    // Validate at least one endpoint is configured.
    let has_anthropic = p
        .anthropic_base_url
        .as_deref()
        .is_some_and(|s| !s.is_empty());
    let has_openai = p.openai_base_url.as_deref().is_some_and(|s| !s.is_empty());
    if !has_anthropic && !has_openai {
        return Err(BuildError::AuthResolve(format!(
            "provider '{}': at least one of anthropic_base_url / openai_base_url is required",
            p.name
        )));
    }

    let thinking = match p.thinking_mode {
        crate::config::ThinkingMode::SplitOnly => minimax_stream::ThinkingMode::SplitOnly,
        crate::config::ThinkingMode::StripAll => minimax_stream::ThinkingMode::StripAll,
    };
    let quirks = quirks_for(
        p.kind,
        thinking,
        p.reasoning_effort.clone(),
        p.sanitize_empty_tools,
    );
    Ok(Arc::new(UpstreamProvider::new(
        p.name.clone(),
        p.anthropic_base_url.clone(),
        p.openai_base_url.clone(),
        auth,
        quirks,
        http,
    )))
}

/// Build the per-name leaf map.
pub fn build_leaves(
    providers: &[ProviderConfig],
    http: reqwest::Client,
) -> Result<HashMap<String, Arc<dyn Provider>>, BuildError> {
    providers
        .iter()
        .filter(|p| p.enabled)
        .map(|p| Ok((p.name.clone(), build_leaf(p, http.clone())?)))
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

    // Per-provider concurrency overrides, keyed by provider name. Set before
    // any rule() so each pool entry picks up its configured limit.
    let concurrency: HashMap<String, usize> = cfg
        .providers
        .iter()
        .filter_map(|p| p.max_concurrent.map(|n| (p.name.clone(), n)))
        .collect();

    let mut builder = RoutingProvider::builder().concurrency(concurrency);
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
    let leaves = build_leaves(&cfg.providers, http)?;
    build_routing_provider(cfg, &leaves, quota)
}

/// Build the per-name account usage adapter map.
///
/// Z.ai providers get a `ZaiAccountUsage` adapter (hits Z.ai's monitoring API).
/// Anthropic providers get an `AnthropicAccountUsage` adapter that hits the
/// undocumented `/api/oauth/usage` endpoint when configured with OAuth auth,
/// and returns `None` for API-key auth.
///
/// `config` is shared so the Anthropic adapter can read the **current**
/// OAuth access token on each call — the background `token_refresh` task
/// updates it in place every 60s.
pub fn build_account_usage(
    config: Arc<std::sync::RwLock<Config>>,
) -> HashMap<String, Arc<dyn AccountUsagePort>> {
    let providers: Vec<ProviderConfig> = {
        let cfg = config.read().expect("config rwlock poisoned");
        cfg.providers.clone()
    };
    providers
        .iter()
        .filter(|p| p.enabled)
        .map(|p| {
            let adapter: Arc<dyn AccountUsagePort> = match p.kind {
                ProviderKind::Zai => {
                    let base_url = derive_monitor_base_url(p);
                    let token = resolve_auth_token(&p.auth);
                    Arc::new(ZaiAccountUsage::new(p.name.clone(), token, base_url))
                }
                ProviderKind::Anthropic => {
                    Arc::new(AnthropicAccountUsage::new(p.name.clone(), config.clone()))
                }
                ProviderKind::DeepSeek => {
                    let token = resolve_auth_token(&p.auth);
                    Arc::new(DeepSeekAccountUsage::new(p.name.clone(), token))
                }
                ProviderKind::OpenAi => Arc::new(super::account_usage::noop::NoopAccountUsage),
                ProviderKind::Codex => Arc::new(CodexAccountUsage::new(
                    p.name.clone(),
                    p.openai_base_url.clone(),
                    p.auth.clone(),
                )),
                ProviderKind::Minimax => {
                    let token = resolve_auth_token(&p.auth);
                    Arc::new(MinimaxAccountUsage::new(
                        p.name.clone(),
                        token,
                        p.anthropic_base_url.clone(),
                    ))
                }
                ProviderKind::Kimi => {
                    let token = resolve_auth_token(&p.auth);
                    Arc::new(KimiAccountUsage::new(
                        p.name.clone(),
                        token,
                        p.anthropic_base_url.clone(),
                        p.openai_base_url.clone(),
                    ))
                }
            };
            (p.name.clone(), adapter)
        })
        .collect()
}

/// Hot-reloading account-usage registry.
///
/// Rebuilds the adapter map only when the configured provider *set* changes
/// (tracked by sorted provider name), so a provider added at runtime appears
/// in the account tab without a daemon restart. While the set is stable the
/// cached adapter instances are reused — this preserves each adapter's
/// internal response cache (notably Anthropic's 5-minute / 429-protection
/// cache, which a rebuild-every-call approach would throw away). The OAuth
/// access token is *not* part of the key: the Anthropic adapter reads it live
/// from the shared config, and the background refresh rotates it every 60s.
pub struct LiveAccountUsage {
    config: Arc<RwLock<Config>>,
    cache: Mutex<CachedAdapters>,
}

struct CachedAdapters {
    names: Vec<String>,
    map: HashMap<String, Arc<dyn AccountUsagePort>>,
}

impl LiveAccountUsage {
    pub fn new(config: Arc<RwLock<Config>>) -> Self {
        Self {
            config,
            cache: Mutex::new(CachedAdapters {
                names: Vec::new(),
                map: HashMap::new(),
            }),
        }
    }
}

impl AccountUsageRegistry for LiveAccountUsage {
    fn adapters(&self) -> HashMap<String, Arc<dyn AccountUsagePort>> {
        // Key on the *enabled* provider set so toggling a provider's `enabled`
        // flag rebuilds the map (disabled providers are excluded from the
        // account tab).
        let names = {
            let cfg = self.config.read().expect("config rwlock poisoned");
            let mut n: Vec<String> = cfg
                .providers
                .iter()
                .filter(|p| p.enabled)
                .map(|p| p.name.clone())
                .collect();
            n.sort();
            n
        };
        let mut cache = self.cache.lock().expect("account usage cache poisoned");
        if cache.names != names {
            cache.map = build_account_usage(self.config.clone());
            cache.names = names;
        }
        cache.map.clone()
    }
}

/// Extract the bearer token value from an auth config for use in
/// the Z.ai monitoring API (`Authorization: <token>` header).
fn resolve_auth_token(auth: &AuthConfig) -> String {
    match auth {
        AuthConfig::ApiKey { value } => value.clone(),
        AuthConfig::Bearer { value } => value.clone(),
        AuthConfig::AnthropicOAuth { access_token, .. } => access_token.clone(),
        AuthConfig::OpenAiOAuth { access_token, .. } => access_token.clone(),
        AuthConfig::CodexAuto => String::new(),
        AuthConfig::Passthrough => String::new(),
    }
}

/// Derive the monitoring base URL from a provider config.
/// The Z.ai monitoring API lives at scheme+host (e.g. "https://api.z.ai"),
/// not at the full openai_base_url path.
fn derive_monitor_base_url(p: &ProviderConfig) -> String {
    // Try to extract scheme+host from openai_base_url first, then anthropic_base_url.
    p.openai_base_url
        .as_deref()
        .or(p.anthropic_base_url.as_deref())
        .and_then(|u| {
            let idx = u.find("://")?;
            let rest = &u[idx + 3..];
            let end = rest.find('/').unwrap_or(rest.len());
            Some(format!("{}{}", &u[..idx + 3], &rest[..end]))
        })
        .unwrap_or_else(|| "https://api.z.ai".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AuthConfig, ProviderKind, ThinkingMode};

    fn cfg(openai: Option<&str>, base: Option<&str>) -> ProviderConfig {
        ProviderConfig {
            name: "zai".into(),
            kind: ProviderKind::Zai,
            auth: AuthConfig::Bearer { value: "x".into() },
            anthropic_base_url: base.map(str::to_string),
            openai_base_url: openai.map(str::to_string),
            reasoning_effort: None,
            thinking_mode: ThinkingMode::SplitOnly,
            max_concurrent: None,
            sanitize_empty_tools: false,
            enabled: true,
        }
    }

    #[test]
    fn minimax_config_builds_dual_capable_provider() {
        let p = build_leaf(
            &ProviderConfig {
                name: "mm".into(),
                kind: ProviderKind::Minimax,
                enabled: true,
                auth: AuthConfig::Bearer { value: "k".into() },
                anthropic_base_url: Some("https://api.minimax.io/anthropic".into()),
                openai_base_url: Some("https://api.minimax.io/v1".into()),
                reasoning_effort: None,
                thinking_mode: crate::config::ThinkingMode::SplitOnly,
                max_concurrent: None,
                sanitize_empty_tools: false,
            },
            reqwest::Client::new(),
        )
        .unwrap();
        let sup = p.supported_formats();
        assert!(sup.anthropic && sup.openai);
    }

    #[test]
    fn provider_with_no_urls_is_rejected() {
        let err = build_leaf(
            &ProviderConfig {
                name: "bad".into(),
                kind: ProviderKind::DeepSeek,
                enabled: true,
                auth: AuthConfig::Bearer { value: "k".into() },
                anthropic_base_url: None,
                openai_base_url: None,
                reasoning_effort: None,
                thinking_mode: crate::config::ThinkingMode::SplitOnly,
                max_concurrent: None,
                sanitize_empty_tools: false,
            },
            reqwest::Client::new(),
        );
        assert!(err.is_err());
    }

    #[test]
    fn build_leaves_excludes_disabled_provider() {
        let cfg = Config {
            providers: vec![
                ProviderConfig {
                    name: "on".into(),
                    kind: ProviderKind::Anthropic,
                    enabled: true,
                    auth: AuthConfig::Passthrough,
                    anthropic_base_url: Some("https://api.anthropic.com".into()),
                    openai_base_url: None,
                    reasoning_effort: None,
                    thinking_mode: ThinkingMode::SplitOnly,
                    max_concurrent: None,
                    sanitize_empty_tools: false,
                },
                ProviderConfig {
                    name: "off".into(),
                    kind: ProviderKind::Anthropic,
                    enabled: false,
                    auth: AuthConfig::Passthrough,
                    anthropic_base_url: None,
                    openai_base_url: None,
                    reasoning_effort: None,
                    thinking_mode: ThinkingMode::SplitOnly,
                    max_concurrent: None,
                    sanitize_empty_tools: false,
                },
            ],
            port: 0,
            proxy_db: std::path::PathBuf::new(),
            pricing_db: std::path::PathBuf::new(),
            routing: vec![],
            affinity: Default::default(),
            quota: vec![],
        };
        let leaves = build_leaves(&cfg.providers, reqwest::Client::new()).unwrap();
        assert!(leaves.contains_key("on"));
        assert!(!leaves.contains_key("off"));
    }

    #[test]
    fn derive_monitor_base_url_strips_path_from_openai_url() {
        let url = derive_monitor_base_url(&cfg(Some("https://api.z.ai/api/coding/paas/v4"), None));
        assert_eq!(url, "https://api.z.ai");
    }

    #[test]
    fn derive_monitor_base_url_falls_back_to_base_url() {
        let url = derive_monitor_base_url(&cfg(None, Some("http://localhost:1234/v1")));
        assert_eq!(url, "http://localhost:1234");
    }

    #[test]
    fn derive_monitor_base_url_handles_url_with_no_path() {
        let url = derive_monitor_base_url(&cfg(Some("https://api.z.ai"), None));
        assert_eq!(url, "https://api.z.ai");
    }

    #[test]
    fn derive_monitor_base_url_defaults_when_unset() {
        let url = derive_monitor_base_url(&cfg(None, None));
        assert_eq!(url, "https://api.z.ai");
    }

    #[test]
    fn build_account_usage_excludes_disabled_provider() {
        let config = Config {
            providers: vec![
                ProviderConfig {
                    name: "on".into(),
                    kind: ProviderKind::Codex,
                    enabled: true,
                    auth: AuthConfig::Bearer {
                        value: "token".into(),
                    },
                    anthropic_base_url: None,
                    openai_base_url: Some("https://example.test/backend-api/codex".into()),
                    reasoning_effort: None,
                    thinking_mode: ThinkingMode::SplitOnly,
                    max_concurrent: None,
                    sanitize_empty_tools: false,
                },
                ProviderConfig {
                    name: "off".into(),
                    kind: ProviderKind::Codex,
                    enabled: false,
                    auth: AuthConfig::Bearer {
                        value: "token".into(),
                    },
                    anthropic_base_url: None,
                    openai_base_url: Some("https://example.test/backend-api/codex".into()),
                    reasoning_effort: None,
                    thinking_mode: ThinkingMode::SplitOnly,
                    max_concurrent: None,
                    sanitize_empty_tools: false,
                },
            ],
            ..empty_config()
        };
        let adapters = build_account_usage(Arc::new(std::sync::RwLock::new(config)));
        assert!(adapters.contains_key("on"));
        assert!(
            !adapters.contains_key("off"),
            "disabled providers must not appear in the account-usage map"
        );
    }

    #[test]
    fn live_account_usage_drops_provider_when_disabled() {
        let config = Arc::new(std::sync::RwLock::new(empty_config()));
        config.write().unwrap().providers.push(ProviderConfig {
            name: "claude".to_string(),
            kind: ProviderKind::Anthropic,
            auth: AuthConfig::AnthropicOAuth {
                access_token: "sk-ant-oat01-x".to_string(),
                refresh_token: "r".to_string(),
                expires_at_ms: 0,
            },
            anthropic_base_url: None,
            openai_base_url: None,
            reasoning_effort: None,
            thinking_mode: ThinkingMode::SplitOnly,
            max_concurrent: None,
            sanitize_empty_tools: false,
            enabled: true,
        });
        let live = LiveAccountUsage::new(config.clone());

        assert!(
            live.adapters().contains_key("claude"),
            "enabled provider should appear"
        );

        // Toggle it off at runtime, as the admin API does.
        config.write().unwrap().providers[0].enabled = false;

        assert!(
            !live.adapters().contains_key("claude"),
            "disabling a provider must remove it from the account-usage map"
        );
    }

    #[test]
    fn build_account_usage_maps_codex_to_supported_adapter() {
        let config = Config {
            providers: vec![ProviderConfig {
                name: "codex-main".to_string(),
                kind: ProviderKind::Codex,
                anthropic_base_url: None,
                openai_base_url: Some("https://example.test/backend-api/codex".to_string()),
                auth: AuthConfig::Bearer {
                    value: "token-123".to_string(),
                },
                reasoning_effort: None,
                thinking_mode: ThinkingMode::SplitOnly,
                max_concurrent: None,
                sanitize_empty_tools: false,
                enabled: true,
            }],
            ..Config {
                port: 0,
                proxy_db: std::path::PathBuf::new(),
                pricing_db: std::path::PathBuf::new(),
                providers: vec![],
                routing: vec![],
                affinity: Default::default(),
                quota: vec![],
            }
        };
        let adapters = build_account_usage(Arc::new(std::sync::RwLock::new(config)));
        let adapter = adapters.get("codex-main").expect("codex adapter exists");

        let result = adapter.fetch_usage();

        assert!(result.is_some(), "Codex should not use NoopAccountUsage");
    }

    fn empty_config() -> Config {
        Config {
            port: 0,
            proxy_db: std::path::PathBuf::new(),
            pricing_db: std::path::PathBuf::new(),
            providers: vec![],
            routing: vec![],
            affinity: Default::default(),
            quota: vec![],
        }
    }

    #[test]
    fn live_account_usage_picks_up_provider_added_at_runtime() {
        let config = Arc::new(std::sync::RwLock::new(empty_config()));
        let live = LiveAccountUsage::new(config.clone());

        // No providers configured yet → empty map.
        assert!(live.adapters().is_empty(), "no providers means no adapters");

        // Add an Anthropic provider at runtime (as the admin API does).
        config.write().unwrap().providers.push(ProviderConfig {
            name: "claude".to_string(),
            kind: ProviderKind::Anthropic,
            auth: AuthConfig::AnthropicOAuth {
                access_token: "sk-ant-oat01-x".to_string(),
                refresh_token: "r".to_string(),
                expires_at_ms: 0,
            },
            anthropic_base_url: None,
            openai_base_url: None,
            reasoning_effort: None,
            thinking_mode: ThinkingMode::SplitOnly,
            max_concurrent: None,
            sanitize_empty_tools: false,
            enabled: true,
        });

        // The registry must surface the newly added provider without a restart.
        let adapters = live.adapters();
        assert!(
            adapters.contains_key("claude"),
            "provider added at runtime must appear in the account-usage map"
        );
    }

    #[test]
    fn live_account_usage_reuses_adapters_while_provider_set_is_stable() {
        let mut config = empty_config();
        config.providers.push(ProviderConfig {
            name: "claude".to_string(),
            kind: ProviderKind::Anthropic,
            auth: AuthConfig::AnthropicOAuth {
                access_token: "sk-ant-oat01-x".to_string(),
                refresh_token: "r".to_string(),
                expires_at_ms: 0,
            },
            anthropic_base_url: None,
            openai_base_url: None,
            reasoning_effort: None,
            thinking_mode: ThinkingMode::SplitOnly,
            max_concurrent: None,
            sanitize_empty_tools: false,
            enabled: true,
        });
        let live = LiveAccountUsage::new(Arc::new(std::sync::RwLock::new(config)));

        let first = live.adapters();
        let second = live.adapters();

        // Same provider set → same adapter instance (preserves response cache).
        assert!(
            Arc::ptr_eq(first.get("claude").unwrap(), second.get("claude").unwrap()),
            "adapter instances must be reused when the provider set is unchanged"
        );
    }
}
