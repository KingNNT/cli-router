use ratatui::layout::Rect;

use crate::adapters::gateways::{DataSource, DataSourceCell};
use crate::adapters::view_models::{DashboardViewModel, PricingViewModel};

#[derive(Debug, Clone, Copy, Default)]
pub struct Hit(pub Rect);

impl Hit {
    pub fn contains(self, col: u16, row: u16) -> bool {
        let Rect {
            x,
            y,
            width,
            height,
        } = self.0;
        col >= x && col < x.saturating_add(width) && row >= y && row < y.saturating_add(height)
    }
}

#[derive(Debug, Default, Clone)]
pub struct HitRegions {
    pub sidebar_items: Vec<Hit>,
    pub window_tabs: Vec<Hit>,
    pub help_hint: Option<Hit>,
    pub content: Option<Hit>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    Sidebar,
    Content,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterWindow {
    Last1Day,
    Last7Days,
    Last30Days,
}

impl FilterWindow {
    pub const ALL: [FilterWindow; 3] = [
        FilterWindow::Last1Day,
        FilterWindow::Last7Days,
        FilterWindow::Last30Days,
    ];

    pub fn next(self) -> Self {
        match self {
            FilterWindow::Last1Day => FilterWindow::Last7Days,
            FilterWindow::Last7Days => FilterWindow::Last30Days,
            FilterWindow::Last30Days => FilterWindow::Last1Day,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            FilterWindow::Last1Day => FilterWindow::Last30Days,
            FilterWindow::Last7Days => FilterWindow::Last1Day,
            FilterWindow::Last30Days => FilterWindow::Last7Days,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FilterWindow::Last1Day => "1d",
            FilterWindow::Last7Days => "7d",
            FilterWindow::Last30Days => "30d",
        }
    }

    pub fn days(self) -> i64 {
        match self {
            FilterWindow::Last1Day => 1,
            FilterWindow::Last7Days => 7,
            FilterWindow::Last30Days => 30,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Dashboard,
    Pricing,
}

impl View {
    pub const ALL: [View; 2] = [View::Dashboard, View::Pricing];

    pub fn label(self) -> &'static str {
        match self {
            View::Dashboard => "Dashboard",
            View::Pricing => "Pricing",
        }
    }
}

pub struct AppState {
    pub view: View,
    pub sidebar_selected: usize,
    pub scroll_offset: u16,
    pub filter_window: FilterWindow,
    pub data_source: DataSourceCell,
    pub dashboard_vm: Option<DashboardViewModel>,
    pub pricing_vm: Option<PricingViewModel>,
    pub dashboard_offset: usize,
    pub dashboard_col_offset: usize,
    pub pricing_offset: usize,
    pub focus: Focus,
    pub status_message: Option<String>,
    pub should_quit: bool,
    pub pricing_query: String,
    pub is_searching: bool,
    pub help_open: bool,
    pub hit_regions: HitRegions,
}

impl AppState {
    pub fn new() -> Self {
        Self::with_data_source(DataSourceCell::new(DataSource::OpenCode))
    }

    pub fn with_data_source(data_source: DataSourceCell) -> Self {
        Self {
            view: View::Dashboard,
            sidebar_selected: 0,
            scroll_offset: 0,
            filter_window: FilterWindow::Last30Days,
            data_source,
            dashboard_vm: None,
            pricing_vm: None,
            dashboard_offset: 0,
            dashboard_col_offset: 0,
            pricing_offset: 0,
            focus: Focus::Sidebar,
            status_message: None,
            should_quit: false,
            pricing_query: String::new(),
            is_searching: false,
            help_open: false,
            hit_regions: HitRegions::default(),
        }
    }

    pub fn sidebar_up(&mut self) {
        if self.sidebar_selected > 0 {
            self.sidebar_selected -= 1;
            self.view = View::ALL[self.sidebar_selected];
            self.scroll_offset = 0;
            self.dashboard_offset = 0;
            self.dashboard_col_offset = 0;
            self.pricing_offset = 0;
            self.pricing_query = String::new();
            self.is_searching = false;
        }
    }

    pub fn sidebar_down(&mut self) {
        if self.sidebar_selected < View::ALL.len() - 1 {
            self.sidebar_selected += 1;
            self.view = View::ALL[self.sidebar_selected];
            self.scroll_offset = 0;
            self.dashboard_offset = 0;
            self.dashboard_col_offset = 0;
            self.pricing_offset = 0;
            self.pricing_query = String::new();
            self.is_searching = false;
        }
    }

    pub fn invalidate_all_vms(&mut self) {
        self.dashboard_vm = None;
        self.pricing_vm = None;
        self.dashboard_offset = 0;
        self.dashboard_col_offset = 0;
        self.pricing_offset = 0;
    }

    pub fn current_offset(&self) -> usize {
        match self.view {
            View::Dashboard => self.dashboard_offset,
            View::Pricing => self.pricing_offset,
        }
    }

    pub fn set_current_offset(&mut self, value: usize) {
        match self.view {
            View::Dashboard => self.dashboard_offset = value,
            View::Pricing => self.pricing_offset = value,
        }
    }

    pub fn clear_pricing_search(&mut self) {
        self.pricing_query.clear();
        self.is_searching = false;
        self.pricing_vm = None;
        self.pricing_offset = 0;
    }

    pub fn enter_content(&mut self) {
        self.focus = Focus::Content;
    }

    pub fn back_to_sidebar(&mut self) {
        self.focus = Focus::Sidebar;
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
