//! Renderer for the Usage tab.

use crate::app::{RangePreset, UsagePaneState};
use proxy_admin_api::DailyUsageRow;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};
use shared::adapters::presenters::formatting::{fmt_cost, fmt_num, fmt_num_compact};

pub fn draw(f: &mut Frame<'_>, area: Rect, state: &UsagePaneState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // range tabs
            Constraint::Length(7), // overview card
            Constraint::Min(0),    // model table
            Constraint::Length(1), // help line
        ])
        .split(area);

    draw_range_tabs(f, chunks[0], state);
    draw_overview(f, chunks[1], state);
    draw_model_table(f, chunks[2], state);
    draw_help(f, chunks[3]);
}

fn draw_range_tabs(f: &mut Frame<'_>, area: Rect, state: &UsagePaneState) {
    let active = state.range_preset.unwrap_or(RangePreset::Today);
    let labels = [
        RangePreset::Today,
        RangePreset::D7,
        RangePreset::D30,
        RangePreset::All,
    ]
    .iter()
    .map(|p| {
        let lbl = format!(" {} ", p.label());
        if *p == active {
            format!("[{}]", lbl.trim())
        } else {
            lbl
        }
    })
    .collect::<Vec<_>>()
    .join("  ");
    let para = Paragraph::new(labels).block(Block::default().borders(Borders::BOTTOM));
    f.render_widget(para, area);
}

fn draw_overview(f: &mut Frame<'_>, area: Rect, state: &UsagePaneState) {
    let block = Block::default().borders(Borders::ALL).title(" Overview ");
    if let Some(err) = &state.last_error {
        let p = Paragraph::new(format!("Error: {err}\n(showing last good summary if any)"))
            .block(block);
        f.render_widget(p, area);
        return;
    }
    let Some(s) = &state.summary else {
        let msg = if state.loading {
            "Loading…"
        } else {
            "No data yet — press [r] to fetch."
        };
        f.render_widget(Paragraph::new(msg).block(block), area);
        return;
    };

    let totals = totals(&s.daily);
    let body = format!(
        "Range:    {} → {}     Requests:  {}\n\
         Input:    {} tokens                Cost: {}\n\
         Output:   {} tokens\n\
         Cache rd: {} tokens   Cache wr: {} tokens",
        first_date(&s.daily).unwrap_or("—"),
        last_date(&s.daily).unwrap_or("—"),
        fmt_num(totals.requests),
        fmt_num_compact(totals.input_tokens),
        fmt_cost(totals.cost_usd),
        fmt_num_compact(totals.output_tokens),
        fmt_num_compact(totals.cache_read_tokens),
        fmt_num_compact(totals.cache_creation_tokens),
    );
    f.render_widget(Paragraph::new(body).block(block), area);
}

fn draw_model_table(f: &mut Frame<'_>, area: Rect, state: &UsagePaneState) {
    let block = Block::default().borders(Borders::ALL).title(" By model ");
    let Some(s) = &state.summary else {
        f.render_widget(block, area);
        return;
    };
    if s.models.is_empty() {
        f.render_widget(
            Paragraph::new("No requests in this range.").block(block),
            area,
        );
        return;
    }

    let header = Row::new(vec![
        Cell::from("Model"),
        Cell::from("Provider"),
        Cell::from("Reqs"),
        Cell::from("Tokens"),
        Cell::from("Cost"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    let rows = s.models.iter().skip(state.table_offset).map(|m| {
        Row::new(vec![
            Cell::from(m.model.clone()),
            Cell::from(m.provider.clone()),
            Cell::from(fmt_num(m.requests)),
            Cell::from(fmt_num_compact(
                m.input_tokens + m.output_tokens + m.cache_read_tokens + m.cache_creation_tokens,
            )),
            Cell::from(fmt_cost(m.cost_usd)),
        ])
    });

    let widths = [
        Constraint::Percentage(35),
        Constraint::Percentage(20),
        Constraint::Length(9),
        Constraint::Length(10),
        Constraint::Length(10),
    ];

    let table = Table::new(rows, widths).header(header).block(block);
    f.render_widget(table, area);
}

fn draw_help(f: &mut Frame<'_>, area: Rect) {
    let p = Paragraph::new(" [1] Today  [2] 7d  [3] 30d  [4] All   [r] Refresh   [↑/↓] Scroll ");
    f.render_widget(p, area);
}

#[derive(Default)]
struct Totals {
    requests: u64,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_creation_tokens: u64,
    cost_usd: f64,
}

fn totals(daily: &[DailyUsageRow]) -> Totals {
    let mut t = Totals::default();
    for d in daily {
        t.requests += d.requests;
        t.input_tokens += d.input_tokens;
        t.output_tokens += d.output_tokens;
        t.cache_read_tokens += d.cache_read_tokens;
        t.cache_creation_tokens += d.cache_creation_tokens;
        t.cost_usd += d.cost_usd;
    }
    t
}

fn first_date(daily: &[DailyUsageRow]) -> Option<&str> {
    daily.first().map(|d| d.date.as_str())
}

fn last_date(daily: &[DailyUsageRow]) -> Option<&str> {
    daily.last().map(|d| d.date.as_str())
}
