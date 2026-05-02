//! Routing provider — selects an upstream provider per-request based on glob
//! matching on the body's `model` field. First match wins. Each rule may
//! declare a fallback chain that is tried on transport error or buffered 5xx.
//!
//! Phase 1 limitation: `name()` returns `"router"` because the trait method
//! is `&self` and not request-scoped. The actual leaf provider that handled a
//! request will be exposed through the admin API in Phase 2 — for now the
//! `provider` column in `proxy.db` will read `router` for routed requests.

use super::messages_protocol;
use crate::application::errors::ProxyError;
use crate::application::ports::{Provider, UpstreamResponse, UsageParser};
use crate::domain::UsageRecord;
use async_trait::async_trait;
use axum::http::HeaderMap;
use bytes::Bytes;
use globset::{Glob, GlobMatcher};
use std::sync::Arc;

pub struct RoutingProvider {
    rules: Vec<Route>,
}

struct Route {
    matcher: GlobMatcher,
    primary: Arc<dyn Provider>,
    fallback: Vec<Arc<dyn Provider>>,
}

impl RoutingProvider {
    pub fn builder() -> RoutingProviderBuilder {
        RoutingProviderBuilder::default()
    }

    fn select(&self, model: &str) -> Option<&Route> {
        self.rules.iter().find(|r| r.matcher.is_match(model))
    }
}

#[derive(Default)]
pub struct RoutingProviderBuilder {
    rules: Vec<Route>,
}

impl RoutingProviderBuilder {
    pub fn rule(
        mut self,
        pattern: &str,
        primary: Arc<dyn Provider>,
        fallback: Vec<Arc<dyn Provider>>,
    ) -> Result<Self, globset::Error> {
        let matcher = Glob::new(pattern)?.compile_matcher();
        self.rules.push(Route {
            matcher,
            primary,
            fallback,
        });
        Ok(self)
    }

    pub fn build(self) -> RoutingProvider {
        RoutingProvider { rules: self.rules }
    }
}

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

    async fn forward(
        &self,
        path: &str,
        headers: &HeaderMap,
        body: Bytes,
        streaming: bool,
    ) -> Result<UpstreamResponse, ProxyError> {
        let model = messages_protocol::parse_model(&body).map_err(ProxyError::BadRequest)?;
        let route = self.select(&model).ok_or_else(|| {
            ProxyError::BadRequest(format!("no routing rule matches model '{model}'"))
        })?;

        let chain: Vec<&Arc<dyn Provider>> = std::iter::once(&route.primary)
            .chain(route.fallback.iter())
            .collect();
        let total = chain.len();

        let mut last_err: Option<ProxyError> = None;
        for (i, prov) in chain.iter().enumerate() {
            let attempt_name = prov.name();
            match prov.forward(path, headers, body.clone(), streaming).await {
                Ok(UpstreamResponse::Buffered {
                    status,
                    headers: resp_headers,
                    body: resp_body,
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
                        return Ok(UpstreamResponse::Buffered {
                            status,
                            headers: resp_headers,
                            body: resp_body,
                        });
                    }
                    continue;
                }
                Ok(other) => return Ok(other),
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
            .rule("glm-*", dummy(), vec![])
            .unwrap()
            .rule("*", dummy(), vec![])
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
            .rule("claude-*", dummy(), vec![])
            .unwrap()
            .rule("*", dummy(), vec![])
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
            .rule("glm-*", dummy(), vec![])
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
        let res = RoutingProvider::builder().rule("[invalid", dummy(), vec![]);
        assert!(res.is_err());
    }
}
