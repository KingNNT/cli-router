use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::NaiveDate;

use crate::application::errors::ApplicationError;
use crate::application::ports::PricingRepository;
use crate::domain::entities::ModelPricing;

pub struct CompositePricingRepository {
    const_pricing: Vec<ModelPricing>,
    sqlite: Arc<dyn PricingRepository>,
}

impl CompositePricingRepository {
    pub fn new(const_pricing: Vec<ModelPricing>, sqlite: Arc<dyn PricingRepository>) -> Self {
        Self {
            const_pricing,
            sqlite,
        }
    }
}

impl PricingRepository for CompositePricingRepository {
    fn find_many(
        &self,
        keys: &[String],
    ) -> Result<HashMap<String, ModelPricing>, ApplicationError> {
        let mut out: HashMap<String, ModelPricing> = HashMap::new();

        for key in keys {
            if let Some(hit) = self.const_pricing.iter().find(|p| &p.lookup_key == key) {
                out.insert(key.clone(), hit.clone());
            }
        }

        let remaining: Vec<String> = keys
            .iter()
            .filter(|k| !out.contains_key(*k))
            .cloned()
            .collect();
        if !remaining.is_empty() {
            let from_sqlite = self.sqlite.find_many(&remaining)?;
            for (k, v) in from_sqlite {
                out.insert(k, v);
            }
        }

        Ok(out)
    }

    fn list(&self) -> Result<Vec<ModelPricing>, ApplicationError> {
        let sqlite_rows = self.sqlite.list()?;
        let const_keys: HashSet<&str> = self
            .const_pricing
            .iter()
            .map(|p| p.lookup_key.as_str())
            .collect();
        let mut out: Vec<ModelPricing> = self.const_pricing.clone();
        for row in sqlite_rows {
            if !const_keys.contains(row.lookup_key.as_str()) {
                out.push(row);
            }
        }
        out.sort_by(|a, b| a.model.as_str().cmp(b.model.as_str()));
        Ok(out)
    }

    fn upsert_many(&self, rows: &[ModelPricing]) -> Result<usize, ApplicationError> {
        self.sqlite.upsert_many(rows)
    }

    fn last_sync(&self) -> Result<Option<NaiveDate>, ApplicationError> {
        self.sqlite.last_sync()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::test_support::FakePricingRepository;
    use crate::domain::value_objects::{ModelId, PricePerToken};

    fn pricing(key: &str, input: f64, provider: &str) -> ModelPricing {
        ModelPricing {
            lookup_key: key.into(),
            model: ModelId::new(key).unwrap(),
            provider_id: provider.into(),
            input_rate: PricePerToken::new(input).unwrap(),
            output_rate: PricePerToken::new(input * 5.0).unwrap(),
            cache_read_rate: None,
            cache_write_rate: None,
            last_synced: NaiveDate::from_ymd_opt(2026, 4, 23).unwrap(),
        }
    }

    fn setup() -> (CompositePricingRepository, Arc<FakePricingRepository>) {
        let sqlite = Arc::new(FakePricingRepository::default());
        let sqlite_dyn: Arc<dyn PricingRepository> = sqlite.clone();
        let const_pricing = vec![pricing("opus4.6", 5e-6, "alias")];
        (
            CompositePricingRepository::new(const_pricing, sqlite_dyn),
            sqlite,
        )
    }

    #[test]
    fn find_many_hits_const_first() {
        let (repo, sqlite) = setup();
        // Put a different pricing for the same key in sqlite — const must still win.
        sqlite
            .upsert_many(&[pricing("opus4.6", 99e-6, "litellm-shouldnt-win")])
            .unwrap();
        let out = repo.find_many(&["opus4.6".to_string()]).unwrap();
        assert_eq!(out.len(), 1);
        let row = &out["opus4.6"];
        assert_eq!(row.provider_id, "alias");
        assert!((row.input_rate.value() - 5e-6).abs() < 1e-12);
    }

    #[test]
    fn find_many_falls_through_to_sqlite_for_unaliased_keys() {
        let (repo, sqlite) = setup();
        sqlite
            .upsert_many(&[pricing("gpt-4o", 2.5e-6, "openai")])
            .unwrap();
        let out = repo
            .find_many(&["gpt-4o".to_string(), "opus4.6".to_string()])
            .unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out["gpt-4o"].provider_id, "openai");
        assert_eq!(out["opus4.6"].provider_id, "alias");
    }

    #[test]
    fn list_merges_and_dedups_with_const_winning() {
        let (repo, sqlite) = setup();
        sqlite
            .upsert_many(&[
                pricing("opus4.6", 99e-6, "litellm-shouldnt-win"),
                pricing("gpt-4o", 2.5e-6, "openai"),
            ])
            .unwrap();
        let out = repo.list().unwrap();
        let opus = out.iter().find(|p| p.lookup_key == "opus4.6").unwrap();
        assert_eq!(opus.provider_id, "alias");
        assert!(out.iter().any(|p| p.lookup_key == "gpt-4o"));
        // Sorted by model.as_str()
        let keys: Vec<&str> = out.iter().map(|p| p.model.as_str()).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
    }

    #[test]
    fn upsert_many_delegates_to_sqlite_only() {
        let (repo, sqlite) = setup();
        let count = repo
            .upsert_many(&[pricing("new-model", 1e-6, "litellm")])
            .unwrap();
        assert_eq!(count, 1);
        // sqlite has the new row; const layer untouched.
        assert_eq!(sqlite.rows.lock().unwrap().len(), 1);
    }

    #[test]
    fn last_sync_delegates_to_sqlite() {
        let (repo, sqlite) = setup();
        assert_eq!(repo.last_sync().unwrap(), None);
        sqlite.upsert_many(&[pricing("x", 1e-6, "p")]).unwrap();
        assert_eq!(
            repo.last_sync().unwrap(),
            Some(NaiveDate::from_ymd_opt(2026, 4, 23).unwrap())
        );
    }
}
