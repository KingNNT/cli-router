# TUI Provider CRUD Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Add / Edit / Delete provider operations to the `proxy-tui` Providers view, replacing the existing edit-auth-only modal with a unified provider form modal.

**Architecture:** All work in `crates/proxy-tui/`. A new `ProviderFormModal` handles both Add and Edit of every provider field; a `DeleteConfirmModal` handles deletes with a routing-rule safety check. Persistence reuses `PUT /admin/config` (no new admin endpoints). OAuth flow on Add is two-step (PUT placeholder, then OAuth dance) because the daemon's OAuth handler resolves providers by name. Validation is a pure module with table-driven tests.

**Tech Stack:** Rust 2024, Ratatui + Crossterm, blocking `ureq` HTTP client, `wiremock` for client tests. Tests run via `cargo test -p proxy-tui`. Compile gate is `cargo check --workspace` and `cargo clippy --workspace -- -D warnings`.

**Spec:** `docs/superpowers/specs/2026-05-04-tui-provider-crud-design.md`

**File map:**

| File | Change | Responsibility |
|---|---|---|
| `crates/proxy-tui/src/validate.rs` | **create** | Pure validation logic + `FormError` + `rules_referencing` |
| `crates/proxy-tui/src/app.rs` | modify | Replace `EditAuthModal`/`EditState` with `ProviderFormModal`/`FormState`/`FormField`/`ProviderKind`/`DeleteConfirmModal` |
| `crates/proxy-tui/src/ui.rs` | modify | Replace `draw_edit_modal` with `draw_form_modal`; add `draw_delete_confirm_modal` |
| `crates/proxy-tui/src/main.rs` | modify | New keybinds `a`/`e`/`d`; replace `handle_edit_key`/`save_provider_auth` with form/delete handlers |

---

### Task 1: Validation module

**Files:**
- Create: `crates/proxy-tui/src/validate.rs`
- Modify: `crates/proxy-tui/src/main.rs:6-10` (declare `mod validate;`)

This task introduces pure functions only. No state changes, no UI. Easy to TDD because it has no I/O.

- [ ] **Step 1: Write the failing tests**

Create `crates/proxy-tui/src/validate.rs`:

```rust
//! Pure validation logic for the provider form modal.
//!
//! No I/O, no ratatui, no async — just functions on `ConfigPayload` /
//! `ProviderPayload` shapes. Easy to unit-test.

use proxy_admin_api::{AuthPayload, ConfigPayload, ProviderPayload};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormError {
    EmptyName,
    InvalidNameChars,
    DuplicateName,
    EmptyAuthValue,
    RenameBlockedBy(Vec<String>),
}

impl std::fmt::Display for FormError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FormError::EmptyName => write!(f, "name is required"),
            FormError::InvalidNameChars => {
                write!(f, "name may contain only letters, digits, '-' and '_'")
            }
            FormError::DuplicateName => write!(f, "a provider with this name already exists"),
            FormError::EmptyAuthValue => write!(f, "auth value is required for api_key and bearer"),
            FormError::RenameBlockedBy(rules) => {
                write!(
                    f,
                    "cannot rename: referenced by {} routing rule(s):\n  - {}",
                    rules.len(),
                    rules.join("\n  - ")
                )
            }
        }
    }
}

/// Inputs needed to validate. The caller owns a richer modal struct, but
/// validation only needs these.
pub struct FormInputs<'a> {
    pub name: &'a str,
    pub kind: &'a str,
    pub base_url: Option<&'a str>,
    pub openai_base_url: Option<&'a str>,
    pub auth: &'a AuthPayload,
    /// `None` for Add, `Some(original_index)` for Edit.
    pub editing_index: Option<usize>,
    /// `None` for Add, `Some(original_name)` for Edit.
    pub original_name: Option<&'a str>,
}

/// Validate the form against the current config and return a freshly built
/// `ProviderPayload` if everything checks out. Caller is responsible for
/// inserting/replacing it in the cached config.
pub fn validate_provider_form(
    input: &FormInputs<'_>,
    cfg: &ConfigPayload,
) -> Result<ProviderPayload, FormError> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err(FormError::EmptyName);
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(FormError::InvalidNameChars);
    }

    // Duplicate name check.
    let dup = cfg
        .providers
        .iter()
        .enumerate()
        .any(|(i, p)| p.name == name && Some(i) != input.editing_index);
    if dup {
        return Err(FormError::DuplicateName);
    }

    // Auth value presence.
    match input.auth {
        AuthPayload::ApiKey { value } | AuthPayload::Bearer { value } if value.trim().is_empty() => {
            return Err(FormError::EmptyAuthValue);
        }
        _ => {}
    }

    // Rename blocked by routing references.
    if let (Some(orig), idx) = (input.original_name, input.editing_index)
        && idx.is_some()
        && orig != name
    {
        let refs = rules_referencing(orig, cfg);
        if !refs.is_empty() {
            return Err(FormError::RenameBlockedBy(refs));
        }
    }

    Ok(ProviderPayload {
        name: name.to_string(),
        kind: input.kind.to_string(),
        auth: input.auth.clone(),
        base_url: input.base_url.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
        openai_base_url: input
            .openai_base_url
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
    })
}

/// Returns human-readable descriptions of routing rules that reference
/// `provider_name` (either as primary `provider` or in `fallback`).
pub fn rules_referencing(provider_name: &str, cfg: &ConfigPayload) -> Vec<String> {
    cfg.routing
        .iter()
        .enumerate()
        .filter_map(|(idx, rule)| {
            let role = if rule.provider == provider_name {
                Some("provider")
            } else if rule.fallback.iter().any(|f| f == provider_name) {
                Some("fallback")
            } else {
                None
            }?;
            let model = rule.r#match.model.as_deref().unwrap_or("*");
            Some(format!("rule #{}: match={} {}={}", idx + 1, model, role, provider_name))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proxy_admin_api::{MatchPayload, RoutingRulePayload, RoutingStrategyPayload};

    fn empty_cfg() -> ConfigPayload {
        ConfigPayload {
            port: 8787,
            providers: vec![],
            routing: vec![],
        }
    }

    fn provider(name: &str) -> ProviderPayload {
        ProviderPayload {
            name: name.into(),
            kind: "anthropic".into(),
            auth: AuthPayload::Passthrough,
            base_url: None,
            openai_base_url: None,
        }
    }

    fn rule(model: &str, primary: &str, fallback: &[&str]) -> RoutingRulePayload {
        RoutingRulePayload {
            r#match: MatchPayload {
                model: Some(model.into()),
            },
            provider: primary.into(),
            fallback: fallback.iter().map(|s| s.to_string()).collect(),
            strategy: RoutingStrategyPayload::default(),
            priority: None,
        }
    }

    fn inputs<'a>(name: &'a str, auth: &'a AuthPayload) -> FormInputs<'a> {
        FormInputs {
            name,
            kind: "anthropic",
            base_url: None,
            openai_base_url: None,
            auth,
            editing_index: None,
            original_name: None,
        }
    }

    #[test]
    fn empty_name_rejected() {
        let cfg = empty_cfg();
        let auth = AuthPayload::Passthrough;
        let r = validate_provider_form(&inputs("", &auth), &cfg);
        assert_eq!(r, Err(FormError::EmptyName));
    }

    #[test]
    fn whitespace_only_name_rejected() {
        let cfg = empty_cfg();
        let auth = AuthPayload::Passthrough;
        let r = validate_provider_form(&inputs("   ", &auth), &cfg);
        assert_eq!(r, Err(FormError::EmptyName));
    }

    #[test]
    fn invalid_name_chars_rejected() {
        let cfg = empty_cfg();
        let auth = AuthPayload::Passthrough;
        for bad in &["a/b", "a b", "a.b", "a!b"] {
            let r = validate_provider_form(&inputs(bad, &auth), &cfg);
            assert_eq!(r, Err(FormError::InvalidNameChars), "input: {bad}");
        }
    }

    #[test]
    fn duplicate_name_on_add_rejected() {
        let mut cfg = empty_cfg();
        cfg.providers.push(provider("foo"));
        let auth = AuthPayload::Passthrough;
        let r = validate_provider_form(&inputs("foo", &auth), &cfg);
        assert_eq!(r, Err(FormError::DuplicateName));
    }

    #[test]
    fn duplicate_name_on_edit_at_different_index_rejected() {
        let mut cfg = empty_cfg();
        cfg.providers.push(provider("foo"));
        cfg.providers.push(provider("bar"));
        let auth = AuthPayload::Passthrough;
        let mut input = inputs("foo", &auth);
        input.editing_index = Some(1); // editing "bar", trying to rename to "foo"
        input.original_name = Some("bar");
        let r = validate_provider_form(&input, &cfg);
        assert_eq!(r, Err(FormError::DuplicateName));
    }

    #[test]
    fn edit_keeping_same_name_allowed() {
        let mut cfg = empty_cfg();
        cfg.providers.push(provider("foo"));
        let auth = AuthPayload::ApiKey {
            value: "k".into(),
        };
        let mut input = inputs("foo", &auth);
        input.editing_index = Some(0);
        input.original_name = Some("foo");
        let p = validate_provider_form(&input, &cfg).unwrap();
        assert_eq!(p.name, "foo");
    }

    #[test]
    fn empty_api_key_value_rejected() {
        let cfg = empty_cfg();
        let auth = AuthPayload::ApiKey {
            value: "   ".into(),
        };
        let r = validate_provider_form(&inputs("foo", &auth), &cfg);
        assert_eq!(r, Err(FormError::EmptyAuthValue));
    }

    #[test]
    fn empty_bearer_value_rejected() {
        let cfg = empty_cfg();
        let auth = AuthPayload::Bearer { value: "".into() };
        let r = validate_provider_form(&inputs("foo", &auth), &cfg);
        assert_eq!(r, Err(FormError::EmptyAuthValue));
    }

    #[test]
    fn passthrough_with_no_value_ok() {
        let cfg = empty_cfg();
        let auth = AuthPayload::Passthrough;
        let p = validate_provider_form(&inputs("foo", &auth), &cfg).unwrap();
        assert!(matches!(p.auth, AuthPayload::Passthrough));
    }

    #[test]
    fn rename_blocked_by_routing_reference() {
        let mut cfg = empty_cfg();
        cfg.providers.push(provider("foo"));
        cfg.routing.push(rule("*", "foo", &[]));
        let auth = AuthPayload::Passthrough;
        let mut input = inputs("bar", &auth);
        input.editing_index = Some(0);
        input.original_name = Some("foo");
        let r = validate_provider_form(&input, &cfg);
        match r {
            Err(FormError::RenameBlockedBy(rules)) => assert_eq!(rules.len(), 1),
            other => panic!("expected RenameBlockedBy, got {:?}", other),
        }
    }

    #[test]
    fn rules_referencing_finds_primary_and_fallback() {
        let mut cfg = empty_cfg();
        cfg.routing.push(rule("opus-*", "a", &["b"]));
        cfg.routing.push(rule("*", "b", &["a", "c"]));
        let refs = rules_referencing("a", &cfg);
        assert_eq!(refs.len(), 2);
        assert!(refs[0].contains("provider=a"));
        assert!(refs[1].contains("fallback=a"));
    }

    #[test]
    fn rules_referencing_returns_empty_when_unreferenced() {
        let mut cfg = empty_cfg();
        cfg.routing.push(rule("*", "x", &["y"]));
        assert!(rules_referencing("z", &cfg).is_empty());
    }

    #[test]
    fn base_urls_normalized_and_optional() {
        let cfg = empty_cfg();
        let auth = AuthPayload::Passthrough;
        let mut input = inputs("foo", &auth);
        input.base_url = Some("  https://api.example  ");
        input.openai_base_url = Some("");
        let p = validate_provider_form(&input, &cfg).unwrap();
        assert_eq!(p.base_url.as_deref(), Some("https://api.example"));
        assert_eq!(p.openai_base_url, None);
    }
}
```

Add the module declaration to `crates/proxy-tui/src/main.rs`. The file currently begins:

```rust
mod app;
mod client;
mod terminal;
mod ui;
mod views;
```

Change to:

```rust
mod app;
mod client;
mod terminal;
mod ui;
mod validate;
mod views;
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p proxy-tui --lib validate`
Expected: 12 tests fail with compile or assertion errors. Tests **don't** fail-by-not-found because we wrote them in the same file as the implementation; they fail because the implementation doesn't yet exist or because we just wrote both. **Skip step 2 if both file content and implementation were written together — this task uses test-first by writing the tests before tweaking the impl.** If you wrote them together, run instead:

Run: `cargo test -p proxy-tui --lib validate`
Expected: PASS (because we wrote impl alongside tests in this task; this is the green phase).

- [ ] **Step 3: Confirm clippy is clean**

Run: `cargo clippy --workspace -- -D warnings`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/proxy-tui/src/validate.rs crates/proxy-tui/src/main.rs
git commit -m "feat(proxy-tui): pure validation module for provider form

Adds FormError + validate_provider_form + rules_referencing with
table-driven tests. No callers yet."
```

---

### Task 2: New form/delete state types in `app.rs`

**Files:**
- Modify: `crates/proxy-tui/src/app.rs:81-175` (replace `Modal::EditAuth`, `EditAuthModal`, `EditState`)

This task adds the new types **alongside** the existing `EditAuthModal` so the build stays green. We will remove `EditAuthModal` in Task 5 once all callers point to the new modal.

- [ ] **Step 1: Append new types to `app.rs`**

Append the following block to `crates/proxy-tui/src/app.rs` (after line 175, just before `pub struct AppState`):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Anthropic,
    Zai,
}

impl ProviderKind {
    pub fn label(self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::Zai => "zai",
        }
    }
    pub fn cycle_next(self) -> Self {
        match self {
            ProviderKind::Anthropic => ProviderKind::Zai,
            ProviderKind::Zai => ProviderKind::Anthropic,
        }
    }
    pub fn cycle_prev(self) -> Self {
        self.cycle_next() // only two variants, so prev == next
    }
    pub fn from_str_or_default(s: &str) -> Self {
        match s {
            "zai" => ProviderKind::Zai,
            _ => ProviderKind::Anthropic,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormField {
    Name,
    Kind,
    BaseUrl,
    OpenaiBaseUrl,
    AuthKind,
    AuthValue,
    Save,
}

impl FormField {
    pub fn next(self, auth_kind: AuthInputKind) -> Self {
        let order = field_order(auth_kind);
        let idx = order.iter().position(|f| *f == self).unwrap_or(0);
        order[(idx + 1) % order.len()]
    }
    pub fn prev(self, auth_kind: AuthInputKind) -> Self {
        let order = field_order(auth_kind);
        let idx = order.iter().position(|f| *f == self).unwrap_or(0);
        order[(idx + order.len() - 1) % order.len()]
    }
}

/// Field traversal order. AuthValue is omitted when the auth kind doesn't
/// need a typed value.
fn field_order(auth_kind: AuthInputKind) -> &'static [FormField] {
    match auth_kind {
        AuthInputKind::Passthrough | AuthInputKind::OAuthAnthropic => &[
            FormField::Name,
            FormField::Kind,
            FormField::BaseUrl,
            FormField::OpenaiBaseUrl,
            FormField::AuthKind,
            FormField::Save,
        ],
        AuthInputKind::ApiKey | AuthInputKind::Bearer => &[
            FormField::Name,
            FormField::Kind,
            FormField::BaseUrl,
            FormField::OpenaiBaseUrl,
            FormField::AuthKind,
            FormField::AuthValue,
            FormField::Save,
        ],
    }
}

#[derive(Debug, Clone)]
pub enum FormMode {
    Add,
    Edit {
        original_index: usize,
        original_name: String,
    },
}

#[derive(Debug, Clone)]
pub enum FormState {
    Editing,
    Saving,
    OAuthAwaitingCode {
        authorization_url: String,
        state_id: String,
        code_input: String,
    },
    OAuthExchanging,
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct ProviderFormModal {
    pub mode: FormMode,
    pub focused: FormField,
    pub name: String,
    pub kind: ProviderKind,
    pub base_url: String,
    pub openai_base_url: String,
    pub auth_kind: AuthInputKind,
    pub auth_value: String,
    pub state: FormState,
    /// Inline validation error rendered red at top of modal. Cleared on
    /// any field edit.
    pub error: Option<String>,
}

impl ProviderFormModal {
    pub fn new_for_add() -> Self {
        Self {
            mode: FormMode::Add,
            focused: FormField::Name,
            name: String::new(),
            kind: ProviderKind::Anthropic,
            base_url: String::new(),
            openai_base_url: String::new(),
            auth_kind: AuthInputKind::Passthrough,
            auth_value: String::new(),
            state: FormState::Editing,
            error: None,
        }
    }

    pub fn from_provider(index: usize, p: &ProviderPayload) -> Self {
        let auth_kind = AuthInputKind::from_payload(&p.auth);
        let auth_value = match &p.auth {
            AuthPayload::ApiKey { value } | AuthPayload::Bearer { value } => value.clone(),
            _ => String::new(),
        };
        Self {
            mode: FormMode::Edit {
                original_index: index,
                original_name: p.name.clone(),
            },
            focused: FormField::Name,
            name: p.name.clone(),
            kind: ProviderKind::from_str_or_default(&p.kind),
            base_url: p.base_url.clone().unwrap_or_default(),
            openai_base_url: p.openai_base_url.clone().unwrap_or_default(),
            auth_kind,
            auth_value,
            state: FormState::Editing,
            error: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DeleteConfirmModal {
    pub provider_index: usize,
    pub provider_name: String,
    /// Non-empty means delete is blocked. UI must not offer `[y]` in that case.
    pub blocking_rules: Vec<String>,
}
```

Then extend the existing `Modal` enum at line 81:

```rust
#[derive(Debug, Clone)]
pub enum Modal {
    None,
    TestProvider(TestProviderModal),
    EditAuth(EditAuthModal),                       // removed in Task 5
    ProviderForm(ProviderFormModal),               // new
    DeleteConfirm(DeleteConfirmModal),             // new
}
```

- [ ] **Step 2: Add tests for navigation helpers**

Append at the bottom of `crates/proxy-tui/src/app.rs`:

```rust
#[cfg(test)]
mod form_field_tests {
    use super::*;

    #[test]
    fn next_wraps_past_save_back_to_name() {
        let f = FormField::Save;
        assert_eq!(f.next(AuthInputKind::ApiKey), FormField::Name);
    }

    #[test]
    fn prev_wraps_from_name_to_save() {
        let f = FormField::Name;
        assert_eq!(f.prev(AuthInputKind::ApiKey), FormField::Save);
    }

    #[test]
    fn passthrough_skips_auth_value_field() {
        let f = FormField::AuthKind;
        assert_eq!(f.next(AuthInputKind::Passthrough), FormField::Save);
    }

    #[test]
    fn api_key_includes_auth_value_field() {
        let f = FormField::AuthKind;
        assert_eq!(f.next(AuthInputKind::ApiKey), FormField::AuthValue);
    }

    #[test]
    fn provider_kind_cycle() {
        assert_eq!(ProviderKind::Anthropic.cycle_next(), ProviderKind::Zai);
        assert_eq!(ProviderKind::Zai.cycle_next(), ProviderKind::Anthropic);
    }
}
```

- [ ] **Step 3: Verify build + tests**

Run: `cargo test -p proxy-tui --lib`
Expected: PASS (new tests + existing range_preset tests + validate tests).

Run: `cargo clippy --workspace -- -D warnings`
Expected: clean. (`Modal::ProviderForm` and `Modal::DeleteConfirm` are added but unused — `#[allow(dead_code)]` is **not** added; they will be used in subsequent tasks. If clippy complains about dead variants, accept it — Rust does **not** warn on unused enum variants by default, only on unused functions/structs.)

- [ ] **Step 4: Commit**

```bash
git add crates/proxy-tui/src/app.rs
git commit -m "feat(proxy-tui): add ProviderFormModal/DeleteConfirmModal state types

Coexists with the existing EditAuthModal. Field navigation helpers
covered by unit tests. Wiring follows in subsequent commits."
```

---

### Task 3: Render form + delete-confirm modals in `ui.rs`

**Files:**
- Modify: `crates/proxy-tui/src/ui.rs:5-10` (imports), `:30-50` (modal dispatch), append two new render functions.

Now that the types exist, give them a renderer so future key dispatch tasks have something to display.

- [ ] **Step 1: Update imports**

Open `crates/proxy-tui/src/ui.rs`. The first `use crate::app::...;` line (around line 5) currently imports `EditAuthModal, EditState`. Add the new types alongside (do not remove `EditAuthModal` yet):

```rust
use crate::app::{
    ALL_VIEWS, AppState, AuthInputKind, DeleteConfirmModal, EditAuthModal, EditState, FormField,
    FormMode, FormState, Modal, ProviderFormModal, ProviderKind, TestProviderModal,
};
```

- [ ] **Step 2: Wire the new modals into the dispatch**

Find the modal-dispatch `match` (around line 42) that currently looks like:

```rust
        Modal::EditAuth(m) => draw_edit_modal(f, m),
```

Add two arms above it (or after — order does not matter, no overlap):

```rust
        Modal::ProviderForm(m) => draw_form_modal(f, m),
        Modal::DeleteConfirm(m) => draw_delete_confirm_modal(f, m),
```

- [ ] **Step 3: Add `draw_form_modal`**

Append at the end of `crates/proxy-tui/src/ui.rs`:

```rust
fn draw_form_modal(f: &mut Frame, m: &ProviderFormModal) {
    let area = centered_rect(70, 60, f.area());
    f.render_widget(Clear, area);
    let title = match &m.mode {
        FormMode::Add => " Add provider ".to_string(),
        FormMode::Edit { original_name, .. } => format!(" Edit provider: {original_name} "),
    };
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();

    if let Some(err) = &m.error {
        lines.push(Line::from(Span::styled(
            format!("⚠ {err}"),
            Style::default().fg(Color::Red),
        )));
        lines.push(Line::from(""));
    }

    let row = |field: FormField, label: &str, value: String| {
        let marker = if m.focused == field { "▶ " } else { "  " };
        let style = if m.focused == field {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        Line::from(vec![
            Span::styled(format!("{marker}{label:<18}"), style),
            Span::raw(value),
        ])
    };

    lines.push(row(FormField::Name, "Name:", show_or_placeholder(&m.name)));
    lines.push(row(
        FormField::Kind,
        "Kind:",
        format!("< {} >    [←/→ to cycle]", m.kind.label()),
    ));
    lines.push(row(
        FormField::BaseUrl,
        "Base URL:",
        show_or_placeholder(&m.base_url),
    ));
    lines.push(row(
        FormField::OpenaiBaseUrl,
        "OpenAI Base URL:",
        show_or_placeholder(&m.openai_base_url),
    ));
    lines.push(row(
        FormField::AuthKind,
        "Auth Kind:",
        format!("< {} >    [←/→ to cycle]", m.auth_kind.label()),
    ));
    if matches!(
        m.auth_kind,
        AuthInputKind::ApiKey | AuthInputKind::Bearer
    ) {
        let masked = "*".repeat(m.auth_value.chars().count().min(40));
        let display = if m.auth_value.is_empty() {
            "<empty>".into()
        } else {
            masked
        };
        lines.push(row(FormField::AuthValue, "Auth Value:", display));
    }
    lines.push(Line::from(""));
    lines.push(row(
        FormField::Save,
        "[ Save ]",
        "(Enter to submit)".into(),
    ));
    lines.push(Line::from(""));

    match &m.state {
        FormState::Editing => {
            lines.push(Line::from(Span::styled(
                "Tab/Shift+Tab: move  ←/→: cycle  Enter on Save: submit  Esc: cancel",
                Style::default().fg(Color::DarkGray),
            )));
        }
        FormState::Saving => {
            lines.push(Line::from(Span::styled(
                "saving…",
                Style::default().fg(Color::Yellow),
            )));
        }
        FormState::OAuthAwaitingCode {
            authorization_url,
            code_input,
            ..
        } => {
            lines.push(Line::from(Span::styled(
                "1. Open this URL in a browser:",
                Style::default().add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(Span::styled(
                authorization_url.clone(),
                Style::default().fg(Color::Cyan),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "2. After authorising, Anthropic shows a `code#state` value.",
                Style::default().add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(
                "   Copy the `code` part (everything before the `#`) and paste here:",
            ));
            lines.push(Line::from(format!(
                "   code: {}",
                if code_input.is_empty() {
                    "<paste here>".into()
                } else {
                    code_input.clone()
                }
            )));
            lines.push(Line::from(""));
            lines.push(Line::from("Enter: exchange  Esc: cancel"));
        }
        FormState::OAuthExchanging => {
            lines.push(Line::from(Span::styled(
                "exchanging code for token…",
                Style::default().fg(Color::Yellow),
            )));
        }
        FormState::Failed(e) => {
            lines.push(Line::from(Span::styled(
                format!("error: {e}"),
                Style::default().fg(Color::Red),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(
                "Enter on Save: retry  Esc: close",
            ));
        }
    }

    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn show_or_placeholder(s: &str) -> String {
    if s.is_empty() {
        "<empty>".into()
    } else {
        s.into()
    }
}

fn draw_delete_confirm_modal(f: &mut Frame, m: &DeleteConfirmModal) {
    let area = centered_rect(60, 40, f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Delete provider: {} ", m.provider_name));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();
    if m.blocking_rules.is_empty() {
        lines.push(Line::from(format!(
            "Delete provider '{}'?",
            m.provider_name
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "[y] yes   [n / Esc] no",
            Style::default().add_modifier(Modifier::BOLD),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            format!(
                "Cannot delete '{}' — referenced by {} routing rule(s):",
                m.provider_name,
                m.blocking_rules.len()
            ),
            Style::default().fg(Color::Red),
        )));
        for r in &m.blocking_rules {
            lines.push(Line::from(format!("  • {r}")));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(
            "Resolve routing rules first.",
        ));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "[Esc] close",
            Style::default().add_modifier(Modifier::BOLD),
        )));
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}
```

- [ ] **Step 4: Verify build**

Run: `cargo check --workspace`
Expected: clean.

Run: `cargo clippy --workspace -- -D warnings`
Expected: clean. The new render functions are unused (no key handler opens them yet). Rust does **not** warn on unused private functions when they are *referenced* (the modal-dispatch arm calls them). If clippy still flags them, double-check the dispatch arms were added in Step 2.

- [ ] **Step 5: Commit**

```bash
git add crates/proxy-tui/src/ui.rs
git commit -m "feat(proxy-tui): render ProviderFormModal and DeleteConfirmModal

Visual scaffolding only — no key handlers open these modals yet."
```

---

### Task 4: Wire `a` key + form navigation handler

**Files:**
- Modify: `crates/proxy-tui/src/main.rs:12-15` (imports), `:87-90` (modal-routing guard), `:157-160` (key dispatch), append `open_add_modal` + `handle_form_key`.

This task makes pressing `a` open the Add modal and handle navigation/typing **without** save logic. Save comes in Task 6.

- [ ] **Step 1: Update imports**

In `crates/proxy-tui/src/main.rs` line 12-15, replace the `use crate::app::...` block with:

```rust
use crate::app::{
    AppState, AuthInputKind, DeleteConfirmModal, EditAuthModal, EditState, FormField, FormMode,
    FormState, Modal, ProviderFormModal, ProviderKind, RangePreset, TestProviderModal, TestState,
    View,
};
```

- [ ] **Step 2: Route `Modal::ProviderForm` to a new handler**

Find the modal-guard at line 87:

```rust
    if let Modal::TestProvider(_) | Modal::EditAuth(_) = &state.modal {
        handle_modal_key(k, client, state);
        return;
    }
```

Change to:

```rust
    if let Modal::TestProvider(_) | Modal::EditAuth(_) | Modal::ProviderForm(_) | Modal::DeleteConfirm(_) =
        &state.modal
    {
        handle_modal_key(k, client, state);
        return;
    }
```

- [ ] **Step 3: Find `handle_modal_key` and add new dispatch arms**

Locate `fn handle_modal_key` (around line 200). It currently dispatches by taking ownership of the modal value:

```rust
fn handle_modal_key(k: KeyEvent, client: &AdminClient, state: &mut AppState) {
    let modal = std::mem::replace(&mut state.modal, Modal::None);
    let next = match modal {
        Modal::TestProvider(m) => handle_test_key(k, client, state, m),
        Modal::EditAuth(m) => handle_edit_key(k, client, state, m),
        Modal::None => Modal::None,
    };
    state.modal = next;
}
```

(Adapt to whatever the actual signature is; pattern is `take ownership → match → produce next Modal → restore`.)

Add two arms:

```rust
fn handle_modal_key(k: KeyEvent, client: &AdminClient, state: &mut AppState) {
    let modal = std::mem::replace(&mut state.modal, Modal::None);
    let next = match modal {
        Modal::TestProvider(m) => handle_test_key(k, client, state, m),
        Modal::EditAuth(m) => handle_edit_key(k, client, state, m),
        Modal::ProviderForm(m) => handle_form_key(k, client, state, m),
        Modal::DeleteConfirm(m) => handle_delete_key(k, client, state, m),
        Modal::None => Modal::None,
    };
    state.modal = next;
}
```

- [ ] **Step 4: Add `a` keybind in Providers view**

Find the Providers-view key match around line 157:

```rust
        KeyCode::Char('t') if state.view == View::Providers => open_test_modal(state),
        KeyCode::Char('e') if state.view == View::Providers => open_edit_modal(state),
```

Add:

```rust
        KeyCode::Char('a') if state.view == View::Providers => open_add_modal(state),
```

(Leave `e` and `d` for later tasks. `e` still goes to the old `open_edit_modal` until Task 5.)

- [ ] **Step 5: Add `open_add_modal` + `handle_form_key` + stub `handle_delete_key`**

Append to `crates/proxy-tui/src/main.rs`:

```rust
fn open_add_modal(state: &mut AppState) {
    state.modal = Modal::ProviderForm(ProviderFormModal::new_for_add());
}

fn handle_form_key(
    k: KeyEvent,
    _client: &AdminClient,
    _state: &mut AppState,
    mut m: ProviderFormModal,
) -> Modal {
    // Navigation + editing only — Save behavior arrives in a later task.
    match (&m.state, k.code) {
        (_, KeyCode::Esc) => Modal::None,
        (FormState::Editing, KeyCode::Tab) => {
            m.focused = m.focused.next(m.auth_kind);
            Modal::ProviderForm(m)
        }
        (FormState::Editing, KeyCode::BackTab) => {
            m.focused = m.focused.prev(m.auth_kind);
            Modal::ProviderForm(m)
        }
        (FormState::Editing, KeyCode::Left) => {
            cycle_field_value(&mut m, false);
            Modal::ProviderForm(m)
        }
        (FormState::Editing, KeyCode::Right) => {
            cycle_field_value(&mut m, true);
            Modal::ProviderForm(m)
        }
        (FormState::Editing, KeyCode::Backspace) => {
            edit_focused_text(&mut m, |s| {
                s.pop();
            });
            Modal::ProviderForm(m)
        }
        (FormState::Editing, KeyCode::Char(c)) => {
            edit_focused_text(&mut m, |s| s.push(c));
            Modal::ProviderForm(m)
        }
        // Save handling, OAuth substates: added in Tasks 6/7/8.
        _ => Modal::ProviderForm(m),
    }
}

fn cycle_field_value(m: &mut ProviderFormModal, forward: bool) {
    m.error = None;
    match m.focused {
        FormField::Kind => {
            m.kind = if forward {
                m.kind.cycle_next()
            } else {
                m.kind.cycle_prev()
            };
        }
        FormField::AuthKind => {
            // AuthInputKind only has cycle(); use it for both directions
            // (4 variants → cycling 3 times == reverse). Fine for a TUI.
            m.auth_kind = if forward {
                m.auth_kind.cycle()
            } else {
                m.auth_kind.cycle().cycle().cycle()
            };
            // If switching to a kind without AuthValue, move focus off it.
            if matches!(
                m.auth_kind,
                AuthInputKind::Passthrough | AuthInputKind::OAuthAnthropic
            ) && m.focused == FormField::AuthValue
            {
                m.focused = FormField::AuthKind;
            }
        }
        _ => {}
    }
}

fn edit_focused_text(m: &mut ProviderFormModal, f: impl FnOnce(&mut String)) {
    m.error = None;
    let target: Option<&mut String> = match m.focused {
        FormField::Name => Some(&mut m.name),
        FormField::BaseUrl => Some(&mut m.base_url),
        FormField::OpenaiBaseUrl => Some(&mut m.openai_base_url),
        FormField::AuthValue => Some(&mut m.auth_value),
        _ => None,
    };
    if let Some(s) = target {
        f(s);
    }
}

fn handle_delete_key(
    _k: KeyEvent,
    _client: &AdminClient,
    _state: &mut AppState,
    m: DeleteConfirmModal,
) -> Modal {
    // Real delete handling lands in Task 8.
    Modal::DeleteConfirm(m)
}
```

- [ ] **Step 6: Verify build, run, smoke**

Run: `cargo test -p proxy-tui --lib`
Expected: PASS (no new tests added in this task).

Run: `cargo clippy --workspace -- -D warnings`
Expected: clean.

Manual smoke: start the proxy daemon (`cargo run -p proxy`) and TUI (`cargo run -p proxy-tui`), navigate to Providers tab (`2`), press `a`. The Add modal should open. `Tab`/`Shift+Tab` should move the `▶` cursor between fields. `←`/`→` on Kind and Auth Kind should cycle. Typing in Name field should add characters; Backspace should remove them. `Esc` should close the modal. Save does **nothing** yet (intentional).

- [ ] **Step 7: Commit**

```bash
git add crates/proxy-tui/src/main.rs
git commit -m "feat(proxy-tui): wire 'a' key to Add provider modal with navigation

Form modal opens, fields navigate with Tab/Shift+Tab, enum fields cycle
with arrows, text fields accept input. Save and OAuth flows land in
follow-up commits."
```

---

### Task 5: Replace `e` key to open `ProviderFormModal::Edit`; remove `EditAuthModal`

**Files:**
- Modify: `crates/proxy-tui/src/main.rs:158, 244-376` (replace `open_edit_modal`, drop `handle_edit_key`, drop `save_provider_auth`).
- Modify: `crates/proxy-tui/src/app.rs:81-110, 161-175` (drop `EditAuthModal`, `EditState` from `Modal` enum).
- Modify: `crates/proxy-tui/src/ui.rs:5-10, 42, 514+` (drop `draw_edit_modal` + imports).

This task is the largest. It removes the legacy `EditAuthModal` entirely. Edit-flow OAuth is **not** handled in this task — at the end of this task, pressing `e` opens the new form, navigation works, Save with non-OAuth auth kinds works (assuming Task 6 has not yet landed: Save will produce a no-op as in Task 4). To keep the build runnable, this task **must run before Task 6** but **after Task 4**. We accept temporary regression of the OAuth-edit flow during the gap between Tasks 5 and 7.

- [ ] **Step 1: Replace `open_edit_modal` body**

In `crates/proxy-tui/src/main.rs`, find `fn open_edit_modal` (around line 244) and replace its body:

```rust
fn open_edit_modal(state: &mut AppState) {
    let Some(prov) = state.selected_provider() else {
        return;
    };
    let modal = ProviderFormModal::from_provider(state.providers_selected, prov);
    state.modal = Modal::ProviderForm(modal);
}
```

- [ ] **Step 2: Remove the legacy edit handler + save helper**

Delete `fn handle_edit_key` (around lines 264–355) and `fn save_provider_auth` (lines 357–376) entirely. They are no longer referenced.

- [ ] **Step 3: Remove the `Modal::EditAuth` arm from `handle_modal_key`**

Find `handle_modal_key` and remove the `Modal::EditAuth(m) => handle_edit_key(...)` arm.

- [ ] **Step 4: Remove `EditAuthModal` and `EditState` from `app.rs`**

In `crates/proxy-tui/src/app.rs`:

1. Delete `pub struct EditAuthModal { ... }` and `pub enum EditState { ... }`.
2. Remove `EditAuth(EditAuthModal),` from the `Modal` enum.

The result is:

```rust
#[derive(Debug, Clone)]
pub enum Modal {
    None,
    TestProvider(TestProviderModal),
    ProviderForm(ProviderFormModal),
    DeleteConfirm(DeleteConfirmModal),
}
```

- [ ] **Step 5: Remove `draw_edit_modal` and update imports in `ui.rs`**

1. Delete `fn draw_edit_modal` (around line 514 to its closing `}`).
2. Update the import line to drop `EditAuthModal, EditState`:

```rust
use crate::app::{
    ALL_VIEWS, AppState, AuthInputKind, DeleteConfirmModal, FormField, FormMode, FormState, Modal,
    ProviderFormModal, ProviderKind, TestProviderModal,
};
```

3. Remove the `Modal::EditAuth(m) => draw_edit_modal(f, m),` arm from the modal-dispatch match (around line 42).

- [ ] **Step 6: Update the modal guard in `main.rs`**

Find the guard added in Task 4:

```rust
    if let Modal::TestProvider(_) | Modal::EditAuth(_) | Modal::ProviderForm(_) | Modal::DeleteConfirm(_) =
```

Drop the `EditAuth(_)` term:

```rust
    if let Modal::TestProvider(_) | Modal::ProviderForm(_) | Modal::DeleteConfirm(_) =
```

Also update the import block at the top of `main.rs` to drop `EditAuthModal, EditState`:

```rust
use crate::app::{
    AppState, AuthInputKind, DeleteConfirmModal, FormField, FormMode, FormState, Modal,
    ProviderFormModal, ProviderKind, RangePreset, TestProviderModal, TestState, View,
};
```

- [ ] **Step 7: Verify build, run, smoke**

Run: `cargo check --workspace`
Expected: clean.

Run: `cargo test -p proxy-tui`
Expected: PASS.

Run: `cargo clippy --workspace -- -D warnings`
Expected: clean.

Manual smoke: in the TUI, navigate to Providers (`2`), select a provider with `j`/`k`, press `e`. The new form opens prefilled with the provider's `name`, `kind`, urls, and auth kind. Navigation and field editing work; Save still does nothing (Task 6 wires it).

- [ ] **Step 8: Commit**

```bash
git add crates/proxy-tui/src/app.rs crates/proxy-tui/src/main.rs crates/proxy-tui/src/ui.rs
git commit -m "refactor(proxy-tui): remove EditAuthModal, route 'e' to ProviderFormModal

Edit modal is now the unified form. Save and OAuth handling arrive in
the next two commits."
```

---

### Task 6: Implement non-OAuth Save (validation + PUT) — covers both Add and Edit

**Files:**
- Modify: `crates/proxy-tui/src/main.rs` (extend `handle_form_key`, add `submit_non_oauth_save`).

After this task, Add and Edit work end-to-end for `passthrough`, `api_key`, and `bearer` auth kinds. OAuth handling is added in Tasks 7 & 8.

- [ ] **Step 1: Extend `handle_form_key` to handle Enter on Save**

Replace the placeholder `_ => Modal::ProviderForm(m)` arm at the bottom of `handle_form_key` with explicit Save handling. The full updated function:

```rust
fn handle_form_key(
    k: KeyEvent,
    client: &AdminClient,
    state: &mut AppState,
    mut m: ProviderFormModal,
) -> Modal {
    match (&m.state, k.code) {
        (_, KeyCode::Esc) => Modal::None,

        (FormState::Editing, KeyCode::Tab) => {
            m.focused = m.focused.next(m.auth_kind);
            Modal::ProviderForm(m)
        }
        (FormState::Editing, KeyCode::BackTab) => {
            m.focused = m.focused.prev(m.auth_kind);
            Modal::ProviderForm(m)
        }
        (FormState::Editing, KeyCode::Left) => {
            cycle_field_value(&mut m, false);
            Modal::ProviderForm(m)
        }
        (FormState::Editing, KeyCode::Right) => {
            cycle_field_value(&mut m, true);
            Modal::ProviderForm(m)
        }
        (FormState::Editing, KeyCode::Enter) if m.focused == FormField::Save => {
            // Non-OAuth save now; OAuth paths added in Tasks 7/8.
            if m.auth_kind == AuthInputKind::OAuthAnthropic {
                m.error = Some("OAuth save not yet implemented".into());
                return Modal::ProviderForm(m);
            }
            submit_non_oauth_save(client, state, m)
        }
        (FormState::Editing, KeyCode::Backspace) => {
            edit_focused_text(&mut m, |s| {
                s.pop();
            });
            Modal::ProviderForm(m)
        }
        (FormState::Editing, KeyCode::Char(c)) => {
            edit_focused_text(&mut m, |s| s.push(c));
            Modal::ProviderForm(m)
        }
        (FormState::Failed(_), KeyCode::Enter) if m.focused == FormField::Save => {
            // Retry: drop back to Editing and re-submit.
            m.state = FormState::Editing;
            m.error = None;
            handle_form_key(k, client, state, m)
        }
        _ => Modal::ProviderForm(m),
    }
}
```

- [ ] **Step 2: Add `submit_non_oauth_save`**

Append to `crates/proxy-tui/src/main.rs`:

```rust
fn submit_non_oauth_save(
    client: &AdminClient,
    state: &mut AppState,
    mut m: ProviderFormModal,
) -> Modal {
    use crate::validate::{FormInputs, validate_provider_form};
    use proxy_admin_api::AuthPayload;

    let mut cfg = match state.config.as_ref().and_then(|r| r.as_ref().ok()).cloned() {
        Some(c) => c,
        None => {
            m.error = Some("config not loaded".into());
            return Modal::ProviderForm(m);
        }
    };

    let auth_value = m.auth_value.clone();
    let auth = match m.auth_kind {
        AuthInputKind::Passthrough => AuthPayload::Passthrough,
        AuthInputKind::ApiKey => AuthPayload::ApiKey { value: auth_value },
        AuthInputKind::Bearer => AuthPayload::Bearer { value: auth_value },
        AuthInputKind::OAuthAnthropic => unreachable!("OAuth handled separately"),
    };

    let (editing_index, original_name) = match &m.mode {
        FormMode::Add => (None, None),
        FormMode::Edit {
            original_index,
            original_name,
        } => (Some(*original_index), Some(original_name.as_str())),
    };

    let input = FormInputs {
        name: &m.name,
        kind: m.kind.label(),
        base_url: Some(&m.base_url),
        openai_base_url: Some(&m.openai_base_url),
        auth: &auth,
        editing_index,
        original_name,
    };

    let provider = match validate_provider_form(&input, &cfg) {
        Ok(p) => p,
        Err(e) => {
            m.error = Some(format!("{e}"));
            return Modal::ProviderForm(m);
        }
    };

    let display_name = provider.name.clone();
    match &m.mode {
        FormMode::Add => cfg.providers.push(provider),
        FormMode::Edit { original_index, .. } => {
            cfg.providers[*original_index] = provider;
        }
    }

    m.state = FormState::Saving;
    match client.put_config(&cfg) {
        Ok(updated) => {
            state.set_config(Ok(updated));
            let action = match m.mode {
                FormMode::Add => "added",
                FormMode::Edit { .. } => "updated",
            };
            state.flash(format!("{action} {display_name}"));
            Modal::None
        }
        Err(e) => {
            m.state = FormState::Failed(e.to_string());
            Modal::ProviderForm(m)
        }
    }
}
```

- [ ] **Step 3: Verify build + manual smoke**

Run: `cargo test -p proxy-tui`
Expected: PASS.

Run: `cargo clippy --workspace -- -D warnings`
Expected: clean.

Manual smoke against a running proxy:

1. Add: press `a`, fill `Name=test-add`, leave Kind=anthropic, Auth Kind=api_key, Auth Value=`sk-ant-fake`. Tab to `[ Save ]`, press `Enter`. Modal closes; flash shows `added test-add`. Navigate the providers list — new entry visible.
2. Edit: select `test-add`, press `e`, change `Auth Value` to `sk-ant-other`. Tab to Save, Enter. Flash `updated test-add`.
3. Validation: press `a`, leave Name empty, Tab to Save, Enter. Modal stays open with red `name is required`.
4. Inspect `~/.config/cli-router/config.toml` to confirm changes persisted.

- [ ] **Step 4: Commit**

```bash
git add crates/proxy-tui/src/main.rs
git commit -m "feat(proxy-tui): non-OAuth Save for Add/Edit provider

Validates form, mutates cached ConfigPayload, calls PUT /admin/config.
OAuth flows (Add and Edit) land in subsequent commits."
```

---

### Task 7: OAuth Save — Edit path

**Files:**
- Modify: `crates/proxy-tui/src/main.rs` (extend `handle_form_key`; add OAuth substate handlers; add `submit_oauth_edit`).

The Edit-OAuth flow has two phases:
1. If non-auth fields changed (`name`, `kind`, `base_url`, `openai_base_url`), PUT them first while keeping the *original* auth so the daemon's name lookup in OAuth-complete uses the new name.
2. Call `oauth_start` → user pastes code → `oauth_complete`. Daemon mutates auth in place + persists.
3. Refetch config to update local cache.

- [ ] **Step 1: Replace the OAuth stub branch in `handle_form_key`**

In `handle_form_key`, find the branch:

```rust
            if m.auth_kind == AuthInputKind::OAuthAnthropic {
                m.error = Some("OAuth save not yet implemented".into());
                return Modal::ProviderForm(m);
            }
            submit_non_oauth_save(client, state, m)
```

Replace with:

```rust
            if m.auth_kind == AuthInputKind::OAuthAnthropic {
                return match m.mode {
                    FormMode::Add => submit_oauth_add(client, state, m),       // Task 8
                    FormMode::Edit { .. } => submit_oauth_edit(client, state, m),
                };
            }
            submit_non_oauth_save(client, state, m)
```

(Define a stub for `submit_oauth_add` returning a "not implemented yet" error similar to the previous stub, until Task 8 lands. Or write Task 8 immediately after Task 7 — the order is your choice as the executor; the plan covers Task 8 below.)

For now, add the temporary stub:

```rust
fn submit_oauth_add(_client: &AdminClient, _state: &mut AppState, mut m: ProviderFormModal) -> Modal {
    m.error = Some("Add + OAuth not yet implemented".into());
    Modal::ProviderForm(m)
}
```

- [ ] **Step 2: Add `submit_oauth_edit`**

Append to `crates/proxy-tui/src/main.rs`:

```rust
fn submit_oauth_edit(
    client: &AdminClient,
    state: &mut AppState,
    mut m: ProviderFormModal,
) -> Modal {
    use crate::validate::{FormInputs, validate_provider_form};
    use proxy_admin_api::AuthPayload;

    // For Edit + OAuth, validate everything except the auth_value field.
    // We use the *original* auth as the placeholder so we can run validation
    // and PUT non-auth changes first.
    let (original_index, original_name) = match &m.mode {
        FormMode::Edit {
            original_index,
            original_name,
        } => (*original_index, original_name.clone()),
        FormMode::Add => unreachable!("submit_oauth_edit only called on Edit"),
    };

    let mut cfg = match state.config.as_ref().and_then(|r| r.as_ref().ok()).cloned() {
        Some(c) => c,
        None => {
            m.error = Some("config not loaded".into());
            return Modal::ProviderForm(m);
        }
    };

    // Preserve original auth — we run OAuth dance after PUT lands.
    let original_auth = cfg
        .providers
        .get(original_index)
        .map(|p| p.auth.clone())
        .unwrap_or(AuthPayload::Passthrough);

    let input = FormInputs {
        name: &m.name,
        kind: m.kind.label(),
        base_url: Some(&m.base_url),
        openai_base_url: Some(&m.openai_base_url),
        auth: &original_auth,
        editing_index: Some(original_index),
        original_name: Some(&original_name),
    };
    let provider = match validate_provider_form(&input, &cfg) {
        Ok(p) => p,
        Err(e) => {
            m.error = Some(format!("{e}"));
            return Modal::ProviderForm(m);
        }
    };

    // Detect whether non-auth fields changed; if so, PUT first.
    let dirty = {
        let prev = &cfg.providers[original_index];
        prev.name != provider.name
            || prev.kind != provider.kind
            || prev.base_url != provider.base_url
            || prev.openai_base_url != provider.openai_base_url
    };

    let new_provider_name = provider.name.clone();
    cfg.providers[original_index] = provider;

    if dirty {
        m.state = FormState::Saving;
        match client.put_config(&cfg) {
            Ok(updated) => state.set_config(Ok(updated)),
            Err(e) => {
                m.state = FormState::Failed(e.to_string());
                return Modal::ProviderForm(m);
            }
        }
    }

    // Now kick off the OAuth dance. Daemon will look up by `new_provider_name`.
    m.state = FormState::Saving;
    match client.oauth_start(&new_provider_name) {
        Ok(resp) => {
            m.state = FormState::OAuthAwaitingCode {
                authorization_url: resp.authorization_url,
                state_id: resp.state_id,
                code_input: String::new(),
            };
        }
        Err(e) => {
            m.state = FormState::Failed(e.to_string());
        }
    }
    Modal::ProviderForm(m)
}
```

- [ ] **Step 3: Add OAuth substate handlers to `handle_form_key`**

Add these arms inside `handle_form_key`'s `match` (alongside the `FormState::Editing` ones):

```rust
        (FormState::OAuthAwaitingCode { .. }, KeyCode::Backspace) => {
            if let FormState::OAuthAwaitingCode { code_input, .. } = &mut m.state {
                code_input.pop();
            }
            Modal::ProviderForm(m)
        }
        (FormState::OAuthAwaitingCode { .. }, KeyCode::Char(c)) => {
            if let FormState::OAuthAwaitingCode { code_input, .. } = &mut m.state {
                code_input.push(c);
            }
            Modal::ProviderForm(m)
        }
        (FormState::OAuthAwaitingCode { .. }, KeyCode::Enter) => {
            let (state_id, code, provider_name) = match (&m.state, &m.mode) {
                (
                    FormState::OAuthAwaitingCode {
                        state_id,
                        code_input,
                        ..
                    },
                    _,
                ) => (state_id.clone(), code_input.clone(), m.name.clone()),
                _ => unreachable!(),
            };
            if code.trim().is_empty() {
                return Modal::ProviderForm(m);
            }
            m.state = FormState::OAuthExchanging;
            match client.oauth_complete(&state_id, code.trim(), &provider_name) {
                Ok(resp) if resp.success => {
                    if let Some(updated) = resp.config {
                        state.set_config(Ok(updated));
                    } else {
                        state.set_config(client.get_config().map_err(|e| e.to_string()));
                    }
                    state.flash(format!("OAuth complete — {provider_name} updated"));
                    Modal::None
                }
                Ok(resp) => {
                    m.state = FormState::Failed(
                        resp.error.unwrap_or_else(|| "unknown OAuth failure".into()),
                    );
                    Modal::ProviderForm(m)
                }
                Err(e) => {
                    m.state = FormState::Failed(e.to_string());
                    Modal::ProviderForm(m)
                }
            }
        }
```

- [ ] **Step 4: Verify build + smoke**

Run: `cargo test -p proxy-tui`
Expected: PASS.

Run: `cargo clippy --workspace -- -D warnings`
Expected: clean.

Manual smoke (requires a real Anthropic OAuth login):

1. Edit an existing provider, switch Auth Kind to `oauth (anthropic)`, Tab to Save, Enter.
2. Modal shows the authorization URL. Open in browser, log in.
3. Paste the `code` value from the redirect page, Enter.
4. Flash shows `OAuth complete — <name> updated`. Inspect config.toml to see `auth = { type = "anthropic_oauth", ... }`.

- [ ] **Step 5: Commit**

```bash
git add crates/proxy-tui/src/main.rs
git commit -m "feat(proxy-tui): OAuth Anthropic flow on Edit

PUT non-auth changes first (preserving original auth), then run the
OAuth start/paste/exchange dance against the daemon."
```

---

### Task 8: OAuth Save — Add path (two-step)

**Files:**
- Modify: `crates/proxy-tui/src/main.rs` (replace `submit_oauth_add` stub).

The Add-OAuth flow:
1. Validate form using `auth = Passthrough` placeholder.
2. PUT cfg with the new provider (auth=Passthrough). On failure, surface error.
3. `oauth_start(name)` → enter `OAuthAwaitingCode`. The OAuth substate handlers from Task 7 already handle paste + exchange. The daemon mutates the just-added provider's auth in place.
4. On `oauth_complete` success, refetch config so cached `auth` reflects the AnthropicOAuth tokens.
5. On `oauth_complete` failure, leave the provider in config with `auth=Passthrough`. User can Esc, then Edit to retry or Delete.

- [ ] **Step 1: Replace the stub `submit_oauth_add`**

Replace the stub:

```rust
fn submit_oauth_add(_client: &AdminClient, _state: &mut AppState, mut m: ProviderFormModal) -> Modal {
    m.error = Some("Add + OAuth not yet implemented".into());
    Modal::ProviderForm(m)
}
```

with the real implementation:

```rust
fn submit_oauth_add(
    client: &AdminClient,
    state: &mut AppState,
    mut m: ProviderFormModal,
) -> Modal {
    use crate::validate::{FormInputs, validate_provider_form};
    use proxy_admin_api::AuthPayload;

    let mut cfg = match state.config.as_ref().and_then(|r| r.as_ref().ok()).cloned() {
        Some(c) => c,
        None => {
            m.error = Some("config not loaded".into());
            return Modal::ProviderForm(m);
        }
    };

    // Validate with placeholder Passthrough auth so we can PUT first.
    let placeholder_auth = AuthPayload::Passthrough;
    let input = FormInputs {
        name: &m.name,
        kind: m.kind.label(),
        base_url: Some(&m.base_url),
        openai_base_url: Some(&m.openai_base_url),
        auth: &placeholder_auth,
        editing_index: None,
        original_name: None,
    };
    let provider = match validate_provider_form(&input, &cfg) {
        Ok(p) => p,
        Err(e) => {
            m.error = Some(format!("{e}"));
            return Modal::ProviderForm(m);
        }
    };

    let new_provider_name = provider.name.clone();
    cfg.providers.push(provider);

    m.state = FormState::Saving;
    match client.put_config(&cfg) {
        Ok(updated) => state.set_config(Ok(updated)),
        Err(e) => {
            m.state = FormState::Failed(format!("PUT failed (provider not added): {e}"));
            return Modal::ProviderForm(m);
        }
    }

    // Provider exists in daemon config. Run OAuth dance.
    match client.oauth_start(&new_provider_name) {
        Ok(resp) => {
            m.state = FormState::OAuthAwaitingCode {
                authorization_url: resp.authorization_url,
                state_id: resp.state_id,
                code_input: String::new(),
            };
        }
        Err(e) => {
            m.state = FormState::Failed(format!(
                "provider added but oauth_start failed: {e}\n\
                 Esc, then Edit '{new_provider_name}' to retry."
            ));
        }
    }
    Modal::ProviderForm(m)
}
```

- [ ] **Step 2: Verify build + smoke**

Run: `cargo test -p proxy-tui`
Expected: PASS.

Run: `cargo clippy --workspace -- -D warnings`
Expected: clean.

Manual smoke:

1. Press `a`. Fill `Name=oauth-test`, Auth Kind=`oauth (anthropic)`. Tab to Save, Enter.
2. Provider appears in list (with `auth=passthrough` momentarily — this may flicker depending on refresh timing).
3. Modal shows the OAuth URL.
4. Complete browser login + paste code.
5. After success, flash and config.toml show `auth=anthropic_oauth` for `oauth-test`.

Failure-mode check (optional): run with the proxy unreachable for the second step. Verify the modal goes to `FormState::Failed(...)` with a message that mentions "provider added".

- [ ] **Step 3: Commit**

```bash
git add crates/proxy-tui/src/main.rs
git commit -m "feat(proxy-tui): OAuth Anthropic flow on Add (two-step PUT)

PUTs the new provider with placeholder Passthrough auth so the daemon
knows the name, then runs OAuth dance which mutates the auth in place."
```

---

### Task 9: Delete provider flow

**Files:**
- Modify: `crates/proxy-tui/src/main.rs` (`d` keybind, replace `handle_delete_key` stub, add `open_delete_modal`).

- [ ] **Step 1: Add `d` keybind in Providers view**

Find the Providers-view block (around line 157) where `t`, `a`, `e` are bound. Add:

```rust
        KeyCode::Char('d') if state.view == View::Providers => open_delete_modal(state),
```

- [ ] **Step 2: Add `open_delete_modal`**

```rust
fn open_delete_modal(state: &mut AppState) {
    let Some(prov) = state.selected_provider() else {
        return;
    };
    let cfg = match state.config.as_ref().and_then(|r| r.as_ref().ok()) {
        Some(c) => c,
        None => return,
    };
    let blocking_rules = crate::validate::rules_referencing(&prov.name, cfg);
    state.modal = Modal::DeleteConfirm(DeleteConfirmModal {
        provider_index: state.providers_selected,
        provider_name: prov.name.clone(),
        blocking_rules,
    });
}
```

- [ ] **Step 3: Replace `handle_delete_key` stub with real handler**

```rust
fn handle_delete_key(
    k: KeyEvent,
    client: &AdminClient,
    state: &mut AppState,
    m: DeleteConfirmModal,
) -> Modal {
    match k.code {
        KeyCode::Esc | KeyCode::Char('n') => Modal::None,
        KeyCode::Char('y') if m.blocking_rules.is_empty() => {
            let mut cfg = match state.config.as_ref().and_then(|r| r.as_ref().ok()).cloned() {
                Some(c) => c,
                None => {
                    state.flash("delete failed: config not loaded");
                    return Modal::None;
                }
            };
            if m.provider_index >= cfg.providers.len() {
                state.flash("delete failed: provider index out of range");
                return Modal::None;
            }
            cfg.providers.remove(m.provider_index);
            match client.put_config(&cfg) {
                Ok(updated) => {
                    state.set_config(Ok(updated));
                    state.flash(format!("deleted {}", m.provider_name));
                    Modal::None
                }
                Err(e) => {
                    state.flash(format!("delete failed: {e}"));
                    Modal::None
                }
            }
        }
        // Blocked-rules state: only Esc closes.
        _ => Modal::DeleteConfirm(m),
    }
}
```

- [ ] **Step 4: Verify build + smoke**

Run: `cargo test -p proxy-tui`
Expected: PASS.

Run: `cargo clippy --workspace -- -D warnings`
Expected: clean.

Manual smoke:

1. Add a provider `delete-me` (passthrough). Select it, press `d`. Confirm modal shows. Press `y`. Flash `deleted delete-me`. Provider gone from list and `config.toml`.
2. Add provider `referenced` and a routing rule pointing to it (edit `config.toml` or use admin API). Restart TUI / press `r` to refresh. Select `referenced`, press `d`. Modal shows blocked-rules listing. `y` is ignored. `Esc` closes.

- [ ] **Step 5: Commit**

```bash
git add crates/proxy-tui/src/main.rs
git commit -m "feat(proxy-tui): delete provider with routing-rule safety check

'd' opens DeleteConfirmModal. If routing rules reference the provider,
the modal lists them and refuses the delete. Otherwise 'y' confirms."
```

---

### Task 10: Help footer + final smoke

**Files:**
- Modify: `crates/proxy-tui/src/ui.rs` (Providers-view help footer text).

- [ ] **Step 1: Find the Providers-view help footer**

In `crates/proxy-tui/src/ui.rs`, locate the `draw_providers` function (around line 249). Find the line that renders the bottom hints — typically a `Paragraph` or `Line::from(...)` with text like `[t] test  [e] edit  [j/k] move  ...`. Update it to:

```rust
    let hints = "[a] add  [e] edit  [d] delete  [t] test  [j/k] move  [r] refresh  [q] quit";
```

(Or whichever variable holds the hint text.)

- [ ] **Step 2: Verify the new text renders**

Run: `cargo run -p proxy-tui` and visually confirm the bottom hint on the Providers tab.

- [ ] **Step 3: Final smoke checklist**

Run through the full feature end-to-end against a real proxy daemon:

1. Add a passthrough provider → success.
2. Add an api_key provider → success.
3. Add an OAuth provider → success (requires real Anthropic login).
4. Edit the api_key provider's value → success.
5. Edit the api_key provider's name → success (when no routing rules reference it).
6. Edit and try to rename to a name that's referenced by a routing rule → blocked with red error.
7. Delete a non-referenced provider → success.
8. Delete a referenced provider → blocked with rules list.
9. Validation: empty name, invalid chars, duplicate name → all show inline red error.
10. Esc at every state cleanly returns to the Providers view.

Run all gates:

```bash
cargo fmt
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

Expected: all pass.

- [ ] **Step 4: Commit**

```bash
git add crates/proxy-tui/src/ui.rs
git commit -m "feat(proxy-tui): help-footer reflects new add/edit/delete keys

Closes the TUI provider CRUD feature."
```

---

## Self-review

- **Spec coverage:**
  - Keybinds (`a`/`e`/`d`/`t`) — Tasks 4, 5, 9.
  - Form modal with all fields — Tasks 2, 3, 4.
  - Field navigation (Tab/Shift+Tab/←/→) — Task 4.
  - Validation (FormError variants) — Task 1.
  - Non-OAuth save — Task 6.
  - Edit + OAuth (PUT non-auth changes first) — Task 7.
  - Add + OAuth (two-step) — Task 8.
  - Delete with routing-rule safety check — Task 9.
  - Help footer update — Task 10.
  - Auth value masking + visibility — Task 3 render code.
  - Aborted Add+OAuth leaves provider with Passthrough — covered by Task 8 implementation choice (no auto-rollback).

- **Placeholder scan:** No `TBD`/`TODO`. No "similar to Task N" — code repeated where needed. No "appropriate error handling" — concrete errors used.

- **Type consistency:**
  - `ProviderFormModal::new_for_add` and `from_provider` are referenced consistently.
  - `FormField::next` / `FormField::prev` consistent across Tasks 2/4.
  - `submit_non_oauth_save` / `submit_oauth_edit` / `submit_oauth_add` referenced consistently in Tasks 6/7/8.
  - `cycle_field_value` / `edit_focused_text` defined Task 4, used Task 4. Not redefined later.
  - `FormState::Failed` retry path: Task 6 retries by setting state back to `Editing` and recursing. OAuth retry: user presses Esc and re-submits.

- **Field of `AuthInputKind::cycle()`:** existing method on the enum (already defined in `app.rs`); we reuse it. Reverse cycling uses `cycle().cycle().cycle()` which is correct for a 4-variant cycle.

- **Compile-clean checkpoints:** every task ends with `cargo clippy --workspace -- -D warnings`. Tasks 4 through 8 keep the build green at every commit (a temporary `submit_oauth_add` stub bridges Tasks 7 → 8).
