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
    AppState, AuthInputKind, FormField, FormState, Modal, ProviderFormModal, RangePreset,
    TestProviderModal, TestState, View,
};
use crate::client::AdminClient;
use chrono::{Datelike, Local, TimeZone};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
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
        if let Event::Key(k) = event::read()? {
            handle_key(k, &client, &mut state);
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

    // Always-available keys (quit + view switching). View switches happen
    // first so the user can leave Usage with `5`→other-tab number keys.
    match k.code {
        KeyCode::Char('q') | KeyCode::Esc => {
            state.should_quit = true;
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
        Modal::DeleteConfirm(m) => Modal::DeleteConfirm(m), // T9 will replace
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
