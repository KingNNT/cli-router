//! End-to-end proxy integration test against a wiremock upstream.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use chrono::NaiveDate;
use proxy::adapters::providers::AnthropicProvider;
use proxy::adapters::storage::{SqliteRequestLogRepository, ensure_current};
use proxy::application::ports::{Provider, RequestLogPort};
use proxy::application::use_cases::HandleMessages;
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

async fn start_proxy(upstream_url: String) -> (SocketAddr, Arc<Mutex<Connection>>) {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();
    let local_user_id: i64 = conn
        .query_row("SELECT id FROM users WHERE external_id='local'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let conn = Arc::new(Mutex::new(conn));

    let http = reqwest::Client::new();
    let provider: Arc<dyn Provider> =
        Arc::new(AnthropicProvider::with_base_url(http, upstream_url));
    let repo = Arc::new(SqliteRequestLogRepository::new(conn.clone()));
    let request_log: Arc<dyn RequestLogPort> = repo.clone();
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let pricing: Arc<dyn PricingRepository> = Arc::new(NullPricing);

    let use_case = Arc::new(HandleMessages::new(
        provider,
        request_log,
        pricing,
        clock,
        local_user_id,
    ));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = proxy::frameworks::build_router(use_case, dummy_admin_state(repo));
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr, conn)
}

fn dummy_admin_state(repo: Arc<SqliteRequestLogRepository>) -> proxy::frameworks::AdminState {
    dummy_admin_state_with_path(
        repo,
        std::env::temp_dir().join("cli-router-test-config.toml"),
    )
}

fn dummy_admin_state_with_path(
    repo: Arc<SqliteRequestLogRepository>,
    config_path: std::path::PathBuf,
) -> proxy::frameworks::AdminState {
    use proxy::adapters::oauth::OAuthSessionStore;
    use proxy::adapters::providers::{AnthropicProvider, LiveProvider};
    use proxy::application::ports::{Provider, RequestLogReadPort};
    use proxy::application::use_cases::{
        CompleteAnthropicOAuth, GetConfig, GetRecentRequests, GetStatus, GetUsageSummary,
        StartAnthropicOAuth, TestProvider, UpdateConfig,
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
    }));
    let oauth_sessions = Arc::new(OAuthSessionStore::new());
    let http = reqwest::Client::new();
    let stub_provider: Arc<dyn Provider> = Arc::new(AnthropicProvider::new(http.clone()));
    let live = Arc::new(LiveProvider::new(stub_provider));
    proxy::frameworks::AdminState {
        get_status: Arc::new(GetStatus::new(read.clone(), 0)),
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
        usage_summary: Arc::new(GetUsageSummary::new(read)),
    }
}

#[tokio::test]
async fn admin_status_returns_zero_counts_on_fresh_db() {
    let upstream = MockServer::start().await;
    let (proxy_addr, _conn) = start_proxy(upstream.uri()).await;
    let resp = reqwest::Client::new()
        .get(format!("http://{proxy_addr}/admin/status"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["total_requests"], 0);
    assert!(body["uptime_seconds"].is_number());
    assert!(body["requests_by_provider"].is_object());
    assert!(body["requests_by_status"].is_object());
}

#[tokio::test]
async fn admin_status_reflects_recorded_requests() {
    let upstream = MockServer::start().await;
    Mock::given(matchers::method("POST"))
        .and(matchers::path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id":"x","type":"message","role":"assistant","content":[],
            "model":"claude-3-5-sonnet-20241022","stop_reason":"end_turn",
            "usage":{"input_tokens":1,"output_tokens":1,
                    "cache_creation_input_tokens":0,"cache_read_input_tokens":0}
        })))
        .mount(&upstream)
        .await;
    let (proxy_addr, _conn) = start_proxy(upstream.uri()).await;

    reqwest::Client::new()
        .post(format!("http://{proxy_addr}/v1/messages"))
        .header("content-type", "application/json")
        .body(r#"{"model":"claude-3-5-sonnet-20241022","stream":false,"messages":[]}"#)
        .send()
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let body: serde_json::Value = reqwest::Client::new()
        .get(format!("http://{proxy_addr}/admin/status"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["total_requests"], 1);
    assert_eq!(body["requests_by_status"]["completed"], 1);
}

#[tokio::test]
async fn admin_recent_returns_request_rows() {
    let upstream = MockServer::start().await;
    Mock::given(matchers::method("POST"))
        .and(matchers::path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id":"x","type":"message","role":"assistant","content":[],
            "model":"claude-3-5-sonnet-20241022","stop_reason":"end_turn",
            "usage":{"input_tokens":42,"output_tokens":7,
                    "cache_creation_input_tokens":0,"cache_read_input_tokens":0}
        })))
        .mount(&upstream)
        .await;
    let (proxy_addr, _conn) = start_proxy(upstream.uri()).await;

    reqwest::Client::new()
        .post(format!("http://{proxy_addr}/v1/messages"))
        .header("content-type", "application/json")
        .body(r#"{"model":"claude-3-5-sonnet-20241022","stream":false,"messages":[]}"#)
        .send()
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let body: serde_json::Value = reqwest::Client::new()
        .get(format!(
            "http://{proxy_addr}/admin/requests/recent?limit=10"
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["model"], "claude-3-5-sonnet-20241022");
    assert_eq!(items[0]["status"], "completed");
    assert_eq!(items[0]["input_tokens"], 42);
    assert_eq!(items[0]["output_tokens"], 7);
}

#[tokio::test]
async fn admin_config_get_returns_empty_dummy_config() {
    let upstream = MockServer::start().await;
    let (proxy_addr, _conn) = start_proxy(upstream.uri()).await;
    let body: serde_json::Value = reqwest::Client::new()
        .get(format!("http://{proxy_addr}/admin/config"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["providers"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn admin_config_put_writes_file_and_replaces_in_memory() {
    use std::time::SystemTime;
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("cli-router-put-{nanos}.toml"));
    let _ = std::fs::remove_file(&path);

    // Build a proxy with a custom config-file path so the PUT roundtrips
    // there instead of clobbering a shared file.
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();
    let local_user_id: i64 = conn
        .query_row("SELECT id FROM users WHERE external_id='local'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let conn = Arc::new(Mutex::new(conn));
    let repo = Arc::new(SqliteRequestLogRepository::new(conn));
    let upstream = MockServer::start().await;

    let provider: Arc<dyn Provider> = Arc::new(AnthropicProvider::with_base_url(
        reqwest::Client::new(),
        upstream.uri(),
    ));
    let request_log: Arc<dyn RequestLogPort> = repo.clone();
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let pricing: Arc<dyn PricingRepository> = Arc::new(NullPricing);
    let use_case = Arc::new(HandleMessages::new(
        provider,
        request_log,
        pricing,
        clock,
        local_user_id,
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let admin = dummy_admin_state_with_path(repo, path.clone());
    let app = proxy::frameworks::build_router(use_case, admin);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Send a valid config payload.
    let payload = serde_json::json!({
        "port": 9090,
        "providers": [{
            "name": "anthropic",
            "kind": "anthropic",
            "auth": {"type": "api_key", "value": "sk-test"},
            "base_url": null,
        }],
        "routing": [{
            "match": {"model": "*"},
            "provider": "anthropic",
            "fallback": [],
        }],
    });
    let resp = reqwest::Client::new()
        .put(format!("http://{addr}/admin/config"))
        .header("content-type", "application/json")
        .body(payload.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // File on disk has the new config.
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(written.contains("port = 9090"));
    assert!(written.contains("sk-test"));

    // GET reflects the in-memory update.
    let body: serde_json::Value = reqwest::Client::new()
        .get(format!("http://{addr}/admin/config"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["port"], 9090);
    assert_eq!(body["providers"][0]["name"], "anthropic");

    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn admin_test_provider_returns_failure_for_unknown_provider() {
    let upstream = MockServer::start().await;
    let (proxy_addr, _conn) = start_proxy(upstream.uri()).await;
    let resp = reqwest::Client::new()
        .post(format!(
            "http://{proxy_addr}/admin/providers/nonexistent/test"
        ))
        .header("content-type", "application/json")
        .body(r#"{"model":"claude-3-5-haiku-latest"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["success"], false);
    assert!(body["error"].as_str().unwrap().contains("nonexistent"));
}

#[tokio::test]
async fn admin_config_put_hot_reloads_routing_to_new_upstream() {
    use proxy::adapters::providers::{LiveProvider, build_from_config};
    use proxy::application::ports::Provider;
    use proxy::application::use_cases::{
        CompleteAnthropicOAuth, GetConfig, GetRecentRequests, GetStatus, GetUsageSummary,
        StartAnthropicOAuth, TestProvider, UpdateConfig,
    };
    use proxy::config::{
        AuthConfig, Config, MatchSpec, ProviderConfig, ProviderKind, RoutingRule, RoutingStrategy,
    };
    use std::sync::RwLock;
    use std::time::SystemTime;

    let upstream_a = MockServer::start().await;
    Mock::given(matchers::method("POST"))
        .and(matchers::path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id":"a","type":"message","role":"assistant","content":[],
            "model":"claude-3-5-sonnet-20241022","stop_reason":"end_turn",
            "usage":{"input_tokens":1,"output_tokens":1,
                    "cache_creation_input_tokens":0,"cache_read_input_tokens":0}
        })))
        .mount(&upstream_a)
        .await;

    let upstream_b = MockServer::start().await;
    Mock::given(matchers::method("POST"))
        .and(matchers::path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id":"b","type":"message","role":"assistant","content":[],
            "model":"claude-3-5-sonnet-20241022","stop_reason":"end_turn",
            "usage":{"input_tokens":1,"output_tokens":1,
                    "cache_creation_input_tokens":0,"cache_read_input_tokens":0}
        })))
        .mount(&upstream_b)
        .await;

    // Custom proxy setup that wires the LiveProvider end-to-end.
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();
    let local_user_id: i64 = conn
        .query_row("SELECT id FROM users WHERE external_id='local'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let conn = Arc::new(Mutex::new(conn));
    let repo = Arc::new(SqliteRequestLogRepository::new(conn));

    let cfg = Config {
        port: 0,
        proxy_db: std::path::PathBuf::new(),
        pricing_db: std::path::PathBuf::new(),
        providers: vec![ProviderConfig {
            name: "anthropic".into(),
            kind: ProviderKind::Anthropic,
            auth: AuthConfig::Passthrough,
            base_url: Some(upstream_a.uri()),
            openai_base_url: None,
        }],
        routing: vec![RoutingRule {
            match_spec: MatchSpec {
                model: Some("*".into()),
            },
            provider: "anthropic".into(),
            fallback: vec![],
            strategy: RoutingStrategy::Failover,
            priority: None,
        }],
        affinity: Default::default(),
    };

    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("cli-router-reload-{nanos}.toml"));
    let _ = std::fs::remove_file(&path);

    let http = reqwest::Client::new();
    let live = Arc::new(LiveProvider::new(
        build_from_config(&cfg, http.clone()).unwrap(),
    ));
    let cfg_lock = Arc::new(RwLock::new(cfg));

    let provider: Arc<dyn Provider> = live.clone();
    let request_log: Arc<dyn RequestLogPort> = repo.clone();
    let read: Arc<dyn proxy::application::ports::RequestLogReadPort> = repo;
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let pricing: Arc<dyn PricingRepository> = Arc::new(NullPricing);
    let use_case = Arc::new(HandleMessages::new(
        provider,
        request_log,
        pricing,
        clock,
        local_user_id,
    ));
    let oauth_sessions = Arc::new(proxy::adapters::oauth::OAuthSessionStore::new());
    let admin = proxy::frameworks::AdminState {
        get_status: Arc::new(GetStatus::new(read.clone(), 0)),
        get_config: Arc::new(GetConfig::new(cfg_lock.clone())),
        get_recent: Arc::new(GetRecentRequests::new(read.clone())),
        update_config: Arc::new(UpdateConfig::new(
            cfg_lock.clone(),
            path.clone(),
            live.clone(),
            http.clone(),
        )),
        test_provider: Arc::new(TestProvider::new(cfg_lock.clone(), http.clone())),
        start_oauth: Arc::new(StartAnthropicOAuth::new(oauth_sessions.clone())),
        complete_oauth: Arc::new(CompleteAnthropicOAuth::new(
            oauth_sessions,
            http,
            cfg_lock,
            path.clone(),
            live,
        )),
        usage_summary: Arc::new(GetUsageSummary::new(read)),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = proxy::frameworks::build_router(use_case, admin);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Initial request goes to upstream A.
    reqwest::Client::new()
        .post(format!("http://{addr}/v1/messages"))
        .header("content-type", "application/json")
        .body(r#"{"model":"claude-3-5-sonnet-20241022","stream":false,"messages":[]}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(upstream_a.received_requests().await.unwrap().len(), 1);
    assert_eq!(upstream_b.received_requests().await.unwrap().len(), 0);

    // PUT new config that swings the provider's base_url to upstream B.
    let new_payload = serde_json::json!({
        "port": 8787,
        "providers": [{
            "name": "anthropic",
            "kind": "anthropic",
            "auth": {"type": "passthrough"},
            "base_url": upstream_b.uri(),
        }],
        "routing": [{
            "match": {"model": "*"},
            "provider": "anthropic",
            "fallback": [],
        }],
    });
    let resp = reqwest::Client::new()
        .put(format!("http://{addr}/admin/config"))
        .header("content-type", "application/json")
        .body(new_payload.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // The very next request should hit upstream B without restarting.
    reqwest::Client::new()
        .post(format!("http://{addr}/v1/messages"))
        .header("content-type", "application/json")
        .body(r#"{"model":"claude-3-5-sonnet-20241022","stream":false,"messages":[]}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(upstream_a.received_requests().await.unwrap().len(), 1);
    assert_eq!(upstream_b.received_requests().await.unwrap().len(), 1);

    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn buffered_request_forwards_and_records_usage() {
    let upstream = MockServer::start().await;
    Mock::given(matchers::method("POST"))
        .and(matchers::path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "msg_1",
            "type": "message",
            "role": "assistant",
            "content": [{"type":"text","text":"hi"}],
            "model": "claude-3-5-sonnet-20241022",
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 50, "output_tokens": 7,
                      "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}
        })))
        .mount(&upstream)
        .await;

    let (proxy_addr, conn) = start_proxy(upstream.uri()).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{proxy_addr}/v1/messages"))
        .header("content-type", "application/json")
        .body(r#"{"model":"claude-3-5-sonnet-20241022","stream":false,"messages":[]}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["id"], "msg_1");

    // Allow async write to land.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let c = conn.lock().unwrap();
    let (status, input, output): (String, i64, i64) = c
        .query_row(
            "SELECT status, input_tokens, output_tokens FROM requests LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(status, "completed");
    assert_eq!(input, 50);
    assert_eq!(output, 7);
}

#[tokio::test]
async fn streaming_request_forwards_chunks_and_records_usage() {
    let upstream = MockServer::start().await;
    let sse_body = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"x\",\"usage\":{\"input_tokens\":12,\"output_tokens\":1,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":0}}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"text\":\"hi\"}}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":34}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
    Mock::given(matchers::method("POST"))
        .and(matchers::path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_body),
        )
        .mount(&upstream)
        .await;

    let (proxy_addr, conn) = start_proxy(upstream.uri()).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{proxy_addr}/v1/messages"))
        .header("content-type", "application/json")
        .body(r#"{"model":"claude-3-5-sonnet-20241022","stream":true,"messages":[]}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let received = resp.text().await.unwrap();
    assert!(received.contains("message_stop"), "stream body forwarded");

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let c = conn.lock().unwrap();
    let (status, input, output): (String, i64, i64) = c
        .query_row(
            "SELECT status, input_tokens, output_tokens FROM requests LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(status, "completed");
    assert_eq!(input, 12);
    assert_eq!(output, 34);
}

#[tokio::test]
async fn upstream_error_is_forwarded_and_row_is_marked_errored() {
    let upstream = MockServer::start().await;
    Mock::given(matchers::method("POST"))
        .and(matchers::path("/v1/messages"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "error": {"type":"authentication_error","message":"invalid x-api-key"}
        })))
        .mount(&upstream)
        .await;

    let (proxy_addr, conn) = start_proxy(upstream.uri()).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{proxy_addr}/v1/messages"))
        .header("content-type", "application/json")
        .body(r#"{"model":"claude-3-5-sonnet-20241022","stream":false,"messages":[]}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let c = conn.lock().unwrap();
    let status: String = c
        .query_row("SELECT status FROM requests LIMIT 1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(status, "errored");
}

async fn start_routing_proxy(rules: Vec<(&'static str, String, Vec<String>)>) -> SocketAddr {
    use proxy::adapters::providers::{AuthHeader, RoutingProvider, ZaiProvider};

    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();
    let local_user_id: i64 = conn
        .query_row("SELECT id FROM users WHERE external_id='local'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let conn = Arc::new(Mutex::new(conn));
    let http = reqwest::Client::new();

    use std::collections::HashMap;
    let mut leaves: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    for (_, primary_url, fallback_urls) in &rules {
        if !leaves.contains_key(primary_url) {
            // Naming the leaf provider by its base URL keeps the helper terse.
            let leaf: Arc<dyn Provider> = Arc::new(AnthropicProvider::configure(
                http.clone(),
                Some(primary_url.clone()),
                AuthHeader::Passthrough,
            ));
            leaves.insert(primary_url.clone(), leaf);
        }
        for u in fallback_urls {
            if !leaves.contains_key(u) {
                let leaf: Arc<dyn Provider> = Arc::new(ZaiProvider::configure(
                    http.clone(),
                    Some(u.clone()),
                    None,
                    AuthHeader::Passthrough,
                ));
                leaves.insert(u.clone(), leaf);
            }
        }
    }

    let mut builder = RoutingProvider::builder();
    for (pattern, primary_url, fallback_urls) in &rules {
        let primary = leaves.get(primary_url).unwrap().clone();
        let fb: Vec<Arc<dyn Provider>> = fallback_urls
            .iter()
            .map(|u| leaves.get(u).unwrap().clone())
            .collect();
        builder = builder
            .rule(
                pattern,
                proxy::config::RoutingStrategy::Failover,
                primary,
                fb,
            )
            .unwrap();
    }
    let provider: Arc<dyn Provider> = Arc::new(builder.build());

    let repo = Arc::new(SqliteRequestLogRepository::new(conn.clone()));
    let request_log: Arc<dyn RequestLogPort> = repo.clone();
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let pricing: Arc<dyn PricingRepository> = Arc::new(NullPricing);

    let use_case = Arc::new(HandleMessages::new(
        provider,
        request_log,
        pricing,
        clock,
        local_user_id,
    ));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = proxy::frameworks::build_router(use_case, dummy_admin_state(repo));
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

fn ok_response_for(model: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(serde_json::json!({
        "id": "msg_x",
        "type": "message",
        "role": "assistant",
        "content": [{"type":"text","text":"hi"}],
        "model": model,
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 1, "output_tokens": 1,
                  "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}
    }))
}

#[tokio::test]
async fn routing_dispatches_glm_models_to_zai_upstream() {
    let zai = MockServer::start().await;
    let anthropic = MockServer::start().await;
    Mock::given(matchers::method("POST"))
        .and(matchers::path("/v1/messages"))
        .respond_with(ok_response_for("glm-4.6"))
        .mount(&zai)
        .await;
    Mock::given(matchers::method("POST"))
        .and(matchers::path("/v1/messages"))
        .respond_with(ok_response_for("claude-3-5-sonnet"))
        .mount(&anthropic)
        .await;

    let proxy_addr = start_routing_proxy(vec![
        ("glm-*", zai.uri(), vec![]),
        ("claude-*", anthropic.uri(), vec![]),
    ])
    .await;

    let resp = reqwest::Client::new()
        .post(format!("http://{proxy_addr}/v1/messages"))
        .header("content-type", "application/json")
        .body(r#"{"model":"glm-4.6","stream":false,"messages":[]}"#)
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["model"], "glm-4.6");
    // Confirm the zai upstream actually received the call (anthropic did not).
    assert_eq!(zai.received_requests().await.unwrap().len(), 1);
    assert_eq!(anthropic.received_requests().await.unwrap().len(), 0);
}

#[tokio::test]
async fn routing_falls_back_on_5xx_to_next_provider() {
    let primary = MockServer::start().await;
    let fallback = MockServer::start().await;
    Mock::given(matchers::method("POST"))
        .and(matchers::path("/v1/messages"))
        .respond_with(ResponseTemplate::new(503).set_body_string("upstream down"))
        .mount(&primary)
        .await;
    Mock::given(matchers::method("POST"))
        .and(matchers::path("/v1/messages"))
        .respond_with(ok_response_for("glm-4.6"))
        .mount(&fallback)
        .await;

    let proxy_addr = start_routing_proxy(vec![("*", primary.uri(), vec![fallback.uri()])]).await;

    let resp = reqwest::Client::new()
        .post(format!("http://{proxy_addr}/v1/messages"))
        .header("content-type", "application/json")
        .body(r#"{"model":"anything","stream":false,"messages":[]}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["model"], "glm-4.6");
    assert_eq!(primary.received_requests().await.unwrap().len(), 1);
    assert_eq!(fallback.received_requests().await.unwrap().len(), 1);
}
