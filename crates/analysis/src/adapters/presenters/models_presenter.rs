use crate::adapters::presenters::formatting::{fmt_cost, fmt_num};
use crate::adapters::view_models::models_vm::{ModelRowVM, ModelsViewModel};
use crate::application::dto::GetModelsBreakdownOutput;

pub fn present_models(out: &GetModelsBreakdownOutput) -> ModelsViewModel {
    let rows: Vec<ModelRowVM> = out
        .models
        .iter()
        .map(|m| {
            let cost_cell = if m.tokens.total().value() == 0 {
                "—".to_string()
            } else if out.unpriced_models.contains(&m.model) && m.cost.value() == 0.0 {
                "?".to_string()
            } else {
                fmt_cost(m.cost.value())
            };
            ModelRowVM {
                model: m.model.as_str().to_string(),
                messages: fmt_num(m.message_count),
                input: fmt_num(m.tokens.input.value()),
                output: fmt_num(m.tokens.output.value()),
                reasoning: fmt_num(m.tokens.reasoning.value()),
                cost: cost_cell,
            }
        })
        .collect();

    let pricing_note = if out.missing_pricing_count > 0 {
        Some(format!(
            "· {} models missing pricing",
            out.missing_pricing_count
        ))
    } else {
        None
    };

    ModelsViewModel {
        empty: rows.is_empty(),
        rows,
        pricing_note,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::dto::Filter;
    use shared::domain::entities::ModelUsage;
    use shared::domain::value_objects::{Cost, ModelId, TokenBreakdown, TokenCount};
    use std::collections::HashSet;

    fn output(models: Vec<ModelUsage>, missing: usize) -> GetModelsBreakdownOutput {
        GetModelsBreakdownOutput {
            filter_applied: Filter::default(),
            models,
            missing_pricing_count: missing,
            unpriced_models: HashSet::new(),
            last_pricing_sync: None,
        }
    }

    #[test]
    fn empty_input_produces_empty_vm() {
        let vm = present_models(&output(vec![], 0));
        assert!(vm.empty);
        assert!(vm.pricing_note.is_none());
    }

    #[test]
    fn rows_are_formatted() {
        let vm = present_models(&output(
            vec![ModelUsage {
                model: ModelId::new("anthropic/opus").unwrap(),
                message_count: 1234,
                tokens: TokenBreakdown {
                    input: TokenCount::new(50000),
                    output: TokenCount::new(12345),
                    reasoning: TokenCount::new(0),
                    ..Default::default()
                },
                cost: Cost::new(12.5).unwrap(),
            }],
            0,
        ));
        assert_eq!(vm.rows.len(), 1);
        assert_eq!(vm.rows[0].model, "anthropic/opus");
        assert_eq!(vm.rows[0].cost, "$12.50");
    }

    #[test]
    fn missing_count_populates_pricing_note() {
        let vm = present_models(&output(vec![], 3));
        assert_eq!(
            vm.pricing_note,
            Some("· 3 models missing pricing".to_string())
        );
    }

    #[test]
    fn zero_tokens_model_shows_dash() {
        let vm = present_models(&output(
            vec![ModelUsage {
                model: ModelId::new("idle/model").unwrap(),
                message_count: 1,
                tokens: TokenBreakdown::default(),
                cost: Cost::new(0.0).unwrap(),
            }],
            0,
        ));
        assert_eq!(vm.rows[0].cost, "—");
    }

    #[test]
    fn unpriced_model_with_tokens_shows_question_mark() {
        let model_id = ModelId::new("zai/glm-5.1").unwrap();
        let mut unpriced = HashSet::new();
        unpriced.insert(model_id.clone());
        let out = GetModelsBreakdownOutput {
            filter_applied: Filter::default(),
            models: vec![ModelUsage {
                model: model_id,
                message_count: 5,
                tokens: TokenBreakdown {
                    input: TokenCount::new(200),
                    ..Default::default()
                },
                cost: Cost::new(0.0).unwrap(),
            }],
            missing_pricing_count: 1,
            unpriced_models: unpriced,
            last_pricing_sync: None,
        };
        let vm = present_models(&out);
        assert_eq!(vm.rows[0].cost, "?");
    }
}
