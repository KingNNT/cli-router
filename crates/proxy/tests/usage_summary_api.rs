//! Integration test: GET /admin/usage/summary against an in-memory SQLite
//! seeded with a handful of completed requests.
//!
//! We mount only the new route directly on its own `Router` with
//! `Arc<GetUsageSummary>` as state — no need to assemble the full
//! `AdminState` for a one-route test.

use std::sync::{Arc, Mutex};

use axum::body::{Body, to_bytes};
use axum::extract::{Query, State};
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use proxy::adapters::storage::{SqliteRequestLogRepository, ensure_current};
use proxy::application::errors::ProxyError;
use proxy::application::ports::RequestLogReadPort;
use proxy::application::use_cases::GetUsageSummary;
use proxy_admin_api::UsageSummaryResponse;
use rusqlite::Connection;
use serde::Deserialize;
use tower::ServiceExt;

#[derive(Deserialize)]
struct UsageSummaryQuery {
    from: i64,
    to: i64,
}

async fn handler(
    State(uc): State<Arc<GetUsageSummary>>,
    Query(q): Query<UsageSummaryQuery>,
) -> Result<Json<UsageSummaryResponse>, ProxyError> {
    Ok(Json(uc.execute(q.from, q.to)?))
}

fn seed(conn: &Connection, id: &str, started_ms: i64, model: &str, cost: f64) {
    conn.execute(
        r#"INSERT INTO requests
           (id, user_id, provider, model, status,
            started_at, finished_at,
            input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
            cost_usd)
           VALUES (?1, 1, 'anthropic', ?2, 'completed', ?3, ?3, 1000, 200, 0, 0, ?4)"#,
        rusqlite::params![id, model, started_ms, cost],
    )
    .unwrap();
}

#[tokio::test]
async fn usage_summary_endpoint_returns_aggregated_rows() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();
    seed(&conn, "r1", 1_730_000_000_000, "claude-opus-4-5", 1.50);
    seed(&conn, "r2", 1_730_000_000_000, "claude-opus-4-5", 0.50);
    seed(&conn, "r3", 1_730_000_000_000, "claude-sonnet-4-6", 0.10);

    let repo = Arc::new(SqliteRequestLogRepository::new(Arc::new(Mutex::new(conn))));
    let read: Arc<dyn RequestLogReadPort> = repo;
    let usage_summary = Arc::new(GetUsageSummary::new(read));

    let app = Router::new()
        .route("/admin/usage/summary", get(handler))
        .with_state(usage_summary);

    let req = Request::builder()
        .uri("/admin/usage/summary?from=0&to=9999999999999")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let parsed: UsageSummaryResponse = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(parsed.daily.len(), 1);
    assert_eq!(parsed.daily[0].requests, 3);
    assert_eq!(parsed.models.len(), 2);
    assert_eq!(parsed.models[0].model, "claude-opus-4-5"); // higher cost
    assert!((parsed.models[0].cost_usd - 2.00).abs() < 1e-9);
}

#[tokio::test]
async fn usage_summary_endpoint_rejects_inverted_range() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();
    let repo = Arc::new(SqliteRequestLogRepository::new(Arc::new(Mutex::new(conn))));
    let read: Arc<dyn RequestLogReadPort> = repo;
    let usage_summary = Arc::new(GetUsageSummary::new(read));

    let app = Router::new()
        .route("/admin/usage/summary", get(handler))
        .with_state(usage_summary);

    let req = Request::builder()
        .uri("/admin/usage/summary?from=200&to=100")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
