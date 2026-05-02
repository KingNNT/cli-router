use chrono::NaiveDate;

use crate::domain::value_objects::{ModelId, PricePerToken};

/// One row in the pricing store. A single LiteLLM entry produces up to two
/// `ModelPricing` values with different `lookup_key`s — one raw, one with
/// the provider composed in — both carrying the same display `model`.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelPricing {
    pub lookup_key: String,
    pub model: ModelId,
    pub provider_id: String,
    pub input_rate: PricePerToken,
    pub output_rate: PricePerToken,
    pub cache_read_rate: Option<PricePerToken>,
    pub cache_write_rate: Option<PricePerToken>,
    pub last_synced: NaiveDate,
    /// The canonical alias this row is aliased TO, or `None` if unaliased.
    pub alias: Option<String>,
}
