use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, NaiveDate};
use serde::Deserialize;

use crate::application::dto::Filter;
use crate::application::ports::UsageRepository;
use shared::adapters::AdapterError;
use shared::application::errors::ApplicationError;
use shared::domain::entities::{DayModelRow, ModelUsage, Overview, UsageRecord};
use shared::domain::value_objects::{
    Cost, DateRange, ModelId, ProjectPath, TokenBreakdown, TokenCount,
};

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
    let project_dirs = std::fs::read_dir(root).map_err(io_err)?;
    for entry in project_dirs {
        let entry = entry.map_err(io_err)?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let session_files = std::fs::read_dir(&path).map_err(io_err)?;
        for file in session_files {
            let file = file.map_err(io_err)?;
            let fp = file.path();
            if fp.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                continue;
            }
            parse_jsonl_into(&fp, &mut out)?;
        }
    }
    Ok(out)
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
    // provider filter is not available in JSONL records — pass-through.
    true
}

impl UsageRepository for ClaudeCodeUsageRepository {
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

    fn by_model(&self, filter: &Filter) -> Result<Vec<ModelUsage>, ApplicationError> {
        let records = self.records().map_err(ApplicationError::from)?;
        let mut map: HashMap<String, (ModelId, u64, TokenBreakdown, Cost)> = HashMap::new();
        for r in records.iter().filter(|r| matches_filter(r, filter)) {
            let key = r.model.as_str().to_string();
            let entry = map
                .entry(key)
                .or_insert_with(|| (r.model.clone(), 0, TokenBreakdown::default(), Cost::zero()));
            entry.1 += 1;
            entry.2 += r.tokens;
            entry.3 += r.cost;
        }
        let mut out: Vec<ModelUsage> = map
            .into_values()
            .map(|(model, count, tokens, cost)| ModelUsage {
                model,
                message_count: count,
                tokens,
                cost,
            })
            .collect();
        out.sort_by(|a, b| {
            b.cost
                .partial_cmp(&a.cost)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.model.as_str().cmp(b.model.as_str()))
        });
        Ok(out)
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
