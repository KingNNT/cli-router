//! Test-only fakes for the shared application ports (Clock + PricingRepository).

use std::collections::HashMap;
use std::sync::Mutex;

use chrono::NaiveDate;

use crate::application::errors::ApplicationError;
use crate::application::ports::{Clock, PricingRepository};
use crate::domain::entities::ModelPricing;

pub struct FixedClock {
    pub today: NaiveDate,
}

impl FixedClock {
    pub fn new(year: i32, month: u32, day: u32) -> Self {
        Self {
            today: NaiveDate::from_ymd_opt(year, month, day).unwrap(),
        }
    }
}

impl Clock for FixedClock {
    fn today(&self) -> NaiveDate {
        self.today
    }
}

#[derive(Default)]
pub struct FakePricingRepository {
    pub rows: Mutex<HashMap<String, ModelPricing>>,
    pub last_sync: Mutex<Option<NaiveDate>>,
}

impl PricingRepository for FakePricingRepository {
    fn upsert_many(&self, incoming: &[ModelPricing]) -> Result<usize, ApplicationError> {
        let mut rows = self.rows.lock().unwrap();
        for r in incoming {
            rows.insert(r.lookup_key.clone(), r.clone());
        }
        if let Some(latest) = incoming.iter().map(|r| r.last_synced).max() {
            *self.last_sync.lock().unwrap() = Some(latest);
        }
        Ok(incoming.len())
    }

    fn find_many(
        &self,
        keys: &[String],
    ) -> Result<HashMap<String, ModelPricing>, ApplicationError> {
        let rows = self.rows.lock().unwrap();
        Ok(keys
            .iter()
            .filter_map(|k| rows.get(k).map(|v| (k.clone(), v.clone())))
            .collect())
    }

    fn list(&self) -> Result<Vec<ModelPricing>, ApplicationError> {
        Ok(self.rows.lock().unwrap().values().cloned().collect())
    }

    fn last_sync(&self) -> Result<Option<NaiveDate>, ApplicationError> {
        Ok(*self.last_sync.lock().unwrap())
    }
}
