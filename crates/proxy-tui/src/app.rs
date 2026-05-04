//! TUI state: current view, modal overlay, cached responses, selection cursors.
//!
//! Pure data + a few transition helpers. Network calls happen in `main.rs`'s
//! event loop and the results are fed back via `AppState::set_*` setters.

use proxy_admin_api::{
    AuthPayload, ConfigPayload, ProviderPayload, QuotaStatusListDto, RecentRequestsResponse,
    StatusResponse, TestProviderResponse, UsageSummaryResponse,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangePreset {
    Today,
    D7,
    D30,
    All,
}

impl RangePreset {
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

#[derive(Debug, Clone, Default)]
pub struct UsagePaneState {
    pub range_preset: Option<RangePreset>,
    pub summary: Option<UsageSummaryResponse>,
    pub table_offset: usize,
    pub last_error: Option<String>,
    pub loading: bool,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum Modal {
    None,
    TestProvider(TestProviderModal),
    EditAuth(EditAuthModal),           // removed in Task 5
    ProviderForm(ProviderFormModal),   // new
    DeleteConfirm(DeleteConfirmModal), // new
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

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Anthropic,
    Zai,
}

#[allow(dead_code)]
impl ProviderKind {
    pub fn label(self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::Zai => "zai",
        }
    }
    pub fn cycle_next(self) -> Self {
        match self {
            ProviderKind::Anthropic => ProviderKind::Zai,
            ProviderKind::Zai => ProviderKind::Anthropic,
        }
    }
    pub fn cycle_prev(self) -> Self {
        self.cycle_next() // only two variants, so prev == next
    }
    pub fn from_str_or_default(s: &str) -> Self {
        match s {
            "zai" => ProviderKind::Zai,
            _ => ProviderKind::Anthropic,
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormField {
    Name,
    Kind,
    BaseUrl,
    OpenaiBaseUrl,
    AuthKind,
    AuthValue,
    Save,
}

#[allow(dead_code)]
impl FormField {
    pub fn next(self, auth_kind: AuthInputKind) -> Self {
        let order = field_order(auth_kind);
        let idx = order.iter().position(|f| *f == self).unwrap_or(0);
        order[(idx + 1) % order.len()]
    }
    pub fn prev(self, auth_kind: AuthInputKind) -> Self {
        let order = field_order(auth_kind);
        let idx = order.iter().position(|f| *f == self).unwrap_or(0);
        order[(idx + order.len() - 1) % order.len()]
    }
}

/// Field traversal order. AuthValue is omitted when the auth kind doesn't
/// need a typed value.
#[allow(dead_code)]
fn field_order(auth_kind: AuthInputKind) -> &'static [FormField] {
    match auth_kind {
        AuthInputKind::Passthrough | AuthInputKind::OAuthAnthropic => &[
            FormField::Name,
            FormField::Kind,
            FormField::BaseUrl,
            FormField::OpenaiBaseUrl,
            FormField::AuthKind,
            FormField::Save,
        ],
        AuthInputKind::ApiKey | AuthInputKind::Bearer => &[
            FormField::Name,
            FormField::Kind,
            FormField::BaseUrl,
            FormField::OpenaiBaseUrl,
            FormField::AuthKind,
            FormField::AuthValue,
            FormField::Save,
        ],
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum FormMode {
    Add,
    Edit {
        original_index: usize,
        original_name: String,
    },
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum FormState {
    Editing,
    Saving,
    OAuthAwaitingCode {
        authorization_url: String,
        state_id: String,
        code_input: String,
    },
    OAuthExchanging,
    Failed(String),
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ProviderFormModal {
    pub mode: FormMode,
    pub focused: FormField,
    pub name: String,
    pub kind: ProviderKind,
    pub base_url: String,
    pub openai_base_url: String,
    pub auth_kind: AuthInputKind,
    pub auth_value: String,
    pub state: FormState,
    /// Inline validation error rendered red at top of modal. Cleared on
    /// any field edit.
    pub error: Option<String>,
}

#[allow(dead_code)]
impl ProviderFormModal {
    pub fn new_for_add() -> Self {
        Self {
            mode: FormMode::Add,
            focused: FormField::Name,
            name: String::new(),
            kind: ProviderKind::Anthropic,
            base_url: String::new(),
            openai_base_url: String::new(),
            auth_kind: AuthInputKind::Passthrough,
            auth_value: String::new(),
            state: FormState::Editing,
            error: None,
        }
    }

    pub fn from_provider(index: usize, p: &ProviderPayload) -> Self {
        let auth_kind = AuthInputKind::from_payload(&p.auth);
        let auth_value = match &p.auth {
            AuthPayload::ApiKey { value } | AuthPayload::Bearer { value } => value.clone(),
            _ => String::new(),
        };
        Self {
            mode: FormMode::Edit {
                original_index: index,
                original_name: p.name.clone(),
            },
            focused: FormField::Name,
            name: p.name.clone(),
            kind: ProviderKind::from_str_or_default(&p.kind),
            base_url: p.base_url.clone().unwrap_or_default(),
            openai_base_url: p.openai_base_url.clone().unwrap_or_default(),
            auth_kind,
            auth_value,
            state: FormState::Editing,
            error: None,
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct DeleteConfirmModal {
    pub provider_index: usize,
    pub provider_name: String,
    /// Non-empty means delete is blocked. UI must not offer `[y]` in that case.
    pub blocking_rules: Vec<String>,
}

pub struct AppState {
    pub view: View,
    pub modal: Modal,
    pub status: Option<Result<StatusResponse, String>>,
    pub config: Option<Result<ConfigPayload, String>>,
    pub recent: Option<Result<RecentRequestsResponse, String>>,
    pub quota: Option<Result<QuotaStatusListDto, String>>,
    pub providers_selected: usize,
    pub requests_selected: usize,
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
            quota: None,
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
mod form_field_tests {
    use super::*;

    #[test]
    fn next_wraps_past_save_back_to_name() {
        let f = FormField::Save;
        assert_eq!(f.next(AuthInputKind::ApiKey), FormField::Name);
    }

    #[test]
    fn prev_wraps_from_name_to_save() {
        let f = FormField::Name;
        assert_eq!(f.prev(AuthInputKind::ApiKey), FormField::Save);
    }

    #[test]
    fn passthrough_skips_auth_value_field() {
        let f = FormField::AuthKind;
        assert_eq!(f.next(AuthInputKind::Passthrough), FormField::Save);
    }

    #[test]
    fn api_key_includes_auth_value_field() {
        let f = FormField::AuthKind;
        assert_eq!(f.next(AuthInputKind::ApiKey), FormField::AuthValue);
    }

    #[test]
    fn provider_kind_cycle() {
        assert_eq!(ProviderKind::Anthropic.cycle_next(), ProviderKind::Zai);
        assert_eq!(ProviderKind::Zai.cycle_next(), ProviderKind::Anthropic);
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
