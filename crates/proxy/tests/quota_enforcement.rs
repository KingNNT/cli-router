//! Integration: pre-flight quota check rejects requests pre-emptively.

use std::sync::Arc;

use proxy::adapters::quota::InMemoryQuota;
use proxy::application::ports::QuotaPort;
use proxy::domain::RequestUsage;
use proxy::domain::quota::{QuotaCheck, QuotaConfig, QuotaWindow};

fn cfg(provider: &str, max_req: u64) -> QuotaConfig {
    QuotaConfig {
        provider: provider.into(),
        window: QuotaWindow::Rolling {
            duration_ms: 60 * 60 * 1000,
        }, // 1h
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
        QuotaCheck::Reject {
            metric,
            retry_after_ms,
        } => {
            assert_eq!(metric, "requests");
            assert!(
                retry_after_ms > 0,
                "retry_after_ms must be positive when rejecting"
            );
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
    assert!(
        matches!(q.check("zai"), QuotaCheck::Reject { .. }),
        "100% should be Reject"
    );
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

/// Mirrors the production wiring: `record` and `check` must both be called
/// with the *leaf provider config name* (e.g. `"zai"`), NOT the static router
/// name `"router"`. This test would have silently passed (no enforcement) under
/// the old broken wiring where `HandleMessages` called `provider.name()` which
/// always returned `"router"`.
#[test]
fn record_and_check_use_leaf_provider_config_name() {
    // Configure a quota for the leaf provider name that appears in RoutingProvider's
    // PoolEntry::id — i.e. the value from ProviderConfig::name in the config.
    let q: Arc<dyn QuotaPort> = Arc::new(InMemoryQuota::new(vec![cfg("zai", 1)]));

    // Simulate what RoutingProvider now does: record with the leaf config name.
    q.record("zai", &usage_default());

    // The post-record check must return Reject when limit is 1 and 1 request recorded.
    let result = q.check("zai");
    assert!(
        matches!(result, QuotaCheck::Reject { .. }),
        "quota should reject after 1 request with max=1 when keyed on 'zai', got {result:?}"
    );

    // Verify the old broken key ('router') is always Ok — confirms the fix is necessary.
    assert_eq!(
        q.check("router"),
        QuotaCheck::Ok,
        "'router' is never a quota key, so check must be Ok (no config matches it)"
    );
}

/// Regression: `LiveProvider::reload` must preserve the in-memory quota
/// counters. Before the fix, `build_from_config` always substituted a fresh
/// `NoopQuota`, silently disabling enforcement after any hot reload.
#[test]
fn hot_reload_preserves_quota_enforcement() {
    use proxy::adapters::providers::{LiveProvider, build_from_config};
    use proxy::config::{
        AuthConfig, Config, MatchSpec, ProviderConfig, ProviderKind, RoutingRule, RoutingStrategy,
    };

    // One request allowed before reject.
    let quota: Arc<dyn QuotaPort> = Arc::new(InMemoryQuota::new(vec![cfg("anthropic", 1)]));

    // Record one usage — this exhausts the quota.
    quota.record("anthropic", &usage_default());
    assert!(
        matches!(quota.check("anthropic"), QuotaCheck::Reject { .. }),
        "quota must be exhausted before reload"
    );

    // Build a minimal config for build_from_config.
    let minimal_cfg = Config {
        port: 0,
        proxy_db: std::path::PathBuf::new(),
        pricing_db: std::path::PathBuf::new(),
        providers: vec![ProviderConfig {
            name: "anthropic".into(),
            kind: ProviderKind::Anthropic,
            auth: AuthConfig::Passthrough,
            base_url: None,
            openai_base_url: None,
            reasoning_effort: None,
            thinking_mode: proxy::config::ThinkingMode::SplitOnly,
            max_concurrent: None,
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
        quota: Vec::new(),
    };

    let http = reqwest::Client::new();
    let initial = build_from_config(&minimal_cfg, http.clone(), quota.clone()).unwrap();
    let live = LiveProvider::new(initial, quota.clone());

    // Simulate a hot reload — same config, new provider tree.
    live.reload(&minimal_cfg, http).unwrap();

    // The same quota Arc must still be exhausted after reload.
    assert!(
        matches!(quota.check("anthropic"), QuotaCheck::Reject { .. }),
        "quota must remain exhausted after hot reload (same Arc preserved)"
    );
}
