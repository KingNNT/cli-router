use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::adapters::presenters::{present_dashboard, present_pricing};
use crate::application::dto::{
    Filter, GetDashboardInput, GetPricingInput, SyncPricingInput,
};
use crate::application::use_cases::{GetDashboard, GetPricing, SyncPricing};
use crate::tui::app_state::FilterWindow;
use crate::tui::{AppState, Focus, View};
use shared::adapters::AdapterError;
use shared::application::ports::Clock;
use shared::domain::value_objects::DateRange;

pub struct TuiController {
    pub get_dashboard: Arc<GetDashboard>,
    pub get_pricing: Arc<GetPricing>,
    pub sync_pricing: Arc<SyncPricing>,
    clock: Arc<dyn Clock>,
}

impl TuiController {
    pub fn new(
        get_dashboard: Arc<GetDashboard>,
        get_pricing: Arc<GetPricing>,
        sync_pricing: Arc<SyncPricing>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            get_dashboard,
            get_pricing,
            sync_pricing,
            clock,
        }
    }

    fn filter_from_window(&self, window: FilterWindow) -> Filter {
        let date_range = Some(DateRange::last_n_days(self.clock.today(), window.days()));
        Filter {
            date_range,
            ..Filter::default()
        }
    }

    fn set_window(&self, state: &mut AppState, window: FilterWindow) -> Result<(), AdapterError> {
        state.filter_window = window;
        state.status_message = Some(format!("Window: {}", window.label()));
        state.invalidate_all_vms();
        state.pricing_query = String::new();
        self.ensure_vm_for_current_view(state)
    }

    pub fn warmup(&self, state: &mut AppState) -> Result<(), AdapterError> {
        self.ensure_vm_for_current_view(state)
    }

    pub fn handle(&self, key: KeyEvent, state: &mut AppState) -> Result<(), AdapterError> {
        // Help popup intercepts everything: any key dismisses it.
        if state.help_open {
            state.help_open = false;
            return Ok(());
        }

        // Global keys are suppressed while the search input is active.
        if !state.is_searching {
            match key.code {
                KeyCode::Char('?') => {
                    state.help_open = true;
                    return Ok(());
                }
                KeyCode::Char('q') => {
                    state.should_quit = true;
                    return Ok(());
                }
                KeyCode::Char('s') => {
                    self.handle_sync(state)?;
                    return Ok(());
                }
                KeyCode::Char('d') => {
                    state.filter_window = state.filter_window.next();
                    state.status_message = Some(format!("Window: {}", state.filter_window.label()));
                    state.invalidate_all_vms();
                    // 'd' resets offsets — also clear any pricing search.
                    state.pricing_query = String::new();
                    self.ensure_vm_for_current_view(state)?;
                    return Ok(());
                }
                KeyCode::Char('1') => {
                    self.set_window(state, FilterWindow::Last1Day)?;
                    return Ok(());
                }
                KeyCode::Char('7') => {
                    self.set_window(state, FilterWindow::Last7Days)?;
                    return Ok(());
                }
                KeyCode::Char('3') => {
                    self.set_window(state, FilterWindow::Last30Days)?;
                    return Ok(());
                }
                KeyCode::Char('t') => {
                    let next = state.data_source.get().toggle();
                    state.data_source.set(next);
                    state.status_message = Some(format!("Source: {}", next.label()));
                    state.invalidate_all_vms();
                    state.pricing_query = String::new();
                    self.ensure_vm_for_current_view(state)?;
                    return Ok(());
                }
                _ => {}
            }
        }

        match state.focus {
            Focus::Sidebar => self.handle_sidebar(key, state)?,
            Focus::Content => self.handle_content(key, state)?,
        }
        Ok(())
    }

    pub fn handle_mouse(
        &self,
        event: MouseEvent,
        state: &mut AppState,
    ) -> Result<(), AdapterError> {
        // Any click while help is open closes it.
        if state.help_open {
            if matches!(event.kind, MouseEventKind::Down(_)) {
                state.help_open = false;
            }
            return Ok(());
        }

        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let col = event.column;
                let row = event.row;

                // Sidebar item click → select view + enter content focus.
                for (i, hit) in state.hit_regions.sidebar_items.clone().iter().enumerate() {
                    if hit.contains(col, row) {
                        state.sidebar_selected = i;
                        state.view = crate::tui::app_state::View::ALL[i];
                        state.focus = crate::tui::app_state::Focus::Content;
                        state.dashboard_offset = 0;
                        state.dashboard_col_offset = 0;
                        state.pricing_offset = 0;
                        state.pricing_query = String::new();
                        state.is_searching = false;
                        self.ensure_vm_for_current_view(state)?;
                        return Ok(());
                    }
                }

                // Window tab click → set that window.
                if state.view == View::Dashboard {
                    for (i, hit) in state.hit_regions.window_tabs.clone().iter().enumerate() {
                        if hit.contains(col, row) {
                            if let Some(target) = FilterWindow::ALL.get(i).copied() {
                                self.set_window(state, target)?;
                            }
                            return Ok(());
                        }
                    }
                }

                // "?" help hint → toggle help.
                if let Some(hit) = state.hit_regions.help_hint
                    && hit.contains(col, row)
                {
                    state.help_open = true;
                    return Ok(());
                }

                // Clicking inside the content area enters Content focus.
                if let Some(hit) = state.hit_regions.content
                    && hit.contains(col, row)
                    && state.focus == crate::tui::app_state::Focus::Sidebar
                {
                    state.enter_content();
                }
            }
            MouseEventKind::ScrollDown => {
                if let Some(hit) = state.hit_regions.content
                    && hit.contains(event.column, event.row)
                {
                    let o = state.current_offset();
                    state.set_current_offset(o.saturating_add(3));
                }
            }
            MouseEventKind::ScrollUp => {
                if let Some(hit) = state.hit_regions.content
                    && hit.contains(event.column, event.row)
                {
                    let o = state.current_offset();
                    state.set_current_offset(o.saturating_sub(3));
                }
            }
            MouseEventKind::ScrollRight if state.view == View::Dashboard => {
                let max = state
                    .dashboard_vm
                    .as_ref()
                    .map(|vm| vm.model_columns.len().saturating_sub(1))
                    .unwrap_or(0);
                state.dashboard_col_offset = state.dashboard_col_offset.saturating_add(1).min(max);
            }
            MouseEventKind::ScrollLeft if state.view == View::Dashboard => {
                state.dashboard_col_offset = state.dashboard_col_offset.saturating_sub(1);
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_sidebar(&self, key: KeyEvent, state: &mut AppState) -> Result<(), AdapterError> {
        match key.code {
            KeyCode::Esc => state.should_quit = true,
            KeyCode::Up | KeyCode::Char('k') => {
                state.sidebar_up();
                self.ensure_vm_for_current_view(state)?;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                state.sidebar_down();
                self.ensure_vm_for_current_view(state)?;
            }
            KeyCode::Enter
            | KeyCode::Char(' ')
            | KeyCode::Right
            | KeyCode::Char('l')
            | KeyCode::Tab => {
                self.ensure_vm_for_current_view(state)?;
                state.enter_content();
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_content(&self, key: KeyEvent, state: &mut AppState) -> Result<(), AdapterError> {
        // Pricing-view search mode has its own input loop.
        if state.view == View::Pricing && state.is_searching {
            return self.handle_pricing_search_input(key, state);
        }

        // Dashboard: Shift+Left/Right scroll model columns horizontally;
        // plain Left/Right arrows cycle the time window.
        if state.view == View::Dashboard {
            let shift = key.modifiers.contains(KeyModifiers::SHIFT);
            if shift && key.code == KeyCode::Left {
                state.dashboard_col_offset = state.dashboard_col_offset.saturating_sub(1);
                return Ok(());
            }
            if shift && key.code == KeyCode::Right {
                let max = state
                    .dashboard_vm
                    .as_ref()
                    .map(|vm| vm.model_columns.len().saturating_sub(1))
                    .unwrap_or(0);
                state.dashboard_col_offset = state.dashboard_col_offset.saturating_add(1).min(max);
                return Ok(());
            }
            if key.code == KeyCode::Left {
                return self.set_window(state, state.filter_window.prev());
            }
            if key.code == KeyCode::Right {
                return self.set_window(state, state.filter_window.next());
            }
        }

        match key.code {
            KeyCode::Esc
            | KeyCode::Backspace
            | KeyCode::Left
            | KeyCode::Char('h')
            | KeyCode::BackTab => {
                if state.view == View::Pricing
                    && !state.pricing_query.is_empty()
                    && matches!(key.code, KeyCode::Esc)
                {
                    state.clear_pricing_search();
                    self.ensure_vm_for_current_view(state)?;
                } else {
                    state.back_to_sidebar();
                }
            }
            // Start search mode — Pricing view only.
            KeyCode::Char('/') if state.view == View::Pricing => {
                state.pricing_query.clear();
                state.is_searching = true;
                state.pricing_vm = None;
                state.pricing_offset = 0;
                self.ensure_vm_for_current_view(state)?;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let o = state.current_offset();
                state.set_current_offset(o.saturating_sub(1));
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let o = state.current_offset();
                state.set_current_offset(o.saturating_add(1));
            }
            KeyCode::PageUp => {
                let o = state.current_offset();
                state.set_current_offset(o.saturating_sub(20));
            }
            KeyCode::PageDown => {
                let o = state.current_offset();
                state.set_current_offset(o.saturating_add(20));
            }
            KeyCode::Home => state.set_current_offset(0),
            KeyCode::End => state.set_current_offset(usize::MAX),
            _ => {}
        }
        Ok(())
    }

    fn handle_pricing_search_input(
        &self,
        key: KeyEvent,
        state: &mut AppState,
    ) -> Result<(), AdapterError> {
        match key.code {
            KeyCode::Enter => {
                state.is_searching = false;
                // Query stays; VM was already rebuilt on the last keystroke.
            }
            KeyCode::Esc => {
                state.clear_pricing_search();
                self.ensure_vm_for_current_view(state)?;
            }
            KeyCode::Backspace => {
                state.pricing_query.pop();
                state.pricing_vm = None;
                state.pricing_offset = 0;
                self.ensure_vm_for_current_view(state)?;
            }
            KeyCode::Char(c) if !c.is_control() => {
                state.pricing_query.push(c);
                state.pricing_vm = None;
                state.pricing_offset = 0;
                self.ensure_vm_for_current_view(state)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_sync(&self, state: &mut AppState) -> Result<(), AdapterError> {
        state.status_message = Some("Syncing…".to_string());
        match self.sync_pricing.execute(SyncPricingInput) {
            Ok(out) => {
                state.status_message = Some(format!(
                    "Synced {} models @ {}",
                    out.synced_count, out.last_synced_at
                ));
                state.invalidate_all_vms();
                self.ensure_vm_for_current_view(state)?;
            }
            Err(e) => {
                state.status_message = Some(format!("Sync failed: {}", e));
            }
        }
        Ok(())
    }

    fn ensure_vm_for_current_view(&self, state: &mut AppState) -> Result<(), AdapterError> {
        let filter = self.filter_from_window(state.filter_window);
        match state.view {
            View::Dashboard if state.dashboard_vm.is_none() => {
                let out = self
                    .get_dashboard
                    .execute(GetDashboardInput {
                        filter: Some(filter),
                    })
                    .map_err(|e| AdapterError::DataMapping(e.to_string()))?;
                let labels: Vec<&str> = FilterWindow::ALL.iter().map(|w| w.label()).collect();
                let idx = FilterWindow::ALL
                    .iter()
                    .position(|w| *w == state.filter_window)
                    .unwrap_or(0);
                state.dashboard_vm = Some(present_dashboard(&out, &labels, idx));
            }
            View::Pricing if state.pricing_vm.is_none() => {
                let out = self
                    .get_pricing
                    .execute(GetPricingInput)
                    .map_err(|e| AdapterError::DataMapping(e.to_string()))?;
                state.pricing_vm = Some(present_pricing(&out, &state.pricing_query));
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::{PricingSource, UsageRepository};
    use crate::application::test_support::{FakePricingSource, FakeUsageRepository};
    use chrono::NaiveDate;
    use crossterm::event::{KeyEvent, KeyModifiers};
    use shared::application::ports::{Clock, PricingRepository};
    use shared::application::test_support::{FakePricingRepository, FixedClock};
    use shared::domain::entities::ModelPricing;
    use shared::domain::value_objects::{ModelId, PricePerToken};

    fn ctl_with_source_rows(
        source_rows: Vec<ModelPricing>,
    ) -> (TuiController, Arc<FakePricingRepository>) {
        let usage = Arc::new(FakeUsageRepository::default());
        let pricing_repo = Arc::new(FakePricingRepository::default());
        let source = Arc::new(FakePricingSource {
            rows: source_rows,
            err: None,
        });
        let clock: Arc<dyn Clock> = Arc::new(FixedClock::new(2026, 4, 23));
        let usage_dyn: Arc<dyn UsageRepository> = usage;
        let pricing_repo_dyn: Arc<dyn PricingRepository> = pricing_repo.clone();
        let source_dyn: Arc<dyn PricingSource> = source;
        let gd = Arc::new(GetDashboard::new(
            usage_dyn.clone(),
            pricing_repo_dyn.clone(),
            clock.clone(),
        ));
        let get_pricing = Arc::new(GetPricing::new(pricing_repo_dyn.clone()));
        let controller_clock = clock.clone();
        let sync = Arc::new(SyncPricing::new(source_dyn, pricing_repo_dyn, clock));
        (
            TuiController::new(gd, get_pricing, sync, controller_clock),
            pricing_repo,
        )
    }

    fn pricing_row(key: &str) -> ModelPricing {
        ModelPricing {
            lookup_key: key.into(),
            model: ModelId::new(key).unwrap(),
            provider_id: "p".into(),
            input_rate: PricePerToken::new(0.00001).unwrap(),
            output_rate: PricePerToken::new(0.00003).unwrap(),
            cache_read_rate: None,
            cache_write_rate: None,
            last_synced: NaiveDate::from_ymd_opt(2026, 4, 23).unwrap(),
        }
    }

    #[test]
    fn warmup_populates_dashboard_vm_only() {
        let (controller, _) = ctl_with_source_rows(vec![]);
        let mut state = AppState::new();
        controller.warmup(&mut state).unwrap();
        assert!(state.dashboard_vm.is_some());
        assert!(state.pricing_vm.is_none());
    }

    #[test]
    fn pressing_s_syncs_and_rebuilds_current_view_vm() {
        let (controller, repo) = ctl_with_source_rows(vec![pricing_row("m1"), pricing_row("m2")]);
        let mut state = AppState::new();
        controller.warmup(&mut state).unwrap();
        assert!(state.dashboard_vm.is_some());
        let key = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE);
        controller.handle(key, &mut state).unwrap();
        assert_eq!(repo.rows.lock().unwrap().len(), 2);
        // Current view (Dashboard) is rebuilt against the freshly-synced pricing;
        // lazy view (Pricing) remains invalidated until visited.
        assert!(state.dashboard_vm.is_some());
        assert!(state.pricing_vm.is_none());
        assert!(
            state
                .status_message
                .as_deref()
                .unwrap()
                .starts_with("Synced 2 models")
        );
    }

    #[test]
    fn navigating_to_pricing_loads_pricing_vm() {
        let (controller, repo) = ctl_with_source_rows(vec![]);
        repo.upsert_many(&[pricing_row("opus")]).unwrap();
        let mut state = AppState::new();
        state.sidebar_selected = 1;
        state.view = View::Pricing;
        controller.warmup(&mut state).unwrap();
        let vm = state.pricing_vm.as_ref().unwrap();
        assert_eq!(vm.rows.len(), 1);
        assert!(!vm.empty);
    }

    #[test]
    fn pressing_q_sets_should_quit() {
        let (controller, _) = ctl_with_source_rows(vec![]);
        let mut state = AppState::new();
        let key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        controller.handle(key, &mut state).unwrap();
        assert!(state.should_quit);
    }

    #[test]
    fn pgdn_advances_pricing_offset_when_content_focused() {
        let (controller, _) = ctl_with_source_rows(vec![]);
        let mut state = AppState::new();
        state.sidebar_selected = 1;
        state.view = View::Pricing;
        state.focus = Focus::Content;
        assert_eq!(state.pricing_offset, 0);
        let key = KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE);
        controller.handle(key, &mut state).unwrap();
        assert_eq!(state.pricing_offset, 20);
    }

    #[test]
    fn pgdn_does_nothing_when_sidebar_focused() {
        let (controller, _) = ctl_with_source_rows(vec![]);
        let mut state = AppState::new();
        state.sidebar_selected = 1;
        state.view = View::Pricing;
        // Default focus is Sidebar
        assert_eq!(state.focus, Focus::Sidebar);
        assert_eq!(state.pricing_offset, 0);
        let key = KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE);
        controller.handle(key, &mut state).unwrap();
        assert_eq!(state.pricing_offset, 0);
    }

    #[test]
    fn pgup_decreases_offset_saturating_at_zero() {
        let (controller, _) = ctl_with_source_rows(vec![]);
        let mut state = AppState::new();
        state.sidebar_selected = 1;
        state.view = View::Pricing;
        state.focus = Focus::Content;
        state.pricing_offset = 10;
        let key = KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE);
        controller.handle(key, &mut state).unwrap();
        assert_eq!(state.pricing_offset, 0);

        // Already at 0 — saturating_sub should keep it at 0
        controller.handle(key, &mut state).unwrap();
        assert_eq!(state.pricing_offset, 0);
    }

    #[test]
    fn home_resets_offset_to_zero() {
        let (controller, _) = ctl_with_source_rows(vec![]);
        let mut state = AppState::new();
        state.sidebar_selected = 1;
        state.view = View::Pricing;
        state.focus = Focus::Content;
        state.pricing_offset = 100;
        let key = KeyEvent::new(KeyCode::Home, KeyModifiers::NONE);
        controller.handle(key, &mut state).unwrap();
        assert_eq!(state.pricing_offset, 0);
    }

    #[test]
    fn end_sets_offset_to_usize_max() {
        let (controller, _) = ctl_with_source_rows(vec![]);
        let mut state = AppState::new();
        state.sidebar_selected = 1;
        state.view = View::Pricing;
        state.focus = Focus::Content;
        let key = KeyEvent::new(KeyCode::End, KeyModifiers::NONE);
        controller.handle(key, &mut state).unwrap();
        assert_eq!(state.pricing_offset, usize::MAX);
    }

    #[test]
    fn sidebar_navigation_resets_all_offsets() {
        let (controller, _) = ctl_with_source_rows(vec![]);
        let mut state = AppState::new();
        // Navigate to Pricing (index 1)
        state.sidebar_selected = 1;
        state.view = View::Pricing;
        state.pricing_offset = 50;
        state.dashboard_offset = 10;

        // sidebar_up resets offsets
        let key_up = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        controller.handle(key_up, &mut state).unwrap();
        assert_eq!(state.pricing_offset, 0);
        assert_eq!(state.dashboard_offset, 0);

        // Set offsets again, then sidebar_down resets them
        state.pricing_offset = 50;
        let key_down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        controller.handle(key_down, &mut state).unwrap();
        assert_eq!(state.pricing_offset, 0);
    }

    // ── Focus model tests ───────────────────────────────────────────────────

    #[test]
    fn default_focus_is_sidebar() {
        let state = AppState::new();
        assert_eq!(state.focus, Focus::Sidebar);
    }

    #[test]
    fn enter_transitions_from_sidebar_to_content() {
        let (controller, _) = ctl_with_source_rows(vec![]);
        let mut state = AppState::new();
        assert_eq!(state.focus, Focus::Sidebar);
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        controller.handle(key, &mut state).unwrap();
        assert_eq!(state.focus, Focus::Content);
    }

    #[test]
    fn space_right_l_tab_also_enter_content() {
        let (controller, _) = ctl_with_source_rows(vec![]);
        for keycode in [
            KeyCode::Char(' '),
            KeyCode::Right,
            KeyCode::Char('l'),
            KeyCode::Tab,
        ] {
            let mut state = AppState::new();
            let key = KeyEvent::new(keycode, KeyModifiers::NONE);
            controller.handle(key, &mut state).unwrap();
            assert_eq!(
                state.focus,
                Focus::Content,
                "{keycode:?} should enter Content"
            );
        }
    }

    #[test]
    fn esc_backspace_left_h_backtab_return_to_sidebar() {
        let (controller, _) = ctl_with_source_rows(vec![]);
        // Note: on Dashboard, Left is reserved for window-switching; it returns to
        // sidebar only on other views. Esc/Backspace/h/BackTab always go back.
        let cases: &[(KeyCode, View)] = &[
            (KeyCode::Esc, View::Dashboard),
            (KeyCode::Backspace, View::Dashboard),
            (KeyCode::Char('h'), View::Dashboard),
            (KeyCode::BackTab, View::Dashboard),
        ];
        for (keycode, view) in cases {
            let mut state = AppState::new();
            state.view = *view;
            state.sidebar_selected = match view {
                View::Dashboard => 0,
                View::Pricing => 1,
            };
            state.focus = Focus::Content;
            let key = KeyEvent::new(*keycode, KeyModifiers::NONE);
            controller.handle(key, &mut state).unwrap();
            assert_eq!(
                state.focus,
                Focus::Sidebar,
                "{keycode:?} should return to Sidebar"
            );
            // should_quit must remain false (Esc from Content goes back, not quit)
            assert!(!state.should_quit, "{keycode:?} should not quit");
        }
    }

    #[test]
    fn esc_from_sidebar_quits() {
        let (controller, _) = ctl_with_source_rows(vec![]);
        let mut state = AppState::new();
        assert_eq!(state.focus, Focus::Sidebar);
        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        controller.handle(key, &mut state).unwrap();
        assert!(state.should_quit);
    }

    #[test]
    fn jk_in_sidebar_navigates_not_scrolls() {
        let (controller, _) = ctl_with_source_rows(vec![]);
        let mut state = AppState::new();
        assert_eq!(state.focus, Focus::Sidebar);
        assert_eq!(state.sidebar_selected, 0);

        let key_j = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
        controller.handle(key_j, &mut state).unwrap();
        assert_eq!(state.sidebar_selected, 1);
        assert_eq!(state.dashboard_offset, 0); // no scroll

        let key_k = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE);
        controller.handle(key_k, &mut state).unwrap();
        assert_eq!(state.sidebar_selected, 0);
    }

    #[test]
    fn jk_in_content_scrolls_active_view() {
        let (controller, _) = ctl_with_source_rows(vec![]);
        let mut state = AppState::new();
        state.focus = Focus::Content;
        assert_eq!(state.view, View::Dashboard);
        assert_eq!(state.dashboard_offset, 0);

        let key_j = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
        controller.handle(key_j, &mut state).unwrap();
        assert_eq!(state.dashboard_offset, 1);
        assert_eq!(state.pricing_offset, 0); // only dashboard advances

        let key_k = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE);
        controller.handle(key_k, &mut state).unwrap();
        assert_eq!(state.dashboard_offset, 0);
    }

    #[test]
    fn q_quits_regardless_of_focus() {
        let (controller, _) = ctl_with_source_rows(vec![]);
        for focus in [Focus::Sidebar, Focus::Content] {
            let mut state = AppState::new();
            state.focus = focus;
            let key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
            controller.handle(key, &mut state).unwrap();
            assert!(state.should_quit, "q should quit from {focus:?}");
        }
    }

    #[test]
    fn s_syncs_regardless_of_focus() {
        for focus in [Focus::Sidebar, Focus::Content] {
            let (controller, repo) =
                ctl_with_source_rows(vec![pricing_row("m1"), pricing_row("m2")]);
            let mut state = AppState::new();
            state.focus = focus;
            controller.warmup(&mut state).unwrap();
            let key = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE);
            controller.handle(key, &mut state).unwrap();
            assert_eq!(
                repo.rows.lock().unwrap().len(),
                2,
                "s should sync from {focus:?}"
            );
        }
    }

    #[test]
    fn d_cycles_window_regardless_of_focus() {
        use crate::tui::app_state::FilterWindow;
        for focus in [Focus::Sidebar, Focus::Content] {
            let (controller, _) = ctl_with_source_rows(vec![]);
            let mut state = AppState::new();
            state.focus = focus;
            assert_eq!(state.filter_window, FilterWindow::Last30Days);
            let key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
            controller.handle(key, &mut state).unwrap();
            // Cycle: 30d → 1d (wraps since 30d is the last position)
            assert_eq!(
                state.filter_window,
                FilterWindow::Last1Day,
                "d should cycle window from {focus:?}"
            );
            assert_eq!(state.dashboard_offset, 0);
            assert_eq!(state.pricing_offset, 0);
        }
    }

    #[test]
    fn pressing_d_cycles_window_and_invalidates_vms() {
        use crate::tui::app_state::FilterWindow;

        let (controller, _) = ctl_with_source_rows(vec![]);
        let mut state = AppState::new();
        controller.warmup(&mut state).unwrap();
        assert_eq!(state.filter_window, FilterWindow::Last30Days);
        assert!(state.dashboard_vm.is_some());

        // 30d → 1d → 7d → 30d (three steps to wrap)
        for (label, expected) in [
            ("1d", FilterWindow::Last1Day),
            ("7d", FilterWindow::Last7Days),
            ("30d", FilterWindow::Last30Days),
        ] {
            let key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
            controller.handle(key, &mut state).unwrap();
            assert_eq!(state.filter_window, expected);
            assert_eq!(
                state.status_message.as_deref(),
                Some(format!("Window: {}", label).as_str())
            );
            assert!(state.dashboard_vm.is_some());
        }
    }

    #[test]
    fn shift_arrows_scroll_dashboard_columns() {
        use crate::adapters::view_models::{
            DashboardViewModel, DayPivotRowVM, ModelBreakdownVM, ModelColumnVM,
        };
        let (controller, _) = ctl_with_source_rows(vec![]);
        let mut state = AppState::new();
        state.view = View::Dashboard;
        state.focus = Focus::Content;
        // Seed a view model with 3 models so scrolling is bounded.
        state.dashboard_vm = Some(DashboardViewModel {
            model_columns: vec![
                ModelColumnVM {
                    model: "a".into(),
                    pricing_note: "exact price".into(),
                },
                ModelColumnVM {
                    model: "b".into(),
                    pricing_note: "exact price".into(),
                },
                ModelColumnVM {
                    model: "c".into(),
                    pricing_note: "exact price".into(),
                },
            ],
            rows: vec![DayPivotRowVM {
                date_label: "04-23".into(),
                model_cells: vec![ModelBreakdownVM::default(); 3],
                total: "0".into(),
                total_cost: "$0.00".into(),
            }],
            column_totals: vec![ModelBreakdownVM::default(); 3],
            grand_total: "0".into(),
            grand_cost: "$0.00".into(),
            ..DashboardViewModel::default()
        });
        assert_eq!(state.dashboard_col_offset, 0);

        let right_shift = KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT);
        controller.handle(right_shift, &mut state).unwrap();
        assert_eq!(state.dashboard_col_offset, 1);

        // Bounded: with 3 models, max offset is 2 (index of last model).
        controller.handle(right_shift, &mut state).unwrap();
        controller.handle(right_shift, &mut state).unwrap();
        controller.handle(right_shift, &mut state).unwrap();
        assert_eq!(state.dashboard_col_offset, 2);

        let left_shift = KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT);
        controller.handle(left_shift, &mut state).unwrap();
        assert_eq!(state.dashboard_col_offset, 1);

        // Plain Left cycles the window, which invalidates VMs and resets the
        // column offset back to 0 (window change → fresh dataset).
        let left_plain = KeyEvent::new(KeyCode::Left, KeyModifiers::NONE);
        controller.handle(left_plain, &mut state).unwrap();
        assert_eq!(state.dashboard_col_offset, 0);
    }

    #[test]
    fn mouse_click_on_sidebar_item_selects_view_and_enters_content() {
        use crate::tui::app_state::Hit;
        use ratatui::layout::Rect;
        let (controller, _) = ctl_with_source_rows(vec![]);
        let mut state = AppState::new();
        // Simulate a previously-rendered sidebar: 2 rows starting at (0, 0).
        state.hit_regions.sidebar_items = (0..2)
            .map(|i| {
                Hit(Rect {
                    x: 0,
                    y: i as u16,
                    width: 16,
                    height: 1,
                })
            })
            .collect();

        // Click on row 1 → Pricing.
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 5,
            row: 1,
            modifiers: KeyModifiers::NONE,
        };
        controller.handle_mouse(click, &mut state).unwrap();
        assert_eq!(state.sidebar_selected, 1);
        assert_eq!(state.view, View::Pricing);
        assert_eq!(state.focus, Focus::Content);
    }

    #[test]
    fn mouse_wheel_scrolls_content() {
        use crate::tui::app_state::Hit;
        use ratatui::layout::Rect;
        let (controller, _) = ctl_with_source_rows(vec![]);
        let mut state = AppState::new();
        state.view = View::Dashboard;
        state.focus = Focus::Content;
        state.hit_regions.content = Some(Hit(Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 40,
        }));

        let scroll_down = MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 50,
            row: 20,
            modifiers: KeyModifiers::NONE,
        };
        controller.handle_mouse(scroll_down, &mut state).unwrap();
        assert_eq!(state.dashboard_offset, 3);

        let scroll_up = MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 50,
            row: 20,
            modifiers: KeyModifiers::NONE,
        };
        controller.handle_mouse(scroll_up, &mut state).unwrap();
        assert_eq!(state.dashboard_offset, 0);
    }

    #[test]
    fn question_mark_toggles_help_popup() {
        let (controller, _) = ctl_with_source_rows(vec![]);
        let mut state = AppState::new();
        assert!(!state.help_open);

        let q = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE);
        controller.handle(q, &mut state).unwrap();
        assert!(state.help_open);
        assert!(!state.should_quit);

        // Any key closes help; should not quit or change window.
        let other = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        controller.handle(other, &mut state).unwrap();
        assert!(!state.help_open);
        assert!(!state.should_quit);
    }

    #[test]
    fn t_key_toggles_data_source_and_invalidates_vms() {
        use crate::adapters::gateways::DataSource;
        let (controller, _) = ctl_with_source_rows(vec![]);
        let mut state = AppState::new();
        controller.warmup(&mut state).unwrap();
        assert_eq!(state.data_source.get(), DataSource::OpenCode);

        let t = KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE);
        controller.handle(t, &mut state).unwrap();
        assert_eq!(state.data_source.get(), DataSource::ClaudeCode);
        assert_eq!(state.status_message.as_deref(), Some("Source: Claude Code"));
        assert!(state.dashboard_vm.is_some()); // re-populated

        controller
            .handle(
                KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE),
                &mut state,
            )
            .unwrap();
        assert_eq!(state.data_source.get(), DataSource::OpenCode);
    }

    #[test]
    fn arrow_keys_switch_window_on_dashboard_content() {
        use crate::tui::app_state::FilterWindow;
        let (controller, _) = ctl_with_source_rows(vec![]);
        let mut state = AppState::new();
        state.focus = Focus::Content;
        state.view = View::Dashboard;
        controller.warmup(&mut state).unwrap();
        assert_eq!(state.filter_window, FilterWindow::Last30Days);

        // Right from 30d wraps to 1d (30d is the last position in the cycle).
        let right = KeyEvent::new(KeyCode::Right, KeyModifiers::NONE);
        controller.handle(right, &mut state).unwrap();
        assert_eq!(state.filter_window, FilterWindow::Last1Day);
        assert!(state.dashboard_vm.is_some());

        // Left → prev (wraps back to 30d)
        let left = KeyEvent::new(KeyCode::Left, KeyModifiers::NONE);
        controller.handle(left, &mut state).unwrap();
        assert_eq!(state.filter_window, FilterWindow::Last30Days);

        // Left again → prev (7d)
        controller
            .handle(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE), &mut state)
            .unwrap();
        assert_eq!(state.filter_window, FilterWindow::Last7Days);
    }

    #[test]
    fn direct_keys_select_specific_windows() {
        use crate::tui::app_state::FilterWindow;
        let (controller, _) = ctl_with_source_rows(vec![]);
        let mut state = AppState::new();
        controller.warmup(&mut state).unwrap();
        assert_eq!(state.filter_window, FilterWindow::Last30Days);

        for (key_char, expected, label) in [
            ('1', FilterWindow::Last1Day, "1d"),
            ('7', FilterWindow::Last7Days, "7d"),
            ('3', FilterWindow::Last30Days, "30d"),
        ] {
            let k = KeyEvent::new(KeyCode::Char(key_char), KeyModifiers::NONE);
            controller.handle(k, &mut state).unwrap();
            assert_eq!(state.filter_window, expected);
            assert_eq!(
                state.status_message.as_deref(),
                Some(format!("Window: {}", label).as_str())
            );
            assert!(state.dashboard_vm.is_some());
            assert_eq!(state.dashboard_offset, 0);
        }
    }

    #[test]
    fn left_from_pricing_content_returns_to_sidebar() {
        let (controller, _) = ctl_with_source_rows(vec![]);
        let mut state = AppState::new();
        state.sidebar_selected = 1;
        state.view = View::Pricing;
        state.focus = Focus::Content;
        let key = KeyEvent::new(KeyCode::Left, KeyModifiers::NONE);
        controller.handle(key, &mut state).unwrap();
        assert_eq!(state.focus, Focus::Sidebar);
    }

    // ── Pricing search tests ──────────────────────────────────────────────────

    fn pricing_state_content() -> AppState {
        let mut state = AppState::new();
        state.sidebar_selected = 1;
        state.view = View::Pricing;
        state.focus = Focus::Content;
        state
    }

    #[test]
    fn slash_on_pricing_sets_is_searching() {
        let (controller, _) = ctl_with_source_rows(vec![]);
        let mut state = pricing_state_content();
        let key = KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE);
        controller.handle(key, &mut state).unwrap();
        assert!(state.is_searching);
        assert!(state.pricing_query.is_empty());
    }

    #[test]
    fn slash_on_non_pricing_view_does_nothing_special() {
        let (controller, _) = ctl_with_source_rows(vec![]);
        let mut state = AppState::new();
        state.focus = Focus::Content;
        // Default view is Dashboard
        assert_eq!(state.view, View::Dashboard);
        let key = KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE);
        controller.handle(key, &mut state).unwrap();
        assert!(!state.is_searching);
        // Focus should still be content (/ is not a back key for dashboard)
        assert_eq!(state.focus, Focus::Content);
    }

    #[test]
    fn typing_chars_while_searching_appends_to_query_and_clears_vm() {
        let (controller, repo) = ctl_with_source_rows(vec![]);
        repo.upsert_many(&[pricing_row("opus")]).unwrap();
        let mut state = pricing_state_content();
        state.is_searching = true;

        let key_o = KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE);
        controller.handle(key_o, &mut state).unwrap();
        assert_eq!(state.pricing_query, "o");
        // VM gets rebuilt immediately via ensure_vm
        assert!(state.pricing_vm.is_some());

        let key_p = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE);
        controller.handle(key_p, &mut state).unwrap();
        assert_eq!(state.pricing_query, "op");
    }

    #[test]
    fn backspace_while_searching_removes_last_char() {
        let (controller, _) = ctl_with_source_rows(vec![]);
        let mut state = pricing_state_content();
        state.is_searching = true;
        state.pricing_query = "op".to_string();

        let key = KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE);
        controller.handle(key, &mut state).unwrap();
        assert_eq!(state.pricing_query, "o");
        assert_eq!(state.pricing_offset, 0);
    }

    #[test]
    fn enter_while_searching_commits_and_keeps_query() {
        let (controller, _) = ctl_with_source_rows(vec![]);
        let mut state = pricing_state_content();
        state.is_searching = true;
        state.pricing_query = "opus".to_string();

        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        controller.handle(key, &mut state).unwrap();
        assert!(!state.is_searching);
        assert_eq!(state.pricing_query, "opus");
    }

    #[test]
    fn esc_while_searching_clears_query_and_stops_searching() {
        let (controller, _) = ctl_with_source_rows(vec![]);
        let mut state = pricing_state_content();
        state.is_searching = true;
        state.pricing_query = "opus".to_string();

        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        controller.handle(key, &mut state).unwrap();
        assert!(!state.is_searching);
        assert!(state.pricing_query.is_empty());
        assert_eq!(state.focus, Focus::Content); // stays on content
    }

    #[test]
    fn esc_on_pricing_content_with_committed_query_clears_filter() {
        let (controller, _) = ctl_with_source_rows(vec![]);
        let mut state = pricing_state_content();
        // Not searching, but query is committed
        state.is_searching = false;
        state.pricing_query = "opus".to_string();

        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        controller.handle(key, &mut state).unwrap();
        assert!(state.pricing_query.is_empty());
        assert!(!state.is_searching);
        assert_eq!(state.focus, Focus::Content); // stays on content
    }

    #[test]
    fn esc_on_pricing_content_with_empty_query_returns_to_sidebar() {
        let (controller, _) = ctl_with_source_rows(vec![]);
        let mut state = pricing_state_content();
        state.is_searching = false;
        state.pricing_query = String::new();

        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        controller.handle(key, &mut state).unwrap();
        assert_eq!(state.focus, Focus::Sidebar);
    }

    #[test]
    fn s_while_searching_appends_to_query_not_sync() {
        let (controller, repo) = ctl_with_source_rows(vec![pricing_row("m1")]);
        let mut state = pricing_state_content();
        state.is_searching = true;
        let initial_repo_len = repo.rows.lock().unwrap().len();

        let key = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE);
        controller.handle(key, &mut state).unwrap();
        assert_eq!(state.pricing_query, "s");
        // repo must not have changed (no sync triggered)
        assert_eq!(repo.rows.lock().unwrap().len(), initial_repo_len);
    }

    #[test]
    fn q_while_searching_appends_to_query_not_quit() {
        let (controller, _) = ctl_with_source_rows(vec![]);
        let mut state = pricing_state_content();
        state.is_searching = true;

        let key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        controller.handle(key, &mut state).unwrap();
        assert_eq!(state.pricing_query, "q");
        assert!(!state.should_quit);
    }

    #[test]
    fn sidebar_navigation_resets_pricing_search() {
        let (controller, _) = ctl_with_source_rows(vec![]);
        let mut state = AppState::new();
        // Navigate to Pricing view from Sidebar
        state.sidebar_selected = 1;
        state.view = View::Pricing;
        state.pricing_query = "opus".to_string();
        state.is_searching = true;
        assert_eq!(state.focus, Focus::Sidebar);

        // sidebar_up clears pricing search
        let key_up = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        controller.handle(key_up, &mut state).unwrap();
        assert!(state.pricing_query.is_empty());
        assert!(!state.is_searching);
    }
}
