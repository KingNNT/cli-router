//! Integration test: GET /admin/account/usage with stub adapters.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use proxy::application::errors::ProxyError;
use proxy::application::ports::{AccountUsagePort, ModelBreakdownRow, RequestLogReadPort};
use proxy::application::use_cases::admin::GetAccountUsage;
use proxy::domain::account_usage::{
    AccountUsageStatus, ModelUsageSnapshot, ProviderAccountUsage, UsageWindow,
};
use proxy_admin_api::AccountUsageResponse;
use tower::ServiceExt;

struct StubUsage {
    result: Option<Result<ProviderAccountUsage, String>>,
}

impl AccountUsagePort for StubUsage {
    fn fetch_usage(&self) -> Option<Result<ProviderAccountUsage, ProxyError>> {
        self.result.as_ref().map(|r| match r {
            Ok(usage) => Ok(usage.clone()),
            Err(msg) => Err(ProxyError::UpstreamUsage {
                provider: "stub".to_string(),
                message: msg.clone(),
            }),
        })
    }
}

struct StubRead;
impl RequestLogReadPort for StubRead {
    fn total_count(&self) -> Result<u64, ProxyError> { Ok(0) }
    fn count_by_provider(&self) -> Result<std::collections::BTreeMap<String, u64>, ProxyError> { Ok(std::collections::BTreeMap::new()) }
    fn count_by_status(&self) -> Result<std::collections::BTreeMap<String, u64>, ProxyError> { Ok(std::collections::BTreeMap::new()) }
    fn recent(&self, _: u32, _: u32) -> Result<Vec<proxy::domain::RequestRow>, ProxyError> { Ok(vec![]) }
    fn summarize(&self, _: i64, _: i64) -> Result<proxy::domain::UsageSummary, ProxyError> {
        Ok(proxy::domain::UsageSummary { from_ms: 0, to_ms: 0, daily: vec![], models: vec![] })
    }
    fn quota_seed(&self, _: i64) -> Result<Vec<proxy::application::ports::QuotaSeedRow>, ProxyError> { Ok(vec![]) }
    fn count_translations(&self) -> Result<proxy::application::ports::TranslationCounts, ProxyError> {
        Ok(proxy::application::ports::TranslationCounts::default())
    }
    fn model_breakdown(&self, _: &str, _: i64, _: i64) -> Result<Vec<ModelBreakdownRow>, ProxyError> { Ok(vec![]) }
}

async fn handler(State(uc): State<Arc<GetAccountUsage>>) -> Json<AccountUsageResponse> {
    Json(uc.execute())
}

#[tokio::test]
async fn account_usage_endpoint_returns_merged_providers() {
    let usage = ProviderAccountUsage {
        provider: "zai".to_string(),
        status: AccountUsageStatus::Available,
        plan: Some("pro".to_string()),
        windows: vec![UsageWindow {
            label: "5h Token".to_string(),
            used_pct: 40.0,
            used: Some(16_000_000),
            limit: Some(40_000_000),
            resets_at_ms: Some(1_746_300_000_000),
            sub_items: vec![],
        }],
        model_usage: Some(ModelUsageSnapshot {
            total_tokens: 12_500_000,
            total_calls: 1_234,
            period_start_ms: 1_746_220_800_000,
            period_end_ms: 1_746_292_800_000,
            model_breakdown: vec![],
        }),
    };

    let mut map = HashMap::new();
    map.insert(
        "zai".to_string(),
        Arc::new(StubUsage {
            result: Some(Ok(usage)),
        }) as Arc<dyn AccountUsagePort>,
    );
    map.insert(
        "anthropic".to_string(),
        Arc::new(StubUsage { result: None }) as Arc<dyn AccountUsagePort>,
    );

    let uc = Arc::new(GetAccountUsage::new(map, Arc::new(StubRead)));
    let app = Router::new()
        .route("/admin/account/usage", get(handler))
        .with_state(uc);

    let req = Request::builder()
        .uri("/admin/account/usage")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let parsed: AccountUsageResponse = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(parsed.providers.len(), 2);
    // Sorted alphabetically: anthropic first, zai second.
    assert_eq!(parsed.providers[0].provider, "anthropic");
    assert_eq!(
        parsed.providers[0].status,
        proxy_admin_api::ProviderUsageStatus::NotSupported
    );
    assert_eq!(parsed.providers[1].provider, "zai");
    assert_eq!(
        parsed.providers[1].status,
        proxy_admin_api::ProviderUsageStatus::Available
    );
    assert_eq!(parsed.providers[1].plan.as_deref(), Some("pro"));
    assert_eq!(parsed.providers[1].windows.len(), 1);
    assert!(parsed.providers[1].model_usage.is_some());
}

#[tokio::test]
async fn account_usage_endpoint_returns_error_provider() {
    let mut map = HashMap::new();
    map.insert(
        "broken".to_string(),
        Arc::new(StubUsage {
            result: Some(Err("timeout".to_string())),
        }) as Arc<dyn AccountUsagePort>,
    );

    let uc = Arc::new(GetAccountUsage::new(map, Arc::new(StubRead)));
    let app = Router::new()
        .route("/admin/account/usage", get(handler))
        .with_state(uc);

    let req = Request::builder()
        .uri("/admin/account/usage")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let parsed: AccountUsageResponse = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(parsed.providers.len(), 1);
    assert_eq!(
        parsed.providers[0].status,
        proxy_admin_api::ProviderUsageStatus::Error
    );
}
