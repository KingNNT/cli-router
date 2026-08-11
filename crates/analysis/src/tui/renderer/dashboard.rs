use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Cell, HighlightSpacing, Paragraph, Row, Table},
};

const TITLE_HEIGHT: u16 = 1;
const TAB_BAR_HEIGHT: u16 = 1;

use crate::adapters::view_models::{DashboardViewModel, ModelBreakdownVM};

const DATE_COL_WIDTH: u16 = 6;
const MIN_SUB_COL_WIDTH: u16 = 5;
const MAX_NUMERIC_WIDTH: u16 = 8;
const MAX_NAME_WIDTH: u16 = 24;
const TOTAL_COL_WIDTH: u16 = 8;
const COST_COL_WIDTH: u16 = 8;
const GROUP_SEP_WIDTH: u16 = 1;
const SUB_LABELS: [&str; 6] = ["In", "Out", "CR", "CW", "Rsn", "$"];

pub fn draw(
    f: &mut Frame,
    vm: &DashboardViewModel,
    area: Rect,
    row_offset: usize,
    col_offset: usize,
    source_label: &str,
) -> Vec<crate::tui::app_state::Hit> {
    // Title row + tab bar reserved at the top, even when empty.
    let top_chunks = Layout::vertical([
        Constraint::Length(TITLE_HEIGHT),
        Constraint::Length(TAB_BAR_HEIGHT),
        Constraint::Min(0),
    ])
    .split(area);
    let title_area = top_chunks[0];
    let tab_area = top_chunks[1];
    let below_tabs = top_chunks[2];

    draw_title(f, source_label, title_area);
    let tab_hits = draw_tab_bar(f, vm, tab_area);

    if vm.empty {
        let msg = Paragraph::new("  No data found for this period.")
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(msg, below_tabs);
        return tab_hits;
    }

    let (banner_area, body_area) = if vm.banner.is_some() {
        let chunks =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(below_tabs);
        (Some(chunks[0]), chunks[1])
    } else {
        (None, below_tabs)
    };

    if let (Some(banner_rect), Some(banner_text)) = (banner_area, vm.banner.as_deref()) {
        let banner =
            Paragraph::new(format!("  {}", banner_text)).style(Style::default().fg(Color::Yellow));
        f.render_widget(banner, banner_rect);
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(body_area);
    let table_area = chunks[0];
    let footer_area = chunks[1];

    // Effective model indices after horizontal scroll.
    let model_start = col_offset.min(vm.model_columns.len());

    // Compute width per sub-column (In/Out/CR/CW/$) for EVERY model from col_offset onwards.
    // First sub-col is widened to fit the full model name (up to MAX_NAME_WIDTH);
    // the rest keep a narrow numeric budget so compact tokens/costs fit snugly.
    let compute_sub = |mi: usize| -> [u16; 6] {
        let model_label = vm.model_columns[mi].model.as_str();
        let pricing_note = vm.model_columns[mi].pricing_note.as_str();
        let mut widths = [MIN_SUB_COL_WIDTH; 6];
        for (si, label) in SUB_LABELS.iter().enumerate() {
            widths[si] = widths[si].max(label.chars().count() as u16);
        }
        for r in &vm.rows {
            if let Some(cell) = r.model_cells.get(mi) {
                widen_to_fit(&mut widths, cell);
            }
        }
        if let Some(tot) = vm.column_totals.get(mi) {
            widen_to_fit(&mut widths, tot);
        }
        // First sub-col doubles as model name header — widen to fit.
        widths[0] = widths[0].max(model_label.chars().count() as u16);
        widths[0] = widths[0].max(pricing_note.chars().count() as u16);
        widths[0] = widths[0].clamp(MIN_SUB_COL_WIDTH, MAX_NAME_WIDTH);
        for w in widths.iter_mut().skip(1) {
            *w = (*w).clamp(MIN_SUB_COL_WIDTH, MAX_NUMERIC_WIDTH);
        }
        widths
    };

    // Fit as many model groups as the table area allows at their REQUESTED widths.
    // Ratatui's Table compresses all columns proportionally when the sum of
    // Constraint::Length exceeds the area, so we must cap the set here.
    // Budget = table width - borders - Date - Total - Cost - inter-column spacing.
    let borders_pad: u16 = 2;
    let spacing_per_col: u16 = 1; // ratatui default column spacing
    let fixed = DATE_COL_WIDTH + TOTAL_COL_WIDTH + COST_COL_WIDTH;
    // A leading separator is drawn between Date and the first model group.
    let leading_sep = GROUP_SEP_WIDTH + spacing_per_col;
    let mut budget = table_area
        .width
        .saturating_sub(borders_pad)
        .saturating_sub(fixed)
        .saturating_sub(leading_sep)
        // spacing for Date|...|Total|Cost — 3 gaps outside model groups.
        .saturating_sub(spacing_per_col * 3);

    let mut visible_models: Vec<usize> = Vec::new();
    let mut sub_widths: Vec<[u16; 6]> = Vec::new();
    for mi in model_start..vm.model_columns.len() {
        let widths = compute_sub(mi);
        // 6 sub-cols + 1 group separator column = 7 gaps + separator width.
        let group_width: u16 = widths.iter().sum::<u16>() + spacing_per_col * 7 + GROUP_SEP_WIDTH;
        if group_width > budget && !visible_models.is_empty() {
            break;
        }
        budget = budget.saturating_sub(group_width);
        visible_models.push(mi);
        sub_widths.push(widths);
        if budget == 0 {
            break;
        }
    }

    let mut all_widths: Vec<u16> = Vec::with_capacity(visible_models.len() * 6 + 4);
    // Track which columns are group separators so every row can fill them with "│".
    let mut is_sep: Vec<bool> = Vec::with_capacity(all_widths.capacity());
    all_widths.push(DATE_COL_WIDTH);
    is_sep.push(false);
    // Leading separator between Date and the first model group.
    if !visible_models.is_empty() {
        all_widths.push(GROUP_SEP_WIDTH);
        is_sep.push(true);
    }
    for w in &sub_widths {
        for &sub_w in w {
            all_widths.push(sub_w);
            is_sep.push(false);
        }
        all_widths.push(GROUP_SEP_WIDTH);
        is_sep.push(true);
    }
    all_widths.push(TOTAL_COL_WIDTH);
    is_sep.push(false);
    all_widths.push(COST_COL_WIDTH);
    is_sep.push(false);

    let sep_style = Style::default().fg(Color::DarkGray);

    // Two-line header: row 1 holds model names; row 2 holds sub-column labels.
    let mut header_cells: Vec<Cell> = Vec::with_capacity(all_widths.len());
    header_cells.push(two_line_header("", "Date"));
    if !visible_models.is_empty() {
        header_cells.push(two_line_header_bar(sep_style));
    }
    for (idx, &mi) in visible_models.iter().enumerate() {
        let model_label = vm.model_columns[mi].model.as_str();
        let pricing_note = vm.model_columns[mi].pricing_note.as_str();
        for (si, sub) in SUB_LABELS.iter().enumerate() {
            if si == 0 {
                header_cells.push(model_header_with_note(
                    model_label,
                    pricing_note,
                    sub_widths[idx][si] as usize,
                ));
            } else {
                header_cells.push(two_line_header_sub("", sub, sub_widths[idx][si] as usize));
            }
        }
        header_cells.push(two_line_header_bar(sep_style));
    }
    header_cells.push(two_line_header("", "Total"));
    header_cells.push(two_line_header("", "Cost"));
    let header = Row::new(header_cells).height(2);

    let separator_style = Style::default().fg(Color::DarkGray);
    let make_separator = || -> Row<'static> {
        let cells: Vec<Cell> = all_widths
            .iter()
            .zip(is_sep.iter())
            .map(|(&w, &sep)| {
                let glyph = if sep { "┼" } else { "─" };
                Cell::from(Span::styled(
                    glyph.to_string().repeat(w as usize),
                    separator_style,
                ))
            })
            .collect();
        Row::new(cells).height(1).style(separator_style)
    };

    let mut all_rows: Vec<Row> = Vec::new();
    all_rows.push(make_separator());

    for r in &vm.rows {
        let mut cells: Vec<Cell> = Vec::with_capacity(all_widths.len());
        cells.push(Cell::from(Span::styled(
            r.date_label.clone(),
            Style::default().fg(Color::Cyan),
        )));
        if !visible_models.is_empty() {
            cells.push(bar_cell(sep_style));
        }
        for &mi in &visible_models {
            if let Some(cell) = r.model_cells.get(mi) {
                push_breakdown_cells(&mut cells, cell, false);
            }
            cells.push(bar_cell(sep_style));
        }
        cells.push(Cell::from(Span::styled(
            r.total.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        cells.push(Cell::from(Span::styled(
            r.total_cost.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        all_rows.push(Row::new(cells).height(1));
    }

    all_rows.push(make_separator());

    // Grand totals row.
    let mut total_cells: Vec<Cell> = Vec::with_capacity(all_widths.len());
    total_cells.push(Cell::from(Span::styled(
        "Total",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )));
    if !visible_models.is_empty() {
        total_cells.push(bar_cell(sep_style));
    }
    for &mi in &visible_models {
        if let Some(cell) = vm.column_totals.get(mi) {
            push_breakdown_cells(&mut total_cells, cell, true);
        }
        total_cells.push(bar_cell(sep_style));
    }
    total_cells.push(Cell::from(Span::styled(
        vm.grand_total.clone(),
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )));
    total_cells.push(Cell::from(Span::styled(
        vm.grand_cost.clone(),
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )));
    all_rows.push(Row::new(total_cells).height(1));

    let total_rows = all_rows.len();
    let reserved_rows: u16 = 3; // 2-line header + footer line
    let available = table_area.height.saturating_sub(reserved_rows) as usize;
    let page_size = available.max(1);
    let max_offset = total_rows.saturating_sub(page_size);
    let effective_offset = row_offset.min(max_offset);
    let start = effective_offset;
    let end = (start + page_size).min(total_rows);
    let visible: Vec<Row> = all_rows.into_iter().skip(start).take(end - start).collect();

    let widths: Vec<Constraint> = all_widths.iter().map(|&w| Constraint::Length(w)).collect();

    let table = Table::new(visible, widths)
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .highlight_spacing(HighlightSpacing::Always);

    f.render_widget(table, table_area);

    let footer_text = if total_rows == 0 {
        String::new()
    } else {
        let mut text = format!("  Rows {}-{} of {}", start + 1, end, total_rows);
        let total_models = vm.model_columns.len();
        let shown_count = visible_models.len();
        if total_models > 0 && (model_start > 0 || shown_count < total_models) {
            text.push_str(&format!(
                "  ·  Models {}-{} of {}  (shift+← → to scroll)",
                model_start + 1,
                model_start + shown_count,
                total_models
            ));
        }
        text
    };
    let footer = Paragraph::new(footer_text).style(Style::default().fg(Color::DarkGray));
    f.render_widget(footer, footer_area);

    tab_hits
}

/// The six sub-column texts of a model group, in render order.
fn breakdown_texts(b: &ModelBreakdownVM) -> [&str; 6] {
    [
        &b.input,
        &b.output,
        &b.cache_read,
        &b.cache_write,
        &b.reasoning,
        &b.cost,
    ]
}

fn widen_to_fit(widths: &mut [u16; 6], cell: &ModelBreakdownVM) {
    for (si, text) in breakdown_texts(cell).iter().enumerate() {
        widths[si] = widths[si].max(text.chars().count() as u16);
    }
}

fn push_breakdown_cells(cells: &mut Vec<Cell<'static>>, b: &ModelBreakdownVM, bold: bool) {
    let base = if bold {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    for (i, text) in breakdown_texts(b).iter().enumerate() {
        let style = if *text == "—" {
            base.fg(Color::DarkGray)
        } else if i == 5 {
            // Cost column: keep bright, even when not bold.
            base
        } else if i >= 2 {
            // CR / CW / Rsn: dim to keep focus on In/Out.
            if bold {
                base.fg(Color::DarkGray)
            } else {
                Style::default().fg(Color::DarkGray)
            }
        } else {
            base
        };
        cells.push(Cell::from(Span::styled((*text).to_string(), style)));
    }
}

fn two_line_header(top: &str, bottom: &str) -> Cell<'static> {
    let text = Text::from(vec![
        Line::from(Span::styled(
            top.to_string(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            bottom.to_string(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
    ]);
    Cell::from(text)
}

fn draw_title(f: &mut Frame, source_label: &str, area: Rect) {
    let line = Line::from(vec![
        Span::styled(
            "  Dashboard",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ·  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            source_label.to_string(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn draw_tab_bar(
    f: &mut Frame,
    vm: &DashboardViewModel,
    area: Rect,
) -> Vec<crate::tui::app_state::Hit> {
    use crate::tui::app_state::Hit;

    let prefix = "  Window  ";
    let mut spans: Vec<Span<'static>> = Vec::new();
    spans.push(Span::styled(
        prefix,
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ));
    let mut hits: Vec<Hit> = Vec::with_capacity(vm.window_tabs.len());
    let mut col: u16 = area.x + prefix.chars().count() as u16;
    for (i, label) in vm.window_tabs.iter().enumerate() {
        let selected = i == vm.selected_window_index;
        let (open, close, style) = if selected {
            (
                "‹",
                "›",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            (" ", " ", Style::default().fg(Color::DarkGray))
        };
        let rendered = format!("{}{}{}", open, label, close);
        let width = rendered.chars().count() as u16;
        hits.push(Hit(Rect {
            x: col,
            y: area.y,
            width,
            height: 1,
        }));
        col = col.saturating_add(width);
        spans.push(Span::styled(rendered, style));
        if i + 1 < vm.window_tabs.len() {
            spans.push(Span::raw(" "));
            col = col.saturating_add(1);
        }
    }
    spans.push(Span::styled(
        "   (← → to switch)",
        Style::default().fg(Color::DarkGray),
    ));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
    hits
}

fn bar_cell(style: Style) -> Cell<'static> {
    Cell::from(Span::styled("│", style))
}

fn two_line_header_bar(style: Style) -> Cell<'static> {
    let text = Text::from(vec![
        Line::from(Span::styled("│", style)),
        Line::from(Span::styled("│", style)),
    ]);
    Cell::from(text)
}

fn two_line_header_sub(top: &str, bottom: &str, budget: usize) -> Cell<'static> {
    two_line_header(&truncate_for_header(top, budget), bottom)
}

fn model_header_with_note(model: &str, pricing_note: &str, budget: usize) -> Cell<'static> {
    let model_trunc = truncate_for_header(model, budget);
    let note_trunc = truncate_for_header(pricing_note, budget);
    let text = Text::from(vec![
        Line::from(Span::styled(
            model_trunc,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            note_trunc,
            Style::default().fg(Color::DarkGray),
        )),
    ]);
    Cell::from(text)
}

fn truncate_for_header(value: &str, budget: usize) -> String {
    if value.chars().count() > budget && budget > 0 {
        let take: String = value.chars().take(budget.saturating_sub(1)).collect();
        format!("{}…", take)
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::view_models::{
        DashboardViewModel, DayPivotRowVM, ModelBreakdownVM, ModelColumnVM,
    };
    use ratatui::{Terminal, backend::TestBackend};

    fn sample_vm(model_name: &str) -> DashboardViewModel {
        let cell = ModelBreakdownVM {
            input: "27".into(),
            output: "3.0K".into(),
            cache_read: "372.0K".into(),
            cache_write: "82.9K".into(),
            reasoning: "1.1K".into(),
            cost: "$0.78".into(),
        };
        DashboardViewModel {
            model_columns: vec![model_column(model_name)],
            rows: vec![DayPivotRowVM {
                date_label: "04-23".into(),
                model_cells: vec![cell.clone()],
                total: "450K".into(),
                total_cost: "$0.78".into(),
            }],
            column_totals: vec![cell],
            grand_total: "450K".into(),
            grand_cost: "$0.78".into(),
            window_tabs: vec!["1d".into(), "7d".into(), "30d".into()],
            selected_window_index: 2,
            empty: false,
            ..DashboardViewModel::default()
        }
    }

    fn model_column(model: &str) -> ModelColumnVM {
        ModelColumnVM {
            model: model.to_string(),
            pricing_note: "exact price".to_string(),
        }
    }

    fn render_to_string(vm: &DashboardViewModel, w: u16, h: u16) -> String {
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                draw(f, vm, f.area(), 0, 0, "OpenCode");
            })
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

    #[test]
    fn long_model_name_shows_fully_up_to_max_name_width() {
        let vm = sample_vm("claude-opus-4-7");
        let rendered = render_to_string(&vm, 120, 12);
        assert!(
            rendered.contains("claude-opus-4-7"),
            "expected full model name in rendered output; got:\n{}",
            rendered
        );
    }

    #[test]
    fn many_models_in_narrow_terminal_still_show_full_first_model_name() {
        // Simulate the real scenario: many Claude Code models, narrow terminal.
        let cell = ModelBreakdownVM {
            input: "16.3K".into(),
            output: "947.0K".into(),
            cache_read: "240.3K".into(),
            cache_write: "3.83M".into(),
            reasoning: "12.0K".into(),
            cost: "$167.86".into(),
        };
        let names = [
            "claude-opus-4-6",
            "claude-opus-4-7",
            "claude-haiku-4-5-20251001",
            "claude-sonnet-4-6",
            "glm-5-turbo",
            "glm-5.1",
            "gpt-4o",
            "sonnet",
            "haiku",
            "opus",
        ];
        let vm = DashboardViewModel {
            model_columns: names.iter().map(|s| model_column(s)).collect(),
            rows: vec![DayPivotRowVM {
                date_label: "04-23".into(),
                model_cells: vec![cell.clone(); names.len()],
                total: "9.31M".into(),
                total_cost: "$1376.87".into(),
            }],
            column_totals: vec![cell; names.len()],
            grand_total: "9.31M".into(),
            grand_cost: "$1376.87".into(),
            window_tabs: vec!["1d".into(), "7d".into(), "30d".into()],
            selected_window_index: 2,
            empty: false,
            ..DashboardViewModel::default()
        };

        // 160-col terminal: room for ~4 models of full width.
        let rendered = render_to_string(&vm, 160, 12);
        assert!(
            rendered.contains("claude-opus-4-6"),
            "first model name should render fully at 160 cols; got:\n{}",
            rendered
        );
    }

    #[test]
    fn model_header_shows_pricing_note() {
        let mut vm = sample_vm("openai/gpt-5.1-codex-latest");
        vm.model_columns = vec![ModelColumnVM {
            model: "openai/gpt-5.1-codex-latest".to_string(),
            pricing_note: "priced as gpt5.1-codex".to_string(),
        }];
        vm.column_totals = vec![ModelBreakdownVM::default()];
        for row in &mut vm.rows {
            row.model_cells = vec![ModelBreakdownVM::default()];
        }

        let rendered = render_to_string(&vm, 120, 12);

        assert!(rendered.contains("priced as") || rendered.contains("↦"));
        assert!(rendered.contains("gpt5.1-codex"));
    }

    #[test]
    fn model_group_header_includes_reasoning_column() {
        let vm = sample_vm("claude-opus-4-7");
        let rendered = render_to_string(&vm, 140, 12);
        assert!(
            rendered.contains("Rsn"),
            "expected Rsn sub-column header; got:\n{}",
            rendered
        );
        assert!(
            rendered.contains("1.1K"),
            "expected reasoning value in the model group; got:\n{}",
            rendered
        );
    }

    #[test]
    fn vertical_separator_drawn_between_model_groups() {
        // Two models: separator "│" should appear between their sub-col groups.
        let cell = ModelBreakdownVM {
            input: "10".into(),
            output: "20".into(),
            cache_read: "30".into(),
            cache_write: "40".into(),
            reasoning: "50".into(),
            cost: "$0.10".into(),
        };
        let vm = DashboardViewModel {
            model_columns: vec![model_column("alpha"), model_column("beta")],
            rows: vec![DayPivotRowVM {
                date_label: "04-23".into(),
                model_cells: vec![cell.clone(), cell.clone()],
                total: "200".into(),
                total_cost: "$0.20".into(),
            }],
            column_totals: vec![cell.clone(), cell],
            grand_total: "200".into(),
            grand_cost: "$0.20".into(),
            window_tabs: vec!["1d".into(), "7d".into(), "30d".into()],
            selected_window_index: 2,
            empty: false,
            ..DashboardViewModel::default()
        };
        let rendered = render_to_string(&vm, 120, 12);
        // Expect 3 separators on the body row: after Date, between alpha/beta,
        // between beta and Total. Count "│" occurrences excluding the outer borders.
        assert!(
            rendered.contains("│"),
            "expected vertical separator in output; got:\n{}",
            rendered
        );
        assert!(
            rendered.contains("┼"),
            "expected plus-glyph at separator intersection; got:\n{}",
            rendered
        );
        // Leading separator: the Date header cell is followed by a "│".
        assert!(
            rendered.contains("Date   │") || rendered.contains("Date  │"),
            "expected leading separator after Date header; got:\n{}",
            rendered
        );
    }

    #[test]
    fn very_long_name_is_truncated_to_max_name_width_with_ellipsis() {
        let vm = sample_vm("claude-haiku-4-5-20251001-preview");
        let rendered = render_to_string(&vm, 120, 12);
        // 24-char clamp → "claude-haiku-4-5-2025100…" or similar prefix.
        assert!(
            rendered.contains("claude-haiku-4-5-2025"),
            "expected truncated prefix; got:\n{}",
            rendered
        );
    }
}
