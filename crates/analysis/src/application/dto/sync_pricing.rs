use chrono::NaiveDate;

#[derive(Debug, Clone, Default)]
pub struct SyncPricingInput;

#[derive(Debug, Clone)]
pub struct SyncPricingOutput {
    pub synced_count: usize,
    pub last_synced_at: NaiveDate,
}
