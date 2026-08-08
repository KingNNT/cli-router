use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::application::dto::Filter;
use crate::application::ports::UsageRepository;
use shared::application::errors::ApplicationError;
use shared::domain::entities::{DayModelRow, Overview};

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

/// Shared cell indicating which `UsageRepository` the dispatcher should route to.
/// Held by both `AppState` and `DispatchingUsageRepository` so toggling the source
/// in the controller immediately affects all subsequent use-case queries.
#[derive(Clone, Default)]
pub struct DataSourceCell(Arc<AtomicU8>);

impl DataSourceCell {
    pub fn new(initial: DataSource) -> Self {
        Self(Arc::new(AtomicU8::new(initial.as_u8())))
    }

    pub fn get(&self) -> DataSource {
        DataSource::from_u8(self.0.load(Ordering::Relaxed))
    }

    pub fn set(&self, source: DataSource) {
        self.0.store(source.as_u8(), Ordering::Relaxed);
    }
}

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

impl UsageRepository for DispatchingUsageRepository {
    fn overview(&self, filter: &Filter) -> Result<Overview, ApplicationError> {
        self.active().overview(filter)
    }

    fn daily_by_model(&self, filter: &Filter) -> Result<Vec<DayModelRow>, ApplicationError> {
        self.active().daily_by_model(filter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::test_support::FakeUsageRepository;
    use chrono::NaiveDate;
    use shared::domain::entities::{DayModelRow, Overview};
    use shared::domain::value_objects::{Cost, ModelId, TokenBreakdown, TokenCount};

    fn row(input: u64) -> DayModelRow {
        DayModelRow {
            date: NaiveDate::from_ymd_opt(2026, 4, 23).unwrap(),
            model: ModelId::new("m").unwrap(),
            tokens: TokenBreakdown {
                input: TokenCount::new(input),
                ..Default::default()
            },
            cost: Cost::zero(),
        }
    }

    fn fake_with(input: u64) -> FakeUsageRepository {
        FakeUsageRepository {
            daily: vec![row(input)],
            ..FakeUsageRepository::default()
        }
    }

    #[test]
    fn dispatcher_routes_to_opencode_by_default() {
        let oc: Arc<dyn UsageRepository> = Arc::new(fake_with(100));
        let cc: Arc<dyn UsageRepository> = Arc::new(fake_with(999));
        let cx: Arc<dyn UsageRepository> = Arc::new(fake_with(555));

        let cell = DataSourceCell::new(DataSource::OpenCode);
        let disp = DispatchingUsageRepository::new(oc, cc, cx, cell);

        let rows = disp.daily_by_model(&Filter::default()).unwrap();
        assert_eq!(rows[0].tokens.input.value(), 100);
    }

    #[test]
    fn dispatcher_follows_source_cell_changes() {
        let oc: Arc<dyn UsageRepository> = Arc::new(fake_with(100));
        let cc: Arc<dyn UsageRepository> = Arc::new(fake_with(999));
        let cx: Arc<dyn UsageRepository> = Arc::new(fake_with(555));

        let cell = DataSourceCell::new(DataSource::OpenCode);
        let disp = DispatchingUsageRepository::new(oc, cc, cx, cell.clone());

        assert_eq!(
            disp.daily_by_model(&Filter::default()).unwrap()[0]
                .tokens
                .input
                .value(),
            100
        );
        cell.set(DataSource::ClaudeCode);
        assert_eq!(
            disp.daily_by_model(&Filter::default()).unwrap()[0]
                .tokens
                .input
                .value(),
            999
        );
    }

    #[test]
    fn data_source_toggle_cycles_three_sources() {
        assert_eq!(DataSource::OpenCode.toggle(), DataSource::ClaudeCode);
        assert_eq!(DataSource::ClaudeCode.toggle(), DataSource::Codex);
        assert_eq!(DataSource::Codex.toggle(), DataSource::OpenCode);
    }

    #[test]
    fn data_source_u8_round_trips() {
        for s in [
            DataSource::OpenCode,
            DataSource::ClaudeCode,
            DataSource::Codex,
        ] {
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

    // Silence unused-trait import warning in tests that don't call Overview directly.
    #[allow(dead_code)]
    fn _overview_shape(_o: Overview) {}
}
