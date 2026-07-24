//! Per-provider thinking level: the closed catalog of levels each upstream
//! actually supports and the request patch each one produces.

use crate::config::{ProviderKind, ThinkingLevel};
use serde_json::{Value, json};

/// The provider-specific request patch for one thinking level, in each wire
/// format that kind can serve. `None` means the kind has no endpoint of that
/// format in `default_urls`, so the branch is never consulted.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ThinkingPatch {
    pub anthropic: Option<Value>,
    pub openai: Option<Value>,
}

fn both(v: Value) -> ThinkingPatch {
    ThinkingPatch {
        anthropic: Some(v.clone()),
        openai: Some(v),
    }
}

fn anthropic_only(v: Value) -> ThinkingPatch {
    ThinkingPatch {
        anthropic: Some(v),
        openai: None,
    }
}

fn openai_only(v: Value) -> ThinkingPatch {
    ThinkingPatch {
        anthropic: None,
        openai: Some(v),
    }
}

/// Levels this kind's upstream actually supports. Drives both the admin API
/// validation and the TUI dropdown, so the two cannot disagree. `Unset` is
/// valid for every kind and is deliberately absent from these lists.
///
/// Researched 2026-07-25; see the spec for the source per provider. DeepSeek
/// accepts `low`/`medium` but maps them to `high` server-side, so offering
/// them would be a lie.
pub fn thinking_levels(kind: ProviderKind) -> &'static [ThinkingLevel] {
    use ThinkingLevel::*;
    match kind {
        ProviderKind::Anthropic => &[Off, Low, Medium, High, XHigh, Max],
        ProviderKind::Codex | ProviderKind::OpenAi => &[Off, Minimal, Low, Medium, High, XHigh],
        ProviderKind::Zai => &[Off, High, Max],
        ProviderKind::DeepSeek => &[High, Max],
        ProviderKind::Kimi => &[Low, High, Max],
        ProviderKind::Minimax => &[Off, Adaptive],
    }
}

/// The request patch for a `(kind, level)` pair, or `None` when the level is
/// `Unset` or outside the kind's offered list.
pub fn thinking_patch(kind: ProviderKind, level: ThinkingLevel) -> Option<ThinkingPatch> {
    if !thinking_levels(kind).contains(&level) {
        return None;
    }
    let effort = level.as_str();
    Some(match (kind, level) {
        // Anthropic disables thinking without naming an effort: the two keys
        // together are rejected above effort `high`.
        (ProviderKind::Anthropic, ThinkingLevel::Off) => {
            anthropic_only(json!({"thinking": {"type": "disabled"}}))
        }
        (ProviderKind::Anthropic, _) => {
            anthropic_only(json!({"output_config": {"effort": effort}}))
        }
        // Codex and OpenAI spell "no reasoning" as an effort value.
        (ProviderKind::Codex | ProviderKind::OpenAi, ThinkingLevel::Off) => {
            openai_only(json!({"reasoning_effort": "none"}))
        }
        (ProviderKind::Codex | ProviderKind::OpenAi | ProviderKind::DeepSeek, _) => {
            openai_only(json!({"reasoning_effort": effort}))
        }
        (ProviderKind::Zai, ThinkingLevel::Off) => both(json!({"thinking": {"type": "disabled"}})),
        (ProviderKind::Zai | ProviderKind::Kimi, _) => both(json!({"reasoning_effort": effort})),
        (ProviderKind::Minimax, ThinkingLevel::Off) => {
            both(json!({"thinking": {"type": "disabled"}}))
        }
        (ProviderKind::Minimax, _) => both(json!({"thinking": {"type": "adaptive"}})),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ProviderKind, ThinkingLevel};
    use serde_json::json;

    #[test]
    fn anthropic_effort_lands_in_output_config() {
        let p = thinking_patch(ProviderKind::Anthropic, ThinkingLevel::High).unwrap();
        assert_eq!(
            p.anthropic,
            Some(json!({"output_config": {"effort": "high"}}))
        );
        // Anthropic-format endpoint only — see default_urls.
        assert_eq!(p.openai, None);
    }

    /// Disabled thinking carries no effort key on purpose: Opus 5 rejects
    /// disabled thinking at effort xhigh/max, and omitting it leaves the
    /// account default (high), which is accepted.
    #[test]
    fn anthropic_off_disables_thinking_without_touching_effort() {
        let p = thinking_patch(ProviderKind::Anthropic, ThinkingLevel::Off).unwrap();
        assert_eq!(p.anthropic, Some(json!({"thinking": {"type": "disabled"}})));
    }

    #[test]
    fn codex_and_openai_use_reasoning_effort_with_none_for_off() {
        for kind in [ProviderKind::Codex, ProviderKind::OpenAi] {
            let high = thinking_patch(kind, ThinkingLevel::High).unwrap();
            assert_eq!(high.openai, Some(json!({"reasoning_effort": "high"})));
            assert_eq!(high.anthropic, None);

            let off = thinking_patch(kind, ThinkingLevel::Off).unwrap();
            assert_eq!(off.openai, Some(json!({"reasoning_effort": "none"})));
        }
    }

    #[test]
    fn zai_serves_both_formats_and_disables_via_thinking_type() {
        let max = thinking_patch(ProviderKind::Zai, ThinkingLevel::Max).unwrap();
        assert_eq!(max.anthropic, Some(json!({"reasoning_effort": "max"})));
        assert_eq!(max.openai, Some(json!({"reasoning_effort": "max"})));

        let off = thinking_patch(ProviderKind::Zai, ThinkingLevel::Off).unwrap();
        assert_eq!(off.openai, Some(json!({"thinking": {"type": "disabled"}})));
    }

    #[test]
    fn minimax_uses_thinking_type_for_both_levels() {
        let adaptive = thinking_patch(ProviderKind::Minimax, ThinkingLevel::Adaptive).unwrap();
        assert_eq!(
            adaptive.openai,
            Some(json!({"thinking": {"type": "adaptive"}}))
        );

        let off = thinking_patch(ProviderKind::Minimax, ThinkingLevel::Off).unwrap();
        assert_eq!(
            off.anthropic,
            Some(json!({"thinking": {"type": "disabled"}}))
        );
    }

    #[test]
    fn kimi_and_deepseek_use_reasoning_effort() {
        let kimi = thinking_patch(ProviderKind::Kimi, ThinkingLevel::Low).unwrap();
        assert_eq!(kimi.openai, Some(json!({"reasoning_effort": "low"})));

        let deepseek = thinking_patch(ProviderKind::DeepSeek, ThinkingLevel::Max).unwrap();
        assert_eq!(deepseek.openai, Some(json!({"reasoning_effort": "max"})));
        assert_eq!(deepseek.anthropic, None);
    }

    #[test]
    fn unset_and_out_of_range_levels_produce_no_patch() {
        assert!(thinking_patch(ProviderKind::Zai, ThinkingLevel::Unset).is_none());
        // DeepSeek maps low/medium to high server-side, so they are not offered.
        assert!(thinking_patch(ProviderKind::DeepSeek, ThinkingLevel::Low).is_none());
        // Kimi K3 has no `max`-less floor below `low` and no `off`.
        assert!(thinking_patch(ProviderKind::Kimi, ThinkingLevel::Off).is_none());
        assert!(thinking_patch(ProviderKind::Minimax, ThinkingLevel::High).is_none());
    }

    #[test]
    fn every_offered_level_has_a_patch_with_at_least_one_branch() {
        for kind in [
            ProviderKind::Anthropic,
            ProviderKind::Codex,
            ProviderKind::OpenAi,
            ProviderKind::Zai,
            ProviderKind::DeepSeek,
            ProviderKind::Kimi,
            ProviderKind::Minimax,
        ] {
            for level in thinking_levels(kind) {
                let p = thinking_patch(kind, *level)
                    .unwrap_or_else(|| panic!("{kind:?} offers {level:?} but has no patch"));
                assert!(
                    p.anthropic.is_some() || p.openai.is_some(),
                    "{kind:?}/{level:?} has no branch at all"
                );
            }
        }
    }

    #[test]
    fn unset_is_never_listed_as_an_offered_level() {
        for kind in [
            ProviderKind::Anthropic,
            ProviderKind::Kimi,
            ProviderKind::Minimax,
        ] {
            assert!(!thinking_levels(kind).contains(&ThinkingLevel::Unset));
        }
    }
}
