# Per-provider Thinking Level Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let every provider kind carry a thinking level chosen from a closed, researched per-kind list, applied to the outgoing request as that upstream's own wire syntax.

**Architecture:** A pure data catalog maps `(ProviderKind, ThinkingLevel)` to the provider's request patch in each wire format. The proxy merges the branch matching the format actually sent upstream, using RFC 7396 JSON Merge Patch, inside the existing request-quirk hooks. A per-provider `thinking_force` flag decides whether the patch overrides what the client sent or only fills in what the client omitted. The old `reasoning_effort` field is migrated into the new one and removed.

**Tech Stack:** Rust (edition 2024), `serde_json` for the patch and merge, `rusqlite` for storage, `ratatui` for the TUI form, no new dependencies.

**Spec:** `docs/superpowers/specs/2026-07-25-provider-thinking-level-design.md`

## Global Constraints

- Branch base is `fix/minimax-format-mode` (PR #18). The highest existing migration is **V11**; the new one is **V12**.
- Edition 2024: prefer let-chains (`if let Some(x) = opt && cond`) over nested `if let`. Clippy's `collapsible_if` is denied.
- `cargo fmt` before every commit; `cargo clippy --workspace --all-targets -- -D warnings` must be clean.
- The pre-commit hook runs the full `cargo test` suite — allow up to 5 minutes per commit.
- Tests live beside the code in `#[cfg(test)] mod tests`, not in a separate tests tree, unless the file already has a crate-level integration test.
- No new crate dependencies. No `anyhow` — each ring keeps its own error enum.
- `crates/proxy-tui` does **not** depend on `crates/proxy`. Any table the TUI needs is duplicated there, exactly as `ReasoningEffortInput::CODEX_EFFORTS` duplicates the proxy-side list today.
- Commit messages follow Conventional Commits. Never add `Co-Authored-By` or any attribution line.

---

### Task 1: ThinkingLevel enum and the per-kind catalog

**Files:**
- Modify: `crates/proxy/src/config.rs` (add enum next to `ThinkingMode`, around line 101)
- Create: `crates/proxy/src/adapters/providers/thinking.rs`
- Modify: `crates/proxy/src/adapters/providers/mod.rs` (register the module)

**Interfaces:**
- Consumes: `crate::config::ProviderKind` (existing).
- Produces: `config::ThinkingLevel` (`Unset`/`Off`/`Minimal`/`Low`/`Medium`/`High`/`XHigh`/`Max`/`Adaptive`, with `as_str()` and `parse()`); `adapters::providers::thinking::{ThinkingPatch, thinking_levels, thinking_patch}`.

- [ ] **Step 1: Add the level enum to `config.rs`**

Insert directly above the existing `ThinkingMode` enum:

```rust
/// How hard the upstream model should think, chosen per provider from the
/// closed list its API actually supports (see
/// `adapters::providers::thinking::thinking_levels`).
///
/// `Unset` means the proxy sends nothing and the upstream default applies.
/// The variants are a union across providers — no single provider offers all
/// of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingLevel {
    #[default]
    Unset,
    Off,
    Minimal,
    Low,
    Medium,
    High,
    #[serde(rename = "xhigh")]
    XHigh,
    Max,
    Adaptive,
}

impl ThinkingLevel {
    /// Wire and storage spelling. Also the value shown in the TUI and accepted
    /// by the admin API.
    pub fn as_str(self) -> &'static str {
        match self {
            ThinkingLevel::Unset => "unset",
            ThinkingLevel::Off => "off",
            ThinkingLevel::Minimal => "minimal",
            ThinkingLevel::Low => "low",
            ThinkingLevel::Medium => "medium",
            ThinkingLevel::High => "high",
            ThinkingLevel::XHigh => "xhigh",
            ThinkingLevel::Max => "max",
            ThinkingLevel::Adaptive => "adaptive",
        }
    }

    /// Parse a stored or wire value. Unknown input yields `None` so callers can
    /// reject it rather than silently defaulting.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "unset" | "" => Some(ThinkingLevel::Unset),
            "off" => Some(ThinkingLevel::Off),
            "minimal" => Some(ThinkingLevel::Minimal),
            "low" => Some(ThinkingLevel::Low),
            "medium" => Some(ThinkingLevel::Medium),
            "high" => Some(ThinkingLevel::High),
            "xhigh" => Some(ThinkingLevel::XHigh),
            "max" => Some(ThinkingLevel::Max),
            "adaptive" => Some(ThinkingLevel::Adaptive),
            _ => None,
        }
    }
}
```

- [ ] **Step 2: Write the failing catalog tests**

Create `crates/proxy/src/adapters/providers/thinking.rs` containing only this test module for now:

```rust
//! Per-provider thinking level: the closed catalog of levels each upstream
//! actually supports and the request patch each one produces.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ProviderKind, ThinkingLevel};
    use serde_json::json;

    #[test]
    fn anthropic_effort_lands_in_output_config() {
        let p = thinking_patch(ProviderKind::Anthropic, ThinkingLevel::High).unwrap();
        assert_eq!(p.anthropic, Some(json!({"output_config": {"effort": "high"}})));
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
        assert_eq!(adaptive.openai, Some(json!({"thinking": {"type": "adaptive"}})));

        let off = thinking_patch(ProviderKind::Minimax, ThinkingLevel::Off).unwrap();
        assert_eq!(off.anthropic, Some(json!({"thinking": {"type": "disabled"}})));
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
        for kind in [ProviderKind::Anthropic, ProviderKind::Kimi, ProviderKind::Minimax] {
            assert!(!thinking_levels(kind).contains(&ThinkingLevel::Unset));
        }
    }
}
```

Register the module in `crates/proxy/src/adapters/providers/mod.rs`, next to the other `pub mod` lines:

```rust
pub mod thinking;
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p proxy --lib thinking::`
Expected: FAIL to compile — `cannot find function 'thinking_patch' in this scope`.

- [ ] **Step 4: Write the catalog**

Insert above the test module in `crates/proxy/src/adapters/providers/thinking.rs`:

```rust
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
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p proxy --lib thinking::`
Expected: PASS, 8 tests.

- [ ] **Step 6: Format, lint, commit**

```bash
cargo fmt
cargo clippy --workspace --all-targets -- -D warnings
git add crates/proxy/src/config.rs crates/proxy/src/adapters/providers/thinking.rs crates/proxy/src/adapters/providers/mod.rs
git commit -m "feat(proxy): add per-kind thinking level catalog"
```

---

### Task 2: RFC 7396 merge and the injection applier

**Files:**
- Modify: `crates/proxy/src/adapters/providers/thinking.rs`

**Interfaces:**
- Consumes: `ThinkingPatch` from Task 1.
- Produces: `ThinkingInjection::new(patch: Value, force: bool)` with `apply(&self, body: Bytes) -> Bytes`; `merge_patch(&mut Value, &Value)`; `prune_existing(&Value, &Value) -> Option<Value>`.

- [ ] **Step 1: Write the failing merge tests**

Append to the existing `mod tests` in `thinking.rs`:

```rust
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
        merge_patch(&mut target, &json!({"thinking": {"type": "disabled", "budget_tokens": null}}));
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p proxy --lib thinking::`
Expected: FAIL to compile — `cannot find function 'merge_patch'`.

- [ ] **Step 3: Write the merge engine**

Append to the non-test part of `thinking.rs`:

```rust
use bytes::Bytes;

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
        serde_json::to_vec(&value)
            .map(Bytes::from)
            .unwrap_or(body)
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p proxy --lib thinking::`
Expected: PASS, 16 tests.

- [ ] **Step 5: Format, lint, commit**

```bash
cargo fmt
cargo clippy --workspace --all-targets -- -D warnings
git add crates/proxy/src/adapters/providers/thinking.rs
git commit -m "feat(proxy): add RFC 7396 merge and thinking injection applier"
```

---

### Task 3: Apply the injection in UpstreamProvider

**Files:**
- Modify: `crates/proxy/src/adapters/providers/upstream.rs`

**Interfaces:**
- Consumes: `ThinkingInjection` from Task 2.
- Produces: `UpstreamProvider::with_thinking(anthropic: Option<ThinkingInjection>, openai: Option<ThinkingInjection>) -> Self` (builder-style, mirrors the existing `with_format_mode`).

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` in `upstream.rs`:

```rust
    #[test]
    fn anthropic_path_merges_the_anthropic_thinking_branch() {
        let p = UpstreamProvider::new(
            "anthropic".to_string(),
            Some("https://api.anthropic.com".to_string()),
            None,
            AuthHeader::Bearer("k".to_string()),
            Quirks::none(),
            reqwest::Client::new(),
        )
        .with_thinking(
            Some(super::super::thinking::ThinkingInjection::new(
                serde_json::json!({"output_config": {"effort": "high"}}),
                true,
            )),
            None,
        );
        let out = p.apply_anthropic_request_quirks(Bytes::from(r#"{"model":"claude-opus-5"}"#));
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["output_config"]["effort"], "high");
    }

    #[test]
    fn openai_path_merges_the_openai_thinking_branch() {
        let p = UpstreamProvider::new(
            "deepseek".to_string(),
            None,
            Some("https://api.deepseek.com/v1".to_string()),
            AuthHeader::Bearer("k".to_string()),
            Quirks::none(),
            reqwest::Client::new(),
        )
        .with_thinking(
            None,
            Some(super::super::thinking::ThinkingInjection::new(
                serde_json::json!({"reasoning_effort": "max"}),
                true,
            )),
        );
        let out = p.apply_openai_request_quirks(Bytes::from(r#"{"model":"deepseek-v4-pro"}"#));
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["reasoning_effort"], "max");
    }

    /// The two branches are independent: an Anthropic-only injection must not
    /// leak into an OpenAI-format body.
    #[test]
    fn each_format_only_sees_its_own_branch() {
        let p = UpstreamProvider::new(
            "zai".to_string(),
            Some("https://api.z.ai/api/anthropic".to_string()),
            Some("https://api.z.ai/api/paas/v4".to_string()),
            AuthHeader::Bearer("k".to_string()),
            Quirks::none(),
            reqwest::Client::new(),
        )
        .with_thinking(
            Some(super::super::thinking::ThinkingInjection::new(
                serde_json::json!({"anthropic_only": true}),
                true,
            )),
            None,
        );
        let out = p.apply_openai_request_quirks(Bytes::from(r#"{"model":"glm-5.2"}"#));
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert!(v.get("anthropic_only").is_none());
    }

    #[test]
    fn no_injection_leaves_the_body_byte_identical() {
        let p = UpstreamProvider::new(
            "zai".to_string(),
            Some("https://api.z.ai/api/anthropic".to_string()),
            None,
            AuthHeader::Bearer("k".to_string()),
            Quirks::none(),
            reqwest::Client::new(),
        );
        let body = Bytes::from(r#"{"model":"glm-5.2"}"#);
        assert_eq!(p.apply_anthropic_request_quirks(body.clone()), body);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p proxy --lib upstream::tests::`
Expected: FAIL to compile — `no method named 'with_thinking'`.

- [ ] **Step 3: Add the fields and the builder method**

In `upstream.rs`, add two fields to the struct and initialise them in `new`:

```rust
pub struct UpstreamProvider {
    name: String,
    anthropic_base_url: Option<String>,
    openai_base_url: Option<String>,
    auth: AuthHeader,
    quirks: Quirks,
    format_mode: FormatMode,
    thinking_anthropic: Option<super::thinking::ThinkingInjection>,
    thinking_openai: Option<super::thinking::ThinkingInjection>,
    http: reqwest::Client,
}
```

In `new`, set both to `None` alongside `format_mode: FormatMode::Both`. Then add the builder method next to `with_format_mode`:

```rust
    /// Attach the provider's resolved thinking patch, one branch per wire
    /// format. Each branch is applied only on requests actually sent in that
    /// format, after `format_mode` and any translation have run.
    pub fn with_thinking(
        mut self,
        anthropic: Option<super::thinking::ThinkingInjection>,
        openai: Option<super::thinking::ThinkingInjection>,
    ) -> Self {
        self.thinking_anthropic = anthropic;
        self.thinking_openai = openai;
        self
    }
```

- [ ] **Step 4: Apply the injections in both quirk hooks**

In `apply_anthropic_request_quirks`, add before the final `body`:

```rust
        if let Some(injection) = &self.thinking_anthropic {
            body = injection.apply(body);
        }
```

In `apply_openai_request_quirks`, add the same with `thinking_openai`:

```rust
        if let Some(injection) = &self.thinking_openai {
            body = injection.apply(body);
        }
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p proxy --lib upstream::tests::`
Expected: PASS, including the four new tests.

- [ ] **Step 6: Format, lint, commit**

```bash
cargo fmt
cargo clippy --workspace --all-targets -- -D warnings
git add crates/proxy/src/adapters/providers/upstream.rs
git commit -m "feat(proxy): apply the thinking patch per wire format in UpstreamProvider"
```

---

### Task 4: Apply the injection in CodexProvider

**Files:**
- Modify: `crates/proxy/src/adapters/providers/codex.rs` (struct around line 27, `forward_openai` around line 130, `translate_request_with_default_reasoning_effort` around line 451)

**Interfaces:**
- Consumes: `ThinkingInjection` from Task 2.
- Produces: `CodexProvider::configure_with_thinking(http, base_url: Option<String>, auth: AuthHeader, thinking: Option<ThinkingInjection>) -> Self`; `translate_request(chat: &Value)` keeps its existing single-argument signature.

- [ ] **Step 1: Write the failing test**

Add to the `mod tests` in `codex.rs`:

```rust
    /// Codex takes a Chat Completions body and translates it to the Responses
    /// API. The patch must land before that translation so the existing
    /// `reasoning_effort` → `reasoning.effort` mapping picks it up.
    #[test]
    fn thinking_patch_reaches_reasoning_effort_through_translation() {
        let injection = crate::adapters::providers::thinking::ThinkingInjection::new(
            serde_json::json!({"reasoning_effort": "xhigh"}),
            true,
        );
        let patched = injection.apply(Bytes::from(
            r#"{"model":"gpt-5.5","messages":[{"role":"user","content":"hi"}]}"#,
        ));
        let chat: Value = serde_json::from_slice(&patched).unwrap();
        let responses = translate_request(&chat).unwrap();
        assert_eq!(responses["reasoning"]["effort"], "xhigh");
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p proxy --lib codex::tests::thinking_patch_reaches_reasoning_effort_through_translation`
Expected: FAIL to compile — `translate_request` takes the default-effort argument, or the function is private with a different name.

- [ ] **Step 3: Swap the stored default for an injection**

In `codex.rs`, replace the `default_reasoning_effort: Option<String>` field with:

```rust
    thinking: Option<super::thinking::ThinkingInjection>,
```

Rename `configure_with_reasoning_effort` to `configure_with_thinking` and change its fourth parameter to `thinking: Option<super::thinking::ThinkingInjection>`; pass it through `build` unchanged. Update `new`, `with_base_url`, `with_auth`, and `configure` to pass `None`.

- [ ] **Step 4: Apply the patch before translating**

In `forward_openai`, apply the injection to the raw body before parsing:

```rust
        let body = match &self.thinking {
            Some(injection) => injection.apply(body),
            None => body,
        };

        // Parse incoming Chat Completions body
        let chat_body: Value = serde_json::from_slice(&body)
            .map_err(|e| ProxyError::BadRequest(format!("invalid JSON body: {e}")))?;

        // Translate to Responses API payload (always sets stream:true)
        let responses_body = translate_request(&chat_body)
            .map_err(|e| ProxyError::BadRequest(format!("codex request translation failed: {e}")))?;
```

- [ ] **Step 5: Collapse the translation helper**

Delete `translate_request_with_default_reasoning_effort` and fold its body into `translate_request(chat: &Value)`, dropping the default-effort parameter. In the reasoning block around line 538, keep only the client-value branch:

```rust
    // reasoning_effort → reasoning.effort
    if let Some(effort) = chat.get("reasoning_effort") {
        reasoning["effort"] = effort.clone();
    }
```

Update every existing call site and test that used the two-argument form to call `translate_request(chat)`.

- [ ] **Step 6: Run the Codex tests to verify they pass**

Run: `cargo test -p proxy --lib codex::`
Expected: PASS, including `translate_reasoning_effort` and the new test.

- [ ] **Step 7: Format, lint, commit**

```bash
cargo fmt
cargo clippy --workspace --all-targets -- -D warnings
git add crates/proxy/src/adapters/providers/codex.rs
git commit -m "feat(proxy): apply the thinking patch in CodexProvider before translation"
```

---

### Task 5: Config fields, migration V12, storage, and builder wiring

**Files:**
- Modify: `crates/proxy/src/config.rs` (`ProviderConfig`)
- Modify: `crates/proxy/src/adapters/storage/schema.rs` (new `MIGRATION_V12`)
- Modify: `crates/proxy/src/adapters/storage/db_config.rs` (insert + select)
- Modify: `crates/proxy/src/adapters/providers/builder.rs` (`build_leaf`)

**Interfaces:**
- Consumes: `thinking_patch` (Task 1), `ThinkingInjection` (Task 2), `with_thinking` (Tasks 3 and 4).
- Produces: `ProviderConfig::thinking_level: ThinkingLevel` and `ProviderConfig::thinking_force: bool`.

- [ ] **Step 1: Write the failing migration test**

Add to the `mod tests` in `schema.rs`:

```rust
    #[test]
    fn v12_adds_thinking_columns_and_backfills_reasoning_effort() {
        let conn = open_in_memory();
        ensure_current(&conn).unwrap();
        conn.execute(
            "INSERT INTO providers (name, kind, base_url, auth_type, reasoning_effort)
             VALUES ('anth','anthropic','','bearer','xhigh')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO providers (name, kind, base_url, auth_type, reasoning_effort)
             VALUES ('cdx','codex','','bearer','none')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO providers (name, kind, base_url, auth_type) VALUES ('zai','zai','','bearer')",
            [],
        )
        .unwrap();

        // The migration already ran, so re-run just its backfill statements the
        // way a pre-existing database would have seen them.
        for (target, sql) in MIGRATIONS.iter().filter(|(v, _)| *v == 12) {
            let _ = target;
            for stmt in sql.split(';').filter(|s| s.trim().starts_with("UPDATE")) {
                conn.execute(stmt, []).unwrap();
            }
        }

        let level = |name: &str| -> String {
            conn.query_row(
                "SELECT thinking_level FROM providers WHERE name = ?1",
                [name],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(level("anth"), "xhigh", "anthropic effort carries over verbatim");
        assert_eq!(level("cdx"), "off", "codex 'none' becomes 'off'");
        assert_eq!(level("zai"), "unset", "a row without an effort stays unset");

        let force: i64 = conn
            .query_row("SELECT thinking_force FROM providers WHERE name='anth'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(force, 0, "force defaults off so existing behaviour is preserved");
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p proxy --lib schema::tests::v12_adds_thinking_columns_and_backfills_reasoning_effort`
Expected: FAIL — `no such column: thinking_level`.

- [ ] **Step 3: Add migration V12**

In `schema.rs`, add `(12, MIGRATION_V12),` to the `MIGRATIONS` array and define it above `ensure_current`:

```rust
// Per-provider thinking level. `unset` means the proxy injects nothing and the
// upstream default applies, which is what every pre-V12 row did. The backfill
// moves the retired `reasoning_effort` values across; that column is left in
// place and no longer read, the same way V9 retired `base_url`.
const MIGRATION_V12: &str = r#"
ALTER TABLE providers ADD COLUMN thinking_level TEXT NOT NULL DEFAULT 'unset';
ALTER TABLE providers ADD COLUMN thinking_force INTEGER NOT NULL DEFAULT 0;

UPDATE providers
   SET thinking_level = reasoning_effort
 WHERE kind = 'anthropic'
   AND COALESCE(reasoning_effort, '') <> '';

UPDATE providers
   SET thinking_level = CASE reasoning_effort
        WHEN 'none' THEN 'off'
        ELSE reasoning_effort
       END
 WHERE kind = 'codex'
   AND COALESCE(reasoning_effort, '') <> '';
"#;
```

- [ ] **Step 4: Add the config fields**

In `config.rs`, add to `ProviderConfig` immediately after `format_mode`:

```rust
    /// How hard this provider's upstream should think. See [`ThinkingLevel`].
    #[serde(default)]
    pub thinking_level: ThinkingLevel,
    /// When true the level overrides whatever the client sent; when false it
    /// only fills in what the client omitted.
    #[serde(default)]
    pub thinking_force: bool,
```

Leave `reasoning_effort` in place for now — Task 8 removes it once every consumer is gone.

- [ ] **Step 5: Read and write the new columns**

In `db_config.rs`, add `thinking_level, thinking_force` to the `INSERT INTO providers` column list and `?17, ?18` to its `VALUES`, then push the two values after `format_mode`:

```rust
                    p.thinking_level.as_str(),
                    p.thinking_force as i64,
```

Add the same two columns to the `SELECT` in `load_providers` and read them at indexes 16 and 17:

```rust
                thinking_level: ThinkingLevel::parse(&row.get::<_, String>(16)?)
                    .unwrap_or_default(),
                thinking_force: row.get::<_, i64>(17)? != 0,
```

Add `ThinkingLevel` to the `use crate::config::{...}` list at the top of the file.

- [ ] **Step 6: Resolve the injection in the builder**

In `builder.rs`, inside `build_leaf`, compute the injections once and hand them to both provider types. Insert before the Codex early-return:

```rust
    let (thinking_anthropic, thinking_openai) = match super::thinking::thinking_patch(
        p.kind,
        p.thinking_level,
    ) {
        Some(patch) => (
            patch
                .anthropic
                .map(|v| super::thinking::ThinkingInjection::new(v, p.thinking_force)),
            patch
                .openai
                .map(|v| super::thinking::ThinkingInjection::new(v, p.thinking_force)),
        ),
        None => (None, None),
    };
```

Change the Codex branch to:

```rust
    if let ProviderKind::Codex = p.kind {
        return Ok(Arc::new(CodexProvider::configure_with_thinking(
            http,
            p.openai_base_url.clone(),
            auth,
            thinking_openai,
        )));
    }
```

And chain the builder call at the end of the function:

```rust
    Ok(Arc::new(
        UpstreamProvider::new(
            p.name.clone(),
            p.anthropic_base_url.clone(),
            p.openai_base_url.clone(),
            auth,
            quirks,
            http,
        )
        .with_format_mode(p.format_mode)
        .with_thinking(thinking_anthropic, thinking_openai),
    ))
```

- [ ] **Step 7: Fix every `ProviderConfig` literal**

Adding two fields breaks every struct literal in tests. Add `thinking_level: crate::config::ThinkingLevel::Unset,` and `thinking_force: false,` to each. Find them with:

```bash
cargo check --workspace --all-targets 2>&1 | grep "missing field" -A 2
```

In `crates/proxy/tests/integration.rs` and `crates/proxy/tests/quota_enforcement.rs` the path is `proxy::config::ThinkingLevel::Unset` — those files are outside the crate.

- [ ] **Step 8: Add a builder test**

Add to the `mod tests` in `builder.rs`:

```rust
    #[test]
    fn build_leaf_resolves_the_thinking_level_into_the_request() {
        let cfg = ProviderConfig {
            name: "deepseek".into(),
            kind: ProviderKind::DeepSeek,
            auth: AuthConfig::Bearer { value: "k".into() },
            anthropic_base_url: None,
            openai_base_url: Some("https://api.deepseek.com/v1".into()),
            format_mode: crate::config::FormatMode::Both,
            thinking_level: crate::config::ThinkingLevel::Max,
            thinking_force: true,
            reasoning_effort: None,
            thinking_mode: ThinkingMode::SplitOnly,
            max_concurrent: None,
            sanitize_empty_tools: false,
            enabled: true,
        };
        // Building must succeed and the provider must advertise the OpenAI
        // endpoint; the patch itself is covered by the UpstreamProvider tests.
        let provider = build_leaf(&cfg, reqwest::Client::new()).unwrap();
        assert!(provider.supported_formats().openai);
    }
```

- [ ] **Step 9: Run the full suite**

Run: `cargo test --workspace`
Expected: PASS, no failures.

- [ ] **Step 10: Format, lint, commit**

```bash
cargo fmt
cargo clippy --workspace --all-targets -- -D warnings
git add crates/proxy/src crates/proxy/tests
git commit -m "feat(proxy): store thinking level per provider and wire it into the builder"
```

---

### Task 6: Admin API surface

**Files:**
- Modify: `crates/proxy-admin-api/src/lib.rs` (`ProviderPayload`, around line 113)
- Modify: `crates/proxy/src/application/use_cases/admin.rs` (`config_to_payload` around line 697, `payload_to_config` around line 752)

**Interfaces:**
- Consumes: `thinking_levels` (Task 1), `ProviderConfig::thinking_level`/`thinking_force` (Task 5).
- Produces: `ProviderPayload::thinking_level: Option<String>` and `ProviderPayload::thinking_force: Option<bool>`; `reasoning_effort` is gone from the payload.

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` in `admin.rs`:

```rust
    #[test]
    fn payload_round_trips_the_thinking_level() {
        let cfg = config_with_thinking(
            ProviderKind::Kimi,
            crate::config::ThinkingLevel::High,
            true,
        );
        let payload = config_to_payload(&cfg);
        assert_eq!(payload.providers[0].thinking_level.as_deref(), Some("high"));
        assert_eq!(payload.providers[0].thinking_force, Some(true));

        let restored = payload_to_config(
            payload,
            PathBuf::from("/tmp/p.db"),
            PathBuf::from("/tmp/pr.db"),
            &cfg,
        )
        .unwrap();
        assert_eq!(restored.providers[0].thinking_level, crate::config::ThinkingLevel::High);
        assert!(restored.providers[0].thinking_force);
    }

    #[test]
    fn payload_to_config_rejects_a_level_the_kind_does_not_offer() {
        // DeepSeek maps low/medium to high server-side, so they are not offered.
        let mut payload = config_to_payload(&config_with_thinking(
            ProviderKind::DeepSeek,
            crate::config::ThinkingLevel::Max,
            false,
        ));
        payload.providers[0].thinking_level = Some("low".into());
        let err = payload_to_config(
            payload,
            PathBuf::from("/tmp/p.db"),
            PathBuf::from("/tmp/pr.db"),
            &Config::default_for_tests(),
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("thinking_level"), "got: {msg}");
        assert!(msg.contains("high"), "the error must list the valid levels: {msg}");
    }

    #[test]
    fn payload_to_config_treats_missing_and_empty_level_as_unset() {
        for value in [None, Some(String::new())] {
            let mut payload = config_to_payload(&config_with_thinking(
                ProviderKind::Zai,
                crate::config::ThinkingLevel::High,
                false,
            ));
            payload.providers[0].thinking_level = value;
            let restored = payload_to_config(
                payload,
                PathBuf::from("/tmp/p.db"),
                PathBuf::from("/tmp/pr.db"),
                &Config::default_for_tests(),
            )
            .unwrap();
            assert_eq!(
                restored.providers[0].thinking_level,
                crate::config::ThinkingLevel::Unset
            );
        }
    }
```

There is no shared `Config` constructor in this test module — the existing tests
(e.g. `config_to_payload_roundtrip` around line 1256) build `Config` literals
inline. Add this helper alongside them and use it for the `_existing` argument
too:

```rust
    fn config_with_thinking(
        kind: ProviderKind,
        level: crate::config::ThinkingLevel,
        force: bool,
    ) -> Config {
        Config {
            port: 8787,
            proxy_db: PathBuf::from("/tmp/proxy.db"),
            pricing_db: PathBuf::from("/tmp/pricing.db"),
            providers: vec![ProviderConfig {
                name: "p".into(),
                kind,
                auth: AuthConfig::Bearer { value: "k".into() },
                anthropic_base_url: None,
                openai_base_url: Some("https://example.invalid".into()),
                format_mode: crate::config::FormatMode::Both,
                thinking_level: level,
                thinking_force: force,
                reasoning_effort: None,
                thinking_mode: crate::config::ThinkingMode::SplitOnly,
                max_concurrent: None,
                sanitize_empty_tools: false,
                enabled: true,
            }],
            routing: vec![],
            affinity: Default::default(),
            quota: vec![],
        }
    }
```

The two rejection tests above pass `&config_with_thinking(kind, ThinkingLevel::Unset, false)`
as the `_existing` argument rather than a `Config::default_for_tests()` that does
not exist.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p proxy --lib admin::tests::payload`
Expected: FAIL to compile — `no field 'thinking_level' on type 'ProviderPayload'`.

- [ ] **Step 3: Change the DTO**

In `crates/proxy-admin-api/src/lib.rs`, remove the `reasoning_effort` field from `ProviderPayload` and add:

```rust
    /// Thinking level for this provider, from the kind's offered list:
    /// `unset` | `off` | `minimal` | `low` | `medium` | `high` | `xhigh` |
    /// `max` | `adaptive`. Absent or empty means `unset`.
    #[serde(default)]
    pub thinking_level: Option<String>,
    /// When true the level overrides a value the client sent; when false it
    /// only fills in what the client omitted.
    #[serde(default)]
    pub thinking_force: Option<bool>,
```

Update the `ProviderPayload` literal in that file's own test module accordingly.

- [ ] **Step 4: Map both directions**

In `admin.rs`, in `config_to_payload`, replace the `reasoning_effort` line with:

```rust
                thinking_level: Some(p.thinking_level.as_str().to_string()),
                thinking_force: Some(p.thinking_force),
```

In `payload_to_config`, delete the whole `let reasoning_effort = match ... ;` block and put this in its place:

```rust
            let thinking_level = match pp.thinking_level.as_deref().map(str::trim) {
                None | Some("") => crate::config::ThinkingLevel::Unset,
                Some(raw) => {
                    let parsed = crate::config::ThinkingLevel::parse(raw);
                    let offered =
                        crate::adapters::providers::thinking::thinking_levels(kind);
                    match parsed {
                        Some(crate::config::ThinkingLevel::Unset) => {
                            crate::config::ThinkingLevel::Unset
                        }
                        Some(level) if offered.contains(&level) => level,
                        _ => {
                            let valid = offered
                                .iter()
                                .map(|l| l.as_str())
                                .collect::<Vec<_>>()
                                .join(", ");
                            return Err(ProxyError::BadRequest(format!(
                                "invalid thinking_level '{raw}' for provider '{}' (valid for kind '{}': unset, {valid})",
                                pp.name,
                                kind_to_str(kind),
                            )));
                        }
                    }
                }
            };
```

Then set `thinking_level,` and `thinking_force: pp.thinking_force.unwrap_or(false),` in the `ProviderConfig` literal, and drop the `reasoning_effort` line from it (leave the struct field itself populated with `None` until Task 8 removes it).

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p proxy --lib admin::`
Expected: PASS.

- [ ] **Step 6: Format, lint, commit**

```bash
cargo fmt
cargo clippy --workspace --all-targets -- -D warnings
git add crates/proxy-admin-api/src/lib.rs crates/proxy/src/application/use_cases/admin.rs
git commit -m "feat(proxy): expose thinking level through the admin API"
```

---

### Task 7: TUI dropdown and force toggle

**Files:**
- Modify: `crates/proxy-tui/src/app.rs` (`ReasoningEffortInput` around line 372, `FormField` around line 630, `field_order`, `ProviderFormModal`)
- Modify: `crates/proxy-tui/src/ui.rs` (form rows around line 1260)
- Modify: `crates/proxy-tui/src/main.rs` (cycle handler around line 1330, `FormInputs` construction at three call sites)
- Modify: `crates/proxy-tui/src/validate.rs` (`FormInputs`, payload construction)

**Interfaces:**
- Consumes: `ProviderPayload::thinking_level`/`thinking_force` (Task 6).
- Produces: `ThinkingLevelInput` with `label()`, `as_option()`, `from_option()`, `cycle_next_for(kind)`, `cycle_prev_for(kind)`, `levels_for(kind)`.

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` in `app.rs`:

```rust
    #[test]
    fn thinking_levels_are_scoped_to_the_provider_kind() {
        use super::ThinkingLevelInput;
        assert_eq!(
            ThinkingLevelInput::levels_for(ProviderKind::DeepSeek),
            &[ThinkingLevelInput::High, ThinkingLevelInput::Max]
        );
        assert_eq!(
            ThinkingLevelInput::levels_for(ProviderKind::Minimax),
            &[ThinkingLevelInput::Off, ThinkingLevelInput::Adaptive]
        );
    }

    #[test]
    fn cycling_wraps_within_the_kind_list_and_includes_unset() {
        use super::ThinkingLevelInput;
        // unset → first offered → ... → last offered → unset
        let k = ProviderKind::DeepSeek;
        assert_eq!(
            ThinkingLevelInput::Unset.cycle_next_for(k),
            ThinkingLevelInput::High
        );
        assert_eq!(
            ThinkingLevelInput::Max.cycle_next_for(k),
            ThinkingLevelInput::Unset
        );
        assert_eq!(
            ThinkingLevelInput::Unset.cycle_prev_for(k),
            ThinkingLevelInput::Max
        );
    }

    /// A level that is invalid for the newly chosen kind must not survive a
    /// kind change, or the form would submit something the API rejects.
    #[test]
    fn a_level_invalid_for_the_new_kind_falls_back_to_unset() {
        use super::ThinkingLevelInput;
        assert_eq!(
            ThinkingLevelInput::Adaptive.clamp_to(ProviderKind::DeepSeek),
            ThinkingLevelInput::Unset
        );
        assert_eq!(
            ThinkingLevelInput::High.clamp_to(ProviderKind::DeepSeek),
            ThinkingLevelInput::High
        );
    }

    #[test]
    fn thinking_level_round_trips_through_payload_strings() {
        use super::ThinkingLevelInput;
        for level in [
            ThinkingLevelInput::Unset,
            ThinkingLevelInput::Off,
            ThinkingLevelInput::High,
            ThinkingLevelInput::XHigh,
            ThinkingLevelInput::Adaptive,
        ] {
            assert_eq!(ThinkingLevelInput::from_option(level.as_option()), level);
        }
    }

    #[test]
    fn force_row_is_offered_only_when_a_level_is_set() {
        let order = field_order(
            AuthInputKind::Bearer,
            ProviderKind::DeepSeek,
            ThinkingLevelInput::Unset,
        );
        assert!(!order.contains(&FormField::ThinkingForce));

        let order = field_order(
            AuthInputKind::Bearer,
            ProviderKind::DeepSeek,
            ThinkingLevelInput::High,
        );
        assert!(order.contains(&FormField::ThinkingForce));
    }
```

`field_order` is a private free function in `app.rs` (line 657) and the test
module lives in the same file, so the test calls it directly — no wrapper is
needed. Note it gains a third parameter in Step 4.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p proxy-tui --bin cli-router-proxy-tui thinking`
Expected: FAIL to compile — `cannot find type 'ThinkingLevelInput'`.

- [ ] **Step 3: Add the cycle widget**

In `app.rs`, replace the whole `ReasoningEffortInput` enum and its `impl` with:

```rust
/// Cycle widget for the per-provider thinking level. The offered list is
/// scoped to the provider kind, mirroring
/// `proxy::adapters::providers::thinking::thinking_levels` — the TUI cannot
/// depend on the proxy crate, so the table is duplicated here the way the
/// effort lists were before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThinkingLevelInput {
    #[default]
    Unset,
    Off,
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
    Max,
    Adaptive,
}

impl ThinkingLevelInput {
    pub fn label(self) -> &'static str {
        match self {
            ThinkingLevelInput::Unset => "unset",
            ThinkingLevelInput::Off => "off",
            ThinkingLevelInput::Minimal => "minimal",
            ThinkingLevelInput::Low => "low",
            ThinkingLevelInput::Medium => "medium",
            ThinkingLevelInput::High => "high",
            ThinkingLevelInput::XHigh => "xhigh",
            ThinkingLevelInput::Max => "max",
            ThinkingLevelInput::Adaptive => "adaptive",
        }
    }

    /// `None` for `Unset` so the payload omits the field entirely.
    pub fn as_option(self) -> Option<&'static str> {
        match self {
            ThinkingLevelInput::Unset => None,
            other => Some(other.label()),
        }
    }

    pub fn from_option(value: Option<&str>) -> Self {
        match value {
            Some("off") => ThinkingLevelInput::Off,
            Some("minimal") => ThinkingLevelInput::Minimal,
            Some("low") => ThinkingLevelInput::Low,
            Some("medium") => ThinkingLevelInput::Medium,
            Some("high") => ThinkingLevelInput::High,
            Some("xhigh") => ThinkingLevelInput::XHigh,
            Some("max") => ThinkingLevelInput::Max,
            Some("adaptive") => ThinkingLevelInput::Adaptive,
            _ => ThinkingLevelInput::Unset,
        }
    }

    pub fn levels_for(kind: ProviderKind) -> &'static [ThinkingLevelInput] {
        use ThinkingLevelInput::*;
        match kind {
            ProviderKind::Anthropic => &[Off, Low, Medium, High, XHigh, Max],
            ProviderKind::Codex | ProviderKind::OpenAi => {
                &[Off, Minimal, Low, Medium, High, XHigh]
            }
            ProviderKind::Zai => &[Off, High, Max],
            ProviderKind::DeepSeek => &[High, Max],
            ProviderKind::Kimi => &[Low, High, Max],
            ProviderKind::Minimax => &[Off, Adaptive],
        }
    }

    /// Reset to `Unset` when the level isn't offered by `kind`, so changing the
    /// provider kind can't leave a value the admin API will reject.
    pub fn clamp_to(self, kind: ProviderKind) -> Self {
        if self == ThinkingLevelInput::Unset || Self::levels_for(kind).contains(&self) {
            self
        } else {
            ThinkingLevelInput::Unset
        }
    }

    pub fn cycle_next_for(self, kind: ProviderKind) -> Self {
        let levels = Self::levels_for(kind);
        match levels.iter().position(|l| *l == self) {
            None => levels[0],
            Some(i) if i + 1 == levels.len() => ThinkingLevelInput::Unset,
            Some(i) => levels[i + 1],
        }
    }

    pub fn cycle_prev_for(self, kind: ProviderKind) -> Self {
        let levels = Self::levels_for(kind);
        match levels.iter().position(|l| *l == self) {
            None => levels[levels.len() - 1],
            Some(0) => ThinkingLevelInput::Unset,
            Some(i) => levels[i - 1],
        }
    }
}
```

- [ ] **Step 4: Rework the form fields**

In `app.rs`, replace `FormField::ReasoningEffort` with two variants:

```rust
    ThinkingLevel,
    ThinkingForce,
```

Change `field_order` to take the current level and offer the rows for every kind except `Codex`-only cases — the level applies to all kinds:

```rust
fn field_order(
    auth_kind: AuthInputKind,
    provider_kind: ProviderKind,
    thinking_level: ThinkingLevelInput,
) -> Vec<FormField> {
    let mut order = vec![
        FormField::Name,
        FormField::Kind,
        FormField::AnthropicBaseUrl,
        FormField::OpenaiBaseUrl,
    ];
    if provider_kind != ProviderKind::Codex {
        order.push(FormField::FormatMode);
    }
    order.push(FormField::ThinkingLevel);
    if thinking_level != ThinkingLevelInput::Unset {
        order.push(FormField::ThinkingForce);
    }
    if provider_kind == ProviderKind::Minimax {
        order.push(FormField::ThinkingMode);
    }
    if provider_kind == ProviderKind::Kimi {
        order.push(FormField::SanitizeEmptyTools);
    }
    order.push(FormField::AuthKind);
    if matches!(auth_kind, AuthInputKind::ApiKey | AuthInputKind::Bearer) {
        order.push(FormField::AuthValue);
    }
    order.push(FormField::Enabled);
    order.push(FormField::Save);
    order
}
```

Update `FormField::next` / `FormField::prev` to take and forward the extra argument, and update every call site in `main.rs`.

In `ProviderFormModal`, replace `reasoning_effort: ReasoningEffortInput` with:

```rust
    pub thinking_level: ThinkingLevelInput,
    pub thinking_force: bool,
```

Set them in `new_for_add` (`ThinkingLevelInput::Unset` / `false`) and in `from_provider`:

```rust
            thinking_level: ThinkingLevelInput::from_option(p.thinking_level.as_deref()),
            thinking_force: p.thinking_force.unwrap_or(false),
```

- [ ] **Step 5: Handle the keys**

In `main.rs`, replace the `FormField::ReasoningEffort` arm of the cycle handler with:

```rust
        FormField::ThinkingLevel => {
            m.thinking_level = if forward {
                m.thinking_level.cycle_next_for(m.kind)
            } else {
                m.thinking_level.cycle_prev_for(m.kind)
            };
        }
        FormField::ThinkingForce => {
            m.thinking_force = !m.thinking_force;
        }
```

In the `FormField::Kind` arm, after the kind changes, clamp the level so an invalid carry-over can't be submitted:

```rust
            m.thinking_level = m.thinking_level.clamp_to(m.kind);
```

- [ ] **Step 6: Render the rows**

In `ui.rs`, replace the `FormField::ReasoningEffort` row block with:

```rust
    lines.push(row(
        FormField::ThinkingLevel,
        "Thinking:",
        format!("< {} >    [←/→ to cycle]", m.thinking_level.label()),
    ));
    if m.thinking_level != crate::app::ThinkingLevelInput::Unset {
        lines.push(row(
            FormField::ThinkingForce,
            "Force:",
            format!(
                "< {} >    [←/→ to toggle]",
                if m.thinking_force { "on" } else { "off" }
            ),
        ));
    }
```

- [ ] **Step 7: Carry the values into the payload**

In `validate.rs`, replace the `reasoning_effort` field of `FormInputs` with:

```rust
    pub thinking_level: Option<&'a str>,
    pub thinking_force: bool,
```

and the corresponding block of the returned `ProviderPayload` with:

```rust
        thinking_level: input.thinking_level.map(str::to_string),
        thinking_force: Some(input.thinking_force),
```

Delete the `reasoning_effort` entry from the payload literal. In `main.rs`, at all three `FormInputs` construction sites, replace the `reasoning_effort` line with:

```rust
        thinking_level: m.thinking_level.as_option(),
        thinking_force: m.thinking_force,
```

Update the `FormInputs` and `ProviderPayload` literals in the `validate.rs` and `ui.rs` test modules the same way; delete the three `*_reasoning_effort` tests in `validate.rs`, since the field they cover is gone.

- [ ] **Step 8: Run the TUI tests to verify they pass**

Run: `cargo test -p proxy-tui`
Expected: PASS, including the five new tests.

- [ ] **Step 9: Format, lint, commit**

```bash
cargo fmt
cargo clippy --workspace --all-targets -- -D warnings
git add crates/proxy-tui/src
git commit -m "feat(proxy-tui): add per-kind thinking level dropdown and force toggle"
```

---

### Task 8: Retire `reasoning_effort` and document the field

**Files:**
- Modify: `crates/proxy/src/config.rs` (drop the field)
- Modify: `crates/proxy/src/adapters/providers/upstream.rs` (drop `Quirks::reasoning_effort`, `inject_effort`, and its tests)
- Modify: `crates/proxy/src/adapters/providers/builder.rs` (drop the `quirks_for` argument)
- Modify: `docs/specs/provider-config.md`

**Interfaces:**
- Consumes: everything from Tasks 1–7.
- Produces: no new API. `quirks_for(kind, thinking, sanitize_empty_tools)` loses its `reasoning_effort` parameter.

- [ ] **Step 1: Delete the field and its plumbing**

Remove `pub reasoning_effort: Option<String>` from `ProviderConfig`. Remove `pub reasoning_effort: Option<String>` from `Quirks`, drop the third parameter of `quirks_for`, delete `inject_effort` and the `apply_anthropic_request_quirks` branch that calls it, and delete the `preset_anthropic_carries_reasoning_effort` and `anthropic_request_quirks_inject_effort_for_anthropic` tests.

Note that `apply_anthropic_request_quirks` still exists — it now carries only the thinking injection added in Task 3.

- [ ] **Step 2: Fix the fallout**

Run `cargo check --workspace --all-targets` and remove every remaining `reasoning_effort:` line it reports from `ProviderConfig` literals, `quirks_for` call sites, and the `builder.rs` call.

- [ ] **Step 3: Run the full suite**

Run: `cargo test --workspace`
Expected: PASS.

- [ ] **Step 4: Document the field**

In `docs/specs/provider-config.md`, replace the `reasoning_effort` row of the `ProviderPayload` table with two rows:

```markdown
| `thinking_level` | `string?` | ❌ | `"unset"` | mọi kind | Mức suy nghĩ của upstream, chọn trong danh sách riêng của từng kind. Xem §5c. |
| `thinking_force` | `bool?` | ❌ | `false` | mọi kind | `true` thì mức này đè lên tham số client gửi; `false` thì chỉ điền khi client bỏ trống. |
```

Add a `## 5c. thinking_level` section after §5b holding the offered-levels table from the spec, the note that DeepSeek maps `low`/`medium` to `high` server-side, the note that MiniMax M2.x ignores `off`, and the note that Kimi K2.x rejects requests carrying both `thinking` and `reasoning_effort`.

- [ ] **Step 5: Format, lint, commit**

```bash
cargo fmt
cargo clippy --workspace --all-targets -- -D warnings
git add crates/proxy/src docs/specs/provider-config.md
git commit -m "refactor(proxy): retire reasoning_effort in favour of thinking_level"
```

---

### Task 9: End-to-end verification against a live provider

**Files:** none — this task changes no code.

**Interfaces:** consumes the whole feature.

- [ ] **Step 1: Build and start a throwaway proxy**

```bash
SC=$(mktemp -d)
sqlite3 ~/.local/share/cli-router/proxy.db ".backup '$SC/test.db'"
sqlite3 $SC/test.db "update settings set value='8799' where key='port'; delete from requests;"
cargo build -p proxy
./target/debug/cli-router-proxy --db $SC/test.db > $SC/proxy.log 2>&1 &
sleep 6
```

Expected: `curl -s http://127.0.0.1:8799/admin/status` returns JSON, and
`sqlite3 $SC/test.db "select name, thinking_level, thinking_force from providers"`
lists every provider at `unset|0`, except any row migrated from `reasoning_effort`.

- [ ] **Step 2: Set a level through the admin API**

```bash
python3 - <<'PY'
import json, subprocess
cfg = json.loads(subprocess.run(["curl","-s","http://127.0.0.1:8799/admin/config"],
                                capture_output=True, text=True).stdout)
for p in cfg["providers"]:
    if p["name"] == "minimax":
        p["thinking_level"] = "off"
        p["thinking_force"] = True
r = subprocess.run(["curl","-s","-X","PUT","http://127.0.0.1:8799/admin/config",
                    "-H","content-type: application/json","-d",json.dumps(cfg)],
                   capture_output=True, text=True)
print(r.stdout[:200])
PY
sqlite3 $SC/test.db "select name, thinking_level, thinking_force from providers where name='minimax'"
```

Expected: `minimax|off|1`.

- [ ] **Step 3: Confirm an invalid level is rejected**

Repeat step 2 with `p["thinking_level"] = "adaptive"` on the `deepseek` provider.
Expected: HTTP 400 whose body contains `invalid thinking_level 'adaptive'` and lists `high, max`.

- [ ] **Step 4: Confirm the patch reaches the upstream**

```bash
curl -s -N -X POST "http://127.0.0.1:8799/v1/messages" \
  -H "content-type: application/json" -H "anthropic-version: 2023-06-01" \
  -d '{"model":"minimax/minimax-m3","max_tokens":128,"stream":true,
       "messages":[{"role":"user","content":"What is 17*23? Reply with just the number."}]}' \
  | grep -c thinking_delta
```

Expected: `0` — thinking is off, so the stream carries no `thinking_delta`. Re-run
with `thinking_level = "adaptive"` and expect a non-zero count.

- [ ] **Step 5: Stop the throwaway proxy**

```bash
pkill -f "target/debug/cli-router-proxy --db"
curl -s -m 2 http://127.0.0.1:8787/admin/status >/dev/null && echo "prod service untouched"
```

- [ ] **Step 6: Record the result**

Report the observed counts from step 4 in the PR description. No commit.
