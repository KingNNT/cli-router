use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Paragraph},
};

use crate::tui::app_state::{Focus, Hit, View};

pub fn draw(f: &mut Frame, selected: usize, focus: Focus, area: Rect) -> Vec<Hit> {
    let items: Vec<Line> = View::ALL
        .iter()
        .enumerate()
        .map(|(i, view)| {
            let is_selected = i == selected;
            let prefix = match (is_selected, focus) {
                (true, Focus::Sidebar) => "▶ ",
                (true, Focus::Content) => "· ",
                (false, _) => "  ",
            };
            let style = if is_selected {
                match focus {
                    Focus::Sidebar => Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                    Focus::Content => Style::default().fg(Color::DarkGray),
                }
            } else {
                Style::default().fg(Color::White)
            };
            Line::styled(format!("{}{}", prefix, view.label()), style)
        })
        .collect();

    let sidebar = Paragraph::new(items).block(
        Block::default()
            .borders(Borders::RIGHT)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(sidebar, area);

    // One row per item, left-aligned inside the sidebar area (excluding the
    // right border). Items start at area.y (no top border on sidebar).
    let inner_width = area.width.saturating_sub(1);
    View::ALL
        .iter()
        .enumerate()
        .map(|(i, _)| {
            Hit(Rect {
                x: area.x,
                y: area.y + i as u16,
                width: inner_width,
                height: 1,
            })
        })
        .collect()
}
