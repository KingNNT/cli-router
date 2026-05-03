//! Integration: pre-flight quota check rejects requests pre-emptively.

use std::sync::Arc;

use proxy::adapters::quota::InMemoryQuota;
use proxy::application::ports::QuotaPort;
use proxy::domain::quota::{QuotaCheck, QuotaConfig, QuotaWindow};
use proxy::domain::RequestUsage;

fn cfg(provider: &str, max_req: u64) -> QuotaConfig {
    QuotaConfig {
        provider: provider.into(),
        window: QuotaWindow::Rolling { duration_ms: 60 * 60 * 1000 }, // 1h
        max_requests: Some(max_req),
        max_input_tokens: None,
        max_output_tokens: None,
        warn_pct: 80,
    }
}

fn usage_default() -> RequestUsage {
    RequestUsage::default()
}

#[test]
fn quota_blocks_after_max_requests() {
    let q: Arc<dyn QuotaPort> = Arc::new(InMemoryQuota::new(vec![cfg("zai", 3)]));
    for _ in 0..3 {
        assert_eq!(q.check("zai"), QuotaCheck::Ok);
        q.record("zai", &usage_default());
    }
    let result = q.check("zai");
    match result {
        QuotaCheck::Reject { metric, retry_after_ms } => {
            assert_eq!(metric, "requests");
            assert!(retry_after_ms > 0, "retry_after_ms must be positive when rejecting");
        }
        other => panic!("expected Reject, got {other:?}"),
    }
}

#[test]
fn warn_at_80_then_reject_at_100() {
    let q: Arc<dyn QuotaPort> = Arc::new(InMemoryQuota::new(vec![cfg("zai", 10)]));
    let u = usage_default();
    for _ in 0..7 {
        q.record("zai", &u);
    }
    assert_eq!(q.check("zai"), QuotaCheck::Ok, "70% should be Ok");
    q.record("zai", &u);
    let r = q.check("zai");
    match r {
        QuotaCheck::Warn { metric, used_pct } => {
            assert_eq!(metric, "requests");
            assert_eq!(used_pct, 80);
        }
        other => panic!("expected Warn at 80%, got {other:?}"),
    }
    for _ in 0..2 {
        q.record("zai", &u);
    }
    assert!(matches!(q.check("zai"), QuotaCheck::Reject { .. }), "100% should be Reject");
}

#[test]
fn unconfigured_provider_always_ok() {
    let q: Arc<dyn QuotaPort> = Arc::new(InMemoryQuota::new(vec![cfg("zai", 3)]));
    let u = usage_default();
    for _ in 0..100 {
        q.record("anthropic", &u);
    }
    assert_eq!(q.check("anthropic"), QuotaCheck::Ok);
}
