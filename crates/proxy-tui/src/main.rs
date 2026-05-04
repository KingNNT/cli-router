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
    AppState, AuthInputKind, EditAuthModal, EditState, Modal, RangePreset, TestProviderModal,
    TestState, View,
};
use crate::client::AdminClient;
use chrono::{Datelike, Local, TimeZone};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use proxy_admin_api::ConfigPayload;
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
    if let Modal::TestProvider(_) | Modal::EditAuth(_) = &state.modal {
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
        Modal::EditAuth(m) => handle_edit_key(k, client, state, m),
        Modal::None => Modal::None,
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
    let kind = AuthInputKind::from_payload(&prov.auth);
    let value_input = match &prov.auth {
        proxy_admin_api::AuthPayload::Passthrough => String::new(),
        proxy_admin_api::AuthPayload::ApiKey { value }
        | proxy_admin_api::AuthPayload::Bearer { value } => value.clone(),
        proxy_admin_api::AuthPayload::AnthropicOAuth { .. } => String::new(),
    };
    state.modal = Modal::EditAuth(EditAuthModal {
        provider_index: state.providers_selected,
        provider_name: prov.name.clone(),
        kind,
        value_input,
        state: EditState::Editing,
    });
}

fn handle_edit_key(
    k: KeyEvent,
    client: &AdminClient,
    state: &mut AppState,
    mut m: EditAuthModal,
) -> Modal {
    match (&m.state, k.code) {
        (_, KeyCode::Esc) => Modal::None,
        (EditState::Editing, KeyCode::Tab) => {
            m.kind = m.kind.cycle();
            Modal::EditAuth(m)
        }
        (EditState::Editing, KeyCode::Enter) => {
            if m.kind == crate::app::AuthInputKind::OAuthAnthropic {
                match client.oauth_start(&m.provider_name) {
                    Ok(resp) => {
                        m.state = EditState::OAuthAwaitingCode {
                            authorization_url: resp.authorization_url,
                            state_id: resp.state_id,
                            code_input: String::new(),
                        };
                    }
                    Err(e) => m.state = EditState::Failed(e.to_string()),
                }
                return Modal::EditAuth(m);
            }
            m.state = EditState::Saving;
            match save_provider_auth(client, state, &m) {
                Ok(()) => {
                    m.state = EditState::Done;
                    state.flash("config saved — applied immediately");
                    Modal::EditAuth(m)
                }
                Err(e) => {
                    m.state = EditState::Failed(e);
                    Modal::EditAuth(m)
                }
            }
        }
        (EditState::Editing, KeyCode::Backspace) => {
            m.value_input.pop();
            Modal::EditAuth(m)
        }
        (EditState::Editing, KeyCode::Char(c)) => {
            m.value_input.push(c);
            Modal::EditAuth(m)
        }
        (EditState::OAuthAwaitingCode { .. }, KeyCode::Enter) => {
            let (state_id, code) = match &m.state {
                EditState::OAuthAwaitingCode {
                    state_id,
                    code_input,
                    ..
                } => (state_id.clone(), code_input.clone()),
                _ => unreachable!(),
            };
            if code.trim().is_empty() {
                return Modal::EditAuth(m);
            }
            m.state = EditState::OAuthExchanging;
            match client.oauth_complete(&state_id, code.trim(), &m.provider_name) {
                Ok(resp) if resp.success => {
                    if let Some(cfg) = resp.config {
                        state.set_config(Ok(cfg));
                    }
                    m.state = EditState::Done;
                    state.flash("OAuth complete — applied immediately");
                }
                Ok(resp) => {
                    m.state = EditState::Failed(
                        resp.error.unwrap_or_else(|| "unknown OAuth failure".into()),
                    );
                }
                Err(e) => m.state = EditState::Failed(e.to_string()),
            }
            Modal::EditAuth(m)
        }
        (EditState::OAuthAwaitingCode { .. }, KeyCode::Backspace) => {
            if let EditState::OAuthAwaitingCode { code_input, .. } = &mut m.state {
                code_input.pop();
            }
            Modal::EditAuth(m)
        }
        (EditState::OAuthAwaitingCode { .. }, KeyCode::Char(c)) => {
            if let EditState::OAuthAwaitingCode { code_input, .. } = &mut m.state {
                code_input.push(c);
            }
            Modal::EditAuth(m)
        }
        _ => Modal::EditAuth(m),
    }
}

fn save_provider_auth(
    client: &AdminClient,
    state: &mut AppState,
    m: &EditAuthModal,
) -> Result<(), String> {
    let mut cfg: ConfigPayload = state
        .config
        .as_ref()
        .and_then(|r| r.as_ref().ok())
        .cloned()
        .ok_or_else(|| "config not loaded".to_string())?;
    let prov = cfg
        .providers
        .get_mut(m.provider_index)
        .ok_or_else(|| "provider index out of range".to_string())?;
    prov.auth = m.kind.into_payload(m.value_input.clone());
    let updated = client.put_config(&cfg).map_err(|e| e.to_string())?;
    state.set_config(Ok(updated));
    Ok(())
}
