//! Integration tests for cross-format translation through the proxy.
//!
//! Three scenarios:
//! 1. Anthropic-format client → OpenAI-native upstream (A→O translation)
//! 2. OpenAI-format client → Anthropic-native upstream (O→A translation)
//! 3. Anthropic-format client → Anthropic-native upstream (passthrough, no translation)

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use chrono::NaiveDate;
use proxy::adapters::providers::{AnthropicProvider, RoutingProvider, ZaiProvider};
use proxy::adapters::storage::{SqliteRequestLogRepository, ensure_current};
use proxy::application::ports::{Provider, RequestLogPort};
use proxy::application::use_cases::HandleMessages;
use proxy::config::RoutingStrategy;
use rusqlite::Connection;
use shared::adapters::clock::SystemClock;
use shared::application::ports::{Clock, PricingRepository};
use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

struct NullPricing;
impl PricingRepository for NullPricing {
    fn upsert_many(
        &self,
        _: &[shared::domain::entities::ModelPricing],
    ) -> Result<usize, shared::application::errors::ApplicationError> {
        Ok(0)
    }
    fn find_many(
        &self,
        _: &[String],
    ) -> Result<
        std::collections::HashMap<String, shared::domain::entities::ModelPricing>,
        shared::application::errors::ApplicationError,
    > {
        Ok(std::collections::HashMap::new())
    }
    fn list(
        &self,
    ) -> Result<
        Vec<shared::domain::entities::ModelPricing>,
        shared::application::errors::ApplicationError,
    > {
        Ok(Vec::new())
    }
    fn last_sync(
        &self,
    ) -> Result<Option<NaiveDate>, shared::application::errors::ApplicationError> {
        Ok(None)
    }
}

/// Start a proxy with a single routing rule pointing to the given leaf provider.
async fn start_translation_proxy(leaf: Arc<dyn Provider>) -> SocketAddr {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();
    let local_user_id: i64 = conn
        .query_row("SELECT id FROM users WHERE external_id='local'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let conn = Arc::new(Mutex::new(conn));

    let provider: Arc<dyn Provider> = Arc::new(
        RoutingProvider::builder()
            .rule("*", RoutingStrategy::Failover, leaf, vec![])
            .unwrap()
            .build(),
    );

    let repo = Arc::new(SqliteRequestLogRepository::new(conn));
    let request_log: Arc<dyn RequestLogPort> = repo.clone();
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let pricing: Arc<dyn PricingRepository> = Arc::new(NullPricing);

    let use_case = Arc::new(HandleMessages::new(
        provider,
        request_log,
        pricing,
        clock,
        local_user_id,
        Arc::new(proxy::adapters::quota::InMemoryQuota::new(vec![])),
    ));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    // Build a minimal admin state with the same repo.
    let admin = dummy_admin_state(repo);
    let app = proxy::frameworks::build_router(use_case, admin);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

fn dummy_admin_state(repo: Arc<SqliteRequestLogRepository>) -> proxy::frameworks::AdminState {
    use proxy::adapters::oauth::OAuthSessionStore;
    use proxy::adapters::providers::LiveProvider;
    use proxy::application::ports::{Provider, RequestLogReadPort};
    use proxy::application::use_cases::{
        CompleteAnthropicOAuth, GetConfig, GetQuotaStatus, GetRecentRequests, GetStatus,
        GetUsageSummary, StartAnthropicOAuth, TestProvider, UpdateConfig,
    };
    use proxy::config::Config;
    use std::path::PathBuf;
    use std::sync::RwLock;

    let read: Arc<dyn RequestLogReadPort> = repo;
    let cfg = Arc::new(RwLock::new(Config {
        port: 0,
        proxy_db: PathBuf::new(),
        pricing_db: PathBuf::new(),
        providers: vec![],
        routing: vec![],
        affinity: Default::default(),
        quota: Vec::new(),
    }));
    let oauth_sessions = Arc::new(OAuthSessionStore::new());
    let http = reqwest::Client::new();
    let stub_provider: Arc<dyn Provider> = Arc::new(AnthropicProvider::new(http.clone()));
    let config_path = std::env::temp_dir().join("cli-router-translation-test-config.toml");
    let live = Arc::new(LiveProvider::new(
        stub_provider,
        Arc::new(proxy::adapters::quota::NoopQuota),
    ));
    proxy::frameworks::AdminState {
        get_status: Arc::new(GetStatus::new(read.clone(), 0, cfg.clone())),
        get_config: Arc::new(GetConfig::new(cfg.clone())),
        get_recent: Arc::new(GetRecentRequests::new(read.clone())),
        update_config: Arc::new(UpdateConfig::new(
            cfg.clone(),
            config_path.clone(),
            live.clone(),
            http.clone(),
        )),
        test_provider: Arc::new(TestProvider::new(cfg.clone(), http.clone())),
        start_oauth: Arc::new(StartAnthropicOAuth::new(oauth_sessions.clone())),
        complete_oauth: Arc::new(CompleteAnthropicOAuth::new(
            oauth_sessions,
            http,
            cfg,
            config_path,
            live,
        )),
        usage_summary: Arc::new(GetUsageSummary::new(read.clone())),
        quota_status: Arc::new(GetQuotaStatus::new(Arc::new(
            proxy::adapters::quota::InMemoryQuota::new(vec![]),
        ))),
        account_usage: Arc::new(proxy::application::use_cases::admin::GetAccountUsage::new(
            std::collections::HashMap::new(),
        )),
    }
}

/// Like `start_translation_proxy` but also returns the repo so callers can
/// inspect persisted rows.
async fn start_translation_proxy_with_repo(
    leaf: Arc<dyn Provider>,
) -> (SocketAddr, Arc<SqliteRequestLogRepository>) {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();
    let local_user_id: i64 = conn
        .query_row("SELECT id FROM users WHERE external_id='local'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let conn = Arc::new(Mutex::new(conn));

    let provider: Arc<dyn Provider> = Arc::new(
        RoutingProvider::builder()
            .rule("*", RoutingStrategy::Failover, leaf, vec![])
            .unwrap()
            .build(),
    );

    let repo = Arc::new(SqliteRequestLogRepository::new(conn));
    let request_log: Arc<dyn RequestLogPort> = repo.clone();
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let pricing: Arc<dyn PricingRepository> = Arc::new(NullPricing);

    let use_case = Arc::new(HandleMessages::new(
        provider,
        request_log,
        pricing,
        clock,
        local_user_id,
        Arc::new(proxy::adapters::quota::InMemoryQuota::new(vec![])),
    ));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let admin = dummy_admin_state(repo.clone());
    let app = proxy::frameworks::build_router(use_case, admin);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr, repo)
}

// ── Test 1: Anthropic client → OpenAI upstream ─────────────────────────────────
//
// Client sends POST /v1/messages (Anthropic format).
// Upstream is ZaiProvider (OpenAI-native) — receives /v1/chat/completions.
// Proxy translates request body A→O and response body O→A.

#[tokio::test]
async fn anthropic_client_to_openai_upstream_translates_request() {
    // OpenAI-format mock upstream: expects chat/completions, returns OpenAI response.
    let upstream = MockServer::start().await;
    // The routing provider calls forward_openai(path="/v1/messages", ...) where path is
    // the original Anthropic path. ZaiProvider appends it to openai_base_url.
    Mock::given(matchers::method("POST"))
        .and(matchers::path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "chatcmpl-test",
            "object": "chat.completion",
            "created": 1_700_000_000u64,
            "model": "test-model",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "Hello from OpenAI upstream!"},
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15
            }
        })))
        .mount(&upstream)
        .await;

    // ZaiProvider is OpenAI-native: forward_openai hits {openai_base_url}/v1/chat/completions.
    let leaf: Arc<dyn Provider> = Arc::new(ZaiProvider::configure(
        reqwest::Client::new(),
        None,
        Some(upstream.uri()),
        proxy::adapters::providers::AuthHeader::Passthrough,
    ));

    let proxy_addr = start_translation_proxy(leaf).await;

    // Send Anthropic-format request to proxy.
    let resp = reqwest::Client::new()
        .post(format!("http://{proxy_addr}/v1/messages"))
        .header("content-type", "application/json")
        .body(
            serde_json::json!({
                "model": "test-model",
                "max_tokens": 100,
                "messages": [{"role": "user", "content": "Hello!"}]
            })
            .to_string(),
        )
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "proxy should return 200");

    let body: serde_json::Value = resp.json().await.unwrap();

    // Response must be Anthropic-shaped: "type": "message", "content": [...]
    assert_eq!(
        body["type"], "message",
        "response type must be 'message' (Anthropic shape)"
    );
    assert!(body["content"].is_array(), "content must be an array");

    // Verify upstream received exactly one request.
    let upstream_reqs = upstream.received_requests().await.unwrap();
    assert_eq!(
        upstream_reqs.len(),
        1,
        "upstream should receive exactly one request"
    );

    // The request body sent to the upstream must be OpenAI-shaped.
    let sent_body: serde_json::Value = serde_json::from_slice(&upstream_reqs[0].body).unwrap();
    assert!(
        sent_body.get("messages").is_some(),
        "upstream body must have 'messages' key (OpenAI shape)"
    );
    assert!(
        sent_body.get("contents").is_none(),
        "upstream body must not have Anthropic-only 'contents' key"
    );
}

// ── Test 2: OpenAI client → Anthropic upstream ─────────────────────────────────
//
// Client sends POST /v1/chat/completions (OpenAI format).
// Upstream is AnthropicProvider (Anthropic-native) — receives /v1/messages.
// Proxy translates request body O→A and response body A→O.

#[tokio::test]
async fn openai_client_to_anthropic_upstream_translates_request() {
    // Anthropic-format mock upstream.
    // HandleMessages calls forward_openai("/chat/completions", ...) for OpenAI-format clients.
    // The RoutingProvider receives this and, because the leaf is Anthropic-native, calls
    // provider.forward("/chat/completions", ...) — that path is appended to base_url verbatim.
    let upstream = MockServer::start().await;
    Mock::given(matchers::method("POST"))
        .and(matchers::path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "msg_test",
            "type": "message",
            "role": "assistant",
            "content": [{"type": "text", "text": "Hello from Anthropic upstream!"}],
            "model": "claude-3-5-sonnet-20241022",
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 5,
                "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": 0
            }
        })))
        .mount(&upstream)
        .await;

    // AnthropicProvider is Anthropic-native: forward() hits {base_url}/v1/messages.
    let leaf: Arc<dyn Provider> = Arc::new(AnthropicProvider::with_base_url(
        reqwest::Client::new(),
        upstream.uri(),
    ));

    let proxy_addr = start_translation_proxy(leaf).await;

    // Send OpenAI-format request to proxy.
    let resp = reqwest::Client::new()
        .post(format!("http://{proxy_addr}/v1/chat/completions"))
        .header("content-type", "application/json")
        .body(
            serde_json::json!({
                "model": "claude-3-5-sonnet-20241022",
                "messages": [{"role": "user", "content": "Hello!"}]
            })
            .to_string(),
        )
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(status, 200, "proxy should return 200");

    // Response must be OpenAI-shaped: "object": "chat.completion", "choices": [...]
    assert_eq!(
        body["object"], "chat.completion",
        "response object must be 'chat.completion' (OpenAI shape)"
    );
    assert!(body["choices"].is_array(), "choices must be an array");

    // Verify upstream received exactly one request.
    let upstream_reqs = upstream.received_requests().await.unwrap();
    assert_eq!(
        upstream_reqs.len(),
        1,
        "upstream should receive exactly one request"
    );

    // The request body sent to the upstream must be Anthropic-shaped.
    let sent_body: serde_json::Value = serde_json::from_slice(&upstream_reqs[0].body).unwrap();
    assert!(
        sent_body.get("messages").is_some(),
        "upstream body must have 'messages' key"
    );
    assert!(
        sent_body.get("max_tokens").is_some(),
        "upstream body must have 'max_tokens' (required by Anthropic)"
    );
    assert!(
        sent_body.get("choices").is_none(),
        "upstream body must not have OpenAI-only 'choices' key"
    );
}

// ── Test 3: Anthropic client → Anthropic upstream (passthrough, no translation) ─

#[tokio::test]
async fn passthrough_when_formats_match_no_translation() {
    let upstream = MockServer::start().await;

    let original_model = "claude-3-5-sonnet-20241022";

    // Record the request body so we can assert it was not mutated.
    Mock::given(matchers::method("POST"))
        .and(matchers::path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "msg_passthrough",
            "type": "message",
            "role": "assistant",
            "content": [{"type": "text", "text": "passthrough"}],
            "model": original_model,
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 5,
                "output_tokens": 3,
                "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": 0
            }
        })))
        .mount(&upstream)
        .await;

    // AnthropicProvider is Anthropic-native — same format as the client → passthrough.
    let leaf: Arc<dyn Provider> = Arc::new(AnthropicProvider::with_base_url(
        reqwest::Client::new(),
        upstream.uri(),
    ));

    let proxy_addr = start_translation_proxy(leaf).await;

    let client_body = serde_json::json!({
        "model": original_model,
        "max_tokens": 50,
        "messages": [{"role": "user", "content": "ping"}]
    });

    let resp = reqwest::Client::new()
        .post(format!("http://{proxy_addr}/v1/messages"))
        .header("content-type", "application/json")
        .body(client_body.to_string())
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);

    let response_body: serde_json::Value = resp.json().await.unwrap();
    // Response stays Anthropic-shaped.
    assert_eq!(response_body["type"], "message");

    // Verify what the upstream received.
    let upstream_reqs = upstream.received_requests().await.unwrap();
    assert_eq!(upstream_reqs.len(), 1);

    let sent_body: serde_json::Value = serde_json::from_slice(&upstream_reqs[0].body).unwrap();

    // Model is preserved.
    assert_eq!(sent_body["model"], original_model);
    // max_tokens preserved (Anthropic field).
    assert_eq!(sent_body["max_tokens"], 50);
}

// ── Test 4: translation_direction persists in the request log ──────────────────
//
// After an anthropic→openai translation completes, the DB row written by
// complete() must have translation_direction = 'anthropic→openai' and
// /admin/status must report translations_completed == 1.

#[tokio::test(flavor = "multi_thread")]
async fn translation_direction_persists_in_request_log() {
    use proxy::application::ports::RequestLogReadPort;

    let upstream = MockServer::start().await;
    Mock::given(matchers::method("POST"))
        .and(matchers::path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "chatcmpl-persist",
            "object": "chat.completion",
            "created": 1_700_000_000u64,
            "model": "test-model",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "ok"},
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 5,
                "completion_tokens": 3,
                "total_tokens": 8
            }
        })))
        .mount(&upstream)
        .await;

    let leaf: Arc<dyn Provider> = Arc::new(ZaiProvider::configure(
        reqwest::Client::new(),
        None,
        Some(upstream.uri()),
        proxy::adapters::providers::AuthHeader::Passthrough,
    ));

    let (proxy_addr, repo) = start_translation_proxy_with_repo(leaf).await;

    // Anthropic-format client → OpenAI-native upstream → anthropic→openai translation.
    let resp = reqwest::Client::new()
        .post(format!("http://{proxy_addr}/v1/messages"))
        .header("content-type", "application/json")
        .body(
            serde_json::json!({
                "model": "test-model",
                "max_tokens": 50,
                "messages": [{"role": "user", "content": "hello"}]
            })
            .to_string(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "proxy should return 200");

    // Verify the persisted row has translation_direction set.
    let rows = repo.recent(10).unwrap();
    assert_eq!(rows.len(), 1, "exactly one request row");
    assert_eq!(
        rows[0].translation_direction.as_deref(),
        Some("anthropic→openai"),
        "translation_direction must be persisted on the completed row"
    );
    assert_eq!(rows[0].status, "completed", "row must be completed");

    // Verify /admin/status reports the translation.
    let counts = repo.count_translations().unwrap();
    assert_eq!(counts.completed, 1, "translations_completed must be 1");
    assert_eq!(
        counts.by_direction.get("anthropic→openai"),
        Some(&1),
        "by_direction must record anthropic→openai"
    );
}
