use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::DateTime;
use serde::Deserialize;

use crate::adapters::gateways::jsonl_records::{daily_by_model_from, overview_from};
use crate::application::dto::Filter;
use crate::application::ports::UsageRepository;
use shared::adapters::AdapterError;
use shared::application::errors::ApplicationError;
use shared::domain::entities::{DayModelRow, Overview, UsageRecord};
use shared::domain::value_objects::{Cost, ModelId, ProjectPath, TokenBreakdown, TokenCount};

pub fn default_projects_root() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".claude").join("projects")
}

pub struct ClaudeCodeUsageRepository {
    root: PathBuf,
    cache: Mutex<Option<Vec<UsageRecord>>>,
}

impl ClaudeCodeUsageRepository {
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
    collect_jsonl(root, &mut out)?;
    Ok(out)
}

/// Walks the whole tree rather than only `<project>/*.jsonl`: Claude Code
/// writes sub-agent transcripts one level deeper, at
/// `<project>/<session-id>/subagents/agent-*.jsonl`, and those rows carry the
/// model the sub-agent actually ran on.
fn collect_jsonl(dir: &Path, out: &mut Vec<UsageRecord>) -> Result<(), AdapterError> {
    for entry in std::fs::read_dir(dir).map_err(io_err)? {
        let path = entry.map_err(io_err)?.path();
        if path.is_dir() {
            collect_jsonl(&path, out)?;
        } else if path.extension().and_then(|s| s.to_str()) == Some("jsonl") {
            parse_jsonl_into(&path, out)?;
        }
    }
    Ok(())
}

fn parse_jsonl_into(path: &Path, out: &mut Vec<UsageRecord>) -> Result<(), AdapterError> {
    let f = File::open(path).map_err(io_err)?;
    let reader = BufReader::new(f);
    for line in reader.lines() {
        let line = match line {
            Ok(s) => s,
            Err(_) => continue,
        };
        if line.trim().is_empty() {
            continue;
        }
        let evt: TranscriptEvent = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue, // tolerate malformed lines
        };
        if evt.event_type.as_deref() != Some("assistant") {
            continue;
        }
        let Some(message) = evt.message else { continue };
        let Some(usage) = message.usage else {
            continue;
        };
        let Some(model_str) = message.model else {
            continue;
        };
        let Some(ts) = evt.timestamp else { continue };
        let Some(cwd) = evt.cwd else { continue };

        let date = match DateTime::parse_from_rfc3339(&ts) {
            Ok(dt) => dt.naive_utc().date(),
            Err(_) => continue,
        };
        let Ok(model) = ModelId::new(&model_str) else {
            continue;
        };
        let Ok(project) = ProjectPath::new(&cwd) else {
            continue;
        };

        let tokens = TokenBreakdown {
            input: TokenCount::new(usage.input_tokens.unwrap_or(0)),
            output: TokenCount::new(usage.output_tokens.unwrap_or(0)),
            reasoning: TokenCount::default(),
            cache_read: TokenCount::new(usage.cache_read_input_tokens.unwrap_or(0)),
            cache_write: TokenCount::new(usage.cache_creation_input_tokens.unwrap_or(0)),
        };

        out.push(UsageRecord {
            date,
            model,
            project,
            tokens,
            cost: Cost::zero(),
            session_id: evt.session_id.unwrap_or_default(),
        });
    }
    Ok(())
}

fn io_err(e: std::io::Error) -> AdapterError {
    AdapterError::DataMapping(format!("claudecode io: {}", e))
}

impl UsageRepository for ClaudeCodeUsageRepository {
    fn overview(&self, filter: &Filter) -> Result<Overview, ApplicationError> {
        let records = self.records().map_err(ApplicationError::from)?;
        Ok(overview_from(&records, filter))
    }

    fn daily_by_model(&self, filter: &Filter) -> Result<Vec<DayModelRow>, ApplicationError> {
        let records = self.records().map_err(ApplicationError::from)?;
        Ok(daily_by_model_from(&records, filter))
    }
}

#[derive(Deserialize)]
struct TranscriptEvent {
    #[serde(rename = "type")]
    event_type: Option<String>,
    timestamp: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    cwd: Option<String>,
    message: Option<TranscriptMessage>,
}

#[derive(Deserialize)]
struct TranscriptMessage {
    model: Option<String>,
    usage: Option<TranscriptUsage>,
}

#[derive(Deserialize)]
struct TranscriptUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use shared::domain::value_objects::DateRange;
    use std::io::Write;

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new(name: &str) -> Self {
            let p = std::env::temp_dir().join(format!(
                "analysis-cc-test-{}-{}",
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

    fn write_jsonl(dir: &Path, name: &str, lines: &[&str]) {
        let fp = dir.join(name);
        let mut f = File::create(&fp).unwrap();
        for line in lines {
            writeln!(f, "{}", line).unwrap();
        }
    }

    #[test]
    fn empty_root_returns_zero_overview() {
        let tmp = TestRoot::new("empty");
        let repo = ClaudeCodeUsageRepository::new(tmp.path().to_path_buf());
        let ov = repo.overview(&Filter::default()).unwrap();
        assert_eq!(ov.message_count, 0);
    }

    #[test]
    fn missing_root_returns_zero_overview() {
        let p = std::env::temp_dir().join(format!("analysis-cc-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        let repo = ClaudeCodeUsageRepository::new(p);
        let ov = repo.overview(&Filter::default()).unwrap();
        assert_eq!(ov.message_count, 0);
    }

    #[test]
    fn parses_single_assistant_event() {
        let tmp = TestRoot::new("single");
        let proj = tmp.path().join("-my-project");
        std::fs::create_dir(&proj).unwrap();
        let line = r#"{"type":"assistant","timestamp":"2026-04-23T09:55:36.350Z","sessionId":"s1","cwd":"/my/project","message":{"model":"claude-opus-4-7","usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":3,"cache_creation_input_tokens":2}}}"#;
        write_jsonl(&proj, "a.jsonl", &[line]);

        let repo = ClaudeCodeUsageRepository::new(tmp.path().to_path_buf());
        let ov = repo.overview(&Filter::default()).unwrap();
        assert_eq!(ov.message_count, 1);
        assert_eq!(ov.tokens.input.value(), 10);
        assert_eq!(ov.tokens.output.value(), 5);
        assert_eq!(ov.tokens.cache_read.value(), 3);
        assert_eq!(ov.tokens.cache_write.value(), 2);
        assert_eq!(ov.cost.value(), 0.0);
        assert_eq!(ov.session_count, 1);
    }

    #[test]
    fn non_assistant_events_are_skipped() {
        let tmp = TestRoot::new("skip");
        let proj = tmp.path().join("-p");
        std::fs::create_dir(&proj).unwrap();
        let user =
            r#"{"type":"user","timestamp":"2026-04-23T09:55:36.350Z","sessionId":"s1","cwd":"/p"}"#;
        let asst = r#"{"type":"assistant","timestamp":"2026-04-23T10:00:00Z","sessionId":"s1","cwd":"/p","message":{"model":"m","usage":{"input_tokens":100}}}"#;
        let malformed = "{not json}";
        write_jsonl(&proj, "a.jsonl", &[user, asst, malformed]);

        let repo = ClaudeCodeUsageRepository::new(tmp.path().to_path_buf());
        let ov = repo.overview(&Filter::default()).unwrap();
        assert_eq!(ov.message_count, 1);
        assert_eq!(ov.tokens.input.value(), 100);
    }

    #[test]
    fn aggregates_daily_by_model() {
        let tmp = TestRoot::new("daily");
        let proj = tmp.path().join("-p");
        std::fs::create_dir(&proj).unwrap();
        let lines = [
            r#"{"type":"assistant","timestamp":"2026-04-23T09:00:00Z","sessionId":"s","cwd":"/p","message":{"model":"opus","usage":{"input_tokens":100}}}"#,
            r#"{"type":"assistant","timestamp":"2026-04-23T10:00:00Z","sessionId":"s","cwd":"/p","message":{"model":"opus","usage":{"input_tokens":200}}}"#,
            r#"{"type":"assistant","timestamp":"2026-04-22T10:00:00Z","sessionId":"s","cwd":"/p","message":{"model":"opus","usage":{"input_tokens":50}}}"#,
        ];
        write_jsonl(&proj, "a.jsonl", &lines);

        let repo = ClaudeCodeUsageRepository::new(tmp.path().to_path_buf());
        let rows = repo.daily_by_model(&Filter::default()).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].date.to_string(), "2026-04-23");
        assert_eq!(rows[0].tokens.input.value(), 300);
        assert_eq!(rows[1].tokens.input.value(), 50);
    }

    #[test]
    fn parses_subagent_transcripts_in_nested_dirs() {
        let tmp = TestRoot::new("subagents");
        let proj = tmp.path().join("-p");
        let subagents = proj.join("s1").join("subagents");
        std::fs::create_dir_all(&subagents).unwrap();
        write_jsonl(
            &proj,
            "s1.jsonl",
            &[
                r#"{"type":"assistant","timestamp":"2026-04-23T09:00:00Z","sessionId":"s1","cwd":"/p","message":{"model":"opus","usage":{"input_tokens":100}}}"#,
            ],
        );
        write_jsonl(
            &subagents,
            "agent-a1.jsonl",
            &[
                r#"{"type":"assistant","timestamp":"2026-04-23T09:30:00Z","sessionId":"s1","cwd":"/p","message":{"model":"sonnet","usage":{"input_tokens":40}}}"#,
            ],
        );

        let repo = ClaudeCodeUsageRepository::new(tmp.path().to_path_buf());
        let ov = repo.overview(&Filter::default()).unwrap();
        assert_eq!(ov.message_count, 2);
        assert_eq!(ov.tokens.input.value(), 140);

        let rows = repo.daily_by_model(&Filter::default()).unwrap();
        assert_eq!(rows.len(), 2);
        let sonnet = rows
            .iter()
            .find(|r| r.model.as_str() == "sonnet")
            .expect("sub-agent model must be reported separately");
        assert_eq!(sonnet.tokens.input.value(), 40);
    }

    #[test]
    fn date_range_filter_excludes_out_of_range() {
        let tmp = TestRoot::new("range");
        let proj = tmp.path().join("-p");
        std::fs::create_dir(&proj).unwrap();
        let lines = [
            r#"{"type":"assistant","timestamp":"2026-04-23T09:00:00Z","sessionId":"s","cwd":"/p","message":{"model":"m","usage":{"input_tokens":100}}}"#,
            r#"{"type":"assistant","timestamp":"2026-03-01T09:00:00Z","sessionId":"s","cwd":"/p","message":{"model":"m","usage":{"input_tokens":999}}}"#,
        ];
        write_jsonl(&proj, "a.jsonl", &lines);

        let repo = ClaudeCodeUsageRepository::new(tmp.path().to_path_buf());
        let filter = Filter {
            date_range: Some(
                DateRange::new(
                    Some(NaiveDate::from_ymd_opt(2026, 4, 1).unwrap()),
                    Some(NaiveDate::from_ymd_opt(2026, 4, 30).unwrap()),
                )
                .unwrap(),
            ),
            ..Filter::default()
        };
        let ov = repo.overview(&filter).unwrap();
        assert_eq!(ov.tokens.input.value(), 100);
    }
}
