//! Per-provider thinking level: the closed catalog of levels each upstream
//! actually supports and the request patch each one produces.

use crate::config::{ProviderKind, ThinkingLevel};
use bytes::Bytes;
use serde_json::{Value, json};

/// The provider-specific request patch for one thinking level, in each wire
/// format that kind can serve. `None` means the kind has no endpoint of that
/// format in `default_urls`, so the branch is never consulted.
#[derive(Debug, Clone, PartialEq)]
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
        ProviderKind::OpencodeGo => &[],
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
        // together are rejected above effort `high`. `budget_tokens: null`
        // deletes any budget the client sent under `force`: Anthropic rejects
        // `budget_tokens` alongside `type: "disabled"` with a 400, so a plain
        // RFC 7396 merge without the null would leave a rejected body behind.
        // Don't "clean up" this null — it is load-bearing.
        (ProviderKind::Anthropic, ThinkingLevel::Off) => {
            anthropic_only(json!({"thinking": {"type": "disabled", "budget_tokens": null}}))
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
        // See the Anthropic `Off` arm above for why `budget_tokens` is nulled.
        (ProviderKind::Zai, ThinkingLevel::Off) => {
            both(json!({"thinking": {"type": "disabled", "budget_tokens": null}}))
        }
        // Kimi rejects a request carrying both `thinking` and
        // `reasoning_effort` together, and Claude Code clients routinely send
        // `thinking` — null it out so the merge removes it. Z.ai (GLM)
        // tolerates both keys, so it keeps the plain patch.
        (ProviderKind::Kimi, _) => both(json!({"reasoning_effort": effort, "thinking": null})),
        (ProviderKind::Zai, _) => both(json!({"reasoning_effort": effort})),
        // See the Anthropic `Off` arm above for why `budget_tokens` is nulled.
        (ProviderKind::Minimax, ThinkingLevel::Off) => {
            both(json!({"thinking": {"type": "disabled", "budget_tokens": null}}))
        }
        (ProviderKind::Minimax, _) => both(json!({"thinking": {"type": "adaptive"}})),
        // OpencodeGo offers no thinking levels, so this arm never actually
        // runs — the guard above already returns `None` first. Expressed as
        // a plain `None` return (not `unreachable!()`) so a future change
        // that adds a level here without updating this arm degrades to no
        // patch instead of panicking in the request hot path.
        (ProviderKind::OpencodeGo, _) => return None,
    })
}

/// RFC 7396 JSON Merge Patch. Objects merge recursively, `null` deletes the
/// key, anything else replaces. Chosen over replacing whole top-level keys so
/// that setting `output_config.effort` cannot wipe a client's
/// `output_config.format`.
pub fn merge_patch(target: &mut Value, patch: &Value) {
    let Some(patch_obj) = patch.as_object() else {
        *target = patch.clone();
        return;
    };
    if !target.is_object() {
        *target = Value::Object(serde_json::Map::new());
    }
    let target_obj = target
        .as_object_mut()
        .expect("target was just coerced to an object");
    for (key, value) in patch_obj {
        if value.is_null() {
            target_obj.remove(key);
        } else {
            let slot = target_obj.entry(key.clone()).or_insert(Value::Null);
            merge_patch(slot, value);
        }
    }
}

/// Drop every leaf of `patch` whose path already exists in `body`, so a
/// non-forcing provider level acts as a default and never overwrites what the
/// client asked for. Returns `None` when nothing is left to apply.
pub fn prune_existing(patch: &Value, body: &Value) -> Option<Value> {
    let (Some(patch_obj), Some(body_obj)) = (patch.as_object(), body.as_object()) else {
        return Some(patch.clone());
    };
    let mut out = serde_json::Map::new();
    for (key, value) in patch_obj {
        match body_obj.get(key) {
            None => {
                out.insert(key.clone(), value.clone());
            }
            Some(existing) if value.is_object() && existing.is_object() => {
                if let Some(nested) = prune_existing(value, existing) {
                    out.insert(key.clone(), nested);
                }
            }
            // The client already set this leaf — leave theirs in place.
            Some(_) => {}
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(Value::Object(out))
    }
}

/// One provider's resolved thinking patch for one wire format, plus whether it
/// overrides the client or merely fills in what the client omitted.
#[derive(Debug, Clone)]
pub struct ThinkingInjection {
    patch: Value,
    force: bool,
}

impl ThinkingInjection {
    pub fn new(patch: Value, force: bool) -> Self {
        Self { patch, force }
    }

    /// Merge the patch into a JSON request body. A body that isn't JSON, or
    /// that would fail to re-serialize, passes through untouched.
    pub fn apply(&self, body: Bytes) -> Bytes {
        let Ok(mut value) = serde_json::from_slice::<Value>(&body) else {
            return body;
        };
        let effective = if self.force {
            self.patch.clone()
        } else {
            match prune_existing(&self.patch, &value) {
                Some(patch) => patch,
                None => return body,
            }
        };
        merge_patch(&mut value, &effective);
        serde_json::to_vec(&value).map(Bytes::from).unwrap_or(body)
    }
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
        assert_eq!(
            p.anthropic,
            Some(json!({"thinking": {"type": "disabled", "budget_tokens": null}}))
        );
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
        assert_eq!(
            off.openai,
            Some(json!({"thinking": {"type": "disabled", "budget_tokens": null}}))
        );
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
            Some(json!({"thinking": {"type": "disabled", "budget_tokens": null}}))
        );
    }

    #[test]
    fn kimi_and_deepseek_use_reasoning_effort() {
        let kimi = thinking_patch(ProviderKind::Kimi, ThinkingLevel::Low).unwrap();
        assert_eq!(
            kimi.openai,
            Some(json!({"reasoning_effort": "low", "thinking": null}))
        );

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
        // OpencodeGo offers no thinking levels at all.
        assert!(thinking_patch(ProviderKind::OpencodeGo, ThinkingLevel::Off).is_none());
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

    use bytes::Bytes;

    #[test]
    fn merge_patch_replaces_scalars_and_recurses_into_objects() {
        let mut target = json!({"output_config": {"format": {"type": "json_schema"}}});
        merge_patch(&mut target, &json!({"output_config": {"effort": "high"}}));
        assert_eq!(
            target,
            json!({"output_config": {"format": {"type": "json_schema"}, "effort": "high"}}),
            "a sibling key the client sent must survive"
        );
    }

    #[test]
    fn merge_patch_deletes_keys_set_to_null() {
        let mut target = json!({"thinking": {"type": "enabled", "budget_tokens": 8000}});
        merge_patch(
            &mut target,
            &json!({"thinking": {"type": "disabled", "budget_tokens": null}}),
        );
        assert_eq!(target, json!({"thinking": {"type": "disabled"}}));
    }

    #[test]
    fn prune_existing_drops_leaves_the_body_already_has() {
        let patch = json!({"output_config": {"effort": "high"}, "reasoning_effort": "high"});
        let body = json!({"output_config": {"effort": "low"}});
        assert_eq!(
            prune_existing(&patch, &body),
            Some(json!({"reasoning_effort": "high"})),
            "effort was already set by the client; only the untouched key remains"
        );
    }

    #[test]
    fn prune_existing_returns_none_when_the_body_covers_everything() {
        let patch = json!({"reasoning_effort": "high"});
        let body = json!({"reasoning_effort": "low"});
        assert_eq!(prune_existing(&patch, &body), None);
    }

    #[test]
    fn injection_without_force_leaves_a_client_value_alone() {
        let inj = ThinkingInjection::new(json!({"reasoning_effort": "max"}), false);
        let out = inj.apply(Bytes::from(r#"{"model":"m","reasoning_effort":"low"}"#));
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["reasoning_effort"], "low");
    }

    #[test]
    fn injection_without_force_fills_in_a_missing_value() {
        let inj = ThinkingInjection::new(json!({"reasoning_effort": "max"}), false);
        let out = inj.apply(Bytes::from(r#"{"model":"m"}"#));
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["reasoning_effort"], "max");
    }

    #[test]
    fn injection_with_force_overrides_a_client_value() {
        let inj = ThinkingInjection::new(json!({"reasoning_effort": "max"}), true);
        let out = inj.apply(Bytes::from(r#"{"model":"m","reasoning_effort":"low"}"#));
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["reasoning_effort"], "max");
    }

    #[test]
    fn injection_passes_non_json_bodies_through_untouched() {
        let inj = ThinkingInjection::new(json!({"reasoning_effort": "max"}), true);
        let body = Bytes::from_static(b"not json at all");
        assert_eq!(inj.apply(body.clone()), body);
    }

    /// Every other force test above merges a flat scalar patch into a flat
    /// body, so none of them exercise a nested patch meeting a sibling key
    /// under the same object. The real Anthropic `off` patch is nested
    /// (`thinking.budget_tokens: null` alongside a client-sent
    /// `thinking.type`), and that's exactly the shape that hid finding 1.
    #[test]
    fn injection_with_force_deletes_a_nested_sibling_key() {
        let patch = thinking_patch(ProviderKind::Anthropic, ThinkingLevel::Off)
            .unwrap()
            .anthropic
            .unwrap();
        let inj = ThinkingInjection::new(patch, true);
        let out = inj.apply(Bytes::from(
            r#"{"model":"claude-opus-5","thinking":{"type":"enabled","budget_tokens":8000}}"#,
        ));
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["thinking"], json!({"type": "disabled"}));
    }
}
