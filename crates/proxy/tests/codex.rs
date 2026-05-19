//! Integration tests for CodexProvider — Chat Completions → Responses API translation.
//!
//! Tests verify that:
//! 1. Buffered requests: Chat Completions → Responses API → Chat Completions round-trip
//! 2. Streaming requests: Responses API SSE → Chat Completions SSE translation
//! 3. Provider wiring: CodexProvider works through the RoutingProvider

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use chrono::NaiveDate;
use proxy::adapters::providers::{AuthHeader, CodexProvider, RoutingProvider};
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

/// Start a proxy with a CodexProvider leaf pointed at the given mock upstream.
async fn start_codex_proxy(
    upstream_url: String,
) -> (SocketAddr, Arc<SqliteRequestLogRepository>) {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();
    let local_user_id: i64 = conn
        .query_row("SELECT id FROM users WHERE external_id='local'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let conn = Arc::new(Mutex::new(conn));

    let leaf: Arc<dyn Provider> = Arc::new(CodexProvider::configure(
        reqwest::Client::new(),
        Some(upstream_url),
        AuthHeader::Passthrough,
    ));

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
    let stub_provider: Arc<dyn Provider> = Arc::new(
        CodexProvider::configure(http.clone(), None, AuthHeader::Passthrough),
    );

    let config_conn = rusqlite::Connection::open_in_memory().unwrap();
    proxy::adapters::storage::ensure_current(&config_conn).unwrap();
    let config_repo: Arc<dyn ConfigRepository> = Arc::new(
        DbConfigRepository::new(config_conn, ":memory:".to_string()),
    );

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
            std::collections::HashMap::new(),
            read.clone(),
        )),
    }
}

// ── Test 1: Buffered (non-streaming) Codex round-trip ─────────────────────────
//
// Client sends Chat Completions (no stream:true) → CodexProvider always streams
// from the Codex backend → buffers the translated SSE → returns single Chat
// Completions JSON response.

#[tokio::test]
async fn codex_buffered_translates_chat_completions_to_responses_and_back() {
    let upstream = MockServer::start().await;

    // Mock the Codex Responses API endpoint with SSE.
    // The CodexProvider always sends stream:true to the backend.
    let sse_body = [
        "event: response.created\ndata: {\"type\":\"response.created\"}\n\n",
        "event: response.in_progress\ndata: {\"type\":\"response.in_progress\"}\n\n",
        "event: response.output_item.added\ndata: {\"type\":\"response.output_item.added\"}\n\n",
        "event: response.content_part.added\ndata: {\"type\":\"response.content_part.added\"}\n\n",
        "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello from Codex!\"}\n\n",
        "event: response.output_text.done\ndata: {\"type\":\"response.output_text.done\"}\n\n",
        "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-5.4\",\"usage\":{\"input_tokens\":15,\"output_tokens\":8,\"total_tokens\":23}}}\n\n",
        "event: response.done\ndata: {\"type\":\"response.done\"}\n\n",
    ].join("");

    Mock::given(matchers::method("POST"))
        .and(matchers::path("/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_body),
        )
        .mount(&upstream)
        .await;

    let (proxy_addr, _repo) = start_codex_proxy(upstream.uri()).await;

    // Send a non-streaming Chat Completions request to the proxy.
    let resp = reqwest::Client::new()
        .post(format!("http://{proxy_addr}/v1/chat/completions"))
        .header("content-type", "application/json")
        .body(
            serde_json::json!({
                "model": "gpt-5.4",
                "messages": [{"role": "user", "content": "Hello!"}],
                "max_tokens": 100
            })
            .to_string(),
        )
        .send()
        .await
        .unwrap();

    let status = resp.status();
    let text = resp.text().await.unwrap();
    let body: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|e| {
        panic!("response is not valid JSON: {e}\nraw text: {text}");
    });
    assert_eq!(status, 200, "proxy should return 200, got body: {body}");

    // Response must be Chat Completions shaped.
    assert_eq!(
        body["object"], "chat.completion",
        "response object must be 'chat.completion', got: {body}"
    );

    let choices = body["choices"].as_array().expect("choices must be an array");
    assert_eq!(choices.len(), 1, "should have exactly one choice");
    assert_eq!(choices[0]["message"]["role"], "assistant");
    assert_eq!(choices[0]["message"]["content"], "Hello from Codex!");
    assert_eq!(choices[0]["finish_reason"], "stop");

    // Usage must be translated from Responses API to Chat Completions format.
    assert_eq!(body["usage"]["prompt_tokens"], 15);
    assert_eq!(body["usage"]["completion_tokens"], 8);
    assert_eq!(body["usage"]["total_tokens"], 23);

    // Verify the upstream received a Responses API shaped request.
    let upstream_reqs = upstream.received_requests().await.unwrap();
    assert_eq!(upstream_reqs.len(), 1, "upstream should receive exactly one request");

    let sent_body: serde_json::Value = serde_json::from_slice(&upstream_reqs[0].body).unwrap();
    // Responses API format has 'input' not 'messages'.
    assert!(
        sent_body.get("input").is_some(),
        "upstream body must have 'input' key (Responses API shape)"
    );
    assert!(
        sent_body.get("instructions").is_some(),
        "upstream body must have 'instructions' (required by Codex)"
    );
    assert_eq!(sent_body["store"], false, "store must be false");
    assert_eq!(sent_body["stream"], true, "stream must always be true for Codex backend");
    // max_output_tokens must NOT be sent (Codex backend rejects it).
    assert!(
        sent_body.get("max_output_tokens").is_none(),
        "max_output_tokens must not be sent to Codex backend"
    );
    assert!(
        sent_body.get("max_tokens").is_none(),
        "max_tokens should not appear in Responses API format"
    );
}

// ── Test 2: Streaming Codex round-trip ────────────────────────────────────────
//
// Client sends Chat Completions with stream:true → CodexProvider translates
// → mock upstream returns Responses API SSE → provider translates to
// Chat Completions SSE.

#[tokio::test]
async fn codex_streaming_translates_responses_sse_to_chat_completions_sse() {
    let upstream = MockServer::start().await;

    // Mock SSE responses from the Codex Responses API.
    let sse_body = [
        "event: response.created\ndata: {\"type\":\"response.created\"}\n\n",
        "event: response.output_item.added\ndata: {\"type\":\"response.output_item.added\"}\n\n",
        "event: response.content_part.added\ndata: {\"type\":\"response.content_part.added\"}\n\n",
        "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello \"}\n\n",
        "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"from \"}\n\n",
        "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"Codex!\"}\n\n",
        "event: response.output_text.done\ndata: {\"type\":\"response.output_text.done\"}\n\n",
        "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":10,\"output_tokens\":6,\"total_tokens\":16}}}\n\n",
        "event: response.done\ndata: {\"type\":\"response.done\"}\n\n",
    ].join("");

    Mock::given(matchers::method("POST"))
        .and(matchers::path("/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_body),
        )
        .mount(&upstream)
        .await;

    let (proxy_addr, _repo) = start_codex_proxy(upstream.uri()).await;

    // Send a streaming Chat Completions request.
    let resp = reqwest::Client::new()
        .post(format!("http://{proxy_addr}/v1/chat/completions"))
        .header("content-type", "application/json")
        .body(
            serde_json::json!({
                "model": "gpt-4o",
                "messages": [{"role": "user", "content": "Hello!"}],
                "stream": true
            })
            .to_string(),
        )
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "proxy should return 200");

    let text = resp.text().await.unwrap();

    // Should contain translated text deltas as Chat Completions SSE chunks.
    assert!(
        text.contains("\"content\":\"Hello \""),
        "SSE output should contain first text delta, got: {text}"
    );
    assert!(
        text.contains("\"content\":\"from \""),
        "SSE output should contain second text delta"
    );
    assert!(
        text.contains("\"content\":\"Codex!\""),
        "SSE output should contain third text delta"
    );

    // Should contain usage from response.completed.
    assert!(
        text.contains("\"prompt_tokens\":10"),
        "SSE output should contain translated usage"
    );

    // Should end with [DONE].
    assert!(
        text.contains("[DONE]"),
        "SSE output should end with [DONE]"
    );
}

// ── Test 3: System message is extracted into instructions ─────────────────────

#[tokio::test]
async fn codex_extracts_system_message_into_instructions() {
    let upstream = MockServer::start().await;

    Mock::given(matchers::method("POST"))
        .and(matchers::path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "resp_sys",
            "object": "response",
            "model": "gpt-4o",
            "output": [
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": "ok"}]
                }
            ],
            "usage": {"input_tokens": 5, "output_tokens": 1, "total_tokens": 6}
        })))
        .mount(&upstream)
        .await;

    let (proxy_addr, _repo) = start_codex_proxy(upstream.uri()).await;

    let resp = reqwest::Client::new()
        .post(format!("http://{proxy_addr}/v1/chat/completions"))
        .header("content-type", "application/json")
        .body(
            serde_json::json!({
                "model": "gpt-4o",
                "messages": [
                    {"role": "system", "content": "You are a helpful pirate."},
                    {"role": "user", "content": "Hello!"}
                ]
            })
            .to_string(),
        )
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);

    // Verify the upstream received the system message as 'instructions'.
    let upstream_reqs = upstream.received_requests().await.unwrap();
    assert_eq!(upstream_reqs.len(), 1);
    let sent_body: serde_json::Value = serde_json::from_slice(&upstream_reqs[0].body).unwrap();
    assert_eq!(
        sent_body["instructions"], "You are a helpful pirate.",
        "system message should be extracted into 'instructions'"
    );

    // The input should only have the user message (system was extracted).
    let input = sent_body["input"].as_array().unwrap();
    assert_eq!(input.len(), 1, "only user message should remain in input");
    assert_eq!(input[0]["role"], "user");
    // Content is a plain string for the Codex backend.
    assert_eq!(input[0]["content"], "Hello!");
}

// ── Test 4: Provider basic properties ─────────────────────────────────────────

#[test]
fn codex_provider_properties() {
    let provider = CodexProvider::configure(
        reqwest::Client::new(),
        Some("http://localhost:9999".into()),
        AuthHeader::Bearer("test-key".into()),
    );
    assert_eq!(provider.name(), "codex");
    assert_eq!(provider.native_format(), proxy::application::ports::ApiFormat::OpenAI);
}
