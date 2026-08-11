#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelColumnVM {
    pub model: String,
    pub pricing_note: String,
}

#[derive(Debug, Clone, Default)]
pub struct DashboardViewModel {
    pub status_msgs: String,
    pub status_cost: String,
    pub pricing_note: Option<String>,
    pub banner: Option<String>,
    pub window_tabs: Vec<String>,
    pub selected_window_index: usize,
    pub model_columns: Vec<ModelColumnVM>,
    pub rows: Vec<DayPivotRowVM>,
    pub column_totals: Vec<ModelBreakdownVM>,
    pub grand_total: TotalBreakdownVM,
    pub grand_cost: String,
    pub empty: bool,
}

#[derive(Debug, Clone)]
pub struct DayPivotRowVM {
    pub date_label: String,
    pub model_cells: Vec<ModelBreakdownVM>,
    pub total: TotalBreakdownVM,
    pub total_cost: String,
}

#[derive(Debug, Clone, Default)]
pub struct ModelBreakdownVM {
    pub input: String,
    pub output: String,
    pub cache_read: String,
    pub cache_write: String,
    pub reasoning: String,
    pub cost: String,
}

/// Aggregate token counts across every model, in the order they are rendered:
/// raw input, input served from cache, raw output, output written to cache,
/// reasoning. No cost field — the dashboard renders `Cost` as its own column.
#[derive(Debug, Clone, Default)]
pub struct TotalBreakdownVM {
    pub input: String,
    pub cache_read: String,
    pub output: String,
    pub cache_write: String,
    pub reasoning: String,
}
