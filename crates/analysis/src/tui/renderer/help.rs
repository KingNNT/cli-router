use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

const GROUPS: &[(&str, &[(&str, &str)])] = &[
    (
        "Navigation",
        &[
            ("j / ↓", "down"),
            ("k / ↑", "up"),
            ("PgUp / PgDn", "page up / down"),
            ("Home / End", "first / last row"),
            ("Enter / Space / Tab / →", "open view"),
            ("Esc / h / Backspace / Shift+Tab", "back to sidebar"),
        ],
    ),
    (
        "Dashboard window",
        &[
            ("← / →", "prev / next window"),
            ("d", "cycle window"),
            ("1 / 7 / 3", "1 day / 7 days / 30 days"),
        ],
    ),
    (
        "Dashboard columns",
        &[("Shift+← / Shift+→", "scroll model columns")],
    ),
    ("Data source", &[("t", "toggle OpenCode ↔ Claude Code")]),
    (
        "Pricing",
        &[
            ("s", "sync pricing from LiteLLM"),
            ("/", "search (Pricing view)"),
            ("Esc", "clear search"),
        ],
    ),
    ("Global", &[("?", "toggle this help"), ("q", "quit")]),
];

pub fn draw(f: &mut Frame, full_area: Rect) {
    let area = centered_rect(72, 22, full_area);
    f.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(Span::styled(
            " Keyboard shortcuts ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(inner);
    let body = chunks[0];
    let footer = chunks[1];

    let mut lines: Vec<Line> = Vec::new();
    for (i, (group, entries)) in GROUPS.iter().enumerate() {
        if i > 0 {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(
            format!(" {}", group),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));
        for (keys, desc) in *entries {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("   {:<32}", keys),
                    Style::default().fg(Color::White),
                ),
                Span::styled(desc.to_string(), Style::default().fg(Color::DarkGray)),
            ]));
        }
    }
    f.render_widget(Paragraph::new(lines), body);

    let hint = Paragraph::new(Span::styled(
        " Press any key to close ",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC),
    ));
    f.render_widget(hint, footer);
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.min(area.width.saturating_sub(2));
    let height = height.min(area.height.saturating_sub(2));
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect {
        x,
        y,
        width,
        height,
    }
}
