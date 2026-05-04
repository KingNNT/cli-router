//! Ratatui client for the proxy admin API. Connects to a localhost daemon
//! and lets the user watch status, browse recent requests, edit provider
//! credentials, and ping a provider — all without touching the config file
//! by hand.

mod app;
mod client;
mod terminal;
mod ui;
mod validate;
mod views;

use crate::app::{
    ALL_VIEWS, AppState, AuthInputKind, DeleteConfirmModal, FormField, FormMode, FormState, Modal,
    PROVIDER_TOOLBAR, PROVIDER_TOOLBAR_GAP, ProviderAction, ProviderFormModal, RangePreset,
    TestProviderModal, TestState, View,
};
use crate::client::AdminClient;
use chrono::{Datelike, Local, TimeZone};
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use std::rc::Rc;
use std::time::Duration;

/// Restore terminal even on panic.
struct TermGuard(Option<terminal::Term>);
impl Drop for TermGuard {
    fn drop(&mut self) {
        if let Some(t) = self.0.take() {
            let _ = terminal::restore(t);
        }
    }
}

fn main() -> std::io::Result<()> {
    let base =
        std::env::var("CLI_ROUTER_PROXY_URL").unwrap_or_else(|_| "http://127.0.0.1:8787".into());
    let client = AdminClient::new(base);
    let mut state = AppState::new();

    refresh_all(&client, &mut state);

    let term = terminal::setup()?;
    let mut guard = TermGuard(Some(term));
    let term = guard.0.as_mut().expect("term present");

    loop {
        term.draw(|f| ui::draw(f, &state))?;
        if !event::poll(Duration::from_millis(250))? {
            continue;
        }
        match event::read()? {
            Event::Key(k) => handle_key(k, &client, &mut state),
            Event::Mouse(m) => {
                let term_area = term
                    .size()
                    .map(|s| Rect::new(0, 0, s.width, s.height))
                    .unwrap_or_default();
                handle_mouse(m, term_area, &client, &mut state);
            }
            _ => {}
        }
        if state.should_quit {
            break;
        }
    }
    Ok(())
}

fn refresh_all(client: &AdminClient, state: &mut AppState) {
    state.set_status(client.get_status().map_err(|e| e.to_string()));
    state.set_config(client.get_config().map_err(|e| e.to_string()));
    state.set_recent(client.get_recent(50).map_err(|e| e.to_string()));
    state.quota = Some(client.get_quota_status().map_err(|e| e.to_string()));
}

fn refresh_view(client: &AdminClient, state: &mut AppState) {
    match state.view {
        View::Status => {
            state.set_status(client.get_status().map_err(|e| e.to_string()));
            state.quota = Some(client.get_quota_status().map_err(|e| e.to_string()));
        }
        View::Providers | View::Routing => {
            state.set_config(client.get_config().map_err(|e| e.to_string()))
        }
        View::Requests => state.set_recent(client.get_recent(50).map_err(|e| e.to_string())),
        // Usage refreshes on demand from `handle_key`'s Usage-tab branch
        // (`fetch_usage`); no auto-refresh tick should hit this arm.
        View::Usage => {}
    }
}

fn handle_key(k: KeyEvent, client: &AdminClient, state: &mut AppState) {
    if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
        state.should_quit = true;
        return;
    }
    if !matches!(&state.modal, Modal::None) {
        handle_modal_key(k, client, state);
        return;
    }
    state.flash = None;

    // Always-available keys (quit + view switching + help). View switches
    // happen first so the user can leave Usage with `5`→other-tab number
    // keys.
    match k.code {
        KeyCode::Char('q') | KeyCode::Esc => {
            state.should_quit = true;
            return;
        }
        KeyCode::Char('?') => {
            state.modal = Modal::Help;
            return;
        }
        KeyCode::Char('5') => {
            state.set_view(View::Usage);
            if state.usage.summary.is_none() && state.usage.last_error.is_none() {
                fetch_usage(client, state);
            }
            return;
        }
        _ => {}
    }

    // Usage-tab-specific keys take precedence over the global handler so
    // that `1`/`2`/`3`/`4` switch range presets instead of leaving the tab.
    if state.view == View::Usage {
        match k.code {
            KeyCode::Char('1') => {
                state.usage.range_preset = Some(RangePreset::Today);
                fetch_usage(client, state);
            }
            KeyCode::Char('2') => {
                state.usage.range_preset = Some(RangePreset::D7);
                fetch_usage(client, state);
            }
            KeyCode::Char('3') => {
                state.usage.range_preset = Some(RangePreset::D30);
                fetch_usage(client, state);
            }
            KeyCode::Char('4') => {
                state.usage.range_preset = Some(RangePreset::All);
                fetch_usage(client, state);
            }
            KeyCode::Char('r') => fetch_usage(client, state),
            KeyCode::Up => {
                state.usage.table_offset = state.usage.table_offset.saturating_sub(1);
            }
            KeyCode::Down => {
                if let Some(s) = &state.usage.summary
                    && state.usage.table_offset + 1 < s.models.len()
                {
                    state.usage.table_offset += 1;
                }
            }
            _ => {}
        }
        return;
    }

    match k.code {
        KeyCode::Char('1') => state.set_view(View::Status),
        KeyCode::Char('2') => state.set_view(View::Providers),
        KeyCode::Char('3') => state.set_view(View::Routing),
        KeyCode::Char('4') => state.set_view(View::Requests),
        KeyCode::Char('r') => {
            refresh_view(client, state);
            state.flash("refreshed");
        }
        KeyCode::Down | KeyCode::Char('j') => state.move_selection_down(),
        KeyCode::Up | KeyCode::Char('k') => state.move_selection_up(),
        KeyCode::Char('a') if state.view == View::Providers => open_add_modal(state),
        KeyCode::Char('t') if state.view == View::Providers => open_test_modal(state),
        KeyCode::Char('e') if state.view == View::Providers => open_edit_modal(state),
        KeyCode::Char('d') if state.view == View::Providers => open_delete_modal(state),
        _ => {}
    }
}

// ---- Mouse support ----

/// Vertical layout that mirrors `ui::draw`. Centralised so the input handler
/// can hit-test against the same rects the renderer produces.
fn main_layout(area: Rect) -> Rc<[Rect]> {
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(2),
        ])
        .split(area)
}

fn usage_layout(body_area: Rect) -> Rc<[Rect]> {
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(7),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(body_area)
}

fn pos_in_rect(x: u16, y: u16, r: Rect) -> bool {
    x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
}

/// Hit-test the tab bar. Tabs widget renders inside a bordered block, so
/// titles live on row `tabs_area.y + 1` starting one cell in from the left
/// border. Each title is `" {N} {Label} "` joined by the default `│` divider.
fn tab_hit(x: u16, y: u16, tabs_area: Rect) -> Option<View> {
    if y != tabs_area.y + 1 {
        return None;
    }
    let mut col = tabs_area.x + 1;
    for (i, v) in ALL_VIEWS.iter().enumerate() {
        if i > 0 {
            col += 1; // single-cell divider
        }
        let label = format!(" {} {} ", i + 1, v.label());
        let w = label.chars().count() as u16;
        if x >= col && x < col + w {
            return Some(*v);
        }
        col += w;
    }
    None
}

/// Hit-test a bordered table inside `body_area` (header + N data rows).
/// `top_pad` is the number of rows the renderer reserves above the header
/// (e.g. a toolbar + gap on the Providers view). Returns the data-row index,
/// or None if the click misses the data area.
fn table_row_hit(x: u16, y: u16, body_area: Rect, row_count: usize, top_pad: u16) -> Option<usize> {
    let inner_x = body_area.x + 1;
    let inner_y = body_area.y + 1;
    let inner_w = body_area.width.saturating_sub(2);
    let inner_h = body_area.height.saturating_sub(2);
    if x < inner_x || x >= inner_x + inner_w {
        return None;
    }
    let header_y = inner_y + top_pad;
    if y <= header_y || y >= inner_y + inner_h {
        return None;
    }
    let row = (y - header_y - 1) as usize;
    if row >= row_count {
        return None;
    }
    Some(row)
}

/// Hit-test the Providers toolbar. The toolbar lives on row `body_area.y + 1`
/// (the first row inside the bordered block), starting one cell in from the
/// left border, with `PROVIDER_TOOLBAR_GAP` between buttons.
fn provider_toolbar_hit(x: u16, y: u16, body_area: Rect) -> Option<ProviderAction> {
    let inner_x = body_area.x + 1;
    let inner_y = body_area.y + 1;
    if y != inner_y {
        return None;
    }
    let gap = PROVIDER_TOOLBAR_GAP.chars().count() as u16;
    let mut col = inner_x;
    for (i, action) in PROVIDER_TOOLBAR.iter().enumerate() {
        if i > 0 {
            col += gap;
        }
        let w = action.label().chars().count() as u16;
        if x >= col && x < col + w {
            return Some(*action);
        }
        col += w;
    }
    None
}

/// Spawn the OS browser/launcher for the given URL. We can't render a
/// clickable hyperlink inside ratatui reliably (OSC 8 support is patchy
/// across terminals), so we shell out to the platform launcher instead.
/// Detached spawn — the child outlives this process if needed.
fn open_url(url: &str) -> std::io::Result<()> {
    let mut cmd = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else if cfg!(target_os = "windows") {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    } else {
        std::process::Command::new("xdg-open")
    };
    cmd.arg(url).spawn()?;
    Ok(())
}

fn dispatch_provider_action(action: ProviderAction, client: &AdminClient, state: &mut AppState) {
    state.flash = None;
    match action {
        ProviderAction::Add => open_add_modal(state),
        ProviderAction::Edit => open_edit_modal(state),
        ProviderAction::Delete => open_delete_modal(state),
        ProviderAction::Test => open_test_modal(state),
        ProviderAction::Refresh => {
            refresh_view(client, state);
            state.flash("refreshed");
        }
    }
}

/// Hit-test the Usage tab's range selector row. Labels are joined by two
/// spaces; the active one is wrapped in brackets — same width as the
/// inactive form, so offsets stay stable.
fn usage_range_hit(x: u16, y: u16, range_area: Rect) -> Option<RangePreset> {
    if y != range_area.y {
        return None;
    }
    let presets = [
        RangePreset::Today,
        RangePreset::D7,
        RangePreset::D30,
        RangePreset::All,
    ];
    let mut col = range_area.x;
    for (i, p) in presets.iter().enumerate() {
        if i > 0 {
            col += 2; // "  " joiner
        }
        let label = format!(" {} ", p.label());
        let w = label.chars().count() as u16;
        if x >= col && x < col + w {
            return Some(*p);
        }
        col += w;
    }
    None
}

fn providers_count(state: &AppState) -> usize {
    state
        .config
        .as_ref()
        .and_then(|r| r.as_ref().ok())
        .map(|c| c.providers.len())
        .unwrap_or(0)
}

fn requests_count(state: &AppState) -> usize {
    state
        .recent
        .as_ref()
        .and_then(|r| r.as_ref().ok())
        .map(|r| r.items.len())
        .unwrap_or(0)
}

fn handle_mouse(m: MouseEvent, term_area: Rect, client: &AdminClient, state: &mut AppState) {
    // Modals are otherwise keyboard-driven; swallow mouse so a stray scroll
    // doesn't mutate the underlying view while the user is editing a form.
    // Exception: while a ProviderForm is awaiting the OAuth code, a left-click
    // anywhere in the modal re-launches the authorization URL in the
    // browser — useful if the auto-launch failed or the window was closed.
    if !matches!(state.modal, Modal::None) {
        if let MouseEventKind::Down(MouseButton::Left) = m.kind {
            let url = match &state.modal {
                Modal::ProviderForm(form) => match &form.state {
                    FormState::OAuthAwaitingCode {
                        authorization_url, ..
                    } => Some(authorization_url.clone()),
                    _ => None,
                },
                _ => None,
            };
            if let Some(url) = url {
                match open_url(&url) {
                    Ok(()) => state.flash("re-opening authorization URL…"),
                    Err(e) => state.flash(format!("could not open browser: {e}")),
                }
            }
        }
        return;
    }

    let chunks = main_layout(term_area);
    let tabs_area = chunks[0];
    let body_area = chunks[1];

    match m.kind {
        MouseEventKind::ScrollUp => match state.view {
            View::Usage => {
                state.usage.table_offset = state.usage.table_offset.saturating_sub(1);
            }
            _ => state.move_selection_up(),
        },
        MouseEventKind::ScrollDown => match state.view {
            View::Usage => {
                if let Some(s) = &state.usage.summary
                    && state.usage.table_offset + 1 < s.models.len()
                {
                    state.usage.table_offset += 1;
                }
            }
            _ => state.move_selection_down(),
        },
        MouseEventKind::Down(MouseButton::Left) => {
            state.flash = None;
            if let Some(v) = tab_hit(m.column, m.row, tabs_area) {
                state.set_view(v);
                if v == View::Usage
                    && state.usage.summary.is_none()
                    && state.usage.last_error.is_none()
                {
                    fetch_usage(client, state);
                }
                return;
            }
            if !pos_in_rect(m.column, m.row, body_area) {
                return;
            }
            match state.view {
                View::Providers => {
                    if let Some(action) = provider_toolbar_hit(m.column, m.row, body_area) {
                        dispatch_provider_action(action, client, state);
                        return;
                    }
                    if let Some(idx) =
                        table_row_hit(m.column, m.row, body_area, providers_count(state), 2)
                    {
                        state.providers_selected = idx;
                    }
                }
                View::Requests => {
                    if let Some(idx) =
                        table_row_hit(m.column, m.row, body_area, requests_count(state), 0)
                    {
                        state.requests_selected = idx;
                    }
                }
                View::Usage => {
                    let usage_chunks = usage_layout(body_area);
                    if let Some(p) = usage_range_hit(m.column, m.row, usage_chunks[0]) {
                        state.usage.range_preset = Some(p);
                        fetch_usage(client, state);
                    }
                }
                View::Status | View::Routing => {}
            }
        }
        _ => {}
    }
}

fn fetch_usage(client: &AdminClient, state: &mut AppState) {
    let preset = state.usage.range_preset.unwrap_or(RangePreset::Today);
    state.usage.range_preset = Some(preset);
    state.usage.loading = true;

    let now = Local::now();
    let now_ms = now.timestamp_millis();
    let local_midnight_ms = Local
        .with_ymd_and_hms(now.year(), now.month(), now.day(), 0, 0, 0)
        .single()
        .map(|d| d.timestamp_millis())
        .unwrap_or(now_ms);

    let (from_ms, to_ms) = preset.to_range(now_ms, local_midnight_ms);

    match client.get_usage_summary(from_ms, to_ms) {
        Ok(resp) => {
            state.usage.summary = Some(resp);
            state.usage.last_error = None;
            state.usage.table_offset = 0;
        }
        Err(e) => {
            state.usage.last_error = Some(format!("{e}"));
        }
    }
    state.usage.loading = false;
}

// ---- Modal handling ----

fn handle_modal_key(k: KeyEvent, client: &AdminClient, state: &mut AppState) {
    let modal = std::mem::replace(&mut state.modal, Modal::None);
    let next = match modal {
        Modal::TestProvider(m) => handle_test_key(k, client, m),
        Modal::None => Modal::None,
        Modal::ProviderForm(m) => handle_form_key(k, client, state, m),
        Modal::DeleteConfirm(m) => handle_delete_key(k, client, state, m),
        Modal::Help => match k.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => Modal::None,
            _ => Modal::Help,
        },
    };
    state.modal = next;
}

fn open_test_modal(state: &mut AppState) {
    let Some(prov) = state.selected_provider() else {
        return;
    };
    let suggested = match prov.kind.as_str() {
        "anthropic" => "claude-3-5-haiku-latest",
        "zai" => "glm-4.5-air",
        _ => "",
    };
    state.modal = Modal::TestProvider(TestProviderModal {
        provider_name: prov.name.clone(),
        model_input: suggested.into(),
        state: TestState::Editing,
    });
}

fn handle_test_key(k: KeyEvent, client: &AdminClient, mut m: TestProviderModal) -> Modal {
    match (&m.state, k.code) {
        (_, KeyCode::Esc) => Modal::None,
        (TestState::Editing, KeyCode::Enter) => {
            m.state = TestState::InFlight;
            // ureq is blocking — fire it inline. UI redraws after this returns.
            let result = client.test_provider(&m.provider_name, &m.model_input);
            m.state = match result {
                Ok(r) => TestState::Done(r),
                Err(e) => TestState::Failed(e.to_string()),
            };
            Modal::TestProvider(m)
        }
        (TestState::Editing, KeyCode::Backspace) => {
            m.model_input.pop();
            Modal::TestProvider(m)
        }
        (TestState::Editing, KeyCode::Char(c)) => {
            m.model_input.push(c);
            Modal::TestProvider(m)
        }
        _ => Modal::TestProvider(m),
    }
}

fn open_edit_modal(state: &mut AppState) {
    let Some(prov) = state.selected_provider() else {
        return;
    };
    let modal = ProviderFormModal::from_provider(state.providers_selected, prov);
    state.modal = Modal::ProviderForm(modal);
}

fn open_add_modal(state: &mut AppState) {
    state.modal = Modal::ProviderForm(ProviderFormModal::new_for_add());
}

fn open_delete_modal(state: &mut AppState) {
    let cfg = match state.config.as_ref().and_then(|r| r.as_ref().ok()).cloned() {
        Some(c) => c,
        None => return,
    };
    let provider_index = state.providers_selected;
    let prov = match cfg.providers.get(provider_index) {
        Some(p) => p,
        None => return,
    };
    let blocking_rules = crate::validate::rules_referencing(&prov.name, &cfg);
    let provider_name = prov.name.clone();
    state.modal = Modal::DeleteConfirm(DeleteConfirmModal {
        provider_index,
        provider_name,
        blocking_rules,
    });
}

fn handle_delete_key(
    k: crossterm::event::KeyEvent,
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
            if m.auth_kind == AuthInputKind::OAuthAnthropic {
                return match m.mode {
                    FormMode::Add => submit_oauth_add(client, state, m), // Task 8
                    FormMode::Edit { .. } => submit_oauth_edit(client, state, m),
                };
            }
            submit_non_oauth_save(client, state, m)
        }
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
                m.error = Some("paste the authorization code first".into());
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
            if *original_index >= cfg.providers.len() {
                m.error = Some("provider list changed; press Esc and reopen".into());
                return Modal::ProviderForm(m);
            }
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

fn submit_oauth_add(client: &AdminClient, state: &mut AppState, mut m: ProviderFormModal) -> Modal {
    use crate::validate::{FormInputs, validate_provider_form};
    use proxy_admin_api::AuthPayload;

    let mut cfg = match state.config.as_ref().and_then(|r| r.as_ref().ok()).cloned() {
        Some(c) => c,
        None => {
            m.error = Some("config not loaded".into());
            return Modal::ProviderForm(m);
        }
    };

    // Retry path: a previous Add+OAuth attempt already PUT this provider but
    // oauth_start failed. The provider is already in the daemon — skip the
    // re-add (which would trip duplicate-name validation) and re-run the
    // OAuth dance.
    let already_added =
        !m.name.trim().is_empty() && cfg.providers.iter().any(|p| p.name == m.name.trim());

    if !already_added {
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

        cfg.providers.push(provider);

        m.state = FormState::Saving;
        match client.put_config(&cfg) {
            Ok(updated) => state.set_config(Ok(updated)),
            Err(e) => {
                m.state = FormState::Failed(format!("PUT failed (provider not added): {e}"));
                return Modal::ProviderForm(m);
            }
        }
    }

    let new_provider_name = m.name.trim().to_string();

    // Provider exists in daemon config. Run OAuth dance.
    m.state = FormState::Saving;
    match client.oauth_start(&new_provider_name) {
        Ok(resp) => {
            let auth_url = resp.authorization_url.clone();
            m.state = FormState::OAuthAwaitingCode {
                authorization_url: resp.authorization_url,
                state_id: resp.state_id,
                code_input: String::new(),
            };
            match open_url(&auth_url) {
                Ok(()) => state.flash("opening authorization URL in your browser…"),
                Err(e) => state.flash(format!(
                    "could not auto-open browser ({e}); copy the URL above"
                )),
            }
        }
        Err(e) => {
            m.state = FormState::Failed(format!(
                "oauth_start failed: {e}. Press Enter to retry, or Esc to abort."
            ));
        }
    }
    Modal::ProviderForm(m)
}

fn submit_oauth_edit(
    client: &AdminClient,
    state: &mut AppState,
    mut m: ProviderFormModal,
) -> Modal {
    use crate::validate::{FormInputs, validate_provider_form};
    use proxy_admin_api::AuthPayload;

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
    let dirty = if original_index < cfg.providers.len() {
        let prev = &cfg.providers[original_index];
        prev.name != provider.name
            || prev.kind != provider.kind
            || prev.base_url != provider.base_url
            || prev.openai_base_url != provider.openai_base_url
    } else {
        m.error = Some("provider list changed; press Esc and reopen".into());
        return Modal::ProviderForm(m);
    };

    let new_provider_name = provider.name.clone();
    cfg.providers[original_index] = provider;

    if dirty {
        m.state = FormState::Saving;
        match client.put_config(&cfg) {
            // PUT succeeded — non-auth fields are now live in the daemon.
            // If oauth_start fails below, the modal goes to Failed but
            // those field changes remain persisted (intentional).
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
            let auth_url = resp.authorization_url.clone();
            m.state = FormState::OAuthAwaitingCode {
                authorization_url: resp.authorization_url,
                state_id: resp.state_id,
                code_input: String::new(),
            };
            match open_url(&auth_url) {
                Ok(()) => state.flash("opening authorization URL in your browser…"),
                Err(e) => state.flash(format!(
                    "could not auto-open browser ({e}); copy the URL above"
                )),
            }
        }
        Err(e) => {
            m.state = FormState::Failed(e.to_string());
        }
    }
    Modal::ProviderForm(m)
}

#[cfg(test)]
mod mouse_hittest_tests {
    use super::*;

    fn term() -> Rect {
        Rect::new(0, 0, 80, 24)
    }

    #[test]
    fn tab_hit_picks_each_view() {
        let chunks = main_layout(term());
        let tabs_area = chunks[0];
        // First label " 1 Status " starts at x=1 (past left border) on row 1.
        // Confirm we land on Status near its midpoint and Providers further right.
        let status_x = tabs_area.x + 5; // mid of " 1 Status "
        assert_eq!(
            tab_hit(status_x, tabs_area.y + 1, tabs_area),
            Some(View::Status)
        );

        // Walk to find Providers location.
        let label = " 1 Status ";
        let providers_x = tabs_area.x + 1 + label.chars().count() as u16 + 1 + 5;
        assert_eq!(
            tab_hit(providers_x, tabs_area.y + 1, tabs_area),
            Some(View::Providers)
        );
    }

    #[test]
    fn tab_hit_misses_off_row() {
        let chunks = main_layout(term());
        let tabs_area = chunks[0];
        assert_eq!(tab_hit(tabs_area.x + 5, tabs_area.y, tabs_area), None);
        assert_eq!(tab_hit(tabs_area.x + 5, tabs_area.y + 2, tabs_area), None);
    }

    #[test]
    fn table_row_hit_data_rows() {
        let body = Rect::new(0, 3, 80, 20);
        // inner_y = 4, header at y=4, row 0 at y=5, row 1 at y=6 (top_pad=0)
        assert_eq!(table_row_hit(10, 5, body, 3, 0), Some(0));
        assert_eq!(table_row_hit(10, 6, body, 3, 0), Some(1));
        assert_eq!(table_row_hit(10, 7, body, 3, 0), Some(2));
    }

    #[test]
    fn table_row_hit_out_of_bounds() {
        let body = Rect::new(0, 3, 80, 20);
        // header row → no selection
        assert_eq!(table_row_hit(10, 4, body, 3, 0), None);
        // row index past row_count
        assert_eq!(table_row_hit(10, 8, body, 3, 0), None);
        // outside body x range
        assert_eq!(table_row_hit(100, 5, body, 3, 0), None);
    }

    #[test]
    fn table_row_hit_with_top_pad() {
        // Providers view: toolbar at inner_y, gap at inner_y+1, header at
        // inner_y+2, row 0 at inner_y+3.
        let body = Rect::new(0, 3, 80, 20);
        assert_eq!(table_row_hit(10, 4, body, 3, 2), None); // toolbar row
        assert_eq!(table_row_hit(10, 5, body, 3, 2), None); // gap row
        assert_eq!(table_row_hit(10, 6, body, 3, 2), None); // header row
        assert_eq!(table_row_hit(10, 7, body, 3, 2), Some(0));
        assert_eq!(table_row_hit(10, 8, body, 3, 2), Some(1));
    }

    #[test]
    fn provider_toolbar_hit_picks_each_button() {
        // Body area starts at (0, 3), so toolbar lives on y=4 starting at x=1.
        let body = Rect::new(0, 3, 80, 20);
        // "[ Add ]" is 7 chars, gap is 2, "[ Edit ]" is 8 chars, …
        assert_eq!(provider_toolbar_hit(2, 4, body), Some(ProviderAction::Add));
        // Click inside the second button: x = 1 + 7 + 2 + 1 = 11.
        assert_eq!(
            provider_toolbar_hit(11, 4, body),
            Some(ProviderAction::Edit)
        );
        // Clicking off the toolbar row misses.
        assert_eq!(provider_toolbar_hit(2, 5, body), None);
        // Clicking in the gap between Add and Edit misses.
        let gap_x = 1 + 7; // first cell after "[ Add ]"
        assert_eq!(provider_toolbar_hit(gap_x, 4, body), None);
    }

    #[test]
    fn usage_range_hit_picks_each_preset() {
        let area = Rect::new(0, 0, 40, 1);
        // " Today " spans x=0..7.
        assert_eq!(usage_range_hit(3, 0, area), Some(RangePreset::Today));
        // joiner "  " then " 7d " starts at x=9, spans 9..13.
        assert_eq!(usage_range_hit(10, 0, area), Some(RangePreset::D7));
        // Out-of-row click misses.
        assert_eq!(usage_range_hit(3, 1, area), None);
    }

    #[test]
    fn pos_in_rect_basic() {
        let r = Rect::new(2, 2, 10, 10);
        assert!(pos_in_rect(2, 2, r));
        assert!(pos_in_rect(11, 11, r));
        assert!(!pos_in_rect(12, 5, r));
        assert!(!pos_in_rect(5, 12, r));
        assert!(!pos_in_rect(1, 5, r));
    }
}
