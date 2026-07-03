pub mod dashboard;
pub mod help;
pub mod layout;
pub mod pricing;
pub mod sidebar;
pub mod status_bar;

use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crate::tui::app_state::{AppState, Focus, Hit, HitRegions, View};

pub fn draw(f: &mut Frame, state: &mut AppState) {
    let areas = layout::compute(f.area());
    let sidebar_items = sidebar::draw(f, state.sidebar_selected, state.focus, areas.sidebar);
    let mut regions = HitRegions {
        sidebar_items,
        ..HitRegions::default()
    };
    draw_header(f, areas.header);

    let content_inner = match state.focus {
        Focus::Sidebar => areas.content,
        Focus::Content => {
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan));
            let inner = block.inner(areas.content);
            f.render_widget(block, areas.content);
            inner
        }
    };
    regions.content = Some(Hit(content_inner));
    dispatch_view(f, state, content_inner, &mut regions);

    let (msgs, cost) = match &state.dashboard_vm {
        Some(vm) => (vm.status_msgs.as_str(), vm.status_cost.as_str()),
        None => ("0", "$0.00"),
    };
    let extra = state.status_message.as_deref();
    let window_label = state.filter_window.label();
    regions.help_hint = Some(Hit(status_bar::draw(
        f,
        status_bar::StatusBar {
            msgs,
            cost,
            extra,
            window_label,
        },
        areas.status,
    )));

    if state.help_open {
        help::draw(f, f.area());
    }

    state.hit_regions = regions;
}

fn dispatch_view(f: &mut Frame, state: &AppState, area: Rect, regions: &mut HitRegions) {
    match state.view {
        View::Dashboard => {
            if let Some(vm) = &state.dashboard_vm {
                let tabs = dashboard::draw(
                    f,
                    vm,
                    area,
                    state.dashboard_offset,
                    state.dashboard_col_offset,
                    state.data_source.get().label(),
                );
                regions.window_tabs = tabs;
            }
        }
        View::Pricing => {
            if let Some(vm) = &state.pricing_vm {
                pricing::draw(f, vm, area, state.pricing_offset, state.is_searching);
            }
        }
    }
}

const APP_NAME: &str = "AI Usage Analyzer";

fn draw_header(f: &mut Frame, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let title = Paragraph::new(Line::from(Span::styled(
        APP_NAME,
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )))
    .alignment(Alignment::Center);
    f.render_widget(title, inner);
}
