use std::sync::{Arc, Mutex};

use analysis::adapters::gateways::claudecode::{ClaudeCodeUsageRepository, default_projects_root};
use analysis::adapters::gateways::http::LiteLlmPricingSource;
use analysis::adapters::gateways::sqlite::SqliteUsageRepository;
use analysis::adapters::gateways::{DataSource, DataSourceCell, DispatchingUsageRepository};
use analysis::application::ports::{PricingSource, UsageRepository};
use analysis::application::use_cases::{GetDashboard, GetPricing, SyncPricing};
use analysis::errors::FrameworkError;
use analysis::tui::controllers::TuiController;
use analysis::tui::{self, AppState};
use shared::adapters::clock::SystemClock;
use shared::adapters::gateways::sqlite::connection::default_db_path;
use shared::adapters::gateways::sqlite::{
    SqlitePricingRepository, default_pricing_db_path, open_readonly, open_writable,
};
use shared::application::ports::{Clock, PricingRepository};

fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

fn run() -> Result<(), FrameworkError> {
    let usage_conn =
        open_readonly(&default_db_path()).map_err(|e| FrameworkError::Terminal(e.to_string()))?;
    let usage_conn = Arc::new(Mutex::new(usage_conn));
    let opencode_repo: Arc<dyn UsageRepository> = Arc::new(SqliteUsageRepository::new(usage_conn));

    let claudecode_repo: Arc<dyn UsageRepository> =
        Arc::new(ClaudeCodeUsageRepository::new(default_projects_root()));

    let data_source = DataSourceCell::new(DataSource::OpenCode);
    let usage_repo: Arc<dyn UsageRepository> = Arc::new(DispatchingUsageRepository::new(
        opencode_repo,
        claudecode_repo,
        data_source.clone(),
    ));

    let clock: Arc<dyn Clock> = Arc::new(SystemClock);

    let pricing_conn = open_writable(&default_pricing_db_path())
        .map_err(|e| FrameworkError::Terminal(e.to_string()))?;
    let pricing_conn = Arc::new(Mutex::new(pricing_conn));
    let sqlite_pricing: Arc<dyn PricingRepository> = Arc::new(
        SqlitePricingRepository::new(pricing_conn)
            .map_err(|e| FrameworkError::Terminal(e.to_string()))?,
    );
    let const_pricing = shared::domain::services::aliases::canonical_pricing(clock.today());
    let pricing_repo: Arc<dyn PricingRepository> = Arc::new(
        shared::adapters::gateways::CompositePricingRepository::new(const_pricing, sqlite_pricing),
    );
    let pricing_source: Arc<dyn PricingSource> = Arc::new(LiteLlmPricingSource::new(clock.clone()));

    let get_dashboard = Arc::new(GetDashboard::new(
        usage_repo.clone(),
        pricing_repo.clone(),
        clock.clone(),
    ));
    let get_pricing = Arc::new(GetPricing::new(pricing_repo.clone()));
    let controller_clock = clock.clone();
    let sync_pricing = Arc::new(SyncPricing::new(pricing_source, pricing_repo, clock));

    let controller = TuiController::new(
        get_dashboard,
        get_pricing,
        sync_pricing,
        controller_clock,
    );
    let mut state = AppState::with_data_source(data_source);

    let mut terminal = tui::enter()?;
    let result = tui::run(&mut terminal, &controller, &mut state);
    let _ = tui::restore(&mut terminal);
    result
}
