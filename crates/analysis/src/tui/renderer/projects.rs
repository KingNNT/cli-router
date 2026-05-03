use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Span,
    widgets::{Cell, HighlightSpacing, Paragraph, Row, Table},
};

use crate::adapters::view_models::ProjectsViewModel;

pub fn draw(f: &mut Frame, vm: &ProjectsViewModel, area: Rect, offset: usize) {
    if vm.empty {
        let msg = Paragraph::new("  No project data for this period.")
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(msg, area);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(area);
    let table_area = chunks[0];
    let footer_area = chunks[1];

    let total = vm.rows.len();
    let reserved_rows: u16 = 3; // header (1) + bottom_margin (1) + footer (1)
    let available = table_area.height.saturating_sub(reserved_rows) as usize;
    let page_size = available.max(1);
    let max_offset = total.saturating_sub(page_size);
    let effective_offset = offset.min(max_offset);
    let start = effective_offset;
    let end = (start + page_size).min(total);
    let visible = &vm.rows[start..end];

    let header_cells = ["Project", "Messages", "Input", "Output", "Cost"]
        .iter()
        .map(|h| {
            Cell::from(*h).style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
        });
    let header = Row::new(header_cells).height(1).bottom_margin(1);

    let rows: Vec<Row> = visible
        .iter()
        .map(|r| {
            Row::new(vec![
                Cell::from(Span::raw(r.project.clone())),
                Cell::from(Span::raw(r.messages.clone())),
                Cell::from(Span::raw(r.input.clone())),
                Cell::from(Span::raw(r.output.clone())),
                Cell::from(Span::styled(
                    r.cost.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                )),
            ])
        })
        .collect();

    let widths = [
        Constraint::Min(30),
        Constraint::Length(10),
        Constraint::Length(12),
        Constraint::Length(12),
        Constraint::Length(10),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .highlight_spacing(HighlightSpacing::Always);
    f.render_widget(table, table_area);

    let footer_text = if total == 0 {
        String::new()
    } else {
        format!("  Rows {}-{} of {}", start + 1, end, total)
    };
    let footer = Paragraph::new(footer_text).style(Style::default().fg(Color::DarkGray));
    f.render_widget(footer, footer_area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::view_models::{ProjectRowVM, ProjectsViewModel};
    use ratatui::{Terminal, backend::TestBackend};

    fn render(vm: &ProjectsViewModel, offset: usize, w: u16, h: u16) -> String {
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, vm, f.area(), offset)).unwrap();
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

    fn sample_row(project: &str) -> ProjectRowVM {
        ProjectRowVM {
            project: project.into(),
            messages: "17".into(),
            input: "2.0K".into(),
            output: "800".into(),
            cost: "$0.42".into(),
        }
    }

    #[test]
    fn empty_state_shows_placeholder_message() {
        let vm = ProjectsViewModel {
            empty: true,
            ..ProjectsViewModel::default()
        };
        let rendered = render(&vm, 0, 80, 10);
        assert!(
            rendered.contains("No project data"),
            "expected placeholder; got:\n{}",
            rendered
        );
    }

    #[test]
    fn header_and_row_render_with_all_columns() {
        let vm = ProjectsViewModel {
            rows: vec![sample_row("/work/alpha")],
            empty: false,
        };
        let rendered = render(&vm, 0, 100, 10);
        for expected in ["Project", "Messages", "Input", "Output", "Cost"] {
            assert!(
                rendered.contains(expected),
                "expected header {:?} in output; got:\n{}",
                expected,
                rendered
            );
        }
        assert!(rendered.contains("/work/alpha"));
        assert!(rendered.contains("$0.42"));
    }

    #[test]
    fn footer_reports_visible_range_and_total() {
        let vm = ProjectsViewModel {
            rows: (0..4).map(|i| sample_row(&format!("/p{}", i))).collect(),
            empty: false,
        };
        let rendered = render(&vm, 0, 100, 12);
        assert!(rendered.contains("of 4"));
        assert!(rendered.contains("Rows 1-"));
    }

    #[test]
    fn offset_beyond_data_clamps_to_last_page() {
        let vm = ProjectsViewModel {
            rows: (0..2).map(|i| sample_row(&format!("/p{}", i))).collect(),
            empty: false,
        };
        let rendered = render(&vm, 999, 100, 12);
        assert!(
            rendered.contains("/p0"),
            "excess offset should clamp; got:\n{}",
            rendered
        );
    }
}
