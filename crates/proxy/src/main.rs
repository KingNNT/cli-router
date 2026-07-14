use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use clap::Parser;
use proxy::adapters::oauth::OAuthSessionStore;
use proxy::adapters::oauth::openai::OAuthSessionStore as OpenAiOAuthSessionStore;
use proxy::adapters::providers::{LiveProvider, build_leaves, build_routing_provider};
use proxy::adapters::quota::InMemoryQuota;
use proxy::adapters::storage::db_config::DbConfigRepository;
use proxy::adapters::storage::{AsyncRequestLog, SqliteRequestLogRepository, ensure_current};
use proxy::application::ports::{
    ConfigRepository, Provider, QuotaPort, RequestLogPort, RequestLogReadPort,
};
use proxy::application::use_cases::{
    CompleteAnthropicOAuth, CompleteOpenAiOAuth, GetConfig, GetQuotaStatus, GetRecentRequests,
    GetStatus, GetUsageSummary, HandleMessages, StartAnthropicOAuth, StartOpenAiOAuth,
    TestProvider, UpdateConfig,
};
use proxy::frameworks::AdminState;
use rusqlite::Connection;
use shared::adapters::clock::SystemClock;
use shared::adapters::gateways::CompositePricingRepository;
use shared::adapters::gateways::sqlite::{SqlitePricingRepository, open_readonly};
use shared::application::ports::{Clock, PricingRepository};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "cli-router-proxy", about = "CLI Router Proxy")]
struct Args {
    /// Path to the SQLite database file
    #[arg(long, default_value_t = default_db_path())]
    db: String,
}

fn default_db_path() -> String {
    std::env::var_os("HOME")
        .map(|h| {
            std::path::PathBuf::from(h)
                .join(".local/share/cli-router/proxy.db")
                .display()
                .to_string()
        })
        .unwrap_or_else(|| "/tmp/cli-router/proxy.db".to_string())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,proxy=debug")),
        )
        .init();

    // Open DB, run migrations
    let db_path = std::path::PathBuf::from(&args.db);
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let proxy_conn = Connection::open(&db_path)?;
    ensure_current(&proxy_conn)?;

    // Config repository — DB is the single source of truth
    let config_repo: Arc<dyn ConfigRepository> =
        Arc::new(DbConfigRepository::new(proxy_conn, args.db));

    let cfg = config_repo.load()?;
    tracing::info!(?cfg, "starting proxy (config from DB)");

    let proxy_conn = Arc::new(Mutex::new(Connection::open(&db_path)?));

    let local_user_id: i64 = {
        let c = proxy_conn.lock().unwrap();
        c.query_row(
            "SELECT id FROM users WHERE external_id = 'local'",
            [],
            |r| r.get(0),
        )?
    };

    let request_repo = Arc::new(SqliteRequestLogRepository::new(proxy_conn));
    // Hot-path writes go through a bounded queue drained by a single background
    // task, so request handlers never block on the SQLite write lock. Reads and
    // the stale-row sweeper keep using the synchronous repo directly (off the
    // request path). 16k is generous headroom; a full queue drops log events
    // rather than slowing requests.
    let request_log_sync: Arc<dyn RequestLogPort> = request_repo.clone();
    let request_log: Arc<dyn RequestLogPort> =
        Arc::new(AsyncRequestLog::spawn(request_log_sync, 16_384));
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

    let http = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        // More idle connections per host so bursts from multiple opencode
        // instances don't churn TCP connections.
        .pool_max_idle_per_host(200)
        // Keep connections alive for 5 minutes — long enough to survive
        // typical think-time gaps between LLM requests.
        .pool_idle_timeout(std::time::Duration::from_secs(300))
        .build()?;

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
        let seed_rows = request_read
            .quota_seed(cutoff_ms)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        quota.seed(seed_rows);
    }

    // Wire quota into the routing provider so pre-flight checks use leaf provider names.
    let quota_port: Arc<dyn QuotaPort> = quota.clone();
    let leaves = build_leaves(&cfg.providers, http.clone())?;
    let initial_router = build_routing_provider(&cfg, &leaves, quota_port.clone())?;

    let live = Arc::new(LiveProvider::new(initial_router, quota_port.clone()));
    let provider: Arc<dyn Provider> = live.clone();

    let cfg_lock = Arc::new(RwLock::new(cfg));

    // Built after cfg_lock so the Anthropic adapter can read the live OAuth
    // token (refreshed in place by the background token_refresh task). The
    // registry rebuilds its adapter map when the provider set changes, so
    // providers added at runtime show up in the account tab without a restart.
    let account_usage_registry = Arc::new(
        proxy::adapters::providers::builder::LiveAccountUsage::new(cfg_lock.clone()),
    );

    let use_case = Arc::new(HandleMessages::new(
        provider,
        request_log,
        pricing,
        clock,
        local_user_id,
        quota_port.clone(),
    ));

    let oauth_sessions = Arc::new(OAuthSessionStore::new());
    let openai_oauth_sessions = Arc::new(OpenAiOAuthSessionStore::new());

    // Spawn background OAuth token refresh (checks every 60s, persists to DB).
    proxy::adapters::providers::token_refresh::spawn(
        cfg_lock.clone(),
        config_repo.clone(),
        http.clone(),
        live.clone(),
    );

    // Spawn background stale-request sweeper.  Runs every 5 minutes and marks
    // any request still in "started" state after 10 minutes as "errored".
    {
        let sweeper_log = use_case.request_log().clone();
        tokio::spawn(async move {
            let interval = std::time::Duration::from_secs(5 * 60);
            let stale_threshold = std::time::Duration::from_secs(10 * 60);
            let mut tick = tokio::time::interval(interval);
            // First tick completes immediately — skip it so we don't sweep on startup.
            tick.tick().await;
            loop {
                tick.tick().await;
                let cutoff_ms = (std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i64)
                    .saturating_sub(stale_threshold.as_millis() as i64);
                match sweeper_log.sweep_stale(cutoff_ms) {
                    Ok(0) => {}
                    Ok(n) => {
                        tracing::info!(
                            n,
                            n,
                            cutoff_ms,
                            "sweeper: marked stale requests as errored"
                        );
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "sweeper: failed to sweep stale requests");
                    }
                }
            }
        });
    }

    let usage_summary = Arc::new(GetUsageSummary::new(request_read.clone()));
    let quota_status = Arc::new(GetQuotaStatus::new(quota_port.clone()));

    let account_usage = Arc::new(proxy::application::use_cases::admin::GetAccountUsage::new(
        account_usage_registry,
        request_read.clone(),
    ));

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
            config_repo.clone(),
            live.clone(),
            http.clone(),
        )),
        test_provider: Arc::new(TestProvider::new(cfg_lock.clone(), http.clone())),
        start_oauth: Arc::new(StartAnthropicOAuth::new(oauth_sessions.clone())),
        complete_oauth: Arc::new(CompleteAnthropicOAuth::new(
            oauth_sessions,
            http.clone(),
            cfg_lock.clone(),
            config_repo.clone(),
            live.clone(),
        )),
        start_openai_oauth: Arc::new(StartOpenAiOAuth::new(openai_oauth_sessions.clone())),
        complete_openai_oauth: Arc::new(CompleteOpenAiOAuth::new(
            openai_oauth_sessions,
            http,
            cfg_lock,
            config_repo,
            live,
        )),
        usage_summary,
        quota_status,
        account_usage,
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
