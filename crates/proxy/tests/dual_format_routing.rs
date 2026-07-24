//! Integration tests proving capability-based dual-format routing.
//!
//! A provider configured with BOTH `anthropic_base_url` and `openai_base_url`
//! serves each client format via passthrough (no translation) — one leaf,
//! two independent native upstreams. A provider configured with only ONE of
//! the two base URLs forces translation for the mismatched client format.
//!
//! These mirror the harness in `translation.rs` (same server-spawn / config
//! helpers) but exercise the routing decision (`RoutingProvider::select_direction`)
//! rather than the translation functions themselves.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use chrono::NaiveDate;
use proxy::adapters::providers::RoutingProvider;
use proxy::adapters::providers::upstream::{Quirks, UpstreamProvider};
use proxy::adapters::storage::{SqliteRequestLogRepository, ensure_current};
use proxy::application::ports::{Provider, RequestLogPort, RequestLogReadPort};
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

fn dummy_admin_state(repo: Arc<SqliteRequestLogRepository>) -> proxy::frameworks::AdminState {
    use proxy::adapters::oauth::OAuthSessionStore;
    use proxy::adapters::providers::LiveProvider;
    use proxy::adapters::storage::db_config::DbConfigRepository;
    use proxy::application::ports::{ConfigRepository, Provider, RequestLogReadPort};
    use proxy::application::use_cases::{
        CompleteAnthropicOAuth, CompleteOpenAiOAuth, GetConfig, GetQuotaStatus, GetRecentRequests,
        GetStatus, GetUsageSummary, StartAnthropicOAuth, StartOpenAiOAuth, TestProvider,
        UpdateConfig,
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
    let stub_provider: Arc<dyn Provider> = Arc::new(UpstreamProvider::new(
        "anthropic".to_string(),
        Some("https://api.anthropic.com".to_string()),
        None,
        proxy::adapters::providers::AuthHeader::Passthrough,
        Quirks::none(),
        http.clone(),
    ));

    // In-memory DB for config storage in tests
    let config_conn = rusqlite::Connection::open_in_memory().unwrap();
    proxy::adapters::storage::ensure_current(&config_conn).unwrap();
    let config_repo: Arc<dyn ConfigRepository> =
        Arc::new(DbConfigRepository::new(config_conn, ":memory:".to_string()));

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
            config_repo.clone(),
            live.clone(),
            http.clone(),
        )),
        test_provider: Arc::new(TestProvider::new(cfg.clone(), http.clone())),
        start_oauth: Arc::new(StartAnthropicOAuth::new(oauth_sessions.clone())),
        complete_oauth: Arc::new(CompleteAnthropicOAuth::new(
            oauth_sessions,
            http.clone(),
            cfg.clone(),
            config_repo.clone(),
            live.clone(),
        )),
        start_openai_oauth: {
            use proxy::adapters::oauth::openai::OAuthSessionStore as OpenAiSessionStore;
            Arc::new(StartOpenAiOAuth::new(Arc::new(OpenAiSessionStore::new())))
        },
        complete_openai_oauth: {
            use proxy::adapters::oauth::openai::OAuthSessionStore as OpenAiSessionStore;
            Arc::new(CompleteOpenAiOAuth::new(
                Arc::new(OpenAiSessionStore::new()),
                http,
                cfg,
                config_repo,
                live,
            ))
        },
        usage_summary: Arc::new(GetUsageSummary::new(read.clone())),
        quota_status: Arc::new(GetQuotaStatus::new(Arc::new(
            proxy::adapters::quota::InMemoryQuota::new(vec![]),
        ))),
        account_usage: Arc::new(proxy::application::use_cases::admin::GetAccountUsage::new(
            Arc::new(std::collections::HashMap::new()),
            read.clone(),
        )),
    }
}

/// Start a proxy with a single routing rule pointing to the given leaf
/// provider, returning the repo so callers can inspect persisted rows
/// (in particular `translation_direction`, which is `None` for passthrough
/// and `Some("anthropic→openai" | "openai→anthropic")` for translated
/// requests).
async fn start_dual_format_proxy(
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

// ── Test 1: dual-format provider — both clients passthrough ────────────────
//
// One leaf provider is configured with BOTH `anthropic_base_url` and
// `openai_base_url`, each pointing at its own wiremock server. Because the
// provider supports the client's native format in both cases,
// `RoutingProvider::select_direction` picks `Direction::Passthrough` for
// each — the request body is cloned verbatim (see
// `RoutingProvider::translate_request`) and no path rewrite happens (see
// `RoutingProvider::translate_path`).
//
// The Anthropic-shaped client body carries a top-level `system` field and a
// single `messages` entry. Anthropic→OpenAI translation would hoist `system`
// into the `messages` array (see `anthropic_to_openai::request::translate`),
// so an exact byte-for-byte match of the body the mock receives against the
// body the client sent is a strong, unambiguous proof of passthrough.

#[tokio::test(flavor = "multi_thread")]
async fn dual_provider_passes_through_both_client_formats() {
    let anthropic_upstream = MockServer::start().await;
    let openai_upstream = MockServer::start().await;

    Mock::given(matchers::method("POST"))
        .and(matchers::path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "msg_dual",
            "type": "message",
            "role": "assistant",
            "content": [{"type": "text", "text": "hello from anthropic upstream"}],
            "model": "test-model",
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 5,
                "output_tokens": 3,
                "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": 0
            }
        })))
        .mount(&anthropic_upstream)
        .await;

    Mock::given(matchers::method("POST"))
        .and(matchers::path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "chatcmpl-dual",
            "object": "chat.completion",
            "created": 1_700_000_000u64,
            "model": "test-model",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "hello from openai upstream"},
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15
            }
        })))
        .mount(&openai_upstream)
        .await;

    // A single leaf with BOTH base URLs configured: FormatSupport::both().
    let leaf: Arc<dyn Provider> = Arc::new(UpstreamProvider::new(
        "dual".to_string(),
        Some(anthropic_upstream.uri()),
        Some(openai_upstream.uri()),
        proxy::adapters::providers::AuthHeader::Passthrough,
        Quirks::none(),
        reqwest::Client::new(),
    ));

    let (proxy_addr, repo) = start_dual_format_proxy(leaf).await;

    // ── Anthropic-format client ─────────────────────────────────────────
    let anthropic_client_body = serde_json::json!({
        "model": "test-model",
        "max_tokens": 100,
        "system": "You are a helpful assistant.",
        "messages": [{"role": "user", "content": "Hello!"}]
    });

    let resp = reqwest::Client::new()
        .post(format!("http://{proxy_addr}/v1/messages"))
        .header("content-type", "application/json")
        .body(anthropic_client_body.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "proxy should return 200");

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["type"], "message",
        "response must stay Anthropic-shaped"
    );
    assert!(body["content"].is_array());

    // Only the Anthropic mock was hit, with the body untouched.
    let anthropic_reqs = anthropic_upstream.received_requests().await.unwrap();
    assert_eq!(
        anthropic_reqs.len(),
        1,
        "anthropic mock should receive one request"
    );
    let openai_reqs = openai_upstream.received_requests().await.unwrap();
    assert_eq!(
        openai_reqs.len(),
        0,
        "openai mock must not be hit by an anthropic-format client"
    );

    let sent_body: serde_json::Value = serde_json::from_slice(&anthropic_reqs[0].body).unwrap();
    assert_eq!(
        sent_body, anthropic_client_body,
        "passthrough must forward the client body byte-for-byte (no translation)"
    );

    // ── OpenAI-format client ────────────────────────────────────────────
    let openai_client_body = serde_json::json!({
        "model": "test-model",
        "messages": [
            {"role": "system", "content": "You are a helpful assistant."},
            {"role": "user", "content": "Hello!"}
        ]
    });

    let resp = reqwest::Client::new()
        .post(format!("http://{proxy_addr}/v1/chat/completions"))
        .header("content-type", "application/json")
        .body(openai_client_body.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "proxy should return 200");

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["object"], "chat.completion",
        "response must stay OpenAI-shaped"
    );
    assert!(body["choices"].is_array());

    // Only the OpenAI mock was hit by this second call; the Anthropic mock
    // count is unchanged.
    let openai_reqs = openai_upstream.received_requests().await.unwrap();
    assert_eq!(
        openai_reqs.len(),
        1,
        "openai mock should receive one request"
    );
    let anthropic_reqs = anthropic_upstream.received_requests().await.unwrap();
    assert_eq!(
        anthropic_reqs.len(),
        1,
        "anthropic mock must not be hit by an openai-format client"
    );

    let sent_body: serde_json::Value = serde_json::from_slice(&openai_reqs[0].body).unwrap();
    assert_eq!(
        sent_body, openai_client_body,
        "passthrough must forward the client body byte-for-byte (no translation)"
    );

    // Both requests were logged with no translation direction (passthrough).
    let rows = repo.recent(10, 0).unwrap();
    assert_eq!(rows.len(), 2, "both requests must be logged");
    assert!(
        rows.iter().all(|r| r.translation_direction.is_none()),
        "passthrough requests must not carry a translation_direction: {rows:?}"
    );
    assert!(rows.iter().all(|r| r.status == "completed"));
}

// ── Test 2: single-format (openai-only) provider — anthropic client is translated ─
//
// The leaf provider is configured with ONLY `openai_base_url`. An
// Anthropic-format client is forced through `Direction::AnthropicToOpenAI`:
// `select_direction` sees the provider doesn't support `ApiFormat::Anthropic`
// and falls back to its sole supported format (OpenAI).
//
// Proof of translation: the Anthropic client sends a top-level `system`
// field and one `messages` entry; `anthropic_to_openai::request::translate`
// rebuilds the body from scratch, folding `system` into a leading
// `{"role": "system", ...}` message and dropping the top-level `system` key
// entirely. The mock therefore must NOT see a top-level `system` field and
// must see a two-entry `messages` array — a body shape the client never
// sent. The client-visible response must stay Anthropic-shaped (translated
// back from the OpenAI-shaped upstream response) and complete (status 200,
// non-streaming).

#[tokio::test(flavor = "multi_thread")]
async fn single_openai_provider_translates_anthropic_client() {
    let openai_upstream = MockServer::start().await;

    Mock::given(matchers::method("POST"))
        .and(matchers::path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "chatcmpl-single",
            "object": "chat.completion",
            "created": 1_700_000_000u64,
            "model": "test-model",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "hello from openai-only upstream"},
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15
            }
        })))
        .mount(&openai_upstream)
        .await;

    // Leaf with ONLY the OpenAI base URL configured: FormatSupport::single(OpenAI).
    let leaf: Arc<dyn Provider> = Arc::new(UpstreamProvider::new(
        "openai-only".to_string(),
        None,
        Some(openai_upstream.uri()),
        proxy::adapters::providers::AuthHeader::Passthrough,
        Quirks::none(),
        reqwest::Client::new(),
    ));

    let (proxy_addr, repo) = start_dual_format_proxy(leaf).await;

    let anthropic_client_body = serde_json::json!({
        "model": "test-model",
        "max_tokens": 100,
        "system": "You are a helpful assistant.",
        "messages": [{"role": "user", "content": "Hello!"}]
    });

    let resp = reqwest::Client::new()
        .post(format!("http://{proxy_addr}/v1/messages"))
        .header("content-type", "application/json")
        .body(anthropic_client_body.to_string())
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(status, 200, "proxy should return 200");

    // Client-visible response is Anthropic-shaped and terminated.
    assert_eq!(
        body["type"], "message",
        "response must be translated back to Anthropic shape"
    );
    assert!(body["content"].is_array());
    assert!(
        body["stop_reason"].is_string(),
        "response must be a complete, non-streaming Anthropic message"
    );

    // The OpenAI mock received exactly one request.
    let openai_reqs = openai_upstream.received_requests().await.unwrap();
    assert_eq!(
        openai_reqs.len(),
        1,
        "openai mock should receive one request"
    );

    let sent_body: serde_json::Value = serde_json::from_slice(&openai_reqs[0].body).unwrap();

    // The body was NOT forwarded verbatim: it must not equal the client's body.
    assert_ne!(
        sent_body, anthropic_client_body,
        "translated body must differ from the verbatim client body"
    );

    // Anthropic-only top-level `system` field is gone — folded into `messages`.
    assert!(
        sent_body.get("system").is_none(),
        "translated body must not carry a top-level 'system' field"
    );

    let messages = sent_body["messages"]
        .as_array()
        .expect("translated body must have an OpenAI-shaped 'messages' array");
    assert_eq!(
        messages.len(),
        2,
        "system must be folded in as a leading message: {messages:?}"
    );
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(messages[0]["content"], "You are a helpful assistant.");
    assert_eq!(messages[1]["role"], "user");

    assert!(
        sent_body.get("choices").is_none(),
        "upstream body must not have OpenAI-response-only 'choices' key"
    );

    // The persisted row must record the translation direction.
    let rows = repo.recent(10, 0).unwrap();
    assert_eq!(rows.len(), 1, "exactly one request row");
    assert_eq!(
        rows[0].translation_direction.as_deref(),
        Some("anthropic→openai"),
        "translation_direction must be persisted for a forced translation"
    );
    assert_eq!(rows[0].status, "completed");
}
