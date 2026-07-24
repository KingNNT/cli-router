//! Admin control-plane use cases — read status, view/edit config, list recent
//! requests, ping a configured provider. Bound to `/admin/*` routes by the
//! framework ring; isolated here so they remain testable without an HTTP
//! transport.

use crate::application::errors::ProxyError;
use crate::application::ports::{
    ConfigRepository, QuotaPort, RequestLogReadPort, UpstreamResponse,
};
use crate::config::{
    AffinityConfig, AuthConfig, Config, MatchSpec, ProviderConfig, ProviderKind, QuotaRule,
    RoutingRule,
};
use crate::domain::RequestRow;
use axum::http::HeaderMap;
use bytes::Bytes;
use proxy_admin_api::{
    AffinityPayload, AuthPayload, ConfigPayload, MatchPayload, ModelBreakdownItemDto,
    ProviderPayload, QuotaPayload, RecentRequestItem, RecentRequestsResponse, RoutingRulePayload,
    StatusResponse, TestProviderResponse,
};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

// ---- GetStatus ----

pub struct GetStatus {
    read: Arc<dyn RequestLogReadPort>,
    started_at_ms: i64,
    config: Arc<RwLock<Config>>,
}

impl GetStatus {
    pub fn new(
        read: Arc<dyn RequestLogReadPort>,
        started_at_ms: i64,
        config: Arc<RwLock<Config>>,
    ) -> Self {
        Self {
            read,
            started_at_ms,
            config,
        }
    }

    pub fn execute(&self) -> Result<StatusResponse, ProxyError> {
        let total = self.read.total_count()?;
        let by_provider = self.read.count_by_provider()?;
        let by_status = self.read.count_by_status()?;
        let translations = self.read.count_translations()?;
        let now_ms = now_epoch_ms();
        let uptime_seconds = ((now_ms - self.started_at_ms) / 1000).max(0) as u64;
        let cfg = self.config.read().expect("config rwlock poisoned");
        Ok(StatusResponse {
            started_at_ms: self.started_at_ms,
            uptime_seconds,
            total_requests: total,
            requests_by_provider: by_provider,
            requests_by_status: by_status,
            affinity: proxy_admin_api::AffinityStatus {
                enabled: cfg.affinity.enabled,
                headers: cfg.affinity.headers.clone(),
            },
            translations_completed: translations.completed,
            translations_failed: translations.failed,
            translation_directions: translations.by_direction,
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

    pub fn execute(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<RecentRequestsResponse, ProxyError> {
        let n = limit.unwrap_or(self.default_limit).clamp(1, 500);
        let o = offset.unwrap_or(0);
        let rows = self.read.recent(n, o)?;
        let total_count = self.read.total_count()?;
        Ok(RecentRequestsResponse {
            items: rows.into_iter().map(row_to_dto).collect(),
            total_count,
        })
    }
}

// ---- GetUsageSummary ----

pub struct GetUsageSummary {
    read: Arc<dyn RequestLogReadPort>,
}

impl GetUsageSummary {
    pub fn new(read: Arc<dyn RequestLogReadPort>) -> Self {
        Self { read }
    }

    pub fn execute(
        &self,
        from_ms: i64,
        to_ms: i64,
    ) -> Result<proxy_admin_api::UsageSummaryResponse, ProxyError> {
        if from_ms > to_ms {
            return Err(ProxyError::BadRequest("from must be <= to".into()));
        }
        let s = self.read.summarize(from_ms, to_ms)?;
        Ok(proxy_admin_api::UsageSummaryResponse {
            from_ms: s.from_ms,
            to_ms: s.to_ms,
            daily: s
                .daily
                .into_iter()
                .map(|d| proxy_admin_api::DailyUsageRow {
                    date: d.date,
                    requests: d.requests,
                    input_tokens: d.input_tokens,
                    output_tokens: d.output_tokens,
                    cache_read_tokens: d.cache_read_tokens,
                    cache_creation_tokens: d.cache_creation_tokens,
                    cost_usd: d.cost_usd,
                })
                .collect(),
            models: s
                .models
                .into_iter()
                .map(|m| proxy_admin_api::ModelUsageRow {
                    model: m.model,
                    provider: m.provider,
                    requests: m.requests,
                    input_tokens: m.input_tokens,
                    output_tokens: m.output_tokens,
                    cache_read_tokens: m.cache_read_tokens,
                    cache_creation_tokens: m.cache_creation_tokens,
                    cost_usd: m.cost_usd,
                })
                .collect(),
        })
    }
}

// ---- UpdateConfig ----

pub struct UpdateConfig {
    config: Arc<RwLock<Config>>,
    config_repo: Arc<dyn ConfigRepository>,
    live: Arc<crate::adapters::providers::LiveProvider>,
    http: reqwest::Client,
}

impl UpdateConfig {
    pub fn new(
        config: Arc<RwLock<Config>>,
        config_repo: Arc<dyn ConfigRepository>,
        live: Arc<crate::adapters::providers::LiveProvider>,
        http: reqwest::Client,
    ) -> Self {
        Self {
            config,
            config_repo,
            live,
            http,
        }
    }

    /// Validate, persist to the config repository, swap in-memory config,
    /// then rebuild the live provider tree so subsequent requests use the new
    /// config without requiring a daemon restart.
    pub fn execute(&self, payload: ConfigPayload) -> Result<(), ProxyError> {
        let existing = {
            let cur = self.config.read().expect("config rwlock poisoned");
            cur.clone()
        };
        let proxy_db = payload
            .proxy_db
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| existing.proxy_db.clone());
        let pricing_db = payload
            .pricing_db
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| existing.pricing_db.clone());
        let new_cfg = payload_to_config(payload, proxy_db, pricing_db, &existing)?;
        new_cfg
            .validate()
            .map_err(|e| ProxyError::BadRequest(format!("{e}")))?;

        self.config_repo
            .save(&new_cfg)
            .map_err(|e| ProxyError::BadRequest(format!("{e}")))?;

        // Hot reload: build a fresh provider tree, swap atomically. If the
        // build fails (bad routing rule somehow slipped past validate), we
        // surface that to the caller — the persistent config is already updated
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
            match crate::adapters::providers::build_leaf(&pcfg, self.http.clone()) {
                Ok(p) => p,
                Err(e) => {
                    return TestProviderResponse {
                        success: false,
                        status_code: None,
                        latency_ms: 0,
                        error: Some(format!("{e}")),
                    };
                }
            }
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
    config_repo: Arc<dyn ConfigRepository>,
    live: Arc<crate::adapters::providers::LiveProvider>,
}

// ---- OAuth (OpenAI / Codex) ----

pub struct StartOpenAiOAuth {
    #[allow(dead_code)]
    sessions: Arc<crate::adapters::oauth::openai::OAuthSessionStore>,
}

impl StartOpenAiOAuth {
    pub fn new(sessions: Arc<crate::adapters::oauth::openai::OAuthSessionStore>) -> Self {
        Self { sessions }
    }

    pub fn execute(&self) -> proxy_admin_api::StartOAuthResponse {
        // For OpenAI Codex, tokens come from `~/.codex/auth.json` (written
        // by `codex login`). The user doesn't need a browser redirect —
        // they just need to run `codex login` once, then press Enter in
        // the TUI to read the cached tokens.
        let msg = match crate::adapters::oauth::openai::read_auth_json() {
            Ok(_) => "Found ~/.codex/auth.json — press Enter to use cached Codex token.",
            Err(_) => "No ~/.codex/auth.json found. Run `codex login` in your terminal first.",
        };
        proxy_admin_api::StartOAuthResponse {
            authorization_url: msg.to_string(),
            state_id: String::new(),
        }
    }
}

pub struct CompleteOpenAiOAuth {
    #[allow(dead_code)]
    sessions: Arc<crate::adapters::oauth::openai::OAuthSessionStore>,
    http: reqwest::Client,
    config: Arc<RwLock<Config>>,
    config_repo: Arc<dyn ConfigRepository>,
    live: Arc<crate::adapters::providers::LiveProvider>,
}

impl CompleteOpenAiOAuth {
    pub fn new(
        sessions: Arc<crate::adapters::oauth::openai::OAuthSessionStore>,
        http: reqwest::Client,
        config: Arc<RwLock<Config>>,
        config_repo: Arc<dyn ConfigRepository>,
        live: Arc<crate::adapters::providers::LiveProvider>,
    ) -> Self {
        Self {
            sessions,
            http,
            config,
            config_repo,
            live,
        }
    }

    pub async fn execute(
        &self,
        req: proxy_admin_api::CompleteOAuthRequest,
    ) -> proxy_admin_api::CompleteOAuthResponse {
        // Read tokens directly from `~/.codex/auth.json` (cached by `codex login`).
        let tokens = match crate::adapters::oauth::openai::read_auth_json() {
            Ok(t) => t,
            Err(e) => {
                return proxy_admin_api::CompleteOAuthResponse {
                    success: false,
                    error: Some(format!("{e}")),
                    config: None,
                };
            }
        };

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
            let expires_at_ms = {
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                let expires_in_ms = tokens.expires_in.unwrap_or(3600) * 1000;
                now_ms + expires_in_ms
            };
            prov.auth = AuthConfig::OpenAiOAuth {
                access_token: tokens.access_token.clone(),
                refresh_token: tokens.refresh_token.clone().unwrap_or_default(),
                expires_at_ms,
            };
            (config_to_payload(&cur), cur.clone())
        };

        // Write back to DB.
        if let Err(e) = self.config_repo.save(&new_cfg_clone) {
            return proxy_admin_api::CompleteOAuthResponse {
                success: false,
                error: Some(format!("save config: {e}")),
                config: None,
            };
        }
        // Hot reload providers so the new token is used immediately.
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

impl CompleteAnthropicOAuth {
    pub fn new(
        sessions: Arc<crate::adapters::oauth::OAuthSessionStore>,
        http: reqwest::Client,
        config: Arc<RwLock<Config>>,
        config_repo: Arc<dyn ConfigRepository>,
        live: Arc<crate::adapters::providers::LiveProvider>,
    ) -> Self {
        Self {
            sessions,
            http,
            config,
            config_repo,
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
        // Tolerate users pasting the full `code#state` string Anthropic
        // displays — strip everything from `#` onward. The locally generated
        // `state` is what we actually send to the token endpoint, since it
        // matches what Anthropic echoes back.
        let trimmed = req.code.trim();
        let code = trimmed.split('#').next().unwrap_or(trimmed);
        let tokens = match crate::adapters::oauth::exchange_code(
            &self.http,
            code,
            &codes.state,
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
                refresh_token: tokens.refresh_token.clone().unwrap_or_default(),
                expires_at_ms,
            };
            (config_to_payload(&cur), cur.clone())
        };
        // Write back to DB.
        if let Err(e) = self.config_repo.save(&new_cfg_clone) {
            return proxy_admin_api::CompleteOAuthResponse {
                success: false,
                error: Some(format!("save config: {e}")),
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

// ---- GetQuotaStatus ----

pub struct GetQuotaStatus {
    quota: Arc<dyn QuotaPort>,
}

impl GetQuotaStatus {
    pub fn new(quota: Arc<dyn QuotaPort>) -> Self {
        Self { quota }
    }

    pub fn execute(&self) -> proxy_admin_api::QuotaStatusListDto {
        let snapshots = self.quota.snapshot();
        proxy_admin_api::QuotaStatusListDto {
            quotas: snapshots.into_iter().map(snapshot_to_dto).collect(),
        }
    }
}

fn snapshot_to_dto(s: crate::domain::quota::QuotaSnapshot) -> proxy_admin_api::QuotaStatusDto {
    let window_str = match s.config.window {
        crate::domain::quota::QuotaWindow::Rolling { duration_ms } => {
            let secs = duration_ms / 1000;
            if secs > 0 && secs % (24 * 3600) == 0 {
                format!("rolling:{}d", secs / 86400)
            } else if secs > 0 && secs % 3600 == 0 {
                format!("rolling:{}h", secs / 3600)
            } else if secs > 0 && secs % 60 == 0 {
                format!("rolling:{}m", secs / 60)
            } else {
                format!("rolling:{secs}s")
            }
        }
        crate::domain::quota::QuotaWindow::Calendar { unit } => match unit {
            crate::domain::quota::CalendarUnit::Minute => "calendar:minute".into(),
            crate::domain::quota::CalendarUnit::Hour => "calendar:hour".into(),
            crate::domain::quota::CalendarUnit::Day => "calendar:day".into(),
        },
    };

    let warn_pct = s.config.warn_pct;
    let mk = |used: u64, max: Option<u64>| {
        let pct = if let Some(m) = max {
            if m > 0 {
                ((used as u128 * 100) / m as u128).min(100) as u8
            } else {
                0
            }
        } else {
            0
        };
        let state = match max {
            None => proxy_admin_api::QuotaMetricState::Unconfigured,
            Some(m) if used >= m => proxy_admin_api::QuotaMetricState::Rejecting,
            Some(_) if pct >= warn_pct => proxy_admin_api::QuotaMetricState::Warn,
            _ => proxy_admin_api::QuotaMetricState::Ok,
        };
        proxy_admin_api::QuotaMetricDto {
            used,
            max,
            pct,
            state,
        }
    };

    proxy_admin_api::QuotaStatusDto {
        provider: s.config.provider.clone(),
        window: window_str,
        window_resets_in_ms: s.next_boundary_ms,
        requests: mk(s.totals.requests, s.config.max_requests),
        input_tokens: mk(s.totals.input_tokens, s.config.max_input_tokens),
        output_tokens: mk(s.totals.output_tokens, s.config.max_output_tokens),
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
        translation_direction: r.translation_direction,
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
                enabled: p.enabled,
                auth: auth_to_payload(&p.auth),
                anthropic_base_url: p.anthropic_base_url.clone(),
                openai_base_url: p.openai_base_url.clone(),
                thinking_level: Some(p.thinking_level.as_str().to_string()),
                thinking_force: Some(p.thinking_force),
                thinking_mode: Some(match p.thinking_mode {
                    crate::config::ThinkingMode::SplitOnly => "split_only".to_string(),
                    crate::config::ThinkingMode::StripAll => "strip_all".to_string(),
                }),
                format_mode: Some(match p.format_mode {
                    crate::config::FormatMode::Both => "both".to_string(),
                    crate::config::FormatMode::Anthropic => "anthropic".to_string(),
                    crate::config::FormatMode::OpenAi => "openai".to_string(),
                }),
                max_concurrent: p.max_concurrent,
                sanitize_empty_tools: Some(p.sanitize_empty_tools),
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
        quota: c
            .quota
            .iter()
            .map(|q| QuotaPayload {
                provider: q.provider.clone(),
                window: q.window.clone(),
                max_requests: q.max_requests,
                max_input_tokens: q.max_input_tokens,
                max_output_tokens: q.max_output_tokens,
                warn_pct: q.warn_pct,
            })
            .collect(),
        affinity: AffinityPayload {
            enabled: c.affinity.enabled,
            headers: c.affinity.headers.clone(),
        },
        proxy_db: c.proxy_db.to_str().map(|s| s.to_owned()),
        pricing_db: c.pricing_db.to_str().map(|s| s.to_owned()),
    }
}

fn payload_to_config(
    p: ConfigPayload,
    proxy_db: PathBuf,
    pricing_db: PathBuf,
    _existing: &Config,
) -> Result<Config, ProxyError> {
    let providers = p
        .providers
        .into_iter()
        .map(|pp| {
            let kind = str_to_kind(&pp.kind)?;
            let thinking_level = match pp.thinking_level.as_deref().map(str::trim) {
                None | Some("") => crate::config::ThinkingLevel::Unset,
                Some(raw) => {
                    let parsed = crate::config::ThinkingLevel::parse(raw);
                    let offered = crate::adapters::providers::thinking::thinking_levels(kind);
                    match parsed {
                        Some(crate::config::ThinkingLevel::Unset) => {
                            crate::config::ThinkingLevel::Unset
                        }
                        Some(level) if offered.contains(&level) => level,
                        _ => {
                            let valid = offered
                                .iter()
                                .map(|l| l.as_str())
                                .collect::<Vec<_>>()
                                .join(", ");
                            return Err(ProxyError::BadRequest(format!(
                                "invalid thinking_level '{raw}' for provider '{}' (valid for kind '{}': unset, {valid})",
                                pp.name,
                                kind_to_str(kind),
                            )));
                        }
                    }
                }
            };
            let thinking_mode = match pp.thinking_mode.as_deref().map(str::trim) {
                None | Some("") | Some("split_only") => crate::config::ThinkingMode::SplitOnly,
                Some("strip_all") => crate::config::ThinkingMode::StripAll,
                Some(other) => {
                    return Err(ProxyError::BadRequest(format!(
                        "invalid thinking_mode '{other}' for provider '{}' (expected 'split_only' or 'strip_all')",
                        pp.name
                    )));
                }
            };
            let format_mode = match pp.format_mode.as_deref().map(str::trim) {
                None | Some("") | Some("both") => crate::config::FormatMode::Both,
                Some("anthropic") => crate::config::FormatMode::Anthropic,
                Some("openai") => crate::config::FormatMode::OpenAi,
                Some(other) => {
                    return Err(ProxyError::BadRequest(format!(
                        "invalid format_mode '{other}' for provider '{}' (expected 'both', 'anthropic' or 'openai')",
                        pp.name
                    )));
                }
            };
            Ok(ProviderConfig {
                name: pp.name,
                kind,
                auth: payload_to_auth(pp.auth),
                anthropic_base_url: pp.anthropic_base_url,
                openai_base_url: pp.openai_base_url,
                thinking_mode,
                format_mode,
                thinking_level,
                thinking_force: pp.thinking_force.unwrap_or(false),
                max_concurrent: pp.max_concurrent,
                sanitize_empty_tools: pp.sanitize_empty_tools.unwrap_or(false),
                enabled: pp.enabled,
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
    let quota = p
        .quota
        .into_iter()
        .map(|qp| QuotaRule {
            provider: qp.provider,
            window: qp.window,
            max_requests: qp.max_requests,
            max_input_tokens: qp.max_input_tokens,
            max_output_tokens: qp.max_output_tokens,
            warn_pct: qp.warn_pct,
        })
        .collect();
    Ok(Config {
        port: p.port,
        proxy_db,
        pricing_db,
        providers,
        routing,
        affinity: AffinityConfig {
            enabled: p.affinity.enabled,
            headers: p.affinity.headers,
        },
        quota,
    })
}

fn strategy_to_payload(
    s: crate::config::RoutingStrategy,
) -> proxy_admin_api::RoutingStrategyPayload {
    match s {
        crate::config::RoutingStrategy::Failover => {
            proxy_admin_api::RoutingStrategyPayload::Failover
        }
        crate::config::RoutingStrategy::RoundRobin => {
            proxy_admin_api::RoutingStrategyPayload::RoundRobin
        }
    }
}

fn payload_to_strategy(
    s: proxy_admin_api::RoutingStrategyPayload,
) -> crate::config::RoutingStrategy {
    match s {
        proxy_admin_api::RoutingStrategyPayload::Failover => {
            crate::config::RoutingStrategy::Failover
        }
        proxy_admin_api::RoutingStrategyPayload::RoundRobin => {
            crate::config::RoutingStrategy::RoundRobin
        }
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
    }
}

fn str_to_kind(s: &str) -> Result<ProviderKind, ProxyError> {
    match s.trim().to_ascii_lowercase().as_str() {
        "openai" | "open_ai" => Ok(ProviderKind::OpenAi),
        "anthropic" => Ok(ProviderKind::Anthropic),
        "zai" => Ok(ProviderKind::Zai),
        "deepseek" => Ok(ProviderKind::DeepSeek),
        "codex" => Ok(ProviderKind::Codex),
        "minimax" => Ok(ProviderKind::Minimax),
        "kimi" | "moonshot" => Ok(ProviderKind::Kimi),
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
        AuthConfig::OpenAiOAuth {
            access_token,
            refresh_token,
            expires_at_ms,
        } => AuthPayload::OpenAiOAuth {
            access_token: access_token.clone(),
            refresh_token: refresh_token.clone(),
            expires_at_ms: *expires_at_ms,
        },
        AuthConfig::CodexAuto => AuthPayload::CodexAuto,
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
        AuthPayload::OpenAiOAuth {
            access_token,
            refresh_token,
            expires_at_ms,
        } => AuthConfig::OpenAiOAuth {
            access_token,
            refresh_token,
            expires_at_ms,
        },
        AuthPayload::CodexAuto => AuthConfig::CodexAuto,
    }
}

// ---------------------------------------------------------------------------
// GetAccountUsage
// ---------------------------------------------------------------------------

use crate::application::ports::AccountUsageRegistry;
use crate::domain::account_usage::{AccountUsageStatus, ProviderAccountUsage};
use proxy_admin_api::{
    AccountUsageResponse, ModelUsageDto, ProviderAccountUsageDto, ProviderUsageStatus,
    UsageSubItemDto, UsageWindowDto,
};

pub struct GetAccountUsage {
    registry: Arc<dyn AccountUsageRegistry>,
    read: Arc<dyn RequestLogReadPort>,
}

impl GetAccountUsage {
    pub fn new(registry: Arc<dyn AccountUsageRegistry>, read: Arc<dyn RequestLogReadPort>) -> Self {
        Self { registry, read }
    }

    pub fn execute(&self) -> AccountUsageResponse {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let from_ms = now_ms - 24 * 3600 * 1000;

        // Pull the live adapter map — picks up providers added at runtime.
        let adapters = self.registry.adapters();
        let mut providers: Vec<ProviderAccountUsageDto> = adapters
            .iter()
            .map(|(name, adapter)| {
                let mut dto = match adapter.fetch_usage() {
                    None => ProviderAccountUsageDto {
                        provider: name.clone(),
                        status: ProviderUsageStatus::NotSupported,
                        plan: None,
                        windows: vec![],
                        model_usage: None,
                        error_message: None,
                        monthly_cost_usd: None,
                    },
                    Some(Ok(usage)) => account_usage_to_dto(usage),
                    Some(Err(e)) => {
                        tracing::warn!(error = %e, "account usage fetch failed");
                        ProviderAccountUsageDto {
                            provider: name.clone(),
                            status: ProviderUsageStatus::Error,
                            plan: None,
                            windows: vec![],
                            model_usage: None,
                            error_message: Some(e.to_string()),
                            monthly_cost_usd: None,
                        }
                    }
                };

                // Enrich with per-model breakdown from proxy request log.
                match self.read.model_breakdown(name, from_ms, now_ms) {
                    Ok(rows) if !rows.is_empty() => {
                        let breakdown: Vec<ModelBreakdownItemDto> = rows
                            .into_iter()
                            .map(|r| ModelBreakdownItemDto {
                                model: r.model,
                                tokens: r.tokens,
                                calls: r.calls,
                                cost_usd: r.cost_usd,
                            })
                            .collect();
                        match &mut dto.model_usage {
                            Some(mu) => {
                                mu.model_breakdown = breakdown;
                            }
                            None => {
                                let total_tokens = breakdown.iter().map(|b| b.tokens).sum();
                                let total_calls = breakdown.iter().map(|b| b.calls).sum();
                                dto.model_usage = Some(ModelUsageDto {
                                    total_tokens,
                                    total_calls,
                                    period_start_ms: from_ms,
                                    period_end_ms: now_ms,
                                    model_breakdown: breakdown,
                                });
                            }
                        }
                    }
                    Ok(_) => {} // empty — no breakdown to add
                    Err(e) => {
                        tracing::debug!(error = %e, provider = %name, "model_breakdown query failed (non-fatal)");
                    }
                }

                // Monthly cost (last 30 days).
                match self.read.monthly_cost(name) {
                    Ok(cost) => { dto.monthly_cost_usd = Some(cost); }
                    Err(e) => { tracing::debug!(error = %e, provider = %name, "monthly_cost query failed (non-fatal)"); }
                }

                dto
            })
            .collect();

        providers.sort_by(|a, b| a.provider.cmp(&b.provider));
        AccountUsageResponse { providers }
    }
}

fn account_usage_to_dto(u: ProviderAccountUsage) -> ProviderAccountUsageDto {
    ProviderAccountUsageDto {
        provider: u.provider,
        status: match &u.status {
            AccountUsageStatus::Available => ProviderUsageStatus::Available,
            AccountUsageStatus::NotSupported => ProviderUsageStatus::NotSupported,
            AccountUsageStatus::Error(_) => ProviderUsageStatus::Error,
        },
        plan: u.plan,
        error_message: match &u.status {
            AccountUsageStatus::Error(msg) => Some(msg.clone()),
            _ => None,
        },
        monthly_cost_usd: None,
        windows: u
            .windows
            .into_iter()
            .map(|w| UsageWindowDto {
                label: w.label,
                used_pct: w.used_pct,
                used: w.used,
                limit: w.limit,
                resets_at_ms: w.resets_at_ms,
                sub_items: w
                    .sub_items
                    .into_iter()
                    .map(|s| UsageSubItemDto {
                        label: s.label,
                        used: s.used,
                    })
                    .collect(),
                is_balance_info: w.is_balance_info,
            })
            .collect(),
        model_usage: u.model_usage.map(|m| ModelUsageDto {
            total_tokens: m.total_tokens,
            total_calls: m.total_calls,
            period_start_ms: m.period_start_ms,
            period_end_ms: m.period_end_ms,
            model_breakdown: m
                .model_breakdown
                .into_iter()
                .map(|item| ModelBreakdownItemDto {
                    model: item.model,
                    tokens: item.tokens,
                    calls: item.calls,
                    cost_usd: item.cost_usd,
                })
                .collect(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::{AccountUsagePort, ModelBreakdownRow};
    use crate::domain::account_usage::{AccountUsageStatus, UsageWindow};
    use crate::domain::{DailyTotal, ModelTotal, RequestRow, UsageSummary};
    use std::collections::{BTreeMap, HashMap};
    use std::path::PathBuf;

    fn empty_stub_read() -> Arc<StubRead> {
        Arc::new(StubRead {
            total: 0,
            by_provider: BTreeMap::new(),
            by_status: BTreeMap::new(),
            rows: vec![],
        })
    }

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
        fn recent(&self, _: u32, _: u32) -> Result<Vec<RequestRow>, ProxyError> {
            Ok(self.rows.clone())
        }
        fn summarize(&self, from_ms: i64, to_ms: i64) -> Result<UsageSummary, ProxyError> {
            Ok(UsageSummary {
                from_ms,
                to_ms,
                daily: vec![DailyTotal {
                    date: "2026-05-03".into(),
                    requests: 3,
                    input_tokens: 100,
                    output_tokens: 50,
                    cache_read_tokens: 10,
                    cache_creation_tokens: 5,
                    cost_usd: 0.25,
                }],
                models: vec![ModelTotal {
                    model: "claude-opus-4-5".into(),
                    provider: "anthropic".into(),
                    requests: 3,
                    input_tokens: 100,
                    output_tokens: 50,
                    cache_read_tokens: 10,
                    cache_creation_tokens: 5,
                    cost_usd: 0.25,
                }],
            })
        }
        fn quota_seed(
            &self,
            _cutoff_ms: i64,
        ) -> Result<Vec<crate::application::ports::QuotaSeedRow>, ProxyError> {
            Ok(vec![])
        }
        fn count_translations(
            &self,
        ) -> Result<crate::application::ports::TranslationCounts, ProxyError> {
            Ok(crate::application::ports::TranslationCounts::default())
        }
        fn model_breakdown(
            &self,
            _provider: &str,
            _from_ms: i64,
            _to_ms: i64,
        ) -> Result<Vec<ModelBreakdownRow>, ProxyError> {
            Ok(vec![])
        }
        fn monthly_cost(&self, _provider: &str) -> Result<f64, ProxyError> {
            Ok(0.0)
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
        let cfg = Arc::new(RwLock::new(Config {
            port: 8787,
            proxy_db: PathBuf::from("/tmp/proxy.db"),
            pricing_db: PathBuf::from("/tmp/pricing.db"),
            providers: vec![],
            routing: vec![],
            affinity: Default::default(),
            quota: Vec::new(),
        }));
        let uc = GetStatus::new(stub(), 1_000, cfg);
        let s = uc.execute().unwrap();
        assert_eq!(s.total_requests, 7);
        assert_eq!(s.requests_by_provider.get("anthropic"), Some(&4));
        assert_eq!(s.requests_by_status.get("completed"), Some(&6));
        assert_eq!(s.started_at_ms, 1_000);
        assert!(s.affinity.enabled); // default is enabled
    }

    #[test]
    fn get_recent_clamps_limit_to_max() {
        let uc = GetRecentRequests::new(stub());
        // No way to observe the clamp via the stub's fixed return, but we
        // can at least confirm it doesn't panic on extreme values.
        let _ = uc.execute(Some(0), None).unwrap();
        let _ = uc.execute(Some(u32::MAX), None).unwrap();
    }

    #[test]
    fn config_to_payload_roundtrip() {
        let cfg = Config {
            port: 8787,
            proxy_db: PathBuf::from("/tmp/proxy.db"),
            pricing_db: PathBuf::from("/tmp/pricing.db"),
            providers: vec![ProviderConfig {
                thinking_level: crate::config::ThinkingLevel::Unset,
                thinking_force: false,
                name: "anthropic".into(),
                kind: ProviderKind::Anthropic,
                auth: AuthConfig::ApiKey {
                    value: "sk-test".into(),
                },
                anthropic_base_url: None,
                openai_base_url: None,
                thinking_mode: crate::config::ThinkingMode::SplitOnly,
                format_mode: crate::config::FormatMode::Both,
                max_concurrent: None,
                sanitize_empty_tools: false,
                enabled: true,
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
            affinity: Default::default(),
            quota: Vec::new(),
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

    fn config_with_thinking(
        kind: ProviderKind,
        level: crate::config::ThinkingLevel,
        force: bool,
    ) -> Config {
        Config {
            port: 8787,
            proxy_db: PathBuf::from("/tmp/proxy.db"),
            pricing_db: PathBuf::from("/tmp/pricing.db"),
            providers: vec![ProviderConfig {
                name: "p".into(),
                kind,
                auth: AuthConfig::Bearer { value: "k".into() },
                anthropic_base_url: None,
                openai_base_url: Some("https://example.invalid".into()),
                format_mode: crate::config::FormatMode::Both,
                thinking_level: level,
                thinking_force: force,
                thinking_mode: crate::config::ThinkingMode::SplitOnly,
                max_concurrent: None,
                sanitize_empty_tools: false,
                enabled: true,
            }],
            routing: vec![],
            affinity: Default::default(),
            quota: vec![],
        }
    }

    #[test]
    fn payload_round_trips_the_thinking_level() {
        let cfg =
            config_with_thinking(ProviderKind::Kimi, crate::config::ThinkingLevel::High, true);
        let payload = config_to_payload(&cfg);
        assert_eq!(payload.providers[0].thinking_level.as_deref(), Some("high"));
        assert_eq!(payload.providers[0].thinking_force, Some(true));

        let restored = payload_to_config(
            payload,
            PathBuf::from("/tmp/p.db"),
            PathBuf::from("/tmp/pr.db"),
            &cfg,
        )
        .unwrap();
        assert_eq!(
            restored.providers[0].thinking_level,
            crate::config::ThinkingLevel::High
        );
        assert!(restored.providers[0].thinking_force);
    }

    #[test]
    fn payload_to_config_rejects_a_level_the_kind_does_not_offer() {
        // DeepSeek maps low/medium to high server-side, so they are not offered.
        let mut payload = config_to_payload(&config_with_thinking(
            ProviderKind::DeepSeek,
            crate::config::ThinkingLevel::Max,
            false,
        ));
        payload.providers[0].thinking_level = Some("low".into());
        let err = payload_to_config(
            payload,
            PathBuf::from("/tmp/p.db"),
            PathBuf::from("/tmp/pr.db"),
            &config_with_thinking(
                ProviderKind::DeepSeek,
                crate::config::ThinkingLevel::Unset,
                false,
            ),
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("thinking_level"), "got: {msg}");
        assert!(
            msg.contains("high"),
            "the error must list the valid levels: {msg}"
        );
    }

    #[test]
    fn payload_to_config_treats_missing_and_empty_level_as_unset() {
        for value in [None, Some(String::new())] {
            let mut payload = config_to_payload(&config_with_thinking(
                ProviderKind::Zai,
                crate::config::ThinkingLevel::High,
                false,
            ));
            payload.providers[0].thinking_level = value;
            let restored = payload_to_config(
                payload,
                PathBuf::from("/tmp/p.db"),
                PathBuf::from("/tmp/pr.db"),
                &config_with_thinking(
                    ProviderKind::Zai,
                    crate::config::ThinkingLevel::Unset,
                    false,
                ),
            )
            .unwrap();
            assert_eq!(
                restored.providers[0].thinking_level,
                crate::config::ThinkingLevel::Unset
            );
        }
    }

    #[test]
    fn config_payload_preserves_thinking_mode() {
        let cfg = Config {
            port: 8787,
            proxy_db: PathBuf::from("/tmp/proxy.db"),
            pricing_db: PathBuf::from("/tmp/pricing.db"),
            providers: vec![ProviderConfig {
                thinking_level: crate::config::ThinkingLevel::Unset,
                thinking_force: false,
                name: "minimax".into(),
                kind: ProviderKind::Minimax,
                auth: AuthConfig::Passthrough,
                anthropic_base_url: None,
                openai_base_url: None,
                thinking_mode: crate::config::ThinkingMode::StripAll,
                format_mode: crate::config::FormatMode::Both,
                max_concurrent: None,
                sanitize_empty_tools: false,
                enabled: true,
            }],
            routing: vec![],
            affinity: AffinityConfig::default(),
            quota: vec![],
        };

        let payload = config_to_payload(&cfg);
        assert_eq!(
            payload.providers[0].thinking_mode.as_deref(),
            Some("strip_all")
        );

        let restored = payload_to_config(
            payload,
            PathBuf::from("/tmp/proxy.db"),
            PathBuf::from("/tmp/pricing.db"),
            &cfg,
        )
        .unwrap();
        assert_eq!(
            restored.providers[0].thinking_mode,
            crate::config::ThinkingMode::StripAll
        );
    }

    #[test]
    fn config_payload_preserves_sanitize_empty_tools() {
        let cfg = Config {
            port: 8787,
            proxy_db: PathBuf::from("/tmp/proxy.db"),
            pricing_db: PathBuf::from("/tmp/pricing.db"),
            providers: vec![ProviderConfig {
                thinking_level: crate::config::ThinkingLevel::Unset,
                thinking_force: false,
                name: "moonshot".into(),
                kind: ProviderKind::Kimi,
                auth: AuthConfig::Passthrough,
                anthropic_base_url: None,
                openai_base_url: None,
                thinking_mode: crate::config::ThinkingMode::SplitOnly,
                format_mode: crate::config::FormatMode::Both,
                max_concurrent: None,
                sanitize_empty_tools: true,
                enabled: true,
            }],
            routing: vec![],
            affinity: AffinityConfig::default(),
            quota: vec![],
        };

        let payload = config_to_payload(&cfg);
        assert_eq!(payload.providers[0].sanitize_empty_tools, Some(true));

        let restored = payload_to_config(
            payload,
            PathBuf::from("/tmp/proxy.db"),
            PathBuf::from("/tmp/pricing.db"),
            &cfg,
        )
        .unwrap();
        assert!(restored.providers[0].sanitize_empty_tools);
    }

    #[test]
    fn payload_to_config_rejects_invalid_thinking_mode() {
        let p = ConfigPayload {
            port: 8787,
            providers: vec![ProviderPayload {
                name: "minimax".into(),
                kind: "minimax".into(),
                enabled: true,
                auth: AuthPayload::Passthrough,
                anthropic_base_url: None,
                openai_base_url: None,
                thinking_mode: Some("garbage".into()),
                thinking_level: None,
                thinking_force: None,
                format_mode: None,
                max_concurrent: None,
                sanitize_empty_tools: None,
            }],
            routing: vec![],
            quota: vec![],
            affinity: AffinityPayload::default(),
            proxy_db: None,
            pricing_db: None,
        };
        let existing = Config {
            port: 8787,
            proxy_db: PathBuf::new(),
            pricing_db: PathBuf::new(),
            providers: vec![],
            routing: vec![],
            affinity: AffinityConfig::default(),
            quota: Vec::new(),
        };

        let err = payload_to_config(p, PathBuf::new(), PathBuf::new(), &existing).unwrap_err();
        assert!(err.to_string().contains("invalid thinking_mode"));
    }

    #[test]
    fn get_usage_summary_maps_domain_to_dto() {
        let uc = GetUsageSummary::new(stub());
        let resp = uc.execute(0, i64::MAX).unwrap();
        assert_eq!(resp.from_ms, 0);
        assert_eq!(resp.to_ms, i64::MAX);
        assert_eq!(resp.daily.len(), 1);
        assert_eq!(resp.daily[0].requests, 3);
        assert_eq!(resp.models.len(), 1);
        assert_eq!(resp.models[0].model, "claude-opus-4-5");
    }

    #[test]
    fn get_usage_summary_rejects_inverted_range() {
        let uc = GetUsageSummary::new(stub());
        let err = uc.execute(100, 50).unwrap_err();
        assert!(matches!(err, ProxyError::BadRequest(_)));
    }

    #[test]
    fn payload_to_config_rejects_unknown_kind() {
        let p = ConfigPayload {
            port: 8787,
            providers: vec![ProviderPayload {
                name: "x".into(),
                kind: "bogus".into(),
                enabled: true,
                auth: AuthPayload::Passthrough,
                anthropic_base_url: None,
                openai_base_url: None,
                thinking_mode: None,
                thinking_level: None,
                thinking_force: None,
                format_mode: None,
                max_concurrent: None,
                sanitize_empty_tools: None,
            }],
            routing: vec![],
            quota: vec![],
            affinity: AffinityPayload {
                enabled: true,
                headers: vec![],
            },
            proxy_db: None,
            pricing_db: None,
        };
        let existing = Config {
            port: 8787,
            proxy_db: PathBuf::new(),
            pricing_db: PathBuf::new(),
            providers: vec![],
            routing: vec![],
            affinity: Default::default(),
            quota: Vec::new(),
        };
        let err = payload_to_config(p, PathBuf::new(), PathBuf::new(), &existing).unwrap_err();
        assert!(format!("{err}").contains("bogus"));
    }

    #[test]
    fn payload_to_config_preserves_quota_rules() {
        use crate::config::QuotaRule;

        let original = Config {
            port: 8787,
            proxy_db: PathBuf::from("/tmp/proxy.db"),
            pricing_db: PathBuf::from("/tmp/pricing.db"),
            providers: vec![],
            routing: vec![],
            affinity: Default::default(),
            quota: vec![QuotaRule {
                provider: "zai".into(),
                window: "rolling:1h".into(),
                max_requests: Some(100),
                max_input_tokens: None,
                max_output_tokens: None,
                warn_pct: 80,
            }],
        };
        let payload = config_to_payload(&original);
        let roundtripped = payload_to_config(
            payload,
            original.proxy_db.clone(),
            original.pricing_db.clone(),
            &original,
        )
        .unwrap();
        assert_eq!(
            roundtripped.quota.len(),
            1,
            "quota rules must survive a config PUT round-trip"
        );
        assert_eq!(roundtripped.quota[0].provider, "zai");
    }

    #[test]
    fn payload_to_config_parses_enabled() {
        let original = Config {
            port: 8787,
            proxy_db: PathBuf::from("/tmp/proxy.db"),
            pricing_db: PathBuf::from("/tmp/pricing.db"),
            providers: vec![ProviderConfig {
                thinking_level: crate::config::ThinkingLevel::Unset,
                thinking_force: false,
                name: "off".into(),
                kind: ProviderKind::Anthropic,
                auth: AuthConfig::Passthrough,
                anthropic_base_url: None,
                openai_base_url: None,
                thinking_mode: crate::config::ThinkingMode::SplitOnly,
                format_mode: crate::config::FormatMode::Both,
                max_concurrent: None,
                sanitize_empty_tools: false,
                enabled: false,
            }],
            routing: vec![],
            affinity: Default::default(),
            quota: vec![],
        };
        let payload = config_to_payload(&original);
        // A disabled provider with no routing references is a valid config.
        let roundtripped = payload_to_config(
            payload,
            original.proxy_db.clone(),
            original.pricing_db.clone(),
            &original,
        )
        .unwrap();
        assert!(
            !roundtripped.providers[0].enabled,
            "enabled=false must survive a config PUT round-trip"
        );
    }

    #[test]
    fn config_to_payload_round_trips_enabled() {
        let cfg = Config {
            port: 8787,
            proxy_db: PathBuf::from("/tmp/proxy.db"),
            pricing_db: PathBuf::from("/tmp/pricing.db"),
            providers: vec![ProviderConfig {
                thinking_level: crate::config::ThinkingLevel::Unset,
                thinking_force: false,
                name: "off".into(),
                kind: ProviderKind::Anthropic,
                auth: AuthConfig::Passthrough,
                anthropic_base_url: None,
                openai_base_url: None,
                thinking_mode: crate::config::ThinkingMode::SplitOnly,
                format_mode: crate::config::FormatMode::Both,
                max_concurrent: None,
                sanitize_empty_tools: false,
                enabled: false,
            }],
            routing: vec![],
            affinity: Default::default(),
            quota: vec![],
        };
        let payload = config_to_payload(&cfg);
        assert!(!payload.providers[0].enabled);
    }

    // --- GetAccountUsage tests ---

    struct StubAccountUsage {
        result: Option<Result<ProviderAccountUsage, String>>,
    }

    impl AccountUsagePort for StubAccountUsage {
        fn fetch_usage(&self) -> Option<Result<ProviderAccountUsage, ProxyError>> {
            self.result.as_ref().map(|r| match r {
                Ok(usage) => Ok(usage.clone()),
                Err(msg) => Err(ProxyError::UpstreamUsage {
                    provider: "stub".to_string(),
                    message: msg.clone(),
                }),
            })
        }
    }

    #[test]
    fn get_account_usage_returns_not_supported_for_none() {
        let mut map = HashMap::new();
        map.insert(
            "anthropic".to_string(),
            Arc::new(StubAccountUsage { result: None }) as Arc<dyn AccountUsagePort>,
        );
        let uc = GetAccountUsage::new(Arc::new(map), empty_stub_read());
        let resp = uc.execute();
        assert_eq!(resp.providers.len(), 1);
        assert_eq!(resp.providers[0].status, ProviderUsageStatus::NotSupported);
    }

    #[test]
    fn get_account_usage_returns_available_on_success() {
        let usage = ProviderAccountUsage {
            provider: "zai".to_string(),
            status: AccountUsageStatus::Available,
            plan: Some("pro".to_string()),
            windows: vec![UsageWindow {
                label: "5h Token".to_string(),
                used_pct: 40.0,
                used: Some(16_000_000),
                limit: Some(40_000_000),
                resets_at_ms: Some(1_746_300_000_000),
                sub_items: vec![],
                is_balance_info: false,
            }],
            model_usage: None,
        };
        let mut map = HashMap::new();
        map.insert(
            "zai".to_string(),
            Arc::new(StubAccountUsage {
                result: Some(Ok(usage)),
            }) as Arc<dyn AccountUsagePort>,
        );
        let uc = GetAccountUsage::new(Arc::new(map), empty_stub_read());
        let resp = uc.execute();
        assert_eq!(resp.providers.len(), 1);
        assert_eq!(resp.providers[0].status, ProviderUsageStatus::Available);
        assert_eq!(resp.providers[0].plan.as_deref(), Some("pro"));
        assert_eq!(resp.providers[0].windows.len(), 1);
    }

    #[test]
    fn get_account_usage_returns_error_on_failure() {
        let mut map = HashMap::new();
        map.insert(
            "bad".to_string(),
            Arc::new(StubAccountUsage {
                result: Some(Err("timeout".to_string())),
            }) as Arc<dyn AccountUsagePort>,
        );
        let uc = GetAccountUsage::new(Arc::new(map), empty_stub_read());
        let resp = uc.execute();
        assert_eq!(resp.providers.len(), 1);
        assert_eq!(resp.providers[0].status, ProviderUsageStatus::Error);
    }

    #[test]
    fn get_account_usage_sorts_providers_by_name() {
        let mut map = HashMap::new();
        map.insert(
            "zai".to_string(),
            Arc::new(StubAccountUsage { result: None }) as Arc<dyn AccountUsagePort>,
        );
        map.insert(
            "anthropic".to_string(),
            Arc::new(StubAccountUsage { result: None }) as Arc<dyn AccountUsagePort>,
        );
        let uc = GetAccountUsage::new(Arc::new(map), empty_stub_read());
        let resp = uc.execute();
        assert_eq!(resp.providers[0].provider, "anthropic");
        assert_eq!(resp.providers[1].provider, "zai");
    }

    #[test]
    fn get_account_usage_empty_map_returns_empty() {
        let uc = GetAccountUsage::new(Arc::new(HashMap::new()), empty_stub_read());
        let resp = uc.execute();
        assert!(resp.providers.is_empty());
    }
}
