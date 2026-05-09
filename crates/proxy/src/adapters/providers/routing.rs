//! Routing provider — selects an upstream provider per-request based on glob
//! matching on the body's `model` field. First match wins across rules.
//!
//! Two strategies per rule:
//! - **failover** (default) — always try `provider` first, then `fallback`
//!   in order on transport error or buffered 5xx.
//! - **round_robin** — rotate across all providers in the pool. On 429 or
//!   5xx, mark that provider as cooling down and try next.
//!
//! Cooldown tracking: each provider in a round-robin pool has a
//! `cooldown_until` timestamp. When a provider returns 429/5xx, it's marked
//! with `now + retry_after_ms`. The rotation skips cooling-down providers.
//! If all providers are cooling down, returns 429 to the client with the
//! shortest remaining cooldown as `Retry-After`.

use super::messages_protocol;
use crate::adapters::translation::stream_wrap;
use crate::adapters::translation::{anthropic_to_openai, openai_to_anthropic};
use crate::application::errors::ProxyError;
use crate::application::ports::{
    ApiFormat, Direction, Provider, QuotaPort, UpstreamResponse, UsageParser,
};
use crate::config::RoutingStrategy;
use crate::domain::UsageRecord;
use async_trait::async_trait;
use axum::http::HeaderMap;
use bytes::Bytes;
use globset::{Glob, GlobMatcher};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// Default cooldown when upstream doesn't send Retry-After (seconds).
const DEFAULT_COOLDOWN_SECS: u64 = 60;

// ── Route ──────────────────────────────────────────────────────────────────────

struct Route {
    matcher: GlobMatcher,
    strategy: RoutingStrategy,
    /// Ordered pool of providers: index 0 is `provider`, rest are `fallback`.
    pool: Vec<PoolEntry>,
}

struct PoolEntry {
    provider: Arc<dyn Provider>,
    /// Stable identifier — provider name from config. Used for affinity scoring.
    id: String,
    /// Epoch millis when cooldown expires. 0 = healthy.
    cooldown_until: AtomicU64,
}

impl PoolEntry {
    fn new(provider: Arc<dyn Provider>, id: String) -> Self {
        Self {
            provider,
            id,
            cooldown_until: AtomicU64::new(0),
        }
    }

    fn is_cooling_down(&self) -> bool {
        let until = self.cooldown_until.load(Ordering::Relaxed);
        if until == 0 {
            return false;
        }
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        if now_ms >= until {
            // Cooldown expired, clear it.
            self.cooldown_until.store(0, Ordering::Relaxed);
            return false;
        }
        true
    }

    fn set_cooldown(&self, duration_ms: u64) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.cooldown_until
            .store(now_ms + duration_ms, Ordering::Relaxed);
    }

    fn remaining_cooldown_ms(&self) -> u64 {
        let until = self.cooldown_until.load(Ordering::Relaxed);
        if until == 0 {
            return 0;
        }
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        until.saturating_sub(now_ms)
    }
}

impl super::affinity::PoolMember for PoolEntry {
    fn id(&self) -> &str {
        &self.id
    }

    fn healthy(&self) -> bool {
        !self.is_cooling_down()
    }
}

// ── RoutingProvider ────────────────────────────────────────────────────────────

/// Split `model` on the first `/`. Returns `Some((namespace, bare_model))`
/// if a `/` is present, `None` otherwise.
fn split_namespace(model: &str) -> Option<(&str, &str)> {
    let idx = model.find('/')?;
    Some((&model[..idx], &model[idx + 1..]))
}

/// Replace the `"model"` field in a JSON body with `new_model`, returning
/// the re-serialized body bytes.
fn rewrite_model_in_body(body: &[u8], new_model: &str) -> Result<Vec<u8>, String> {
    let mut value: serde_json::Value =
        serde_json::from_slice(body).map_err(|e| format!("invalid json: {e}"))?;
    value["model"] = serde_json::Value::String(new_model.to_string());
    serde_json::to_vec(&value).map_err(|e| format!("json serialize: {e}"))
}

pub struct RoutingProvider {
    rules: Vec<Route>,
    /// Counter for round-robin rotation. Incremented per request.
    rr_counter: AtomicUsize,
    /// Name → provider map for namespace routing (e.g. "zai" → ZaiProvider).
    leaves: std::collections::HashMap<String, Arc<dyn Provider>>,
    /// Conversation-affinity config. Controls sticky rendezvous hashing.
    affinity: crate::config::AffinityConfig,
    /// Quota enforcement — checked per leaf provider before forwarding.
    quota: Arc<dyn QuotaPort>,
}

impl RoutingProvider {
    pub fn builder() -> RoutingProviderBuilder {
        RoutingProviderBuilder::default()
    }

    fn select(&self, model: &str) -> Option<&Route> {
        self.rules.iter().find(|r| r.matcher.is_match(model))
    }

    /// If `model` contains a namespace prefix (e.g. "zai/glm-5"), look up
    /// the provider by name. Returns the provider and the bare model name.
    fn resolve_provider<'a>(&'a self, model: &'a str) -> Option<(&'a Arc<dyn Provider>, &'a str)> {
        let (namespace, bare_model) = split_namespace(model)?;
        let provider = self.leaves.get(namespace)?;
        Some((provider, bare_model))
    }
}

pub struct RoutingProviderBuilder {
    rules: Vec<Route>,
    leaves: std::collections::HashMap<String, Arc<dyn Provider>>,
    affinity: crate::config::AffinityConfig,
    quota: Arc<dyn QuotaPort>,
}

impl Default for RoutingProviderBuilder {
    fn default() -> Self {
        Self {
            rules: Vec::new(),
            leaves: std::collections::HashMap::new(),
            affinity: Default::default(),
            quota: Arc::new(crate::adapters::quota::NoopQuota),
        }
    }
}

impl RoutingProviderBuilder {
    /// Set the name→provider map for namespace routing.
    pub fn leaves(mut self, leaves: std::collections::HashMap<String, Arc<dyn Provider>>) -> Self {
        self.leaves = leaves;
        self
    }

    pub fn affinity(mut self, affinity: crate::config::AffinityConfig) -> Self {
        self.affinity = affinity;
        self
    }

    pub fn quota(mut self, quota: Arc<dyn QuotaPort>) -> Self {
        self.quota = quota;
        self
    }

    pub fn rule(
        mut self,
        pattern: &str,
        strategy: RoutingStrategy,
        primary: Arc<dyn Provider>,
        fallback: Vec<Arc<dyn Provider>>,
    ) -> Result<Self, globset::Error> {
        let matcher = Glob::new(pattern)?.compile_matcher();
        let mut pool = Vec::with_capacity(1 + fallback.len());
        pool.push(PoolEntry::new(primary.clone(), primary.name().to_string()));
        for fb in fallback {
            let id = fb.name().to_string();
            pool.push(PoolEntry::new(fb, id));
        }
        self.rules.push(Route {
            matcher,
            strategy,
            pool,
        });
        Ok(self)
    }

    pub fn build(self) -> RoutingProvider {
        RoutingProvider {
            rules: self.rules,
            rr_counter: AtomicUsize::new(0),
            leaves: self.leaves,
            affinity: self.affinity,
            quota: self.quota,
        }
    }
}

// `native_format` intentionally not overridden — routing is dynamic and the leaf
// provider's native_format is what matters; translation triggers per-entry inside
// messages_protocol after routing has selected an entry.
#[async_trait]
impl Provider for RoutingProvider {
    fn name(&self) -> &'static str {
        "router"
    }

    fn parse_model(&self, body: &[u8]) -> Result<String, String> {
        messages_protocol::parse_model(body)
    }

    fn usage_parser(&self) -> Box<dyn UsageParser> {
        messages_protocol::usage_parser()
    }

    fn parse_usage_json(&self, body: &[u8]) -> Result<UsageRecord, String> {
        messages_protocol::parse_usage_json(body)
    }

    fn usage_parser_openai(&self) -> Box<dyn UsageParser> {
        messages_protocol::openai_usage_parser()
    }

    fn parse_usage_json_openai(&self, body: &[u8]) -> Result<UsageRecord, String> {
        messages_protocol::parse_openai_usage_json(body)
    }

    async fn forward(
        &self,
        path: &str,
        headers: &HeaderMap,
        body: Bytes,
        streaming: bool,
    ) -> Result<UpstreamResponse, ProxyError> {
        let model = messages_protocol::parse_model(&body).map_err(ProxyError::BadRequest)?;

        // Namespace routing: if model contains "/", extract namespace and route
        // directly to the named provider.
        if let Some((provider, bare_model)) = self.resolve_provider(&model) {
            self.check_quota(provider.name())?;
            let rewritten =
                rewrite_model_in_body(&body, bare_model).map_err(ProxyError::BadRequest)?;
            let rewritten_body = Bytes::from(rewritten);
            let direction = Direction::from_pair(ApiFormat::Anthropic, provider.native_format());
            let native_path = Self::translate_path(path, direction);
            let send_body = Self::translate_request(&rewritten_body, direction)?;
            let raw_resp = match provider.native_format() {
                ApiFormat::Anthropic => provider.forward(native_path, headers, send_body, streaming).await,
                ApiFormat::OpenAI => {
                    provider
                        .forward_openai(native_path, headers, send_body, streaming)
                        .await
                }
            }?;
            return Self::translate_upstream_response(raw_resp, direction);
        }

        // If model contains "/" but we couldn't resolve, it's an unknown namespace.
        if let Some((ns, _)) = split_namespace(&model) {
            return Err(ProxyError::BadRequest(format!(
                "unknown provider namespace '{ns}'"
            )));
        }

        // No namespace — use existing glob-based routing.
        let route = self.select(&model).ok_or_else(|| {
            ProxyError::BadRequest(format!("no routing rule matches model '{model}'"))
        })?;

        match route.strategy {
            RoutingStrategy::Failover => {
                self.forward_failover(route, path, headers, body, streaming, ApiFormat::Anthropic)
                    .await
            }
            RoutingStrategy::RoundRobin => {
                self.forward_round_robin(
                    route,
                    path,
                    headers,
                    body,
                    streaming,
                    ApiFormat::Anthropic,
                )
                .await
            }
        }
    }

    async fn forward_openai(
        &self,
        path: &str,
        headers: &HeaderMap,
        body: Bytes,
        streaming: bool,
    ) -> Result<UpstreamResponse, ProxyError> {
        let model = messages_protocol::parse_model(&body).map_err(ProxyError::BadRequest)?;

        // Namespace routing.
        if let Some((provider, bare_model)) = self.resolve_provider(&model) {
            self.check_quota(provider.name())?;
            let rewritten =
                rewrite_model_in_body(&body, bare_model).map_err(ProxyError::BadRequest)?;
            let rewritten_body = Bytes::from(rewritten);
            let direction = Direction::from_pair(ApiFormat::OpenAI, provider.native_format());
            let native_path = Self::translate_path(path, direction);
            let send_body = Self::translate_request(&rewritten_body, direction)?;
            let raw_resp = match provider.native_format() {
                ApiFormat::OpenAI => {
                    provider
                        .forward_openai(native_path, headers, send_body, streaming)
                        .await
                }
                ApiFormat::Anthropic => provider.forward(native_path, headers, send_body, streaming).await,
            }?;
            return Self::translate_upstream_response(raw_resp, direction);
        }

        if let Some((ns, _)) = split_namespace(&model) {
            return Err(ProxyError::BadRequest(format!(
                "unknown provider namespace '{ns}'"
            )));
        }

        // Glob-based routing.
        let route = self.select(&model).ok_or_else(|| {
            ProxyError::BadRequest(format!("no routing rule matches model '{model}'"))
        })?;

        match route.strategy {
            RoutingStrategy::Failover => {
                self.forward_failover_openai(route, path, headers, body, streaming)
                    .await
            }
            RoutingStrategy::RoundRobin => {
                self.forward_round_robin_openai(route, path, headers, body, streaming)
                    .await
            }
        }
    }
}

impl RoutingProvider {
    /// Translate a request body from `client_format` to the provider's native format.
    /// Returns `(translated_body, direction)`. For `Passthrough` the original body is returned.
    fn translate_request(body: &Bytes, direction: Direction) -> Result<Bytes, ProxyError> {
        match direction {
            Direction::Passthrough => Ok(body.clone()),
            Direction::AnthropicToOpenAI => {
                let translated = anthropic_to_openai::request::translate(body)?;
                Ok(Bytes::from(translated))
            }
            Direction::OpenAIToAnthropic => {
                let translated = openai_to_anthropic::request::translate(body)?;
                Ok(Bytes::from(translated))
            }
        }
    }

    /// Translate a buffered response body from the provider's native format back to `client_format`.
    fn translate_response_buffered(
        body: &Bytes,
        direction: Direction,
    ) -> Result<Bytes, ProxyError> {
        match direction {
            Direction::Passthrough => Ok(body.clone()),
            Direction::AnthropicToOpenAI => {
                // upstream was OpenAI, translate back to Anthropic for client
                let translated = anthropic_to_openai::response::translate(body)?;
                Ok(Bytes::from(translated))
            }
            Direction::OpenAIToAnthropic => {
                // upstream was Anthropic, translate back to OpenAI for client
                let translated = openai_to_anthropic::response::translate(body)?;
                Ok(Bytes::from(translated))
            }
        }
    }

    /// Translate an `UpstreamResponse` according to `direction`.
    /// For streaming, wraps the byte stream with the appropriate FSM wrapper.
    fn translate_upstream_response(
        resp: UpstreamResponse,
        direction: Direction,
    ) -> Result<UpstreamResponse, ProxyError> {
        let direction_label = direction.as_label().map(str::to_string);
        match direction {
            Direction::Passthrough => Ok(resp),
            Direction::AnthropicToOpenAI => match resp {
                UpstreamResponse::Buffered {
                    status,
                    headers,
                    body,
                    provider_id,
                    ..
                } => {
                    let translated_body = Self::translate_response_buffered(&body, direction)?;
                    Ok(UpstreamResponse::Buffered {
                        status,
                        headers,
                        body: translated_body,
                        provider_id,
                        translation_direction: direction_label,
                    })
                }
                UpstreamResponse::Streaming {
                    status,
                    headers,
                    body,
                    provider_id,
                    ..
                } => Ok(UpstreamResponse::Streaming {
                    status,
                    headers,
                    body: stream_wrap::wrap_openai_to_anthropic(body),
                    provider_id,
                    translation_direction: direction_label,
                }),
            },
            Direction::OpenAIToAnthropic => match resp {
                UpstreamResponse::Buffered {
                    status,
                    headers,
                    body,
                    provider_id,
                    ..
                } => {
                    let translated_body = Self::translate_response_buffered(&body, direction)?;
                    Ok(UpstreamResponse::Buffered {
                        status,
                        headers,
                        body: translated_body,
                        provider_id,
                        translation_direction: direction_label,
                    })
                }
                UpstreamResponse::Streaming {
                    status,
                    headers,
                    body,
                    provider_id,
                    ..
                } => Ok(UpstreamResponse::Streaming {
                    status,
                    headers,
                    body: stream_wrap::wrap_anthropic_to_openai(body),
                    provider_id,
                    translation_direction: direction_label,
                }),
            },
        }
    }

    /// Translate the HTTP path to match the leaf provider's native format.
    /// When crossing format boundaries, `/v1/messages` ↔ `/chat/completions`.
    fn translate_path(path: &str, direction: Direction) -> &str {
        match direction {
            Direction::Passthrough => path,
            Direction::AnthropicToOpenAI => match path {
                "/v1/messages" => "/chat/completions",
                _ => path,
            },
            Direction::OpenAIToAnthropic => match path {
                "/chat/completions" => "/v1/messages",
                _ => path,
            },
        }
    }

    /// Pre-flight quota check for a leaf provider. Returns `Err(QuotaExceeded)`
    /// when the quota is exhausted, logs a warning on `Warn`, and is a no-op on `Ok`.
    fn check_quota(&self, provider_id: &str) -> Result<(), ProxyError> {
        match self.quota.check(provider_id) {
            crate::domain::quota::QuotaCheck::Ok => Ok(()),
            crate::domain::quota::QuotaCheck::Warn { metric, used_pct } => {
                tracing::warn!(
                    provider = %provider_id,
                    metric,
                    used_pct,
                    "quota approaching limit"
                );
                Ok(())
            }
            crate::domain::quota::QuotaCheck::Reject {
                metric,
                retry_after_ms,
            } => Err(ProxyError::QuotaExceeded {
                provider: provider_id.to_string(),
                metric,
                retry_after_ms,
            }),
        }
    }

    /// Return pool indices in the order they should be attempted.
    ///
    /// Sticky path: if affinity is enabled and a hash can be derived from the
    /// request, sort indices by rendezvous score (highest first).
    /// Fallback: standard round-robin starting from the next counter slot.
    fn compute_attempt_order(
        &self,
        pool: &[PoolEntry],
        headers: &HeaderMap,
        body: &[u8],
    ) -> Vec<usize> {
        let pool_size = pool.len();
        if pool_size == 0 {
            return vec![];
        }

        if self.affinity.enabled
            && let Some(hash) =
                super::affinity::affinity_hash(headers, body, &self.affinity.headers)
        {
            let mut scored: Vec<(usize, u64)> = pool
                .iter()
                .enumerate()
                .map(|(i, e)| (i, super::affinity::score_for(&e.id, hash)))
                .collect();
            scored.sort_by_key(|b| std::cmp::Reverse(b.1));
            return scored.into_iter().map(|(i, _)| i).collect();
        }

        // Fallback: round-robin from the next counter slot.
        let start = self.rr_counter.fetch_add(1, Ordering::Relaxed) % pool_size;
        (0..pool_size)
            .map(|offset| (start + offset) % pool_size)
            .collect()
    }

    /// Failover: try pool[0] first, then pool[1..] on 5xx/error.
    /// `client_format` is the API format the client used (Anthropic or OpenAI).
    async fn forward_failover(
        &self,
        route: &Route,
        path: &str,
        headers: &HeaderMap,
        body: Bytes,
        streaming: bool,
        client_format: ApiFormat,
    ) -> Result<UpstreamResponse, ProxyError> {
        let total = route.pool.len();
        let mut last_err: Option<ProxyError> = None;

        for (i, entry) in route.pool.iter().enumerate() {
            let attempt_name = entry.provider.name();
            // Pre-flight quota check against the leaf provider's config name.
            if let Err(e) = self.check_quota(&entry.id) {
                if i + 1 == total {
                    return Err(e);
                }
                last_err = Some(e);
                continue;
            }

            let direction = Direction::from_pair(client_format, entry.provider.native_format());
            let native_path = Self::translate_path(path, direction);
            let send_body = Self::translate_request(&body, direction)?;

            // Call the leaf provider using its native format.
            let raw_resp = match entry.provider.native_format() {
                ApiFormat::Anthropic => {
                    entry
                        .provider
                        .forward(native_path, headers, send_body, streaming)
                        .await
                }
                ApiFormat::OpenAI => {
                    entry
                        .provider
                        .forward_openai(native_path, headers, send_body, streaming)
                        .await
                }
            };

            match raw_resp {
                Ok(UpstreamResponse::Buffered {
                    status,
                    headers: resp_headers,
                    body: resp_body,
                    provider_id,
                    ..
                }) if status >= 500 => {
                    let preview =
                        String::from_utf8_lossy(&resp_body[..resp_body.len().min(200)]).to_string();
                    tracing::warn!(
                        provider = attempt_name,
                        status,
                        body = %preview,
                        attempt = i,
                        "upstream 5xx; trying next fallback"
                    );
                    if i + 1 == total {
                        // Return the last 5xx — apply translation anyway so the client
                        // gets the shape it expects.
                        let translated = Self::translate_upstream_response(
                            UpstreamResponse::Buffered {
                                status,
                                headers: resp_headers,
                                body: resp_body,
                                provider_id,
                                translation_direction: None,
                            },
                            direction,
                        )
                        .unwrap_or_else(|_| UpstreamResponse::Buffered {
                            status: 502,
                            headers: HeaderMap::new(),
                            body: Bytes::from_static(
                                b"{\"error\":{\"type\":\"translation_error\"}}",
                            ),
                            provider_id: attempt_name.to_string(),
                            translation_direction: None,
                        });
                        return Ok(translated);
                    }
                    continue;
                }
                Ok(other) => {
                    return Self::translate_upstream_response(other, direction);
                }
                Err(e) => {
                    tracing::warn!(
                        provider = attempt_name,
                        error = %e,
                        attempt = i,
                        "upstream error; trying next fallback"
                    );
                    last_err = Some(e);
                    continue;
                }
            }
        }

        Err(last_err.unwrap_or_else(|| ProxyError::BadRequest("routing chain exhausted".into())))
    }

    /// Round-robin: rotate across pool, skip cooling-down providers,
    /// set cooldown on 429/5xx.
    /// `client_format` is the API format the client used (Anthropic or OpenAI).
    async fn forward_round_robin(
        &self,
        route: &Route,
        path: &str,
        headers: &HeaderMap,
        body: Bytes,
        streaming: bool,
        client_format: ApiFormat,
    ) -> Result<UpstreamResponse, ProxyError> {
        let pool_size = route.pool.len();
        let order = self.compute_attempt_order(&route.pool, headers, &body);

        for &idx in &order {
            let entry = &route.pool[idx];
            let attempt_name = entry.provider.name();

            // Skip if cooling down.
            if entry.is_cooling_down() {
                tracing::debug!(
                    provider = attempt_name,
                    idx,
                    "skipping cooling-down provider"
                );
                continue;
            }

            // Pre-flight quota check against the leaf provider's config name.
            if let Err(e) = self.check_quota(&entry.id) {
                tracing::debug!(provider = %entry.id, "skipping quota-exceeded provider in round-robin");
                let _ = e; // try next provider
                continue;
            }

            let direction = Direction::from_pair(client_format, entry.provider.native_format());
            let native_path = Self::translate_path(path, direction);
            let send_body = Self::translate_request(&body, direction)?;

            let raw_resp = match entry.provider.native_format() {
                ApiFormat::Anthropic => {
                    entry
                        .provider
                        .forward(native_path, headers, send_body, streaming)
                        .await
                }
                ApiFormat::OpenAI => {
                    entry
                        .provider
                        .forward_openai(native_path, headers, send_body, streaming)
                        .await
                }
            };

            match raw_resp {
                Ok(UpstreamResponse::Buffered {
                    status,
                    headers: resp_headers,
                    body: _,
                    ..
                }) if status == 429 => {
                    let cooldown_ms = extract_retry_after_ms(&resp_headers) * 1000;
                    tracing::warn!(
                        provider = attempt_name,
                        status,
                        cooldown_secs = cooldown_ms / 1000,
                        "upstream 429 (rate limited); cooling down"
                    );
                    entry.set_cooldown(cooldown_ms);
                    continue;
                }
                Ok(UpstreamResponse::Buffered {
                    status,
                    body: resp_body,
                    ..
                }) if status >= 500 => {
                    let preview =
                        String::from_utf8_lossy(&resp_body[..resp_body.len().min(200)]).to_string();
                    tracing::warn!(
                        provider = attempt_name,
                        status,
                        body = %preview,
                        "upstream 5xx; cooling down"
                    );
                    entry.set_cooldown(DEFAULT_COOLDOWN_SECS * 1000);
                    continue;
                }
                Ok(other) => return Self::translate_upstream_response(other, direction),
                Err(e) => {
                    tracing::warn!(
                        provider = attempt_name,
                        error = %e,
                        "upstream error; cooling down"
                    );
                    entry.set_cooldown(DEFAULT_COOLDOWN_SECS * 1000);
                    continue;
                }
            }
        }

        // All providers are cooling down. Return 429 with shortest remaining cooldown.
        let min_remaining = route
            .pool
            .iter()
            .map(|e| e.remaining_cooldown_ms())
            .filter(|&ms| ms > 0)
            .min()
            .unwrap_or(DEFAULT_COOLDOWN_SECS * 1000);

        let retry_after_secs = min_remaining.div_ceil(1000);

        Err(ProxyError::UpstreamRateLimited {
            retry_after_secs,
            message: format!(
                "all {} providers in round-robin pool are rate-limited",
                pool_size
            ),
        })
    }

    /// Failover for OpenAI-format client requests.
    async fn forward_failover_openai(
        &self,
        route: &Route,
        path: &str,
        headers: &HeaderMap,
        body: Bytes,
        streaming: bool,
    ) -> Result<UpstreamResponse, ProxyError> {
        let total = route.pool.len();
        let mut last_err: Option<ProxyError> = None;

        for (i, entry) in route.pool.iter().enumerate() {
            let attempt_name = entry.provider.name();
            // Pre-flight quota check against the leaf provider's config name.
            if let Err(e) = self.check_quota(&entry.id) {
                if i + 1 == total {
                    return Err(e);
                }
                last_err = Some(e);
                continue;
            }

            let direction = Direction::from_pair(ApiFormat::OpenAI, entry.provider.native_format());
            let native_path = Self::translate_path(path, direction);
            let send_body = Self::translate_request(&body, direction)?;

            let raw_resp = match entry.provider.native_format() {
                ApiFormat::OpenAI => {
                    entry
                        .provider
                        .forward_openai(native_path, headers, send_body, streaming)
                        .await
                }
                ApiFormat::Anthropic => {
                    entry
                        .provider
                        .forward(native_path, headers, send_body, streaming)
                        .await
                }
            };

            match raw_resp {
                Ok(UpstreamResponse::Buffered {
                    status,
                    headers: resp_headers,
                    body: resp_body,
                    provider_id,
                    ..
                }) if status >= 500 => {
                    let preview =
                        String::from_utf8_lossy(&resp_body[..resp_body.len().min(200)]).to_string();
                    tracing::warn!(
                        provider = attempt_name,
                        status,
                        body = %preview,
                        attempt = i,
                        "upstream 5xx; trying next fallback"
                    );
                    if i + 1 == total {
                        let translated = Self::translate_upstream_response(
                            UpstreamResponse::Buffered {
                                status,
                                headers: resp_headers,
                                body: resp_body,
                                provider_id,
                                translation_direction: None,
                            },
                            direction,
                        )
                        .unwrap_or_else(|_| UpstreamResponse::Buffered {
                            status: 502,
                            headers: HeaderMap::new(),
                            body: Bytes::from_static(
                                b"{\"error\":{\"type\":\"translation_error\"}}",
                            ),
                            provider_id: attempt_name.to_string(),
                            translation_direction: None,
                        });
                        return Ok(translated);
                    }
                    continue;
                }
                Ok(other) => {
                    return Self::translate_upstream_response(other, direction);
                }
                Err(e) => {
                    tracing::warn!(
                        provider = attempt_name,
                        error = %e,
                        attempt = i,
                        "upstream error; trying next fallback"
                    );
                    last_err = Some(e);
                    continue;
                }
            }
        }

        Err(last_err.unwrap_or_else(|| ProxyError::BadRequest("routing chain exhausted".into())))
    }

    /// Round-robin for OpenAI-format client requests.
    async fn forward_round_robin_openai(
        &self,
        route: &Route,
        path: &str,
        headers: &HeaderMap,
        body: Bytes,
        streaming: bool,
    ) -> Result<UpstreamResponse, ProxyError> {
        let pool_size = route.pool.len();
        let order = self.compute_attempt_order(&route.pool, headers, &body);

        for &idx in &order {
            let entry = &route.pool[idx];
            let attempt_name = entry.provider.name();

            if entry.is_cooling_down() {
                tracing::debug!(
                    provider = attempt_name,
                    idx,
                    "skipping cooling-down provider"
                );
                continue;
            }

            // Pre-flight quota check against the leaf provider's config name.
            if let Err(e) = self.check_quota(&entry.id) {
                tracing::debug!(provider = %entry.id, "skipping quota-exceeded provider in round-robin");
                let _ = e; // try next provider
                continue;
            }

            let direction = Direction::from_pair(ApiFormat::OpenAI, entry.provider.native_format());
            let native_path = Self::translate_path(path, direction);
            let send_body = Self::translate_request(&body, direction)?;

            let raw_resp = match entry.provider.native_format() {
                ApiFormat::OpenAI => {
                    entry
                        .provider
                        .forward_openai(native_path, headers, send_body, streaming)
                        .await
                }
                ApiFormat::Anthropic => {
                    entry
                        .provider
                        .forward(native_path, headers, send_body, streaming)
                        .await
                }
            };

            match raw_resp {
                Ok(UpstreamResponse::Buffered {
                    status,
                    headers: resp_headers,
                    body: _,
                    ..
                }) if status == 429 => {
                    let cooldown_ms = extract_retry_after_ms(&resp_headers) * 1000;
                    tracing::warn!(
                        provider = attempt_name,
                        status,
                        cooldown_secs = cooldown_ms / 1000,
                        "upstream 429 (rate limited); cooling down"
                    );
                    entry.set_cooldown(cooldown_ms);
                    continue;
                }
                Ok(UpstreamResponse::Buffered {
                    status,
                    body: resp_body,
                    ..
                }) if status >= 500 => {
                    let preview =
                        String::from_utf8_lossy(&resp_body[..resp_body.len().min(200)]).to_string();
                    tracing::warn!(
                        provider = attempt_name,
                        status,
                        body = %preview,
                        "upstream 5xx; cooling down"
                    );
                    entry.set_cooldown(DEFAULT_COOLDOWN_SECS * 1000);
                    continue;
                }
                Ok(other) => return Self::translate_upstream_response(other, direction),
                Err(e) => {
                    tracing::warn!(
                        provider = attempt_name,
                        error = %e,
                        "upstream error; cooling down"
                    );
                    entry.set_cooldown(DEFAULT_COOLDOWN_SECS * 1000);
                    continue;
                }
            }
        }

        let min_remaining = route
            .pool
            .iter()
            .map(|e| e.remaining_cooldown_ms())
            .filter(|&ms| ms > 0)
            .min()
            .unwrap_or(DEFAULT_COOLDOWN_SECS * 1000);

        let retry_after_secs = min_remaining.div_ceil(1000);

        Err(ProxyError::UpstreamRateLimited {
            retry_after_secs,
            message: format!(
                "all {} providers in round-robin pool are rate-limited",
                pool_size
            ),
        })
    }
}

/// Extract `retry-after` header value in seconds. Defaults to DEFAULT_COOLDOWN_SECS.
fn extract_retry_after_ms(headers: &HeaderMap) -> u64 {
    headers
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_COOLDOWN_SECS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::providers::AnthropicProvider;

    fn dummy() -> Arc<dyn Provider> {
        Arc::new(AnthropicProvider::new(reqwest::Client::new()))
    }

    #[test]
    fn select_matches_glob_prefix() {
        let p = RoutingProvider::builder()
            .rule("glm-*", RoutingStrategy::Failover, dummy(), vec![])
            .unwrap()
            .rule("*", RoutingStrategy::Failover, dummy(), vec![])
            .unwrap()
            .build();
        assert_eq!(p.select("glm-4.6").unwrap().matcher.glob().glob(), "glm-*");
        assert_eq!(
            p.select("claude-3-5-sonnet").unwrap().matcher.glob().glob(),
            "*"
        );
    }

    #[test]
    fn select_first_match_wins() {
        let p = RoutingProvider::builder()
            .rule("claude-*", RoutingStrategy::Failover, dummy(), vec![])
            .unwrap()
            .rule("*", RoutingStrategy::Failover, dummy(), vec![])
            .unwrap()
            .build();
        assert_eq!(
            p.select("claude-3-5-sonnet").unwrap().matcher.glob().glob(),
            "claude-*"
        );
    }

    #[test]
    fn select_returns_none_when_no_rule_matches() {
        let p = RoutingProvider::builder()
            .rule("glm-*", RoutingStrategy::Failover, dummy(), vec![])
            .unwrap()
            .build();
        assert!(p.select("claude-x").is_none());
    }

    #[test]
    fn name_is_router() {
        let p = RoutingProvider::builder().build();
        assert_eq!(p.name(), "router");
    }

    #[test]
    fn parse_model_delegates_to_protocol() {
        let p = RoutingProvider::builder().build();
        let body = br#"{"model":"glm-4.6","messages":[]}"#;
        assert_eq!(p.parse_model(body).unwrap(), "glm-4.6");
    }

    #[test]
    fn rule_rejects_invalid_glob() {
        let res =
            RoutingProvider::builder().rule("[invalid", RoutingStrategy::Failover, dummy(), vec![]);
        assert!(res.is_err());
    }

    #[test]
    fn round_robin_pool_has_all_providers() {
        let p = RoutingProvider::builder()
            .rule(
                "*",
                RoutingStrategy::RoundRobin,
                dummy(),
                vec![dummy(), dummy()],
            )
            .unwrap()
            .build();
        let route = p.select("anything").unwrap();
        assert_eq!(route.pool.len(), 3);
        assert!(matches!(route.strategy, RoutingStrategy::RoundRobin));
    }

    #[test]
    fn pool_entry_cooldown_lifecycle() {
        let entry = PoolEntry::new(dummy(), "test-a".to_string());
        assert!(!entry.is_cooling_down());

        // Set cooldown for 5 seconds.
        entry.set_cooldown(5000);
        assert!(entry.is_cooling_down());
        assert!(entry.remaining_cooldown_ms() > 0);

        // Set cooldown in the past — should auto-expire.
        entry.cooldown_until.store(1, Ordering::Relaxed);
        assert!(!entry.is_cooling_down());
        assert_eq!(entry.remaining_cooldown_ms(), 0);
    }

    #[test]
    fn split_namespace_returns_some_for_slash_separated() {
        assert_eq!(split_namespace("zai/glm-5"), Some(("zai", "glm-5")));
    }

    #[test]
    fn split_namespace_returns_none_for_no_slash() {
        assert_eq!(split_namespace("glm-5"), None);
    }

    #[test]
    fn split_namespace_splits_on_first_slash_only() {
        assert_eq!(
            split_namespace("zai/glm-5/extra"),
            Some(("zai", "glm-5/extra"))
        );
    }

    #[test]
    fn split_namespace_empty_after_slash() {
        assert_eq!(split_namespace("zai/"), Some(("zai", "")));
    }

    #[test]
    fn rewrite_model_in_body_replaces_model_field() {
        let body = br#"{"model":"zai/glm-5","messages":[{"role":"user","content":"hi"}]}"#;
        let result = rewrite_model_in_body(body, "glm-5").unwrap();
        let v: serde_json::Value = serde_json::from_slice(&result).unwrap();
        assert_eq!(v["model"].as_str().unwrap(), "glm-5");
    }

    #[test]
    fn rewrite_model_in_body_preserves_other_fields() {
        let body = br#"{"model":"zai/glm-5","max_tokens":50,"stream":true}"#;
        let result = rewrite_model_in_body(body, "glm-5").unwrap();
        let v: serde_json::Value = serde_json::from_slice(&result).unwrap();
        assert_eq!(v["model"].as_str().unwrap(), "glm-5");
        assert_eq!(v["max_tokens"].as_u64().unwrap(), 50);
        assert!(v["stream"].as_bool().unwrap());
    }

    #[test]
    fn resolve_provider_finds_named_provider() {
        use crate::adapters::providers::ZaiProvider;

        let zai: Arc<dyn Provider> = Arc::new(ZaiProvider::new(reqwest::Client::new()));
        let anthropic: Arc<dyn Provider> = Arc::new(AnthropicProvider::new(reqwest::Client::new()));

        let mut leaves = std::collections::HashMap::new();
        leaves.insert("zai".to_string(), zai);
        leaves.insert("anthropic".to_string(), anthropic);

        let router = RoutingProvider::builder()
            .rule("*", RoutingStrategy::Failover, dummy(), vec![])
            .unwrap()
            .leaves(leaves)
            .build();

        let resolved = router.resolve_provider("zai/glm-5");
        assert!(resolved.is_some(), "namespace should resolve provider");
        let (provider, bare_model) = resolved.unwrap();
        assert_eq!(provider.name(), "zai");
        assert_eq!(bare_model, "glm-5");
    }

    #[test]
    fn resolve_provider_returns_none_for_no_namespace() {
        let router = RoutingProvider::builder().build();
        assert!(router.resolve_provider("glm-5").is_none());
    }

    #[test]
    fn resolve_provider_returns_none_for_unknown_namespace() {
        let mut leaves = std::collections::HashMap::new();
        leaves.insert("zai".to_string(), dummy());

        let router = RoutingProvider::builder().leaves(leaves).build();

        assert!(router.resolve_provider("nonexistent/glm-5").is_none());
    }

    #[tokio::test]
    async fn forward_with_unknown_namespace_returns_400() {
        let router = RoutingProvider::builder()
            .rule("*", RoutingStrategy::Failover, dummy(), vec![])
            .unwrap()
            .build();

        let body = Bytes::from_static(br#"{"model":"nonexistent/glm-5","messages":[]}"#);
        let result = router
            .forward("/v1/messages", &HeaderMap::new(), body, false)
            .await;
        match result {
            Err(ProxyError::BadRequest(msg)) => {
                assert!(
                    msg.contains("nonexistent"),
                    "error should mention the namespace, got: {msg}"
                );
            }
            _other => panic!("expected BadRequest, got success response"),
        }
    }
}
