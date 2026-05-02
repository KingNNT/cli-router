use ratatui::layout::{Constraint, Direction, Layout, Rect};

pub struct Areas {
    pub sidebar: Rect,
    pub header: Rect,
    pub content: Rect,
    pub status: Rect,
}

pub fn compute(full: Rect) -> Areas {
    // Header spans the full terminal width across the top; status spans the
    // bottom. The middle row is split horizontally into sidebar + content.
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(full);

    let middle = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(16), Constraint::Min(0)])
        .split(rows[1]);

    Areas {
        sidebar: middle[0],
        header: rows[0],
        content: middle[1],
        status: rows[2],
    }
}
