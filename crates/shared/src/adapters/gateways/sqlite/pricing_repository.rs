use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::NaiveDate;
use rusqlite::{Connection, params};

use crate::adapters::errors::AdapterError;
use crate::application::errors::ApplicationError;
use crate::application::ports::PricingRepository;
use crate::domain::entities::ModelPricing;
use crate::domain::value_objects::{ModelId, PricePerToken};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS pricing (
    lookup_key             TEXT PRIMARY KEY,
    model_id               TEXT NOT NULL,
    provider_id            TEXT NOT NULL,
    input_per_token        REAL NOT NULL,
    output_per_token       REAL NOT NULL,
    cache_read_per_token   REAL,
    cache_write_per_token  REAL,
    last_synced_at         INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_pricing_model_id ON pricing(model_id);
"#;

pub struct SqlitePricingRepository {
    conn: Arc<Mutex<Connection>>,
}

impl SqlitePricingRepository {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Result<Self, AdapterError> {
        let c = conn.lock().unwrap();
        c.execute_batch(SCHEMA)?;
        drop(c);
        Ok(Self { conn })
    }
}

fn to_epoch_days(d: NaiveDate) -> i64 {
    d.signed_duration_since(NaiveDate::from_ymd_opt(1970, 1, 1).unwrap())
        .num_days()
}

fn from_epoch_days(days: i64) -> Result<NaiveDate, AdapterError> {
    NaiveDate::from_ymd_opt(1970, 1, 1)
        .unwrap()
        .checked_add_signed(chrono::Duration::days(days))
        .ok_or_else(|| AdapterError::DataMapping(format!("invalid epoch days: {}", days)))
}

fn row_to_pricing(row: &rusqlite::Row) -> Result<ModelPricing, AdapterError> {
    let lookup_key: String = row.get(0)?;
    let model_id: String = row.get(1)?;
    let provider_id: String = row.get(2)?;
    let input: f64 = row.get(3)?;
    let output: f64 = row.get(4)?;
    let cache_read: Option<f64> = row.get(5)?;
    let cache_write: Option<f64> = row.get(6)?;
    let last_synced_days: i64 = row.get(7)?;
    Ok(ModelPricing {
        lookup_key,
        model: ModelId::new(model_id).map_err(AdapterError::from)?,
        provider_id,
        input_rate: PricePerToken::new(input).map_err(AdapterError::from)?,
        output_rate: PricePerToken::new(output).map_err(AdapterError::from)?,
        cache_read_rate: cache_read
            .map(PricePerToken::new)
            .transpose()
            .map_err(AdapterError::from)?,
        cache_write_rate: cache_write
            .map(PricePerToken::new)
            .transpose()
            .map_err(AdapterError::from)?,
        last_synced: from_epoch_days(last_synced_days)?,
    })
}

impl PricingRepository for SqlitePricingRepository {
    fn upsert_many(&self, rows: &[ModelPricing]) -> Result<usize, ApplicationError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(AdapterError::from)?;
        {
            let mut stmt = tx
                .prepare(
                    "INSERT OR REPLACE INTO pricing \
                     (lookup_key, model_id, provider_id, input_per_token, output_per_token, \
                      cache_read_per_token, cache_write_per_token, last_synced_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                )
                .map_err(AdapterError::from)?;
            for r in rows {
                stmt.execute(params![
                    r.lookup_key,
                    r.model.as_str(),
                    r.provider_id,
                    r.input_rate.value(),
                    r.output_rate.value(),
                    r.cache_read_rate.as_ref().map(|p| p.value()),
                    r.cache_write_rate.as_ref().map(|p| p.value()),
                    to_epoch_days(r.last_synced),
                ])
                .map_err(AdapterError::from)?;
            }
        }
        tx.commit().map_err(AdapterError::from)?;
        Ok(rows.len())
    }

    fn find_many(
        &self,
        keys: &[String],
    ) -> Result<HashMap<String, ModelPricing>, ApplicationError> {
        if keys.is_empty() {
            return Ok(HashMap::new());
        }
        // Model ids flow through providers with inconsistent casing: LiteLLM
        // upstream keeps its original (often mixed) case, while clients and
        // usage data commonly lowercase. Compare both sides lowercased so the
        // mismatch doesn't silently drop the row. Result-map keys are also
        // lowercased so they line up with the lowercased candidates emitted
        // by `pricing_lookup_keys`.
        let lower_keys: Vec<String> = keys.iter().map(|k| k.to_lowercase()).collect();
        let conn = self.conn.lock().unwrap();
        let placeholders = (1..=lower_keys.len())
            .map(|i| format!("?{}", i))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT lookup_key, model_id, provider_id, input_per_token, output_per_token, \
                    cache_read_per_token, cache_write_per_token, last_synced_at \
             FROM pricing WHERE LOWER(lookup_key) IN ({})",
            placeholders
        );
        let mut stmt = conn.prepare(&sql).map_err(AdapterError::from)?;
        let params: Vec<&dyn rusqlite::types::ToSql> = lower_keys
            .iter()
            .map(|k| k as &dyn rusqlite::types::ToSql)
            .collect();
        let iter = stmt
            .query_map(params.as_slice(), |r| Ok(row_to_pricing(r)))
            .map_err(AdapterError::from)?;
        let mut out = HashMap::new();
        for row in iter {
            let inner = row.map_err(AdapterError::from)?;
            let pricing = inner.map_err(ApplicationError::from)?;
            out.insert(pricing.lookup_key.to_lowercase(), pricing);
        }
        Ok(out)
    }

    fn list(&self) -> Result<Vec<ModelPricing>, ApplicationError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT lookup_key, model_id, provider_id, input_per_token, output_per_token, \
                        cache_read_per_token, cache_write_per_token, last_synced_at \
                 FROM pricing ORDER BY model_id",
            )
            .map_err(AdapterError::from)?;
        let iter = stmt
            .query_map([], |r| Ok(row_to_pricing(r)))
            .map_err(AdapterError::from)?;
        let mut out = Vec::new();
        for row in iter {
            let inner = row.map_err(AdapterError::from)?;
            out.push(inner.map_err(ApplicationError::from)?);
        }
        Ok(out)
    }

    fn last_sync(&self) -> Result<Option<NaiveDate>, ApplicationError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT MAX(last_synced_at) FROM pricing")
            .map_err(AdapterError::from)?;
        let res: Option<i64> = stmt
            .query_row([], |r| r.get(0))
            .map_err(AdapterError::from)?;
        match res {
            Some(days) => Ok(Some(from_epoch_days(days).map_err(ApplicationError::from)?)),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> SqlitePricingRepository {
        let conn = Connection::open_in_memory().unwrap();
        SqlitePricingRepository::new(Arc::new(Mutex::new(conn))).unwrap()
    }

    fn row(
        key: &str,
        model: &str,
        input: f64,
        cache_read: Option<f64>,
        date: (i32, u32, u32),
    ) -> ModelPricing {
        ModelPricing {
            lookup_key: key.into(),
            model: ModelId::new(model).unwrap(),
            provider_id: "p".into(),
            input_rate: PricePerToken::new(input).unwrap(),
            output_rate: PricePerToken::new(0.00003).unwrap(),
            cache_read_rate: cache_read.map(|v| PricePerToken::new(v).unwrap()),
            cache_write_rate: None,
            last_synced: NaiveDate::from_ymd_opt(date.0, date.1, date.2).unwrap(),
        }
    }

    #[test]
    fn upsert_same_key_replaces() {
        let repo = setup();
        repo.upsert_many(&[row("k", "m", 0.00001, None, (2026, 4, 22))])
            .unwrap();
        repo.upsert_many(&[row("k", "m", 0.00002, None, (2026, 4, 23))])
            .unwrap();
        let all = repo.list().unwrap();
        assert_eq!(all.len(), 1);
        assert!((all[0].input_rate.value() - 0.00002).abs() < 1e-9);
    }

    #[test]
    fn find_many_matches_case_insensitively() {
        // LiteLLM upstream stores keys in mixed case (e.g. "minimax/MiniMax-M3"),
        // but usage data and clients commonly lowercase the model id. The lookup
        // must reconcile that mismatch so the dashboard can price a model whose
        // stored row has different case than the incoming candidate key.
        let repo = setup();
        repo.upsert_many(&[row(
            "minimax/MiniMax-M3",
            "minimax/MiniMax-M3",
            0.0000003,
            None,
            (2026, 4, 23),
        )])
        .unwrap();
        let out = repo.find_many(&["minimax/minimax-m3".to_string()]).unwrap();
        assert_eq!(out.len(), 1, "expected case-insensitive match");
        assert!(
            out.contains_key("minimax/minimax-m3"),
            "map must be keyed by lowercased stored value to match lowercased candidate"
        );
        let hit = &out["minimax/minimax-m3"];
        // The ModelPricing's own lookup_key field keeps the original (mixed) case
        // so the pricing tab still displays the upstream name.
        assert_eq!(hit.lookup_key, "minimax/MiniMax-M3");
    }

    #[test]
    fn find_many_returns_hits_and_ignores_misses() {
        let repo = setup();
        repo.upsert_many(&[
            row("a", "ma", 0.00001, None, (2026, 4, 23)),
            row("b", "mb", 0.00001, None, (2026, 4, 23)),
        ])
        .unwrap();
        let out = repo
            .find_many(&["a".to_string(), "missing".to_string(), "b".to_string()])
            .unwrap();
        assert_eq!(out.len(), 2);
        assert!(out.contains_key("a"));
        assert!(out.contains_key("b"));
    }

    #[test]
    fn empty_keys_short_circuits() {
        let repo = setup();
        let out = repo.find_many(&[]).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn last_sync_none_on_empty() {
        let repo = setup();
        assert_eq!(repo.last_sync().unwrap(), None);
    }

    #[test]
    fn last_sync_tracks_max() {
        let repo = setup();
        repo.upsert_many(&[
            row("a", "ma", 0.00001, None, (2026, 4, 22)),
            row("b", "mb", 0.00001, None, (2026, 4, 23)),
        ])
        .unwrap();
        assert_eq!(
            repo.last_sync().unwrap(),
            Some(NaiveDate::from_ymd_opt(2026, 4, 23).unwrap())
        );
    }

    #[test]
    fn cache_rates_round_trip_as_optional() {
        let repo = setup();
        repo.upsert_many(&[row("k", "m", 0.00001, Some(0.0000025), (2026, 4, 23))])
            .unwrap();
        let rows = repo.list().unwrap();
        assert!((rows[0].cache_read_rate.as_ref().unwrap().value() - 0.0000025).abs() < 1e-9);
        assert!(rows[0].cache_write_rate.is_none());
    }

    #[test]
    fn schema_creation_is_idempotent() {
        let conn = Arc::new(Mutex::new(Connection::open_in_memory().unwrap()));
        SqlitePricingRepository::new(conn.clone()).unwrap();
        SqlitePricingRepository::new(conn).unwrap();
    }
}
