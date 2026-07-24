//! Ratatui client for the proxy admin API. Connects to a localhost daemon
//! and lets the user watch status, browse recent requests, edit provider
//! credentials, and ping a provider through the proxy admin API.

mod app;
mod client;
mod terminal;
mod ui;
mod validate;
mod views;

use crate::app::{
    ALL_VIEWS, AppMode, AppState, AuthInputKind, ConfigSection, DeleteConfirmModal,
    DisableConfirmModal, FormField, FormMode, FormState, Modal, PROVIDER_TOOLBAR,
    PROVIDER_TOOLBAR_GAP, ProviderAction, ProviderFormModal, ProviderKind, QuotaField,
    QuotaFormModal, RangePreset, RoutingField, RoutingFormModal, TestProviderModal, TestState,
    View,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartupFlow {
    Connected,
    ProxyRequired,
}

fn decide_startup_flow(proxy_available: bool) -> StartupFlow {
    match proxy_available {
        true => StartupFlow::Connected,
        false => StartupFlow::ProxyRequired,
    }
}

fn proxy_required_message() -> String {
    "Proxy is not running. Config is stored in the SQLite database managed by the proxy daemon. Start `cli-router proxy` first, then reopen the TUI to configure providers.".into()
}

fn main() -> std::io::Result<()> {
    let base =
        std::env::var("CLI_ROUTER_PROXY_URL").unwrap_or_else(|_| "http://127.0.0.1:8787".into());
    let client = AdminClient::new(base);
    let mut state = AppState::new();

    let status_result = client.get_status();
    let flow = decide_startup_flow(status_result.is_ok());

    match flow {
        StartupFlow::Connected => {
            // Connected to running proxy. Config edits go through the admin API
            // and persist to SQLite via the proxy daemon.
            state.mode = AppMode::Connected;
            state.set_status(status_result.map_err(|e| e.to_string()));
            refresh_all(&client, &mut state);
        }
        StartupFlow::ProxyRequired => {
            // DB-backed config is managed by the proxy daemon, so the TUI must
            // not create or edit config files when the proxy is unavailable.
            state.mode = AppMode::ProxyRequired;
            state.set_view(View::Config);
            state.set_config(Err(proxy_required_message()));
        }
    }

    let term = terminal::setup()?;
    let mut guard = TermGuard(Some(term));
    let term = guard.0.as_mut().expect("term present");

    loop {
        drain_account_rx(&mut state);
        term.draw(|f| ui::draw(f, &state))?;
        // Poll faster while a background fetch is pending so the UI flips
        // from "Loading…" to data quickly once the result arrives.
        let poll_ms = if state.account_rx.is_some() { 50 } else { 250 };
        if !event::poll(Duration::from_millis(poll_ms))? {
            continue;
        }
        match event::read()? {
            Event::Key(k) => {
                let term_area = term
                    .size()
                    .map(|s| Rect::new(0, 0, s.width, s.height))
                    .unwrap_or_default();
                handle_key(k, &client, &mut state, term_area);
            }
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
    state.set_recent(client.get_recent(50, 0).map_err(|e| e.to_string()));
    state.quota = Some(client.get_quota_status().map_err(|e| e.to_string()));
}

fn refresh_view(client: &AdminClient, state: &mut AppState) {
    match state.view {
        View::Status => {
            state.set_status(client.get_status().map_err(|e| e.to_string()));
            state.quota = Some(client.get_quota_status().map_err(|e| e.to_string()));
        }
        View::Config => state.set_config(client.get_config().map_err(|e| e.to_string())),
        View::Requests => state.set_recent(client.get_recent(50, 0).map_err(|e| e.to_string())),
        // Usage refreshes on demand from `handle_key`'s Usage-tab branch
        // (`fetch_usage`); no auto-refresh tick should hit this arm.
        View::Usage => {}
        // Account refreshes on demand via fetch_account.
        View::Account => {}
    }
}

fn handle_key(k: KeyEvent, client: &AdminClient, state: &mut AppState, term_area: Rect) {
    if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
        state.should_quit = true;
        return;
    }

    if !matches!(&state.modal, Modal::None) {
        handle_modal_key(k, client, state);
        return;
    }
    state.flash = None;

    // Always-available keys (quit + help + tab switching).
    match k.code {
        KeyCode::Char('q') => {
            state.should_quit = true;
            return;
        }
        KeyCode::Char('?') => {
            state.modal = Modal::Help;
            return;
        }
        KeyCode::Esc => {
            // Go back to dashboard (Status / tab 1). This is the escape
            // route from the Usage tab where 1–4 are range presets.
            state.set_view(View::Status);
            return;
        }
        KeyCode::Char('4') => {
            state.set_view(View::Usage);
            if state.usage.summary.is_none() && state.usage.last_error.is_none() {
                fetch_usage(client, state);
            }
            return;
        }
        KeyCode::Char('5') => {
            state.set_view(View::Account);
            fetch_account(client, state);
            return;
        }
        _ => {}
    }

    // Usage-tab-specific keys. 1–4 switch range presets; Esc goes to
    // dashboard (handled above); 5 switches to this tab (handled above).
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

    // Account-tab-specific keys.
    if state.view == View::Account {
        match k.code {
            KeyCode::Char('r') => fetch_account(client, state),
            KeyCode::Up => {
                state.account.scroll_offset = state.account.scroll_offset.saturating_sub(1);
            }
            KeyCode::Down => {
                if let Some(u) = &state.account.usage
                    && state.account.scroll_offset + 1 < u.providers.len()
                {
                    state.account.scroll_offset += 1;
                }
            }
            _ => {}
        }
        return;
    }

    // Requests-tab-specific keys.
    if state.view == View::Requests {
        let visible_height = requests_visible_height(term_area, state);
        match k.code {
            KeyCode::Down | KeyCode::Char('j') => {
                // If near the bottom of the viewport, scroll down too.
                if state.requests.selected + 1 < state.requests.items.len() {
                    state.requests.selected += 1;
                    let max_scroll = state.requests.items.len().saturating_sub(visible_height);
                    if state.requests.selected
                        > state.requests.scroll_offset + visible_height.saturating_sub(1)
                    {
                        state.requests.scroll_offset = (state.requests.selected + 1)
                            .saturating_sub(visible_height)
                            .min(max_scroll);
                    }
                }
                // Auto-fetch more if we're near the end of loaded items.
                if state.requests.selected + 5 >= state.requests.items.len()
                    && state.requests.has_more()
                {
                    fetch_requests_next_page(client, state);
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                state.requests.selected = state.requests.selected.saturating_sub(1);
                if state.requests.selected < state.requests.scroll_offset {
                    state.requests.scroll_offset = state.requests.selected;
                }
            }
            KeyCode::PageDown => {
                let jump = visible_height.max(1);
                let target = (state.requests.selected + jump)
                    .min(state.requests.items.len().saturating_sub(1));
                state.requests.selected = target;
                let max_scroll = state.requests.items.len().saturating_sub(visible_height);
                state.requests.scroll_offset = (state.requests.selected + 1)
                    .saturating_sub(visible_height)
                    .min(max_scroll);
                // Auto-fetch more if we're near the end.
                if state.requests.selected + 5 >= state.requests.items.len()
                    && state.requests.has_more()
                {
                    fetch_requests_next_page(client, state);
                }
            }
            KeyCode::PageUp => {
                let jump = visible_height.max(1);
                state.requests.selected = state.requests.selected.saturating_sub(jump);
                if state.requests.selected < state.requests.scroll_offset {
                    state.requests.scroll_offset = state.requests.selected;
                }
            }
            KeyCode::Home => {
                state.requests.selected = 0;
                state.requests.scroll_offset = 0;
            }
            KeyCode::End => {
                state.requests.selected = state.requests.items.len().saturating_sub(1);
                let max_scroll = state.requests.items.len().saturating_sub(visible_height);
                state.requests.scroll_offset = max_scroll;
                // Fetch more if we're at the end and there's more data.
                if state.requests.has_more() {
                    fetch_requests_next_page(client, state);
                }
            }
            KeyCode::Char('r') => {
                state.requests.selected = 0;
                state.requests.scroll_offset = 0;
                refresh_view(client, state);
                state.flash("refreshed");
            }
            _ => {}
        }
        return;
    }

    // On all other tabs, 1–4 switch tabs.
    match k.code {
        KeyCode::Char('1') => state.set_view(View::Status),
        KeyCode::Char('2') => state.set_view(View::Config),
        KeyCode::Char('3') => state.set_view(View::Requests),
        KeyCode::Char('r') => {
            if state.mode == AppMode::Connected {
                refresh_view(client, state);
                state.flash("refreshed");
            } else {
                state.set_config(Err(proxy_required_message()));
            }
        }
        KeyCode::Down | KeyCode::Char('j') => state.move_selection_down(),
        KeyCode::Up | KeyCode::Char('k') => state.move_selection_up(),
        _ => {}
    }

    // Config-tab-specific keys.
    if state.view == View::Config {
        handle_config_key(k, client, state);
    }
}

// ---- Config tab key handling ----

fn handle_config_key(k: KeyEvent, client: &AdminClient, state: &mut AppState) {
    match k.code {
        // Section switching
        KeyCode::Left | KeyCode::Char('[') | KeyCode::BackTab => {
            state.config_section = state.config_section.prev();
        }
        KeyCode::Right | KeyCode::Char(']') | KeyCode::Tab => {
            state.config_section = state.config_section.next();
        }
        // Up/Down handled by move_selection_up/down above
        _ => {}
    }

    if state.mode == AppMode::ProxyRequired {
        state.set_config(Err(proxy_required_message()));
        return;
    }

    // Section-specific actions
    match state.config_section {
        ConfigSection::Providers => match k.code {
            KeyCode::Char('a') => open_add_modal(state),
            KeyCode::Char('e') => open_edit_modal(state),
            KeyCode::Char('d') => open_delete_modal(state),
            KeyCode::Char('t') => open_test_modal(state),
            KeyCode::Char('z') => toggle_provider_enabled(client, state),
            _ => {}
        },
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
                delete_routing_rule(client, state);
            }
            _ => {}
        },
        ConfigSection::Quotas => match k.code {
            KeyCode::Char('a') => {
                state.modal = Modal::QuotaForm(QuotaFormModal::new_for_add());
            }
            KeyCode::Char('e') => {
                open_quota_edit_modal(state);
            }
            KeyCode::Char('d') => {
                delete_quota_rule(client, state);
            }
            _ => {}
        },
        ConfigSection::Settings => {
            if k.code == KeyCode::Char('e') {
                toggle_affinity(client, state);
            }
        }
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
/// border. Each title is `" {N} {Label}"` joined by the default `│` divider.
///
/// The ratatui `Tabs` widget wraps every title with default left and right
/// padding of one space each, so the clickable region per tab is
/// `padding_left + title + padding_right` (title width + 2).
fn tab_hit(x: u16, y: u16, tabs_area: Rect) -> Option<View> {
    if y != tabs_area.y + 1 {
        return None;
    }
    let mut col = tabs_area.x + 1;
    for (i, v) in ALL_VIEWS.iter().enumerate() {
        if i > 0 {
            col += 1; // single-cell divider (│)
        }
        let label = format!(" {} {} ", i + 1, v.label());
        // +2 for the ratatui Tabs default left/right padding (one space each).
        let w = label.chars().count() as u16 + 2;
        if x >= col && x < col + w {
            return Some(*v);
        }
        col += w;
    }
    None
}

/// Hit-test the config section sub-tabs.  The `draw_config` function places the
/// section tabs on the first row inside the Config block's inner area.  The
/// block occupies `body_area`, so the first inner row is `body_area.y + 1`.
/// Each tab title is `" {Label} "` wrapped by the ratatui `Tabs` widget with
/// default left/right padding of one space each.
fn config_section_tab_hit(x: u16, y: u16, body_area: Rect) -> Option<ConfigSection> {
    let tabs_y = body_area.y + 1;
    if y != tabs_y {
        return None;
    }
    let mut col = body_area.x + 1;
    for (i, section) in ConfigSection::ALL.iter().enumerate() {
        if i > 0 {
            col += 1; // single-cell divider (│)
        }
        let label = format!(" {} ", section.label());
        // +2 for the ratatui Tabs default left/right padding (one space each).
        let w = label.chars().count() as u16 + 2;
        if x >= col && x < col + w {
            return Some(*section);
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
    state.requests.item_count()
}

fn requests_visible_height(term_area: Rect, _state: &AppState) -> usize {
    let chunks = main_layout(term_area);
    let body_area = chunks[1];
    // Block border (2) + header (1) + footer (1) = 4 rows of overhead.
    let visible = body_area.height.saturating_sub(4) as usize;
    visible.max(1)
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
            View::Requests => {
                if state.requests.selected > 0 {
                    state.requests.selected -= 1;
                    if state.requests.selected < state.requests.scroll_offset {
                        state.requests.scroll_offset = state.requests.selected;
                    }
                }
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
            View::Requests => {
                if state.requests.selected + 1 < state.requests.items.len() {
                    state.requests.selected += 1;
                    let visible_height = requests_visible_height(term_area, state);
                    let max_scroll = state.requests.items.len().saturating_sub(visible_height);
                    if state.requests.selected
                        > state.requests.scroll_offset + visible_height.saturating_sub(1)
                    {
                        state.requests.scroll_offset = (state.requests.selected + 1)
                            .saturating_sub(visible_height)
                            .min(max_scroll);
                    }
                }
                // Auto-fetch more if near the end.
                if state.requests.selected + 5 >= state.requests.items.len()
                    && state.requests.has_more()
                {
                    fetch_requests_next_page(client, state);
                }
            }
            _ => state.move_selection_down(),
        },
        MouseEventKind::Down(MouseButton::Left) => {
            state.flash = None;
            if let Some(v) = tab_hit(m.column, m.row, tabs_area) {
                state.set_view(v);
                match v {
                    View::Usage
                        if state.usage.summary.is_none() && state.usage.last_error.is_none() =>
                    {
                        fetch_usage(client, state);
                    }
                    View::Account => fetch_account(client, state),
                    _ => {}
                }
                return;
            }
            if !pos_in_rect(m.column, m.row, body_area) {
                return;
            }
            match state.view {
                View::Config => {
                    // Check section tab clicks first
                    if let Some(section) = config_section_tab_hit(m.column, m.row, body_area) {
                        state.config_section = section;
                        return;
                    }
                    if let Some(action) = provider_toolbar_hit(m.column, m.row, body_area) {
                        if state.mode == AppMode::ProxyRequired {
                            state.set_config(Err(proxy_required_message()));
                        } else {
                            dispatch_provider_action(action, client, state);
                        }
                        return;
                    }
                    if let Some(idx) =
                        table_row_hit(m.column, m.row, body_area, providers_count(state), 2)
                    {
                        state.providers_selected = idx;
                    }
                }
                View::Requests => {
                    // top_pad=1 for the footer row rendered inside the block
                    if let Some(idx) =
                        table_row_hit(m.column, m.row, body_area, requests_count(state), 0)
                    {
                        let actual = idx + state.requests.scroll_offset;
                        if actual < state.requests.items.len() {
                            state.requests.selected = actual;
                        }
                    }
                }
                View::Usage => {
                    let usage_chunks = usage_layout(body_area);
                    if let Some(p) = usage_range_hit(m.column, m.row, usage_chunks[0]) {
                        state.usage.range_preset = Some(p);
                        fetch_usage(client, state);
                    }
                }
                View::Status | View::Account => {}
            }
        }
        _ => {}
    }
}

/// Fetch the next page of requests and append to the existing list.
fn fetch_requests_next_page(client: &AdminClient, state: &mut AppState) {
    if !state.requests.has_more() || state.requests.loading {
        return;
    }
    state.requests.loading = true;
    let page_size = state.requests.page_size;
    let offset = state.requests.fetched_offset as u32;
    match client.get_recent(page_size, offset) {
        Ok(resp) => {
            state.requests.items.extend(resp.items);
            state.requests.total_count = resp.total_count;
            state.requests.fetched_offset = state.requests.items.len();
            state.requests.loading = false;
            state.requests.last_error = None;
        }
        Err(e) => {
            state.requests.last_error = Some(e.to_string());
            state.requests.loading = false;
        }
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

fn fetch_account(client: &AdminClient, state: &mut AppState) {
    // Non-blocking: spawn the HTTP call on a worker thread, leave the UI
    // free to keep redrawing. The main loop drains the receiver between
    // event polls and updates state when the response arrives.
    state.account.loading = true;
    state.account.last_error = None;
    let (tx, rx) = std::sync::mpsc::channel();
    let client = client.clone();
    std::thread::spawn(move || {
        let res = client.get_account_usage().map_err(|e| e.to_string());
        let _ = tx.send(res);
    });
    state.account_rx = Some(rx);
}

/// Drain any pending result from the in-flight Account fetch.
/// Called once per main-loop iteration before reading events.
fn drain_account_rx(state: &mut AppState) {
    let Some(rx) = state.account_rx.as_ref() else {
        return;
    };
    match rx.try_recv() {
        Ok(Ok(resp)) => {
            state.account.usage = Some(resp);
            state.account.last_error = None;
            state.account.scroll_offset = 0;
            state.account.loading = false;
            state.account_rx = None;
        }
        Ok(Err(e)) => {
            state.account.last_error = Some(e);
            state.account.loading = false;
            state.account_rx = None;
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => {}
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            // Worker died without sending — clear the in-flight flag.
            state.account.loading = false;
            state.account_rx = None;
        }
    }
}

// ---- Modal handling ----

fn handle_modal_key(k: KeyEvent, client: &AdminClient, state: &mut AppState) {
    let modal = std::mem::replace(&mut state.modal, Modal::None);
    let next = match modal {
        Modal::TestProvider(m) => handle_test_key(k, client, m),
        Modal::None => Modal::None,
        Modal::ProviderForm(m) => handle_form_key(k, client, state, m),
        Modal::DeleteConfirm(m) => handle_delete_key(k, client, state, m),
        Modal::DisableConfirm(m) => handle_disable_confirm_key(k, client, state, m),
        Modal::Help => match k.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => Modal::None,
            _ => Modal::Help,
        },
        Modal::RoutingForm(m) => handle_routing_form_key(k, client, state, m),
        Modal::QuotaForm(m) => handle_quota_form_key(k, client, state, m),
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
        "deepseek" => "deepseek-chat",
        "openai" => "gpt-4o-mini",
        "kimi" => "kimi-k2-0711-preview",
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

/// Toggle the selected provider's `enabled` flag. If disabling would affect
/// routing rules, open a confirmation modal instead of toggling immediately.
fn toggle_provider_enabled(client: &AdminClient, state: &mut AppState) {
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
    let currently_enabled = prov.enabled;

    // Disabling a referenced provider needs confirmation; enabling or
    // disabling an unreferenced one can be applied immediately.
    if !currently_enabled {
        let mut cfg = cfg;
        cfg.providers[provider_index].enabled = true;
        apply_toggle(client, state, cfg, &provider_name, "enabled");
        return;
    }

    let rules = crate::validate::rules_referencing(&provider_name, &cfg);
    if rules.is_empty() {
        let mut cfg = cfg;
        cfg.providers[provider_index].enabled = false;
        apply_toggle(client, state, cfg, &provider_name, "disabled");
    } else {
        state.modal = Modal::DisableConfirm(DisableConfirmModal {
            provider_index,
            provider_name,
            rules,
        });
    }
}

fn apply_toggle(
    client: &AdminClient,
    state: &mut AppState,
    cfg: proxy_admin_api::ConfigPayload,
    provider_name: &str,
    action: &str,
) {
    match client.put_config(&cfg) {
        Ok(updated) => {
            state.set_config(Ok(updated));
            state.flash(format!("{action} {provider_name}"));
        }
        Err(e) => state.flash(format!("toggle failed: {e}")),
    }
}

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
            if m.provider_index >= cfg.providers.len() {
                state.flash("toggle failed: provider index out of range");
                return Modal::None;
            }
            cfg.providers[m.provider_index].enabled = false;
            cfg.routing.retain(|r| {
                r.provider != m.provider_name && !r.fallback.contains(&m.provider_name)
            });
            match client.put_config(&cfg) {
                Ok(updated) => {
                    state.set_config(Ok(updated));
                    state.flash(format!(
                        "disabled {} and removed referenced rules",
                        m.provider_name
                    ));
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

fn handle_form_key(
    k: KeyEvent,
    client: &AdminClient,
    state: &mut AppState,
    mut m: ProviderFormModal,
) -> Modal {
    match (&m.state, k.code) {
        (_, KeyCode::Esc) => Modal::None,

        (FormState::Editing, KeyCode::Down) => {
            m.focused = m.focused.next(m.auth_kind, m.kind);
            Modal::ProviderForm(m)
        }
        (FormState::Editing, KeyCode::Up) => {
            m.focused = m.focused.prev(m.auth_kind, m.kind);
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
            if m.auth_kind == AuthInputKind::OAuthAnthropic
                || m.auth_kind == AuthInputKind::OAuthOpenAi
            {
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
            let auth_kind = m.auth_kind;
            let result = match auth_kind {
                AuthInputKind::OAuthOpenAi => {
                    client.oauth_complete_openai(&state_id, code.trim(), &provider_name)
                }
                _ => client.oauth_complete(&state_id, code.trim(), &provider_name),
            };
            match result {
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
        AuthInputKind::OAuthAnthropic | AuthInputKind::OAuthOpenAi => {
            unreachable!("OAuth handled separately")
        }
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
        anthropic_base_url: Some(&m.anthropic_base_url),
        openai_base_url: Some(&m.openai_base_url),
        reasoning_effort: m.reasoning_effort.as_option(),
        thinking_mode: m.thinking_mode.as_option(),
        sanitize_empty_tools: m.sanitize_empty_tools,
        enabled: m.enabled,

        max_concurrent: m.max_concurrent,
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
            let prev = m.kind;
            m.kind = if forward {
                m.kind.cycle_next()
            } else {
                m.kind.cycle_prev()
            };
            m.reasoning_effort = m.reasoning_effort.clamp_for(m.kind);
            if m.kind != ProviderKind::Minimax {
                m.thinking_mode = crate::app::ThinkingModeInput::Unset;
            }
            // Re-derive endpoint URLs from the new kind's defaults, but only
            // in the add flow and only into fields that still hold an
            // auto-fill value — empty, or exactly the PREVIOUS kind's
            // default. That way cycling through several kinds keeps each
            // buffer in sync with whichever kind is currently selected,
            // instead of leaving a stale default from an earlier kind
            // behind. Genuine user-typed text (anything else) is never
            // touched.
            if matches!(m.mode, FormMode::Add) {
                let (prev_anthropic_default, prev_openai_default) = prev.default_urls();
                let (new_anthropic_default, new_openai_default) = m.kind.default_urls();

                let anthropic_is_autofill = m.anthropic_base_url.is_empty()
                    || Some(m.anthropic_base_url.as_str()) == prev_anthropic_default;
                if anthropic_is_autofill {
                    m.anthropic_base_url = new_anthropic_default.unwrap_or_default().to_string();
                }

                let openai_is_autofill = m.openai_base_url.is_empty()
                    || Some(m.openai_base_url.as_str()) == prev_openai_default;
                if openai_is_autofill {
                    m.openai_base_url = new_openai_default.unwrap_or_default().to_string();
                }
            }
        }
        FormField::ReasoningEffort => {
            m.reasoning_effort = if forward {
                m.reasoning_effort.cycle_next_for(m.kind)
            } else {
                m.reasoning_effort.cycle_prev_for(m.kind)
            };
        }
        FormField::ThinkingMode => {
            m.thinking_mode = if forward {
                m.thinking_mode.cycle_next()
            } else {
                m.thinking_mode.cycle_prev()
            };
        }
        FormField::SanitizeEmptyTools => {
            m.sanitize_empty_tools = !m.sanitize_empty_tools;
        }
        FormField::Enabled => {
            m.enabled = !m.enabled;
        }
        FormField::AuthKind => {
            // AuthInputKind only has cycle(); use it for both directions
            // (5 variants → cycling 4 times == reverse). Fine for a TUI.
            m.auth_kind = if forward {
                m.auth_kind.cycle()
            } else {
                m.auth_kind.cycle().cycle().cycle().cycle()
            };
            // If switching to a kind without AuthValue, move focus off it.
            if matches!(
                m.auth_kind,
                AuthInputKind::Passthrough
                    | AuthInputKind::OAuthAnthropic
                    | AuthInputKind::OAuthOpenAi
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
        FormField::AnthropicBaseUrl => Some(&mut m.anthropic_base_url),
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
            anthropic_base_url: Some(&m.anthropic_base_url),
            openai_base_url: Some(&m.openai_base_url),
            reasoning_effort: m.reasoning_effort.as_option(),
            thinking_mode: m.thinking_mode.as_option(),
            sanitize_empty_tools: m.sanitize_empty_tools,
            enabled: m.enabled,

            max_concurrent: m.max_concurrent,
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
    let start_result = match m.auth_kind {
        AuthInputKind::OAuthOpenAi => client.oauth_start_openai(&new_provider_name),
        _ => client.oauth_start(&new_provider_name),
    };
    match start_result {
        Ok(resp) => {
            let auth_url = resp.authorization_url.clone();
            m.state = FormState::OAuthAwaitingCode {
                authorization_url: resp.authorization_url,
                state_id: resp.state_id,
                code_input: String::new(),
            };
            if auth_url.starts_with("http") {
                match open_url(&auth_url) {
                    Ok(()) => state.flash("opening authorization URL in your browser…"),
                    Err(e) => state.flash(format!(
                        "could not auto-open browser ({e}); copy the URL above"
                    )),
                }
            } else {
                state.flash(&auth_url);
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

// ---- Routing / Quota form handlers ----

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
    state.modal = Modal::RoutingForm(RoutingFormModal::from_rule(idx, &rule, available_providers));
}

fn open_quota_edit_modal(state: &mut AppState) {
    let cfg = match state.config.as_ref().and_then(|r| r.as_ref().ok()) {
        Some(c) => c,
        None => return,
    };
    let idx = state.quota_selected;
    let quota = match cfg.quota.get(idx) {
        Some(q) => q,
        None => return,
    };
    state.modal = Modal::QuotaForm(QuotaFormModal::from_rule(idx, quota));
}

fn delete_routing_rule(client: &AdminClient, state: &mut AppState) {
    let mut cfg = match state.config.as_ref().and_then(|r| r.as_ref().ok()).cloned() {
        Some(c) => c,
        None => return,
    };
    let idx = state.routing_selected;
    if idx >= cfg.routing.len() {
        return;
    }
    cfg.routing.remove(idx);
    // Clamp selection
    if state.routing_selected >= cfg.routing.len() {
        state.routing_selected = cfg.routing.len().saturating_sub(1);
    }
    match save_config_state(client, &cfg, state) {
        Ok(updated) => {
            state.set_config(Ok(updated));
            state.flash("routing rule deleted");
        }
        Err(e) => state.flash(format!("delete failed: {e}")),
    }
}

fn delete_quota_rule(client: &AdminClient, state: &mut AppState) {
    let mut cfg = match state.config.as_ref().and_then(|r| r.as_ref().ok()).cloned() {
        Some(c) => c,
        None => return,
    };
    let idx = state.quota_selected;
    if idx >= cfg.quota.len() {
        return;
    }
    cfg.quota.remove(idx);
    // Clamp selection
    if state.quota_selected >= cfg.quota.len() {
        state.quota_selected = cfg.quota.len().saturating_sub(1);
    }
    match save_config_state(client, &cfg, state) {
        Ok(updated) => {
            state.set_config(Ok(updated));
            state.flash("quota rule deleted");
        }
        Err(e) => state.flash(format!("delete failed: {e}")),
    }
}

fn toggle_affinity(client: &AdminClient, state: &mut AppState) {
    let mut cfg = match state.config.as_ref().and_then(|r| r.as_ref().ok()).cloned() {
        Some(c) => c,
        None => return,
    };
    cfg.affinity.enabled = !cfg.affinity.enabled;
    match save_config_state(client, &cfg, state) {
        Ok(updated) => {
            let enabled = updated.affinity.enabled;
            state.set_config(Ok(updated));
            let status = if enabled { "enabled" } else { "disabled" };
            state.flash(format!("affinity {status}"));
        }
        Err(e) => state.flash(format!("toggle failed: {e}")),
    }
}

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

fn handle_quota_form_key(
    k: KeyEvent,
    client: &AdminClient,
    state: &mut AppState,
    mut m: QuotaFormModal,
) -> Modal {
    match k.code {
        KeyCode::Esc => Modal::None,
        KeyCode::Down => {
            m.focused = m.focused.next();
            Modal::QuotaForm(m)
        }
        KeyCode::Up => {
            m.focused = m.focused.prev();
            Modal::QuotaForm(m)
        }
        KeyCode::Enter if m.focused == QuotaField::Save => {
            // Submit
            if m.provider.trim().is_empty() {
                m.error = Some("provider is required".into());
                return Modal::QuotaForm(m);
            }
            if m.window.trim().is_empty() {
                m.error = Some("window is required".into());
                return Modal::QuotaForm(m);
            }

            let mut cfg = match state.config.as_ref().and_then(|r| r.as_ref().ok()).cloned() {
                Some(c) => c,
                None => {
                    m.error = Some("config not loaded".into());
                    return Modal::QuotaForm(m);
                }
            };

            let quota = proxy_admin_api::QuotaPayload {
                provider: m.provider.trim().to_string(),
                window: m.window.trim().to_string(),
                max_requests: m.max_requests.trim().parse::<u64>().ok(),
                max_input_tokens: m.max_input_tokens.trim().parse::<u64>().ok(),
                max_output_tokens: m.max_output_tokens.trim().parse::<u64>().ok(),
                warn_pct: m.warn_pct.trim().parse::<u8>().unwrap_or(80),
            };

            match &m.mode {
                FormMode::Add => cfg.quota.push(quota),
                FormMode::Edit { original_index, .. } => {
                    if *original_index >= cfg.quota.len() {
                        m.error = Some("quota list changed; press Esc and reopen".into());
                        return Modal::QuotaForm(m);
                    }
                    cfg.quota[*original_index] = quota;
                }
            }

            match save_config_via_client(client, &cfg, state) {
                Ok(updated) => {
                    state.set_config(Ok(updated));
                    let action = match m.mode {
                        FormMode::Add => "added",
                        FormMode::Edit { .. } => "updated",
                    };
                    state.flash(format!("quota rule {action}"));
                    Modal::None
                }
                Err(e) => {
                    m.error = Some(format!("save failed: {e}"));
                    Modal::QuotaForm(m)
                }
            }
        }
        KeyCode::Backspace => {
            edit_quota_text(&mut m, |s| {
                s.pop();
            });
            Modal::QuotaForm(m)
        }
        KeyCode::Char(c) => {
            edit_quota_text(&mut m, |s| s.push(c));
            Modal::QuotaForm(m)
        }
        _ => Modal::QuotaForm(m),
    }
}

fn edit_quota_text(m: &mut QuotaFormModal, f: impl FnOnce(&mut String)) {
    m.error = None;
    let target: Option<&mut String> = match m.focused {
        QuotaField::Provider => Some(&mut m.provider),
        QuotaField::Window => Some(&mut m.window),
        QuotaField::MaxRequests => Some(&mut m.max_requests),
        QuotaField::MaxInputTokens => Some(&mut m.max_input_tokens),
        QuotaField::MaxOutputTokens => Some(&mut m.max_output_tokens),
        QuotaField::WarnPct => Some(&mut m.warn_pct),
        QuotaField::Save => None,
    };
    if let Some(s) = target {
        f(s);
    }
}

// ---- Dual-mode save helpers ----

/// Save config through the admin API when connected; otherwise require proxy.
fn save_config_state(
    client: &AdminClient,
    cfg: &proxy_admin_api::ConfigPayload,
    state: &AppState,
) -> Result<proxy_admin_api::ConfigPayload, String> {
    if state.mode == AppMode::Connected {
        client.put_config(cfg).map_err(|e| e.to_string())
    } else {
        Err(proxy_required_message())
    }
}

/// Save config through the admin API when connected; otherwise require proxy.
fn save_config_via_client(
    client: &AdminClient,
    cfg: &proxy_admin_api::ConfigPayload,
    state: &AppState,
) -> Result<proxy_admin_api::ConfigPayload, String> {
    if state.mode == AppMode::Connected {
        client.put_config(cfg).map_err(|e| e.to_string())
    } else {
        Err(proxy_required_message())
    }
}

fn strategy_cycle(
    s: &proxy_admin_api::RoutingStrategyPayload,
) -> proxy_admin_api::RoutingStrategyPayload {
    match s {
        proxy_admin_api::RoutingStrategyPayload::Failover => {
            proxy_admin_api::RoutingStrategyPayload::RoundRobin
        }
        proxy_admin_api::RoutingStrategyPayload::RoundRobin => {
            proxy_admin_api::RoutingStrategyPayload::Failover
        }
    }
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
        anthropic_base_url: Some(&m.anthropic_base_url),
        openai_base_url: Some(&m.openai_base_url),
        reasoning_effort: m.reasoning_effort.as_option(),
        thinking_mode: m.thinking_mode.as_option(),
        sanitize_empty_tools: m.sanitize_empty_tools,
        enabled: m.enabled,

        max_concurrent: m.max_concurrent,
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
            || prev.anthropic_base_url != provider.anthropic_base_url
            || prev.openai_base_url != provider.openai_base_url
            || prev.reasoning_effort != provider.reasoning_effort
            || prev.thinking_mode != provider.thinking_mode
            || prev.sanitize_empty_tools != provider.sanitize_empty_tools
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
    let start_result = match m.auth_kind {
        AuthInputKind::OAuthOpenAi => client.oauth_start_openai(&new_provider_name),
        _ => client.oauth_start(&new_provider_name),
    };
    match start_result {
        Ok(resp) => {
            let auth_url = resp.authorization_url.clone();
            m.state = FormState::OAuthAwaitingCode {
                authorization_url: resp.authorization_url,
                state_id: resp.state_id,
                code_input: String::new(),
            };
            if auth_url.starts_with("http") {
                match open_url(&auth_url) {
                    Ok(()) => state.flash("opening authorization URL in your browser…"),
                    Err(e) => state.flash(format!(
                        "could not auto-open browser ({e}); copy the URL above"
                    )),
                }
            } else {
                state.flash(&auth_url);
            }
        }
        Err(e) => {
            m.state = FormState::Failed(e.to_string());
        }
    }
    Modal::ProviderForm(m)
}

#[cfg(test)]
mod startup_flow_tests {
    use super::*;

    #[test]
    fn connected_proxy_uses_connected_flow() {
        assert_eq!(decide_startup_flow(true), StartupFlow::Connected);
    }

    #[test]
    fn offline_requires_proxy() {
        assert_eq!(decide_startup_flow(false), StartupFlow::ProxyRequired);
    }

    #[test]
    fn proxy_required_message_mentions_sqlite_and_proxy() {
        let msg = proxy_required_message();
        assert!(msg.contains("SQLite database"));
        assert!(msg.contains("proxy daemon"));
        assert!(msg.contains("cli-router proxy"));
    }
}

#[cfg(test)]
mod modal_key_tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use proxy_admin_api::{
        AffinityPayload, ConfigPayload, RecentRequestItem, RecentRequestsResponse,
    };

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn client() -> AdminClient {
        AdminClient::new("http://127.0.0.1:9")
    }

    fn app_state_with_config() -> AppState {
        let mut state = AppState::new();
        state.set_config(Ok(ConfigPayload {
            port: 3456,
            providers: Vec::new(),
            routing: Vec::new(),
            quota: Vec::new(),
            affinity: AffinityPayload::default(),
            proxy_db: None,
            pricing_db: None,
        }));
        state
    }

    fn recent_item(id: &str) -> RecentRequestItem {
        RecentRequestItem {
            id: id.to_string(),
            started_at_ms: 1_700_000_000_000,
            finished_at_ms: None,
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4".to_string(),
            status: "started".to_string(),
            input_tokens: None,
            output_tokens: None,
            cache_read_tokens: None,
            cache_creation_tokens: None,
            cost_usd: None,
            error_message: None,
            translation_direction: None,
        }
    }

    #[test]
    fn requests_r_reload_resets_selection_and_scroll() {
        let client = client();
        let mut state = AppState::new();
        state.set_view(View::Requests);
        state.set_recent(Ok(RecentRequestsResponse {
            items: vec![recent_item("old-1"), recent_item("old-2")],
            total_count: 2,
        }));
        state.requests.selected = 1;
        state.requests.scroll_offset = 1;

        handle_key(
            key(KeyCode::Char('r')),
            &client,
            &mut state,
            Rect::new(0, 0, 80, 24),
        );

        assert_eq!(state.requests.selected, 0);
        assert_eq!(state.requests.scroll_offset, 0);
    }

    #[test]
    fn provider_form_down_and_up_move_focus() {
        let client = client();
        let mut state = app_state_with_config();
        let modal = handle_form_key(
            key(KeyCode::Down),
            &client,
            &mut state,
            ProviderFormModal::new_for_add(),
        );
        let Modal::ProviderForm(m) = modal else {
            panic!("expected provider form modal");
        };
        assert_eq!(m.focused, FormField::Kind);

        let modal = handle_form_key(key(KeyCode::Up), &client, &mut state, m);
        let Modal::ProviderForm(m) = modal else {
            panic!("expected provider form modal");
        };
        assert_eq!(m.focused, FormField::Name);
    }

    #[test]
    fn provider_form_tab_no_longer_moves_focus() {
        let client = client();
        let mut state = app_state_with_config();
        let modal = handle_form_key(
            key(KeyCode::Tab),
            &client,
            &mut state,
            ProviderFormModal::new_for_add(),
        );
        let Modal::ProviderForm(m) = modal else {
            panic!("expected provider form modal");
        };
        assert_eq!(m.focused, FormField::Name);
    }

    #[test]
    fn provider_form_kind_change_prefills_empty_urls_in_add_flow() {
        let client = client();
        let mut state = app_state_with_config();
        let mut m = ProviderFormModal::new_for_add();
        m.focused = FormField::Kind;
        // Start from Codex (both URL buffers untouched) and cycle forward
        // one step to Minimax, its immediate successor.
        m.kind = ProviderKind::Codex;
        assert!(m.anthropic_base_url.is_empty());
        assert!(m.openai_base_url.is_empty());

        let modal = handle_form_key(key(KeyCode::Right), &client, &mut state, m);
        let Modal::ProviderForm(m) = modal else {
            panic!("expected provider form modal");
        };

        assert_eq!(m.kind, ProviderKind::Minimax);
        assert_eq!(m.anthropic_base_url, "https://api.minimaxi.com/anthropic");
        assert_eq!(m.openai_base_url, "https://api.minimaxi.com/v1");
    }

    #[test]
    fn provider_form_kind_change_does_not_overwrite_typed_url() {
        let client = client();
        let mut state = app_state_with_config();
        let mut m = ProviderFormModal::new_for_add();
        m.focused = FormField::Kind;
        m.anthropic_base_url = "https://custom.example.com".into();

        let modal = handle_form_key(key(KeyCode::Right), &client, &mut state, m);
        let Modal::ProviderForm(m) = modal else {
            panic!("expected provider form modal");
        };

        assert_eq!(m.kind, ProviderKind::Zai);
        // User-typed value survives the kind change untouched.
        assert_eq!(m.anthropic_base_url, "https://custom.example.com");
        // The other, still-empty buffer gets prefilled from Zai's defaults.
        assert_eq!(m.openai_base_url, "https://api.z.ai/api/paas/v4");
    }

    #[test]
    fn provider_form_kind_change_does_not_prefill_in_edit_flow() {
        let client = client();
        let mut state = app_state_with_config();
        let payload = proxy_admin_api::ProviderPayload {
            name: "anthropic".into(),
            kind: "anthropic".into(),
            enabled: true,
            auth: proxy_admin_api::AuthPayload::Passthrough,
            anthropic_base_url: None,
            openai_base_url: None,
            reasoning_effort: None,
            thinking_mode: None,
            max_concurrent: None,
            sanitize_empty_tools: None,
        };
        let mut m = ProviderFormModal::from_provider(0, &payload);
        m.focused = FormField::Kind;
        assert!(m.anthropic_base_url.is_empty());
        assert!(m.openai_base_url.is_empty());

        let modal = handle_form_key(key(KeyCode::Right), &client, &mut state, m);
        let Modal::ProviderForm(m) = modal else {
            panic!("expected provider form modal");
        };

        assert_eq!(m.kind, ProviderKind::Zai);
        // Edit flow never prefills, even though both buffers were empty.
        assert!(m.anthropic_base_url.is_empty());
        assert!(m.openai_base_url.is_empty());
    }

    #[test]
    fn provider_form_kind_change_re_derives_stale_defaults_across_multiple_cycles() {
        let client = client();
        let mut state = app_state_with_config();
        let mut m = ProviderFormModal::new_for_add();
        m.focused = FormField::Kind;
        // A genuine user-typed value that doesn't match any kind's default.
        m.anthropic_base_url = "https://custom.example.com".into();

        // Anthropic -> Zai: openai_base_url (empty) is auto-filled with
        // Zai's defaults. The custom anthropic_base_url is untouched
        // because it isn't empty and doesn't match Anthropic's default.
        let modal = handle_form_key(key(KeyCode::Right), &client, &mut state, m);
        let Modal::ProviderForm(mut m) = modal else {
            panic!("expected provider form modal");
        };
        assert_eq!(m.kind, ProviderKind::Zai);
        assert_eq!(m.anthropic_base_url, "https://custom.example.com");
        assert_eq!(m.openai_base_url, "https://api.z.ai/api/paas/v4");

        // Now overwrite anthropic_base_url with Zai's own default so we can
        // prove the stale-bleed fix: cycling forward to DeepSeek (whose
        // anthropic default is None) must CLEAR it instead of leaving
        // Zai's host behind bound to a DeepSeek key.
        m.anthropic_base_url = "https://api.z.ai/api/anthropic".into();
        let modal = handle_form_key(key(KeyCode::Right), &client, &mut state, m);
        let Modal::ProviderForm(m) = modal else {
            panic!("expected provider form modal");
        };
        assert_eq!(m.kind, ProviderKind::DeepSeek);
        assert!(
            m.anthropic_base_url.is_empty(),
            "stale Zai anthropic default must be cleared for a kind with no anthropic URL"
        );
        // openai_base_url held Zai's default (an auto-fill value), so it is
        // replaced with DeepSeek's default rather than left stale.
        assert_eq!(m.openai_base_url, "https://api.deepseek.com/v1");

        // Verify a genuine user-typed value survives a further kind
        // change: type a custom openai_base_url now, then cycle again.
        let mut m = m;
        m.openai_base_url = "https://custom-openai.example.com".into();
        let modal = handle_form_key(key(KeyCode::Right), &client, &mut state, m);
        let Modal::ProviderForm(m) = modal else {
            panic!("expected provider form modal");
        };
        assert_eq!(m.kind, ProviderKind::OpenAi);
        assert_eq!(m.openai_base_url, "https://custom-openai.example.com");
    }

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
        assert_eq!(
            m.error.as_deref(),
            Some("no providers configured; add a provider first")
        );
    }

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

    #[test]
    fn routing_fallback_appends_in_order() {
        let client = client();
        let mut state = app_state_with_config();
        let providers = vec![
            "anthropic".to_string(),
            "zai".to_string(),
            "openai".to_string(),
        ];
        let mut form = RoutingFormModal::new_for_add(providers);
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
        assert_eq!(
            m.fallback,
            vec!["openai".to_string(), "anthropic".to_string()]
        );
    }

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
        assert_eq!(
            m.error.as_deref(),
            Some("fallback must not contain the primary provider")
        );
    }

    #[test]
    fn routing_from_rule_filters_stale_fallback() {
        let rule = proxy_admin_api::RoutingRulePayload {
            r#match: proxy_admin_api::MatchPayload {
                model: Some("claude-*".into()),
            },
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
            r#match: proxy_admin_api::MatchPayload {
                model: Some("claude-*".into()),
            },
            provider: "ghost".to_string(),
            fallback: vec![],
            strategy: proxy_admin_api::RoutingStrategyPayload::Failover,
            priority: None,
        };
        let m =
            RoutingFormModal::from_rule(0, &rule, vec!["anthropic".to_string(), "zai".to_string()]);
        assert_eq!(m.provider_index, 0);
        assert_eq!(m.available_providers[0], "anthropic");
    }

    #[test]
    fn open_routing_edit_modal_flashes_when_provider_stale() {
        let mut state = AppState::new();
        let rule = proxy_admin_api::RoutingRulePayload {
            r#match: proxy_admin_api::MatchPayload {
                model: Some("claude-*".into()),
            },
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
                anthropic_base_url: None,
                openai_base_url: None,
                reasoning_effort: None,
                thinking_mode: None,
                max_concurrent: None,
                sanitize_empty_tools: None,
            }],
            routing: vec![rule],
            quota: vec![],
            affinity: AffinityPayload::default(),
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
        assert!(
            state.flash.as_ref().map_or(false, |f| f.contains("ghost")),
            "expected flash to mention stale provider, got: {:?}",
            state.flash
        );
    }

    #[test]
    fn quota_form_enter_on_non_save_does_not_submit() {
        let client = client();
        let mut state = app_state_with_config();
        let mut form = QuotaFormModal::new_for_add();
        form.provider = "anthropic".into();
        form.window = "1d".into();
        form.focused = QuotaField::Provider;

        let modal = handle_quota_form_key(key(KeyCode::Enter), &client, &mut state, form);
        let Modal::QuotaForm(m) = modal else {
            panic!("expected quota form modal");
        };
        assert_eq!(m.focused, QuotaField::Provider);
        assert!(
            state
                .config
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap()
                .quota
                .is_empty()
        );
    }

    #[test]
    fn quota_form_down_can_focus_save() {
        let client = client();
        let mut state = app_state_with_config();
        let mut form = QuotaFormModal::new_for_add();
        form.focused = QuotaField::WarnPct;

        let modal = handle_quota_form_key(key(KeyCode::Down), &client, &mut state, form);
        let Modal::QuotaForm(m) = modal else {
            panic!("expected quota form modal");
        };
        assert_eq!(m.focused, QuotaField::Save);
    }

    #[test]
    fn quota_form_s_no_longer_submits() {
        let client = client();
        let mut state = app_state_with_config();
        let mut form = QuotaFormModal::new_for_add();
        form.provider = "anthropic".into();
        form.window = "1d".into();
        form.focused = QuotaField::Save;

        let modal = handle_quota_form_key(key(KeyCode::Char('s')), &client, &mut state, form);
        let Modal::QuotaForm(m) = modal else {
            panic!("expected quota form modal");
        };
        assert_eq!(m.warn_pct, "80");
        assert!(
            state
                .config
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap()
                .quota
                .is_empty()
        );
    }
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

        // Walk to find each tab position, accounting for:
        //   left_pad(1) + title + right_pad(1) + divider(1)
        let labels = [
            " 1 Status ",
            " 2 Providers ",
            " 3 Routing ",
            " 4 Requests ",
            " 5 Usage ",
        ];
        let mut col = tabs_area.x + 1;
        let mut expected_views = ALL_VIEWS.iter();
        for (i, label) in labels.iter().enumerate() {
            if i > 0 {
                col += 1; // divider
            }
            let tab_width = label.chars().count() as u16 + 2; // +2 padding
            let mid_x = col + tab_width / 2;
            assert_eq!(
                tab_hit(mid_x, tabs_area.y + 1, tabs_area),
                Some(*expected_views.next().unwrap()),
                "clicking midpoint of tab {i} (col={mid_x}) should be {:?}",
                ALL_VIEWS[i],
            );
            col += tab_width;
        }
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
