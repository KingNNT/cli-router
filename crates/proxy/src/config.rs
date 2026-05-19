//! Application configuration types. Config is persisted in SQLite via
//! `ConfigRepository`; this module holds the domain types and validation.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_proxy_db")]
    pub proxy_db: PathBuf,
    #[serde(default = "default_pricing_db")]
    pub pricing_db: PathBuf,
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
    #[serde(default)]
    pub routing: Vec<RoutingRule>,
    /// Conversation-affinity hashing. Defaults to enabled with sensible header list.
    #[serde(default)]
    pub affinity: AffinityConfig,
    #[serde(default)]
    pub quota: Vec<QuotaRule>,
    /// Port for the Swagger UI / ReDoc docs server. Default: 8788.
    #[serde(default = "default_docs_port")]
    pub docs_port: u16,
    /// Whether to start the docs server. Default: true.
    #[serde(default = "default_true")]
    pub docs_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub name: String,
    pub kind: ProviderKind,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub openai_base_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Anthropic,
    Zai,
    #[serde(alias = "deepseek")]
    DeepSeek,
    #[serde(alias = "openai")]
    OpenAi,
    Codex,
}

/// How the proxy authenticates *to* the upstream when forwarding a request.
///
/// `Passthrough` keeps the client's incoming auth headers intact — this is the
/// default and matches the Phase 0 behaviour. The other variants strip
/// incoming `x-api-key`/`authorization` and inject the configured value.
#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthConfig {
    #[default]
    Passthrough,
    ApiKey {
        value: String,
    },
    Bearer {
        value: String,
    },
    /// Full Anthropic OAuth session. The proxy auto-refreshes the access token
    /// before expiry using the refresh token, and retries once on 401.
    #[serde(rename = "anthropic_oauth")]
    AnthropicOAuth {
        access_token: String,
        refresh_token: String,
        /// Unix epoch millis when the access token expires.
        expires_at_ms: u64,
    },
    /// OpenAI OAuth session. Same refresh semantics as AnthropicOAuth.
    #[serde(rename = "openai_oauth")]
    OpenAiOAuth {
        access_token: String,
        refresh_token: String,
        /// Unix epoch millis when the access token expires.
        expires_at_ms: u64,
    },
    /// Auto-read tokens from `~/.codex/auth.json` (Codex CLI cache).
    /// The proxy reads the file at startup and during background refresh.
    /// No tokens are stored in config.toml.
    #[serde(rename = "codex_auto")]
    CodexAuto,
}

// Manual Debug to keep secrets out of logs. The derived Debug would print
// every API key / OAuth token verbatim — anything that debug-prints a Config
// (e.g. tracing::info!(?cfg)) would leak credentials to log files.
impl std::fmt::Debug for AuthConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        const REDACTED: &str = "<redacted>";
        match self {
            AuthConfig::Passthrough => f.debug_tuple("Passthrough").finish(),
            AuthConfig::ApiKey { .. } => {
                f.debug_struct("ApiKey").field("value", &REDACTED).finish()
            }
            AuthConfig::Bearer { .. } => {
                f.debug_struct("Bearer").field("value", &REDACTED).finish()
            }
            AuthConfig::AnthropicOAuth { expires_at_ms, .. } => f
                .debug_struct("AnthropicOAuth")
                .field("access_token", &REDACTED)
                .field("refresh_token", &REDACTED)
                .field("expires_at_ms", expires_at_ms)
                .finish(),
            AuthConfig::OpenAiOAuth { expires_at_ms, .. } => f
                .debug_struct("OpenAiOAuth")
                .field("access_token", &REDACTED)
                .field("refresh_token", &REDACTED)
                .field("expires_at_ms", expires_at_ms)
                .finish(),
            AuthConfig::CodexAuto => f.debug_tuple("CodexAuto").finish(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RoutingStrategy {
    /// Always try `provider` first, then `fallback` in order on 5xx/error.
    #[default]
    Failover,
    /// Round-robin across all providers in the pool. On 429/5xx, skip and
    /// try next with cooldown.
    RoundRobin,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingRule {
    #[serde(rename = "match")]
    pub match_spec: MatchSpec,
    pub provider: String,
    #[serde(default)]
    pub fallback: Vec<String>,
    #[serde(default)]
    pub strategy: RoutingStrategy,
    /// Lower number = higher priority = checked first. Defaults to the
    /// rule's position in the TOML array (0, 1, 2, …). Rules with the same
    /// priority keep their file order.
    #[serde(default)]
    pub priority: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchSpec {
    /// Glob pattern matched against the request body's `model` field.
    /// Examples: `"glm-*"`, `"claude-*"`, `"*"` (catch-all).
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuotaRule {
    pub provider: String,
    pub window: String,
    #[serde(default)]
    pub max_requests: Option<u64>,
    #[serde(default)]
    pub max_input_tokens: Option<u64>,
    #[serde(default)]
    pub max_output_tokens: Option<u64>,
    #[serde(default = "default_warn_pct")]
    pub warn_pct: u8,
}

fn default_warn_pct() -> u8 {
    80
}

impl QuotaRule {
    pub fn to_domain(&self) -> Result<crate::domain::quota::QuotaConfig, ConfigError> {
        let window = crate::domain::quota::parse_window(&self.window)
            .map_err(|e| ConfigError::InvalidQuota(format!("provider {}: {e}", self.provider)))?;
        if self.warn_pct > 100 {
            return Err(ConfigError::InvalidQuota(format!(
                "provider {}: warn_pct must be 0..=100",
                self.provider
            )));
        }
        Ok(crate::domain::quota::QuotaConfig {
            provider: self.provider.clone(),
            window,
            max_requests: self.max_requests,
            max_input_tokens: self.max_input_tokens,
            max_output_tokens: self.max_output_tokens,
            warn_pct: self.warn_pct,
        })
    }
}

/// Affinity hashing config — controls how requests are pinned to upstream keys.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AffinityConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_affinity_headers")]
    pub headers: Vec<String>,
}

impl Default for AffinityConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            headers: default_affinity_headers(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_affinity_headers() -> Vec<String> {
    vec![
        "x-session-id".to_string(),
        "anthropic-session-id".to_string(),
    ]
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("config validation: {0}")]
    Validation(String),
    #[error("invalid quota: {0}")]
    InvalidQuota(String),
}

impl Config {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.providers.is_empty() {
            return Err(ConfigError::Validation("no providers configured".into()));
        }
        let mut names = std::collections::HashSet::new();
        for p in &self.providers {
            if !names.insert(&p.name) {
                return Err(ConfigError::Validation(format!(
                    "duplicate provider name: {}",
                    p.name
                )));
            }
        }
        if self.routing.is_empty() {
            return Err(ConfigError::Validation(
                "no routing rules configured".into(),
            ));
        }
        for (i, r) in self.routing.iter().enumerate() {
            if !self.providers.iter().any(|p| p.name == r.provider) {
                return Err(ConfigError::Validation(format!(
                    "routing rule {i} references unknown provider '{}'",
                    r.provider
                )));
            }
            for fb in &r.fallback {
                if !self.providers.iter().any(|p| p.name == *fb) {
                    return Err(ConfigError::Validation(format!(
                        "routing rule {i} fallback references unknown provider '{fb}'"
                    )));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
fn parse_kind(s: &str) -> Option<ProviderKind> {
    match s.trim().to_ascii_lowercase().as_str() {
        "anthropic" => Some(ProviderKind::Anthropic),
        "zai" | "z.ai" | "z-ai" => Some(ProviderKind::Zai),
        "deepseek" | "deep-seek" => Some(ProviderKind::DeepSeek),
        "openai" | "open_ai" => Some(ProviderKind::OpenAi),
        "codex" => Some(ProviderKind::Codex),
        _ => None,
    }
}

fn default_port() -> u16 {
    8787
}

fn default_docs_port() -> u16 {
    8788
}

fn default_proxy_db() -> PathBuf {
    home().join(".local/share/cli-router/proxy.db")
}

fn default_pricing_db() -> PathBuf {
    home().join(".local/share/cli-router/pricing.db")
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_config_debug_redacts_secrets() {
        let cases = [
            AuthConfig::ApiKey {
                value: "sk-leak-apikey".into(),
            },
            AuthConfig::Bearer {
                value: "sk-leak-bearer".into(),
            },
            AuthConfig::AnthropicOAuth {
                access_token: "sk-leak-access".into(),
                refresh_token: "sk-leak-refresh".into(),
                expires_at_ms: 1_700_000_000_000,
            },
            AuthConfig::OpenAiOAuth {
                access_token: "sk-leak-oai-access".into(),
                refresh_token: "sk-leak-oai-refresh".into(),
                expires_at_ms: 1_700_000_000_000,
            },
        ];
        for auth in &cases {
            let s = format!("{auth:?}");
            assert!(!s.contains("sk-leak"), "debug leaked secret: {s}");
            assert!(s.contains("redacted"), "debug should mark redaction: {s}");
        }

        // Sanity: Passthrough has nothing to redact and non-secret fields stay visible.
        assert_eq!(format!("{:?}", AuthConfig::Passthrough), "Passthrough");
        let oauth = AuthConfig::AnthropicOAuth {
            access_token: "x".into(),
            refresh_token: "y".into(),
            expires_at_ms: 42,
        };
        assert!(format!("{oauth:?}").contains("expires_at_ms: 42"));
    }

    #[test]
    fn parse_kind_accepts_aliases() {
        assert_eq!(parse_kind("anthropic"), Some(ProviderKind::Anthropic));
        assert_eq!(parse_kind("ANTHROPIC"), Some(ProviderKind::Anthropic));
        assert_eq!(parse_kind("zai"), Some(ProviderKind::Zai));
        assert_eq!(parse_kind("Z.AI"), Some(ProviderKind::Zai));
        assert_eq!(parse_kind("z-ai"), Some(ProviderKind::Zai));
        assert_eq!(parse_kind("openai"), Some(ProviderKind::OpenAi));
    }

    #[test]
    fn parse_kind_accepts_deepseek_aliases() {
        assert_eq!(parse_kind("deepseek"), Some(ProviderKind::DeepSeek));
        assert_eq!(parse_kind("deep-seek"), Some(ProviderKind::DeepSeek));
        assert_eq!(parse_kind("DEEPSEEK"), Some(ProviderKind::DeepSeek));
    }

    #[test]
    fn parse_kind_accepts_openai_aliases() {
        assert_eq!(parse_kind("openai"), Some(ProviderKind::OpenAi));
        assert_eq!(parse_kind("open_ai"), Some(ProviderKind::OpenAi));
        assert_eq!(parse_kind("OpenAI"), Some(ProviderKind::OpenAi));
    }

    #[test]
    fn parse_kind_accepts_codex() {
        assert_eq!(parse_kind("codex"), Some(ProviderKind::Codex));
        assert_eq!(parse_kind("CODEX"), Some(ProviderKind::Codex));
        assert_eq!(parse_kind("Codex"), Some(ProviderKind::Codex));
    }

    #[test]
    fn validate_rejects_empty_providers() {
        let cfg = Config {
            port: 8787,
            proxy_db: PathBuf::new(),
            pricing_db: PathBuf::new(),
            providers: vec![],
            routing: vec![],
            affinity: AffinityConfig::default(),
            quota: Vec::new(),
            docs_port: 8788,
            docs_enabled: true,
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_rejects_routing_with_unknown_provider() {
        let cfg = Config {
            port: 8787,
            proxy_db: PathBuf::new(),
            pricing_db: PathBuf::new(),
            providers: vec![ProviderConfig {
                name: "anthropic".into(),
                kind: ProviderKind::Anthropic,
                auth: AuthConfig::Passthrough,
                base_url: None,
                openai_base_url: None,
            }],
            routing: vec![RoutingRule {
                match_spec: MatchSpec {
                    model: Some("*".into()),
                },
                provider: "nonexistent".into(),
                fallback: vec![],
                strategy: Default::default(),
                priority: None,
            }],
            affinity: AffinityConfig::default(),
            quota: Vec::new(),
            docs_port: 8788,
            docs_enabled: true,
        };
        let err = cfg.validate().unwrap_err();
        assert!(format!("{err}").contains("nonexistent"));
    }

    #[test]
    fn validate_rejects_duplicate_provider_names() {
        let cfg = Config {
            port: 8787,
            proxy_db: PathBuf::new(),
            pricing_db: PathBuf::new(),
            providers: vec![
                ProviderConfig {
                    name: "x".into(),
                    kind: ProviderKind::Anthropic,
                    auth: AuthConfig::Passthrough,
                    base_url: None,
                    openai_base_url: None,
                },
                ProviderConfig {
                    name: "x".into(),
                    kind: ProviderKind::Zai,
                    auth: AuthConfig::Passthrough,
                    base_url: None,
                    openai_base_url: None,
                },
            ],
            routing: vec![RoutingRule {
                match_spec: MatchSpec {
                    model: Some("*".into()),
                },
                provider: "x".into(),
                fallback: vec![],
                strategy: Default::default(),
                priority: None,
            }],
            affinity: AffinityConfig::default(),
            quota: Vec::new(),
            docs_port: 8788,
            docs_enabled: true,
        };
        assert!(format!("{}", cfg.validate().unwrap_err()).contains("duplicate"));
    }

    #[test]
    fn validate_rejects_unknown_fallback_provider() {
        let cfg = Config {
            port: 8787,
            proxy_db: PathBuf::new(),
            pricing_db: PathBuf::new(),
            providers: vec![ProviderConfig {
                name: "anthropic".into(),
                kind: ProviderKind::Anthropic,
                auth: AuthConfig::Passthrough,
                base_url: None,
                openai_base_url: None,
            }],
            routing: vec![RoutingRule {
                match_spec: MatchSpec {
                    model: Some("*".into()),
                },
                provider: "anthropic".into(),
                fallback: vec!["ghost".into()],
                strategy: Default::default(),
                priority: None,
            }],
            affinity: AffinityConfig::default(),
            quota: Vec::new(),
            docs_port: 8788,
            docs_enabled: true,
        };
        assert!(format!("{}", cfg.validate().unwrap_err()).contains("ghost"));
    }

    #[test]
    fn quota_invalid_window_string_errors_at_conversion() {
        let rule = QuotaRule {
            provider: "anthropic".into(),
            window: "not-valid".into(),
            max_requests: None,
            max_input_tokens: None,
            max_output_tokens: None,
            warn_pct: 80,
        };
        let err = rule.to_domain().unwrap_err();
        assert!(format!("{err}").contains("anthropic"));
    }

    #[test]
    fn quota_warn_pct_over_100_errors_at_conversion() {
        let rule = QuotaRule {
            provider: "zai".into(),
            window: "rolling:1h".into(),
            max_requests: Some(10),
            max_input_tokens: None,
            max_output_tokens: None,
            warn_pct: 101,
        };
        let err = rule.to_domain().unwrap_err();
        assert!(format!("{err}").contains("warn_pct"));
    }

    #[test]
    fn codex_auto_auth_debug_does_not_leak() {
        let auth = AuthConfig::CodexAuto;
        let s = format!("{auth:?}");
        assert_eq!(s, "CodexAuto");
    }
}
