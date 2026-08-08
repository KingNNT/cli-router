# Codex Usage Source Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Codex CLI as a third usage data source in the `analysis` TUI, so the dashboard can report Codex token usage and cost alongside OpenCode and Claude Code.

**Architecture:** A pure parser turns Codex rollout JSONL lines into `UsageRecord`s; a thin repository wraps it with a recursive directory walk and an in-memory cache, implementing the existing `UsageRepository` port; `DataSource` grows a third variant so `DispatchingUsageRepository` can route to it. No presentation code changes — the status bar already renders `DataSource::label()`.

**Tech Stack:** Rust edition 2024, `serde` + `serde_json` (JSONL parsing), `chrono` (RFC 3339 timestamps), `ratatui` (untouched here). Tests are `#[cfg(test)] mod tests` blocks alongside the code.

**Spec:** `docs/superpowers/specs/2026-08-08-codex-usage-source-design.md`

## Global Constraints

- All crates are **edition 2024**. Prefer let-chains (`if let Some(x) = opt && cond`) — clippy's `collapsible_if` fails the build on nested `if let`.
- **No new dependencies.** `serde`, `serde_json`, and `chrono` are already dependencies of the `analysis` crate.
- Gates that must pass before every commit: `cargo fmt`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`.
- The repo's **pre-commit hook runs the full `cargo test` suite** and takes over five minutes. Give every `git commit` a timeout of at least 600 seconds. If a commit appears to hang, check `git log --oneline -1` before retrying — it may have succeeded.
- Work on branch `feature/codex-usage-source` (already created and checked out). Never commit to `develop`.
- Commit messages follow Conventional Commits. **Never** add `Co-Authored-By`, `Generated with`, or any attribution line.
- Ring rule: `adapters/` may import from `application/` and `domain/`, never from `tui/`.

---

## File Structure

| File | Responsibility |
| --- | --- |
| `crates/analysis/src/adapters/gateways/codex/parser.rs` | **New.** Serde line types + `parse_rollout`: JSONL lines → `Vec<UsageRecord>`. Owns the token-conversion and dedup rules. No filesystem access, so it is testable from string literals. |
| `crates/analysis/src/adapters/gateways/codex/repository.rs` | **New.** `CodexUsageRepository`: recursive walk of `~/.codex/sessions`, one-shot cache, `UsageRepository` impl, filter matching. |
| `crates/analysis/src/adapters/gateways/codex/mod.rs` | **New.** Re-exports, mirroring `claudecode/mod.rs`. |
| `crates/analysis/src/adapters/gateways/mod.rs` | Declare `pub mod codex;`. |
| `crates/analysis/src/adapters/gateways/dispatching_usage_repository.rs` | Third `DataSource` variant + third dispatch arm. |
| `crates/analysis/src/main.rs` | Construct and wire `CodexUsageRepository`. |
| `crates/analysis/src/tui/controllers/tui_controller.rs` | Extend the `t`-key test to walk three states. |
| `CLAUDE.md` | Document the third source. |

The parser/repository split matters: every subtle rule in the spec (inclusive input, reasoning nesting, duplicate events, model attribution) lives in `parser.rs` and is tested without touching disk.

**Note on duplication:** `in_range` and `matches_filter` are copied from `claudecode/repository.rs` rather than extracted into a shared module. The spec's file list is deliberate — extracting them would modify a working adapter that is out of scope for this change. Leave the duplication.

---

## Task 1: Rollout parser

**Files:**
- Create: `crates/analysis/src/adapters/gateways/codex/parser.rs`
- Create: `crates/analysis/src/adapters/gateways/codex/mod.rs`
- Modify: `crates/analysis/src/adapters/gateways/mod.rs`

**Interfaces:**
- Consumes: `shared::domain::entities::UsageRecord`, `shared::domain::value_objects::{Cost, ModelId, ProjectPath, TokenBreakdown, TokenCount}`.
- Produces: `pub fn parse_rollout(lines: impl Iterator<Item = String>, out: &mut Vec<UsageRecord>)` — appends one `UsageRecord` per counted request. Task 2 calls it once per rollout file.

- [ ] **Step 1: Create the module files so the crate compiles**

Create `crates/analysis/src/adapters/gateways/codex/mod.rs`:

```rust
pub mod parser;

pub use parser::parse_rollout;
```

Add to `crates/analysis/src/adapters/gateways/mod.rs`, keeping the list alphabetical:

```rust
pub mod claudecode;
pub mod codex;
pub mod dispatching_usage_repository;
pub mod http;
pub mod sqlite;

pub use dispatching_usage_repository::{DataSource, DataSourceCell, DispatchingUsageRepository};
```

Create `crates/analysis/src/adapters/gateways/codex/parser.rs` with only this (the test module comes next):

```rust
//! Parser for Codex CLI rollout files (`~/.codex/sessions/**/rollout-*.jsonl`).
//!
//! Codex emits one `token_count` event per API request. The event carries no
//! model or turn id, so the model is taken from the most recent `turn_context`
//! line — parsing must be sequential and stateful.

use chrono::DateTime;
use serde::Deserialize;

use shared::domain::entities::UsageRecord;
use shared::domain::value_objects::{Cost, ModelId, ProjectPath, TokenBreakdown, TokenCount};

pub fn parse_rollout(_lines: impl Iterator<Item = String>, _out: &mut Vec<UsageRecord>) {
    todo!("implemented in step 4")
}
```

- [ ] **Step 2: Write the failing tests**

Append to `crates/analysis/src/adapters/gateways/codex/parser.rs`:

```rust
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
        let ctx2 = r#"{"timestamp":"2026-08-08T12:30:00.000Z","type":"turn_context","payload":{"turn_id":"t2","cwd":"/proj","model":"gpt-5.6-terra"}}"#;
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
```

- [ ] **Step 3: Run the tests to verify they fail**

```bash
cargo test -p analysis codex::parser 2>&1 | tail -20
```

Expected: the tests compile and fail — every one panics at the `todo!("implemented in step 4")` in `parse_rollout`.

- [ ] **Step 4: Implement the parser**

Replace the `parse_rollout` stub in `crates/analysis/src/adapters/gateways/codex/parser.rs` with the serde types, the conversion helper, and the real function (keep the `//!` doc comment and the `use` block from step 1 at the top of the file):

```rust
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
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p analysis codex::parser 2>&1 | tail -20
```

Expected: `test result: ok. 10 passed`.

- [ ] **Step 6: Run the lint gates**

```bash
cargo fmt && cargo clippy -p analysis --all-targets -- -D warnings 2>&1 | tail -20
```

Expected: no warnings. If clippy reports `dead_code` on `TokenUsage::reasoning_output_tokens` or `total_tokens`, add `#[allow(dead_code)]` above the struct with the comment `// Fields are compared as a unit for duplicate detection.` — per `.claude/rules/rust-style.md`, that attribute is acceptable in this repo.

- [ ] **Step 7: Commit**

```bash
git add crates/analysis/src/adapters/gateways/codex crates/analysis/src/adapters/gateways/mod.rs
git commit -m "feat(analysis): parse codex rollout jsonl into usage records"
```

Allow at least 600 seconds — the pre-commit hook runs the whole test suite.

---

## Task 2: `CodexUsageRepository`

**Files:**
- Create: `crates/analysis/src/adapters/gateways/codex/repository.rs`
- Modify: `crates/analysis/src/adapters/gateways/codex/mod.rs`

**Interfaces:**
- Consumes: `parse_rollout` from Task 1; `crate::application::ports::UsageRepository`; `crate::application::dto::Filter`.
- Produces: `pub struct CodexUsageRepository` with `pub fn new(root: PathBuf) -> Self`, and `pub fn default_sessions_root() -> PathBuf`. Task 4 constructs both.

- [ ] **Step 1: Write the failing tests**

Create `crates/analysis/src/adapters/gateways/codex/repository.rs` with the test module first, plus a stub so the file compiles:

```rust
use std::path::PathBuf;

pub fn default_sessions_root() -> PathBuf {
    todo!("implemented in step 3")
}

pub struct CodexUsageRepository;

impl CodexUsageRepository {
    pub fn new(_root: PathBuf) -> Self {
        todo!("implemented in step 3")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::dto::Filter;
    use crate::application::ports::UsageRepository;
    use chrono::NaiveDate;
    use shared::domain::value_objects::DateRange;
    use std::fs::File;
    use std::io::Write;
    use std::path::Path;

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new(name: &str) -> Self {
            let p = std::env::temp_dir().join(format!(
                "analysis-codex-test-{}-{}",
                std::process::id(),
                name
            ));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Writes a rollout file at `<root>/<y>/<m>/<d>/<name>`, creating the
    /// nested date directories Codex uses.
    fn write_rollout(root: &Path, day: &str, name: &str, lines: &[&str]) {
        let dir = root.join(day);
        std::fs::create_dir_all(&dir).unwrap();
        let mut f = File::create(dir.join(name)).unwrap();
        for line in lines {
            writeln!(f, "{}", line).unwrap();
        }
    }

    fn session(ts_day: &str, model: &str, input: u64, output: u64) -> Vec<String> {
        vec![
            format!(
                r#"{{"timestamp":"{d}T12:00:00.000Z","type":"session_meta","payload":{{"session_id":"s-{d}","cwd":"/proj"}}}}"#,
                d = ts_day
            ),
            format!(
                r#"{{"timestamp":"{d}T12:00:01.000Z","type":"turn_context","payload":{{"turn_id":"t1","cwd":"/proj","model":"{m}"}}}}"#,
                d = ts_day,
                m = model
            ),
            format!(
                r#"{{"timestamp":"{d}T12:00:02.000Z","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":{i},"total_tokens":{i}}},"last_token_usage":{{"input_tokens":{i},"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":{o},"total_tokens":{i}}}}}}}}}"#,
                d = ts_day,
                i = input,
                o = output
            ),
        ]
    }

    fn write_session(root: &Path, day_dir: &str, ts_day: &str, name: &str, model: &str, input: u64, output: u64) {
        let lines = session(ts_day, model, input, output);
        let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
        write_rollout(root, day_dir, name, &refs);
    }

    #[test]
    fn missing_root_returns_zero_overview() {
        let p = std::env::temp_dir().join(format!("analysis-codex-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        let repo = CodexUsageRepository::new(p);
        let ov = repo.overview(&Filter::default()).unwrap();
        assert_eq!(ov.message_count, 0);
    }

    #[test]
    fn empty_root_returns_zero_overview() {
        let tmp = TestRoot::new("empty");
        let repo = CodexUsageRepository::new(tmp.path().to_path_buf());
        let ov = repo.overview(&Filter::default()).unwrap();
        assert_eq!(ov.message_count, 0);
    }

    #[test]
    fn discovers_files_in_nested_date_directories() {
        let tmp = TestRoot::new("nested");
        write_session(tmp.path(), "2026/08/07", "2026-08-07", "rollout-a.jsonl", "gpt-5.6-sol", 100, 10);
        write_session(tmp.path(), "2026/08/08", "2026-08-08", "rollout-b.jsonl", "gpt-5.6-sol", 200, 20);

        let repo = CodexUsageRepository::new(tmp.path().to_path_buf());
        let ov = repo.overview(&Filter::default()).unwrap();
        assert_eq!(ov.message_count, 2);
        assert_eq!(ov.tokens.input.value(), 300);
        assert_eq!(ov.tokens.output.value(), 30);
        assert_eq!(ov.session_count, 2);
    }

    #[test]
    fn non_jsonl_files_are_ignored() {
        let tmp = TestRoot::new("ext");
        write_session(tmp.path(), "2026/08/08", "2026-08-08", "rollout-a.jsonl", "gpt-5.6-sol", 100, 10);
        write_rollout(tmp.path(), "2026/08/08", "notes.txt", &["garbage"]);

        let repo = CodexUsageRepository::new(tmp.path().to_path_buf());
        let ov = repo.overview(&Filter::default()).unwrap();
        assert_eq!(ov.message_count, 1);
    }

    #[test]
    fn aggregates_daily_by_model() {
        let tmp = TestRoot::new("daily");
        write_session(tmp.path(), "2026/08/08", "2026-08-08", "a.jsonl", "gpt-5.6-sol", 100, 10);
        write_session(tmp.path(), "2026/08/08", "2026-08-08", "b.jsonl", "gpt-5.6-sol", 200, 20);
        write_session(tmp.path(), "2026/08/07", "2026-08-07", "c.jsonl", "gpt-5.6-terra", 50, 5);

        let repo = CodexUsageRepository::new(tmp.path().to_path_buf());
        let rows = repo.daily_by_model(&Filter::default()).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].date.to_string(), "2026-08-08");
        assert_eq!(rows[0].model.as_str(), "gpt-5.6-sol");
        assert_eq!(rows[0].tokens.input.value(), 300);
        assert_eq!(rows[1].date.to_string(), "2026-08-07");
        assert_eq!(rows[1].tokens.input.value(), 50);
    }

    #[test]
    fn date_range_filter_excludes_out_of_range() {
        let tmp = TestRoot::new("range");
        write_session(tmp.path(), "2026/08/08", "2026-08-08", "a.jsonl", "gpt-5.6-sol", 100, 10);
        write_session(tmp.path(), "2026/07/01", "2026-07-01", "b.jsonl", "gpt-5.6-sol", 999, 99);

        let repo = CodexUsageRepository::new(tmp.path().to_path_buf());
        let filter = Filter {
            date_range: Some(
                DateRange::new(
                    Some(NaiveDate::from_ymd_opt(2026, 8, 1).unwrap()),
                    Some(NaiveDate::from_ymd_opt(2026, 8, 31).unwrap()),
                )
                .unwrap(),
            ),
            ..Filter::default()
        };
        let ov = repo.overview(&filter).unwrap();
        assert_eq!(ov.tokens.input.value(), 100);
    }

    #[test]
    fn model_filter_selects_one_model() {
        let tmp = TestRoot::new("model");
        write_session(tmp.path(), "2026/08/08", "2026-08-08", "a.jsonl", "gpt-5.6-sol", 100, 10);
        write_session(tmp.path(), "2026/08/08", "2026-08-08", "b.jsonl", "gpt-5.6-terra", 700, 70);

        let repo = CodexUsageRepository::new(tmp.path().to_path_buf());
        let filter = Filter {
            model: Some(shared::domain::value_objects::ModelId::new("gpt-5.6-terra").unwrap()),
            ..Filter::default()
        };
        let ov = repo.overview(&filter).unwrap();
        assert_eq!(ov.tokens.input.value(), 700);
    }

    #[test]
    fn default_sessions_root_points_at_codex_sessions() {
        let root = default_sessions_root();
        assert!(root.ends_with(".codex/sessions"), "got {:?}", root);
    }
}
```

Update `crates/analysis/src/adapters/gateways/codex/mod.rs`:

```rust
pub mod parser;
pub mod repository;

pub use parser::parse_rollout;
pub use repository::{CodexUsageRepository, default_sessions_root};
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p analysis codex::repository 2>&1 | tail -20
```

Expected: compile error — `CodexUsageRepository` does not implement `UsageRepository`, so `repo.overview(...)` has no method. That is the failure signal for this task.

- [ ] **Step 3: Implement the repository**

Replace the stub at the top of `crates/analysis/src/adapters/gateways/codex/repository.rs` (everything above `#[cfg(test)]`) with:

```rust
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::NaiveDate;

use crate::adapters::gateways::codex::parser::parse_rollout;
use crate::application::dto::Filter;
use crate::application::ports::UsageRepository;
use shared::adapters::AdapterError;
use shared::application::errors::ApplicationError;
use shared::domain::entities::{DayModelRow, Overview, UsageRecord};
use shared::domain::value_objects::{Cost, DateRange, ModelId, TokenBreakdown};

pub fn default_sessions_root() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".codex").join("sessions")
}

/// Reads Codex CLI rollout files. Loads every file once on first query and
/// serves later queries from the cache — the TUI re-queries on every filter
/// change, and the files are small.
pub struct CodexUsageRepository {
    root: PathBuf,
    cache: Mutex<Option<Vec<UsageRecord>>>,
}

impl CodexUsageRepository {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            cache: Mutex::new(None),
        }
    }

    fn records(&self) -> Result<Vec<UsageRecord>, AdapterError> {
        {
            let guard = self.cache.lock().unwrap();
            if let Some(cached) = guard.as_ref() {
                return Ok(cached.clone());
            }
        }
        let loaded = load_all(&self.root)?;
        let mut guard = self.cache.lock().unwrap();
        *guard = Some(loaded.clone());
        Ok(loaded)
    }
}

fn load_all(root: &Path) -> Result<Vec<UsageRecord>, AdapterError> {
    let mut out: Vec<UsageRecord> = Vec::new();
    if !root.exists() {
        return Ok(out);
    }
    visit_dir(root, &mut out)?;
    Ok(out)
}

/// Codex nests sessions as `YYYY/MM/DD/rollout-*.jsonl`, so the walk recurses
/// rather than reading a single level like the Claude Code adapter.
fn visit_dir(dir: &Path, out: &mut Vec<UsageRecord>) -> Result<(), AdapterError> {
    for entry in std::fs::read_dir(dir).map_err(io_err)? {
        let entry = entry.map_err(io_err)?;
        let path = entry.path();
        if path.is_dir() {
            visit_dir(&path, out)?;
        } else if path.extension().and_then(|s| s.to_str()) == Some("jsonl") {
            let f = File::open(&path).map_err(io_err)?;
            let reader = BufReader::new(f);
            parse_rollout(reader.lines().map_while(Result::ok), out);
        }
    }
    Ok(())
}

fn io_err(e: std::io::Error) -> AdapterError {
    AdapterError::DataMapping(format!("codex io: {}", e))
}

fn in_range(date: NaiveDate, range: Option<&DateRange>) -> bool {
    let Some(r) = range else { return true };
    if let Some(from) = r.from
        && date < from
    {
        return false;
    }
    if let Some(to) = r.to
        && date > to
    {
        return false;
    }
    true
}

fn matches_filter(r: &UsageRecord, filter: &Filter) -> bool {
    if !in_range(r.date, filter.date_range.as_ref()) {
        return false;
    }
    if let Some(p) = &filter.project
        && r.project.as_str() != p.as_str()
    {
        return false;
    }
    if let Some(m) = &filter.model
        && r.model.as_str() != m.as_str()
    {
        return false;
    }
    if let Some(s) = &filter.session_id
        && r.session_id != *s
    {
        return false;
    }
    // provider filter is not available in rollout records — pass-through.
    true
}

impl UsageRepository for CodexUsageRepository {
    fn overview(&self, filter: &Filter) -> Result<Overview, ApplicationError> {
        let records = self.records().map_err(ApplicationError::from)?;
        let mut tokens = TokenBreakdown::default();
        let mut cost = Cost::zero();
        let mut messages: u64 = 0;
        let mut sessions: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for r in records.iter().filter(|r| matches_filter(r, filter)) {
            tokens += r.tokens;
            cost += r.cost;
            messages += 1;
            if !r.session_id.is_empty() {
                sessions.insert(r.session_id.as_str());
            }
        }
        let range = filter.date_range.unwrap_or_else(DateRange::unbounded);
        Ok(Overview {
            range,
            session_count: sessions.len() as u64,
            message_count: messages,
            tokens,
            cost,
        })
    }

    fn daily_by_model(&self, filter: &Filter) -> Result<Vec<DayModelRow>, ApplicationError> {
        let records = self.records().map_err(ApplicationError::from)?;
        type Group = (TokenBreakdown, Cost);
        let mut map: HashMap<(NaiveDate, String), (ModelId, Group)> = HashMap::new();
        for r in records.iter().filter(|r| matches_filter(r, filter)) {
            let key = (r.date, r.model.as_str().to_string());
            let entry = map
                .entry(key)
                .or_insert_with(|| (r.model.clone(), (TokenBreakdown::default(), Cost::zero())));
            entry.1.0 += r.tokens;
            entry.1.1 += r.cost;
        }
        let mut out: Vec<DayModelRow> = map
            .into_iter()
            .map(|((date, _), (model, (tokens, cost)))| DayModelRow {
                date,
                model,
                tokens,
                cost,
            })
            .collect();
        out.sort_by(|a, b| {
            b.date
                .cmp(&a.date)
                .then_with(|| a.model.as_str().cmp(b.model.as_str()))
        });
        Ok(out)
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p analysis codex 2>&1 | tail -20
```

Expected: all Task 1 and Task 2 codex tests pass — 18 total.

- [ ] **Step 5: Run the lint gates**

```bash
cargo fmt && cargo clippy -p analysis --all-targets -- -D warnings 2>&1 | tail -20
```

Expected: no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/analysis/src/adapters/gateways/codex
git commit -m "feat(analysis): add codex usage repository"
```

Allow at least 600 seconds.

---

## Task 3: Third `DataSource` variant

**Files:**
- Modify: `crates/analysis/src/adapters/gateways/dispatching_usage_repository.rs`
- Modify: `crates/analysis/src/tui/controllers/tui_controller.rs` (test only, around line 925)
- Modify: `crates/analysis/src/main.rs:26-40`

**Interfaces:**
- Consumes: `CodexUsageRepository` and `default_sessions_root` from Task 2.
- Produces: `DataSource::Codex`; `DispatchingUsageRepository::new(opencode, claudecode, codex, current)` — a **four-argument** constructor.

`main.rs` is updated in this task, not the next one: `cargo test -p analysis` compiles the binary target too, so the crate cannot be green while the composition root still calls the three-argument constructor.

- [ ] **Step 1: Write the failing tests**

In `crates/analysis/src/adapters/gateways/dispatching_usage_repository.rs`, replace the `data_source_toggle_flips` test with these three, and add the routing test:

```rust
    #[test]
    fn data_source_toggle_cycles_three_sources() {
        assert_eq!(DataSource::OpenCode.toggle(), DataSource::ClaudeCode);
        assert_eq!(DataSource::ClaudeCode.toggle(), DataSource::Codex);
        assert_eq!(DataSource::Codex.toggle(), DataSource::OpenCode);
    }

    #[test]
    fn data_source_u8_round_trips() {
        for s in [DataSource::OpenCode, DataSource::ClaudeCode, DataSource::Codex] {
            assert_eq!(DataSource::from_u8(s.as_u8()), s);
        }
        // Unknown values still fall back to the default source.
        assert_eq!(DataSource::from_u8(99), DataSource::OpenCode);
    }

    #[test]
    fn codex_has_its_own_label() {
        assert_eq!(DataSource::Codex.label(), "Codex");
    }

    #[test]
    fn dispatcher_routes_to_codex_when_selected() {
        let oc: Arc<dyn UsageRepository> = Arc::new(fake_with(100));
        let cc: Arc<dyn UsageRepository> = Arc::new(fake_with(999));
        let cx: Arc<dyn UsageRepository> = Arc::new(fake_with(555));

        let cell = DataSourceCell::new(DataSource::OpenCode);
        let disp = DispatchingUsageRepository::new(oc, cc, cx, cell.clone());

        cell.set(DataSource::Codex);
        let rows = disp.daily_by_model(&Filter::default()).unwrap();
        assert_eq!(rows[0].tokens.input.value(), 555);
    }
```

Also update the two existing dispatcher tests (`dispatcher_routes_to_opencode_by_default` and `dispatcher_follows_source_cell_changes`) to pass a third repository into the constructor. In each, add before the `DispatchingUsageRepository::new` call:

```rust
        let cx: Arc<dyn UsageRepository> = Arc::new(fake_with(555));
```

and change the constructor call to `DispatchingUsageRepository::new(oc, cc, cx, cell)` (keep `cell.clone()` where the original used it).

In `crates/analysis/src/tui/controllers/tui_controller.rs`, replace the body of `t_key_toggles_data_source_and_invalidates_vms` with a three-step walk:

```rust
    #[test]
    fn t_key_toggles_data_source_and_invalidates_vms() {
        use crate::adapters::gateways::DataSource;
        let (controller, _) = ctl_with_source_rows(vec![]);
        let mut state = AppState::new();
        controller.warmup(&mut state).unwrap();
        assert_eq!(state.data_source.get(), DataSource::OpenCode);

        let t = KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE);
        controller.handle(t, &mut state).unwrap();
        assert_eq!(state.data_source.get(), DataSource::ClaudeCode);
        assert_eq!(state.status_message.as_deref(), Some("Source: Claude Code"));
        assert!(state.dashboard_vm.is_some()); // re-populated

        controller
            .handle(
                KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE),
                &mut state,
            )
            .unwrap();
        assert_eq!(state.data_source.get(), DataSource::Codex);
        assert_eq!(state.status_message.as_deref(), Some("Source: Codex"));
        assert!(state.dashboard_vm.is_some());

        controller
            .handle(
                KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE),
                &mut state,
            )
            .unwrap();
        assert_eq!(state.data_source.get(), DataSource::OpenCode);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p analysis dispatching 2>&1 | tail -20
```

Expected: compile error — no variant named `Codex` on `DataSource`, and `DispatchingUsageRepository::new` takes 3 arguments, not 4.

- [ ] **Step 3: Implement the third variant**

In `crates/analysis/src/adapters/gateways/dispatching_usage_repository.rs`, change the enum and its four methods:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataSource {
    OpenCode,
    ClaudeCode,
    Codex,
}

impl DataSource {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => DataSource::ClaudeCode,
            2 => DataSource::Codex,
            _ => DataSource::OpenCode,
        }
    }

    pub fn as_u8(self) -> u8 {
        match self {
            DataSource::OpenCode => 0,
            DataSource::ClaudeCode => 1,
            DataSource::Codex => 2,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            DataSource::OpenCode => "OpenCode",
            DataSource::ClaudeCode => "Claude Code",
            DataSource::Codex => "Codex",
        }
    }

    /// Advances to the next source, wrapping around.
    pub fn toggle(self) -> Self {
        match self {
            DataSource::OpenCode => DataSource::ClaudeCode,
            DataSource::ClaudeCode => DataSource::Codex,
            DataSource::Codex => DataSource::OpenCode,
        }
    }
}
```

Then the struct, constructor, and dispatch arm:

```rust
pub struct DispatchingUsageRepository {
    opencode: Arc<dyn UsageRepository>,
    claudecode: Arc<dyn UsageRepository>,
    codex: Arc<dyn UsageRepository>,
    current: DataSourceCell,
}

impl DispatchingUsageRepository {
    pub fn new(
        opencode: Arc<dyn UsageRepository>,
        claudecode: Arc<dyn UsageRepository>,
        codex: Arc<dyn UsageRepository>,
        current: DataSourceCell,
    ) -> Self {
        Self {
            opencode,
            claudecode,
            codex,
            current,
        }
    }

    fn active(&self) -> &Arc<dyn UsageRepository> {
        match self.current.get() {
            DataSource::OpenCode => &self.opencode,
            DataSource::ClaudeCode => &self.claudecode,
            DataSource::Codex => &self.codex,
        }
    }
}
```

- [ ] **Step 4: Wire the repository into the composition root**

In `crates/analysis/src/main.rs`, add the import next to the Claude Code one:

```rust
use analysis::adapters::gateways::claudecode::{ClaudeCodeUsageRepository, default_projects_root};
use analysis::adapters::gateways::codex::{CodexUsageRepository, default_sessions_root};
```

Then replace lines 32–40 (the Claude Code repo through the dispatcher construction) with:

```rust
    let claudecode_repo: Arc<dyn UsageRepository> =
        Arc::new(ClaudeCodeUsageRepository::new(default_projects_root()));

    let codex_repo: Arc<dyn UsageRepository> =
        Arc::new(CodexUsageRepository::new(default_sessions_root()));

    let data_source = DataSourceCell::new(DataSource::OpenCode);
    let usage_repo: Arc<dyn UsageRepository> = Arc::new(DispatchingUsageRepository::new(
        opencode_repo,
        claudecode_repo,
        codex_repo,
        data_source.clone(),
    ));
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test --workspace 2>&1 | tail -20
cargo fmt && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -20
```

Expected: the whole workspace passes with no clippy warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/analysis/src/adapters/gateways/dispatching_usage_repository.rs crates/analysis/src/tui/controllers/tui_controller.rs crates/analysis/src/main.rs
git commit -m "feat(analysis): cycle the data source through three variants"
```

Allow at least 600 seconds.

---

## Task 4: Verify against real data and document

**Files:**
- Modify: `CLAUDE.md`

**Interfaces:**
- Consumes: the wired binary from Task 3.
- Produces: nothing — verification and documentation only.

- [ ] **Step 1: Verify against real data**

```bash
cargo run -p analysis
```

Press `t` twice to reach `Source: Codex`. Expected: the status bar reads `Codex`, and the dashboard shows rows for `gpt-5.6-sol` / `gpt-5.6-terra` / `gpt-5.5` with non-zero cost (all three models resolve in `pricing.db`). Press `q` to quit.

If the dashboard is empty, check that `~/.codex/sessions` exists and contains `rollout-*.jsonl` files before assuming a code defect.

- [ ] **Step 2: Update `CLAUDE.md`**

In the **Project** section, change the `analysis` bullet so the data sources read:

```
- **`analysis`** — interactive Ratatui TUI that reads the OpenCode SQLite database at
  `~/.local/share/opencode/opencode.db`, Claude Code's JSONL session files, and Codex CLI's
  rollout JSONL files at `~/.codex/sessions/`, then renders token/cost usage as a
  ccusage-style dashboard. Menu-driven, not argv-driven. Has both `lib` and `bin` targets.
```

In the **Architecture** tree, add the Codex adapter to the `analysis/adapters/` line:

```
│       ├── adapters/       SqliteUsageRepository, ClaudeCodeUsageRepository,
│       │                   CodexUsageRepository, DispatchingUsageRepository,
│       │                   LiteLlmPricingSource, presenters, view models
```

In the **Design docs** list, add:

```
- **Codex usage source** — spec `docs/superpowers/specs/2026-08-08-codex-usage-source-design.md`, plan `docs/superpowers/plans/2026-08-08-codex-usage-source.md`.
```

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: note codex as a third analysis usage source"
```

Allow at least 600 seconds.

---

## Done When

- `cargo test --workspace` passes with 18 new codex tests plus the updated dispatcher and controller tests.
- `cargo clippy --workspace --all-targets -- -D warnings` is clean.
- Pressing `t` in `cargo run -p analysis` cycles OpenCode → Claude Code → Codex → OpenCode, and the Codex view shows real token and cost figures.
- Four commits on `feature/codex-usage-source`, none on `develop`.
