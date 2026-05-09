//! TUI state: current view, modal overlay, cached responses, selection cursors.
//!
//! Pure data + a few transition helpers. Network calls happen in `main.rs`'s
//! event loop and the results are fed back via `AppState::set_*` setters.

use proxy_admin_api::{
    AuthPayload, ConfigPayload, ProviderPayload, QuotaStatusListDto, RecentRequestsResponse,
    StatusResponse, TestProviderResponse, UsageSummaryResponse,
};

/// Whether the TUI is connected to a running proxy or operating offline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppMode {
    Connected,
    Offline,
}

/// Sub-sections within the Config tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSection {
    Providers,
    Routing,
    Quotas,
    Settings,
}

impl ConfigSection {
    pub fn label(self) -> &'static str {
        match self {
            Self::Providers => "Providers",
            Self::Routing => "Routing",
            Self::Quotas => "Quotas",
            Self::Settings => "Settings",
        }
    }

    pub const ALL: &[ConfigSection] = &[
        ConfigSection::Providers,
        ConfigSection::Routing,
        ConfigSection::Quotas,
        ConfigSection::Settings,
    ];

    pub fn next(self) -> Self {
        let idx = Self::ALL.iter().position(|&s| s == self).unwrap_or(0);
        Self::ALL[(idx + 1) % Self::ALL.len()]
    }

    pub fn prev(self) -> Self {
        let idx = Self::ALL.iter().position(|&s| s == self).unwrap_or(0);
        Self::ALL[(idx + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Status,
    Config,
    Requests,
    Usage,
    Account,
}

impl View {
    pub fn label(self) -> &'static str {
        match self {
            View::Status => "Status",
            View::Config => "Config",
            View::Requests => "Requests",
            View::Usage => "Usage",
            View::Account => "Account",
        }
    }
}

pub const ALL_VIEWS: &[View] = &[
    View::Status,
    View::Config,
    View::Requests,
    View::Usage,
    View::Account,
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

#[derive(Debug, Clone, Default)]
pub struct AccountPaneState {
    pub usage: Option<proxy_admin_api::AccountUsageResponse>,
    pub last_error: Option<String>,
    pub loading: bool,
    pub scroll_offset: usize,
}

#[derive(Debug, Clone)]
pub struct RequestsPaneState {
    /// Currently loaded items.
    pub items: Vec<proxy_admin_api::RecentRequestItem>,
    /// Total row count in the database (for pagination indicator).
    pub total_count: u64,
    /// Index of the highlighted row within `items`.
    pub selected: usize,
    /// Vertical scroll offset for the visible viewport.
    pub scroll_offset: usize,
    /// Backend offset – how many rows we have fetched so far.
    pub fetched_offset: usize,
    /// Number of rows per backend fetch (page size).
    pub page_size: u32,
    /// Last error from the API, if any.
    pub last_error: Option<String>,
    /// Whether a fetch is in progress.
    pub loading: bool,
}

impl Default for RequestsPaneState {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            total_count: 0,
            selected: 0,
            scroll_offset: 0,
            fetched_offset: 0,
            page_size: 50,
            last_error: None,
            loading: false,
        }
    }
}

impl RequestsPaneState {
    pub fn item_count(&self) -> usize {
        self.items.len()
    }

    /// Whether more items can be fetched from the backend.
    pub fn has_more(&self) -> bool {
        (self.items.len() as u64) < self.total_count
    }
}

#[derive(Debug, Clone)]
pub enum Modal {
    None,
    TestProvider(TestProviderModal),
    ProviderForm(ProviderFormModal),
    DeleteConfirm(DeleteConfirmModal),
    Help,
    Wizard(WizardState),
    RoutingForm(RoutingFormModal),
    QuotaForm(QuotaFormModal),
}

/// Action exposed as a clickable button on the Providers tab toolbar.
/// Mirrors the existing keyboard shortcuts so mouse and keyboard stay in sync.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderAction {
    Add,
    Edit,
    Delete,
    Test,
    Refresh,
}

impl ProviderAction {
    pub fn label(self) -> &'static str {
        match self {
            ProviderAction::Add => "[ Add ]",
            ProviderAction::Edit => "[ Edit ]",
            ProviderAction::Delete => "[ Delete ]",
            ProviderAction::Test => "[ Test ]",
            ProviderAction::Refresh => "[ Refresh ]",
        }
    }
}

pub const PROVIDER_TOOLBAR: &[ProviderAction] = &[
    ProviderAction::Add,
    ProviderAction::Edit,
    ProviderAction::Delete,
    ProviderAction::Test,
    ProviderAction::Refresh,
];

/// Joiner rendered between toolbar buttons. Hit-test math depends on this
/// matching the renderer exactly.
pub const PROVIDER_TOOLBAR_GAP: &str = "  ";

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Anthropic,
    Zai,
}

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

#[derive(Debug, Clone)]
pub enum FormMode {
    Add,
    Edit {
        original_index: usize,
        original_name: String,
    },
}

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

#[derive(Debug, Clone)]
pub struct DeleteConfirmModal {
    pub provider_index: usize,
    pub provider_name: String,
    /// Non-empty means delete is blocked. UI must not offer `[y]` in that case.
    pub blocking_rules: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingField {
    MatchModel,
    Provider,
    Fallback,
    Strategy,
    Priority,
}

impl RoutingField {
    pub fn next(self) -> Self {
        match self {
            Self::MatchModel => Self::Provider,
            Self::Provider => Self::Fallback,
            Self::Fallback => Self::Strategy,
            Self::Strategy => Self::Priority,
            Self::Priority => Self::MatchModel,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::MatchModel => Self::Priority,
            Self::Provider => Self::MatchModel,
            Self::Fallback => Self::Provider,
            Self::Strategy => Self::Fallback,
            Self::Priority => Self::Strategy,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RoutingFormModal {
    pub mode: FormMode,
    pub focused: RoutingField,
    pub match_model: String,
    pub provider: String,
    pub fallback: String,
    pub strategy: proxy_admin_api::RoutingStrategyPayload,
    pub priority: String,
    pub error: Option<String>,
}

impl RoutingFormModal {
    pub fn new_for_add() -> Self {
        Self {
            mode: FormMode::Add,
            focused: RoutingField::MatchModel,
            match_model: String::new(),
            provider: String::new(),
            fallback: String::new(),
            strategy: proxy_admin_api::RoutingStrategyPayload::default(),
            priority: String::new(),
            error: None,
        }
    }

    pub fn from_rule(index: usize, rule: &proxy_admin_api::RoutingRulePayload) -> Self {
        Self {
            mode: FormMode::Edit {
                original_index: index,
                original_name: rule.provider.clone(),
            },
            focused: RoutingField::MatchModel,
            match_model: rule.r#match.model.clone().unwrap_or_default(),
            provider: rule.provider.clone(),
            fallback: rule.fallback.join(", "),
            strategy: rule.strategy.clone(),
            priority: rule.priority.map(|p| p.to_string()).unwrap_or_default(),
            error: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaField {
    Provider,
    Window,
    MaxRequests,
    MaxInputTokens,
    MaxOutputTokens,
    WarnPct,
}

impl QuotaField {
    pub fn next(self) -> Self {
        match self {
            Self::Provider => Self::Window,
            Self::Window => Self::MaxRequests,
            Self::MaxRequests => Self::MaxInputTokens,
            Self::MaxInputTokens => Self::MaxOutputTokens,
            Self::MaxOutputTokens => Self::WarnPct,
            Self::WarnPct => Self::Provider,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Provider => Self::WarnPct,
            Self::Window => Self::Provider,
            Self::MaxRequests => Self::Window,
            Self::MaxInputTokens => Self::MaxRequests,
            Self::MaxOutputTokens => Self::MaxInputTokens,
            Self::WarnPct => Self::MaxOutputTokens,
        }
    }
}

#[derive(Debug, Clone)]
pub struct QuotaFormModal {
    pub mode: FormMode,
    pub focused: QuotaField,
    pub provider: String,
    pub window: String,
    pub max_requests: String,
    pub max_input_tokens: String,
    pub max_output_tokens: String,
    pub warn_pct: String,
    pub error: Option<String>,
}

impl QuotaFormModal {
    pub fn new_for_add() -> Self {
        Self {
            mode: FormMode::Add,
            focused: QuotaField::Provider,
            provider: String::new(),
            window: String::new(),
            max_requests: String::new(),
            max_input_tokens: String::new(),
            max_output_tokens: String::new(),
            warn_pct: "80".into(),
            error: None,
        }
    }

    pub fn from_rule(index: usize, quota: &proxy_admin_api::QuotaPayload) -> Self {
        Self {
            mode: FormMode::Edit {
                original_index: index,
                original_name: quota.provider.clone(),
            },
            focused: QuotaField::Provider,
            provider: quota.provider.clone(),
            window: quota.window.clone(),
            max_requests: quota.max_requests.map(|v| v.to_string()).unwrap_or_default(),
            max_input_tokens: quota.max_input_tokens.map(|v| v.to_string()).unwrap_or_default(),
            max_output_tokens: quota.max_output_tokens.map(|v| v.to_string()).unwrap_or_default(),
            warn_pct: quota.warn_pct.to_string(),
            error: None,
        }
    }
}

/// First-run wizard state.
#[derive(Debug, Clone)]
pub struct WizardState {
    pub step: WizardStep,
    pub form: ProviderFormModal,
    pub saved_path: Option<std::path::PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WizardStep {
    Welcome,
    AddProvider,
    Done,
}

pub struct AppState {
    pub mode: AppMode,
    pub view: View,
    pub modal: Modal,
    pub status: Option<Result<StatusResponse, String>>,
    pub config: Option<Result<ConfigPayload, String>>,
    pub quota: Option<Result<QuotaStatusListDto, String>>,
    pub config_section: ConfigSection,
    pub wizard: Option<WizardState>,
    pub providers_selected: usize,
    pub routing_selected: usize,
    pub quota_selected: usize,
    pub requests: RequestsPaneState,
    pub usage: UsagePaneState,
    pub account: AccountPaneState,
    /// Background-thread channel for in-flight Account fetches.
    /// `None` = no fetch in progress; `Some(rx)` = waiting on a result.
    pub account_rx: Option<std::sync::mpsc::Receiver<Result<proxy_admin_api::AccountUsageResponse, String>>>,
    pub flash: Option<String>,
    pub should_quit: bool,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            mode: AppMode::Offline,
            view: View::Status,
            modal: Modal::None,
            status: None,
            config: None,
            quota: None,
            config_section: ConfigSection::Providers,
            wizard: None,
            providers_selected: 0,
            routing_selected: 0,
            quota_selected: 0,
            requests: RequestsPaneState::default(),
            usage: UsagePaneState::default(),
            account: AccountPaneState::default(),
            account_rx: None,
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
        match r {
            Ok(resp) => {
                self.requests.items = resp.items;
                self.requests.total_count = resp.total_count;
                self.requests.fetched_offset = self.requests.items.len();
                if self.requests.selected >= self.requests.items.len() {
                    self.requests.selected = self.requests.items.len().saturating_sub(1);
                }
                self.requests.last_error = None;
                self.requests.loading = false;
            }
            Err(e) => {
                self.requests.last_error = Some(e);
                self.requests.loading = false;
            }
        }
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
            View::Config => {
                if let Some(Ok(cfg)) = &self.config
                    && self.providers_selected + 1 < cfg.providers.len()
                {
                    self.providers_selected += 1;
                }
            }
            View::Requests => {
                if self.requests.selected + 1 < self.requests.items.len() {
                    self.requests.selected += 1;
                }
            }
            _ => {}
        }
    }

    pub fn move_selection_up(&mut self) {
        match self.view {
            View::Config => {
                self.providers_selected = self.providers_selected.saturating_sub(1);
            }
            View::Requests => {
                self.requests.selected = self.requests.selected.saturating_sub(1);
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
