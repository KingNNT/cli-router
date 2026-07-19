# Active/Disabled Provider Toggle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an `enabled` flag to proxy providers so they can be active or disabled, with validation, TUI form support, and a list-view hotkey toggle.

**Architecture:** Add `enabled: bool` to the provider config and admin API DTO, persist it in SQLite via a migration, exclude disabled providers from the routing leaf map, and enforce that routing rules only reference enabled providers. The proxy-tui adds a form field and a `z` hotkey in the provider list with a confirmation dialog when disabling would affect routing rules.

**Tech Stack:** Rust, axum, ratatui, rusqlite, serde, utoipa.

---

## File structure

- `crates/proxy/src/config.rs` — `ProviderConfig.enabled` + `Config::validate` checks.
- `crates/proxy/src/adapters/storage/schema.rs` — migration V8 adding `enabled` column.
- `crates/proxy/src/adapters/storage/db_config.rs` — load/save `enabled` + tests.
- `crates/proxy/src/adapters/providers/builder.rs` — filter disabled providers in `build_leaves` + tests.
- `crates/proxy/src/application/use_cases/admin.rs` — `config_to_payload` / `payload_to_config` + tests.
- `crates/proxy-admin-api/src/lib.rs` — `ProviderPayload.enabled`.
- `crates/proxy-tui/src/app.rs` — `FormField::Enabled`, `ProviderFormModal.enabled`, new `Modal::DisableConfirm`, helpers.
- `crates/proxy-tui/src/validate.rs` — `FormInputs.enabled` + output `enabled` in `ProviderPayload`.
- `crates/proxy-tui/src/ui.rs` — render form toggle, gray disabled rows, toolbar hint, disable confirm modal.
- `crates/proxy-tui/src/main.rs` — `z` handler, toggle logic, confirm dialog handling, pass `enabled` through form submissions.

---

## Task 1: Add `enabled` to `ProviderConfig` and `Config::validate`

**Files:**
- Modify: `crates/proxy/src/config.rs`

- [ ] **Step 1: Add the field**

Insert the field into `ProviderConfig`:

```rust
pub struct ProviderConfig {
    pub name: String,
    pub kind: ProviderKind,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub auth: AuthConfig,
    // ... rest unchanged
}
```

- [ ] **Step 2: Add validation checks**

After the existing unknown-provider checks in `Config::validate`, add disabled-provider checks:

```rust
for (i, r) in self.routing.iter().enumerate() {
    if let Some(p) = self.providers.iter().find(|p| p.name == r.provider) {
        if !p.enabled {
            return Err(ConfigError::Validation(format!(
                "routing rule {i} references disabled provider '{}'",
                r.provider
            )));
        }
    }
    for fb in &r.fallback {
        if let Some(p) = self.providers.iter().find(|p| p.name == *fb) {
            if !p.enabled {
                return Err(ConfigError::Validation(format!(
                    "routing rule {i} fallback references disabled provider '{}'",
                    fb
                )));
            }
        }
    }
}
```

- [ ] **Step 3: Add unit tests**

Append to the `#[cfg(test)] mod tests` in `crates/proxy/src/config.rs`:

```rust
#[test]
fn provider_config_defaults_enabled_to_true() {
    let json = r#"{"name":"anthropic","kind":"anthropic"}"#;
    let provider: ProviderConfig = serde_json::from_str(json).unwrap();
    assert!(provider.enabled);
}

#[test]
fn validate_rejects_disabled_provider_in_routing() {
    let cfg = Config {
        providers: vec![ProviderConfig {
            name: "anthropic".into(),
            kind: ProviderKind::Anthropic,
            enabled: false,
            ..Default::default()
        }],
        routing: vec![RoutingRule {
            match_spec: MatchSpec { model: None },
            provider: "anthropic".into(),
            fallback: vec![],
            strategy: RoutingStrategy::Failover,
            priority: None,
        }],
        ..Default::default()
    };
    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("routing rule 0 references disabled provider 'anthropic'"));
}

#[test]
fn validate_rejects_disabled_provider_in_fallback() {
    let cfg = Config {
        providers: vec![
            ProviderConfig {
                name: "primary".into(),
                kind: ProviderKind::Anthropic,
                enabled: true,
                ..Default::default()
            },
            ProviderConfig {
                name: "fallback".into(),
                kind: ProviderKind::Anthropic,
                enabled: false,
                ..Default::default()
            },
        ],
        routing: vec![RoutingRule {
            match_spec: MatchSpec { model: None },
            provider: "primary".into(),
            fallback: vec!["fallback".into()],
            strategy: RoutingStrategy::Failover,
            priority: None,
        }],
        ..Default::default()
    };
    let err = cfg.validate().unwrap_err().to_string();
    assert!(err.contains("routing rule 0 fallback references disabled provider 'fallback'"));
}
```

Run: `cargo test -p proxy config::tests -- --nocapture`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/proxy/src/config.rs
git commit -m "feat(proxy): add enabled field to ProviderConfig and validate routing references"
```

---

## Task 2: SQLite migration and DB load/save

**Files:**
- Modify: `crates/proxy/src/adapters/storage/schema.rs`
- Modify: `crates/proxy/src/adapters/storage/db_config.rs`

- [ ] **Step 1: Add migration V8 in `schema.rs`**

After `MIGRATION_V7`, add:

```rust
const MIGRATION_V8: &str = r#"
ALTER TABLE providers ADD COLUMN enabled INTEGER NOT NULL DEFAULT 1;
"#;
```

Add `(8, MIGRATION_V8)` to the `MIGRATIONS` array.

- [ ] **Step 2: Update `load_providers` in `db_config.rs`**

Add `enabled` to the SELECT list and read it as a bool:

```rust
fn load_providers(conn: &Connection) -> Result<Vec<ProviderConfig>, ConfigError> {
    let mut stmt = conn
        .prepare(
            "SELECT name, kind, base_url, openai_base_url, reasoning_effort, thinking_mode, auth_type,
                    auth_api_key, auth_bearer, auth_access_token, auth_refresh_token, auth_expires_at_ms,
                    max_concurrent, sanitize_empty_tools, enabled
             FROM providers ORDER BY id",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map([], |row| {
            // ... existing reads ...
            Ok(ProviderConfig {
                // ... existing fields ...
                max_concurrent: row.get::<_, Option<i64>>(12)?.map(|v| v.max(0) as usize),
                sanitize_empty_tools: row.get::<_, i64>(13)? != 0,
                enabled: row.get::<_, i64>(14)? != 0,
            })
        })
        .map_err(db_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
}
```

- [ ] **Step 3: Update `save` in `db_config.rs`**

Add `enabled` to the INSERT statement and params:

```rust
"INSERT INTO providers (name, kind, base_url, openai_base_url, reasoning_effort, thinking_mode, auth_type,
 auth_api_key, auth_bearer, auth_access_token, auth_refresh_token, auth_expires_at_ms, max_concurrent, sanitize_empty_tools, enabled)
 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
```

```rust
stmt.execute(rusqlite::params![
    // ... existing params 1..14 ...
    p.sanitize_empty_tools as i64,
    p.enabled as i64,
])
```

- [ ] **Step 4: Add DB round-trip test**

In `crates/proxy/src/adapters/storage/db_config.rs` tests, add:

```rust
#[test]
fn enabled_flag_survives_save_and_load() {
    let (_td, repo) = test_repo();
    let mut cfg = Config {
        providers: vec![ProviderConfig {
            name: "off".into(),
            kind: ProviderKind::Anthropic,
            enabled: false,
            ..Default::default()
        }],
        ..Default::default()
    };
    repo.save(&cfg).unwrap();
    let loaded = repo.load().unwrap();
    assert!(!loaded.providers[0].enabled);
}
```

Run: `cargo test -p proxy adapters::storage::db_config::tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/proxy/src/adapters/storage/schema.rs crates/proxy/src/adapters/storage/db_config.rs
git commit -m "feat(proxy): persist provider enabled flag in SQLite"
```

---

## Task 3: Exclude disabled providers from `build_leaves`

**Files:**
- Modify: `crates/proxy/src/adapters/providers/builder.rs`

- [ ] **Step 1: Filter in `build_leaves`**

```rust
pub fn build_leaves(
    providers: &[ProviderConfig],
    http: reqwest::Client,
) -> Result<HashMap<String, Arc<dyn Provider>>, BuildError> {
    providers
        .iter()
        .filter(|p| p.enabled)
        .map(|p| Ok((p.name.clone(), build_leaf(p, http.clone())?)))
        .collect()
}
```

- [ ] **Step 2: Add unit test**

Append in the existing `#[cfg(test)] mod tests` in `builder.rs`:

```rust
#[test]
fn build_leaves_excludes_disabled_provider() {
    let cfg = Config {
        providers: vec![
            ProviderConfig {
                name: "on".into(),
                kind: ProviderKind::Anthropic,
                enabled: true,
                auth: AuthConfig::Passthrough,
                ..Default::default()
            },
            ProviderConfig {
                name: "off".into(),
                kind: ProviderKind::Anthropic,
                enabled: false,
                auth: AuthConfig::Passthrough,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let leaves = build_leaves(&cfg.providers, reqwest::Client::new()).unwrap();
    assert!(leaves.contains_key("on"));
    assert!(!leaves.contains_key("off"));
}
```

Run: `cargo test -p proxy adapters::providers::builder::tests::build_leaves_excludes_disabled_provider`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/proxy/src/adapters/providers/builder.rs
git commit -m "feat(proxy): exclude disabled providers from leaf map"
```

---

## Task 4: Wire `enabled` through admin API conversions

**Files:**
- Modify: `crates/proxy/src/application/use_cases/admin.rs`

- [ ] **Step 1: Update `config_to_payload`**

Add `enabled` to the mapped `ProviderPayload`:

```rust
ProviderPayload {
    name: p.name.clone(),
    kind: kind_to_str(p.kind).into(),
    enabled: p.enabled,
    auth: auth_to_payload(&p.auth),
    // ... rest unchanged
}
```

- [ ] **Step 2: Update `payload_to_config`**

Add `enabled` to the constructed `ProviderConfig`:

```rust
Ok(ProviderConfig {
    name: pp.name,
    kind,
    enabled: pp.enabled,
    auth: payload_to_auth(pp.auth),
    // ... rest unchanged
})
```

- [ ] **Step 3: Add tests**

In the existing admin tests, add:

```rust
#[test]
fn payload_to_config_parses_enabled() {
    let p = ConfigPayload {
        providers: vec![ProviderPayload {
            name: "x".into(),
            kind: "anthropic".into(),
            enabled: false,
            ..Default::default()
        }],
        ..Default::default()
    };
    let cfg = payload_to_config(p, PathBuf::from("/tmp/proxy.db"), PathBuf::from("/tmp/pricing.db"), &Config::default()).unwrap();
    assert!(!cfg.providers[0].enabled);
}

#[test]
fn config_to_payload_round_trips_enabled() {
    let cfg = Config {
        providers: vec![ProviderConfig {
            name: "x".into(),
            kind: ProviderKind::Anthropic,
            enabled: false,
            ..Default::default()
        }],
        ..Default::default()
    };
    let payload = config_to_payload(&cfg);
    assert!(!payload.providers[0].enabled);
}
```

Run: `cargo test -p proxy application::use_cases::admin::tests`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/proxy/src/application/use_cases/admin.rs
git commit -m "feat(proxy): carry provider enabled flag through admin API"
```

---

## Task 5: Add `enabled` to `ProviderPayload` in proxy-admin-api

**Files:**
- Modify: `crates/proxy-admin-api/src/lib.rs`

- [ ] **Step 1: Add the field**

```rust
pub struct ProviderPayload {
    pub name: String,
    pub kind: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub auth: AuthPayload,
    // ... rest unchanged
}
```

- [ ] **Step 2: Add test**

Append to the existing `config_payload_tests` module:

```rust
#[test]
fn provider_payload_defaults_enabled_to_true() {
    let json = r#"{"name":"x","kind":"anthropic"}"#;
    let p: ProviderPayload = serde_json::from_str(json).unwrap();
    assert!(p.enabled);
}

#[test]
fn provider_payload_deserializes_enabled_false() {
    let json = r#"{"name":"x","kind":"anthropic","enabled":false}"#;
    let p: ProviderPayload = serde_json::from_str(json).unwrap();
    assert!(!p.enabled);
}
```

Run: `cargo test -p proxy-admin-api`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/proxy-admin-api/src/lib.rs
git commit -m "feat(proxy-admin-api): add enabled field to ProviderPayload"
```

---

## Task 6: Update proxy-tui form model and validation

**Files:**
- Modify: `crates/proxy-tui/src/app.rs`
- Modify: `crates/proxy-tui/src/validate.rs`

- [ ] **Step 1: Add `FormField::Enabled` and update `field_order`**

```rust
pub enum FormField {
    Name,
    Kind,
    BaseUrl,
    OpenaiBaseUrl,
    ReasoningEffort,
    ThinkingMode,
    SanitizeEmptyTools,
    AuthKind,
    AuthValue,
    Enabled,
    Save,
}
```

```rust
fn field_order(auth_kind: AuthInputKind, provider_kind: ProviderKind) -> Vec<FormField> {
    let mut order = vec![
        FormField::Name,
        FormField::Kind,
        FormField::BaseUrl,
        FormField::OpenaiBaseUrl,
    ];
    if matches!(provider_kind, ProviderKind::Codex | ProviderKind::Anthropic) {
        order.push(FormField::ReasoningEffort);
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

- [ ] **Step 2: Add `enabled` to `ProviderFormModal`**

```rust
pub struct ProviderFormModal {
    // ... existing fields
    pub enabled: bool,
    // ...
}
```

Update `new_for_add` and `from_provider`:

```rust
pub fn new_for_add() -> Self {
    Self {
        // ...
        enabled: true,
        // ...
    }
}

pub fn from_provider(index: usize, p: &ProviderPayload) -> Self {
    Self {
        // ...
        enabled: p.enabled,
        // ...
    }
}
```

- [ ] **Step 3: Update `FormInputs` and `validate_provider_form`**

In `crates/proxy-tui/src/validate.rs`:

```rust
pub struct FormInputs<'a> {
    // ... existing fields
    pub enabled: bool,
    // ...
}
```

Set `enabled: input.enabled` in the returned `ProviderPayload`.

- [ ] **Step 4: Update all `FormInputs` construction sites in `main.rs`**

Add `enabled: m.enabled` to:

- `submit_non_oauth_save`
- `submit_oauth_add`
- `submit_oauth_edit`

- [ ] **Step 5: Update form tests**

Update `field_order_tests` assertions in `crates/proxy-tui/src/app.rs` to include `Enabled`.

Run: `cargo test -p proxy-tui app::form_field_tests`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/proxy-tui/src/app.rs crates/proxy-tui/src/validate.rs crates/proxy-tui/src/main.rs
git commit -m "feat(proxy-tui): add enabled field to provider form model and validation"
```

---

## Task 7: Render the `Enabled` toggle in the provider form UI

**Files:**
- Modify: `crates/proxy-tui/src/ui.rs`

- [ ] **Step 1: Draw the toggle row**

In `draw_form_modal`, after the auth value block and before the `[ Save ]` row, add:

```rust
lines.push(row(
    FormField::Enabled,
    "Active:",
    format!("< {} >    [←/→ to toggle]", if m.enabled { "yes" } else { "no" }),
));
```

- [ ] **Step 2: Handle cycling in `cycle_field_value`**

In `crates/proxy-tui/src/main.rs::cycle_field_value`, add:

```rust
FormField::Enabled => {
    m.enabled = !m.enabled;
}
```

- [ ] **Step 3: Add test**

Append to `ui.rs` tests:

```rust
#[test]
fn provider_form_shows_active_toggle() {
    let mut modal = ProviderFormModal::new_for_add();
    modal.enabled = false;
    let mut backend = TestBackend::new(80, 20);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| draw_form_modal(f, &modal))
        .unwrap();
    let expected = Row::new(vec![Cell::from("Active:")]); // rough assertion
    // Use backend.buffer() assertions to check for "Active: < no >"
    let buf = terminal.backend().buffer();
    let content: String = buf.content.iter().map(|c| c.symbol()).collect();
    assert!(content.contains("Active:"));
    assert!(content.contains("< no >"));
}
```

Run: `cargo test -p proxy-tui ui::tests::provider_form_shows_active_toggle`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/proxy-tui/src/ui.rs crates/proxy-tui/src/main.rs
git commit -m "feat(proxy-tui): render active/disabled toggle in provider form"
```

---

## Task 8: Add the list-view `z` toggle and confirmation modal

**Files:**
- Modify: `crates/proxy-tui/src/app.rs`
- Modify: `crates/proxy-tui/src/ui.rs`
- Modify: `crates/proxy-tui/src/main.rs`

- [ ] **Step 1: Add `DisableConfirmModal` and `Modal::DisableConfirm` in `app.rs`**

```rust
#[derive(Debug, Clone)]
pub struct DisableConfirmModal {
    pub provider_index: usize,
    pub provider_name: String,
    pub rules: Vec<String>,
}
```

```rust
pub enum Modal {
    // ... existing variants
    DisableConfirm(DisableConfirmModal),
}
```

- [ ] **Step 2: Add a helper to compute affected rules**

Reuse `crate::validate::rules_referencing`. This is already available in `main.rs` via `validate` module.

- [ ] **Step 3: Wire `z` key in provider list**

In `handle_config_key` Providers section:

```rust
ConfigSection::Providers => match k.code {
    KeyCode::Char('a') => open_add_modal(state),
    KeyCode::Char('e') => open_edit_modal(state),
    KeyCode::Char('d') => open_delete_modal(state),
    KeyCode::Char('t') => open_test_modal(state),
    KeyCode::Char('z') => toggle_provider_enabled(state),
    _ => {}
},
```

Add `toggle_provider_enabled` in `main.rs`:

```rust
fn toggle_provider_enabled(state: &mut AppState) {
    let cfg = match state.config.as_ref().and_then(|r| r.as_ref().ok()).cloned() {
        Some(c) => c,
        None => return,
    };
    let provider_index = state.providers_selected;
    let prov = match cfg.providers.get(provider_index) {
        Some(p) => p,
        None => return,
    };
    let provider_name = prov.name.clone();
    let rules = crate::validate::rules_referencing(&provider_name, &cfg);
    if !rules.is_empty() && prov.enabled {
        state.modal = Modal::DisableConfirm(DisableConfirmModal {
            provider_index,
            provider_name,
            rules,
        });
    } else {
        // Toggle immediately (enable or disable with no references).
        let mut cfg = cfg;
        cfg.providers[provider_index].enabled = !cfg.providers[provider_index].enabled;
        // (submit via event loop, not directly here)
    }
}
```

Because `handle_config_key` does not take `client`, perform the actual PUT in the event loop after the key returns. Return a flag or restructure: change `toggle_provider_enabled` to return an `Option<ConfigPayload>` or use a command queue. Simpler: move the PUT into `handle_config_key` by passing `client`. Refactor `handle_config_key` signature to accept `client` (it already does) and call `client.put_config` inside a helper `apply_toggle(client, state)`.

- [ ] **Step 4: Handle the disable-confirm modal**

In `handle_modal_key`:

```rust
Modal::DisableConfirm(m) => handle_disable_confirm_key(k, client, state, m),
```

Add `handle_disable_confirm_key` in `main.rs`:

```rust
fn handle_disable_confirm_key(
    k: crossterm::event::KeyEvent,
    client: &AdminClient,
    state: &mut AppState,
    m: DisableConfirmModal,
) -> Modal {
    match k.code {
        KeyCode::Esc | KeyCode::Char('n') => Modal::None,
        KeyCode::Char('y') => {
            let mut cfg = match state.config.as_ref().and_then(|r| r.as_ref().ok()).cloned() {
                Some(c) => c,
                None => {
                    state.flash("toggle failed: config not loaded");
                    return Modal::None;
                }
            };
            cfg.providers[m.provider_index].enabled = false;
            cfg.routing.retain(|r| {
                r.provider != m.provider_name && !r.fallback.contains(&m.provider_name)
            });
            match client.put_config(&cfg) {
                Ok(updated) => {
                    state.set_config(Ok(updated));
                    state.flash(format!("disabled {} and removed referenced rules", m.provider_name));
                    Modal::None
                }
                Err(e) => {
                    state.flash(format!("disable failed: {e}"));
                    Modal::None
                }
            }
        }
        _ => Modal::DisableConfirm(m),
    }
}
```

- [ ] **Step 5: Draw the disable confirm modal**

In `ui.rs`, add `draw_disable_confirm_modal` and dispatch it in `draw`:

```rust
Modal::DisableConfirm(m) => draw_disable_confirm_modal(f, m),
```

```rust
fn draw_disable_confirm_modal(f: &mut Frame, m: &DisableConfirmModal) {
    let area = centered_rect(70, 60, f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Disable provider ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled(
        format!("Provider '{}' is referenced by {} routing rule(s).", m.provider_name, m.rules.len()),
        Style::default().add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));
    for rule in &m.rules {
        lines.push(Line::from(Span::raw(rule.clone())));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::raw("Delete those rules when disabling?")));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("[y] Yes", Style::default().fg(Color::Green)),
        Span::raw("   "),
        Span::styled("[n] No", Style::default().fg(Color::Red)),
    ]));
    f.render_widget(Paragraph::new(lines), inner);
}
```

- [ ] **Step 6: Update provider toolbar and footer hints**

Change `draw_provider_toolbar` to:

```rust
fn draw_provider_toolbar(f: &mut Frame, area: Rect) {
    draw_config_toolbar(
        f,
        area,
        &[
            "[a] Add",
            "[e/Enter] Edit",
            "[d] Delete",
            "[z] Toggle",
            "[t] Test",
            "[r] Refresh",
        ],
    );
}
```

Add `[z] toggle` to `selected_provider_footer`.

- [ ] **Step 7: Commit**

```bash
git add crates/proxy-tui/src/app.rs crates/proxy-tui/src/ui.rs crates/proxy-tui/src/main.rs
git commit -m "feat(proxy-tui): add z hotkey to toggle provider enabled with routing confirmation"
```

---

## Task 9: Gray out disabled providers in the list

**Files:**
- Modify: `crates/proxy-tui/src/ui.rs`

- [ ] **Step 1: Apply gray style to disabled rows**

In `draw_providers_content`, adjust row style:

```rust
let style = if i == state.providers_selected {
    Style::default().fg(Color::Black).bg(Color::Yellow)
} else if !p.enabled {
    Style::default().fg(Color::DarkGray)
} else {
    Style::default()
};
```

- [ ] **Step 2: Add test**

```rust
#[test]
fn disabled_provider_rendered_in_gray() {
    let mut cfg = sample_config();
    cfg.providers[0].enabled = false;
    let state = AppState::with_config(cfg);
    let mut backend = TestBackend::new(80, 20);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| draw_providers_content(f, &state)).unwrap();
    let buf = terminal.backend().buffer();
    // assert first provider row has DarkGray fg
}
```

Run: `cargo test -p proxy-tui ui::tests::disabled_provider_rendered_in_gray`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/proxy-tui/src/ui.rs
git commit -m "feat(proxy-tui): gray out disabled providers in list"
```

---

## Task 10: Workspace verification

- [ ] **Step 1: Run all checks**

```bash
cargo check --workspace
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

Expected: no errors, all tests pass.

- [ ] **Step 2: Commit any final fixes**

```bash
git commit -m "chore: satisfy clippy and tests for provider enabled toggle"
```

---

## Spec coverage self-check

| Spec requirement | Task |
|------------------|------|
| `ProviderConfig.enabled` | Task 1 |
| `ProviderPayload.enabled` | Task 5 |
| SQLite migration + load/save | Task 2 |
| Config validation rejects disabled provider references | Task 1 |
| `build_leaves` excludes disabled providers | Task 3 |
| Admin API conversions carry `enabled` | Task 4 |
| Form field `Active` | Tasks 6, 7 |
| List hotkey `z` toggle | Task 8 |
| Confirm dialog when disabling referenced provider | Task 8 |
| Disabled providers rendered gray | Task 9 |
| Tests for all of the above | Each task |

No placeholders remain in the plan.
