//! Renderer for the Account tab — upstream provider quota and usage.

use crate::app::AccountPaneState;
use proxy_admin_api::{ProviderUsageStatus, UsageWindowDto};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Paragraph, Wrap};
use shared::adapters::presenters::formatting::fmt_num_compact;

pub fn draw(f: &mut Frame<'_>, area: Rect, state: &AccountPaneState) {
    if let Some(err) = &state.last_error {
        let msg = Paragraph::new(format!("Error: {err}\n(showing last good data if any)"))
            .wrap(Wrap { trim: true });
        f.render_widget(msg, area);
        return;
    }

    let Some(usage) = &state.usage else {
        let msg = if state.loading {
            "Loading..."
        } else {
            "No data yet — press [r] to fetch."
        };
        let p = Paragraph::new(msg);
        f.render_widget(p, area);
        return;
    };

    if usage.providers.is_empty() {
        let p = Paragraph::new("No providers configured.");
        f.render_widget(p, area);
        return;
    }

    // Render all provider blocks into one paragraph.
    let mut lines: Vec<ratatui::text::Line> = Vec::new();
    let mut visible_idx = 0;

    for provider in &usage.providers {
        if visible_idx < state.scroll_offset {
            visible_idx += 1;
            continue;
        }
        render_provider(&mut lines, provider);
        lines.push(ratatui::text::Line::raw(""));
        visible_idx += 1;
    }

    if lines.is_empty() {
        let p = Paragraph::new("Scroll up to see providers.");
        f.render_widget(p, area);
        return;
    }

    // Remove trailing empty line.
    if lines
        .last()
        .map(|l| l.to_string().is_empty())
        .unwrap_or(false)
    {
        lines.pop();
    }

    let p = Paragraph::new(lines).wrap(Wrap { trim: false });
    f.render_widget(p, area);
}

fn render_provider(
    lines: &mut Vec<ratatui::text::Line>,
    p: &proxy_admin_api::ProviderAccountUsageDto,
) {
    // Title line: provider name + plan tier.
    let title = match (&p.plan, p.status) {
        (Some(plan), ProviderUsageStatus::Available) => {
            format!("╭─ {} ({}) ─", p.provider, capitalize(plan))
        }
        (_, ProviderUsageStatus::NotSupported) => {
            format!("╭─ {} ─", p.provider)
        }
        (_, ProviderUsageStatus::Error) => {
            format!("╭─ {} (error) ─", p.provider)
        }
        (None, ProviderUsageStatus::Available) => {
            format!("╭─ {} ─", p.provider)
        }
    };
    lines.push(ratatui::text::Line::styled(
        title,
        Style::default().add_modifier(Modifier::BOLD),
    ));

    match p.status {
        ProviderUsageStatus::NotSupported => {
            lines.push(ratatui::text::Line::raw(
                "│  No account usage API available for this provider.",
            ));
            lines.push(ratatui::text::Line::raw("╰─"));
            return;
        }
        ProviderUsageStatus::Error => {
            lines.push(ratatui::text::Line::raw(
                "│  Failed to fetch usage data from this provider.",
            ));
            lines.push(ratatui::text::Line::raw("╰─"));
            return;
        }
        ProviderUsageStatus::Available => {}
    }

    // Quota windows.
    for window in &p.windows {
        render_window(lines, window);
    }

    // Model usage.
    if let Some(m) = &p.model_usage {
        lines.push(ratatui::text::Line::raw(format!(
            "│  Model usage (24h):  Tokens: {}   Calls: {}",
            fmt_num_compact(m.total_tokens),
            fmt_num_compact(m.total_calls),
        )));
    }

    lines.push(ratatui::text::Line::raw("╰─"));
}

fn render_window(lines: &mut Vec<ratatui::text::Line>, w: &UsageWindowDto) {
    let bar = progress_bar(w.used_pct);
    let color = if w.used_pct >= 80.0 {
        Color::Red
    } else if w.used_pct >= 50.0 {
        Color::Yellow
    } else {
        Color::Green
    };

    let reset = match w.resets_at_ms {
        Some(ms) => {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64;
            let diff_secs = ((ms - now_ms) / 1000).max(0);
            format!("resets in {}", fmt_duration_secs(diff_secs as u64))
        }
        None => String::new(),
    };

    // Main window line: label + bar + percentage + reset.
    lines.push(ratatui::text::Line::from(vec![
        ratatui::text::Span::raw(format!("│  {:16} ", w.label,)),
        ratatui::text::Span::styled(bar, Style::default().fg(color)),
        ratatui::text::Span::raw(format!("  {:5.1}%   {}", w.used_pct, reset)),
    ]));

    // Detail line: used / limit.
    if let (Some(used), Some(limit)) = (w.used, w.limit) {
        lines.push(ratatui::text::Line::raw(format!(
            "│  {:16} {} / {}",
            "",
            fmt_num_compact(used),
            fmt_num_compact(limit),
        )));
    }

    // Sub-items (e.g. MCP tool breakdown).
    if !w.sub_items.is_empty() {
        let sub_line: String = w
            .sub_items
            .iter()
            .map(|s| format!("{}: {}", s.label, fmt_num_compact(s.used)))
            .collect::<Vec<_>>()
            .join("  ");
        lines.push(ratatui::text::Line::raw(format!(
            "│  {:16} {}",
            "", sub_line,
        )));
    }
}

fn progress_bar(pct: f64) -> String {
    let filled = (pct.min(100.0) / 5.0).round() as usize; // 20-char bar
    let empty = 20 - filled.min(20);
    "\u{2588}".repeat(filled.min(20)) + &"\u{2591}".repeat(empty)
}

fn fmt_duration_secs(secs: u64) -> String {
    if secs >= 24 * 3600 {
        format!("{}d {}h", secs / 86400, (secs % 86400) / 3600)
    } else if secs >= 3600 {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    } else {
        format!("{}m {}s", secs / 60, secs % 60)
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}
