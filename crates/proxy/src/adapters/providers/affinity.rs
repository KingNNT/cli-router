//! Conversation-affinity hashing and rendezvous picking.
//!
//! [`affinity_hash`] derives a stable `u64` from a request: prefer one of the
//! configured headers, fall back to the first 1024 chars of normalized
//! system + first two messages. Returns `None` when the request has neither
//! signal (caller should fall back to round-robin).
//!
//! [`pick_sticky_entry`] uses rendezvous (highest random weight) hashing over
//! the healthy entries of a pool, so the same affinity always picks the same
//! entry while a removed/cooling-down entry only re-routes its share (1/N).

use std::hash::{Hash, Hasher};

use http::HeaderMap;
use siphasher::sip::SipHasher13;

/// Compute the affinity hash for a request.
pub fn affinity_hash(headers: &HeaderMap, body: &[u8], header_names: &[String]) -> Option<u64> {
    if let Some(h) = header_lookup(headers, header_names) {
        return Some(siphash_str(&h));
    }
    let normalized = normalize(&extract_body_signal(body)?);
    if normalized.is_empty() {
        return None;
    }
    let truncated: String = normalized.chars().take(1024).collect();
    Some(siphash_str(&truncated))
}

fn header_lookup(headers: &HeaderMap, names: &[String]) -> Option<String> {
    for name in names {
        if let Some(v) = headers.get(name)
            && let Ok(s) = v.to_str()
        {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// Extract system + first two messages text content as one string.
/// Returns `None` if the body is not parseable JSON or has no signal.
fn extract_body_signal(body: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    let mut buf = String::new();

    if let Some(sys) = v.get("system") {
        append_value_text(sys, &mut buf);
    }

    if let Some(msgs) = v.get("messages").and_then(|m| m.as_array()) {
        for msg in msgs.iter().take(2) {
            if let Some(content) = msg.get("content") {
                append_value_text(content, &mut buf);
            }
        }
    }

    if buf.is_empty() { None } else { Some(buf) }
}

/// Append text from a JSON value:
/// - string: the string itself
/// - array: each element's `.text` field if present, else recurse into the element
/// - object with `.text`: that text
///
/// Other shapes are ignored.
fn append_value_text(v: &serde_json::Value, out: &mut String) {
    match v {
        serde_json::Value::String(s) => {
            out.push(' ');
            out.push_str(s);
        }
        serde_json::Value::Array(items) => {
            for item in items {
                append_value_text(item, out);
            }
        }
        serde_json::Value::Object(map) => {
            if let Some(t) = map.get("text").and_then(|x| x.as_str()) {
                out.push(' ');
                out.push_str(t);
            }
        }
        _ => {}
    }
}

/// Lowercase + collapse whitespace runs to a single space + trim.
fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_ws = true;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !prev_ws {
                out.push(' ');
                prev_ws = true;
            }
        } else {
            for lc in ch.to_lowercase() {
                out.push(lc);
            }
            prev_ws = false;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

/// Pool member trait so the picker stays decoupled from the concrete `PoolEntry`.
pub trait PoolMember {
    fn id(&self) -> &str;
    fn healthy(&self) -> bool;
}

/// Rendezvous-hash picker over healthy pool members.
pub fn pick_sticky_entry<T: PoolMember>(pool: &[T], affinity: u64) -> Option<&T> {
    pool.iter()
        .filter(|e| e.healthy())
        .max_by_key(|e| score_for(e.id(), affinity))
}

/// Public scoring function so routing can sort indices by score.
pub fn score_for(id: &str, affinity: u64) -> u64 {
    let mut h = SipHasher13::new();
    id.hash(&mut h);
    affinity.hash(&mut h);
    h.finish()
}

fn siphash_str(s: &str) -> u64 {
    let mut h = SipHasher13::new();
    s.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_headers() -> HeaderMap {
        HeaderMap::new()
    }

    // === Body extraction + normalization (Task 3) ===

    #[test]
    fn hash_deterministic_for_same_body() {
        let body = br#"{"system":"You are helpful","messages":[{"role":"user","content":"hi"}]}"#;
        let a = affinity_hash(&no_headers(), body, &[]).unwrap();
        let b = affinity_hash(&no_headers(), body, &[]).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn hash_differs_for_different_systems() {
        let b1 = br#"{"system":"You are A","messages":[{"role":"user","content":"hi"}]}"#;
        let b2 = br#"{"system":"You are B","messages":[{"role":"user","content":"hi"}]}"#;
        assert_ne!(
            affinity_hash(&no_headers(), b1, &[]).unwrap(),
            affinity_hash(&no_headers(), b2, &[]).unwrap()
        );
    }

    #[test]
    fn hash_normalizes_whitespace_variations() {
        let b1 = br#"{"system":"hello   world","messages":[]}"#;
        let b2 = "{\"system\":\"hello\\tworld\",\"messages\":[]}".as_bytes();
        let b3 = br#"{"system":" HELLO world ","messages":[]}"#;
        let h1 = affinity_hash(&no_headers(), b1, &[]).unwrap();
        let h2 = affinity_hash(&no_headers(), b2, &[]).unwrap();
        let h3 = affinity_hash(&no_headers(), b3, &[]).unwrap();
        assert_eq!(h1, h2, "tab vs spaces should normalize");
        assert_eq!(h1, h3, "case + leading/trailing should normalize");
    }

    #[test]
    fn hash_returns_none_on_invalid_json() {
        assert!(affinity_hash(&no_headers(), b"this is not json", &[]).is_none());
    }

    #[test]
    fn hash_returns_none_on_empty_messages_and_no_system() {
        assert!(affinity_hash(&no_headers(), br#"{"messages":[]}"#, &[]).is_none());
    }

    #[test]
    fn hash_handles_anthropic_array_system() {
        let body = br#"{"system":[{"type":"text","text":"part one"},{"type":"text","text":"part two"}],"messages":[]}"#;
        let body2 = br#"{"system":[{"type":"text","text":"part one"}],"messages":[]}"#;
        assert_ne!(
            affinity_hash(&no_headers(), body, &[]).unwrap(),
            affinity_hash(&no_headers(), body2, &[]).unwrap()
        );
    }

    #[test]
    fn hash_uses_only_first_two_messages() {
        let b1 = br#"{"messages":[{"role":"user","content":"a"},{"role":"assistant","content":"b"},{"role":"user","content":"c"}]}"#;
        let b2 = br#"{"messages":[{"role":"user","content":"a"},{"role":"assistant","content":"b"},{"role":"user","content":"DIFFERENT"}]}"#;
        assert_eq!(
            affinity_hash(&no_headers(), b1, &[]).unwrap(),
            affinity_hash(&no_headers(), b2, &[]).unwrap(),
            "third message should not affect hash"
        );
    }

    #[test]
    fn hash_handles_openai_array_content_parts() {
        let body = br#"{"messages":[{"role":"system","content":[{"type":"text","text":"sys text"}]},{"role":"user","content":"hi"}]}"#;
        assert!(affinity_hash(&no_headers(), body, &[]).is_some());
    }

    // === Header priority (Task 4) ===

    #[test]
    fn hash_priority_header_over_body() {
        let mut h = HeaderMap::new();
        h.insert("x-session-id", "abc123".parse().unwrap());

        let b1 = br#"{"system":"A","messages":[]}"#;
        let b2 = br#"{"system":"B","messages":[]}"#;

        let names = vec!["x-session-id".to_string()];
        let h1 = affinity_hash(&h, b1, &names).unwrap();
        let h2 = affinity_hash(&h, b2, &names).unwrap();
        assert_eq!(h1, h2, "header should override body");
    }

    #[test]
    fn hash_skips_empty_header_value() {
        let mut h = HeaderMap::new();
        h.insert("x-session-id", "   ".parse().unwrap());

        let body = br#"{"system":"hi","messages":[]}"#;
        let names = vec!["x-session-id".to_string()];
        let with_empty = affinity_hash(&h, body, &names).unwrap();
        let body_only = affinity_hash(&HeaderMap::new(), body, &[]).unwrap();
        assert_eq!(with_empty, body_only);
    }

    #[test]
    fn hash_walks_header_list_in_order() {
        let mut h = HeaderMap::new();
        h.insert("anthropic-session-id", "from-anthropic".parse().unwrap());

        let body = br#"{"system":"hi","messages":[]}"#;
        let names = vec![
            "x-session-id".to_string(),
            "anthropic-session-id".to_string(),
        ];
        let result = affinity_hash(&h, body, &names).unwrap();

        let mut h2 = HeaderMap::new();
        h2.insert("x-session-id", "from-anthropic".parse().unwrap());
        let names2 = vec!["x-session-id".to_string()];
        let parallel = affinity_hash(&h2, b"{}", &names2).unwrap();
        assert_eq!(
            result, parallel,
            "same value through different header name yields same hash"
        );
    }

    // === Rendezvous picker (Task 5) ===

    struct MockEntry {
        id: String,
        healthy: bool,
    }
    impl PoolMember for MockEntry {
        fn id(&self) -> &str {
            &self.id
        }
        fn healthy(&self) -> bool {
            self.healthy
        }
    }
    fn entry(id: &str, healthy: bool) -> MockEntry {
        MockEntry {
            id: id.into(),
            healthy,
        }
    }

    #[test]
    fn pick_returns_none_when_pool_empty() {
        let pool: Vec<MockEntry> = vec![];
        assert!(pick_sticky_entry(&pool, 12345).is_none());
    }

    #[test]
    fn pick_returns_none_when_all_unhealthy() {
        let pool = vec![entry("a", false), entry("b", false)];
        assert!(pick_sticky_entry(&pool, 12345).is_none());
    }

    #[test]
    fn pick_is_deterministic_for_same_inputs() {
        let pool = vec![entry("a", true), entry("b", true), entry("c", true)];
        let r1 = pick_sticky_entry(&pool, 42).unwrap().id().to_string();
        let r2 = pick_sticky_entry(&pool, 42).unwrap().id().to_string();
        assert_eq!(r1, r2);
    }

    #[test]
    fn pick_skips_unhealthy_entries() {
        let healthy_pool = vec![entry("a", true), entry("b", true), entry("c", true)];
        let mut affinity = 0u64;
        let mut chose_a = String::new();
        for cand in 0..1000u64 {
            if pick_sticky_entry(&healthy_pool, cand).unwrap().id() == "a" {
                affinity = cand;
                chose_a = "a".into();
                break;
            }
        }
        assert_eq!(chose_a, "a", "should find an affinity that picks 'a'");

        let pool_a_down = vec![entry("a", false), entry("b", true), entry("c", true)];
        let alt = pick_sticky_entry(&pool_a_down, affinity)
            .unwrap()
            .id()
            .to_string();
        assert_ne!(alt, "a", "must skip unhealthy 'a'");
    }

    #[test]
    fn pick_redistributes_only_affected_when_one_entry_removed() {
        let before = vec![
            entry("a", true),
            entry("b", true),
            entry("c", true),
            entry("d", true),
        ];
        let after = vec![entry("a", true), entry("b", true), entry("c", true)];

        let mut affected = 0;
        let mut total = 0;
        for affinity in 0..200u64 {
            let pre = pick_sticky_entry(&before, affinity)
                .unwrap()
                .id()
                .to_string();
            if pre == "d" {
                continue;
            }
            total += 1;
            let post = pick_sticky_entry(&after, affinity)
                .unwrap()
                .id()
                .to_string();
            if pre != post {
                affected += 1;
            }
        }
        assert!(total > 100, "sanity: most affinities don't pick 'd'");
        assert_eq!(
            affected, 0,
            "non-'d' picks must be unchanged when 'd' is removed"
        );
    }
}
