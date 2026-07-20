//! All ratatui rendering. One entry `draw(frame, state)` builds the whole
//! layout: tab bar, body for the active view, status line, optional modal.

use crate::app::{
    ALL_VIEWS, AppMode, AppState, AuthInputKind, ConfigSection, DeleteConfirmModal,
    DisableConfirmModal, FormField, FormMode, FormState, Modal, ProviderFormModal, QuotaField,
    QuotaFormModal, RoutingField, RoutingFormModal, TestProviderModal, TestState, View,
};
use chrono::{Local, TimeZone};
use proxy_admin_api::{
    AuthPayload, QuotaMetricDto, QuotaMetricState, QuotaStatusListDto, StatusResponse,
};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, Tabs, Wrap};

pub fn draw(f: &mut Frame, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(2),
        ])
        .split(f.area());

    draw_tabs(f, chunks[0], state);
    match state.view {
        View::Status => draw_status(f, chunks[1], state.status.as_ref(), state.quota.as_ref()),
        View::Config => draw_config(f, chunks[1], state),
        View::Requests => draw_requests(f, chunks[1], state),
        View::Usage => crate::views::usage::draw(f, chunks[1], &state.usage),
        View::Account => crate::views::account::draw(f, chunks[1], &state.account),
    }
    draw_status_line(f, chunks[2], state);

    match &state.modal {
        Modal::None => {}
        Modal::TestProvider(m) => draw_test_modal(f, m),
        Modal::ProviderForm(m) => draw_form_modal(f, m),
        Modal::DeleteConfirm(m) => draw_delete_confirm_modal(f, m),
        Modal::DisableConfirm(m) => draw_disable_confirm_modal(f, m),
        Modal::Help => draw_help_modal(f),
        Modal::RoutingForm(m) => draw_routing_form_modal(f, m),
        Modal::QuotaForm(m) => draw_quota_form_modal(f, m),
    }
}

fn draw_tabs(f: &mut Frame, area: Rect, state: &AppState) {
    let titles: Vec<Line> = ALL_VIEWS
        .iter()
        .enumerate()
        .map(|(i, v)| Line::from(format!(" {} {} ", i + 1, v.label())))
        .collect();
    let active = ALL_VIEWS.iter().position(|v| *v == state.view).unwrap_or(0);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" cli-router proxy admin ");
    let tabs = Tabs::new(titles)
        .block(block)
        .select(active)
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );
    f.render_widget(tabs, area);
}

// ---- Status view ----

fn draw_status(
    f: &mut Frame,
    area: Rect,
    status: Option<&Result<StatusResponse, String>>,
    quota: Option<&Result<QuotaStatusListDto, String>>,
) {
    let halves = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(area);

    draw_status_panel(f, halves[0], status);
    draw_quota_panel(f, halves[1], quota);
}

fn draw_status_panel(f: &mut Frame, area: Rect, status: Option<&Result<StatusResponse, String>>) {
    let block = Block::default().borders(Borders::ALL).title(" Status ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let lines = match status {
        None => vec![Line::from("loading…")],
        Some(Err(e)) => vec![Line::from(Span::styled(
            format!("error: {e}"),
            Style::default().fg(Color::Red),
        ))],
        Some(Ok(s)) => {
            let started = format_ms(s.started_at_ms);
            let affinity_line = if s.affinity.enabled {
                format!(
                    "Affinity: enabled ({} headers + body fallback)",
                    s.affinity.headers.len()
                )
            } else {
                "Affinity: disabled (round-robin)".to_string()
            };
            let mut v = vec![
                Line::from(format!("Started:        {started}")),
                Line::from(format!(
                    "Uptime:         {}",
                    format_uptime(s.uptime_seconds)
                )),
                Line::from(format!("Total requests: {}", s.total_requests)),
                Line::from(affinity_line),
                Line::from(""),
                Line::from(Span::styled(
                    "By provider — where requests were routed",
                    Style::default().add_modifier(Modifier::BOLD),
                )),
            ];
            if s.requests_by_provider.is_empty() {
                v.push(Line::from("  (no requests yet)"));
            } else {
                for (k, n) in &s.requests_by_provider {
                    v.push(Line::from(format!("  {k:<16} {n}")));
                }
            }
            v.push(Line::from(""));
            v.push(Line::from(Span::styled(
                "By status — current/final request result",
                Style::default().add_modifier(Modifier::BOLD),
            )));
            if s.requests_by_status.is_empty() {
                v.push(Line::from("  (no requests yet)"));
            } else {
                for (k, n) in &s.requests_by_status {
                    v.push(Line::from(format!("  {k:<16} {n}")));
                }
            }
            v.push(Line::from(""));
            let translation_line = if s.translations_completed + s.translations_failed > 0 {
                let by_dir: Vec<String> = s
                    .translation_directions
                    .iter()
                    .map(|(k, count)| format!("{} {count}", xform_short_label(k)))
                    .collect();
                format!(
                    "Translation — API format conversions: {} completed | {} failed | {}",
                    s.translations_completed,
                    s.translations_failed,
                    by_dir.join(", ")
                )
            } else {
                "Translation: none yet".to_string()
            };
            v.push(Line::from(translation_line));
            v
        }
    };
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn format_uptime(seconds: u64) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;

    let days = seconds / DAY;
    let hours = (seconds % DAY) / HOUR;
    let minutes = (seconds % HOUR) / MINUTE;
    let secs = seconds % MINUTE;

    if days > 0 {
        format!("{days}d {hours}h {minutes}m {secs}s")
    } else if hours > 0 {
        format!("{hours}h {minutes}m {secs}s")
    } else if minutes > 0 {
        format!("{minutes}m {secs}s")
    } else {
        format!("{secs}s")
    }
}

// ---- Quota panel ----

fn draw_quota_panel(f: &mut Frame, area: Rect, quota: Option<&Result<QuotaStatusListDto, String>>) {
    let block = Block::default()
        .title(" Quota Usage ")
        .borders(Borders::ALL);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let quotas = match quota {
        Some(Ok(q)) => &q.quotas,
        Some(Err(e)) => {
            let p = Paragraph::new(format!("error: {e}")).style(Style::default().fg(Color::Red));
            f.render_widget(p, inner);
            return;
        }
        None => {
            f.render_widget(
                Paragraph::new("loading…").style(Style::default().fg(Color::DarkGray)),
                inner,
            );
            return;
        }
    };

    if quotas.is_empty() {
        f.render_widget(
            Paragraph::new("(no quotas configured)").style(Style::default().fg(Color::DarkGray)),
            inner,
        );
        return;
    }

    let mut lines: Vec<Line> = Vec::new();
    for q in quotas {
        lines.push(Line::from(Span::styled(
            format!(
                "{}  ({})  resets in {}",
                q.provider,
                q.window,
                fmt_duration_ms(q.window_resets_in_ms)
            ),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        push_metric_line(&mut lines, "req    ", &q.requests);
        push_metric_line(&mut lines, "in_tok ", &q.input_tokens);
        push_metric_line(&mut lines, "out_tok", &q.output_tokens);
        lines.push(Line::from(""));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn push_metric_line(out: &mut Vec<Line<'static>>, label: &str, m: &QuotaMetricDto) {
    if matches!(m.state, QuotaMetricState::Unconfigured) {
        return;
    }
    let bar = bar_string(m.pct);
    let color = match m.state {
        QuotaMetricState::Ok => Color::Green,
        QuotaMetricState::Warn => Color::Yellow,
        QuotaMetricState::Rejecting => Color::Red,
        QuotaMetricState::Unconfigured => Color::DarkGray,
    };
    let max_str = m.max.map(|n| n.to_string()).unwrap_or_else(|| "—".into());
    out.push(Line::from(vec![
        Span::raw(format!("  {} ", label)),
        Span::styled(bar, Style::default().fg(color)),
        Span::raw(format!(" {}% ({}/{})", m.pct, m.used, max_str)),
    ]));
}

fn bar_string(pct: u8) -> String {
    let filled = (pct as usize).min(100) / 5; // 20-char bar
    let empty = 20 - filled;
    "\u{2588}".repeat(filled) + &"\u{2591}".repeat(empty)
}

fn fmt_duration_ms(ms: u64) -> String {
    let secs = ms / 1000;
    if secs >= 3600 {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

// ---- Config view (with section tabs) ----

fn draw_config(f: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default().borders(Borders::ALL).title(" Config ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.height == 0 {
        return;
    }

    // Row 0 = section tabs; row 1 = separator; row 2+ = section content.
    let tabs_area = Rect::new(inner.x, inner.y, inner.width, 1);
    draw_config_section_tabs(f, tabs_area, state.config_section);

    if inner.height < 2 {
        return;
    }
    let hint_height = if inner.height >= 8 { 1 } else { 0 };
    let content_height = inner.height.saturating_sub(1 + hint_height);
    if content_height == 0 {
        return;
    }
    let content_area = Rect::new(inner.x, inner.y + 1, inner.width, content_height);

    match state.config_section {
        ConfigSection::Providers => draw_providers_content(f, content_area, state),
        ConfigSection::Routing => draw_routing_content(f, content_area, state),
        ConfigSection::Quotas => draw_quotas_content(f, content_area, state),
        ConfigSection::Settings => draw_settings_content(f, content_area, state),
    }

    if hint_height > 0 {
        let hint_area = Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1);
        draw_config_hint(
            f,
            hint_area,
            "←/→ section · ↑/↓ select · Enter/e edit · a add · d delete · ? help".into(),
        );
    }
}

fn draw_config_section_tabs(f: &mut Frame, area: Rect, active: ConfigSection) {
    let titles: Vec<Line> = ConfigSection::ALL
        .iter()
        .map(|s| Line::from(format!(" {} ", s.label())))
        .collect();
    let active_idx = ConfigSection::ALL
        .iter()
        .position(|&s| s == active)
        .unwrap_or(0);
    let tabs = Tabs::new(titles).select(active_idx).highlight_style(
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );
    f.render_widget(tabs, area);
}

fn draw_config_toolbar(f: &mut Frame, area: Rect, actions: &[&str]) {
    let mut spans: Vec<Span> = Vec::new();
    for (i, label) in actions.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(
            *label,
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_config_hint(f: &mut Frame, area: Rect, text: String) {
    f.render_widget(
        Paragraph::new(text).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

fn provider_summary(cfg: &proxy_admin_api::ConfigPayload) -> String {
    let mut oauth = 0usize;
    let mut api_key = 0usize;
    let mut bearer = 0usize;
    let mut passthrough = 0usize;
    let mut codex_auto = 0usize;

    for provider in &cfg.providers {
        match provider.auth {
            AuthPayload::AnthropicOAuth { .. } | AuthPayload::OpenAiOAuth { .. } => oauth += 1,
            AuthPayload::ApiKey { .. } => api_key += 1,
            AuthPayload::Bearer { .. } => bearer += 1,
            AuthPayload::Passthrough => passthrough += 1,
            AuthPayload::CodexAuto => codex_auto += 1,
        }
    }

    let mut parts = vec![format!("Providers: {} total", cfg.providers.len())];
    if oauth > 0 {
        parts.push(format!("{} OAuth", oauth));
    }
    if api_key > 0 {
        parts.push(format!("{} API key", api_key));
    }
    if bearer > 0 {
        parts.push(format!("{} Bearer", bearer));
    }
    if passthrough > 0 {
        parts.push(format!("{} passthrough", passthrough));
    }
    if codex_auto > 0 {
        parts.push(format!("{} Codex auto", codex_auto));
    }
    parts.join(" · ")
}

fn provider_detail_label(auth: &AuthPayload) -> Option<String> {
    match auth {
        AuthPayload::ApiKey { value } => Some(format!("API key: {}", redact(value))),
        AuthPayload::Bearer { value } => Some(format!("Bearer: {}", redact(value))),
        _ => None,
    }
}

fn selected_provider_footer(
    state: &AppState,
    cfg: &proxy_admin_api::ConfigPayload,
) -> Option<String> {
    let provider = cfg.providers.get(state.providers_selected)?;
    let mut parts = vec![
        format!("Selected: {}", provider.name),
        provider.kind.clone(),
    ];
    if let Some(auth) = provider_detail_label(&provider.auth) {
        parts.push(auth);
    }
    if let Some(base_url) = provider
        .base_url
        .as_deref()
        .or(provider.openai_base_url.as_deref())
        .filter(|s| !s.is_empty())
    {
        parts.push(base_url.to_string());
    }
    parts.push("[t] test".into());
    parts.push("[z] toggle active".into());
    Some(parts.join(" · "))
}

fn routing_summary(cfg: &proxy_admin_api::ConfigPayload) -> String {
    format!(
        "Routing: {} rules · evaluated top to bottom · default match: *",
        cfg.routing.len()
    )
}

fn selected_route_footer(state: &AppState, cfg: &proxy_admin_api::ConfigPayload) -> Option<String> {
    let route = cfg.routing.get(state.routing_selected)?;
    let model = route.r#match.model.as_deref().unwrap_or("*");
    let fallback = if route.fallback.is_empty() {
        "—".to_string()
    } else {
        route.fallback.join(", ")
    };
    Some(format!(
        "Selected: rule {} · {} → {} · fallback: {}",
        state.routing_selected + 1,
        model,
        route.provider,
        fallback
    ))
}

fn format_count(value: u64) -> String {
    let s = value.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

fn format_optional_count(value: Option<u64>) -> String {
    value.map(format_count).unwrap_or_else(|| "—".into())
}

fn quota_summary(cfg: &proxy_admin_api::ConfigPayload) -> String {
    let request_limits = cfg
        .quota
        .iter()
        .filter(|q| q.max_requests.is_some())
        .count();
    let token_limits = cfg
        .quota
        .iter()
        .filter(|q| q.max_input_tokens.is_some() || q.max_output_tokens.is_some())
        .count();
    format!(
        "Quotas: {} rules · {} request limits · {} token limits",
        cfg.quota.len(),
        request_limits,
        token_limits
    )
}

fn selected_quota_footer(state: &AppState, cfg: &proxy_admin_api::ConfigPayload) -> Option<String> {
    let quota = cfg.quota.get(state.quota_selected)?;
    let mut parts = vec![
        format!("Selected: {}", quota.provider),
        format!("{} window", quota.window),
    ];
    if let Some(max_requests) = quota.max_requests {
        parts.push(format!("{} req", format_count(max_requests)));
    }
    if let Some(max_input_tokens) = quota.max_input_tokens {
        parts.push(format!("{} input tok", format_count(max_input_tokens)));
    }
    if let Some(max_output_tokens) = quota.max_output_tokens {
        parts.push(format!("{} output tok", format_count(max_output_tokens)));
    }
    parts.push(format!("warn {}%", quota.warn_pct));
    Some(parts.join(" · "))
}

// ---- Providers section (content-only, no outer Block) ----

fn draw_providers_content(f: &mut Frame, area: Rect, state: &AppState) {
    if area.height == 0 {
        return;
    }

    let cfg = match &state.config {
        None => return draw_message(f, area, "Loading config…"),
        Some(Err(e)) => return draw_error(f, area, e),
        Some(Ok(c)) => c,
    };

    let summary_area = Rect::new(area.x, area.y, area.width, 1);
    f.render_widget(Paragraph::new(provider_summary(cfg)), summary_area);

    if area.height < 2 {
        return;
    }
    let toolbar_area = Rect::new(area.x, area.y + 1, area.width, 1);
    draw_provider_toolbar(f, toolbar_area);

    if area.height < 4 {
        return;
    }
    let footer_height = if area.height >= 6 && !cfg.providers.is_empty() {
        1
    } else {
        0
    };
    let table_height = area.height.saturating_sub(3 + footer_height);
    let table_area = Rect::new(area.x, area.y + 3, area.width, table_height);

    if cfg.providers.is_empty() {
        return draw_message(
            f,
            table_area,
            "No providers configured. Press [a] to add your first provider.",
        );
    }

    let header = Row::new(vec![
        Cell::from("Name"),
        Cell::from("Kind"),
        Cell::from("Auth"),
        Cell::from("Base URL"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    let rows: Vec<Row> = cfg
        .providers
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let style = if i == state.providers_selected {
                Style::default().fg(Color::Black).bg(Color::Yellow)
            } else if !p.enabled {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default()
            };
            Row::new(vec![
                Cell::from(p.name.clone()),
                Cell::from(p.kind.clone()),
                Cell::from(auth_summary(&p.auth)),
                Cell::from(
                    p.base_url
                        .as_deref()
                        .or(p.openai_base_url.as_deref())
                        .unwrap_or_default()
                        .to_string(),
                ),
            ])
            .style(style)
        })
        .collect();

    let widths = [
        Constraint::Length(20),
        Constraint::Length(14),
        Constraint::Length(28),
        Constraint::Min(20),
    ];
    let table = Table::new(rows, widths).header(header);
    f.render_widget(table, table_area);

    if footer_height > 0
        && let Some(footer) = selected_provider_footer(state, cfg)
    {
        let footer_area = Rect::new(area.x, area.y + area.height - 1, area.width, 1);
        draw_config_hint(f, footer_area, footer);
    }
}

// ---- Routing section ----

fn draw_routing_content(f: &mut Frame, area: Rect, state: &AppState) {
    if area.height == 0 {
        return;
    }

    let cfg = match &state.config {
        None => return draw_message(f, area, "Loading config…"),
        Some(Err(e)) => return draw_error(f, area, e),
        Some(Ok(c)) => c,
    };

    let summary_area = Rect::new(area.x, area.y, area.width, 1);
    f.render_widget(Paragraph::new(routing_summary(cfg)), summary_area);

    if area.height < 2 {
        return;
    }
    let toolbar_area = Rect::new(area.x, area.y + 1, area.width, 1);
    draw_routing_toolbar(f, toolbar_area);

    if area.height < 4 {
        return;
    }
    let footer_height = if area.height >= 6 && !cfg.routing.is_empty() {
        1
    } else {
        0
    };
    let table_height = area.height.saturating_sub(3 + footer_height);
    let table_area = Rect::new(area.x, area.y + 3, area.width, table_height);

    if cfg.routing.is_empty() {
        return draw_message(
            f,
            table_area,
            "No routing rules configured. Press [a] to add one.",
        );
    }

    let header = Row::new(vec![
        Cell::from("#"),
        Cell::from("Model Match"),
        Cell::from("Primary"),
        Cell::from("Fallbacks"),
        Cell::from("Strategy"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    let rows: Vec<Row> = cfg
        .routing
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let style = if i == state.routing_selected {
                Style::default().fg(Color::Black).bg(Color::Yellow)
            } else {
                Style::default()
            };
            let strategy_str = strategy_label(&r.strategy);
            let fallback = if r.fallback.is_empty() {
                "—".to_string()
            } else {
                r.fallback.join(", ")
            };
            Row::new(vec![
                Cell::from(format!("{}", i + 1)),
                Cell::from(r.r#match.model.clone().unwrap_or_else(|| "*".into())),
                Cell::from(r.provider.clone()),
                Cell::from(fallback),
                Cell::from(strategy_str),
            ])
            .style(style)
        })
        .collect();

    let widths = [
        Constraint::Length(4),
        Constraint::Length(20),
        Constraint::Length(20),
        Constraint::Length(24),
        Constraint::Min(12),
    ];
    let table = Table::new(rows, widths).header(header);
    f.render_widget(table, table_area);

    if footer_height > 0
        && let Some(footer) = selected_route_footer(state, cfg)
    {
        let footer_area = Rect::new(area.x, area.y + area.height - 1, area.width, 1);
        draw_config_hint(f, footer_area, footer);
    }
}

fn draw_routing_toolbar(f: &mut Frame, area: Rect) {
    draw_config_toolbar(f, area, &["[a] Add", "[e/Enter] Edit", "[d] Delete"]);
}

// ---- Quotas section ----

fn draw_quotas_content(f: &mut Frame, area: Rect, state: &AppState) {
    if area.height == 0 {
        return;
    }

    let cfg = match &state.config {
        None => return draw_message(f, area, "Loading config…"),
        Some(Err(e)) => return draw_error(f, area, e),
        Some(Ok(c)) => c,
    };

    let summary_area = Rect::new(area.x, area.y, area.width, 1);
    f.render_widget(Paragraph::new(quota_summary(cfg)), summary_area);

    if area.height < 2 {
        return;
    }
    let toolbar_area = Rect::new(area.x, area.y + 1, area.width, 1);
    draw_quotas_toolbar(f, toolbar_area);

    if area.height < 4 {
        return;
    }
    let footer_height = if area.height >= 6 && !cfg.quota.is_empty() {
        1
    } else {
        0
    };
    let table_height = area.height.saturating_sub(3 + footer_height);
    let table_area = Rect::new(area.x, area.y + 3, area.width, table_height);

    if cfg.quota.is_empty() {
        return draw_message(
            f,
            table_area,
            "No quota rules configured. Press [a] to add one.",
        );
    }

    let header = Row::new(vec![
        Cell::from("#"),
        Cell::from("Provider"),
        Cell::from("Window"),
        Cell::from("Requests"),
        Cell::from("Input Tok"),
        Cell::from("Output Tok"),
        Cell::from("Warn"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    let rows: Vec<Row> = cfg
        .quota
        .iter()
        .enumerate()
        .map(|(i, q)| {
            let style = if i == state.quota_selected {
                Style::default().fg(Color::Black).bg(Color::Yellow)
            } else {
                Style::default()
            };
            Row::new(vec![
                Cell::from(format!("{}", i + 1)),
                Cell::from(q.provider.clone()),
                Cell::from(q.window.clone()),
                Cell::from(format_optional_count(q.max_requests)),
                Cell::from(format_optional_count(q.max_input_tokens)),
                Cell::from(format_optional_count(q.max_output_tokens)),
                Cell::from(format!("{}%", q.warn_pct)),
            ])
            .style(style)
        })
        .collect();

    let widths = [
        Constraint::Length(4),
        Constraint::Length(16),
        Constraint::Length(12),
        Constraint::Length(12),
        Constraint::Length(14),
        Constraint::Length(14),
        Constraint::Min(6),
    ];
    let table = Table::new(rows, widths).header(header);
    f.render_widget(table, table_area);

    if footer_height > 0
        && let Some(footer) = selected_quota_footer(state, cfg)
    {
        let footer_area = Rect::new(area.x, area.y + area.height - 1, area.width, 1);
        draw_config_hint(f, footer_area, footer);
    }
}

fn draw_quotas_toolbar(f: &mut Frame, area: Rect) {
    draw_config_toolbar(f, area, &["[a] Add", "[e/Enter] Edit", "[d] Delete"]);
}

// ---- Settings section ----

fn draw_settings_content(f: &mut Frame, area: Rect, state: &AppState) {
    let cfg = match &state.config {
        None => return draw_message(f, area, "Loading config…"),
        Some(Err(e)) => return draw_error(f, area, e),
        Some(Ok(c)) => c,
    };

    let affinity_str = if cfg.affinity.enabled {
        "enabled"
    } else {
        "disabled"
    };
    let proxy_db_str = cfg.proxy_db.as_deref().unwrap_or("default");
    let pricing_db_str = cfg.pricing_db.as_deref().unwrap_or("default");
    let headers_str = if cfg.affinity.headers.is_empty() {
        "—".to_string()
    } else {
        cfg.affinity.headers.join(", ")
    };

    let lines = vec![
        Line::from(Span::styled(
            "Runtime",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled(
                "  Port:       ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(cfg.port.to_string()),
        ]),
        Line::from(vec![
            Span::styled(
                "  Proxy DB:   ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(proxy_db_str),
        ]),
        Line::from(vec![
            Span::styled(
                "  Pricing DB: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(pricing_db_str),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Affinity",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled(
                "  Status:     ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(affinity_str),
            Span::raw("    "),
            Span::styled(
                "[e] Toggle",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                "  Headers:    ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(headers_str),
        ]),
    ];
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

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

// ---- Requests view ----

fn draw_requests(f: &mut Frame, area: Rect, state: &AppState) {
    let reqs = &state.requests;

    // Reserve 1 row at the bottom for the pagination footer.
    let outer = Block::default()
        .borders(Borders::ALL)
        .title(" Recent requests ");
    let inner = outer.inner(area);
    f.render_widget(outer, area);

    if reqs.loading && reqs.items.is_empty() {
        return draw_message(f, inner, "loading…");
    }
    if let Some(e) = &reqs.last_error {
        return draw_error(f, inner, e);
    }
    if reqs.items.is_empty() {
        return draw_message(f, inner, "(no requests recorded yet)");
    }

    // Split inner into table area (all but last row) and footer (1 row).
    let table_h = inner.height.saturating_sub(1);
    if table_h == 0 {
        return;
    }
    let table_area = Rect {
        height: table_h,
        ..inner
    };
    let footer_area = Rect {
        y: inner.y + table_h,
        height: 1,
        ..inner
    };

    draw_requests_table(f, table_area, reqs);
    draw_requests_footer(f, footer_area, reqs);
}

fn draw_requests_table(f: &mut Frame, area: Rect, reqs: &crate::app::RequestsPaneState) {
    let header = Row::new(vec![
        Cell::from("started"),
        Cell::from("provider"),
        Cell::from("model"),
        Cell::from("xform"),
        Cell::from("status"),
        Cell::from("in"),
        Cell::from("out"),
        Cell::from("cost"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    let visible_height = area.height.saturating_sub(1) as usize; // minus header row
    let max_scroll = reqs.items.len().saturating_sub(visible_height);
    let scroll_offset = reqs.scroll_offset.min(max_scroll);

    let rows: Vec<Row> = reqs
        .items
        .iter()
        .enumerate()
        .skip(scroll_offset)
        .take(visible_height)
        .map(|(i, item)| {
            let style = if i == reqs.selected {
                Style::default().fg(Color::Black).bg(Color::Yellow)
            } else {
                Style::default()
            };
            let xform = match item.translation_direction.as_deref() {
                None => "\u{2014}".to_string(), // —
                Some("anthropic\u{2192}openai") => "A\u{2192}O".to_string(),
                Some("openai\u{2192}anthropic") => "O\u{2192}A".to_string(),
                Some(other) => other.chars().next().unwrap_or('?').to_string(),
            };
            Row::new(vec![
                Cell::from(format_ms(item.started_at_ms)),
                Cell::from(item.provider.clone()),
                Cell::from(trunc(&item.model, 24)),
                Cell::from(xform),
                Cell::from(item.status.clone()),
                Cell::from(opt_num(item.input_tokens)),
                Cell::from(opt_num(item.output_tokens)),
                Cell::from(
                    item.cost_usd
                        .map(|c| format!("${c:.4}"))
                        .unwrap_or_default(),
                ),
            ])
            .style(style)
        })
        .collect();

    let widths = [
        Constraint::Length(19),
        Constraint::Length(10),
        Constraint::Length(24),
        Constraint::Length(4),
        Constraint::Length(10),
        Constraint::Length(8),
        Constraint::Length(8),
        Constraint::Length(10),
    ];
    let table = Table::new(rows, widths).header(header);
    f.render_widget(table, area);
}

fn draw_requests_footer(f: &mut Frame, area: Rect, reqs: &crate::app::RequestsPaneState) {
    let loaded = reqs.items.len();
    let total = reqs.total_count as usize;
    let from = if loaded == 0 { 0 } else { 1 };
    let to = loaded;
    let more = if reqs.has_more() {
        " │ ↓/PgDn=more"
    } else {
        ""
    };
    let label = if total == 0 {
        "no requests".to_string()
    } else {
        format!("Showing {from}–{to} of {total}{more}")
    };
    let footer = Paragraph::new(Span::styled(
        label,
        Style::default().add_modifier(Modifier::DIM),
    ));
    f.render_widget(footer, area);
}

// ---- Status line ----

fn draw_status_line(f: &mut Frame, area: Rect, state: &AppState) {
    let mut line = vec![Line::from(Span::styled(
        "[?] Help",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ))];
    match state.mode {
        AppMode::Connected => {}
        AppMode::ProxyRequired => {
            line.push(Line::from(Span::styled(
                "⚠ Proxy required — config lives in SQLite DB",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )));
        }
    }
    if let Some(msg) = &state.flash {
        line.push(Line::from(Span::styled(
            msg.clone(),
            Style::default().fg(Color::LightGreen),
        )));
    }
    let p = Paragraph::new(line)
        .alignment(Alignment::Left)
        .block(Block::default().borders(Borders::TOP));
    f.render_widget(p, area);
}

// ---- Modals ----

fn draw_test_modal(f: &mut Frame, m: &TestProviderModal) {
    let area = centered_rect(60, 30, f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Test '{}' ", m.provider_name));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines = vec![
        Line::from(format!("Model:  {}", m.model_input)),
        Line::from(""),
    ];
    match &m.state {
        TestState::Editing => lines.push(Line::from("Enter: send  Esc: cancel")),
        TestState::InFlight => lines.push(Line::from(Span::styled(
            "sending…",
            Style::default().fg(Color::Yellow),
        ))),
        TestState::Done(r) => {
            let color = if r.success { Color::Green } else { Color::Red };
            lines.push(Line::from(Span::styled(
                format!(
                    "{} • status={} • latency={}ms",
                    if r.success { "OK" } else { "FAIL" },
                    r.status_code
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "-".into()),
                    r.latency_ms
                ),
                Style::default().fg(color),
            )));
            if let Some(err) = &r.error {
                lines.push(Line::from(Span::styled(
                    format!("error: {err}"),
                    Style::default().fg(Color::Red),
                )));
            }
            lines.push(Line::from(""));
            lines.push(Line::from("Esc: close"));
        }
        TestState::Failed(e) => {
            lines.push(Line::from(Span::styled(
                format!("transport error: {e}"),
                Style::default().fg(Color::Red),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from("Esc: close"));
        }
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

// ---- helpers ----

fn draw_message(f: &mut Frame, area: Rect, msg: &str) {
    f.render_widget(
        Paragraph::new(msg).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

fn draw_error(f: &mut Frame, area: Rect, err: &str) {
    f.render_widget(
        Paragraph::new(format!("error: {err}"))
            .style(Style::default().fg(Color::Red))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn auth_summary(a: &AuthPayload) -> String {
    match a {
        AuthPayload::Passthrough => "passthrough".into(),
        AuthPayload::ApiKey { value } => format!("api_key: {}", redact(value)),
        AuthPayload::Bearer { value } => format!("bearer: {}", redact(value)),
        AuthPayload::AnthropicOAuth { .. } => "anthropic_oauth".into(),
        AuthPayload::OpenAiOAuth { .. } => "openai_oauth".into(),
        AuthPayload::CodexAuto => "codex_auto".into(),
    }
}

fn redact(s: &str) -> String {
    if s.is_empty() {
        return "<empty>".into();
    }
    if s.len() <= 8 {
        return "*".repeat(s.len());
    }
    let prefix: String = s.chars().take(8).collect();
    let suffix: String = s
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{prefix}…{suffix}")
}

fn format_ms(ms: i64) -> String {
    Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| "-".into())
}

fn opt_num(n: Option<i64>) -> String {
    n.map(|x| x.to_string()).unwrap_or_default()
}

fn trunc(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let kept: String = s.chars().take(max - 1).collect();
        format!("{kept}…")
    }
}

fn xform_short_label(dir: &str) -> &str {
    match dir {
        "anthropic→openai" => "Anthropic→OpenAI",
        "openai→anthropic" => "OpenAI→Anthropic",
        _ => "unknown direction",
    }
}

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
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
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
    if m.kind == crate::app::ProviderKind::Codex {
        lines.push(row(
            FormField::ReasoningEffort,
            "Reasoning Effort:",
            format!("< {} >    [←/→ to cycle]", m.reasoning_effort.label()),
        ));
    }
    if m.kind == crate::app::ProviderKind::Minimax {
        lines.push(row(
            FormField::ThinkingMode,
            "Thinking Mode:",
            format!("< {} >    [←/→ to cycle]", m.thinking_mode.label()),
        ));
    }
    if m.kind == crate::app::ProviderKind::Kimi {
        lines.push(row(
            FormField::SanitizeEmptyTools,
            "Sanitize empty tools:",
            format!(
                "< {} >    [←/→ to toggle]",
                if m.sanitize_empty_tools { "on" } else { "off" }
            ),
        ));
    }
    lines.push(row(
        FormField::AuthKind,
        "Auth Kind:",
        format!("< {} >    [←/→ to cycle]", m.auth_kind.label()),
    ));
    if matches!(m.auth_kind, AuthInputKind::ApiKey | AuthInputKind::Bearer) {
        let masked = "*".repeat(m.auth_value.chars().count().min(40));
        let display = if m.auth_value.is_empty() {
            "<empty>".into()
        } else {
            masked
        };
        lines.push(row(FormField::AuthValue, "Auth Value:", display));
    }
    lines.push(row(
        FormField::Enabled,
        "Active:",
        format!(
            "< {} >    [←/→ to toggle]",
            if m.enabled { "yes" } else { "no" }
        ),
    ));
    lines.push(Line::from(""));
    lines.push(row(FormField::Save, "[ Save ]", "(Enter to submit)".into()));
    lines.push(Line::from(""));

    match &m.state {
        FormState::Editing => {
            lines.push(Line::from(Span::styled(
                "↑/↓: move  ←/→: cycle  Enter on Save: submit  Esc: cancel",
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
                "1. Your browser should have opened automatically.",
                Style::default().add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(Span::styled(
                authorization_url.clone(),
                Style::default().fg(Color::Cyan),
            )));
            lines.push(Line::from(Span::styled(
                "   (click anywhere in this modal to re-open, or copy the URL above)",
                Style::default().fg(Color::DarkGray),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "2. After authorising, Anthropic shows a `code#state` value.",
                Style::default().add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(
                "   Paste it here (the whole thing or just the `code` part — both work):",
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
            lines.push(Line::from("Enter on Save: retry  Esc: close"));
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
        lines.push(Line::from("Resolve routing rules first."));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "[Esc] close",
            Style::default().add_modifier(Modifier::BOLD),
        )));
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn draw_disable_confirm_modal(f: &mut Frame, m: &DisableConfirmModal) {
    let area = centered_rect(70, 60, f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Disable provider ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(
        format!(
            "Provider '{}' is referenced by {} routing rule(s).",
            m.provider_name,
            m.rules.len()
        ),
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

    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn draw_help_modal(f: &mut Frame) {
    let area = centered_rect(60, 75, f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Help — keyboard & mouse ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let bold = Style::default().add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(Color::DarkGray);

    let lines = vec![
        Line::from(Span::styled("Global", bold)),
        Line::from("  1 / 2 / 3 / 4         switch tab"),
        Line::from("  5                     usage tab"),
        Line::from("  Esc                   back to status (dashboard)"),
        Line::from("  r                     refresh current view"),
        Line::from("  ?                     toggle this help"),
        Line::from("  q / Ctrl+C            quit"),
        Line::from(""),
        Line::from(Span::styled("Providers tab", bold)),
        Line::from(
            "  ← / →                switch section (Providers / Routing / Quotas / Settings)",
        ),
        Line::from("  ↑ / ↓ / j / k         move selection"),
        Line::from("  a                     add provider"),
        Line::from("  e                     edit selected"),
        Line::from("  d                     delete selected"),
        Line::from("  t                     test selected"),
        Line::from(Span::styled("  (or click the toolbar buttons)", dim)),
        Line::from(""),
        Line::from(Span::styled("Status tab metrics", bold)),
        Line::from("  By provider       Counts requests by backend/provider name."),
        Line::from("  By status         Counts requests by lifecycle state."),
        Line::from("                    completed = finished successfully"),
        Line::from("                    errored   = failed"),
        Line::from("                    started   = still running or not finalized yet"),
        Line::from("  Translation       Counts API format conversions."),
        Line::from(
            "                    Anthropic→OpenAI means the proxy received Anthropic-style input",
        ),
        Line::from("                    and converted it for an OpenAI-style backend."),
        Line::from(""),
        Line::from(Span::styled("Requests tab", bold)),
        Line::from("  r                     refresh requests"),
        Line::from("  ↑ / ↓ / j / k         move selection"),
        Line::from("  PgUp / PgDn           scroll page"),
        Line::from("  Home / End            jump to start / end"),
        Line::from("  scroll wheel          scroll table"),
        Line::from(""),
        Line::from(Span::styled("Usage tab", bold)),
        Line::from("  1 / 2 / 3 / 4         range presets (Today / 7d / 30d / All)"),
        Line::from("  ↑ / ↓                 scroll model table"),
        Line::from("  Esc                   back to status (dashboard)"),
        Line::from(""),
        Line::from(Span::styled("Mouse", bold)),
        Line::from("  click tab             switch view"),
        Line::from("  click section tab      switch config section"),
        Line::from("  click row             select provider / request"),
        Line::from("  click button          run action"),
        Line::from("  scroll wheel          scroll / move selection"),
        Line::from("  click OAuth modal     re-open authorization URL"),
        Line::from(""),
        Line::from(Span::styled("[?] / [Esc] close help", bold)),
    ];
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

// ---- Routing form modal ----

fn draw_routing_form_modal(f: &mut Frame, m: &RoutingFormModal) {
    let area = centered_rect(65, 55, f.area());
    f.render_widget(Clear, area);
    let title = match &m.mode {
        FormMode::Add => " Add routing rule ".to_string(),
        FormMode::Edit { .. } => " Edit routing rule ".to_string(),
    };
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();

    if let Some(err) = &m.error {
        lines.push(Line::from(Span::styled(
            format!("\u{26a0} {err}"),
            Style::default().fg(Color::Red),
        )));
        lines.push(Line::from(""));
    }

    let row = |field: RoutingField, label: &str, value: String| {
        let marker = if m.focused == field {
            "\u{25b6} "
        } else {
            "  "
        };
        let style = if m.focused == field {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        Line::from(vec![
            Span::styled(format!("{marker}{label:<16}"), style),
            Span::raw(value),
        ])
    };

    lines.push(row(
        RoutingField::MatchModel,
        "Match Model:",
        show_or_placeholder(&m.match_model),
    ));
    lines.push(row(
        RoutingField::Provider,
        "Provider:",
        show_or_placeholder(&m.provider),
    ));
    lines.push(row(
        RoutingField::Fallback,
        "Fallback:",
        show_or_placeholder(&m.fallback),
    ));
    let strategy_str = strategy_label(&m.strategy);
    lines.push(row(
        RoutingField::Strategy,
        "Strategy:",
        format!("< {strategy_str} >    [Enter to cycle]"),
    ));
    lines.push(row(
        RoutingField::Priority,
        "Priority:",
        show_or_placeholder(&m.priority),
    ));
    lines.push(Line::from(""));
    lines.push(row(
        RoutingField::Save,
        "[ Save ]",
        "(Enter to submit)".into(),
    ));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "↑/↓: move  Enter on Strategy: cycle  Enter on Save: submit  Esc: cancel",
        Style::default().fg(Color::DarkGray),
    )));
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn strategy_label(s: &proxy_admin_api::RoutingStrategyPayload) -> &'static str {
    match s {
        proxy_admin_api::RoutingStrategyPayload::Failover => "failover",
        proxy_admin_api::RoutingStrategyPayload::RoundRobin => "round_robin",
    }
}

// ---- Quota form modal ----

fn draw_quota_form_modal(f: &mut Frame, m: &QuotaFormModal) {
    let area = centered_rect(65, 55, f.area());
    f.render_widget(Clear, area);
    let title = match &m.mode {
        FormMode::Add => " Add quota rule ".to_string(),
        FormMode::Edit { .. } => " Edit quota rule ".to_string(),
    };
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();

    if let Some(err) = &m.error {
        lines.push(Line::from(Span::styled(
            format!("\u{26a0} {err}"),
            Style::default().fg(Color::Red),
        )));
        lines.push(Line::from(""));
    }

    let row = |field: QuotaField, label: &str, value: String| {
        let marker = if m.focused == field {
            "\u{25b6} "
        } else {
            "  "
        };
        let style = if m.focused == field {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        Line::from(vec![
            Span::styled(format!("{marker}{label:<20}"), style),
            Span::raw(value),
        ])
    };

    lines.push(row(
        QuotaField::Provider,
        "Provider:",
        show_or_placeholder(&m.provider),
    ));
    lines.push(row(
        QuotaField::Window,
        "Window:",
        show_or_placeholder(&m.window),
    ));
    lines.push(row(
        QuotaField::MaxRequests,
        "Max Requests:",
        show_or_placeholder(&m.max_requests),
    ));
    lines.push(row(
        QuotaField::MaxInputTokens,
        "Max Input Tokens:",
        show_or_placeholder(&m.max_input_tokens),
    ));
    lines.push(row(
        QuotaField::MaxOutputTokens,
        "Max Output Tokens:",
        show_or_placeholder(&m.max_output_tokens),
    ));
    lines.push(row(
        QuotaField::WarnPct,
        "Warn %:",
        show_or_placeholder(&m.warn_pct),
    ));
    lines.push(Line::from(""));
    lines.push(row(
        QuotaField::Save,
        "[ Save ]",
        "(Enter to submit)".into(),
    ));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "↑/↓: move  Enter on Save: submit  Esc: cancel",
        Style::default().fg(Color::DarkGray),
    )));
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{AppState, Modal};
    use proxy_admin_api::{
        AffinityPayload, AffinityStatus, AuthPayload, ConfigPayload, MatchPayload, ProviderPayload,
        QuotaPayload, RoutingRulePayload, RoutingStrategyPayload, StatusResponse,
    };
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::collections::BTreeMap;

    fn render_state(state: &AppState, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test backend should initialize");
        terminal
            .draw(|frame| draw(frame, state))
            .expect("draw should succeed");
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    fn sample_status() -> StatusResponse {
        let mut by_provider = BTreeMap::new();
        by_provider.insert("router".to_string(), 42);
        let mut by_status = BTreeMap::new();
        by_status.insert("completed".to_string(), 40);
        by_status.insert("errored".to_string(), 1);
        by_status.insert("started".to_string(), 1);
        let mut translation_directions = BTreeMap::new();
        translation_directions.insert("anthropic→openai".to_string(), 7);

        StatusResponse {
            started_at_ms: 1_700_000_000_000,
            uptime_seconds: 60,
            total_requests: 42,
            requests_by_provider: by_provider,
            requests_by_status: by_status,
            affinity: AffinityStatus {
                enabled: true,
                headers: vec!["authorization".to_string()],
            },
            translations_completed: 7,
            translations_failed: 0,
            translation_directions,
        }
    }

    fn empty_config() -> ConfigPayload {
        ConfigPayload {
            port: 8787,
            providers: vec![],
            routing: vec![],
            quota: vec![],
            affinity: AffinityPayload {
                enabled: true,
                headers: vec!["x-session-id".into(), "anthropic-conversation-id".into()],
            },
            proxy_db: None,
            pricing_db: None,
        }
    }

    fn config_state(config: ConfigPayload) -> AppState {
        let mut state = AppState::new();
        state.view = View::Config;
        state.mode = AppMode::Connected;
        state.config = Some(Ok(config));
        state
    }

    #[test]
    fn config_providers_empty_state_suggests_add_action() {
        let state = config_state(empty_config());

        let rendered = render_state(&state, 120, 28);

        assert!(
            rendered.contains("No providers configured. Press [a] to add your first provider.")
        );
        assert!(rendered.contains("[a] Add"));
    }

    #[test]
    fn config_providers_render_summary_readable_headers_and_selected_footer() {
        let mut config = empty_config();
        config.providers = vec![
            ProviderPayload {
                name: "anthropic".into(),
                kind: "anthropic".into(),
                enabled: true,
                auth: AuthPayload::AnthropicOAuth {
                    access_token: "access".into(),
                    refresh_token: "refresh".into(),
                    expires_at_ms: 9_999,
                },
                base_url: Some("https://api.anthropic.com".into()),
                openai_base_url: None,
                reasoning_effort: None,
                thinking_mode: None,
                max_concurrent: None,
                sanitize_empty_tools: None,
            },
            ProviderPayload {
                name: "openai".into(),
                kind: "openai".into(),
                enabled: true,
                auth: AuthPayload::ApiKey {
                    value: "sk-test-secret-value".into(),
                },
                base_url: None,
                openai_base_url: Some("https://api.openai.com/v1".into()),
                reasoning_effort: None,
                thinking_mode: None,
                max_concurrent: None,
                sanitize_empty_tools: None,
            },
        ];
        let mut state = config_state(config);
        state.providers_selected = 0;

        let rendered = render_state(&state, 140, 30);

        assert!(rendered.contains("Providers: 2 total"));
        assert!(rendered.contains("1 OAuth"));
        assert!(rendered.contains("1 API key"));
        assert!(rendered.contains("Name"));
        assert!(rendered.contains("Kind"));
        assert!(rendered.contains("Auth"));
        assert!(rendered.contains("Base URL"));
        assert!(rendered.contains("OAuth"));
        assert!(rendered.contains("Selected: anthropic"));
        assert!(rendered.contains("[t] test"));

        state.providers_selected = 1;
        let rendered = render_state(&state, 140, 30);
        assert!(rendered.contains("API key:"));
    }

    #[test]
    fn config_routing_empty_state_suggests_add_action() {
        let mut state = config_state(empty_config());
        state.config_section = ConfigSection::Routing;

        let rendered = render_state(&state, 120, 28);

        assert!(rendered.contains("Routing: 0 rules"));
        assert!(rendered.contains("No routing rules configured. Press [a] to add one."));
        assert!(rendered.contains("[a] Add"));
    }

    #[test]
    fn config_routing_renders_summary_headers_fallback_dash_and_selected_footer() {
        let mut config = empty_config();
        config.routing = vec![
            RoutingRulePayload {
                r#match: MatchPayload {
                    model: Some("claude-*".into()),
                },
                provider: "anthropic".into(),
                fallback: vec![],
                strategy: RoutingStrategyPayload::Failover,
                priority: None,
            },
            RoutingRulePayload {
                r#match: MatchPayload { model: None },
                provider: "openai".into(),
                fallback: vec!["deepseek".into()],
                strategy: RoutingStrategyPayload::RoundRobin,
                priority: None,
            },
        ];
        let mut state = config_state(config);
        state.config_section = ConfigSection::Routing;
        state.routing_selected = 0;

        let rendered = render_state(&state, 140, 30);

        assert!(rendered.contains("Routing: 2 rules"));
        assert!(rendered.contains("evaluated top to bottom"));
        assert!(rendered.contains("Model Match"));
        assert!(rendered.contains("Primary"));
        assert!(rendered.contains("Fallbacks"));
        assert!(rendered.contains("—"));
        assert!(rendered.contains("Selected: rule 1"));
        assert!(rendered.contains("claude-* → anthropic"));
        assert!(rendered.contains("fallback: —"));
    }

    #[test]
    fn config_quotas_empty_state_suggests_add_action() {
        let mut state = config_state(empty_config());
        state.config_section = ConfigSection::Quotas;

        let rendered = render_state(&state, 120, 28);

        assert!(rendered.contains("Quotas: 0 rules"));
        assert!(rendered.contains("No quota rules configured. Press [a] to add one."));
        assert!(rendered.contains("[a] Add"));
    }

    #[test]
    fn config_quotas_render_summary_readable_numbers_percent_and_selected_footer() {
        let mut config = empty_config();
        config.quota = vec![QuotaPayload {
            provider: "anthropic".into(),
            window: "1d".into(),
            max_requests: Some(1_000),
            max_input_tokens: Some(2_000_000),
            max_output_tokens: None,
            warn_pct: 80,
        }];
        let mut state = config_state(config);
        state.config_section = ConfigSection::Quotas;
        state.quota_selected = 0;

        let rendered = render_state(&state, 140, 30);

        assert!(rendered.contains("Quotas: 1 rules"));
        assert!(rendered.contains("1 request limits"));
        assert!(rendered.contains("1 token limits"));
        assert!(rendered.contains("Provider"));
        assert!(rendered.contains("Requests"));
        assert!(rendered.contains("Input Tok"));
        assert!(rendered.contains("Output Tok"));
        assert!(rendered.contains("1,000"));
        assert!(rendered.contains("2,000,000"));
        assert!(rendered.contains("80%"));
        assert!(rendered.contains("Selected: anthropic"));
        assert!(rendered.contains("1d window"));
        assert!(rendered.contains("warn 80%"));
    }

    #[test]
    fn config_settings_renders_grouped_runtime_and_affinity_sections() {
        let mut config = empty_config();
        config.proxy_db = None;
        config.pricing_db = Some("/tmp/pricing.db".into());
        config.affinity.headers = vec![];
        let mut state = config_state(config);
        state.config_section = ConfigSection::Settings;

        let rendered = render_state(&state, 120, 28);

        assert!(rendered.contains("Runtime"));
        assert!(rendered.contains("Port:"));
        assert!(rendered.contains("Proxy DB:"));
        assert!(rendered.contains("Pricing DB:"));
        assert!(rendered.contains("Affinity"));
        assert!(rendered.contains("Status:"));
        assert!(rendered.contains("[e] Toggle"));
        assert!(rendered.contains("Headers:"));
        assert!(rendered.contains("—"));
    }

    #[test]
    fn config_tab_renders_navigation_hint_when_space_allows() {
        let state = config_state(empty_config());

        let rendered = render_state(&state, 120, 28);

        assert!(rendered.contains("←/→ section"));
        assert!(rendered.contains("↑/↓ select"));
        assert!(rendered.contains("Enter/e edit"));
    }

    #[test]
    fn format_uptime_uses_compact_human_readable_units() {
        assert_eq!(format_uptime(45), "45s");
        assert_eq!(format_uptime(3 * 60 + 12), "3m 12s");
        assert_eq!(format_uptime(4 * 60 * 60 + 8 * 60 + 30), "4h 8m 30s");
        assert_eq!(
            format_uptime(2 * 24 * 60 * 60 + 3 * 60 * 60 + 15 * 60 + 4),
            "2d 3h 15m 4s"
        );
    }

    #[test]
    fn status_view_renders_human_readable_uptime() {
        let mut status = sample_status();
        status.uptime_seconds = 72_080;
        let mut state = AppState::new();
        state.status = Some(Ok(status));

        let rendered = render_state(&state, 160, 48);

        assert!(rendered.contains("Uptime:         20h 1m 20s"));
        assert!(!rendered.contains("Uptime:         72080 s"));
    }

    #[test]
    fn status_view_explains_metric_sections_inline() {
        let mut state = AppState::new();
        state.status = Some(Ok(sample_status()));

        let rendered = render_state(&state, 160, 48);

        assert!(rendered.contains("By provider — where requests were routed"));
        assert!(rendered.contains("By status — current/final request result"));
        assert!(rendered.contains("Translation — API format conversions:"));
    }

    #[test]
    fn status_view_uses_readable_translation_direction_labels() {
        let mut state = AppState::new();
        state.status = Some(Ok(sample_status()));

        let rendered = render_state(&state, 160, 48);

        assert!(rendered.contains("Anthropic→OpenAI 7"));
        assert!(!rendered.contains("A→O 7"));
    }

    #[test]
    fn help_modal_explains_status_metrics() {
        let mut state = AppState::new();
        state.modal = Modal::Help;

        let rendered = render_state(&state, 160, 48);

        assert!(rendered.contains("Status tab metrics"));
        assert!(rendered.contains("By provider"));
        assert!(rendered.contains("Counts requests by backend/provider name."));
        assert!(rendered.contains("completed = finished successfully"));
        assert!(
            rendered.contains("Anthropic→OpenAI means the proxy received Anthropic-style input")
        );
    }

    #[test]
    fn provider_form_help_mentions_arrow_navigation_not_tab() {
        let mut state = AppState::new();
        state.modal = Modal::ProviderForm(crate::app::ProviderFormModal::new_for_add());

        let output = render_state(&state, 120, 30);

        assert!(output.contains("move"));
        assert!(output.contains("←/→: cycle"));
        assert!(output.contains("Enter on Save: submit"));
        assert!(!output.contains("Tab/Shift+Tab: move"));
    }

    #[test]
    fn provider_form_shows_active_toggle() {
        let mut state = AppState::new();
        let mut modal = crate::app::ProviderFormModal::new_for_add();
        modal.enabled = false;
        state.modal = Modal::ProviderForm(modal);

        let output = render_state(&state, 120, 30);

        assert!(output.contains("Active:"));
        assert!(output.contains("< no >"));
    }

    #[test]
    fn routing_form_renders_save_row_and_arrow_help() {
        let mut state = AppState::new();
        state.modal = Modal::RoutingForm(crate::app::RoutingFormModal::new_for_add());

        let output = render_state(&state, 120, 30);

        assert!(output.contains("[ Save ]"));
        assert!(output.contains("move"));
        assert!(output.contains("Enter on Save: submit"));
        assert!(!output.contains("s: submit"));
        assert!(!output.contains("Tab/Shift+Tab: move"));
    }

    #[test]
    fn quota_form_renders_save_row_and_arrow_help() {
        let mut state = AppState::new();
        state.modal = Modal::QuotaForm(crate::app::QuotaFormModal::new_for_add());

        let output = render_state(&state, 120, 30);

        assert!(output.contains("[ Save ]"));
        assert!(output.contains("move"));
        assert!(output.contains("Enter on Save: submit"));
        assert!(!output.contains("s: submit"));
        assert!(!output.contains("Tab/Shift+Tab: move"));
    }
}
