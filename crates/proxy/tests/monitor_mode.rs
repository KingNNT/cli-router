//! Routing behavior for providers parked in `ProviderMode::Monitor`: their
//! account usage keeps being polled, but no request may reach them.

use std::sync::Arc;

use bytes::Bytes;
use http::HeaderMap;
use proxy::adapters::providers::build_from_config;
use proxy::adapters::quota::InMemoryQuota;
use proxy::application::ports::QuotaPort;
use proxy::config::{
    AuthConfig, Config, MatchSpec, ProviderConfig, ProviderKind, ProviderMode, RoutingRule,
    RoutingStrategy,
};
use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

async fn upstream(id: &str) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(matchers::method("POST"))
        .and(matchers::path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": id, "type": "message", "role": "assistant", "content": [],
            "model": "claude-3-5-sonnet-20241022", "stop_reason": "end_turn",
            "usage": {"input_tokens": 1, "output_tokens": 1,
                      "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}
        })))
        .mount(&server)
        .await;
    server
}

fn provider(name: &str, url: String, mode: ProviderMode) -> ProviderConfig {
    ProviderConfig {
        name: name.into(),
        mode,
        kind: ProviderKind::Anthropic,
        auth: AuthConfig::Passthrough,
        anthropic_base_url: Some(url),
        openai_base_url: None,
        format_mode: proxy::config::FormatMode::Both,
        thinking_level: proxy::config::ThinkingLevel::Unset,
        thinking_force: false,
        thinking_mode: proxy::config::ThinkingMode::SplitOnly,
        max_concurrent: None,
        sanitize_empty_tools: false,
        model_formats: None,
    }
}

fn rule(model: &str, primary: &str, fallback: Vec<String>) -> RoutingRule {
    RoutingRule {
        match_spec: MatchSpec {
            model: Some(model.into()),
        },
        provider: primary.into(),
        fallback,
        strategy: RoutingStrategy::Failover,
        priority: None,
    }
}

fn config(providers: Vec<ProviderConfig>, routing: Vec<RoutingRule>) -> Config {
    Config {
        port: 0,
        proxy_db: std::path::PathBuf::new(),
        pricing_db: std::path::PathBuf::new(),
        providers,
        routing,
        affinity: Default::default(),
        quota: Vec::new(),
    }
}

async fn forward(cfg: &Config, model: &str) -> Result<(), String> {
    let quota: Arc<dyn QuotaPort> = Arc::new(InMemoryQuota::new(vec![]));
    let router = build_from_config(cfg, reqwest::Client::new(), quota).map_err(|e| e.to_string())?;
    let body = Bytes::from(format!(r#"{{"model":"{model}","messages":[]}}"#));
    router
        .forward("/v1/messages", &HeaderMap::new(), body, false)
        .await
        .map(|_| ())
        .map_err(|e| format!("{e:?}"))
}

#[tokio::test]
async fn monitor_primary_hands_the_request_to_its_fallback() {
    let parked = upstream("parked").await;
    let live = upstream("live").await;
    let cfg = config(
        vec![
            provider("parked", parked.uri(), ProviderMode::Monitor),
            provider("live", live.uri(), ProviderMode::Enabled),
        ],
        vec![rule("*", "parked", vec!["live".into()])],
    );

    forward(&cfg, "claude-3-5-sonnet-20241022").await.unwrap();

    assert_eq!(live.received_requests().await.unwrap().len(), 1);
    assert_eq!(
        parked.received_requests().await.unwrap().len(),
        0,
        "a parked provider must never be called"
    );
}

#[tokio::test]
async fn rule_with_no_routable_provider_falls_through_to_the_next_rule() {
    let parked = upstream("parked").await;
    let live = upstream("live").await;
    let cfg = config(
        vec![
            provider("parked", parked.uri(), ProviderMode::Monitor),
            provider("live", live.uri(), ProviderMode::Enabled),
        ],
        vec![rule("*", "parked", vec![]), rule("*", "live", vec![])],
    );

    forward(&cfg, "claude-3-5-sonnet-20241022").await.unwrap();

    assert_eq!(live.received_requests().await.unwrap().len(), 1);
    assert_eq!(parked.received_requests().await.unwrap().len(), 0);
}

#[tokio::test]
async fn namespace_override_cannot_reach_a_monitor_provider() {
    let parked = upstream("parked").await;
    let live = upstream("live").await;
    let cfg = config(
        vec![
            provider("parked", parked.uri(), ProviderMode::Monitor),
            provider("live", live.uri(), ProviderMode::Enabled),
        ],
        vec![rule("*", "live", vec![])],
    );

    let err = forward(&cfg, "parked/claude-3-5-sonnet-20241022")
        .await
        .unwrap_err();

    assert!(
        err.contains("parked"),
        "error should name the unreachable provider, got: {err}"
    );
    assert_eq!(parked.received_requests().await.unwrap().len(), 0);
}

#[tokio::test]
async fn a_rule_naming_an_unknown_provider_still_fails_the_build() {
    let live = upstream("live").await;
    let cfg = config(
        vec![provider("live", live.uri(), ProviderMode::Enabled)],
        vec![rule("*", "typo", vec![])],
    );

    let err = forward(&cfg, "claude-3-5-sonnet-20241022")
        .await
        .unwrap_err();

    assert!(
        err.contains("typo"),
        "a misspelled provider must not be silently skipped, got: {err}"
    );
}
