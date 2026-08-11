use crate::domain::entities::ModelPricing;
use crate::domain::value_objects::{Cost, TokenBreakdown};

/// Cost of a set of tokens priced against a model's rates.
///
/// Reasoning tokens are billed at the output rate — they are a bucket disjoint
/// from `output`, not a subset of it.
///
/// When `cache_read_rate` or `cache_write_rate` is `None`, cache tokens are
/// priced at `input_rate` — the neutral assumption.
pub fn calculate_cost(tokens: &TokenBreakdown, pricing: &ModelPricing) -> Cost {
    let input = tokens.input.value() as f64 * pricing.input_rate.value();
    // Reasoning is a disjoint bucket from `output` in every source that reports
    // it, and providers bill it as output. Pricing it at anything else — or not
    // at all — silently undercounts thinking-heavy models.
    let output =
        (tokens.output.value() + tokens.reasoning.value()) as f64 * pricing.output_rate.value();
    let cache_read = tokens.cache_read.value() as f64
        * pricing
            .cache_read_rate
            .as_ref()
            .map(|r| r.value())
            .unwrap_or(pricing.input_rate.value());
    let cache_write = tokens.cache_write.value() as f64
        * pricing
            .cache_write_rate
            .as_ref()
            .map(|r| r.value())
            .unwrap_or(pricing.input_rate.value());
    Cost::new(input + output + cache_read + cache_write).unwrap_or(Cost::zero())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::value_objects::{ModelId, PricePerToken, TokenCount};
    use chrono::NaiveDate;

    fn pricing(
        input: f64,
        output: f64,
        cache_read: Option<f64>,
        cache_write: Option<f64>,
    ) -> ModelPricing {
        ModelPricing {
            lookup_key: "m".into(),
            model: ModelId::new("m").unwrap(),
            provider_id: "p".into(),
            input_rate: PricePerToken::new(input).unwrap(),
            output_rate: PricePerToken::new(output).unwrap(),
            cache_read_rate: cache_read.map(|v| PricePerToken::new(v).unwrap()),
            cache_write_rate: cache_write.map(|v| PricePerToken::new(v).unwrap()),
            last_synced: NaiveDate::from_ymd_opt(2026, 4, 23).unwrap(),
        }
    }

    fn tokens(input: u64, output: u64, cache_read: u64, cache_write: u64) -> TokenBreakdown {
        TokenBreakdown {
            input: TokenCount::new(input),
            output: TokenCount::new(output),
            reasoning: TokenCount::new(0),
            cache_read: TokenCount::new(cache_read),
            cache_write: TokenCount::new(cache_write),
        }
    }

    #[test]
    fn input_and_output_only() {
        let c = calculate_cost(
            &tokens(1000, 500, 0, 0),
            &pricing(0.00001, 0.00003, None, None),
        );
        // 1000 * 0.00001 + 500 * 0.00003 = 0.01 + 0.015 = 0.025
        assert!((c.value() - 0.025).abs() < 1e-9);
    }

    #[test]
    fn cache_tokens_priced_at_input_rate_when_no_cache_rate() {
        let c = calculate_cost(
            &tokens(0, 0, 100, 0),
            &pricing(0.00001, 0.00003, None, None),
        );
        assert!((c.value() - 0.001).abs() < 1e-9);
    }

    #[test]
    fn cache_read_uses_cache_rate_when_provided() {
        let c = calculate_cost(
            &tokens(0, 0, 100, 0),
            &pricing(0.00001, 0.00003, Some(0.0000025), None),
        );
        // 100 * 0.0000025 = 0.00025
        assert!((c.value() - 0.00025).abs() < 1e-9);
    }

    #[test]
    fn cache_write_uses_cache_rate_when_provided() {
        let c = calculate_cost(
            &tokens(0, 0, 0, 100),
            &pricing(0.00001, 0.00003, None, Some(0.0000125)),
        );
        assert!((c.value() - 0.00125).abs() < 1e-9);
    }

    #[test]
    fn full_mix_with_both_cache_rates() {
        let c = calculate_cost(
            &tokens(1000, 500, 200, 100),
            &pricing(0.00001, 0.00003, Some(0.0000025), Some(0.0000125)),
        );
        // 0.01 + 0.015 + 0.0005 + 0.00125 = 0.02675
        assert!((c.value() - 0.02675).abs() < 1e-9);
    }

    #[test]
    fn reasoning_is_priced_at_the_output_rate() {
        let mut t = tokens(0, 100, 0, 0);
        t.reasoning = TokenCount::new(50);
        let c = calculate_cost(&t, &pricing(0.00001, 0.00003, None, None));
        // (100 + 50) * 0.00003 = 0.0045
        assert!((c.value() - 0.0045).abs() < 1e-9);
    }

    /// Characterisation test from a real OpenCode row: model `gpt-5-codex`,
    /// input 8872, output 236, reasoning 192, no cache, stored cost 0.01537.
    /// Solving that cost against the published rates shows OpenCode bills
    /// `output + reasoning` at the output rate, so our number must match it.
    #[test]
    fn matches_opencodes_own_cost_for_a_reasoning_heavy_row() {
        let mut t = tokens(8872, 236, 0, 0);
        t.reasoning = TokenCount::new(192);
        let c = calculate_cost(&t, &pricing(1.25e-6, 1.0e-5, Some(1.25e-7), None));
        assert!((c.value() - 0.01537).abs() < 1e-9, "got {}", c.value());
    }
}
