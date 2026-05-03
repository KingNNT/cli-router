use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Span,
    widgets::{Cell, HighlightSpacing, Paragraph, Row, Table},
};

use crate::adapters::view_models::ModelsViewModel;

pub fn draw(f: &mut Frame, vm: &ModelsViewModel, area: Rect, offset: usize) {
    if vm.empty {
        let msg = Paragraph::new("  No model data for this period.")
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

    let header_cells = ["Model", "Messages", "Input", "Output", "Reasoning", "Cost"]
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
                Cell::from(Span::raw(r.model.clone())),
                Cell::from(Span::raw(r.messages.clone())),
                Cell::from(Span::raw(r.input.clone())),
                Cell::from(Span::raw(r.output.clone())),
                Cell::from(Span::raw(r.reasoning.clone())),
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
    use crate::adapters::view_models::{ModelRowVM, ModelsViewModel};
    use ratatui::{Terminal, backend::TestBackend};

    fn render(vm: &ModelsViewModel, offset: usize, w: u16, h: u16) -> String {
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

    fn sample_row(model: &str) -> ModelRowVM {
        ModelRowVM {
            model: model.into(),
            messages: "42".into(),
            input: "1.2K".into(),
            output: "3.4K".into(),
            reasoning: "0".into(),
            cost: "$0.78".into(),
        }
    }

    #[test]
    fn empty_state_shows_placeholder_message() {
        let vm = ModelsViewModel {
            empty: true,
            ..ModelsViewModel::default()
        };
        let rendered = render(&vm, 0, 80, 10);
        assert!(
            rendered.contains("No model data"),
            "expected placeholder; got:\n{}",
            rendered
        );
    }

    #[test]
    fn header_and_row_render_with_all_columns() {
        let vm = ModelsViewModel {
            rows: vec![sample_row("claude-opus-4-7")],
            pricing_note: None,
            empty: false,
        };
        let rendered = render(&vm, 0, 100, 10);
        for expected in ["Model", "Messages", "Input", "Output", "Reasoning", "Cost"] {
            assert!(
                rendered.contains(expected),
                "expected header {:?} in output; got:\n{}",
                expected,
                rendered
            );
        }
        assert!(rendered.contains("claude-opus-4-7"));
        assert!(rendered.contains("$0.78"));
    }

    #[test]
    fn footer_reports_visible_range_and_total() {
        let vm = ModelsViewModel {
            rows: (0..5).map(|i| sample_row(&format!("m{}", i))).collect(),
            pricing_note: None,
            empty: false,
        };
        let rendered = render(&vm, 0, 100, 12);
        assert!(
            rendered.contains("of 5"),
            "expected total count in footer; got:\n{}",
            rendered
        );
        assert!(
            rendered.contains("Rows 1-"),
            "expected range marker in footer; got:\n{}",
            rendered
        );
    }

    #[test]
    fn offset_beyond_data_clamps_to_last_page() {
        let vm = ModelsViewModel {
            rows: (0..3).map(|i| sample_row(&format!("m{}", i))).collect(),
            pricing_note: None,
            empty: false,
        };
        let rendered = render(&vm, 999, 100, 12);
        assert!(
            rendered.contains("m0"),
            "excess offset should clamp so first row still visible; got:\n{}",
            rendered
        );
    }
}
