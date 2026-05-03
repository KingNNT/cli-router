use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use proxy::adapters::oauth::OAuthSessionStore;
use proxy::adapters::providers::{LiveProvider, build_from_config};
use proxy::adapters::quota::InMemoryQuota;
use proxy::adapters::storage::{SqliteRequestLogRepository, ensure_current};
use proxy::application::ports::{Provider, RequestLogPort, RequestLogReadPort};
use proxy::application::use_cases::{
    CompleteAnthropicOAuth, GetConfig, GetRecentRequests, GetStatus, GetUsageSummary,
    HandleMessages, StartAnthropicOAuth, TestProvider, UpdateConfig,
};
use proxy::config::Config;
use proxy::frameworks::AdminState;
use rusqlite::Connection;
use shared::adapters::clock::SystemClock;
use shared::adapters::gateways::CompositePricingRepository;
use shared::adapters::gateways::sqlite::{SqlitePricingRepository, open_readonly};
use shared::application::ports::{Clock, PricingRepository};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,proxy=debug")),
        )
        .init();

    let cfg = Config::from_env()?;
    tracing::info!(?cfg, "starting proxy");
    let config_path = Config::resolved_path();

    if let Some(parent) = cfg.proxy_db.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let proxy_conn = Connection::open(&cfg.proxy_db)?;
    ensure_current(&proxy_conn)?;
    let proxy_conn = Arc::new(Mutex::new(proxy_conn));

    let local_user_id: i64 = {
        let c = proxy_conn.lock().unwrap();
        c.query_row(
            "SELECT id FROM users WHERE external_id = 'local'",
            [],
            |r| r.get(0),
        )?
    };

    let request_repo = Arc::new(SqliteRequestLogRepository::new(proxy_conn));
    let request_log: Arc<dyn RequestLogPort> = request_repo.clone();
    let request_read: Arc<dyn RequestLogReadPort> = request_repo;

    let clock: Arc<dyn Clock> = Arc::new(SystemClock);

    let pricing: Arc<dyn PricingRepository> = if cfg.pricing_db.exists() {
        let pc = open_readonly(&cfg.pricing_db)?;
        let sqlite = SqlitePricingRepository::new(Arc::new(Mutex::new(pc)))?;
        let const_pricing = shared::domain::services::aliases::canonical_pricing(clock.today());
        Arc::new(CompositePricingRepository::new(
            const_pricing,
            Arc::new(sqlite),
        ))
    } else {
        tracing::warn!(path = %cfg.pricing_db.display(), "pricing.db missing — costs will be NULL");
        let const_pricing = shared::domain::services::aliases::canonical_pricing(clock.today());
        Arc::new(CompositePricingRepository::new(
            const_pricing,
            Arc::new(NullPricing),
        ))
    };

    let http = reqwest::Client::builder().build()?;
    let initial_router = build_from_config(&cfg, http.clone())?;
    let live = Arc::new(LiveProvider::new(initial_router));
    let provider: Arc<dyn Provider> = live.clone();

    let port = cfg.port;

    // Build quota adapter from config, then seed from historical request log.
    let now_ms_u64 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let quota_configs = cfg
        .quota
        .iter()
        .map(|r| r.to_domain())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e: proxy::config::ConfigError| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, e.to_string())
        })?;

    let quota = Arc::new(InMemoryQuota::new(quota_configs));

    let max_window_ms: u64 = cfg
        .quota
        .iter()
        .filter_map(|r| {
            proxy::domain::quota::parse_window(&r.window)
                .ok()
                .map(|w| w.duration_ms())
        })
        .max()
        .unwrap_or(0);

    if max_window_ms > 0 {
        let cutoff_ms = (now_ms_u64 as i64).saturating_sub(max_window_ms as i64);
        let seed_rows = request_read.quota_seed(cutoff_ms).map_err(|e| {
            std::io::Error::other(e.to_string())
        })?;
        quota.seed(seed_rows);
    }

    let cfg_lock = Arc::new(RwLock::new(cfg));

    let use_case = Arc::new(HandleMessages::new(
        provider,
        request_log,
        pricing,
        clock,
        local_user_id,
        quota,
    ));

    let oauth_sessions = Arc::new(OAuthSessionStore::new());

    // Spawn background OAuth token refresh (checks every 60s, persists to disk).
    proxy::adapters::providers::token_refresh::spawn(
        cfg_lock.clone(),
        config_path.clone(),
        http.clone(),
        live.clone(),
    );

    let usage_summary = Arc::new(GetUsageSummary::new(request_read.clone()));

    let admin = AdminState {
        get_status: Arc::new(GetStatus::new(
            request_read.clone(),
            now_epoch_ms(),
            cfg_lock.clone(),
        )),
        get_config: Arc::new(GetConfig::new(cfg_lock.clone())),
        get_recent: Arc::new(GetRecentRequests::new(request_read)),
        update_config: Arc::new(UpdateConfig::new(
            cfg_lock.clone(),
            config_path.clone(),
            live.clone(),
            http.clone(),
        )),
        test_provider: Arc::new(TestProvider::new(cfg_lock.clone(), http.clone())),
        start_oauth: Arc::new(StartAnthropicOAuth::new(oauth_sessions.clone())),
        complete_oauth: Arc::new(CompleteAnthropicOAuth::new(
            oauth_sessions,
            http,
            cfg_lock,
            config_path,
            live,
        )),
        usage_summary,
    };

    let addr: SocketAddr = format!("127.0.0.1:{port}").parse()?;
    proxy::serve(addr, use_case, admin).await?;
    Ok(())
}

fn now_epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Placeholder pricing repo when pricing.db is missing — always empty.
struct NullPricing;
impl PricingRepository for NullPricing {
    fn upsert_many(
        &self,
        _rows: &[shared::domain::entities::ModelPricing],
    ) -> Result<usize, shared::application::errors::ApplicationError> {
        Ok(0)
    }
    fn find_many(
        &self,
        _keys: &[String],
    ) -> Result<
        std::collections::HashMap<String, shared::domain::entities::ModelPricing>,
        shared::application::errors::ApplicationError,
    > {
        Ok(std::collections::HashMap::new())
    }
    fn list(
        &self,
    ) -> Result<
        Vec<shared::domain::entities::ModelPricing>,
        shared::application::errors::ApplicationError,
    > {
        Ok(Vec::new())
    }
    fn last_sync(
        &self,
    ) -> Result<Option<chrono::NaiveDate>, shared::application::errors::ApplicationError> {
        Ok(None)
    }
}
