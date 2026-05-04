//! All ratatui rendering. One entry `draw(frame, state)` builds the whole
//! layout: tab bar, body for the active view, status line, optional modal.

use crate::app::{
    ALL_VIEWS, AppState, AuthInputKind, DeleteConfirmModal, FormField, FormMode, FormState, Modal,
    ProviderFormModal, TestProviderModal, TestState, View,
};
use chrono::{Local, TimeZone};
use proxy_admin_api::{
    AuthPayload, ConfigPayload, QuotaMetricDto, QuotaMetricState, QuotaStatusListDto,
    RecentRequestsResponse, StatusResponse,
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
        View::Providers => draw_providers(f, chunks[1], state),
        View::Routing => draw_routing(f, chunks[1], state.config.as_ref()),
        View::Requests => draw_requests(f, chunks[1], state),
        View::Usage => crate::views::usage::draw(f, chunks[1], &state.usage),
    }
    draw_status_line(f, chunks[2], state);

    match &state.modal {
        Modal::None => {}
        Modal::TestProvider(m) => draw_test_modal(f, m),
        Modal::ProviderForm(m) => draw_form_modal(f, m),
        Modal::DeleteConfirm(m) => draw_delete_confirm_modal(f, m),
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
                Line::from(format!("Uptime:         {} s", s.uptime_seconds)),
                Line::from(format!("Total requests: {}", s.total_requests)),
                Line::from(affinity_line),
                Line::from(""),
                Line::from(Span::styled(
                    "By provider",
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
                "By status",
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
                    "Translation: {} completed | {} failed | {}",
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

// ---- Providers view ----

fn draw_providers(f: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default().borders(Borders::ALL).title(" Providers ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let cfg = match &state.config {
        None => return draw_message(f, inner, "loading…"),
        Some(Err(e)) => return draw_error(f, inner, e),
        Some(Ok(c)) => c,
    };
    if cfg.providers.is_empty() {
        return draw_message(f, inner, "(no providers configured)");
    }

    let header = Row::new(vec![
        Cell::from("name"),
        Cell::from("kind"),
        Cell::from("auth"),
        Cell::from("base_url"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    let rows: Vec<Row> = cfg
        .providers
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let style = if i == state.providers_selected {
                Style::default().fg(Color::Black).bg(Color::Yellow)
            } else {
                Style::default()
            };
            Row::new(vec![
                Cell::from(p.name.clone()),
                Cell::from(p.kind.clone()),
                Cell::from(auth_summary(&p.auth)),
                Cell::from(p.base_url.clone().unwrap_or_default()),
            ])
            .style(style)
        })
        .collect();

    let widths = [
        Constraint::Length(20),
        Constraint::Length(14),
        Constraint::Length(40),
        Constraint::Min(20),
    ];
    let table = Table::new(rows, widths).header(header);
    f.render_widget(table, inner);
}

// ---- Routing view ----

fn draw_routing(f: &mut Frame, area: Rect, cfg: Option<&Result<ConfigPayload, String>>) {
    let block = Block::default().borders(Borders::ALL).title(" Routing ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let cfg = match cfg {
        None => return draw_message(f, inner, "loading…"),
        Some(Err(e)) => return draw_error(f, inner, e),
        Some(Ok(c)) => c,
    };
    if cfg.routing.is_empty() {
        return draw_message(f, inner, "(no routing rules)");
    }

    let header = Row::new(vec![
        Cell::from("#"),
        Cell::from("match.model"),
        Cell::from("provider"),
        Cell::from("fallback"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    let rows: Vec<Row> = cfg
        .routing
        .iter()
        .enumerate()
        .map(|(i, r)| {
            Row::new(vec![
                Cell::from(format!("{}", i + 1)),
                Cell::from(r.r#match.model.clone().unwrap_or_else(|| "*".into())),
                Cell::from(r.provider.clone()),
                Cell::from(r.fallback.join(", ")),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(4),
        Constraint::Length(20),
        Constraint::Length(20),
        Constraint::Min(20),
    ];
    let table = Table::new(rows, widths).header(header);
    f.render_widget(table, inner);
}

// ---- Requests view ----

fn draw_requests(f: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Recent requests ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let recent = match &state.recent {
        None => return draw_message(f, inner, "loading…"),
        Some(Err(e)) => return draw_error(f, inner, e),
        Some(Ok(r)) => r,
    };
    if recent.items.is_empty() {
        return draw_message(f, inner, "(no requests recorded yet)");
    }

    draw_requests_table(f, inner, recent, state.requests_selected);
}

fn draw_requests_table(
    f: &mut Frame,
    area: Rect,
    recent: &RecentRequestsResponse,
    selected: usize,
) {
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

    let rows: Vec<Row> = recent
        .items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let style = if i == selected {
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

// ---- Status line ----

fn draw_status_line(f: &mut Frame, area: Rect, state: &AppState) {
    let mut hints = vec!["1-4: switch  r: refresh  q: quit"];
    if matches!(state.view, View::Providers) {
        hints.push("a: add  e: edit  d: delete  t: test");
    }
    if matches!(state.view, View::Providers | View::Requests) {
        hints.push("↑↓: select");
    }
    let mut line = vec![Line::from(hints.join("    "))];
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
        "anthropic\u{2192}openai" => "A\u{2192}O",
        "openai\u{2192}anthropic" => "O\u{2192}A",
        _ => "?",
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
    lines.push(Line::from(""));
    lines.push(row(FormField::Save, "[ Save ]", "(Enter to submit)".into()));
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
