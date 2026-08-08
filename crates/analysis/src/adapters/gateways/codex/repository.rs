use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::adapters::gateways::codex::parser::parse_rollout;
use crate::adapters::gateways::jsonl_records::{daily_by_model_from, overview_from};
use crate::application::dto::Filter;
use crate::application::ports::UsageRepository;
use shared::adapters::AdapterError;
use shared::application::errors::ApplicationError;
use shared::domain::entities::{DayModelRow, Overview, UsageRecord};

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
        let file_type = entry.file_type().map_err(io_err)?;
        if file_type.is_symlink() {
            continue;
        }
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

impl UsageRepository for CodexUsageRepository {
    fn overview(&self, filter: &Filter) -> Result<Overview, ApplicationError> {
        let records = self.records().map_err(ApplicationError::from)?;
        Ok(overview_from(&records, filter))
    }

    fn daily_by_model(&self, filter: &Filter) -> Result<Vec<DayModelRow>, ApplicationError> {
        let records = self.records().map_err(ApplicationError::from)?;
        Ok(daily_by_model_from(&records, filter))
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

    fn write_session(
        root: &Path,
        day_dir: &str,
        ts_day: &str,
        name: &str,
        model: &str,
        input: u64,
        output: u64,
    ) {
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
        write_session(
            tmp.path(),
            "2026/08/07",
            "2026-08-07",
            "rollout-a.jsonl",
            "gpt-5.6-sol",
            100,
            10,
        );
        write_session(
            tmp.path(),
            "2026/08/08",
            "2026-08-08",
            "rollout-b.jsonl",
            "gpt-5.6-sol",
            200,
            20,
        );

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
        write_session(
            tmp.path(),
            "2026/08/08",
            "2026-08-08",
            "rollout-a.jsonl",
            "gpt-5.6-sol",
            100,
            10,
        );
        write_rollout(tmp.path(), "2026/08/08", "notes.txt", &["garbage"]);

        let repo = CodexUsageRepository::new(tmp.path().to_path_buf());
        let ov = repo.overview(&Filter::default()).unwrap();
        assert_eq!(ov.message_count, 1);
    }

    #[test]
    fn aggregates_daily_by_model() {
        let tmp = TestRoot::new("daily");
        write_session(
            tmp.path(),
            "2026/08/08",
            "2026-08-08",
            "a.jsonl",
            "gpt-5.6-sol",
            100,
            10,
        );
        write_session(
            tmp.path(),
            "2026/08/08",
            "2026-08-08",
            "b.jsonl",
            "gpt-5.6-sol",
            200,
            20,
        );
        write_session(
            tmp.path(),
            "2026/08/07",
            "2026-08-07",
            "c.jsonl",
            "gpt-5.6-terra",
            50,
            5,
        );

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
        write_session(
            tmp.path(),
            "2026/08/08",
            "2026-08-08",
            "a.jsonl",
            "gpt-5.6-sol",
            100,
            10,
        );
        write_session(
            tmp.path(),
            "2026/07/01",
            "2026-07-01",
            "b.jsonl",
            "gpt-5.6-sol",
            999,
            99,
        );

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
        write_session(
            tmp.path(),
            "2026/08/08",
            "2026-08-08",
            "a.jsonl",
            "gpt-5.6-sol",
            100,
            10,
        );
        write_session(
            tmp.path(),
            "2026/08/08",
            "2026-08-08",
            "b.jsonl",
            "gpt-5.6-terra",
            700,
            70,
        );

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

    #[test]
    fn symlinked_directories_are_not_followed() {
        let tmp = TestRoot::new("symlink");
        write_session(
            tmp.path(),
            "2026/08/08",
            "2026-08-08",
            "a.jsonl",
            "gpt-5.6-sol",
            100,
            10,
        );
        // A cycle: <root>/2026/08/loop -> <root>/2026
        std::os::unix::fs::symlink(tmp.path().join("2026"), tmp.path().join("2026/08/loop"))
            .unwrap();

        let repo = CodexUsageRepository::new(tmp.path().to_path_buf());
        let ov = repo.overview(&Filter::default()).unwrap();
        assert_eq!(ov.message_count, 1);
    }
}
