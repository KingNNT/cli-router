//! `LiveProvider` — a `Provider` whose inner implementation can be swapped
//! at runtime. Used to hot-reload the routing table when config changes via
//! the admin API, without restarting the daemon.
//!
//! Reads (forward, parse_*) take a read lock and clone the `Arc<dyn Provider>`
//! out before doing any work, so a concurrent `swap()` doesn't block in-flight
//! requests. Writes (`swap`) are rare (admin PUT, OAuth complete) and quick.

use super::builder::{BuildError, build_from_config};
use crate::application::errors::ProxyError;
use crate::application::ports::{Provider, QuotaPort, UpstreamResponse, UsageParser};
use crate::config::Config;
use crate::domain::UsageRecord;
use async_trait::async_trait;
use axum::http::HeaderMap;
use bytes::Bytes;
use std::sync::{Arc, RwLock};

pub struct LiveProvider {
    inner: RwLock<Arc<dyn Provider>>,
    quota: Arc<dyn QuotaPort>,
}

impl LiveProvider {
    pub fn new(initial: Arc<dyn Provider>, quota: Arc<dyn QuotaPort>) -> Self {
        Self {
            inner: RwLock::new(initial),
            quota,
        }
    }

    /// Replace the active provider with `new`. Subsequent requests use it.
    pub fn swap(&self, new: Arc<dyn Provider>) {
        let mut g = self.inner.write().expect("LiveProvider rwlock poisoned");
        *g = new;
    }

    /// Build a fresh provider tree from `cfg` and atomically swap it in,
    /// reusing the same quota handle so in-memory counters survive hot-reload.
    pub fn reload(&self, cfg: &Config, http: reqwest::Client) -> Result<(), BuildError> {
        let new = build_from_config(cfg, http, self.quota.clone())?;
        self.swap(new);
        Ok(())
    }

    fn current(&self) -> Arc<dyn Provider> {
        self.inner
            .read()
            .expect("LiveProvider rwlock poisoned")
            .clone()
    }
}

#[async_trait]
impl Provider for LiveProvider {
    fn name(&self) -> &'static str {
        // Static name — same Phase 1 limitation: per-request leaf provider
        // lives in the routing table, not visible from this trait.
        "router"
    }

    fn parse_model(&self, body: &[u8]) -> Result<String, String> {
        self.current().parse_model(body)
    }

    fn usage_parser(&self) -> Box<dyn UsageParser> {
        self.current().usage_parser()
    }

    fn parse_usage_json(&self, body: &[u8]) -> Result<UsageRecord, String> {
        self.current().parse_usage_json(body)
    }

    fn usage_parser_openai(&self) -> Box<dyn UsageParser> {
        self.current().usage_parser_openai()
    }

    fn parse_usage_json_openai(&self, body: &[u8]) -> Result<UsageRecord, String> {
        self.current().parse_usage_json_openai(body)
    }

    async fn forward(
        &self,
        path: &str,
        headers: &HeaderMap,
        body: Bytes,
        streaming: bool,
    ) -> Result<UpstreamResponse, ProxyError> {
        let cur = self.current();
        cur.forward(path, headers, body, streaming).await
    }

    async fn forward_openai(
        &self,
        path: &str,
        headers: &HeaderMap,
        body: Bytes,
        streaming: bool,
    ) -> Result<UpstreamResponse, ProxyError> {
        let cur = self.current();
        cur.forward_openai(path, headers, body, streaming).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::providers::AnthropicProvider;
    use crate::adapters::quota::NoopQuota;

    fn noop_quota() -> Arc<dyn QuotaPort> {
        Arc::new(NoopQuota)
    }

    #[test]
    fn current_returns_initial_after_construction() {
        let initial: Arc<dyn Provider> = Arc::new(AnthropicProvider::new(reqwest::Client::new()));
        let live = LiveProvider::new(initial.clone(), noop_quota());
        assert_eq!(live.current().name(), "anthropic");
    }

    #[test]
    fn swap_replaces_inner() {
        let initial: Arc<dyn Provider> = Arc::new(AnthropicProvider::new(reqwest::Client::new()));
        let live = LiveProvider::new(initial, noop_quota());
        let new: Arc<dyn Provider> = Arc::new(crate::adapters::providers::ZaiProvider::new(
            reqwest::Client::new(),
        ));
        live.swap(new);
        assert_eq!(live.current().name(), "zai");
    }
}
