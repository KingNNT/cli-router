use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use shared::application::ports::{Clock, PricingRepository};
use shared::adapters::clock::SystemClock;
use shared::adapters::gateways::sqlite::{open_readonly, SqlitePricingRepository};
use shared::adapters::gateways::CompositePricingRepository;
use proxy::adapters::providers::AnthropicProvider;
use proxy::adapters::storage::{ensure_current, SqliteRequestLogRepository};
use proxy::application::ports::{Provider, RequestLogPort};
use proxy::application::use_cases::HandleMessages;
use proxy::config::Config;
use rusqlite::Connection;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,proxy=debug")),
        )
        .init();

    let cfg = Config::from_env();
    tracing::info!(?cfg, "starting proxy");

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

    let request_log: Arc<dyn RequestLogPort> =
        Arc::new(SqliteRequestLogRepository::new(proxy_conn));

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
    let provider: Arc<dyn Provider> = Arc::new(AnthropicProvider::new(http));

    let use_case = Arc::new(HandleMessages::new(
        provider,
        request_log,
        pricing,
        clock,
        local_user_id,
    ));

    let addr: SocketAddr = format!("127.0.0.1:{}", cfg.port).parse()?;
    proxy::serve(addr, use_case).await?;
    Ok(())
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
    ) -> Result<Vec<shared::domain::entities::ModelPricing>, shared::application::errors::ApplicationError> {
        Ok(Vec::new())
    }
    fn last_sync(
        &self,
    ) -> Result<Option<chrono::NaiveDate>, shared::application::errors::ApplicationError> {
        Ok(None)
    }
}
