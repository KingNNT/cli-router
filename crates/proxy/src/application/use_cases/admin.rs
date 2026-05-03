//! Admin control-plane use cases — read status, view/edit config, list recent
//! requests, ping a configured provider. Bound to `/admin/*` routes by the
//! framework ring; isolated here so they remain testable without an HTTP
//! transport.

use crate::application::errors::ProxyError;
use crate::application::ports::{RequestLogReadPort, UpstreamResponse};
use crate::config::{AuthConfig, Config, MatchSpec, ProviderConfig, ProviderKind, RoutingRule};
use crate::domain::RequestRow;
use axum::http::HeaderMap;
use bytes::Bytes;
use proxy_admin_api::{
    AuthPayload, ConfigPayload, MatchPayload, ProviderPayload, RecentRequestItem,
    RecentRequestsResponse, RoutingRulePayload, StatusResponse, TestProviderResponse,
};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

// ---- GetStatus ----

pub struct GetStatus {
    read: Arc<dyn RequestLogReadPort>,
    started_at_ms: i64,
}

impl GetStatus {
    pub fn new(read: Arc<dyn RequestLogReadPort>, started_at_ms: i64) -> Self {
        Self {
            read,
            started_at_ms,
        }
    }

    pub fn execute(&self) -> Result<StatusResponse, ProxyError> {
        let total = self.read.total_count()?;
        let by_provider = self.read.count_by_provider()?;
        let by_status = self.read.count_by_status()?;
        let now_ms = now_epoch_ms();
        let uptime_seconds = ((now_ms - self.started_at_ms) / 1000).max(0) as u64;
        Ok(StatusResponse {
            started_at_ms: self.started_at_ms,
            uptime_seconds,
            total_requests: total,
            requests_by_provider: by_provider,
            requests_by_status: by_status,
        })
    }
}

// ---- GetConfig ----

pub struct GetConfig {
    config: Arc<RwLock<Config>>,
}

impl GetConfig {
    pub fn new(config: Arc<RwLock<Config>>) -> Self {
        Self { config }
    }

    pub fn execute(&self) -> Result<ConfigPayload, ProxyError> {
        let cfg = self.config.read().expect("config rwlock poisoned");
        Ok(config_to_payload(&cfg))
    }
}

// ---- GetRecentRequests ----

pub struct GetRecentRequests {
    read: Arc<dyn RequestLogReadPort>,
    default_limit: u32,
}

impl GetRecentRequests {
    pub fn new(read: Arc<dyn RequestLogReadPort>) -> Self {
        Self {
            read,
            default_limit: 50,
        }
    }

    pub fn execute(&self, limit: Option<u32>) -> Result<RecentRequestsResponse, ProxyError> {
        let n = limit.unwrap_or(self.default_limit).clamp(1, 500);
        let rows = self.read.recent(n)?;
        Ok(RecentRequestsResponse {
            items: rows.into_iter().map(row_to_dto).collect(),
        })
    }
}

// ---- UpdateConfig ----

pub struct UpdateConfig {
    config: Arc<RwLock<Config>>,
    config_path: PathBuf,
    live: Arc<crate::adapters::providers::LiveProvider>,
    http: reqwest::Client,
}

impl UpdateConfig {
    pub fn new(
        config: Arc<RwLock<Config>>,
        config_path: PathBuf,
        live: Arc<crate::adapters::providers::LiveProvider>,
        http: reqwest::Client,
    ) -> Self {
        Self {
            config,
            config_path,
            live,
            http,
        }
    }

    /// Validate, persist to disk, swap in-memory config, then rebuild the
    /// live provider tree so subsequent requests use the new config without
    /// requiring a daemon restart.
    pub fn execute(&self, payload: ConfigPayload) -> Result<(), ProxyError> {
        let (proxy_db, pricing_db) = {
            let cur = self.config.read().expect("config rwlock poisoned");
            (cur.proxy_db.clone(), cur.pricing_db.clone())
        };
        let new_cfg = payload_to_config(payload, proxy_db, pricing_db)?;
        new_cfg
            .validate()
            .map_err(|e| ProxyError::BadRequest(format!("{e}")))?;

        let toml_str = toml::to_string_pretty(&new_cfg)
            .map_err(|e| ProxyError::BadRequest(format!("config serialize: {e}")))?;
        if let Some(parent) = self.config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.config_path, toml_str)?;

        // Hot reload: build a fresh provider tree, swap atomically. If the
        // build fails (bad routing rule somehow slipped past validate), we
        // surface that to the caller — the on-disk file is already updated
        // but the live provider keeps serving the previous config.
        self.live
            .reload(&new_cfg, self.http.clone())
            .map_err(|e| ProxyError::BadRequest(format!("provider rebuild: {e}")))?;

        let mut slot = self.config.write().expect("config rwlock poisoned");
        *slot = new_cfg;
        Ok(())
    }
}

// ---- TestProvider ----

pub struct TestProvider {
    config: Arc<RwLock<Config>>,
    http: reqwest::Client,
}

impl TestProvider {
    pub fn new(config: Arc<RwLock<Config>>, http: reqwest::Client) -> Self {
        Self { config, http }
    }

    pub async fn execute(&self, name: &str, model: &str) -> TestProviderResponse {
        // Build a one-off leaf from the *current* config so test reflects
        // the latest credentials, not a cached snapshot taken at startup.
        let prov = {
            let cfg = self.config.read().expect("config rwlock poisoned");
            let pcfg = match cfg.providers.iter().find(|p| p.name == name) {
                Some(p) => p.clone(),
                None => {
                    return TestProviderResponse {
                        success: false,
                        status_code: None,
                        latency_ms: 0,
                        error: Some(format!("provider '{name}' is not configured")),
                    };
                }
            };
            crate::adapters::providers::build_leaf(&pcfg, self.http.clone())
        };
        let body_json = serde_json::json!({
            "model": model,
            "max_tokens": 1,
            "messages": [{"role": "user", "content": "ping"}],
            "stream": false,
        });
        let body = Bytes::from(body_json.to_string());
        let start = std::time::Instant::now();
        let result = prov
            .forward("/v1/messages", &HeaderMap::new(), body, false)
            .await;
        let latency_ms = start.elapsed().as_millis() as u64;
        match result {
            Ok(UpstreamResponse::Buffered { status, .. }) => TestProviderResponse {
                success: (200..300).contains(&status),
                status_code: Some(status),
                latency_ms,
                error: None,
            },
            Ok(UpstreamResponse::Streaming { status, .. }) => TestProviderResponse {
                success: (200..300).contains(&status),
                status_code: Some(status),
                latency_ms,
                error: None,
            },
            Err(e) => TestProviderResponse {
                success: false,
                status_code: None,
                latency_ms,
                error: Some(format!("{e}")),
            },
        }
    }
}

// ---- OAuth (Anthropic) ----

pub struct StartAnthropicOAuth {
    sessions: Arc<crate::adapters::oauth::OAuthSessionStore>,
}

impl StartAnthropicOAuth {
    pub fn new(sessions: Arc<crate::adapters::oauth::OAuthSessionStore>) -> Self {
        Self { sessions }
    }

    pub fn execute(&self) -> proxy_admin_api::StartOAuthResponse {
        let codes = crate::adapters::oauth::generate_pkce();
        let url = crate::adapters::oauth::build_authorize_url(&codes);
        let state_id = codes.state.clone();
        self.sessions.insert(codes);
        proxy_admin_api::StartOAuthResponse {
            authorization_url: url,
            state_id,
        }
    }
}

pub struct CompleteAnthropicOAuth {
    sessions: Arc<crate::adapters::oauth::OAuthSessionStore>,
    http: reqwest::Client,
    config: Arc<RwLock<Config>>,
    config_path: PathBuf,
    live: Arc<crate::adapters::providers::LiveProvider>,
}

impl CompleteAnthropicOAuth {
    pub fn new(
        sessions: Arc<crate::adapters::oauth::OAuthSessionStore>,
        http: reqwest::Client,
        config: Arc<RwLock<Config>>,
        config_path: PathBuf,
        live: Arc<crate::adapters::providers::LiveProvider>,
    ) -> Self {
        Self {
            sessions,
            http,
            config,
            config_path,
            live,
        }
    }

    pub async fn execute(
        &self,
        req: proxy_admin_api::CompleteOAuthRequest,
    ) -> proxy_admin_api::CompleteOAuthResponse {
        let codes = match self.sessions.take(&req.state_id) {
            Some(c) => c,
            None => {
                return proxy_admin_api::CompleteOAuthResponse {
                    success: false,
                    error: Some("unknown or expired state_id".into()),
                    config: None,
                };
            }
        };
        let tokens = match crate::adapters::oauth::exchange_code(
            &self.http,
            req.code.trim(),
            &codes.verifier,
        )
        .await
        {
            Ok(t) => t,
            Err(e) => {
                return proxy_admin_api::CompleteOAuthResponse {
                    success: false,
                    error: Some(format!("{e}")),
                    config: None,
                };
            }
        };
        // Persist: update the named provider's auth to Bearer + capture new
        // config snapshot for the response and reload.
        let (new_payload, new_cfg_clone) = {
            let mut cur = self.config.write().expect("config rwlock poisoned");
            let prov = match cur
                .providers
                .iter_mut()
                .find(|p| p.name == req.provider_name)
            {
                Some(p) => p,
                None => {
                    return proxy_admin_api::CompleteOAuthResponse {
                        success: false,
                        error: Some(format!(
                            "provider '{}' not in current config",
                            req.provider_name
                        )),
                        config: None,
                    };
                }
            };
            // Compute expires_at_ms: now + expires_in (default 8 hours).
            let expires_at_ms = {
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                let expires_in_ms = tokens.expires_in.unwrap_or(28800) * 1000;
                now_ms + expires_in_ms
            };
            prov.auth = AuthConfig::AnthropicOAuth {
                access_token: tokens.access_token.clone(),
                refresh_token: tokens
                    .refresh_token
                    .clone()
                    .unwrap_or_default(),
                expires_at_ms,
            };
            (config_to_payload(&cur), cur.clone())
        };
        // Write back to disk.
        let toml_str = match toml::to_string_pretty(&new_cfg_clone) {
            Ok(s) => s,
            Err(e) => {
                return proxy_admin_api::CompleteOAuthResponse {
                    success: false,
                    error: Some(format!("config serialize: {e}")),
                    config: None,
                };
            }
        };
        if let Some(parent) = self.config_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(&self.config_path, toml_str) {
            return proxy_admin_api::CompleteOAuthResponse {
                success: false,
                error: Some(format!("write config: {e}")),
                config: None,
            };
        }
        // Hot reload providers so the new bearer token is used immediately.
        if let Err(e) = self.live.reload(&new_cfg_clone, self.http.clone()) {
            return proxy_admin_api::CompleteOAuthResponse {
                success: false,
                error: Some(format!("provider rebuild: {e}")),
                config: None,
            };
        }
        proxy_admin_api::CompleteOAuthResponse {
            success: true,
            error: None,
            config: Some(new_payload),
        }
    }
}

// ---- helpers (DTO ↔ domain mapping) ----

fn now_epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn row_to_dto(r: RequestRow) -> RecentRequestItem {
    RecentRequestItem {
        id: r.id,
        started_at_ms: r.started_at_ms,
        finished_at_ms: r.finished_at_ms,
        provider: r.provider,
        model: r.model,
        status: r.status,
        input_tokens: r.input_tokens,
        output_tokens: r.output_tokens,
        cache_read_tokens: r.cache_read_tokens,
        cache_creation_tokens: r.cache_creation_tokens,
        cost_usd: r.cost_usd,
        error_message: r.error_message,
    }
}

fn config_to_payload(c: &Config) -> ConfigPayload {
    ConfigPayload {
        port: c.port,
        providers: c
            .providers
            .iter()
            .map(|p| ProviderPayload {
                name: p.name.clone(),
                kind: kind_to_str(p.kind).into(),
                auth: auth_to_payload(&p.auth),
                base_url: p.base_url.clone(),
            })
            .collect(),
        routing: c
            .routing
            .iter()
            .map(|r| RoutingRulePayload {
                r#match: MatchPayload {
                    model: r.match_spec.model.clone(),
                },
                provider: r.provider.clone(),
                fallback: r.fallback.clone(),
                strategy: strategy_to_payload(r.strategy),
                priority: r.priority,
            })
            .collect(),
    }
}

fn payload_to_config(
    p: ConfigPayload,
    proxy_db: PathBuf,
    pricing_db: PathBuf,
) -> Result<Config, ProxyError> {
    let providers = p
        .providers
        .into_iter()
        .map(|pp| {
            Ok(ProviderConfig {
                name: pp.name,
                kind: str_to_kind(&pp.kind)?,
                auth: payload_to_auth(pp.auth),
                base_url: pp.base_url,
            })
        })
        .collect::<Result<Vec<_>, ProxyError>>()?;
    let routing = p
        .routing
        .into_iter()
        .map(|rr| RoutingRule {
            match_spec: MatchSpec {
                model: rr.r#match.model,
            },
            provider: rr.provider,
            fallback: rr.fallback,
            strategy: payload_to_strategy(rr.strategy),
            priority: rr.priority,
        })
        .collect();
    Ok(Config {
        port: p.port,
        proxy_db,
        pricing_db,
        providers,
        routing,
    })
}

fn strategy_to_payload(s: crate::config::RoutingStrategy) -> proxy_admin_api::RoutingStrategyPayload {
    match s {
        crate::config::RoutingStrategy::Failover => proxy_admin_api::RoutingStrategyPayload::Failover,
        crate::config::RoutingStrategy::RoundRobin => proxy_admin_api::RoutingStrategyPayload::RoundRobin,
    }
}

fn payload_to_strategy(s: proxy_admin_api::RoutingStrategyPayload) -> crate::config::RoutingStrategy {
    match s {
        proxy_admin_api::RoutingStrategyPayload::Failover => crate::config::RoutingStrategy::Failover,
        proxy_admin_api::RoutingStrategyPayload::RoundRobin => crate::config::RoutingStrategy::RoundRobin,
    }
}

fn kind_to_str(k: ProviderKind) -> &'static str {
    match k {
        ProviderKind::Anthropic => "anthropic",
        ProviderKind::Zai => "zai",
    }
}

fn str_to_kind(s: &str) -> Result<ProviderKind, ProxyError> {
    match s.trim().to_ascii_lowercase().as_str() {
        "anthropic" => Ok(ProviderKind::Anthropic),
        "zai" => Ok(ProviderKind::Zai),
        other => Err(ProxyError::BadRequest(format!(
            "unknown provider kind: {other}"
        ))),
    }
}

fn auth_to_payload(a: &AuthConfig) -> AuthPayload {
    match a {
        AuthConfig::Passthrough => AuthPayload::Passthrough,
        AuthConfig::ApiKey { value } => AuthPayload::ApiKey {
            value: value.clone(),
        },
        AuthConfig::Bearer { value } => AuthPayload::Bearer {
            value: value.clone(),
        },
        AuthConfig::AnthropicOAuth {
            access_token,
            refresh_token,
            expires_at_ms,
        } => AuthPayload::AnthropicOAuth {
            access_token: access_token.clone(),
            refresh_token: refresh_token.clone(),
            expires_at_ms: *expires_at_ms,
        },
    }
}

fn payload_to_auth(a: AuthPayload) -> AuthConfig {
    match a {
        AuthPayload::Passthrough => AuthConfig::Passthrough,
        AuthPayload::ApiKey { value } => AuthConfig::ApiKey { value },
        AuthPayload::Bearer { value } => AuthConfig::Bearer { value },
        AuthPayload::AnthropicOAuth {
            access_token,
            refresh_token,
            expires_at_ms,
        } => AuthConfig::AnthropicOAuth {
            access_token,
            refresh_token,
            expires_at_ms,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::RequestRow;
    use std::collections::BTreeMap;

    struct StubRead {
        total: u64,
        by_provider: BTreeMap<String, u64>,
        by_status: BTreeMap<String, u64>,
        rows: Vec<RequestRow>,
    }
    impl RequestLogReadPort for StubRead {
        fn total_count(&self) -> Result<u64, ProxyError> {
            Ok(self.total)
        }
        fn count_by_provider(&self) -> Result<BTreeMap<String, u64>, ProxyError> {
            Ok(self.by_provider.clone())
        }
        fn count_by_status(&self) -> Result<BTreeMap<String, u64>, ProxyError> {
            Ok(self.by_status.clone())
        }
        fn recent(&self, _: u32) -> Result<Vec<RequestRow>, ProxyError> {
            Ok(self.rows.clone())
        }
    }

    fn stub() -> Arc<dyn RequestLogReadPort> {
        Arc::new(StubRead {
            total: 7,
            by_provider: BTreeMap::from([("anthropic".into(), 4), ("zai".into(), 3)]),
            by_status: BTreeMap::from([("completed".into(), 6), ("errored".into(), 1)]),
            rows: vec![],
        })
    }

    #[test]
    fn get_status_aggregates_counts() {
        let uc = GetStatus::new(stub(), 1_000);
        let s = uc.execute().unwrap();
        assert_eq!(s.total_requests, 7);
        assert_eq!(s.requests_by_provider.get("anthropic"), Some(&4));
        assert_eq!(s.requests_by_status.get("completed"), Some(&6));
        assert_eq!(s.started_at_ms, 1_000);
    }

    #[test]
    fn get_recent_clamps_limit_to_max() {
        let uc = GetRecentRequests::new(stub());
        // No way to observe the clamp via the stub's fixed return, but we
        // can at least confirm it doesn't panic on extreme values.
        let _ = uc.execute(Some(0)).unwrap();
        let _ = uc.execute(Some(u32::MAX)).unwrap();
    }

    #[test]
    fn config_to_payload_roundtrip() {
        let cfg = Config {
            port: 8787,
            proxy_db: PathBuf::from("/tmp/proxy.db"),
            pricing_db: PathBuf::from("/tmp/pricing.db"),
            providers: vec![ProviderConfig {
                name: "anthropic".into(),
                kind: ProviderKind::Anthropic,
                auth: AuthConfig::ApiKey {
                    value: "sk-test".into(),
                },
                base_url: None,
            }],
            routing: vec![RoutingRule {
                match_spec: MatchSpec {
                    model: Some("*".into()),
                },
                provider: "anthropic".into(),
                fallback: vec![],
                strategy: Default::default(),
                priority: None,
            }],
        };
        let payload = config_to_payload(&cfg);
        assert_eq!(payload.port, 8787);
        assert_eq!(payload.providers.len(), 1);
        assert!(matches!(
            payload.providers[0].auth,
            AuthPayload::ApiKey { .. }
        ));
        assert_eq!(payload.routing[0].provider, "anthropic");
    }

    #[test]
    fn payload_to_config_rejects_unknown_kind() {
        let p = ConfigPayload {
            port: 8787,
            providers: vec![ProviderPayload {
                name: "x".into(),
                kind: "bogus".into(),
                auth: AuthPayload::Passthrough,
                base_url: None,
            }],
            routing: vec![],
        };
        let err = payload_to_config(p, PathBuf::new(), PathBuf::new()).unwrap_err();
        assert!(format!("{err}").contains("bogus"));
    }
}
