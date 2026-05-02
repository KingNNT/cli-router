use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Cell, HighlightSpacing, Paragraph, Row, Table},
    Frame,
};

use crate::adapters::view_models::PricingViewModel;

pub fn draw(f: &mut Frame, vm: &PricingViewModel, area: Rect, offset: usize, is_searching: bool) {
    if vm.empty && vm.query.is_empty() {
        let msg = Paragraph::new(vec![
            Line::from("  No pricing data. Press `s` to sync from LiteLLM."),
            Line::from(""),
            Line::from(Span::styled(
                format!("  Last synced: {}", vm.last_sync_label),
                Style::default().fg(Color::DarkGray),
            )),
        ])
        .style(Style::default().fg(Color::DarkGray));
        f.render_widget(msg, area);
        return;
    }

    // Determine whether to show the input/banner line.
    let show_input = is_searching || !vm.query.is_empty();

    // Split area: optional input line, table, footer.
    let constraints: Vec<Constraint> = if show_input {
        vec![
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ]
    } else {
        vec![Constraint::Min(0), Constraint::Length(1)]
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    let (table_area, footer_area) = if show_input {
        (chunks[1], chunks[2])
    } else {
        (chunks[0], chunks[1])
    };

    // Render the input/banner line.
    if show_input {
        let input_line = if is_searching {
            Line::from(vec![
                Span::styled(
                    "/",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(vm.query.clone(), Style::default()),
                Span::styled(
                    "_",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::SLOW_BLINK),
                ),
            ])
        } else {
            Line::from(vec![
                Span::styled("Search: \"", Style::default().fg(Color::DarkGray)),
                Span::styled(vm.query.clone(), Style::default().fg(Color::Cyan)),
                Span::styled("\" (Esc to clear)", Style::default().fg(Color::DarkGray)),
            ])
        };
        f.render_widget(Paragraph::new(input_line), chunks[0]);
    }

    let total = vm.rows.len();
    let reserved_rows: u16 = 3; // header (1) + bottom_margin (1) + footer line (1)
    let available = table_area.height.saturating_sub(reserved_rows) as usize;
    let page_size = available.max(1);

    let max_offset = total.saturating_sub(page_size);
    let effective_offset = offset.min(max_offset);
    let start = effective_offset;
    let end = (start + page_size).min(total);
    let visible = &vm.rows[start..end];

    let header_cells = [
        "Model",
        "Input/tok",
        "Output/tok",
        "Cache R",
        "Cache W",
        "Provider",
        "Alias",
    ]
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
                Cell::from(Span::raw(r.input.clone())),
                Cell::from(Span::raw(r.output.clone())),
                Cell::from(Span::styled(
                    r.cache_read.clone(),
                    Style::default().fg(Color::DarkGray),
                )),
                Cell::from(Span::styled(
                    r.cache_write.clone(),
                    Style::default().fg(Color::DarkGray),
                )),
                Cell::from(Span::styled(
                    r.provider.clone(),
                    Style::default().fg(Color::DarkGray),
                )),
                Cell::from(Span::styled(
                    r.alias.clone(),
                    Style::default().fg(Color::DarkGray),
                )),
            ])
        })
        .collect();

    let widths = [
        Constraint::Min(30),
        Constraint::Length(12),
        Constraint::Length(12),
        Constraint::Length(12),
        Constraint::Length(12),
        Constraint::Length(15),
        Constraint::Length(15),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .highlight_spacing(HighlightSpacing::Always);
    f.render_widget(table, table_area);

    let footer_text = if total == 0 {
        format!("  No results for \"{}\"", vm.query)
    } else {
        format!(
            "  Rows {}-{} of {}   PgUp/PgDn to scroll, Home/End for first/last   / to search",
            start + 1,
            end,
            total
        )
    };
    let footer = Paragraph::new(footer_text).style(Style::default().fg(Color::DarkGray));
    f.render_widget(footer, footer_area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::view_models::{PricingRowVM, PricingViewModel};
    use ratatui::{backend::TestBackend, Terminal};

    fn render(vm: &PricingViewModel, offset: usize, is_searching: bool, w: u16, h: u16) -> String {
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| draw(f, vm, f.area(), offset, is_searching))
            .unwrap();
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

    fn sample_row(model: &str) -> PricingRowVM {
        PricingRowVM {
            model: model.into(),
            provider: "anthropic".into(),
            input: "$3.00/M".into(),
            output: "$15.00/M".into(),
            cache_read: "$0.30/M".into(),
            cache_write: "$3.75/M".into(),
            alias: "opus".into(),
        }
    }

    #[test]
    fn empty_without_query_shows_sync_hint_and_last_sync() {
        let vm = PricingViewModel {
            rows: vec![],
            last_sync_label: "2026-04-23".into(),
            empty: true,
            query: String::new(),
        };
        let rendered = render(&vm, 0, false, 80, 10);
        assert!(
            rendered.contains("No pricing data"),
            "expected sync hint; got:\n{}",
            rendered
        );
        assert!(
            rendered.contains("2026-04-23"),
            "expected last-sync label; got:\n{}",
            rendered
        );
    }

    #[test]
    fn searching_shows_slash_prompt_with_query() {
        let vm = PricingViewModel {
            rows: vec![sample_row("claude-opus-4-7")],
            last_sync_label: "2026-04-23".into(),
            empty: false,
            query: "opu".into(),
        };
        let rendered = render(&vm, 0, true, 120, 10);
        assert!(
            rendered.contains("/opu"),
            "expected active-search prompt; got:\n{}",
            rendered
        );
    }

    #[test]
    fn committed_query_shows_search_banner() {
        let vm = PricingViewModel {
            rows: vec![sample_row("claude-opus-4-7")],
            last_sync_label: "2026-04-23".into(),
            empty: false,
            query: "opus".into(),
        };
        let rendered = render(&vm, 0, false, 120, 10);
        assert!(
            rendered.contains("Search:") && rendered.contains("opus"),
            "expected committed-search banner; got:\n{}",
            rendered
        );
    }

    #[test]
    fn empty_with_query_shows_no_results_footer() {
        let vm = PricingViewModel {
            rows: vec![],
            last_sync_label: "2026-04-23".into(),
            empty: false,
            query: "xyz".into(),
        };
        let rendered = render(&vm, 0, false, 120, 10);
        assert!(
            rendered.contains("No results for") && rendered.contains("xyz"),
            "expected no-results footer; got:\n{}",
            rendered
        );
    }

    #[test]
    fn header_and_row_render_with_all_columns() {
        let vm = PricingViewModel {
            rows: vec![sample_row("claude-opus-4-7")],
            last_sync_label: "2026-04-23".into(),
            empty: false,
            query: String::new(),
        };
        let rendered = render(&vm, 0, false, 160, 10);
        for expected in [
            "Model",
            "Input/tok",
            "Output/tok",
            "Cache R",
            "Cache W",
            "Provider",
            "Alias",
        ] {
            assert!(
                rendered.contains(expected),
                "expected header {:?}; got:\n{}",
                expected,
                rendered
            );
        }
        assert!(rendered.contains("claude-opus-4-7"));
        assert!(rendered.contains("anthropic"));
        assert!(rendered.contains("opus"));
    }
}
