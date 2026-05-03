use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

pub struct StatusBar<'a> {
    pub msgs: &'a str,
    pub cost: &'a str,
    pub extra: Option<&'a str>,
    pub window_label: &'a str,
}

pub fn draw(f: &mut Frame, bar: StatusBar<'_>, area: Rect) -> Rect {
    let StatusBar {
        msgs,
        cost,
        extra,
        window_label,
    } = bar;
    let left = match extra {
        Some(e) => format!(" ? help · {} ", e),
        None => " ? help ".to_string(),
    };
    let right = format!(" {} msgs │ {} │ {} ", msgs, cost, window_label);
    let padding = area.width as usize;
    let pad_len = padding.saturating_sub(left.len() + right.len());
    let status = Line::from(vec![
        Span::styled(left, Style::default().fg(Color::White)),
        Span::styled(" ".repeat(pad_len), Style::default()),
        Span::styled(right, Style::default().fg(Color::Cyan)),
    ]);
    f.render_widget(
        Paragraph::new(status).style(Style::default().bg(Color::DarkGray)),
        area,
    );

    // Hit rect for the "? help" hint — the prefix " ? help " (8 chars).
    let help_hint_width: u16 = " ? help ".len() as u16;
    Rect {
        x: area.x,
        y: area.y,
        width: help_hint_width.min(area.width),
        height: area.height.min(1),
    }
}
