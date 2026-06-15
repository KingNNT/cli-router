use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

const GROUPS: &[(&str, &[(&str, &str)])] = &[
    (
        "Navigation",
        &[
            ("j / ↓", "down"),
            ("k / ↑", "up"),
            ("PgUp / PgDn", "page up / down"),
            ("Home / End", "first / last row"),
            ("Enter / Space / Tab / → / l", "open view (sidebar)"),
            ("Esc", "quit (sidebar) · back to sidebar (content)"),
            ("h / Backspace / Shift+Tab", "back to sidebar (content)"),
        ],
    ),
    (
        "Dashboard window",
        &[
            ("← / →", "prev / next window (Dashboard)"),
            ("d", "cycle window"),
            ("1 / 7 / 3", "1 day / 7 days / 30 days"),
        ],
    ),
    (
        "Dashboard columns",
        &[("Shift+← / Shift+→", "scroll model columns (Dashboard)")],
    ),
    ("Data source", &[("t", "toggle OpenCode ↔ Claude Code")]),
    (
        "Pricing",
        &[
            ("s", "sync pricing from LiteLLM"),
            ("/", "search (Pricing view)"),
            ("Enter", "confirm search (keep query)"),
            ("Esc", "clear search"),
        ],
    ),
    ("Global", &[("?", "toggle this help"), ("q", "quit")]),
];

pub fn draw(f: &mut Frame, full_area: Rect) {
    // Height must accommodate every group (6 groups + 1 footer + 2 borders).
    // 32 keeps the panel readable on a 24-line terminal (auto-clipped) and
    // never truncates any shortcut on a normal 30+ row window.
    let area = centered_rect(72, 32, full_area);
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn render(w: u16, h: u16) -> String {
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, f.area())).unwrap();
        let buf = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..h {
            for x in 0..w {
                out.push_str(buf.cell((x, y)).unwrap().symbol());
            }
            out.push('\n');
        }
        out
    }

    // Use a tall terminal so the centered panel does not clip the body
    // — every shortcut row must be visible to the test.
    const TEST_W: u16 = 100;
    const TEST_H: u16 = 40;

    #[test]
    fn help_lists_vim_l_shortcut_for_open_view() {
        let rendered = render(TEST_W, TEST_H);
        let open_view = find_line_with(&rendered, "open view")
            .unwrap_or_else(|| panic!("missing 'open view' entry in help:\n{rendered}"));
        assert!(
            open_view.contains("l"),
            "'open view' entry should also document vim 'l'; got: {open_view:?}"
        );
    }

    #[test]
    fn help_lists_enter_shortcut_for_pricing_search_confirm() {
        let rendered = render(TEST_W, TEST_H);
        // "Enter" must appear in BOTH the Navigation group (open view) and the
        // Pricing group (confirm search). Counting ≥ 2 is the simplest check
        // that spans the right groups without depending on box-drawing
        // characters or row ordering.
        let enter_count = rendered.matches("Enter").count();
        assert!(
            enter_count >= 2,
            "expected 'Enter' ≥ 2 times (Navigation + Pricing); got {enter_count}:\n{rendered}"
        );
    }

    #[test]
    fn help_documents_esc_quit_in_sidebar() {
        let rendered = render(TEST_W, TEST_H);
        // "quit" must appear at least twice: once for 'q' (Global) and once for
        // Esc in the sidebar. If it only appears once, the user is misled into
        // thinking Esc = back-to-sidebar everywhere.
        let quit_count = rendered.matches("quit").count();
        assert!(
            quit_count >= 2,
            "expected 'quit' ≥ 2 times (for 'q' and sidebar Esc); got {quit_count}:\n{rendered}"
        );
    }

    #[test]
    fn help_documents_esc_back_to_sidebar_in_content() {
        let rendered = render(TEST_W, TEST_H);
        // After the fix, both the Esc row and the h/Backspace/Shift+Tab row
        // must say "back to sidebar" — so the phrase must appear ≥ 2 times.
        let back_count = rendered.matches("back to sidebar").count();
        assert!(
            back_count >= 2,
            "expected 'back to sidebar' ≥ 2 times (for Esc and h/Backspace/Shift+Tab); got {back_count}:\n{rendered}"
        );
    }

    /// Return the first rendered line that contains `needle` (trimmed).
    fn find_line_with(rendered: &str, needle: &str) -> Option<String> {
        rendered
            .lines()
            .find(|l| l.contains(needle))
            .map(|l| l.trim().to_string())
    }
}
