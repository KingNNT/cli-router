# Routing Form Provider/Fallback Dropdowns Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the free-text Provider and Fallback inputs on the `proxy-tui` routing form with cycle-dropdown selectors sourced from the configured providers list.

**Architecture:** `RoutingFormModal` switches from `provider: String` + `fallback: String` (comma-separated) to index-based selection over a provider-name snapshot taken at modal-open time. Provider is a single-select cycle (←/→/Enter). Fallback is an ordered multi-select via toggle (←/→ cycle cursor, Enter toggles membership, Backspace removes last). Stale references are surfaced via flash and filtered. On-disk config shape is unchanged.

**Tech Stack:** Rust, ratatui, crossterm. Existing `proxy-tui` patterns: `RoutingFormModal` in `app.rs`, `draw_routing_form_modal` in `ui.rs`, `handle_routing_form_key` in `main.rs`. The codebase already has a "cycle dropdown" pattern used for `Kind:`, `Auth Kind:`, and `Strategy:` rows — we mirror it.

**Spec:** `docs/superpowers/specs/2026-07-20-routing-form-provider-dropdown-design.md`

---

## File Structure

- **Modify** `crates/proxy-tui/src/app.rs` — `RoutingFormModal` struct fields, `new_for_add` / `from_rule` constructors, plus a small `impl` helper for cycling indices.
- **Modify** `crates/proxy-tui/src/ui.rs` — `draw_routing_form_modal` Provider and Fallback row rendering and the help line.
- **Modify** `crates/proxy-tui/src/main.rs` — `handle_routing_form_key` (cycling/toggle/validation), `edit_routing_text` (drop Provider/Fallback arms), `open_routing_edit_modal` (stale flash), `handle_config_key` call sites (pass snapshot).

No new files. All changes are in-crate. On-disk config (`RoutingRulePayload`) is untouched.

---

## Task 1: Update `RoutingFormModal` struct and constructors

**Files:**
- Modify: `crates/proxy-tui/src/app.rs` (struct at lines 749-759, `impl` at lines 761-790)
- Test: `crates/proxy-tui/src/app.rs` (unit tests inside `mod form_field_tests`)

- [ ] **Step 1: Write failing unit test for `new_for_add` preserving provider order**

Add this test inside `mod form_field_tests` in `crates/proxy-tui/src/app.rs` (after the existing `routing_field_navigation_includes_save` test):

```rust
#[test]
fn routing_form_construction_preserves_providers_order() {
    let providers = vec!["zai".to_string(), "anthropic".to_string(), "openai".to_string()];
    let m = super::RoutingFormModal::new_for_add(providers.clone());
    assert_eq!(m.available_providers, providers);
    assert_eq!(m.provider_index, 0);
    assert!(m.fallback.is_empty());
    assert_eq!(m.fallback_cursor, 0);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p proxy-tui routing_form_construction_preserves_providers_order`
Expected: compile error — `new_for_add` takes 0 arguments, struct has no `available_providers`/`provider_index`/`fallback_cursor` fields.

- [ ] **Step 3: Update the struct definition**

Replace the struct body at `crates/proxy-tui/src/app.rs:749-759`:

```rust
#[derive(Debug, Clone)]
pub struct RoutingFormModal {
    pub mode: FormMode,
    pub focused: RoutingField,
    pub match_model: String,
    pub available_providers: Vec<String>,
    pub provider_index: usize,
    pub fallback: Vec<String>,
    pub fallback_cursor: usize,
    pub strategy: proxy_admin_api::RoutingStrategyPayload,
    pub priority: String,
    pub error: Option<String>,
}
```

- [ ] **Step 4: Update `new_for_add` and `from_rule`**

Replace the `impl RoutingFormModal` block at `crates/proxy-tui/src/app.rs:761-790`:

```rust
impl RoutingFormModal {
    pub fn new_for_add(available_providers: Vec<String>) -> Self {
        Self {
            mode: FormMode::Add,
            focused: RoutingField::MatchModel,
            match_model: String::new(),
            provider_index: 0,
            available_providers,
            fallback: Vec::new(),
            fallback_cursor: 0,
            strategy: proxy_admin_api::RoutingStrategyPayload::default(),
            priority: String::new(),
            error: None,
        }
    }

    pub fn from_rule(
        index: usize,
        rule: &proxy_admin_api::RoutingRulePayload,
        available_providers: Vec<String>,
    ) -> Self {
        let provider_index = available_providers
            .iter()
            .position(|p| p == &rule.provider)
            .unwrap_or(0);
        let fallback: Vec<String> = rule
            .fallback
            .iter()
            .filter(|f| available_providers.contains(f))
            .cloned()
            .collect();
        Self {
            mode: FormMode::Edit {
                original_index: index,
                original_name: rule.provider.clone(),
            },
            focused: RoutingField::MatchModel,
            match_model: rule.r#match.model.clone().unwrap_or_default(),
            provider_index,
            available_providers,
            fallback,
            fallback_cursor: 0,
            strategy: rule.strategy.clone(),
            priority: rule.priority.map(|p| p.to_string()).unwrap_or_default(),
            error: None,
        }
    }
}
```

- [ ] **Step 5: Run the new test to verify it passes**

Run: `cargo test -p proxy-tui routing_form_construction_preserves_providers_order`
Expected: PASS.

- [ ] **Step 6: Verify compile errors elsewhere (expected)**

Run: `cargo check -p proxy-tui`
Expected: compile errors in `main.rs` (call sites pass no provider list) and `ui.rs` (references `m.provider`). These are fixed in later tasks. Do not commit yet.

- [ ] **Step 7: Commit**

```bash
git add crates/proxy-tui/src/app.rs
git commit -m "refactor(proxy-tui): RoutingFormModal holds provider index + fallback Vec

Replace free-text provider/fallback with index-based selection over a
providers snapshot taken at modal-open time. Constructors now take the
snapshot. UI and key-handler updates follow in subsequent commits."
```

---

## Task 2: Update existing `main.rs` tests that use the old constructor signatures

**Files:**
- Modify: `crates/proxy-tui/src/main.rs` (tests around lines 2036-2099)

These tests use `RoutingFormModal::new_for_add()` (no args) and read `form.provider`. We update them to use the new signature so the crate compiles before adding new behavior. The actual cycling behavior tests come in Task 5.

- [ ] **Step 1: Update `routing_form_enter_on_non_save_does_not_submit`**

In `crates/proxy-tui/src/main.rs`, find the test `routing_form_enter_on_non_save_does_not_submit` (around line 2036) and replace its body:

```rust
#[test]
fn routing_form_enter_on_non_save_does_not_submit() {
    let client = client();
    let mut state = app_state_with_config();
    let mut form = RoutingFormModal::new_for_add(vec!["anthropic".to_string()]);
    form.match_model = "claude-*".into();
    form.focused = RoutingField::Provider;

    let modal = handle_routing_form_key(key(KeyCode::Enter), &client, &mut state, form);
    let Modal::RoutingForm(m) = modal else {
        panic!("expected routing form modal");
    };
    assert_eq!(m.focused, RoutingField::Provider);
    assert_eq!(m.provider_index, 0); // Enter cycles, but with 1 provider it stays at 0
    assert!(
        state
            .config
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()
            .routing
            .is_empty()
    );
}
```

Note: this test currently asserts Enter on Provider does nothing. After Task 3 lands, Enter on Provider cycles forward — but with only 1 provider in the snapshot, the index stays at 0. The assertion is updated to reflect that.

- [ ] **Step 2: Update `routing_form_down_can_focus_save`**

Around line 2062:

```rust
#[test]
fn routing_form_down_can_focus_save() {
    let client = client();
    let mut state = app_state_with_config();
    let mut form = RoutingFormModal::new_for_add(vec!["anthropic".to_string()]);
    form.focused = RoutingField::Priority;

    let modal = handle_routing_form_key(key(KeyCode::Down), &client, &mut state, form);
    let Modal::RoutingForm(m) = modal else {
        panic!("expected routing form modal");
    };
    assert_eq!(m.focused, RoutingField::Save);
}
```

- [ ] **Step 3: Update `routing_form_s_no_longer_submits`**

Around line 2075:

```rust
#[test]
fn routing_form_s_no_longer_submits() {
    let client = client();
    let mut state = app_state_with_config();
    let mut form = RoutingFormModal::new_for_add(vec!["anthropic".to_string()]);
    form.match_model = "claude-*".into();
    form.focused = RoutingField::Save;

    let modal = handle_routing_form_key(key(KeyCode::Char('s')), &client, &mut state, form);
    let Modal::RoutingForm(m) = modal else {
        panic!("expected routing form modal");
    };
    assert_eq!(m.available_providers[m.provider_index], "anthropic");
    assert!(
        state
            .config
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()
            .routing
            .is_empty()
    );
}
```

- [ ] **Step 4: Run the existing tests**

Run: `cargo test -p proxy-tui routing_form_`
Expected: compile errors in `main.rs` lines 370-371, 1454, and 1628-1639 (the call sites and `edit_routing_text`) plus `ui.rs` Provider/Fallback rendering. These are fixed in Tasks 3-6. The tests themselves should compile.

- [ ] **Step 5: Commit**

```bash
git add crates/proxy-tui/src/main.rs
git commit -m "test(proxy-tui): update routing form tests for new constructor signature

Pass provider snapshot to RoutingFormModal::new_for_add and read
provider_index instead of provider string. Behavior assertions will be
expanded in later commits."
```

---

## Task 3: Update `handle_routing_form_key` and `edit_routing_text`

**Files:**
- Modify: `crates/proxy-tui/src/main.rs:1534-1640`

- [ ] **Step 1: Write failing test for provider cycling**

Add this test inside `mod modal_key_tests` (or the same mod containing the other `routing_form_*` tests) in `crates/proxy-tui/src/main.rs`:

```rust
#[test]
fn routing_provider_cycles_with_left_right() {
    let client = client();
    let mut state = app_state_with_config();
    let providers = vec![
        "anthropic".to_string(),
        "zai".to_string(),
        "openai".to_string(),
    ];
    let mut form = RoutingFormModal::new_for_add(providers);
    form.focused = RoutingField::Provider;
    assert_eq!(form.provider_index, 0);

    // Right cycles forward
    let modal = handle_routing_form_key(key(KeyCode::Right), &client, &mut state, form);
    let Modal::RoutingForm(m) = modal else {
        panic!("expected routing form modal");
    };
    assert_eq!(m.provider_index, 1);

    // Right again
    let modal = handle_routing_form_key(key(KeyCode::Right), &client, &mut state, m);
    let Modal::RoutingForm(m) = modal else {
        panic!("expected routing form modal");
    };
    assert_eq!(m.provider_index, 2);

    // Right wraps to 0
    let modal = handle_routing_form_key(key(KeyCode::Right), &client, &mut state, m);
    let Modal::RoutingForm(m) = modal else {
        panic!("expected routing form modal");
    };
    assert_eq!(m.provider_index, 0);

    // Left wraps to last (2)
    let modal = handle_routing_form_key(key(KeyCode::Left), &client, &mut state, m);
    let Modal::RoutingForm(m) = modal else {
        panic!("expected routing form modal");
    };
    assert_eq!(m.provider_index, 2);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p proxy-tui routing_provider_cycles_with_left_right`
Expected: FAIL — `Right`/`Left` are not handled for `RoutingField::Provider` (fall through to `_ => Modal::RoutingForm(m)`), so `provider_index` never changes.

- [ ] **Step 3: Replace `handle_routing_form_key` body**

Replace the function body at `crates/proxy-tui/src/main.rs:1534-1627` with:

```rust
fn handle_routing_form_key(
    k: KeyEvent,
    client: &AdminClient,
    state: &mut AppState,
    mut m: RoutingFormModal,
) -> Modal {
    match k.code {
        KeyCode::Esc => Modal::None,
        KeyCode::Down => {
            m.focused = m.focused.next();
            Modal::RoutingForm(m)
        }
        KeyCode::Up => {
            m.focused = m.focused.prev();
            Modal::RoutingForm(m)
        }
        KeyCode::Left | KeyCode::Right | KeyCode::Enter if m.focused == RoutingField::Provider => {
            if m.available_providers.is_empty() {
                return Modal::RoutingForm(m);
            }
            let len = m.available_providers.len();
            let forward = matches!(k.code, KeyCode::Right | KeyCode::Enter);
            m.provider_index = if forward {
                (m.provider_index + 1) % len
            } else if m.provider_index == 0 {
                len - 1
            } else {
                m.provider_index - 1
            };
            Modal::RoutingForm(m)
        }
        KeyCode::Left | KeyCode::Right if m.focused == RoutingField::Fallback => {
            if m.available_providers.is_empty() {
                return Modal::RoutingForm(m);
            }
            let len = m.available_providers.len();
            m.fallback_cursor = if matches!(k.code, KeyCode::Right) {
                (m.fallback_cursor + 1) % len
            } else if m.fallback_cursor == 0 {
                len - 1
            } else {
                m.fallback_cursor - 1
            };
            Modal::RoutingForm(m)
        }
        KeyCode::Enter if m.focused == RoutingField::Fallback => {
            if m.available_providers.is_empty() {
                return Modal::RoutingForm(m);
            }
            let candidate = m.available_providers[m.fallback_cursor].clone();
            if let Some(pos) = m.fallback.iter().position(|f| f == &candidate) {
                m.fallback.remove(pos);
            } else {
                m.fallback.push(candidate);
            }
            Modal::RoutingForm(m)
        }
        KeyCode::Backspace if m.focused == RoutingField::Fallback => {
            m.fallback.pop();
            Modal::RoutingForm(m)
        }
        KeyCode::Enter if m.focused == RoutingField::Strategy => {
            m.strategy = strategy_cycle(&m.strategy);
            Modal::RoutingForm(m)
        }
        KeyCode::Enter if m.focused == RoutingField::Save => {
            // Submit
            if m.match_model.trim().is_empty() {
                m.error = Some("match model is required".into());
                return Modal::RoutingForm(m);
            }
            if m.available_providers.is_empty() {
                m.error = Some("no providers configured; add a provider first".into());
                return Modal::RoutingForm(m);
            }
            let provider_name = m.available_providers[m.provider_index].clone();
            if m.fallback.iter().any(|f| f == &provider_name) {
                m.error = Some("fallback must not contain the primary provider".into());
                return Modal::RoutingForm(m);
            }

            let mut cfg = match state.config.as_ref().and_then(|r| r.as_ref().ok()).cloned() {
                Some(c) => c,
                None => {
                    m.error = Some("config not loaded".into());
                    return Modal::RoutingForm(m);
                }
            };

            let rule = proxy_admin_api::RoutingRulePayload {
                r#match: proxy_admin_api::MatchPayload {
                    model: Some(m.match_model.trim().to_string()),
                },
                provider: provider_name,
                fallback: m.fallback.clone(),
                strategy: m.strategy.clone(),
                priority: m.priority.trim().parse::<u32>().ok(),
            };

            match &m.mode {
                FormMode::Add => cfg.routing.push(rule),
                FormMode::Edit { original_index, .. } => {
                    if *original_index >= cfg.routing.len() {
                        m.error = Some("routing list changed; press Esc and reopen".into());
                        return Modal::RoutingForm(m);
                    }
                    cfg.routing[*original_index] = rule;
                }
            }

            match save_config_via_client(client, &cfg, state) {
                Ok(updated) => {
                    state.set_config(Ok(updated));
                    let action = match m.mode {
                        FormMode::Add => "added",
                        FormMode::Edit { .. } => "updated",
                    };
                    state.flash(format!("routing rule {action}"));
                    Modal::None
                }
                Err(e) => {
                    m.error = Some(format!("save failed: {e}"));
                    Modal::RoutingForm(m)
                }
            }
        }
        KeyCode::Backspace => {
            edit_routing_text(&mut m, |s| {
                s.pop();
            });
            Modal::RoutingForm(m)
        }
        KeyCode::Char(c) => {
            edit_routing_text(&mut m, |s| {
                s.push(c);
            });
            Modal::RoutingForm(m)
        }
        _ => Modal::RoutingForm(m),
    }
}
```

- [ ] **Step 4: Replace `edit_routing_text` body**

Replace the function at `crates/proxy-tui/src/main.rs:1629-1640` with:

```rust
fn edit_routing_text(m: &mut RoutingFormModal, f: impl FnOnce(&mut String)) {
    m.error = None;
    let target: Option<&mut String> = match m.focused {
        RoutingField::MatchModel => Some(&mut m.match_model),
        RoutingField::Priority => Some(&mut m.priority),
        _ => None,
    };
    if let Some(s) = target {
        f(s);
    }
}
```

- [ ] **Step 5: Run the new test to verify it passes**

Run: `cargo test -p proxy-tui routing_provider_cycles_with_left_right`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/proxy-tui/src/main.rs
git commit -m "feat(proxy-tui): cycle/toggle provider & fallback in routing form

Provider: ←/→/Enter cycle the index through the snapshot.
Fallback: ←/→ cycle the cursor, Enter toggles membership, Backspace
removes the last entry. Save now validates: providers non-empty,
primary not in fallback. MatchModel and Priority remain text-editable."
```

---

## Task 4: Update `open_routing_edit_modal` and `handle_config_key` call sites

**Files:**
- Modify: `crates/proxy-tui/src/main.rs:369-375` (`handle_config_key` routing section)
- Modify: `crates/proxy-tui/src/main.rs:1444-1455` (`open_routing_edit_modal`)

- [ ] **Step 1: Write failing test for stale-provider flash**

Add to `mod modal_key_tests` in `crates/proxy-tui/src/main.rs`:

```rust
#[test]
fn open_routing_edit_modal_flashes_when_provider_stale() {
    let mut state = AppState::new();
    let rule = proxy_admin_api::RoutingRulePayload {
        r#match: proxy_admin_api::MatchPayload { model: Some("claude-*".into()) },
        provider: "ghost".to_string(),
        fallback: vec!["also-gone".to_string(), "anthropic".to_string()],
        strategy: proxy_admin_api::RoutingStrategyPayload::Failover,
        priority: None,
    };
    state.set_config(Ok(ConfigPayload {
        port: 3456,
        providers: vec![proxy_admin_api::ProviderPayload {
            name: "anthropic".into(),
            kind: "anthropic".into(),
            enabled: true,
            auth: proxy_admin_api::AuthPayload::Passthrough,
            base_url: None,
        }],
        routing: vec![rule],
        quota: vec![],
        affinity: proxy_admin_api::AffinityPayload::default(),
        proxy_db: None,
        pricing_db: None,
    }));
    state.routing_selected = 0;

    open_routing_edit_modal(&mut state);

    match &state.modal {
        Modal::RoutingForm(m) => {
            assert_eq!(m.provider_index, 0); // defaulted
            assert_eq!(m.available_providers, vec!["anthropic".to_string()]);
            assert_eq!(m.fallback, vec!["anthropic".to_string()]); // "also-gone" filtered out
        }
        other => panic!("expected RoutingForm modal, got {other:?}"),
    }
    assert!(state.flash.as_ref().map_or(false, |f| f.contains("ghost")));
}
```

Note: if `state.flash` is private or accessed differently in this codebase, adjust accordingly — see how other tests read the flash. Run `rg 'state\.flash' crates/proxy-tui/src/main.rs` if unsure.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p proxy-tui open_routing_edit_modal_flashes_when_provider_stale`
Expected: compile error — `open_routing_edit_modal` currently calls `RoutingFormModal::from_rule(idx, rule)` with no providers arg.

- [ ] **Step 3: Update `open_routing_edit_modal`**

Replace the body at `crates/proxy-tui/src/main.rs:1444-1455`:

```rust
fn open_routing_edit_modal(state: &mut AppState) {
    let cfg = match state.config.as_ref().and_then(|r| r.as_ref().ok()) {
        Some(c) => c.clone(),
        None => return,
    };
    let idx = state.routing_selected;
    let rule = match cfg.routing.get(idx) {
        Some(r) => r.clone(),
        None => return,
    };
    let available_providers: Vec<String> = cfg.providers.iter().map(|p| p.name.clone()).collect();
    if !available_providers.contains(&rule.provider) {
        state.flash(format!(
            "\u{26a0} original provider '{}' no longer exists",
            rule.provider
        ));
    }
    state.modal = Modal::RoutingForm(RoutingFormModal::from_rule(
        idx,
        &rule,
        available_providers,
    ));
}
```

Note: we clone `cfg`, `rule`, and the providers slice so we can borrow `state` mutably for `state.flash` and `state.modal` after reading `state.config`. Check the exact signature of `state.flash` — if it takes `Into<String>` vs `String`, the call shape may need a tiny tweak.

- [ ] **Step 4: Update `handle_config_key` add call site**

At `crates/proxy-tui/src/main.rs:369-375`, replace:

```rust
ConfigSection::Routing => match k.code {
    KeyCode::Char('a') => {
        let providers = state
            .config
            .as_ref()
            .and_then(|r| r.as_ref().ok())
            .map(|c| c.providers.iter().map(|p| p.name.clone()).collect())
            .unwrap_or_default();
        state.modal = Modal::RoutingForm(RoutingFormModal::new_for_add(providers));
    }
    KeyCode::Char('e') => {
        open_routing_edit_modal(state);
    }
    KeyCode::Char('d') => {
```

Leave the rest of the match arms (`KeyCode::Char('d')` and onwards) unchanged — they were not shown here.

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p proxy-tui open_routing_edit_modal_flashes_when_provider_stale`
Expected: PASS.

- [ ] **Step 6: Verify the full routing test suite still compiles & passes**

Run: `cargo test -p proxy-tui routing_`
Expected: PASS (all routing tests).

- [ ] **Step 7: Commit**

```bash
git add crates/proxy-tui/src/main.rs
git commit -m "feat(proxy-tui): snapshot providers at routing modal open, flash on stale

handle_config_key and open_routing_edit_modal now pass the current
providers list (in config order) into RoutingFormModal::new_for_add /
from_rule. Editing a rule whose provider was deleted shows a flash
and defaults the cursor to index 0."
```

---

## Task 5: Update `draw_routing_form_modal` rendering

**Files:**
- Modify: `crates/proxy-tui/src/ui.rs:1533-1611` (Provider row at 1578-1582, Fallback row at 1583-1587, help line at 1606-1609)

- [ ] **Step 1: Write failing render test for Provider cycle display**

Add to `mod tests` in `crates/proxy-tui/src/ui.rs`:

```rust
#[test]
fn routing_form_renders_provider_as_cycle() {
    use crate::app::{FormMode, RoutingField, RoutingFormModal};

    let m = RoutingFormModal {
        mode: FormMode::Add,
        focused: RoutingField::Provider,
        match_model: "claude-*".into(),
        available_providers: vec!["anthropic".into(), "zai".into()],
        provider_index: 0,
        fallback: vec![],
        fallback_cursor: 0,
        strategy: proxy_admin_api::RoutingStrategyPayload::Failover,
        priority: String::new(),
        error: None,
    };
    let mut state = AppState::new();
    state.modal = Modal::RoutingForm(m);
    let out = render_state(&state, 100, 24);
    assert!(out.contains("< anthropic >"), "got: {out}");
    assert!(out.contains("←/→"), "got: {out}");
    // Free-text rendering should be gone
    assert!(!out.contains("Provider:         \n"));
}

#[test]
fn routing_form_renders_fallback_with_selected_list() {
    use crate::app::{FormMode, RoutingField, RoutingFormModal};

    let m = RoutingFormModal {
        mode: FormMode::Add,
        focused: RoutingField::Fallback,
        match_model: "claude-*".into(),
        available_providers: vec!["anthropic".into(), "zai".into(), "openai".into()],
        provider_index: 0,
        fallback: vec!["zai".into(), "openai".into()],
        fallback_cursor: 1,
        strategy: proxy_admin_api::RoutingStrategyPayload::Failover,
        priority: String::new(),
        error: None,
    };
    let mut state = AppState::new();
    state.modal = Modal::RoutingForm(m);
    let out = render_state(&state, 100, 24);
    assert!(out.contains("selected: [zai, openai]"), "got: {out}");
    assert!(out.contains("Enter toggles"), "got: {out}");
}
```

If `render_state` has a different signature in this file, adjust — run `rg 'fn render_state' crates/proxy-tui/src/ui.rs` to check.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p proxy-tui routing_form_renders_provider_as_cycle routing_form_renders_fallback_with_selected_list`
Expected: FAIL — the current rendering shows `show_or_placeholder(&m.provider)` which doesn't compile (`m.provider` is gone) or doesn't match the expected strings.

- [ ] **Step 3: Replace Provider and Fallback rows and help line**

In `crates/proxy-tui/src/ui.rs`, replace lines 1578-1587 (the two `lines.push(row(...))` calls for Provider and Fallback) with:

```rust
    let provider_display = if m.available_providers.is_empty() {
        "(no providers configured)".to_string()
    } else {
        format!(
            "< {} >    [←/→ or Enter to cycle]",
            m.available_providers[m.provider_index]
        )
    };
    lines.push(row(
        RoutingField::Provider,
        "Provider:",
        provider_display,
    ));

    let fallback_display = if m.available_providers.is_empty() {
        "(no providers configured)".to_string()
    } else {
        let cursor_name = &m.available_providers[m.fallback_cursor];
        let selected = if m.fallback.is_empty() {
            "(none)".to_string()
        } else {
            m.fallback.join(", ")
        };
        format!(
            "< {cursor_name} > | selected: [{selected}]    [Enter toggles, Backspace removes last]"
        )
    };
    lines.push(row(
        RoutingField::Fallback,
        "Fallback:",
        fallback_display,
    ));
```

Then update the help line at lines 1606-1609 from:

```rust
    lines.push(Line::from(Span::styled(
        "↑/↓: move  Enter on Strategy: cycle  Enter on Save: submit  Esc: cancel",
        Style::default().fg(Color::DarkGray),
    )));
```

to:

```rust
    lines.push(Line::from(Span::styled(
        "↑/↓: move  ←/→/Enter on Provider/Fallback: cycle  Enter on Fallback: toggle  Enter on Strategy: cycle  Enter on Save: submit  Esc: cancel",
        Style::default().fg(Color::DarkGray),
    )));
```

- [ ] **Step 4: Run the new tests to verify they pass**

Run: `cargo test -p proxy-tui routing_form_renders_provider_as_cycle routing_form_renders_fallback_with_selected_list`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/proxy-tui/src/ui.rs
git commit -m "feat(proxy-tui): render Provider/Fallback as cycle dropdowns

Provider row shows < name > with ←/→ hint. Fallback row shows the
cursor plus an ordered selected list. Help line documents the new
interactions."
```

---

## Task 6: Add remaining behavior tests

**Files:**
- Modify: `crates/proxy-tui/src/main.rs` (add tests inside `mod modal_key_tests`)

Tests 1 (`routing_provider_cycles_with_left_right`) was added in Task 3. Now add the rest. Each step is its own test, written then verified to pass (they test already-implemented behavior, so they should pass on the first run).

- [ ] **Step 1: Add provider Enter-cycle test**

```rust
#[test]
fn routing_provider_enter_cycles_forward() {
    let client = client();
    let mut state = app_state_with_config();
    let providers = vec![
        "anthropic".to_string(),
        "zai".to_string(),
        "openai".to_string(),
    ];
    let mut form = RoutingFormModal::new_for_add(providers);
    form.focused = RoutingField::Provider;

    let modal = handle_routing_form_key(key(KeyCode::Enter), &client, &mut state, form);
    let Modal::RoutingForm(m) = modal else {
        panic!("expected routing form modal");
    };
    assert_eq!(m.provider_index, 1);
}
```

Run: `cargo test -p proxy-tui routing_provider_enter_cycles_forward`
Expected: PASS.

- [ ] **Step 2: Add empty-providers-blocks-save test**

```rust
#[test]
fn routing_provider_no_providers_configured_blocks_save() {
    let client = client();
    let mut state = app_state_with_config();
    let mut form = RoutingFormModal::new_for_add(vec![]);
    form.match_model = "claude-*".into();
    form.focused = RoutingField::Save;

    let modal = handle_routing_form_key(key(KeyCode::Enter), &client, &mut state, form);
    let Modal::RoutingForm(m) = modal else {
        panic!("expected routing form modal");
    };
    assert_eq!(m.error.as_deref(), Some("no providers configured; add a provider first"));
}
```

Run: `cargo test -p proxy-tui routing_provider_no_providers_configured_blocks_save`
Expected: PASS.

- [ ] **Step 3: Add fallback toggle add-then-remove test**

```rust
#[test]
fn routing_fallback_enter_toggles_add_then_remove() {
    let client = client();
    let mut state = app_state_with_config();
    let providers = vec!["anthropic".to_string(), "zai".to_string()];
    let mut form = RoutingFormModal::new_for_add(providers);
    form.focused = RoutingField::Fallback;
    form.fallback_cursor = 1; // points at "zai"

    // First Enter: add "zai"
    let modal = handle_routing_form_key(key(KeyCode::Enter), &client, &mut state, form);
    let Modal::RoutingForm(m) = modal else {
        panic!("expected routing form modal");
    };
    assert_eq!(m.fallback, vec!["zai".to_string()]);

    // Second Enter: remove "zai"
    let modal = handle_routing_form_key(key(KeyCode::Enter), &client, &mut state, m);
    let Modal::RoutingForm(m) = modal else {
        panic!("expected routing form modal");
    };
    assert!(m.fallback.is_empty());
}
```

Run: `cargo test -p proxy-tui routing_fallback_enter_toggles_add_then_remove`
Expected: PASS.

- [ ] **Step 4: Add fallback order preservation test**

```rust
#[test]
fn routing_fallback_appends_in_order() {
    let client = client();
    let mut state = app_state_with_config();
    let providers = vec![
        "anthropic".to_string(),
        "zai".to_string(),
        "openai".to_string(),
    ];
    let mut form = RoutingFormModal::new_add(providers);
    form.focused = RoutingField::Fallback;
    form.fallback_cursor = 2; // openai first

    let modal = handle_routing_form_key(key(KeyCode::Enter), &client, &mut state, form);
    let Modal::RoutingForm(mut m) = modal else {
        panic!("expected routing form modal");
    };
    m.fallback_cursor = 0; // anthropic second
    let modal = handle_routing_form_key(key(KeyCode::Enter), &client, &mut state, m);
    let Modal::RoutingForm(m) = modal else {
        panic!("expected routing form modal");
    };
    assert_eq!(m.fallback, vec!["openai".to_string(), "anthropic".to_string()]);
}
```

Note: I wrote `new_add` by mistake — use `new_for_add`. Fix before saving.

Run: `cargo test -p proxy-tui routing_fallback_appends_in_order`
Expected: PASS.

- [ ] **Step 5: Add fallback Backspace test**

```rust
#[test]
fn routing_fallback_backspace_removes_last() {
    let client = client();
    let mut state = app_state_with_config();
    let providers = vec!["anthropic".to_string(), "zai".to_string()];
    let mut form = RoutingFormModal::new_for_add(providers);
    form.focused = RoutingField::Fallback;
    form.fallback = vec!["anthropic".to_string(), "zai".to_string()];

    let modal = handle_routing_form_key(key(KeyCode::Backspace), &client, &mut state, form);
    let Modal::RoutingForm(m) = modal else {
        panic!("expected routing form modal");
    };
    assert_eq!(m.fallback, vec!["anthropic".to_string()]);
}
```

Run: `cargo test -p proxy-tui routing_fallback_backspace_removes_last`
Expected: PASS.

- [ ] **Step 6: Add cursor-independence test**

```rust
#[test]
fn routing_fallback_cycles_cursor_independently_of_selection() {
    let client = client();
    let mut state = app_state_with_config();
    let providers = vec![
        "anthropic".to_string(),
        "zai".to_string(),
        "openai".to_string(),
    ];
    let mut form = RoutingFormModal::new_for_add(providers);
    form.focused = RoutingField::Fallback;
    form.fallback = vec!["zai".to_string()];
    form.fallback_cursor = 1;

    // Right moves cursor to 2 but leaves selection alone
    let modal = handle_routing_form_key(key(KeyCode::Right), &client, &mut state, form);
    let Modal::RoutingForm(m) = modal else {
        panic!("expected routing form modal");
    };
    assert_eq!(m.fallback_cursor, 2);
    assert_eq!(m.fallback, vec!["zai".to_string()]);
}
```

Run: `cargo test -p proxy-tui routing_fallback_cycles_cursor_independently_of_selection`
Expected: PASS.

- [ ] **Step 7: Add save-blocks-when-fallback-contains-primary test**

```rust
#[test]
fn routing_save_blocks_when_fallback_contains_primary() {
    let client = client();
    let mut state = app_state_with_config();
    let providers = vec!["anthropic".to_string(), "zai".to_string()];
    let mut form = RoutingFormModal::new_for_add(providers);
    form.match_model = "claude-*".into();
    form.focused = RoutingField::Save;
    // provider_index = 0 → "anthropic"
    form.fallback = vec!["anthropic".to_string()];

    let modal = handle_routing_form_key(key(KeyCode::Enter), &client, &mut state, form);
    let Modal::RoutingForm(m) = modal else {
        panic!("expected routing form modal");
    };
    assert_eq!(m.error.as_deref(), Some("fallback must not contain the primary provider"));
}
```

Run: `cargo test -p proxy-tui routing_save_blocks_when_fallback_contains_primary`
Expected: PASS.

- [ ] **Step 8: Add from_rule stale fallback filter test**

Add to `mod modal_key_tests` (or whichever module contains other `from_rule`-related tests):

```rust
#[test]
fn routing_from_rule_filters_stale_fallback() {
    let rule = proxy_admin_api::RoutingRulePayload {
        r#match: proxy_admin_api::MatchPayload { model: Some("claude-*".into()) },
        provider: "anthropic".to_string(),
        fallback: vec!["gone".to_string(), "kept".to_string()],
        strategy: proxy_admin_api::RoutingStrategyPayload::Failover,
        priority: None,
    };
    let m = RoutingFormModal::from_rule(0, &rule, vec!["anthropic".into(), "kept".into()]);
    assert_eq!(m.fallback, vec!["kept".to_string()]);
}

#[test]
fn routing_from_rule_provider_not_in_snapshot_defaults_to_zero() {
    let rule = proxy_admin_api::RoutingRulePayload {
        r#match: proxy_admin_api::MatchPayload { model: Some("claude-*".into()) },
        provider: "ghost".to_string(),
        fallback: vec![],
        strategy: proxy_admin_api::RoutingStrategyPayload::Failover,
        priority: None,
    };
    let m = RoutingFormModal::from_rule(
        0,
        &rule,
        vec!["anthropic".to_string(), "zai".to_string()],
    );
    assert_eq!(m.provider_index, 0);
    assert_eq!(m.available_providers[0], "anthropic");
}
```

Run: `cargo test -p proxy-tui routing_from_rule_`
Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add crates/proxy-tui/src/main.rs
git commit -m "test(proxy-tui): cover cycling, toggling, validation, stale handling

Adds tests for: provider ←/→/Enter cycling, no-providers save block,
fallback Enter toggle (add/remove), order preservation, Backspace
undo, cursor/selection independence, primary-in-fallback save block,
from_rule stale filtering, from_rule stale provider default."
```

---

## Task 7: Full workspace verification

**Files:** None (verification only).

- [ ] **Step 1: Run the full proxy-tui test suite**

Run: `cargo test -p proxy-tui`
Expected: All tests PASS.

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -p proxy-tui -- -D warnings`
Expected: No warnings.

- [ ] **Step 3: Run rustfmt check**

Run: `cargo fmt --check -p proxy-tui`
Expected: No changes needed. If it reports differences, run `cargo fmt -p proxy-tui` and re-commit.

- [ ] **Step 4: Run the whole workspace check (sanity)**

Run: `cargo check --workspace`
Expected: PASS.

- [ ] **Step 5: Manual smoke test**

Run: `mise run dev:proxy-tui` (or `cargo run -p proxy-tui` against a running dev proxy)
- Open Config tab → Routing section
- Press `a` to add a rule → verify Provider shows `< name >` and cycles with ←/→
- Focus Fallback → cycle cursor with ←/→ → press Enter to toggle → verify `selected: [...]` updates
- Press Backspace on Fallback → verify last entry removed
- Press Save → verify rule appears in list
- Edit the rule → verify fields populate correctly
- Stop the TUI

- [ ] **Step 6: Final commit if formatting changed**

```bash
git add crates/proxy-tui/
git commit -m "style(proxy-tui): rustfmt routing form dropdowns"
```

---

## Self-Review Notes

**Spec coverage check:**

- ✅ Provider single-select cycle → Tasks 1, 3, 5
- ✅ Fallback multi-select via toggle → Tasks 3, 5
- ✅ Empty providers list handling → Task 3 (Save block) + Task 5 (render)
- ✅ Stale references (flash + filter) → Task 4
- ✅ Data model changes → Task 1
- ✅ Construction signature changes → Task 1
- ✅ Validation invariants (5) → Task 3
- ✅ UI rendering (cycle markers, help line) → Task 5
- ✅ Key handling tables → Task 3
- ✅ `edit_routing_text` trim → Task 3
- ✅ Compatibility (on-disk unchanged, no API changes) → Implicit; verified in Task 7 (workspace check)
- ✅ Tests: all 13 spec-listed tests → Tasks 1, 5, 6 (with Test 1 in Task 1 and Tests 11-12 in Task 5)
- ✅ Verification gates → Task 7

**Type consistency:** `provider_index: usize`, `fallback_cursor: usize`, `available_providers: Vec<String>`, `fallback: Vec<String>` used consistently across all tasks.

**One known typo to fix:** Task 6 Step 4 has `new_add` instead of `new_for_add` — flagged inline; fix when transcribing.
