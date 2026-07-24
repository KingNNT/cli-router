//! TUI state: current view, modal overlay, cached responses, selection cursors.
//!
//! Pure data + a few transition helpers. Network calls happen in `main.rs`'s
//! event loop and the results are fed back via `AppState::set_*` setters.

use proxy_admin_api::{
    AuthPayload, ConfigPayload, ProviderPayload, QuotaStatusListDto, RecentRequestsResponse,
    StatusResponse, TestProviderResponse, UsageSummaryResponse,
};

/// Whether the TUI is connected to a running proxy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppMode {
    Connected,
    ProxyRequired,
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
    DisableConfirm(DisableConfirmModal),
    Help,
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
    /// Same OAuth flow but for OpenAI providers.
    OAuthOpenAi,
}

impl AuthInputKind {
    pub fn label(self) -> &'static str {
        match self {
            AuthInputKind::Passthrough => "passthrough",
            AuthInputKind::ApiKey => "api_key",
            AuthInputKind::Bearer => "bearer",
            AuthInputKind::OAuthAnthropic => "oauth (anthropic)",
            AuthInputKind::OAuthOpenAi => "oauth (openai)",
        }
    }

    pub fn cycle(self) -> Self {
        match self {
            AuthInputKind::Passthrough => AuthInputKind::ApiKey,
            AuthInputKind::ApiKey => AuthInputKind::Bearer,
            AuthInputKind::Bearer => AuthInputKind::OAuthAnthropic,
            AuthInputKind::OAuthAnthropic => AuthInputKind::OAuthOpenAi,
            AuthInputKind::OAuthOpenAi => AuthInputKind::Passthrough,
        }
    }

    pub fn from_payload(a: &AuthPayload) -> Self {
        match a {
            AuthPayload::Passthrough => AuthInputKind::Passthrough,
            AuthPayload::ApiKey { .. } => AuthInputKind::ApiKey,
            AuthPayload::Bearer { .. } => AuthInputKind::Bearer,
            AuthPayload::AnthropicOAuth { .. } => AuthInputKind::OAuthAnthropic,
            AuthPayload::OpenAiOAuth { .. } => AuthInputKind::OAuthOpenAi,
            AuthPayload::CodexAuto => AuthInputKind::Passthrough,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Anthropic,
    Zai,
    DeepSeek,
    OpenAi,
    Codex,
    Minimax,
    Kimi,
}

impl ProviderKind {
    pub fn label(self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::Zai => "zai",
            ProviderKind::DeepSeek => "deepseek",
            ProviderKind::OpenAi => "openai",
            ProviderKind::Codex => "codex",
            ProviderKind::Minimax => "minimax",
            ProviderKind::Kimi => "kimi",
        }
    }
    pub fn cycle_next(self) -> Self {
        match self {
            ProviderKind::Anthropic => ProviderKind::Zai,
            ProviderKind::Zai => ProviderKind::DeepSeek,
            ProviderKind::DeepSeek => ProviderKind::OpenAi,
            ProviderKind::OpenAi => ProviderKind::Codex,
            ProviderKind::Codex => ProviderKind::Minimax,
            ProviderKind::Minimax => ProviderKind::Kimi,
            ProviderKind::Kimi => ProviderKind::Anthropic,
        }
    }
    pub fn cycle_prev(self) -> Self {
        match self {
            ProviderKind::Anthropic => ProviderKind::Kimi,
            ProviderKind::Zai => ProviderKind::Anthropic,
            ProviderKind::DeepSeek => ProviderKind::Zai,
            ProviderKind::OpenAi => ProviderKind::DeepSeek,
            ProviderKind::Codex => ProviderKind::OpenAi,
            ProviderKind::Minimax => ProviderKind::Codex,
            ProviderKind::Kimi => ProviderKind::Minimax,
        }
    }
    pub fn from_str_or_default(s: &str) -> Self {
        match s {
            "zai" => ProviderKind::Zai,
            "deepseek" => ProviderKind::DeepSeek,
            "openai" => ProviderKind::OpenAi,
            "codex" => ProviderKind::Codex,
            "minimax" => ProviderKind::Minimax,
            "kimi" | "moonshot" => ProviderKind::Kimi,
            _ => ProviderKind::Anthropic,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningEffortInput {
    Unset,
    None,
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
    Max,
}

impl ReasoningEffortInput {
    pub fn label(self) -> &'static str {
        match self {
            ReasoningEffortInput::Unset => "unset",
            ReasoningEffortInput::None => "none",
            ReasoningEffortInput::Minimal => "minimal",
            ReasoningEffortInput::Low => "low",
            ReasoningEffortInput::Medium => "medium",
            ReasoningEffortInput::High => "high",
            ReasoningEffortInput::XHigh => "xhigh",
            ReasoningEffortInput::Max => "max",
        }
    }

    pub fn as_option(self) -> Option<&'static str> {
        match self {
            ReasoningEffortInput::Unset => None,
            ReasoningEffortInput::None => Some("none"),
            ReasoningEffortInput::Minimal => Some("minimal"),
            ReasoningEffortInput::Low => Some("low"),
            ReasoningEffortInput::Medium => Some("medium"),
            ReasoningEffortInput::High => Some("high"),
            ReasoningEffortInput::XHigh => Some("xhigh"),
            ReasoningEffortInput::Max => Some("max"),
        }
    }

    pub fn from_option(value: Option<&str>) -> Self {
        match value {
            Some("none") => ReasoningEffortInput::None,
            Some("minimal") => ReasoningEffortInput::Minimal,
            Some("low") => ReasoningEffortInput::Low,
            Some("medium") => ReasoningEffortInput::Medium,
            Some("high") => ReasoningEffortInput::High,
            Some("xhigh") => ReasoningEffortInput::XHigh,
            Some("max") => ReasoningEffortInput::Max,
            _ => ReasoningEffortInput::Unset,
        }
    }

    /// Whether this effort value is valid for the given provider kind.
    pub fn is_valid_for(self, provider_kind: ProviderKind) -> bool {
        match provider_kind {
            ProviderKind::Codex => matches!(
                self,
                ReasoningEffortInput::None
                    | ReasoningEffortInput::Minimal
                    | ReasoningEffortInput::Low
                    | ReasoningEffortInput::Medium
                    | ReasoningEffortInput::High
                    | ReasoningEffortInput::XHigh
            ),
            ProviderKind::Anthropic => matches!(
                self,
                ReasoningEffortInput::Low
                    | ReasoningEffortInput::Medium
                    | ReasoningEffortInput::High
                    | ReasoningEffortInput::XHigh
                    | ReasoningEffortInput::Max
            ),
            _ => false,
        }
    }

    /// Effort levels for each provider (Unset is handled separately).
    const CODEX_EFFORTS: &[ReasoningEffortInput] = &[
        ReasoningEffortInput::None,
        ReasoningEffortInput::Minimal,
        ReasoningEffortInput::Low,
        ReasoningEffortInput::Medium,
        ReasoningEffortInput::High,
        ReasoningEffortInput::XHigh,
    ];

    const ANTHROPIC_EFFORTS: &[ReasoningEffortInput] = &[
        ReasoningEffortInput::Low,
        ReasoningEffortInput::Medium,
        ReasoningEffortInput::High,
        ReasoningEffortInput::XHigh,
        ReasoningEffortInput::Max,
    ];

    /// Return the effort list for a provider kind.
    fn efforts_for(provider_kind: ProviderKind) -> &'static [ReasoningEffortInput] {
        match provider_kind {
            ProviderKind::Codex => Self::CODEX_EFFORTS,
            ProviderKind::Anthropic => Self::ANTHROPIC_EFFORTS,
            _ => &[],
        }
    }

    /// Cycle to the next valid effort for the given provider.
    /// Unset always goes to the first valid effort; values outside the
    /// provider's set are clamped back to the first valid effort.
    pub fn cycle_next_for(self, provider_kind: ProviderKind) -> Self {
        let efforts = Self::efforts_for(provider_kind);
        if efforts.is_empty() {
            return ReasoningEffortInput::Unset;
        }
        match self {
            ReasoningEffortInput::Unset => efforts[0],
            current => {
                if let Some(idx) = efforts.iter().position(|&e| e == current) {
                    efforts[(idx + 1) % efforts.len()]
                } else {
                    efforts[0]
                }
            }
        }
    }

    pub fn cycle_prev_for(self, provider_kind: ProviderKind) -> Self {
        let efforts = Self::efforts_for(provider_kind);
        if efforts.is_empty() {
            return ReasoningEffortInput::Unset;
        }
        match self {
            ReasoningEffortInput::Unset => efforts[efforts.len() - 1],
            current => {
                if let Some(idx) = efforts.iter().position(|&e| e == current) {
                    efforts[(idx + efforts.len() - 1) % efforts.len()]
                } else {
                    efforts[efforts.len() - 1]
                }
            }
        }
    }

    /// Clamp the current value to a valid one for the given provider.
    /// If the value is not valid for this provider, returns Unset.
    pub fn clamp_for(self, provider_kind: ProviderKind) -> Self {
        if self == ReasoningEffortInput::Unset || self.is_valid_for(provider_kind) {
            self
        } else {
            ReasoningEffortInput::Unset
        }
    }
}

/// Cycle widget state for MiniMax thinking_mode. Mirrors ReasoningEffortInput.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThinkingModeInput {
    #[default]
    Unset,
    SplitOnly,
    StripAll,
}

impl ThinkingModeInput {
    pub fn label(self) -> &'static str {
        match self {
            ThinkingModeInput::Unset => "unset",
            ThinkingModeInput::SplitOnly => "split_only",
            ThinkingModeInput::StripAll => "strip_all",
        }
    }

    pub fn as_option(self) -> Option<&'static str> {
        match self {
            ThinkingModeInput::Unset => None,
            ThinkingModeInput::SplitOnly => Some("split_only"),
            ThinkingModeInput::StripAll => Some("strip_all"),
        }
    }

    pub fn from_option(opt: Option<&str>) -> Self {
        match opt {
            Some("split_only") => ThinkingModeInput::SplitOnly,
            Some("strip_all") => ThinkingModeInput::StripAll,
            _ => ThinkingModeInput::Unset,
        }
    }

    pub fn cycle_next(self) -> Self {
        match self {
            ThinkingModeInput::Unset => ThinkingModeInput::SplitOnly,
            ThinkingModeInput::SplitOnly => ThinkingModeInput::StripAll,
            ThinkingModeInput::StripAll => ThinkingModeInput::Unset,
        }
    }

    pub fn cycle_prev(self) -> Self {
        match self {
            ThinkingModeInput::Unset => ThinkingModeInput::StripAll,
            ThinkingModeInput::SplitOnly => ThinkingModeInput::Unset,
            ThinkingModeInput::StripAll => ThinkingModeInput::SplitOnly,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormField {
    Name,
    Kind,
    AnthropicBaseUrl,
    OpenaiBaseUrl,
    ReasoningEffort,
    ThinkingMode,
    SanitizeEmptyTools,
    AuthKind,
    AuthValue,
    Enabled,
    Save,
}

impl FormField {
    pub fn next(self, auth_kind: AuthInputKind, provider_kind: ProviderKind) -> Self {
        let order = field_order(auth_kind, provider_kind);
        let idx = order.iter().position(|f| *f == self).unwrap_or(0);
        order[(idx + 1) % order.len()]
    }
    pub fn prev(self, auth_kind: AuthInputKind, provider_kind: ProviderKind) -> Self {
        let order = field_order(auth_kind, provider_kind);
        let idx = order.iter().position(|f| *f == self).unwrap_or(0);
        order[(idx + order.len() - 1) % order.len()]
    }
}

/// Field traversal order. AuthValue is omitted when the auth kind doesn't
/// need a typed value.
fn field_order(auth_kind: AuthInputKind, provider_kind: ProviderKind) -> Vec<FormField> {
    let mut order = vec![
        FormField::Name,
        FormField::Kind,
        FormField::AnthropicBaseUrl,
        FormField::OpenaiBaseUrl,
    ];
    if matches!(provider_kind, ProviderKind::Codex | ProviderKind::Anthropic) {
        order.push(FormField::ReasoningEffort);
    }
    if provider_kind == ProviderKind::Minimax {
        order.push(FormField::ThinkingMode);
    }
    if provider_kind == ProviderKind::Kimi {
        order.push(FormField::SanitizeEmptyTools);
    }
    order.push(FormField::AuthKind);
    if matches!(auth_kind, AuthInputKind::ApiKey | AuthInputKind::Bearer) {
        order.push(FormField::AuthValue);
    }
    order.push(FormField::Enabled);
    order.push(FormField::Save);
    order
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
    pub anthropic_base_url: String,
    pub openai_base_url: String,
    pub reasoning_effort: ReasoningEffortInput,
    pub thinking_mode: ThinkingModeInput,
    pub sanitize_empty_tools: bool,
    /// Whether this provider is active and eligible for routing.
    pub enabled: bool,
    pub auth_kind: AuthInputKind,
    pub auth_value: String,
    /// Preserved across edits (not yet editable in the form UI). Carries the
    /// provider's per-provider concurrency cap so a TUI edit doesn't reset it.
    pub max_concurrent: Option<usize>,
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
            anthropic_base_url: String::new(),
            openai_base_url: String::new(),
            reasoning_effort: ReasoningEffortInput::Unset,
            thinking_mode: ThinkingModeInput::Unset,
            sanitize_empty_tools: false,
            enabled: true,
            auth_kind: AuthInputKind::Passthrough,
            auth_value: String::new(),
            max_concurrent: None,
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
            anthropic_base_url: p.anthropic_base_url.clone().unwrap_or_default(),
            openai_base_url: p.openai_base_url.clone().unwrap_or_default(),
            reasoning_effort: ReasoningEffortInput::from_option(p.reasoning_effort.as_deref()),
            thinking_mode: ThinkingModeInput::from_option(p.thinking_mode.as_deref()),
            sanitize_empty_tools: p.sanitize_empty_tools.unwrap_or(false),
            enabled: p.enabled,
            auth_kind,
            auth_value,
            max_concurrent: p.max_concurrent,
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

/// Confirmation modal shown when disabling a provider that is referenced by
/// one or more routing rules. `rules` is non-empty by construction.
#[derive(Debug, Clone)]
pub struct DisableConfirmModal {
    pub provider_index: usize,
    pub provider_name: String,
    pub rules: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingField {
    MatchModel,
    Provider,
    Fallback,
    Strategy,
    Priority,
    Save,
}

impl RoutingField {
    pub fn next(self) -> Self {
        match self {
            Self::MatchModel => Self::Provider,
            Self::Provider => Self::Fallback,
            Self::Fallback => Self::Strategy,
            Self::Strategy => Self::Priority,
            Self::Priority => Self::Save,
            Self::Save => Self::MatchModel,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::MatchModel => Self::Save,
            Self::Provider => Self::MatchModel,
            Self::Fallback => Self::Provider,
            Self::Strategy => Self::Fallback,
            Self::Priority => Self::Strategy,
            Self::Save => Self::Priority,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RoutingFormModal {
    pub mode: FormMode,
    pub focused: RoutingField,
    pub match_model: String,
    pub available_providers: Vec<String>,
    pub provider_index: usize,
    pub fallback: Vec<String>,
    pub fallback_cursor: usize,
    pub strategy: proxy_admin_api::RoutingStrategyPayload,
    pub priority: String,
    pub error: Option<String>,
}

impl RoutingFormModal {
    pub fn new_for_add(available_providers: Vec<String>) -> Self {
        Self {
            mode: FormMode::Add,
            focused: RoutingField::MatchModel,
            match_model: String::new(),
            provider_index: 0,
            available_providers,
            fallback: Vec::new(),
            fallback_cursor: 0,
            strategy: proxy_admin_api::RoutingStrategyPayload::default(),
            priority: String::new(),
            error: None,
        }
    }

    pub fn from_rule(
        index: usize,
        rule: &proxy_admin_api::RoutingRulePayload,
        available_providers: Vec<String>,
    ) -> Self {
        let provider_index = available_providers
            .iter()
            .position(|p| p == &rule.provider)
            .unwrap_or(0);
        let fallback: Vec<String> = rule
            .fallback
            .iter()
            .filter(|f| available_providers.contains(f))
            .cloned()
            .collect();
        Self {
            mode: FormMode::Edit {
                original_index: index,
                original_name: rule.provider.clone(),
            },
            focused: RoutingField::MatchModel,
            match_model: rule.r#match.model.clone().unwrap_or_default(),
            provider_index,
            available_providers,
            fallback,
            fallback_cursor: 0,
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
    Save,
}

impl QuotaField {
    pub fn next(self) -> Self {
        match self {
            Self::Provider => Self::Window,
            Self::Window => Self::MaxRequests,
            Self::MaxRequests => Self::MaxInputTokens,
            Self::MaxInputTokens => Self::MaxOutputTokens,
            Self::MaxOutputTokens => Self::WarnPct,
            Self::WarnPct => Self::Save,
            Self::Save => Self::Provider,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Provider => Self::Save,
            Self::Window => Self::Provider,
            Self::MaxRequests => Self::Window,
            Self::MaxInputTokens => Self::MaxRequests,
            Self::MaxOutputTokens => Self::MaxInputTokens,
            Self::WarnPct => Self::MaxOutputTokens,
            Self::Save => Self::WarnPct,
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
            max_requests: quota
                .max_requests
                .map(|v| v.to_string())
                .unwrap_or_default(),
            max_input_tokens: quota
                .max_input_tokens
                .map(|v| v.to_string())
                .unwrap_or_default(),
            max_output_tokens: quota
                .max_output_tokens
                .map(|v| v.to_string())
                .unwrap_or_default(),
            warn_pct: quota.warn_pct.to_string(),
            error: None,
        }
    }
}

pub struct AppState {
    pub mode: AppMode,
    pub view: View,
    pub modal: Modal,
    pub status: Option<Result<StatusResponse, String>>,
    pub config: Option<Result<ConfigPayload, String>>,
    pub quota: Option<Result<QuotaStatusListDto, String>>,
    pub config_section: ConfigSection,
    pub providers_selected: usize,
    pub routing_selected: usize,
    pub quota_selected: usize,
    pub requests: RequestsPaneState,
    pub usage: UsagePaneState,
    pub account: AccountPaneState,
    /// Background-thread channel for in-flight Account fetches.
    /// `None` = no fetch in progress; `Some(rx)` = waiting on a result.
    pub account_rx:
        Option<std::sync::mpsc::Receiver<Result<proxy_admin_api::AccountUsageResponse, String>>>,
    pub flash: Option<String>,
    pub should_quit: bool,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            mode: AppMode::ProxyRequired,
            view: View::Status,
            modal: Modal::None,
            status: None,
            config: None,
            quota: None,
            config_section: ConfigSection::Providers,
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
            View::Config => match self.config_section {
                ConfigSection::Providers => {
                    if let Some(Ok(cfg)) = &self.config
                        && self.providers_selected + 1 < cfg.providers.len()
                    {
                        self.providers_selected += 1;
                    }
                }
                ConfigSection::Routing => {
                    if let Some(Ok(cfg)) = &self.config
                        && self.routing_selected + 1 < cfg.routing.len()
                    {
                        self.routing_selected += 1;
                    }
                }
                ConfigSection::Quotas => {
                    if let Some(Ok(cfg)) = &self.config
                        && self.quota_selected + 1 < cfg.quota.len()
                    {
                        self.quota_selected += 1;
                    }
                }
                ConfigSection::Settings => {}
            },
            View::Requests if self.requests.selected + 1 < self.requests.items.len() => {
                self.requests.selected += 1;
            }
            _ => {}
        }
    }

    pub fn move_selection_up(&mut self) {
        match self.view {
            View::Config => match self.config_section {
                ConfigSection::Providers => {
                    self.providers_selected = self.providers_selected.saturating_sub(1);
                }
                ConfigSection::Routing => {
                    self.routing_selected = self.routing_selected.saturating_sub(1);
                }
                ConfigSection::Quotas => {
                    self.quota_selected = self.quota_selected.saturating_sub(1);
                }
                ConfigSection::Settings => {}
            },
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
        assert_eq!(
            f.next(AuthInputKind::ApiKey, ProviderKind::Anthropic),
            FormField::Name
        );
    }

    #[test]
    fn prev_wraps_from_name_to_save() {
        let f = FormField::Name;
        assert_eq!(
            f.prev(AuthInputKind::ApiKey, ProviderKind::Anthropic),
            FormField::Save
        );
    }

    #[test]
    fn passthrough_skips_auth_value_field() {
        let f = FormField::AuthKind;
        assert_eq!(
            f.next(AuthInputKind::Passthrough, ProviderKind::Anthropic),
            FormField::Enabled
        );
    }

    #[test]
    fn field_order_includes_enabled() {
        let order = field_order(AuthInputKind::Passthrough, ProviderKind::Anthropic);
        assert!(order.contains(&FormField::Enabled));
        assert_eq!(order.last().copied(), Some(FormField::Save));
    }

    #[test]
    fn api_key_includes_auth_value_field() {
        let f = FormField::AuthKind;
        assert_eq!(
            f.next(AuthInputKind::ApiKey, ProviderKind::Anthropic),
            FormField::AuthValue
        );
    }

    #[test]
    fn codex_includes_reasoning_effort_field() {
        let f = FormField::OpenaiBaseUrl;
        assert_eq!(
            f.next(AuthInputKind::Passthrough, ProviderKind::Codex),
            FormField::ReasoningEffort
        );
    }

    #[test]
    fn thinking_mode_input_cycle() {
        use super::ThinkingModeInput;
        assert_eq!(
            ThinkingModeInput::Unset.cycle_next(),
            ThinkingModeInput::SplitOnly
        );
        assert_eq!(
            ThinkingModeInput::SplitOnly.cycle_next(),
            ThinkingModeInput::StripAll
        );
        assert_eq!(
            ThinkingModeInput::StripAll.cycle_next(),
            ThinkingModeInput::Unset
        );
        assert_eq!(
            ThinkingModeInput::Unset.cycle_prev(),
            ThinkingModeInput::StripAll
        );
    }

    #[test]
    fn thinking_mode_input_from_option() {
        use super::ThinkingModeInput;
        assert_eq!(
            ThinkingModeInput::from_option(Some("split_only")),
            ThinkingModeInput::SplitOnly
        );
        assert_eq!(
            ThinkingModeInput::from_option(Some("strip_all")),
            ThinkingModeInput::StripAll
        );
        assert_eq!(
            ThinkingModeInput::from_option(None),
            ThinkingModeInput::Unset
        );
        assert_eq!(
            ThinkingModeInput::from_option(Some("garbage")),
            ThinkingModeInput::Unset
        );
    }

    #[test]
    fn minimax_includes_thinking_mode_field() {
        use super::{AuthInputKind, FormField, ProviderKind, field_order};
        let order = field_order(AuthInputKind::Passthrough, ProviderKind::Minimax);
        assert!(order.contains(&FormField::ThinkingMode));
        assert!(!order.contains(&FormField::ReasoningEffort));
    }

    #[test]
    fn routing_field_navigation_includes_save() {
        assert_eq!(RoutingField::Priority.next(), RoutingField::Save);
        assert_eq!(RoutingField::Save.next(), RoutingField::MatchModel);
        assert_eq!(RoutingField::MatchModel.prev(), RoutingField::Save);
        assert_eq!(RoutingField::Save.prev(), RoutingField::Priority);
    }

    #[test]
    fn routing_form_construction_preserves_providers_order() {
        let providers = vec![
            "zai".to_string(),
            "anthropic".to_string(),
            "openai".to_string(),
        ];
        let m = super::RoutingFormModal::new_for_add(providers.clone());
        assert_eq!(m.available_providers, providers);
        assert_eq!(m.provider_index, 0);
        assert!(m.fallback.is_empty());
        assert_eq!(m.fallback_cursor, 0);
    }

    #[test]
    fn quota_field_navigation_includes_save() {
        assert_eq!(QuotaField::WarnPct.next(), QuotaField::Save);
        assert_eq!(QuotaField::Save.next(), QuotaField::Provider);
        assert_eq!(QuotaField::Provider.prev(), QuotaField::Save);
        assert_eq!(QuotaField::Save.prev(), QuotaField::WarnPct);
    }

    #[test]
    fn codex_auto_auth_uses_non_oauth_form_mode() {
        assert_eq!(
            AuthInputKind::from_payload(&AuthPayload::CodexAuto),
            AuthInputKind::Passthrough
        );
    }

    #[test]
    fn provider_kind_cycle() {
        // next direction
        assert_eq!(ProviderKind::Anthropic.cycle_next(), ProviderKind::Zai);
        assert_eq!(ProviderKind::Zai.cycle_next(), ProviderKind::DeepSeek);
        assert_eq!(ProviderKind::DeepSeek.cycle_next(), ProviderKind::OpenAi);
        assert_eq!(ProviderKind::OpenAi.cycle_next(), ProviderKind::Codex);
        assert_eq!(ProviderKind::Codex.cycle_next(), ProviderKind::Minimax);
        assert_eq!(ProviderKind::Minimax.cycle_next(), ProviderKind::Kimi);
        assert_eq!(ProviderKind::Kimi.cycle_next(), ProviderKind::Anthropic);
        // prev direction
        assert_eq!(ProviderKind::Anthropic.cycle_prev(), ProviderKind::Kimi);
        assert_eq!(ProviderKind::Kimi.cycle_prev(), ProviderKind::Minimax);
        assert_eq!(ProviderKind::Minimax.cycle_prev(), ProviderKind::Codex);
        assert_eq!(ProviderKind::Codex.cycle_prev(), ProviderKind::OpenAi);
        assert_eq!(ProviderKind::OpenAi.cycle_prev(), ProviderKind::DeepSeek);
        assert_eq!(ProviderKind::DeepSeek.cycle_prev(), ProviderKind::Zai);
        assert_eq!(ProviderKind::Zai.cycle_prev(), ProviderKind::Anthropic);
    }

    #[test]
    fn kimi_includes_sanitize_empty_tools_field() {
        use super::{AuthInputKind, FormField, ProviderKind, field_order};
        let order = field_order(AuthInputKind::Passthrough, ProviderKind::Kimi);
        assert!(order.contains(&FormField::SanitizeEmptyTools));
    }

    #[test]
    fn non_kimi_excludes_sanitize_empty_tools_field() {
        use super::{AuthInputKind, FormField, ProviderKind, field_order};
        let order = field_order(AuthInputKind::Passthrough, ProviderKind::Zai);
        assert!(!order.contains(&FormField::SanitizeEmptyTools));
    }

    #[test]
    fn cycle_includes_kimi() {
        assert_eq!(ProviderKind::Minimax.cycle_next(), ProviderKind::Kimi);
        assert_eq!(ProviderKind::Kimi.cycle_next(), ProviderKind::Anthropic);
        assert_eq!(ProviderKind::Anthropic.cycle_prev(), ProviderKind::Kimi);
        assert_eq!(ProviderKind::Kimi.cycle_prev(), ProviderKind::Minimax);
        assert_eq!(
            ProviderKind::from_str_or_default("kimi"),
            ProviderKind::Kimi
        );
        assert_eq!(ProviderKind::Kimi.label(), "kimi");
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
