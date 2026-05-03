//! TUI state: current view, modal overlay, cached responses, selection cursors.
//!
//! Pure data + a few transition helpers. Network calls happen in `main.rs`'s
//! event loop and the results are fed back via `AppState::set_*` setters.

use proxy_admin_api::{
    AuthPayload, ConfigPayload, ProviderPayload, RecentRequestsResponse, StatusResponse,
    TestProviderResponse, UsageSummaryResponse,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Status,
    Providers,
    Routing,
    Requests,
    Usage,
}

impl View {
    pub fn label(self) -> &'static str {
        match self {
            View::Status => "Status",
            View::Providers => "Providers",
            View::Routing => "Routing",
            View::Requests => "Requests",
            View::Usage => "Usage",
        }
    }
}

pub const ALL_VIEWS: &[View] = &[
    View::Status,
    View::Providers,
    View::Routing,
    View::Requests,
    View::Usage,
];

#[allow(dead_code)] // wired up in the Usage controller (Task 10)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangePreset {
    Today,
    D7,
    D30,
    All,
}

impl RangePreset {
    #[allow(dead_code)] // consumed by the Usage renderer (Task 10)
    pub fn label(self) -> &'static str {
        match self {
            RangePreset::Today => "Today",
            RangePreset::D7 => "7d",
            RangePreset::D30 => "30d",
            RangePreset::All => "All",
        }
    }

    /// Resolve the preset to an inclusive `(from_ms, to_ms)` window using the
    /// supplied "now" (epoch ms in local timezone). Splitting `now` out makes
    /// this trivially testable with deterministic times.
    #[allow(dead_code)] // consumed by the Usage controller (Task 10)
    pub fn to_range(self, now_ms: i64, local_midnight_ms: i64) -> (i64, i64) {
        const DAY_MS: i64 = 86_400_000;
        match self {
            RangePreset::Today => (local_midnight_ms, now_ms),
            RangePreset::D7 => (now_ms - 7 * DAY_MS, now_ms),
            RangePreset::D30 => (now_ms - 30 * DAY_MS, now_ms),
            RangePreset::All => (0, now_ms),
        }
    }
}

#[allow(dead_code)] // fields read by the Usage renderer/controller (Task 10)
#[derive(Debug, Clone, Default)]
pub struct UsagePaneState {
    pub range_preset: Option<RangePreset>,
    pub summary: Option<UsageSummaryResponse>,
    pub table_offset: usize,
    pub last_error: Option<String>,
    pub loading: bool,
}

#[derive(Debug, Clone)]
pub enum Modal {
    None,
    TestProvider(TestProviderModal),
    EditAuth(EditAuthModal),
}

#[derive(Debug, Clone)]
pub struct TestProviderModal {
    pub provider_name: String,
    pub model_input: String,
    pub state: TestState,
}

#[derive(Debug, Clone)]
pub enum TestState {
    Editing,
    InFlight,
    Done(TestProviderResponse),
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct EditAuthModal {
    pub provider_index: usize,
    pub provider_name: String,
    pub kind: AuthInputKind,
    pub value_input: String,
    pub state: EditState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthInputKind {
    Passthrough,
    ApiKey,
    Bearer,
    /// Triggers the OAuth paste flow against Anthropic — final stored auth
    /// is `Bearer { value: <access_token> }`, but selecting this kind in
    /// the modal initiates the browser dance instead of asking for a
    /// pasted value.
    OAuthAnthropic,
}

impl AuthInputKind {
    pub fn label(self) -> &'static str {
        match self {
            AuthInputKind::Passthrough => "passthrough",
            AuthInputKind::ApiKey => "api_key",
            AuthInputKind::Bearer => "bearer",
            AuthInputKind::OAuthAnthropic => "oauth (anthropic)",
        }
    }

    pub fn cycle(self) -> Self {
        match self {
            AuthInputKind::Passthrough => AuthInputKind::ApiKey,
            AuthInputKind::ApiKey => AuthInputKind::Bearer,
            AuthInputKind::Bearer => AuthInputKind::OAuthAnthropic,
            AuthInputKind::OAuthAnthropic => AuthInputKind::Passthrough,
        }
    }

    pub fn from_payload(a: &AuthPayload) -> Self {
        match a {
            AuthPayload::Passthrough => AuthInputKind::Passthrough,
            AuthPayload::ApiKey { .. } => AuthInputKind::ApiKey,
            AuthPayload::Bearer { .. } => AuthInputKind::Bearer,
            AuthPayload::AnthropicOAuth { .. } => AuthInputKind::OAuthAnthropic,
        }
    }

    pub fn into_payload(self, value: String) -> AuthPayload {
        match self {
            AuthInputKind::Passthrough => AuthPayload::Passthrough,
            AuthInputKind::ApiKey => AuthPayload::ApiKey { value },
            AuthInputKind::Bearer | AuthInputKind::OAuthAnthropic => AuthPayload::Bearer { value },
        }
    }
}

#[derive(Debug, Clone)]
pub enum EditState {
    Editing,
    Saving,
    /// OAuth flow: daemon has issued an authorization URL, waiting for the
    /// user to paste the `code` value from the redirect page.
    OAuthAwaitingCode {
        authorization_url: String,
        state_id: String,
        code_input: String,
    },
    OAuthExchanging,
    Done,
    Failed(String),
}

pub struct AppState {
    pub view: View,
    pub modal: Modal,
    pub status: Option<Result<StatusResponse, String>>,
    pub config: Option<Result<ConfigPayload, String>>,
    pub recent: Option<Result<RecentRequestsResponse, String>>,
    pub providers_selected: usize,
    pub requests_selected: usize,
    #[allow(dead_code)] // read by the Usage renderer/controller (Task 10)
    pub usage: UsagePaneState,
    pub flash: Option<String>,
    pub should_quit: bool,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            view: View::Status,
            modal: Modal::None,
            status: None,
            config: None,
            recent: None,
            providers_selected: 0,
            requests_selected: 0,
            usage: UsagePaneState::default(),
            flash: None,
            should_quit: false,
        }
    }

    pub fn set_view(&mut self, v: View) {
        self.view = v;
    }

    pub fn set_status(&mut self, r: Result<StatusResponse, String>) {
        self.status = Some(r);
    }

    pub fn set_config(&mut self, r: Result<ConfigPayload, String>) {
        if let Ok(cfg) = &r {
            // Clamp selection if provider list shrank.
            if self.providers_selected >= cfg.providers.len() {
                self.providers_selected = cfg.providers.len().saturating_sub(1);
            }
        }
        self.config = Some(r);
    }

    pub fn set_recent(&mut self, r: Result<RecentRequestsResponse, String>) {
        if let Ok(resp) = &r
            && self.requests_selected >= resp.items.len()
        {
            self.requests_selected = resp.items.len().saturating_sub(1);
        }
        self.recent = Some(r);
    }

    pub fn flash(&mut self, msg: impl Into<String>) {
        self.flash = Some(msg.into());
    }

    pub fn selected_provider(&self) -> Option<&ProviderPayload> {
        let cfg = self.config.as_ref().and_then(|r| r.as_ref().ok())?;
        cfg.providers.get(self.providers_selected)
    }

    pub fn move_selection_down(&mut self) {
        match self.view {
            View::Providers => {
                if let Some(Ok(cfg)) = &self.config
                    && self.providers_selected + 1 < cfg.providers.len()
                {
                    self.providers_selected += 1;
                }
            }
            View::Requests => {
                if let Some(Ok(r)) = &self.recent
                    && self.requests_selected + 1 < r.items.len()
                {
                    self.requests_selected += 1;
                }
            }
            _ => {}
        }
    }

    pub fn move_selection_up(&mut self) {
        match self.view {
            View::Providers => {
                self.providers_selected = self.providers_selected.saturating_sub(1);
            }
            View::Requests => {
                self.requests_selected = self.requests_selected.saturating_sub(1);
            }
            _ => {}
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod range_preset_tests {
    use super::*;

    #[test]
    fn today_uses_local_midnight_to_now() {
        let now = 1_730_086_400_000;
        let midnight = 1_730_073_600_000;
        assert_eq!(RangePreset::Today.to_range(now, midnight), (midnight, now));
    }

    #[test]
    fn d7_subtracts_seven_days() {
        let now = 1_730_086_400_000;
        let (from, to) = RangePreset::D7.to_range(now, 0);
        assert_eq!(to, now);
        assert_eq!(now - from, 7 * 86_400_000);
    }

    #[test]
    fn all_starts_at_epoch() {
        let now = 1_730_086_400_000;
        assert_eq!(RangePreset::All.to_range(now, 0), (0, now));
    }
}
