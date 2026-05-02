use crate::adapters::view_models::pricing_vm::{PricingRowVM, PricingViewModel};
use crate::application::dto::GetPricingOutput;
use shared::domain::value_objects::PricePerToken;

fn fmt_rate(opt: &Option<PricePerToken>) -> String {
    match opt {
        Some(p) => format!("${:.7}", p.value()),
        None => "—".to_string(),
    }
}

fn fmt_rate_required(p: &PricePerToken) -> String {
    format!("${:.7}", p.value())
}

pub fn present_pricing(out: &GetPricingOutput, query: &str) -> PricingViewModel {
    let q = query.to_lowercase();
    let matches = |p: &shared::domain::entities::ModelPricing| {
        if q.is_empty() {
            return true;
        }
        p.model.as_str().to_lowercase().contains(&q)
            || p.provider_id.to_lowercase().contains(&q)
            || p.alias
                .as_ref()
                .is_some_and(|a| a.to_lowercase().contains(&q))
    };

    // De-duplicate by model (a single model may have raw + composed rows).
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let rows: Vec<PricingRowVM> = out
        .rows
        .iter()
        .filter(|p| matches(p))
        .filter(|p| seen.insert(p.model.as_str().to_string()))
        .map(|p| PricingRowVM {
            model: p.model.as_str().to_string(),
            provider: p.provider_id.clone(),
            input: fmt_rate_required(&p.input_rate),
            output: fmt_rate_required(&p.output_rate),
            cache_read: fmt_rate(&p.cache_read_rate),
            cache_write: fmt_rate(&p.cache_write_rate),
            alias: match &p.alias {
                Some(a) => format!("→ {}", a),
                None => String::new(),
            },
        })
        .collect();

    let last_sync_label = match out.last_sync {
        Some(d) => d.to_string(),
        None => "never".to_string(),
    };

    PricingViewModel {
        empty: rows.is_empty(),
        rows,
        last_sync_label,
        query: query.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shared::domain::entities::ModelPricing;
    use shared::domain::value_objects::{ModelId, PricePerToken};
    use chrono::NaiveDate;

    fn pricing(model: &str, cache_read: Option<f64>) -> ModelPricing {
        ModelPricing {
            lookup_key: model.into(),
            model: ModelId::new(model).unwrap(),
            provider_id: "anthropic".into(),
            input_rate: PricePerToken::new(0.000015).unwrap(),
            output_rate: PricePerToken::new(0.000075).unwrap(),
            cache_read_rate: cache_read.map(|v| PricePerToken::new(v).unwrap()),
            cache_write_rate: None,
            last_synced: NaiveDate::from_ymd_opt(2026, 4, 23).unwrap(),
            alias: None,
        }
    }

    #[test]
    fn empty_output_is_empty_vm() {
        let out = GetPricingOutput {
            rows: vec![],
            last_sync: None,
        };
        let vm = present_pricing(&out, "");
        assert!(vm.empty);
        assert_eq!(vm.last_sync_label, "never");
    }

    #[test]
    fn formats_rates_and_dedupes_by_model() {
        let out = GetPricingOutput {
            rows: vec![
                pricing("opus", Some(0.0000015)),
                pricing("opus", Some(0.0000015)), // duplicate (raw + composed)
                pricing("sonnet", None),
            ],
            last_sync: Some(NaiveDate::from_ymd_opt(2026, 4, 23).unwrap()),
        };
        let vm = present_pricing(&out, "");
        assert_eq!(vm.rows.len(), 2);
        assert_eq!(vm.rows[0].input, "$0.0000150");
        assert_eq!(vm.rows[0].cache_read, "$0.0000015");
        assert_eq!(vm.rows[1].cache_read, "—");
        assert_eq!(vm.last_sync_label, "2026-04-23");
    }

    #[test]
    fn alias_some_formats_with_arrow() {
        let mut p = pricing("opus", None);
        p.alias = Some("opus4.6".to_string());
        let out = GetPricingOutput {
            rows: vec![p],
            last_sync: None,
        };
        let vm = present_pricing(&out, "");
        assert_eq!(vm.rows[0].alias, "→ opus4.6");
    }

    #[test]
    fn alias_none_formats_as_empty_string() {
        let out = GetPricingOutput {
            rows: vec![pricing("sonnet", None)],
            last_sync: None,
        };
        let vm = present_pricing(&out, "");
        assert_eq!(vm.rows[0].alias, "");
    }

    // ── Search / filter tests ────────────────────────────────────────────────

    fn make_output() -> GetPricingOutput {
        let mut opus = pricing("opus", None);
        opus.provider_id = "anthropic".into();

        let mut sonnet = pricing("claude-sonnet-4-5", None);
        sonnet.provider_id = "anthropic".into();
        sonnet.alias = Some("sonnet-4-5".to_string());

        let mut gpt = pricing("gpt-4o", None);
        gpt.provider_id = "openai".into();

        GetPricingOutput {
            rows: vec![opus, sonnet, gpt],
            last_sync: None,
        }
    }

    #[test]
    fn empty_query_returns_full_list() {
        let out = make_output();
        let vm = present_pricing(&out, "");
        assert_eq!(vm.rows.len(), 3);
        assert!(!vm.empty);
    }

    #[test]
    fn query_filters_case_insensitively_by_model() {
        let out = make_output();
        let vm = present_pricing(&out, "opus");
        assert_eq!(vm.rows.len(), 1);
        assert_eq!(vm.rows[0].model, "opus");
        assert_eq!(vm.query, "opus");
    }

    #[test]
    fn query_filters_by_provider_id() {
        let out = make_output();
        let vm = present_pricing(&out, "openai");
        assert_eq!(vm.rows.len(), 1);
        assert_eq!(vm.rows[0].model, "gpt-4o");
    }

    #[test]
    fn query_filters_by_alias() {
        let out = make_output();
        let vm = present_pricing(&out, "sonnet-4-5");
        assert_eq!(vm.rows.len(), 1);
        assert_eq!(vm.rows[0].model, "claude-sonnet-4-5");
    }

    #[test]
    fn no_match_produces_empty_vm_with_query_set() {
        let out = make_output();
        let vm = present_pricing(&out, "zzznomatch");
        assert!(vm.empty);
        assert!(vm.rows.is_empty());
        assert_eq!(vm.query, "zzznomatch");
    }
}
