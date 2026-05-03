//! Layered config: defaults < TOML file at `~/.config/cli-router/config.toml`
//! < env var interpolation (`${VAR}` inside string fields).
//!
//! Design notes:
//! - Multiple providers can be active simultaneously; routing rules pick which
//!   one a given request goes to (first-match-wins on glob over the body's
//!   `model` field).
//! - `AuthConfig::Passthrough` keeps Phase 0 behaviour: forward whatever auth
//!   header the client sent. Useful for tests and for the default config
//!   generated when the user has neither a config file nor API keys set.
//! - `CLI_ROUTER_UPSTREAM=zai|anthropic` is honoured for backward
//!   compatibility when the file doesn't exist.

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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub name: String,
    pub kind: ProviderKind,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub base_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Anthropic,
    Zai,
}

/// How the proxy authenticates *to* the upstream when forwarding a request.
///
/// `Passthrough` keeps the client's incoming auth headers intact — this is the
/// default and matches the Phase 0 behaviour. The other variants strip
/// incoming `x-api-key`/`authorization` and inject the configured value.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("toml parse: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("config validation: {0}")]
    Validation(String),
}

impl Config {
    /// Load config from the standard path, env override, or fall back to a
    /// legacy single-provider default driven by env vars.
    /// The resolved config-file path: env override, else XDG default.
    /// The file may or may not exist — `from_env` falls back to defaults.
    pub fn resolved_path() -> PathBuf {
        std::env::var_os("CLI_ROUTER_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(default_config_path)
    }

    pub fn from_env() -> Result<Self, ConfigError> {
        let path = Self::resolved_path();

        let mut cfg = if path.exists() {
            let s = std::fs::read_to_string(&path)?;
            toml::from_str::<Config>(&s)?
        } else {
            Self::legacy_default()
        };

        cfg.interpolate_env();
        cfg.validate()?;
        Ok(cfg)
    }

    /// In-memory default used when no config file exists. Honours
    /// `CLI_ROUTER_UPSTREAM` and reads API key from the matching env var.
    fn legacy_default() -> Self {
        let kind = std::env::var("CLI_ROUTER_UPSTREAM")
            .ok()
            .as_deref()
            .and_then(parse_kind)
            .unwrap_or(ProviderKind::Anthropic);

        let (name, env_key) = match kind {
            ProviderKind::Anthropic => ("anthropic", "ANTHROPIC_API_KEY"),
            ProviderKind::Zai => ("zai", "ZAI_API_KEY"),
        };

        let auth = match std::env::var(env_key).ok() {
            Some(v) if !v.is_empty() => AuthConfig::ApiKey { value: v },
            _ => AuthConfig::Passthrough,
        };

        Config {
            port: default_port(),
            proxy_db: default_proxy_db(),
            pricing_db: default_pricing_db(),
            providers: vec![ProviderConfig {
                name: name.into(),
                kind,
                auth,
                base_url: None,
            }],
            routing: vec![RoutingRule {
                match_spec: MatchSpec {
                    model: Some("*".into()),
                },
                provider: name.into(),
                fallback: vec![],
                strategy: Default::default(),
                priority: None,
            }],
        }
    }

    /// Replace `${VAR}` occurrences in every user-visible string field with
    /// the corresponding env var value (empty string if unset).
    fn interpolate_env(&mut self) {
        for p in &mut self.providers {
            if let Some(b) = &mut p.base_url {
                *b = interpolate(b);
            }
            match &mut p.auth {
                AuthConfig::ApiKey { value } | AuthConfig::Bearer { value } => {
                    *value = interpolate(value);
                }
                AuthConfig::Passthrough => {}
                AuthConfig::AnthropicOAuth { .. } => {
                    // OAuth tokens are set by the daemon, not env vars.
                }
            }
        }
    }

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

fn parse_kind(s: &str) -> Option<ProviderKind> {
    match s.trim().to_ascii_lowercase().as_str() {
        "anthropic" => Some(ProviderKind::Anthropic),
        "zai" | "z.ai" | "z-ai" => Some(ProviderKind::Zai),
        _ => None,
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn default_port() -> u16 {
    8787
}

fn default_proxy_db() -> PathBuf {
    home().join(".local/share/cli-router/proxy.db")
}

fn default_pricing_db() -> PathBuf {
    home().join(".local/share/cli-router/pricing.db")
}

fn default_config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"));
    base.join("cli-router/config.toml")
}

/// Replace `${VAR}` with the env var value (or empty string if unset).
/// Unknown patterns like `${` without a closing `}` are left untouched.
fn interpolate(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            if let Some(end) = bytes[i + 2..].iter().position(|&b| b == b'}') {
                let var_name = &s[i + 2..i + 2 + end];
                let value = std::env::var(var_name).unwrap_or_default();
                out.push_str(&value);
                i += 2 + end + 1;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolate_replaces_env_vars() {
        // SAFETY: this var name is unique to this test
        std::env::set_var("CLI_ROUTER_TEST_INTERPOLATE_X", "hello");
        assert_eq!(
            interpolate("a-${CLI_ROUTER_TEST_INTERPOLATE_X}-b"),
            "a-hello-b"
        );
    }

    #[test]
    fn interpolate_empty_for_unset_var() {
        assert_eq!(interpolate("${CLI_ROUTER_DEFINITELY_UNSET_XYZ}"), "");
    }

    #[test]
    fn interpolate_leaves_unbalanced_braces_alone() {
        assert_eq!(interpolate("foo${bar"), "foo${bar");
    }

    #[test]
    fn parse_kind_accepts_aliases() {
        assert_eq!(parse_kind("anthropic"), Some(ProviderKind::Anthropic));
        assert_eq!(parse_kind("ANTHROPIC"), Some(ProviderKind::Anthropic));
        assert_eq!(parse_kind("zai"), Some(ProviderKind::Zai));
        assert_eq!(parse_kind("Z.AI"), Some(ProviderKind::Zai));
        assert_eq!(parse_kind("z-ai"), Some(ProviderKind::Zai));
        assert_eq!(parse_kind("openai"), None);
    }

    #[test]
    fn toml_parses_full_config() {
        let toml_str = r#"
            port = 9000
            [[providers]]
            name = "anthropic"
            kind = "anthropic"
            auth = { type = "api_key", value = "sk-abc" }

            [[providers]]
            name = "zai"
            kind = "zai"
            auth = { type = "api_key", value = "zai-xyz" }

            [[routing]]
            match = { model = "glm-*" }
            provider = "zai"

            [[routing]]
            match = { model = "*" }
            provider = "anthropic"
            fallback = ["zai"]
        "#;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.port, 9000);
        assert_eq!(cfg.providers.len(), 2);
        assert_eq!(cfg.providers[0].name, "anthropic");
        assert_eq!(cfg.providers[1].kind, ProviderKind::Zai);
        assert_eq!(cfg.routing.len(), 2);
        assert_eq!(cfg.routing[0].match_spec.model.as_deref(), Some("glm-*"));
        assert_eq!(cfg.routing[1].fallback, vec!["zai".to_string()]);
    }

    #[test]
    fn auth_passthrough_is_default() {
        let toml_str = r#"
            [[providers]]
            name = "x"
            kind = "anthropic"

            [[routing]]
            match = { model = "*" }
            provider = "x"
        "#;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        assert!(matches!(cfg.providers[0].auth, AuthConfig::Passthrough));
    }

    #[test]
    fn validate_rejects_routing_with_unknown_provider() {
        let toml_str = r#"
            [[providers]]
            name = "anthropic"
            kind = "anthropic"

            [[routing]]
            match = { model = "*" }
            provider = "nonexistent"
        "#;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        let err = cfg.validate().unwrap_err();
        assert!(format!("{err}").contains("nonexistent"));
    }

    #[test]
    fn validate_rejects_duplicate_provider_names() {
        let toml_str = r#"
            [[providers]]
            name = "x"
            kind = "anthropic"
            [[providers]]
            name = "x"
            kind = "zai"
            [[routing]]
            match = { model = "*" }
            provider = "x"
        "#;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        assert!(format!("{}", cfg.validate().unwrap_err()).contains("duplicate"));
    }

    #[test]
    fn validate_rejects_empty_providers() {
        let cfg = Config {
            port: 8787,
            proxy_db: PathBuf::new(),
            pricing_db: PathBuf::new(),
            providers: vec![],
            routing: vec![],
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn toml_parses_anthropic_oauth_auth() {
        let toml_str = r#"
            [[providers]]
            name = "anthropic"
            kind = "anthropic"
            auth = { type = "anthropic_oauth", access_token = "sk-ant-oat-abc", refresh_token = "sk-ant-oar-xyz", expires_at_ms = 1746300000000 }

            [[routing]]
            match = { model = "*" }
            provider = "anthropic"
        "#;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        assert!(matches!(
            cfg.providers[0].auth,
            AuthConfig::AnthropicOAuth { .. }
        ));
        if let AuthConfig::AnthropicOAuth {
            access_token,
            refresh_token,
            expires_at_ms,
        } = &cfg.providers[0].auth
        {
            assert_eq!(access_token, "sk-ant-oat-abc");
            assert_eq!(refresh_token, "sk-ant-oar-xyz");
            assert_eq!(*expires_at_ms, 1746300000000);
        }
    }

    #[test]
    fn validate_rejects_unknown_fallback_provider() {
        let toml_str = r#"
            [[providers]]
            name = "anthropic"
            kind = "anthropic"

            [[routing]]
            match = { model = "*" }
            provider = "anthropic"
            fallback = ["ghost"]
        "#;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        assert!(format!("{}", cfg.validate().unwrap_err()).contains("ghost"));
    }
}
