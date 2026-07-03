//! End-to-end test for the composition root.
//!
//! Loads `adapters/tests/fixtures/seed.sql` into in-memory SQLite, wires the real
//! gateways (SqliteUsageRepository, SqlitePricingRepository) and use cases
//! the same way `main.rs` does, and asserts the results match the fixture.
//!
//! This is the only test that crosses every ring boundary.

use std::sync::{Arc, Mutex};

use chrono::NaiveDate;
use rusqlite::Connection;

use analysis::adapters::gateways::sqlite::SqliteUsageRepository;
use analysis::application::dto::{
    Filter, GetDashboardInput, GetModelsBreakdownInput, GetPricingInput,
};
use analysis::application::ports::UsageRepository;
use analysis::application::use_cases::{GetDashboard, GetModelsBreakdown, GetPricing};
use shared::adapters::gateways::sqlite::SqlitePricingRepository;
use shared::application::ports::{Clock, PricingRepository};
use shared::domain::entities::ModelPricing;
use shared::domain::value_objects::{ModelId, PricePerToken};

struct FixedClock(NaiveDate);

impl Clock for FixedClock {
    fn today(&self) -> NaiveDate {
        self.0
    }
}

fn seeded_usage_conn() -> Arc<Mutex<Connection>> {
    let conn = Connection::open_in_memory().unwrap();
    let sql = include_str!("fixtures/seed.sql");
    conn.execute_batch(sql).unwrap();
    Arc::new(Mutex::new(conn))
}

fn wire() -> (
    Arc<dyn UsageRepository>,
    Arc<dyn PricingRepository>,
    Arc<dyn Clock>,
) {
    let usage_repo: Arc<dyn UsageRepository> =
        Arc::new(SqliteUsageRepository::new(seeded_usage_conn()));

    let pricing_conn = Arc::new(Mutex::new(Connection::open_in_memory().unwrap()));
    let pricing_repo: Arc<dyn PricingRepository> =
        Arc::new(SqlitePricingRepository::new(pricing_conn).unwrap());

    // Clock date doesn't matter when we pass an explicit filter with no date range.
    let clock: Arc<dyn Clock> = Arc::new(FixedClock(NaiveDate::from_ymd_opt(2026, 4, 25).unwrap()));

    (usage_repo, pricing_repo, clock)
}

fn unfiltered() -> Filter {
    Filter::default() // date_range: None → no time filter
}

#[test]
fn dashboard_aggregates_fixture_totals() {
    let (usage_repo, pricing_repo, clock) = wire();
    let uc = GetDashboard::new(usage_repo, pricing_repo, clock);

    let out = uc
        .execute(GetDashboardInput {
            filter: Some(unfiltered()),
        })
        .unwrap();

    // Seed.sql has 3 assistant rows and 1 user row (filtered out), across 2 sessions.
    assert_eq!(out.overview.message_count, 3, "user-role must be excluded");
    assert_eq!(out.overview.session_count, 2);
    assert_eq!(out.overview.tokens.input.value(), 3500);
    assert_eq!(out.overview.tokens.output.value(), 1800);
    assert_eq!(out.overview.tokens.cache_read.value(), 600);
    assert_eq!(out.overview.tokens.cache_write.value(), 300);

    // Pricing store is empty → every row is unpriced; cost collapses to 0 after reconcile.
    assert!(out.missing_pricing_count > 0);

    // Two distinct models → two unpriced entries.
    assert_eq!(out.unpriced_models.len(), 2);

    // Seed covers two calendar days.
    assert!(
        out.rows.len() >= 2,
        "expected at least 2 day-model rows, got {}",
        out.rows.len()
    );
}

#[test]
fn dashboard_reconciles_cost_when_pricing_present() {
    let (usage_repo, pricing_repo, clock) = wire();

    // Seed pricing for the opus model only; sonnet stays unpriced.
    pricing_repo
        .upsert_many(&[ModelPricing {
            lookup_key: "anthropic/claude-opus-4-7".into(),
            model: ModelId::new("claude-opus-4-7").unwrap(),
            provider_id: "anthropic".into(),
            input_rate: PricePerToken::new(0.000_015).unwrap(),
            output_rate: PricePerToken::new(0.000_075).unwrap(),
            cache_read_rate: Some(PricePerToken::new(0.000_001_5).unwrap()),
            cache_write_rate: Some(PricePerToken::new(0.000_018_75).unwrap()),
            last_synced: NaiveDate::from_ymd_opt(2026, 4, 24).unwrap(),
        }])
        .unwrap();

    let uc = GetDashboard::new(usage_repo, pricing_repo, clock);
    let out = uc
        .execute(GetDashboardInput {
            filter: Some(unfiltered()),
        })
        .unwrap();

    // Only sonnet remains unpriced.
    assert_eq!(
        out.unpriced_models.len(),
        1,
        "expected only sonnet unpriced; got {:?}",
        out.unpriced_models
    );

    // Opus row costs: (1000+2000)*0.000015 + (500+1000)*0.000075
    //   + (200+400)*0.0000015 + (100+200)*0.00001875
    //   = 0.045 + 0.1125 + 0.0009 + 0.005625 = 0.164025
    // Plus sonnet's original cost fallback of 0.20.
    let total_cost = out.overview.cost.value();
    assert!(
        (total_cost - (0.164_025 + 0.20)).abs() < 1e-6,
        "overview cost was {}",
        total_cost
    );

    assert_eq!(
        out.last_pricing_sync,
        Some(NaiveDate::from_ymd_opt(2026, 4, 24).unwrap())
    );
}

#[test]
fn models_breakdown_returns_one_row_per_model_sorted_by_cost() {
    let (usage_repo, pricing_repo, clock) = wire();
    let uc = GetModelsBreakdown::new(usage_repo, pricing_repo, clock);

    let out = uc
        .execute(GetModelsBreakdownInput {
            filter: Some(unfiltered()),
        })
        .unwrap();

    assert_eq!(out.models.len(), 2, "two distinct models in seed.sql");

    let sonnet = out
        .models
        .iter()
        .find(|r| r.model.as_str().contains("sonnet"))
        .expect("sonnet row present");
    assert_eq!(sonnet.tokens.input.value(), 500);
    assert_eq!(sonnet.tokens.output.value(), 300);

    let opus = out
        .models
        .iter()
        .find(|r| r.model.as_str().contains("opus"))
        .expect("opus row present");
    assert_eq!(opus.tokens.input.value(), 3000);
    assert_eq!(opus.tokens.output.value(), 1500);
}

#[test]
fn pricing_use_case_returns_rows_after_upsert() {
    let (_usage, pricing_repo, _clock) = wire();

    pricing_repo
        .upsert_many(&[ModelPricing {
            lookup_key: "anthropic/claude-opus-4-7".into(),
            model: ModelId::new("claude-opus-4-7").unwrap(),
            provider_id: "anthropic".into(),
            input_rate: PricePerToken::new(0.000_015).unwrap(),
            output_rate: PricePerToken::new(0.000_075).unwrap(),
            cache_read_rate: None,
            cache_write_rate: None,
            last_synced: NaiveDate::from_ymd_opt(2026, 4, 24).unwrap(),
        }])
        .unwrap();

    let uc = GetPricing::new(pricing_repo);
    let out = uc.execute(GetPricingInput).unwrap();
    assert_eq!(out.rows.len(), 1);
    assert_eq!(out.rows[0].lookup_key, "anthropic/claude-opus-4-7");
}
