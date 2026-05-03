//! Integration: sticky-affinity pins a conversation to a single upstream
//! across multiple calls; different conversations distribute across keys.

use http::HeaderMap;
use proxy::adapters::providers::affinity::{affinity_hash, pick_sticky_entry, score_for, PoolMember};

struct MockEntry {
    id: String,
    healthy: bool,
}
impl PoolMember for MockEntry {
    fn id(&self) -> &str { &self.id }
    fn healthy(&self) -> bool { self.healthy }
}

#[test]
fn same_affinity_picks_same_id_in_two_key_pool() {
    let pool: Vec<MockEntry> = vec![
        MockEntry { id: "key-a".into(), healthy: true },
        MockEntry { id: "key-b".into(), healthy: true },
    ];

    let body = br#"{"system":"long enough","messages":[{"role":"user","content":"hello world"}]}"#;
    let h1 = affinity_hash(&HeaderMap::new(), body, &[]).unwrap();
    let h2 = affinity_hash(&HeaderMap::new(), body, &[]).unwrap();

    let pick1 = pick_sticky_entry(&pool, h1).unwrap();
    let pick2 = pick_sticky_entry(&pool, h2).unwrap();
    assert_eq!(pick1.id(), pick2.id());
}

#[test]
fn distributes_across_keys_for_different_bodies() {
    let pool: Vec<MockEntry> = vec![
        MockEntry { id: "key-a".into(), healthy: true },
        MockEntry { id: "key-b".into(), healthy: true },
    ];

    let mut a_count = 0;
    let mut b_count = 0;
    for i in 0..200 {
        let body = format!(r#"{{"system":"sys-{i}","messages":[]}}"#);
        let hash = affinity_hash(&HeaderMap::new(), body.as_bytes(), &[]).unwrap();
        let p = pick_sticky_entry(&pool, hash).unwrap();
        match p.id() {
            "key-a" => a_count += 1,
            "key-b" => b_count += 1,
            _ => unreachable!(),
        }
    }
    // Expect rough balance — siphash distribution should put each in 30-70% range.
    assert!(a_count > 50, "expected key-a > 50, got {a_count}");
    assert!(b_count > 50, "expected key-b > 50, got {b_count}");
}

#[test]
fn cooldown_routes_around_sticky_then_returns() {
    // Find an affinity that picks "key-a" when both healthy.
    let healthy_pool = vec![
        MockEntry { id: "key-a".into(), healthy: true },
        MockEntry { id: "key-b".into(), healthy: true },
    ];

    let mut affinity = 0u64;
    let mut found = false;
    for cand in 0..1000u64 {
        if pick_sticky_entry(&healthy_pool, cand).unwrap().id() == "key-a" {
            affinity = cand;
            found = true;
            break;
        }
    }
    assert!(found, "should find an affinity that picks key-a");

    // Mark key-a unhealthy → fallback to key-b.
    let cooldown_pool = vec![
        MockEntry { id: "key-a".into(), healthy: false },
        MockEntry { id: "key-b".into(), healthy: true },
    ];
    assert_eq!(pick_sticky_entry(&cooldown_pool, affinity).unwrap().id(), "key-b");

    // Once key-a recovers, same affinity returns to key-a (no state to reset).
    assert_eq!(pick_sticky_entry(&healthy_pool, affinity).unwrap().id(), "key-a");
}

#[test]
fn header_priority_over_body_via_public_api() {
    let pool: Vec<MockEntry> = vec![
        MockEntry { id: "key-a".into(), healthy: true },
        MockEntry { id: "key-b".into(), healthy: true },
    ];

    let mut h = HeaderMap::new();
    h.insert("x-session-id", "stable-session".parse().unwrap());

    let names = vec!["x-session-id".to_string()];

    // Same header, two completely different bodies → same pick.
    let body1 = br#"{"system":"alpha","messages":[]}"#;
    let body2 = br#"{"system":"omega","messages":[{"role":"user","content":"different"}]}"#;
    let pick1 = pick_sticky_entry(&pool, affinity_hash(&h, body1, &names).unwrap()).unwrap();
    let pick2 = pick_sticky_entry(&pool, affinity_hash(&h, body2, &names).unwrap()).unwrap();
    assert_eq!(pick1.id(), pick2.id());
}

#[test]
fn unused_score_for_helper_is_consistent() {
    // The score_for export is what `routing.rs::compute_attempt_order` uses;
    // assert it agrees with pick_sticky_entry under the same inputs.
    let pool: Vec<MockEntry> = vec![
        MockEntry { id: "key-a".into(), healthy: true },
        MockEntry { id: "key-b".into(), healthy: true },
        MockEntry { id: "key-c".into(), healthy: true },
    ];
    let affinity = 12345u64;
    let by_pick = pick_sticky_entry(&pool, affinity).unwrap().id().to_string();
    let by_score = pool
        .iter()
        .max_by_key(|e| score_for(e.id(), affinity))
        .unwrap()
        .id()
        .to_string();
    assert_eq!(by_pick, by_score);
}
