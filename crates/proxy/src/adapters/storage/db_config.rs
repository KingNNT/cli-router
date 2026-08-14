//! SQLite-backed config repository — reads/writes normalized config tables.

use std::sync::Mutex;

use rusqlite::Connection;

use crate::application::ports::ConfigRepository;
use crate::config::{
    AffinityConfig, AuthConfig, Config, ConfigError, FormatMode, MatchSpec, ProviderConfig,
    ProviderKind, QuotaRule, RoutingRule, RoutingStrategy, ThinkingLevel, ThinkingMode,
};

/// Convert rusqlite errors into ConfigError::Validation.
fn db_err(e: rusqlite::Error) -> ConfigError {
    ConfigError::Validation(e.to_string())
}

pub struct DbConfigRepository {
    conn: Mutex<Connection>,
    /// Default DB path (used for settings if not set in DB).
    db_path: String,
}

impl DbConfigRepository {
    pub fn new(conn: Connection, db_path: String) -> Self {
        Self {
            conn: Mutex::new(conn),
            db_path,
        }
    }
}

impl ConfigRepository for DbConfigRepository {
    fn load(&self) -> Result<Config, ConfigError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| ConfigError::Validation(e.to_string()))?;

        let port = load_setting(&conn, "port")
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(8787);

        let proxy_db = load_setting(&conn, "proxy_db")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from(&self.db_path));

        let pricing_db = load_setting(&conn, "pricing_db")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(default_pricing_db);

        let affinity = AffinityConfig {
            enabled: load_setting(&conn, "affinity_enabled")
                .and_then(|s| s.parse::<bool>().ok())
                .unwrap_or(true),
            headers: load_setting(&conn, "affinity_headers")
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_else(default_affinity_headers),
        };

        let providers = load_providers(&conn)?;
        let routing = load_routing(&conn)?;
        let quota = load_quota(&conn)?;

        Ok(Config {
            port,
            proxy_db,
            pricing_db,
            providers,
            routing,
            affinity,
            quota,
        })
    }

    fn save(&self, config: &Config) -> Result<(), ConfigError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| ConfigError::Validation(e.to_string()))?;

        let tx = conn.unchecked_transaction().map_err(db_err)?;

        // Save settings
        {
            tx.execute("DELETE FROM settings", []).map_err(db_err)?;
            let mut stmt = tx
                .prepare("INSERT INTO settings (key, value) VALUES (?1, ?2)")
                .map_err(db_err)?;
            stmt.execute(["port", &config.port.to_string()])
                .map_err(db_err)?;
            stmt.execute(["proxy_db", &config.proxy_db.display().to_string()])
                .map_err(db_err)?;
            stmt.execute(["pricing_db", &config.pricing_db.display().to_string()])
                .map_err(db_err)?;
            stmt.execute(["affinity_enabled", &config.affinity.enabled.to_string()])
                .map_err(db_err)?;
            let headers_json = serde_json::to_string(&config.affinity.headers)
                .map_err(|e| ConfigError::Validation(e.to_string()))?;
            stmt.execute(["affinity_headers", &headers_json])
                .map_err(db_err)?;
        }

        // Save providers
        {
            tx.execute("DELETE FROM providers", []).map_err(db_err)?;
            let mut stmt = tx
                .prepare(
                    "INSERT INTO providers (name, kind, anthropic_base_url, openai_base_url, thinking_mode, auth_type,
                     auth_api_key, auth_bearer, auth_access_token, auth_refresh_token, auth_expires_at_ms, max_concurrent, sanitize_empty_tools, enabled, format_mode, thinking_level, thinking_force, model_formats)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
                )
                .map_err(db_err)?;
            for p in &config.providers {
                let kind_str = kind_to_str(p.kind);
                let (auth_type, ak, bearer, at, rt, exp) = auth_to_columns(&p.auth);
                stmt.execute(rusqlite::params![
                    p.name,
                    kind_str,
                    p.anthropic_base_url,
                    p.openai_base_url,
                    match p.thinking_mode {
                        ThinkingMode::SplitOnly => "split_only",
                        ThinkingMode::StripAll => "strip_all",
                    },
                    auth_type,
                    ak,
                    bearer,
                    at,
                    rt,
                    exp,
                    p.max_concurrent.map(|v| v as i64),
                    p.sanitize_empty_tools as i64,
                    p.enabled as i64,
                    match p.format_mode {
                        FormatMode::Both => "both",
                        FormatMode::Anthropic => "anthropic",
                        FormatMode::OpenAi => "openai",
                    },
                    p.thinking_level.as_str(),
                    p.thinking_force as i64,
                    p.model_formats,
                ])
                .map_err(db_err)?;
            }
        }

        // Save routing rules
        {
            tx.execute("DELETE FROM routing_rules", [])
                .map_err(db_err)?;
            let mut stmt = tx
                .prepare(
                    "INSERT INTO routing_rules (priority, provider, model_glob, strategy, fallback)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                )
                .map_err(db_err)?;
            for (i, r) in config.routing.iter().enumerate() {
                let strategy_str = strategy_to_str(r.strategy);
                let fallback_str = r.fallback.join(",");
                stmt.execute(rusqlite::params![
                    i as i64,
                    r.provider,
                    r.match_spec.model.as_deref().unwrap_or("*"),
                    strategy_str,
                    fallback_str,
                ])
                .map_err(db_err)?;
            }
        }

        // Save quota rules
        {
            tx.execute("DELETE FROM quota_rules", []).map_err(db_err)?;
            let mut stmt = tx
                .prepare(
                    "INSERT INTO quota_rules (provider, window, max_requests, max_input_tokens, max_output_tokens, warn_pct)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                )
                .map_err(db_err)?;
            for q in &config.quota {
                stmt.execute(rusqlite::params![
                    q.provider,
                    q.window,
                    q.max_requests,
                    q.max_input_tokens,
                    q.max_output_tokens,
                    q.warn_pct,
                ])
                .map_err(db_err)?;
            }
        }

        tx.commit().map_err(db_err)?;
        Ok(())
    }
}

// -- Helpers --

/// Treat an empty URL string as "not configured".
fn none_if_empty(v: Option<String>) -> Option<String> {
    v.filter(|s| !s.is_empty())
}

fn load_setting(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
        r.get(0)
    })
    .ok()
}

fn load_providers(conn: &Connection) -> Result<Vec<ProviderConfig>, ConfigError> {
    let mut stmt = conn
        .prepare(
            "SELECT name, kind, anthropic_base_url, openai_base_url, thinking_mode, auth_type,
                    auth_api_key, auth_bearer, auth_access_token, auth_refresh_token, auth_expires_at_ms,
                    max_concurrent, sanitize_empty_tools, enabled, format_mode, thinking_level, thinking_force,
                    model_formats
             FROM providers ORDER BY id",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map([], |row| {
            let kind_str: String = row.get(1)?;
            let auth_type_str: String = row.get(5)?;
            let thinking_mode_str: String = row.get(4)?;
            Ok(ProviderConfig {
                name: row.get(0)?,
                kind: parse_kind(&kind_str),
                anthropic_base_url: none_if_empty(row.get(2)?),
                openai_base_url: none_if_empty(row.get(3)?),
                thinking_mode: {
                    match thinking_mode_str.as_str() {
                        "strip_all" => ThinkingMode::StripAll,
                        _ => ThinkingMode::SplitOnly,
                    }
                },
                auth: columns_to_auth(
                    &auth_type_str,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                ),
                max_concurrent: row.get::<_, Option<i64>>(11)?.map(|v| v.max(0) as usize),
                sanitize_empty_tools: row.get::<_, i64>(12)? != 0,
                enabled: row.get::<_, i64>(13)? != 0,
                format_mode: match row.get::<_, String>(14)?.as_str() {
                    "anthropic" => FormatMode::Anthropic,
                    "openai" => FormatMode::OpenAi,
                    _ => FormatMode::Both,
                },
                thinking_level: ThinkingLevel::parse(&row.get::<_, String>(15)?)
                    .unwrap_or_default(),
                thinking_force: row.get::<_, i64>(16)? != 0,
                model_formats: none_if_empty(row.get(17)?),
            })
        })
        .map_err(db_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
}

fn load_routing(conn: &Connection) -> Result<Vec<RoutingRule>, ConfigError> {
    let mut stmt = conn
        .prepare(
            "SELECT provider, model_glob, strategy, fallback FROM routing_rules ORDER BY priority",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map([], |row| {
            let model_glob: String = row.get(1)?;
            let strategy_str: String = row.get(2)?;
            let fallback_str: String = row.get(3)?;
            Ok(RoutingRule {
                match_spec: MatchSpec {
                    model: if model_glob == "*" {
                        None
                    } else {
                        Some(model_glob)
                    },
                },
                provider: row.get(0)?,
                fallback: if fallback_str.is_empty() {
                    vec![]
                } else {
                    fallback_str.split(',').map(String::from).collect()
                },
                strategy: parse_strategy(&strategy_str),
                priority: None,
            })
        })
        .map_err(db_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
}

fn load_quota(conn: &Connection) -> Result<Vec<QuotaRule>, ConfigError> {
    let mut stmt = conn
        .prepare(
            "SELECT provider, window, max_requests, max_input_tokens, max_output_tokens, warn_pct FROM quota_rules",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map([], |row| {
            Ok(QuotaRule {
                provider: row.get(0)?,
                window: row.get(1)?,
                max_requests: row.get(2)?,
                max_input_tokens: row.get(3)?,
                max_output_tokens: row.get(4)?,
                warn_pct: row.get(5)?,
            })
        })
        .map_err(db_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
}

fn parse_kind(s: &str) -> ProviderKind {
    match s.trim().to_ascii_lowercase().as_str() {
        "anthropic" => ProviderKind::Anthropic,
        "zai" | "z.ai" | "z-ai" => ProviderKind::Zai,
        "deepseek" | "deep_seek" => ProviderKind::DeepSeek,
        "openai" | "open_ai" => ProviderKind::OpenAi,
        "codex" => ProviderKind::Codex,
        "minimax" => ProviderKind::Minimax,
        "kimi" | "moonshot" => ProviderKind::Kimi,
        "opencode_go" | "opencode-go" => ProviderKind::OpencodeGo,
        _ => ProviderKind::Anthropic,
    }
}

fn kind_to_str(k: ProviderKind) -> &'static str {
    match k {
        ProviderKind::Anthropic => "anthropic",
        ProviderKind::Zai => "zai",
        ProviderKind::DeepSeek => "deepseek",
        ProviderKind::OpenAi => "openai",
        ProviderKind::Codex => "codex",
        ProviderKind::Minimax => "minimax",
        ProviderKind::Kimi => "kimi",
        ProviderKind::OpencodeGo => "opencode_go",
    }
}

fn parse_strategy(s: &str) -> RoutingStrategy {
    match s {
        "round_robin" => RoutingStrategy::RoundRobin,
        _ => RoutingStrategy::Failover,
    }
}

fn strategy_to_str(s: RoutingStrategy) -> &'static str {
    match s {
        RoutingStrategy::Failover => "failover",
        RoutingStrategy::RoundRobin => "round_robin",
    }
}

fn columns_to_auth(
    auth_type: &str,
    api_key: Option<String>,
    bearer: Option<String>,
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_at_ms: Option<u64>,
) -> AuthConfig {
    match auth_type {
        "api_key" => AuthConfig::ApiKey {
            value: api_key.unwrap_or_default(),
        },
        "bearer" => AuthConfig::Bearer {
            value: bearer.unwrap_or_default(),
        },
        "anthropic_oauth" => AuthConfig::AnthropicOAuth {
            access_token: access_token.unwrap_or_default(),
            refresh_token: refresh_token.unwrap_or_default(),
            expires_at_ms: expires_at_ms.unwrap_or(0),
        },
        "openai_oauth" => AuthConfig::OpenAiOAuth {
            access_token: access_token.unwrap_or_default(),
            refresh_token: refresh_token.unwrap_or_default(),
            expires_at_ms: expires_at_ms.unwrap_or(0),
        },
        "codex_auto" => AuthConfig::CodexAuto,
        _ => AuthConfig::Passthrough,
    }
}

#[allow(clippy::type_complexity)]
fn auth_to_columns(
    auth: &AuthConfig,
) -> (
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<u64>,
) {
    match auth {
        AuthConfig::Passthrough => ("passthrough".into(), None, None, None, None, None),
        AuthConfig::ApiKey { value } => (
            "api_key".into(),
            Some(value.clone()),
            None,
            None,
            None,
            None,
        ),
        AuthConfig::Bearer { value } => {
            ("bearer".into(), None, Some(value.clone()), None, None, None)
        }
        AuthConfig::AnthropicOAuth {
            access_token,
            refresh_token,
            expires_at_ms,
        } => (
            "anthropic_oauth".into(),
            None,
            None,
            Some(access_token.clone()),
            Some(refresh_token.clone()),
            Some(*expires_at_ms),
        ),
        AuthConfig::OpenAiOAuth {
            access_token,
            refresh_token,
            expires_at_ms,
        } => (
            "openai_oauth".into(),
            None,
            None,
            Some(access_token.clone()),
            Some(refresh_token.clone()),
            Some(*expires_at_ms),
        ),
        AuthConfig::CodexAuto => ("codex_auto".into(), None, None, None, None, None),
    }
}

fn default_pricing_db() -> std::path::PathBuf {
    std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_default()
        .join(".local/share/cli-router/pricing.db")
}

fn default_affinity_headers() -> Vec<String> {
    vec![
        "x-session-id".to_string(),
        "anthropic-session-id".to_string(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::storage::schema::ensure_current;

    fn test_repo() -> DbConfigRepository {
        let conn = Connection::open_in_memory().unwrap();
        ensure_current(&conn).unwrap();
        DbConfigRepository::new(conn, "/tmp/test.db".to_string())
    }

    #[test]
    fn load_returns_defaults_when_empty() {
        let repo = test_repo();
        let cfg = repo.load().unwrap();
        assert_eq!(cfg.port, 8787);
        assert!(cfg.providers.is_empty());
        assert!(cfg.routing.is_empty());
        assert!(cfg.quota.is_empty());
        assert!(cfg.affinity.enabled);
    }

    #[test]
    fn save_then_load_round_trips() {
        let repo = test_repo();
        let mut cfg = repo.load().unwrap();
        cfg.port = 9999;
        cfg.providers.push(ProviderConfig {
            thinking_level: crate::config::ThinkingLevel::High,
            thinking_force: true,
            name: "test".into(),
            kind: ProviderKind::Zai,
            auth: AuthConfig::Bearer {
                value: "secret".into(),
            },
            anthropic_base_url: Some("https://example.com".into()),
            openai_base_url: Some("https://example.com/v1".into()),
            thinking_mode: ThinkingMode::SplitOnly,
            format_mode: crate::config::FormatMode::Both,
            max_concurrent: None,
            sanitize_empty_tools: false,
            enabled: true,
            model_formats: None,
        });
        cfg.routing.push(RoutingRule {
            match_spec: MatchSpec {
                model: Some("glm-*".into()),
            },
            provider: "test".into(),
            fallback: vec!["fallback".into()],
            strategy: RoutingStrategy::Failover,
            priority: None,
        });
        cfg.quota.push(QuotaRule {
            provider: "test".into(),
            window: "rolling:1h".into(),
            max_requests: Some(100),
            max_input_tokens: None,
            max_output_tokens: None,
            warn_pct: 80,
        });

        repo.save(&cfg).unwrap();
        let loaded = repo.load().unwrap();

        assert_eq!(loaded.port, 9999);
        assert_eq!(loaded.providers.len(), 1);
        assert_eq!(loaded.providers[0].name, "test");
        assert_eq!(loaded.providers[0].kind, ProviderKind::Zai);
        assert_eq!(
            loaded.providers[0].thinking_level,
            crate::config::ThinkingLevel::High
        );
        assert!(loaded.providers[0].thinking_force);
        assert!(matches!(
            loaded.providers[0].auth,
            AuthConfig::Bearer { .. }
        ));
        assert_eq!(loaded.routing.len(), 1);
        assert_eq!(loaded.routing[0].provider, "test");
        assert_eq!(loaded.routing[0].fallback, vec!["fallback"]);
        assert_eq!(loaded.quota.len(), 1);
        assert_eq!(loaded.quota[0].max_requests, Some(100));
    }

    #[test]
    fn provider_two_urls_round_trip() {
        let repo = test_repo();
        let mut cfg = repo.load().unwrap();
        cfg.providers.push(ProviderConfig {
            thinking_level: crate::config::ThinkingLevel::Unset,
            thinking_force: false,
            name: "mm".into(),
            kind: ProviderKind::Minimax,
            enabled: true,
            auth: AuthConfig::Bearer { value: "k".into() },
            anthropic_base_url: Some("https://a/anthropic".into()),
            openai_base_url: Some("https://a/v1".into()),
            thinking_mode: ThinkingMode::SplitOnly,
            format_mode: crate::config::FormatMode::Both,
            max_concurrent: None,
            sanitize_empty_tools: false,
            model_formats: None,
        });
        repo.save(&cfg).unwrap();
        let loaded = repo.load().unwrap();
        let p = loaded.providers.iter().find(|p| p.name == "mm").unwrap();
        assert_eq!(p.anthropic_base_url.as_deref(), Some("https://a/anthropic"));
        assert_eq!(p.openai_base_url.as_deref(), Some("https://a/v1"));
    }

    #[test]
    fn opencode_go_kind_round_trips_through_save_and_load() {
        let repo = test_repo();
        let mut cfg = repo.load().unwrap();
        cfg.providers.push(ProviderConfig {
            thinking_level: crate::config::ThinkingLevel::Unset,
            thinking_force: false,
            name: "og".into(),
            kind: ProviderKind::OpencodeGo,
            enabled: true,
            auth: AuthConfig::Bearer { value: "k".into() },
            anthropic_base_url: Some("https://opencode.ai/zen/go".into()),
            openai_base_url: Some("https://opencode.ai/zen/go/v1".into()),
            thinking_mode: ThinkingMode::SplitOnly,
            format_mode: crate::config::FormatMode::Both,
            max_concurrent: None,
            sanitize_empty_tools: false,
            model_formats: None,
        });
        repo.save(&cfg).unwrap();
        let loaded = repo.load().unwrap();
        let p = loaded.providers.iter().find(|p| p.name == "og").unwrap();
        assert_eq!(p.kind, ProviderKind::OpencodeGo);
    }

    #[test]
    fn parse_kind_accepts_opencode_go_aliases() {
        assert_eq!(parse_kind("opencode_go"), ProviderKind::OpencodeGo);
        assert_eq!(parse_kind("opencode-go"), ProviderKind::OpencodeGo);
        assert_eq!(kind_to_str(ProviderKind::OpencodeGo), "opencode_go");
    }

    #[test]
    fn empty_urls_normalize_to_none_on_load() {
        let repo = test_repo();
        let mut cfg = repo.load().unwrap();
        cfg.providers.push(ProviderConfig {
            thinking_level: crate::config::ThinkingLevel::Unset,
            thinking_force: false,
            name: "empty-urls".into(),
            kind: ProviderKind::Anthropic,
            enabled: true,
            auth: AuthConfig::Passthrough,
            anthropic_base_url: Some(String::new()),
            openai_base_url: Some(String::new()),
            thinking_mode: ThinkingMode::SplitOnly,
            format_mode: crate::config::FormatMode::Both,
            max_concurrent: None,
            sanitize_empty_tools: false,
            model_formats: None,
        });
        repo.save(&cfg).unwrap();
        let loaded = repo.load().unwrap();
        let p = loaded
            .providers
            .iter()
            .find(|p| p.name == "empty-urls")
            .unwrap();
        assert_eq!(p.anthropic_base_url, None);
        assert_eq!(p.openai_base_url, None);
    }

    #[test]
    fn save_overwrites_previous() {
        let repo = test_repo();
        let mut cfg = repo.load().unwrap();
        cfg.port = 1111;
        repo.save(&cfg).unwrap();

        cfg.port = 2222;
        repo.save(&cfg).unwrap();

        let loaded = repo.load().unwrap();
        assert_eq!(loaded.port, 2222);
    }

    #[test]
    fn all_auth_types_round_trip() {
        let repo = test_repo();
        let auth_types: Vec<AuthConfig> = vec![
            AuthConfig::Passthrough,
            AuthConfig::ApiKey {
                value: "sk-test".into(),
            },
            AuthConfig::Bearer {
                value: "bearer-test".into(),
            },
            AuthConfig::AnthropicOAuth {
                access_token: "at".into(),
                refresh_token: "rt".into(),
                expires_at_ms: 123,
            },
            AuthConfig::OpenAiOAuth {
                access_token: "oat".into(),
                refresh_token: "ort".into(),
                expires_at_ms: 456,
            },
            AuthConfig::CodexAuto,
        ];

        for (i, auth) in auth_types.iter().enumerate() {
            let mut cfg = repo.load().unwrap();
            cfg.providers.push(ProviderConfig {
                thinking_level: crate::config::ThinkingLevel::Unset,
                thinking_force: false,
                name: format!("p{i}"),
                kind: ProviderKind::Anthropic,
                auth: auth.clone(),
                anthropic_base_url: None,
                openai_base_url: None,
                thinking_mode: ThinkingMode::SplitOnly,
                format_mode: crate::config::FormatMode::Both,
                max_concurrent: None,
                sanitize_empty_tools: false,
                enabled: true,
                model_formats: None,
            });
            repo.save(&cfg).unwrap();
            let loaded = repo.load().unwrap();
            let loaded_auth = &loaded
                .providers
                .iter()
                .find(|p| p.name == format!("p{i}"))
                .unwrap()
                .auth;
            match auth {
                AuthConfig::Passthrough => assert!(matches!(loaded_auth, AuthConfig::Passthrough)),
                AuthConfig::ApiKey { value } => {
                    let AuthConfig::ApiKey { value: v } = loaded_auth else {
                        panic!("wrong type")
                    };
                    assert_eq!(v, value);
                }
                AuthConfig::Bearer { value } => {
                    let AuthConfig::Bearer { value: v } = loaded_auth else {
                        panic!("wrong type")
                    };
                    assert_eq!(v, value);
                }
                AuthConfig::AnthropicOAuth { access_token, .. } => {
                    let AuthConfig::AnthropicOAuth {
                        access_token: at, ..
                    } = loaded_auth
                    else {
                        panic!("wrong type")
                    };
                    assert_eq!(at, access_token);
                }
                AuthConfig::OpenAiOAuth { access_token, .. } => {
                    let AuthConfig::OpenAiOAuth {
                        access_token: at, ..
                    } = loaded_auth
                    else {
                        panic!("wrong type")
                    };
                    assert_eq!(at, access_token);
                }
                AuthConfig::CodexAuto => assert!(matches!(loaded_auth, AuthConfig::CodexAuto)),
            }
        }
    }

    #[test]
    fn sanitize_empty_tools_survives_save_and_load() {
        let repo = test_repo();
        let mut cfg = repo.load().unwrap();
        cfg.providers.push(ProviderConfig {
            thinking_level: crate::config::ThinkingLevel::Unset,
            thinking_force: false,
            name: "moonshot".into(),
            kind: ProviderKind::Kimi,
            auth: AuthConfig::Passthrough,
            anthropic_base_url: None,
            openai_base_url: None,
            thinking_mode: ThinkingMode::SplitOnly,
            format_mode: crate::config::FormatMode::Both,
            max_concurrent: None,
            sanitize_empty_tools: true,
            enabled: true,
            model_formats: None,
        });
        repo.save(&cfg).unwrap();
        let loaded = repo.load().unwrap();
        assert!(
            loaded
                .providers
                .iter()
                .any(|p| p.name == "moonshot" && p.sanitize_empty_tools)
        );
    }

    #[test]
    fn model_formats_round_trips_through_the_db() {
        let repo = test_repo();
        let mut cfg = repo.load().unwrap();
        cfg.providers.push(ProviderConfig {
            thinking_level: crate::config::ThinkingLevel::Unset,
            thinking_force: false,
            name: "og".into(),
            kind: ProviderKind::OpencodeGo,
            auth: AuthConfig::Passthrough,
            anthropic_base_url: None,
            openai_base_url: None,
            thinking_mode: ThinkingMode::SplitOnly,
            format_mode: crate::config::FormatMode::Both,
            max_concurrent: None,
            sanitize_empty_tools: false,
            enabled: true,
            model_formats: Some("qwen3.*=anthropic,grok-4.5=responses".into()),
        });
        repo.save(&cfg).unwrap();
        let loaded = repo.load().unwrap();
        assert_eq!(
            loaded.providers[0].model_formats.as_deref(),
            Some("qwen3.*=anthropic,grok-4.5=responses")
        );
    }

    #[test]
    fn missing_model_formats_column_value_loads_as_none() {
        let repo = test_repo();
        let mut cfg = repo.load().unwrap();
        cfg.providers.push(ProviderConfig {
            thinking_level: crate::config::ThinkingLevel::Unset,
            thinking_force: false,
            name: "og".into(),
            kind: ProviderKind::OpencodeGo,
            auth: AuthConfig::Passthrough,
            anthropic_base_url: None,
            openai_base_url: None,
            thinking_mode: ThinkingMode::SplitOnly,
            format_mode: crate::config::FormatMode::Both,
            max_concurrent: None,
            sanitize_empty_tools: false,
            enabled: true,
            model_formats: None,
        });
        repo.save(&cfg).unwrap();
        let loaded = repo.load().unwrap();
        assert!(loaded.providers[0].model_formats.is_none());
    }

    #[test]
    fn enabled_flag_survives_save_and_load() {
        let repo = test_repo();
        let mut cfg = repo.load().unwrap();
        cfg.providers.push(ProviderConfig {
            thinking_level: crate::config::ThinkingLevel::Unset,
            thinking_force: false,
            name: "off".into(),
            kind: ProviderKind::Anthropic,
            auth: AuthConfig::Passthrough,
            anthropic_base_url: None,
            openai_base_url: None,
            thinking_mode: ThinkingMode::SplitOnly,
            format_mode: crate::config::FormatMode::Both,
            max_concurrent: None,
            sanitize_empty_tools: false,
            enabled: false,
            model_formats: None,
        });
        repo.save(&cfg).unwrap();
        let loaded = repo.load().unwrap();
        assert!(
            loaded
                .providers
                .iter()
                .any(|p| p.name == "off" && !p.enabled)
        );
    }
}
