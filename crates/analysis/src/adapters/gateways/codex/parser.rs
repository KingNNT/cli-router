//! Parser for Codex CLI rollout files (`~/.codex/sessions/**/rollout-*.jsonl`).
//!
//! Codex emits one `token_count` event per API request. The event carries no
//! model or turn id, so the model is taken from the most recent `turn_context`
//! line — parsing must be sequential and stateful.

use chrono::DateTime;
use serde::Deserialize;

use shared::domain::entities::UsageRecord;
use shared::domain::value_objects::{Cost, ModelId, ProjectPath, TokenBreakdown, TokenCount};

/// One line of a rollout file. Every line has a `type`; the shape of `payload`
/// depends on it, so all payload fields are optional and shared across variants.
#[derive(Deserialize)]
struct RolloutLine {
    #[serde(rename = "type")]
    line_type: Option<String>,
    timestamp: Option<String>,
    payload: Option<Payload>,
}

#[derive(Deserialize)]
struct Payload {
    /// Present on `event_msg` lines: `"token_count"`, `"agent_message"`, …
    #[serde(rename = "type")]
    payload_type: Option<String>,
    /// `session_meta` only.
    session_id: Option<String>,
    /// `session_meta` and `turn_context`.
    cwd: Option<String>,
    /// `turn_context` only.
    model: Option<String>,
    /// `event_msg` / `token_count` only.
    info: Option<TokenInfo>,
}

#[derive(Deserialize)]
struct TokenInfo {
    total_token_usage: Option<TokenUsage>,
    last_token_usage: Option<TokenUsage>,
}

/// `PartialEq` drives duplicate detection: Codex re-emits a `token_count`
/// event without a matching request, and the re-emission repeats the previous
/// event's `total_token_usage` field for field.
#[derive(Deserialize, Clone, PartialEq)]
#[allow(dead_code)]
// Fields are compared as a unit for duplicate detection.
struct TokenUsage {
    input_tokens: Option<u64>,
    cached_input_tokens: Option<u64>,
    cache_write_input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    reasoning_output_tokens: Option<u64>,
    total_tokens: Option<u64>,
}

/// Codex reports `input_tokens` inclusive of both cache buckets, and
/// `reasoning_output_tokens` inside `output_tokens`. The domain's buckets are
/// disjoint — `TokenBreakdown::total()` sums all five — so the cache counts are
/// subtracted out and reasoning is left at zero.
fn to_breakdown(u: &TokenUsage) -> TokenBreakdown {
    let cached = u.cached_input_tokens.unwrap_or(0);
    let written = u.cache_write_input_tokens.unwrap_or(0);
    let fresh = u
        .input_tokens
        .unwrap_or(0)
        .saturating_sub(cached)
        .saturating_sub(written);
    TokenBreakdown {
        input: TokenCount::new(fresh),
        output: TokenCount::new(u.output_tokens.unwrap_or(0)),
        reasoning: TokenCount::default(),
        cache_read: TokenCount::new(cached),
        cache_write: TokenCount::new(written),
    }
}

/// Parse one rollout file's lines, appending a `UsageRecord` per API request.
///
/// Tolerant by design: a malformed line, an unparseable timestamp, or an
/// invalid model/project is skipped on its own so one bad line cannot hide the
/// rest of the session.
pub fn parse_rollout(lines: impl Iterator<Item = String>, out: &mut Vec<UsageRecord>) {
    let mut session_id = String::new();
    let mut session_cwd: Option<String> = None;
    let mut current_model: Option<String> = None;
    let mut current_cwd: Option<String> = None;
    let mut prev_total: Option<TokenUsage> = None;

    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(evt) = serde_json::from_str::<RolloutLine>(&line) else {
            continue; // tolerate malformed lines
        };
        let Some(payload) = evt.payload else { continue };

        match evt.line_type.as_deref() {
            Some("session_meta") => {
                if let Some(id) = payload.session_id {
                    session_id = id;
                }
                if payload.cwd.is_some() {
                    session_cwd = payload.cwd;
                }
            }
            Some("turn_context") => {
                if payload.model.is_some() {
                    current_model = payload.model;
                }
                if payload.cwd.is_some() {
                    current_cwd = payload.cwd;
                }
            }
            Some("event_msg") => {
                if payload.payload_type.as_deref() != Some("token_count") {
                    continue;
                }
                let Some(info) = payload.info else { continue };

                // Re-emitted events repeat the running total verbatim.
                if let Some(total) = info.total_token_usage {
                    if prev_total.as_ref() == Some(&total) {
                        continue;
                    }
                    prev_total = Some(total);
                }

                let Some(last) = info.last_token_usage else {
                    continue;
                };
                let Some(model_str) = current_model.as_deref() else {
                    continue; // no turn_context seen yet — model unknown
                };
                let Some(cwd) = current_cwd.as_deref().or(session_cwd.as_deref()) else {
                    continue;
                };
                let Some(ts) = evt.timestamp.as_deref() else {
                    continue;
                };
                let Ok(dt) = DateTime::parse_from_rfc3339(ts) else {
                    continue;
                };
                let Ok(model) = ModelId::new(model_str) else {
                    continue;
                };
                let Ok(project) = ProjectPath::new(cwd) else {
                    continue;
                };

                out.push(UsageRecord {
                    date: dt.naive_utc().date(),
                    model,
                    project,
                    tokens: to_breakdown(&last),
                    cost: Cost::zero(),
                    session_id: session_id.clone(),
                });
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const META: &str = r#"{"timestamp":"2026-08-08T12:20:57.961Z","type":"session_meta","payload":{"session_id":"s1","cwd":"/proj","cli_version":"0.147.0"}}"#;
    const CTX: &str = r#"{"timestamp":"2026-08-08T12:20:58.074Z","type":"turn_context","payload":{"turn_id":"t1","cwd":"/proj","model":"gpt-5.6-sol"}}"#;
    const COUNT: &str = r#"{"timestamp":"2026-08-08T12:21:05.540Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":32542,"cached_input_tokens":32435,"cache_write_input_tokens":104,"output_tokens":174,"reasoning_output_tokens":18,"total_tokens":32716},"last_token_usage":{"input_tokens":32542,"cached_input_tokens":32435,"cache_write_input_tokens":104,"output_tokens":174,"reasoning_output_tokens":18,"total_tokens":32716},"model_context_window":258400}}}"#;

    fn parse(lines: &[&str]) -> Vec<UsageRecord> {
        let mut out = Vec::new();
        parse_rollout(lines.iter().map(|s| s.to_string()), &mut out);
        out
    }

    #[test]
    fn parses_one_request_into_one_record() {
        let records = parse(&[META, CTX, COUNT]);
        assert_eq!(records.len(), 1);
        let r = &records[0];
        assert_eq!(r.model.as_str(), "gpt-5.6-sol");
        assert_eq!(r.project.as_str(), "/proj");
        assert_eq!(r.session_id, "s1");
        assert_eq!(r.date.to_string(), "2026-08-08");
        assert_eq!(r.cost.value(), 0.0);
    }

    #[test]
    fn input_tokens_exclude_cached_and_written() {
        // Codex reports input_tokens inclusive of both cache buckets:
        // 32542 = 3 fresh + 32435 cached + 104 written.
        let records = parse(&[META, CTX, COUNT]);
        let t = records[0].tokens;
        assert_eq!(t.input.value(), 3);
        assert_eq!(t.cache_read.value(), 32435);
        assert_eq!(t.cache_write.value(), 104);
    }

    #[test]
    fn reasoning_stays_zero_because_it_is_inside_output() {
        let records = parse(&[META, CTX, COUNT]);
        let t = records[0].tokens;
        assert_eq!(t.output.value(), 174);
        assert_eq!(t.reasoning.value(), 0);
        // total() must not double-count: 3 + 174 + 0 + 32435 + 104
        assert_eq!(t.total().value(), 32716);
    }

    #[test]
    fn input_underflow_saturates_to_zero() {
        let odd = r#"{"timestamp":"2026-08-08T12:21:05.540Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":40,"cache_write_input_tokens":0,"output_tokens":1,"total_tokens":11},"last_token_usage":{"input_tokens":10,"cached_input_tokens":40,"cache_write_input_tokens":0,"output_tokens":1,"total_tokens":11}}}}"#;
        let records = parse(&[META, CTX, odd]);
        assert_eq!(records[0].tokens.input.value(), 0);
        assert_eq!(records[0].tokens.cache_read.value(), 40);
    }

    #[test]
    fn repeated_total_usage_is_counted_once() {
        // Same event emitted twice — identical total_token_usage.
        let records = parse(&[META, CTX, COUNT, COUNT]);
        assert_eq!(records.len(), 1);
    }

    #[test]
    fn distinct_totals_are_both_counted() {
        let second = r#"{"timestamp":"2026-08-08T12:22:05.540Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":40000,"cached_input_tokens":32435,"cache_write_input_tokens":104,"output_tokens":200,"total_tokens":40200},"last_token_usage":{"input_tokens":100,"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":26,"total_tokens":126}}}}"#;
        let records = parse(&[META, CTX, COUNT, second]);
        assert_eq!(records.len(), 2);
        assert_eq!(records[1].tokens.input.value(), 100);
        assert_eq!(records[1].tokens.output.value(), 26);
    }

    #[test]
    fn model_comes_from_the_most_recent_turn_context() {
        let ctx2 = r#"{"timestamp":"2026-08-30:00.000Z","type":"turn_context","payload":{"turn_id":"t2","cwd":"/proj","model":"gpt-5.6-terra"}}"#;
        let count2 = r#"{"timestamp":"2026-08-08T12:30:05.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":99999,"total_tokens":99999},"last_token_usage":{"input_tokens":500,"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":50,"total_tokens":550}}}}"#;
        let records = parse(&[META, CTX, COUNT, ctx2, count2]);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].model.as_str(), "gpt-5.6-sol");
        assert_eq!(records[1].model.as_str(), "gpt-5.6-terra");
        assert_eq!(records[1].tokens.input.value(), 500);
    }

    #[test]
    fn token_count_before_any_turn_context_is_skipped() {
        let records = parse(&[META, COUNT]);
        assert!(records.is_empty());
    }

    #[test]
    fn malformed_and_irrelevant_lines_are_tolerated() {
        let junk = "{not json}";
        let other_event = r#"{"timestamp":"2026-08-08T12:21:00.000Z","type":"event_msg","payload":{"type":"agent_message","message":"hi"}}"#;
        let response_item = r#"{"timestamp":"2026-08-08T12:21:01.000Z","type":"response_item","payload":{"type":"message"}}"#;
        let records = parse(&[META, junk, CTX, other_event, response_item, COUNT, ""]);
        assert_eq!(records.len(), 1);
    }

    #[test]
    fn project_falls_back_to_session_meta_cwd() {
        let ctx_no_cwd = r#"{"timestamp":"2026-08-08T12:20:58.074Z","type":"turn_context","payload":{"turn_id":"t1","model":"gpt-5.6-sol"}}"#;
        let records = parse(&[META, ctx_no_cwd, COUNT]);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].project.as_str(), "/proj");
    }
}
