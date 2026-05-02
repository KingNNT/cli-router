//! End-to-end proxy integration test against a wiremock upstream.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use shared::application::ports::{Clock, PricingRepository};
use shared::adapters::clock::SystemClock;
use chrono::NaiveDate;
use proxy::adapters::providers::AnthropicProvider;
use proxy::adapters::storage::{ensure_current, SqliteRequestLogRepository};
use proxy::application::ports::{Provider, RequestLogPort};
use proxy::application::use_cases::HandleMessages;
use rusqlite::Connection;
use wiremock::{matchers, Mock, MockServer, ResponseTemplate};

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
    ) -> Result<Vec<shared::domain::entities::ModelPricing>, shared::application::errors::ApplicationError> {
        Ok(Vec::new())
    }
    fn last_sync(&self) -> Result<Option<NaiveDate>, shared::application::errors::ApplicationError> {
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
    let request_log: Arc<dyn RequestLogPort> =
        Arc::new(SqliteRequestLogRepository::new(conn.clone()));
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
    let app = proxy::frameworks::build_router(use_case);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr, conn)
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
