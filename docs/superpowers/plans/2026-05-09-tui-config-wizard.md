# TUI Config Wizard & Full Config Editor — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a first-run wizard and a unified Config tab to the TUI so users can manage all config sections (providers, routing, quotas, settings) without manually editing TOML.

**Architecture:** Extend `ConfigPayload` in `proxy-admin-api` to carry all config fields (quota, affinity, DB paths). The TUI gains dual-mode editing: connected → Admin API; offline → direct TOML file. A wizard flow activates when no config file exists. The existing Providers and Routing tabs merge into a single Config tab with inner sections.

**Tech Stack:** Rust, ratatui, crossterm, toml crate (new dep), existing proxy-admin-api DTOs.

---

## File Structure

### New files
- `crates/proxy-tui/src/wizard.rs` — wizard state machine + rendering helpers
- `crates/proxy-tui/src/config_writer.rs` — dual-mode (Admin API / TOML file) config read/write

### Modified files
- `crates/proxy-admin-api/src/lib.rs` — extend `ConfigPayload` with quota, affinity, DB paths; add new DTOs
- `crates/proxy-tui/Cargo.toml` — add `toml` dependency
- `crates/proxy-tui/src/app.rs` — new enums (`AppMode`, `ConfigSection`), new `View` variant, new modal variants, wizard state
- `crates/proxy-tui/src/ui.rs` — wizard screens, Config tab with section switching, new form modals
- `crates/proxy-tui/src/main.rs` — startup mode detection, wizard integration, config section key handling
- `crates/proxy/src/application/use_cases/admin.rs` — update `config_to_payload` / `payload_to_config` for new fields
- `crates/proxy/src/frameworks/admin.rs` — minor: may need to handle expanded payload

---

## Task 1: Extend `ConfigPayload` with Quota, Affinity, and DB Paths

The current `ConfigPayload` only carries `port`, `providers`, and `routing`. We extend it to carry the full config so the TUI can edit everything through one DTO.

**Files:**
- Modify: `crates/proxy-admin-api/src/lib.rs`
- Test: `crates/proxy-admin-api/src/lib.rs` (inline tests)

- [ ] **Step 1: Add new DTO types to `proxy-admin-api`**

Add these types after the existing DTOs in `crates/proxy-admin-api/src/lib.rs`:

```rust
/// Affinity config for conversation-affinity hashing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AffinityPayload {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_affinity_headers")]
    pub headers: Vec<String>,
}

fn default_true() -> bool {
    true
}

fn default_affinity_headers() -> Vec<String> {
    vec![
        "x-session-id".into(),
        "x-request-id".into(),
        "x-api-key".into(),
    ]
}

/// A single quota rule.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QuotaPayload {
    pub provider: String,
    pub window: String,
    #[serde(default)]
    pub max_requests: Option<u64>,
    #[serde(default)]
    pub max_input_tokens: Option<u64>,
    #[serde(default)]
    pub max_output_tokens: Option<u64>,
    #[serde(default = "default_warn_pct")]
    pub warn_pct: u8,
}

fn default_warn_pct() -> u8 {
    80
}
```

- [ ] **Step 2: Extend `ConfigPayload` with new fields**

Change the `ConfigPayload` struct to include quota, affinity, proxy_db, pricing_db:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigPayload {
    pub port: u16,
    #[serde(default)]
    pub providers: Vec<ProviderPayload>,
    #[serde(default)]
    pub routing: Vec<RoutingRulePayload>,
    #[serde(default)]
    pub quota: Vec<QuotaPayload>,
    #[serde(default)]
    pub affinity: AffinityPayload,
    #[serde(default)]
    pub proxy_db: Option<String>,
    #[serde(default)]
    pub pricing_db: Option<String>,
}
```

Add `#[serde(default)]` to the existing `providers` and `routing` fields too, so partial TOML files parse correctly.

- [ ] **Step 3: Add TOML round-trip test**

Add a test at the bottom of `crates/proxy-admin-api/src/lib.rs` (or in an existing test module):

```rust
#[cfg(test)]
mod config_payload_tests {
    use super::*;

    #[test]
    fn config_payload_round_trips_through_toml() {
        let payload = ConfigPayload {
            port: 8787,
            providers: vec![ProviderPayload {
                name: "anthropic".into(),
                kind: "anthropic".into(),
                auth: AuthPayload::Passthrough,
                base_url: None,
                openai_base_url: None,
            }],
            routing: vec![],
            quota: vec![QuotaPayload {
                provider: "zai".into(),
                window: "rolling:1h".into(),
                max_requests: Some(100),
                max_input_tokens: None,
                max_output_tokens: None,
                warn_pct: 80,
            }],
            affinity: AffinityPayload {
                enabled: true,
                headers: vec!["x-session-id".into()],
            },
            proxy_db: None,
            pricing_db: None,
        };
        let toml_str = toml::to_string_pretty(&payload).unwrap();
        let parsed: ConfigPayload = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.port, 8787);
        assert_eq!(parsed.providers.len(), 1);
        assert_eq!(parsed.quota.len(), 1);
        assert_eq!(parsed.quota[0].provider, "zai");
        assert!(parsed.affinity.enabled);
    }

    #[test]
    fn config_payload_defaults_when_empty_toml() {
        let toml_str = "port = 8787\n";
        let parsed: ConfigPayload = toml::from_str(toml_str).unwrap();
        assert!(parsed.providers.is_empty());
        assert!(parsed.routing.is_empty());
        assert!(parsed.quota.is_empty());
        assert!(parsed.affinity.enabled);
    }
}
```

Add `toml` as a dev-dependency in `crates/proxy-admin-api/Cargo.toml`:

```toml
[dev-dependencies]
toml = { workspace = true }
```

- [ ] **Step 4: Run tests to verify**

Run: `cargo test -p proxy-admin-api`
Expected: All tests pass, including the two new tests.

- [ ] **Step 5: Commit**

```bash
git add crates/proxy-admin-api/
git commit -m "feat(admin-api): extend ConfigPayload with quota, affinity, DB paths"
```

---

## Task 2: Update Proxy Conversion Functions

Update `config_to_payload` and `payload_to_config` in the proxy to round-trip the new fields.

**Files:**
- Modify: `crates/proxy/src/application/use_cases/admin.rs`

- [ ] **Step 1: Update `config_to_payload`**

In `crates/proxy/src/application/use_cases/admin.rs`, update the `config_to_payload` function to include quota, affinity, proxy_db, pricing_db:

```rust
fn config_to_payload(c: &Config) -> ConfigPayload {
    ConfigPayload {
        port: c.port,
        providers: c
            .providers
            .iter()
            .map(|p| ProviderPayload {
                name: p.name.clone(),
                kind: kind_to_str(p.kind).into(),
                auth: auth_to_payload(&p.auth),
                base_url: p.base_url.clone(),
                openai_base_url: p.openai_base_url.clone(),
            })
            .collect(),
        routing: c
            .routing
            .iter()
            .map(|r| RoutingRulePayload {
                r#match: MatchPayload {
                    model: r.match_spec.model.clone(),
                },
                provider: r.provider.clone(),
                fallback: r.fallback.clone(),
                strategy: strategy_to_payload(r.strategy),
                priority: r.priority,
            })
            .collect(),
        quota: c
            .quota
            .iter()
            .map(|q| proxy_admin_api::QuotaPayload {
                provider: q.provider.clone(),
                window: q.window.clone(),
                max_requests: q.max_requests,
                max_input_tokens: q.max_input_tokens,
                max_output_tokens: q.max_output_tokens,
                warn_pct: q.warn_pct,
            })
            .collect(),
        affinity: proxy_admin_api::AffinityPayload {
            enabled: c.affinity.enabled,
            headers: c.affinity.headers.clone(),
        },
        proxy_db: Some(c.proxy_db.to_string_lossy().into_owned()),
        pricing_db: Some(c.pricing_db.to_string_lossy().into_owned()),
    }
}
```

- [ ] **Step 2: Update `payload_to_config`**

Replace the existing `payload_to_config` function. Remove the "preserve existing" comments for quota and affinity since they are now round-tripped:

```rust
fn payload_to_config(
    p: ConfigPayload,
    existing: &Config,
) -> Result<Config, ProxyError> {
    let providers = p
        .providers
        .into_iter()
        .map(|pp| {
            Ok(ProviderConfig {
                name: pp.name,
                kind: str_to_kind(&pp.kind)?,
                auth: payload_to_auth(pp.auth),
                base_url: pp.base_url,
                openai_base_url: pp.openai_base_url,
            })
        })
        .collect::<Result<Vec<_>, ProxyError>>()?;
    let routing = p
        .routing
        .into_iter()
        .map(|rr| RoutingRule {
            match_spec: MatchSpec {
                model: rr.r#match.model,
            },
            provider: rr.provider,
            fallback: rr.fallback,
            strategy: payload_to_strategy(rr.strategy),
            priority: rr.priority,
        })
        .collect();
    let quota = p
        .quota
        .into_iter()
        .map(|qp| crate::config::QuotaRule {
            provider: qp.provider,
            window: qp.window,
            max_requests: qp.max_requests,
            max_input_tokens: qp.max_input_tokens,
            max_output_tokens: qp.max_output_tokens,
            warn_pct: qp.warn_pct,
        })
        .collect();
    let proxy_db = p
        .proxy_db
        .map(PathBuf::from)
        .unwrap_or_else(|| existing.proxy_db.clone());
    let pricing_db = p
        .pricing_db
        .map(PathBuf::from)
        .unwrap_or_else(|| existing.pricing_db.clone());
    Ok(Config {
        port: p.port,
        proxy_db,
        pricing_db,
        providers,
        routing,
        affinity: crate::config::AffinityConfig {
            enabled: p.affinity.enabled,
            headers: p.affinity.headers,
        },
        quota,
    })
}
```

Note: the function signature changes — `proxy_db` and `pricing_db` are no longer separate params. Update all call sites.

- [ ] **Step 3: Update call sites**

In `UpdateConfig::execute`, update the call:

```rust
pub fn execute(&self, payload: ConfigPayload) -> Result<(), ProxyError> {
    let existing = self.config.read().expect("config rwlock poisoned").clone();
    let new_cfg = payload_to_config(payload, &existing)?;
    // ... rest unchanged ...
}
```

Search for all other calls to `payload_to_config` and update their signatures.

- [ ] **Step 4: Update existing tests**

Update any test that constructs `payload_to_config(...)` calls to use the new 2-arg signature. Update any test that checks `roundtripped.quota` to expect the quota to come from the payload instead of being preserved from existing.

- [ ] **Step 5: Run tests to verify**

Run: `cargo test -p proxy`
Expected: All tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/proxy/
git commit -m "feat(proxy): round-trip quota, affinity, DB paths through ConfigPayload"
```

---

## Task 3: Add `toml` Dependency to `proxy-tui`

**Files:**
- Modify: `crates/proxy-tui/Cargo.toml`

- [ ] **Step 1: Add `toml` to dependencies**

Add to `crates/proxy-tui/Cargo.toml` under `[dependencies]`:

```toml
toml = { workspace = true }
```

- [ ] **Step 2: Verify build**

Run: `cargo check -p proxy-tui`
Expected: Compiles successfully.

- [ ] **Step 3: Commit**

```bash
git add crates/proxy-tui/Cargo.toml
git commit -m "chore(proxy-tui): add toml dependency for offline config editing"
```

---

## Task 4: Create `config_writer.rs` — Dual-Mode Config Read/Write

This module handles reading and writing config in both connected (Admin API) and offline (TOML file) modes.

**Files:**
- Create: `crates/proxy-tui/src/config_writer.rs`
- Modify: `crates/proxy-tui/src/main.rs` (add `mod config_writer;`)

- [ ] **Step 1: Write the failing test**

Create `crates/proxy-tui/src/config_writer.rs` with tests first:

```rust
use proxy_admin_api::ConfigPayload;

/// Read config from the TOML file at the given path.
pub fn read_from_file(path: &std::path::Path) -> Result<ConfigPayload, String> {
    todo!()
}

/// Write config to the TOML file at the given path.
pub fn write_to_file(config: &ConfigPayload, path: &std::path::Path) -> Result<(), String> {
    todo!()
}

/// Detect the resolved config file path (mirrors proxy's `resolved_path` logic).
pub fn resolved_config_path() -> std::path::PathBuf {
    if let Some(path) = std::env::var_os("CLI_ROUTER_CONFIG") {
        return std::path::PathBuf::from(path);
    }
    let profile = std::env::var("CLI_ROUTER_PROFILE").ok();
    let filename = match profile.as_deref() {
        Some("dev") => "config.dev.toml",
        _ => "config.toml",
    };
    dirs_config_dir().join(filename)
}

fn dirs_config_dir() -> std::path::PathBuf {
    // Mirrors proxy's config_dir()
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        std::path::PathBuf::from(xdg)
    } else {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        std::path::PathBuf::from(home).join(".config")
    }
    .join("cli-router")
}

#[cfg(test)]
mod tests {
    use super::*;
    use proxy_admin_api::*;

    fn sample_payload() -> ConfigPayload {
        ConfigPayload {
            port: 8787,
            providers: vec![ProviderPayload {
                name: "test-provider".into(),
                kind: "anthropic".into(),
                auth: AuthPayload::Passthrough,
                base_url: None,
                openai_base_url: None,
            }],
            routing: vec![],
            quota: vec![],
            affinity: AffinityPayload {
                enabled: true,
                headers: vec!["x-session-id".into()],
            },
            proxy_db: None,
            pricing_db: None,
        }
    }

    #[test]
    fn roundtrip_toml_file() {
        let dir = std::env::temp_dir().join("cli-router-test-roundtrip");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("config.toml");

        let original = sample_payload();
        write_to_file(&original, &path).unwrap();
        let loaded = read_from_file(&path).unwrap();

        assert_eq!(loaded.port, original.port);
        assert_eq!(loaded.providers.len(), 1);
        assert_eq!(loaded.providers[0].name, "test-provider");
        assert!(loaded.affinity.enabled);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_from_missing_file_returns_error() {
        let path = std::env::temp_dir().join("nonexistent-config-xyz.toml");
        let result = read_from_file(&path);
        assert!(result.is_err());
    }

    #[test]
    fn write_creates_parent_dirs() {
        let dir = std::env::temp_dir().join("cli-router-test-mkdirs").join("nested");
        let path = dir.join("config.toml");

        let payload = sample_payload();
        write_to_file(&payload, &path).unwrap();
        assert!(path.exists());

        let _ = std::fs::remove_dir_all(
            std::env::temp_dir().join("cli-router-test-mkdirs"),
        );
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p proxy-tui config_writer`
Expected: Compilation error or panic from `todo!()`.

- [ ] **Step 3: Implement the functions**

Replace the `todo!()` bodies:

```rust
use proxy_admin_api::ConfigPayload;

/// Read config from the TOML file at the given path.
pub fn read_from_file(path: &std::path::Path) -> Result<ConfigPayload, String> {
    let s = std::fs::read_to_string(path)
        .map_err(|e| format!("read {}: {e}", path.display()))?;
    toml::from_str(&s).map_err(|e| format!("parse {}: {e}", path.display()))
}

/// Write config to the TOML file at the given path.
pub fn write_to_file(config: &ConfigPayload, path: &std::path::Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create dir {}: {e}", parent.display()))?;
    }
    let toml_str = toml::to_string_pretty(config)
        .map_err(|e| format!("serialize config: {e}"))?;
    std::fs::write(path, toml_str)
        .map_err(|e| format!("write {}: {e}", path.display()))
}

/// Detect the resolved config file path (mirrors proxy's `resolved_path` logic).
pub fn resolved_config_path() -> std::path::PathBuf {
    if let Some(path) = std::env::var_os("CLI_ROUTER_CONFIG") {
        return std::path::PathBuf::from(path);
    }
    let profile = std::env::var("CLI_ROUTER_PROFILE").ok();
    let filename = match profile.as_deref() {
        Some("dev") => "config.dev.toml",
        _ => "config.toml",
    };
    dirs_config_dir().join(filename)
}

fn dirs_config_dir() -> std::path::PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        std::path::PathBuf::from(xdg)
    } else {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        std::path::PathBuf::from(home).join(".config")
    }
    .join("cli-router")
}
```

- [ ] **Step 4: Add `mod config_writer;` to `main.rs`**

Add at the top of `crates/proxy-tui/src/main.rs`:

```rust
mod config_writer;
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p proxy-tui config_writer`
Expected: All 3 tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/proxy-tui/src/config_writer.rs crates/proxy-tui/src/main.rs
git commit -m "feat(proxy-tui): add dual-mode config_writer for TOML file read/write"
```

---

## Task 5: Update App State — `AppMode`, `ConfigSection`, `WizardState`

Add new enums and state fields to `app.rs`.

**Files:**
- Modify: `crates/proxy-tui/src/app.rs`

- [ ] **Step 1: Add `AppMode` enum**

Add after the existing imports/enums in `crates/proxy-tui/src/app.rs`:

```rust
/// Whether the TUI is connected to a running proxy or operating offline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppMode {
    Connected,
    Offline,
}
```

- [ ] **Step 2: Add `ConfigSection` enum**

```rust
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
```

- [ ] **Step 3: Update `View` enum**

Replace:
```rust
pub enum View {
    Status,
    Providers,
    Routing,
    Requests,
    Usage,
    Account,
}
```

With:
```rust
pub enum View {
    Status,
    Config,
    Requests,
    Usage,
    Account,
}
```

- [ ] **Step 4: Update `ALL_VIEWS` constant**

```rust
pub const ALL_VIEWS: &[View] = &[
    View::Status,
    View::Config,
    View::Requests,
    View::Usage,
    View::Account,
];
```

- [ ] **Step 5: Update `View::label()` method**

```rust
impl View {
    pub fn label(self) -> &'static str {
        match self {
            Self::Status => "Status",
            Self::Config => "Config",
            Self::Requests => "Requests",
            Self::Usage => "Usage",
            Self::Account => "Account",
        }
    }
}
```

- [ ] **Step 6: Add new modal variants**

Add `Wizard` and new form modals to the `Modal` enum:

```rust
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
```

- [ ] **Step 7: Add `RoutingFormModal` struct**

```rust
#[derive(Debug, Clone)]
pub struct RoutingFormModal {
    pub mode: FormMode,
    pub focused: RoutingField,
    pub match_model: String,
    pub provider: String,
    pub fallback: String,
    pub strategy: RoutingStrategyPayload,
    pub priority: String,
    pub error: Option<String>,
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

impl RoutingFormModal {
    pub fn new_for_add() -> Self {
        Self {
            mode: FormMode::Add,
            focused: RoutingField::MatchModel,
            match_model: String::new(),
            provider: String::new(),
            fallback: String::new(),
            strategy: RoutingStrategyPayload::default(),
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
```

- [ ] **Step 8: Add `QuotaFormModal` struct**

```rust
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
```

- [ ] **Step 9: Add `WizardState` to `app.rs`**

```rust
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
```

- [ ] **Step 10: Add new fields to `AppState`**

Add fields to `AppState`:

```rust
pub struct AppState {
    // ... existing fields ...
    pub mode: AppMode,
    pub config_section: ConfigSection,
    pub wizard: Option<WizardState>,
    pub routing_selected: usize,
    pub quota_selected: usize,
}
```

Update `AppState::new()` to initialize the new fields:

```rust
pub fn new() -> Self {
    Self {
        // ... existing defaults ...
        mode: AppMode::Offline,
        config_section: ConfigSection::Providers,
        wizard: None,
        routing_selected: 0,
        quota_selected: 0,
    }
}
```

Also update `AppState::default()` similarly.

- [ ] **Step 11: Fix all compile errors from View change**

Search for all references to `View::Providers` and `View::Routing` in `app.rs`, `main.rs`, and `ui.rs`. Replace:
- `View::Providers` → `View::Config`
- `View::Routing` → `View::Config` (with `config_section: ConfigSection::Routing` where needed)

Update `View::label()` match arms. Update tab number key bindings:
- `KeyCode::Char('2')` → `View::Config` (was Providers)
- Remove `KeyCode::Char('3')` for Routing (now part of Config)
- `KeyCode::Char('3')` → `View::Requests` (was 4)
- `KeyCode::Char('4')` → `View::Usage` (was 5)
- `KeyCode::Char('5')` → `View::Account` (was 6)

- [ ] **Step 12: Verify build**

Run: `cargo check -p proxy-tui`
Expected: Compiles with no errors. There will be warnings about unused fields — that's fine, they'll be used in later tasks.

- [ ] **Step 13: Commit**

```bash
git add crates/proxy-tui/src/app.rs
git commit -m "feat(proxy-tui): add AppMode, ConfigSection, WizardState, and new form modals"
```

---

## Task 6: Create `wizard.rs` — First-Run Wizard

**Files:**
- Create: `crates/proxy-tui/src/wizard.rs`
- Modify: `crates/proxy-tui/src/main.rs` (add `mod wizard;`)

- [ ] **Step 1: Create wizard module with state and rendering**

Create `crates/proxy-tui/src/wizard.rs`:

```rust
//! First-run wizard: guides the user through creating a config file when none exists.

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use crate::app::{AppState, WizardState, WizardStep};
use crate::ui::draw_form_modal;

/// Create a new wizard state.
pub fn new_wizard() -> WizardState {
    WizardState {
        step: WizardStep::Welcome,
        form: crate::app::ProviderFormModal::new_for_add(),
        saved_path: None,
    }
}

/// Draw the wizard UI.
pub fn draw_wizard(f: &mut Frame, state: &AppState) {
    let area = centered_rect(f.area(), 60, 70);
    f.render_widget(Clear, area);

    let wizard = match &state.wizard {
        Some(w) => w,
        None => return,
    };

    match wizard.step {
        WizardStep::Welcome => draw_welcome(f, area),
        WizardStep::AddProvider => draw_add_provider(f, area, &wizard.form),
        WizardStep::Done => draw_done(f, area, &wizard.saved_path),
    }
}

fn draw_welcome(f: &mut Frame, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Welcome to cli-router ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let text = vec![
        Line::from(""),
        Line::from(Span::styled(
            "No config found. Let's set one up.",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("This wizard will help you:"),
        Line::from("  \u{2022} Add your first LLM provider"),
        Line::from("  \u{2022} Configure API authentication"),
        Line::from(""),
        Line::from("Config will be saved to:"),
        Line::from(Span::styled(
            format!(
                "  {}",
                crate::config_writer::resolved_config_path().display()
            ),
            Style::default().fg(Color::Cyan),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Press Enter to start",
            Style::default().add_modifier(Modifier::BOLD).fg(Color::Green),
        )),
        Line::from(Span::styled(
            "  Press q to quit",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    let paragraph = Paragraph::new(text)
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true });
    f.render_widget(paragraph, inner);
}

fn draw_add_provider(f: &mut Frame, area: Rect, form: &crate::app::ProviderFormModal) {
    // Delegate to the existing form modal drawing with a custom title.
    // We use the same draw_form_modal but with wizard branding.
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Setup: Add Provider (1/2) ");
    f.render_widget(block, area);

    // Draw the form fields inside the block's inner area.
    // We reuse the existing draw_form_modal helper by wrapping it.
    let inner = block.inner(area);
    draw_form_modal_fields(f, inner, form);
}

/// Draw form fields (extracted from ui.rs for reuse).
/// This is a thin wrapper that calls into the existing form rendering.
fn draw_form_modal_fields(
    f: &mut Frame,
    area: Rect,
    form: &crate::app::ProviderFormModal,
) {
    // Create a temporary Modal so we can reuse draw_form_modal.
    // We do this by calling a helper that takes the form directly.
    // For now, we'll inline a simplified version that matches the style.
    use crate::app::FormField;
    use ratatui::widgets::Paragraph;

    let rows = Layout::vertical([
        Constraint::Length(1), // error
        Constraint::Length(2), // name
        Constraint::Length(1), // kind
        Constraint::Length(2), // auth kind
        Constraint::Length(2), // auth value (conditional)
        Constraint::Length(2), // base_url
        Constraint::Length(2), // buttons
    ])
    .split(area);

    // Error line
    if let Some(err) = &form.error {
        let err_text = Paragraph::new(Span::styled(err, Style::default().fg(Color::Red)));
        f.render_widget(err_text, rows[0]);
    }

    // Name field
    draw_field(f, rows[1], "Name:", &form.name, form.focused == FormField::Name);

    // Kind field (display only, cycle with keys)
    let kind_str = format!("{:?}", form.kind);
    draw_field(f, rows[2], "Kind:", &kind_str, form.focused == FormField::Kind);

    // Auth kind
    let auth_str = form.auth_kind.label().to_string();
    draw_field(
        f,
        rows[3],
        "Auth:",
        &auth_str,
        form.focused == FormField::AuthKind,
    );

    // Auth value (only if not Passthrough)
    if !matches!(form.auth_kind, crate::app::AuthInputKind::Passthrough) {
        draw_field(
            f,
            rows[4],
            "Key:",
            &"*".repeat(form.auth_value.len()),
            form.focused == FormField::AuthValue,
        );
    }

    // Base URL
    draw_field(
        f,
        rows[5],
        "Base URL:",
        &if form.base_url.is_empty() {
            "(default)".into()
        } else {
            form.base_url.clone()
        },
        form.focused == FormField::BaseUrl,
    );

    // Buttons
    let btns = Paragraph::new(Line::from(vec![
        Span::styled(
            " [Save] ",
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Green),
        ),
        Span::raw("  "),
        Span::styled(
            " [Skip - add later] ",
            Style::default().fg(Color::DarkGray),
        ),
    ]));
    f.render_widget(btns, rows[6]);
}

fn draw_field(f: &mut Frame, area: Rect, label: &str, value: &str, focused: bool) {
    let style = if focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    let text = if value.is_empty() {
        format!("{label} ")
    } else {
        format!("{label} {value}")
    };
    let paragraph = Paragraph::new(Span::styled(text, style));
    f.render_widget(paragraph, area);
}

fn draw_done(f: &mut Frame, area: Rect, saved_path: &Option<std::path::PathBuf>) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Setup Complete ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let path_str = saved_path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "config file".into());

    let text = vec![
        Line::from(""),
        Line::from(Span::styled(
            "\u{2713} Config saved!",
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Green),
        )),
        Line::from(""),
        Line::from(format!("  {path_str} created.")),
        Line::from(""),
        Line::from("Next steps:"),
        Line::from("  \u{2022} Start the proxy: cli-router-proxy"),
        Line::from("  \u{2022} Or re-run TUI to manage config"),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                " [E] Exit ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw("   "),
            Span::styled(
                " [T] Open TUI ",
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .fg(Color::Green),
            ),
        ]),
    ];
    let paragraph = Paragraph::new(text)
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true });
    f.render_widget(paragraph, inner);
}

/// Center a rect within the given area.
fn centered_rect(area: Rect, percent_x: u16, percent_y: u16) -> Rect {
    let popup_layout = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .split(area);

    Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .split(popup_layout[1])[1]
}
```

- [ ] **Step 2: Add `mod wizard;` to `main.rs`**

Add to the module declarations in `crates/proxy-tui/src/main.rs`:

```rust
mod wizard;
```

- [ ] **Step 3: Verify build**

Run: `cargo check -p proxy-tui`
Expected: Compiles (may have warnings about unused functions — they'll be used in Task 8).

- [ ] **Step 4: Commit**

```bash
git add crates/proxy-tui/src/wizard.rs crates/proxy-tui/src/main.rs
git commit -m "feat(proxy-tui): add first-run wizard rendering module"
```

---

## Task 7: Update `ui.rs` — Config Tab with Sections

Update the main UI drawing code to render the Config tab with section sub-tabs, and handle the new layout.

**Files:**
- Modify: `crates/proxy-tui/src/ui.rs`

- [ ] **Step 1: Update `draw_tabs` to use new View enum**

Find the `draw_tabs` function. Update it to render the new tab list:

Replace `Status | Providers | Routing | Requests | Usage | Account` with `Status | Config | Requests | Usage | Account`. The numbers shift from 1–6 to 1–5.

- [ ] **Step 2: Update `draw` to dispatch to `draw_config` instead of `draw_providers`/`draw_routing`**

In the main `draw` function, replace the match arms for `View::Providers` and `View::Routing` with a single `View::Config` arm that calls `draw_config`.

- [ ] **Step 3: Add `draw_config` function**

```rust
pub fn draw_config(f: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default().borders(Borders::ALL).title(" Config ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.height == 0 {
        return;
    }

    // Section tabs row
    let tabs_area = Rect::new(inner.x, inner.y, inner.width, 1);
    draw_config_sections(f, tabs_area, state.config_section);

    if inner.height < 3 {
        return;
    }
    let content_area = Rect::new(inner.x, inner.y + 2, inner.width, inner.height - 2);

    match state.config_section {
        ConfigSection::Providers => draw_providers_content(f, content_area, state),
        ConfigSection::Routing => draw_routing_content(f, content_area, state),
        ConfigSection::Quotas => draw_quotas_content(f, content_area, state),
        ConfigSection::Settings => draw_settings_content(f, content_area, state),
    }
}
```

- [ ] **Step 4: Add `draw_config_sections`**

```rust
fn draw_config_sections(f: &mut Frame, area: Rect, active: ConfigSection) {
    use ConfigSection::*;
    let tabs: Vec<Span> = ALL
        .iter()
        .flat_map(|s| {
            let style = if *s == active {
                Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else {
                Style::default()
            };
            let label = match s {
                Providers => " Providers ",
                Routing => " Routing ",
                Quotas => " Quotas ",
                Settings => " Settings ",
            };
            vec![Span::styled(label, style), Span::raw(" ")]
        })
        .collect();
    let paragraph = Paragraph::new(Line::from(tabs));
    f.render_widget(paragraph, area);
}
```

- [ ] **Step 5: Add placeholder section drawing functions**

```rust
fn draw_providers_content(f: &mut Frame, area: Rect, state: &AppState) {
    // Reuse existing provider table drawing logic (extracted from draw_providers)
    // Keep the same table header and rows, but without the outer Block (already drawn by draw_config)
    let toolbar_area = Rect::new(area.x, area.y, area.width, 1);
    draw_provider_toolbar(f, toolbar_area);

    if area.height < 3 {
        return;
    }
    let table_area = Rect::new(area.x, area.y + 2, area.width, area.height - 2);

    let cfg = match &state.config {
        None => return draw_message(f, table_area, "loading\u{2026}"),
        Some(Err(e)) => return draw_error(f, table_area, e),
        Some(Ok(c)) => c,
    };
    if cfg.providers.is_empty() {
        return draw_message(f, table_area, "(no providers configured)");
    }

    // ... same table rendering as before (copy from existing draw_providers) ...
}

fn draw_routing_content(f: &mut Frame, area: Rect, state: &AppState) {
    let toolbar_area = Rect::new(area.x, area.y, area.width, 1);
    draw_routing_toolbar(f, toolbar_area);

    if area.height < 3 {
        return;
    }
    let table_area = Rect::new(area.x, area.y + 2, area.width, area.height - 2);

    let cfg = match &state.config {
        None => return draw_message(f, table_area, "loading\u{2026}"),
        Some(Err(e)) => return draw_error(f, table_area, e),
        Some(Ok(c)) => c,
    };
    if cfg.routing.is_empty() {
        return draw_message(f, table_area, "(no routing rules)");
    }

    // Same table as existing draw_routing, but with selection highlight
    let header = Row::new(vec![
        Cell::from("#"),
        Cell::from("match.model"),
        Cell::from("provider"),
        Cell::from("fallback"),
        Cell::from("strategy"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    let rows: Vec<Row> = cfg
        .routing
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let style = if i == state.routing_selected {
                Style::default().fg(Color::Black).bg(Color::Yellow)
            } else {
                Style::default()
            };
            Row::new(vec![
                Cell::from(format!("{}", i + 1)),
                Cell::from(r.r#match.model.clone().unwrap_or_else(|| "*".into())),
                Cell::from(r.provider.clone()),
                Cell::from(r.fallback.join(", ")),
                Cell::from(format!("{:?}", r.strategy).to_lowercase()),
            ])
            .style(style)
        })
        .collect();

    let widths = [
        Constraint::Length(4),
        Constraint::Length(20),
        Constraint::Length(20),
        Constraint::Min(20),
        Constraint::Length(12),
    ];
    let table = Table::new(rows, widths).header(header);
    f.render_widget(table, table_area);
}

fn draw_routing_toolbar(f: &mut Frame, area: Rect) {
    let spans = Line::from(vec![
        Span::styled("[a]dd ", Style::default().fg(Color::Green)),
        Span::styled("[e]dit ", Style::default().fg(Color::Yellow)),
        Span::styled("[d]elete ", Style::default().fg(Color::Red)),
    ]);
    f.render_widget(Paragraph::new(spans), area);
}

fn draw_quotas_content(f: &mut Frame, area: Rect, state: &AppState) {
    let toolbar_area = Rect::new(area.x, area.y, area.width, 1);
    draw_quota_toolbar(f, toolbar_area);

    if area.height < 3 {
        return;
    }
    let table_area = Rect::new(area.x, area.y + 2, area.width, area.height - 2);

    let cfg = match &state.config {
        None => return draw_message(f, table_area, "loading\u{2026}"),
        Some(Err(e)) => return draw_error(f, table_area, e),
        Some(Ok(c)) => c,
    };
    if cfg.quota.is_empty() {
        return draw_message(f, table_area, "(no quota rules)");
    }

    let header = Row::new(vec![
        Cell::from("#"),
        Cell::from("provider"),
        Cell::from("window"),
        Cell::from("max_req"),
        Cell::from("max_in"),
        Cell::from("max_out"),
        Cell::from("warn%"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    let rows: Vec<Row> = cfg
        .quota
        .iter()
        .enumerate()
        .map(|(i, q)| {
            let style = if i == state.quota_selected {
                Style::default().fg(Color::Black).bg(Color::Yellow)
            } else {
                Style::default()
            };
            Row::new(vec![
                Cell::from(format!("{}", i + 1)),
                Cell::from(q.provider.clone()),
                Cell::from(q.window.clone()),
                Cell::from(q.max_requests.map(|v| v.to_string()).unwrap_or_default()),
                Cell::from(q.max_input_tokens.map(|v| v.to_string()).unwrap_or_default()),
                Cell::from(q.max_output_tokens.map(|v| v.to_string()).unwrap_or_default()),
                Cell::from(q.warn_pct.to_string()),
            ])
            .style(style)
        })
        .collect();

    let widths = [
        Constraint::Length(4),
        Constraint::Length(14),
        Constraint::Length(14),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(8),
    ];
    let table = Table::new(rows, widths).header(header);
    f.render_widget(table, table_area);
}

fn draw_quota_toolbar(f: &mut Frame, area: Rect) {
    let spans = Line::from(vec![
        Span::styled("[a]dd ", Style::default().fg(Color::Green)),
        Span::styled("[e]dit ", Style::default().fg(Color::Yellow)),
        Span::styled("[d]elete ", Style::default().fg(Color::Red)),
    ]);
    f.render_widget(Paragraph::new(spans), area);
}

fn draw_settings_content(f: &mut Frame, area: Rect, state: &AppState) {
    let cfg = match &state.config {
        None => return draw_message(f, area, "loading\u{2026}"),
        Some(Err(e)) => return draw_error(f, area, e),
        Some(Ok(c)) => c,
    };

    let lines = vec![
        Line::from(format!("Port:        {}", cfg.port)),
        Line::from(format!(
            "Proxy DB:    {}",
            cfg.proxy_db.as_deref().unwrap_or("(default)")
        )),
        Line::from(format!(
            "Pricing DB:  {}",
            cfg.pricing_db.as_deref().unwrap_or("(default)")
        )),
        Line::from(format!(
            "Affinity:    {}",
            if cfg.affinity.enabled { "enabled" } else { "disabled" }
        )),
        Line::from(format!(
            "Headers:     {}",
            cfg.affinity.headers.join(", ")
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Press [e] to edit settings",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    let paragraph = Paragraph::new(lines);
    f.render_widget(paragraph, area);
}
```

- [ ] **Step 6: Remove old `draw_providers` and `draw_routing` functions**

Delete or replace the old `draw_providers` and `draw_routing` functions. The content is now in `draw_providers_content` and `draw_routing_content`.

- [ ] **Step 7: Update wizard modal dispatch in `draw`**

In the main `draw` function, add handling for the `Modal::Wizard` variant:

```rust
if let Some(ref wizard) = state.wizard {
    crate::wizard::draw_wizard(f, state);
    return;
}
```

This goes before the normal view rendering — if the wizard is active, it takes over the screen.

- [ ] **Step 8: Add modal rendering for new form types**

In the modal rendering section of `draw`, add cases for `Modal::RoutingForm` and `Modal::QuotaForm`. These follow the same pattern as `Modal::ProviderForm` — centered modal with form fields.

- [ ] **Step 9: Verify build**

Run: `cargo check -p proxy-tui`
Expected: Compiles.

- [ ] **Step 10: Commit**

```bash
git add crates/proxy-tui/src/ui.rs
git commit -m "feat(proxy-tui): add Config tab with section sub-tabs for providers/routing/quotas/settings"
```

---

## Task 8: Update `main.rs` — Startup Flow, Wizard, and Config Section Key Handling

Wire up the wizard activation, mode detection, and key bindings for the new Config tab sections.

**Files:**
- Modify: `crates/proxy-tui/src/main.rs`

- [ ] **Step 1: Update `main` function for mode detection**

Replace the beginning of `main()` with mode detection logic:

```rust
fn main() -> std::io::Result<()> {
    let config_path = config_writer::resolved_config_path();
    let config_exists = config_path.exists();

    let base =
        std::env::var("CLI_ROUTER_PROXY_URL").unwrap_or_else(|_| "http://127.0.0.1:8787".into());
    let client = AdminClient::new(base);
    let mut state = AppState::new();

    // Try connecting to the proxy.
    let connected = client.get_status().is_ok();
    state.mode = if connected {
        AppMode::Connected
    } else {
        AppMode::Offline
    };

    if connected {
        refresh_all(&client, &mut state);
    } else if config_exists {
        // Offline mode: read config from file.
        match config_writer::read_from_file(&config_path) {
            Ok(cfg) => {
                state.config = Some(Ok(cfg));
            }
            Err(e) => {
                state.config = Some(Err(e));
            }
        }
    } else {
        // No config, no proxy — launch wizard.
        state.wizard = Some(crate::wizard::new_wizard());
    }

    // ... rest of main (terminal setup, event loop) unchanged ...
}
```

- [ ] **Step 2: Update `refresh_all` to handle offline mode**

```rust
fn refresh_all(client: &AdminClient, state: &mut AppState) {
    state.set_status(client.get_status().map_err(|e| e.to_string()));
    state.set_config(client.get_config().map_err(|e| e.to_string()));
    state.set_recent(client.get_recent(50, 0).map_err(|e| e.to_string()));
    state.quota = Some(client.get_quota_status().map_err(|e| e.to_string()));
}

fn refresh_offline(state: &mut AppState) {
    let path = config_writer::resolved_config_path();
    match config_writer::read_from_file(&path) {
        Ok(cfg) => {
            state.config = Some(Ok(cfg));
            state.status = None;
            state.flash("reloaded config from file");
        }
        Err(e) => {
            state.config = Some(Err(e));
        }
    }
}
```

- [ ] **Step 3: Add wizard key handling**

Add a new function for wizard key events:

```rust
fn handle_wizard_key(k: KeyEvent, state: &mut AppState) {
    let wizard = match &mut state.wizard {
        Some(w) => w,
        None => return,
    };

    match wizard.step {
        WizardStep::Welcome => match k.code {
            KeyCode::Enter => {
                wizard.step = WizardStep::AddProvider;
                wizard.form = crate::app::ProviderFormModal::new_for_add();
            }
            KeyCode::Char('q') => state.should_quit = true,
            _ => {}
        },
        WizardStep::AddProvider => match k.code {
            KeyCode::Char('s') | KeyCode::Enter => {
                // Save: validate and write config
                if let Some(config) = build_wizard_config(&wizard.form) {
                    let path = config_writer::resolved_config_path();
                    match config_writer::write_to_file(&config, &path) {
                        Ok(()) => {
                            wizard.saved_path = Some(path);
                            wizard.step = WizardStep::Done;
                        }
                        Err(e) => {
                            wizard.form.error = Some(e);
                        }
                    }
                }
            }
            KeyCode::Char('S') => {
                // Skip: save minimal config with no providers
                let config = minimal_config();
                let path = config_writer::resolved_config_path();
                match config_writer::write_to_file(&config, &path) {
                    Ok(()) => {
                        wizard.saved_path = Some(path);
                        wizard.step = WizardStep::Done;
                    }
                    Err(e) => {
                        wizard.form.error = Some(e);
                    }
                }
            }
            KeyCode::Esc => state.should_quit = true,
            _ => {
                // Delegate to form key handling (reuse handle_form_key pattern)
                handle_wizard_form_key(k, &mut wizard.form);
            }
        },
        WizardStep::Done => match k.code {
            KeyCode::Char('e') | KeyCode::Char('q') => state.should_quit = true,
            KeyCode::Char('t') | KeyCode::Enter => {
                // Exit wizard, enter normal TUI
                state.wizard = None;
                let path = config_writer::resolved_config_path();
                match config_writer::read_from_file(&path) {
                    Ok(cfg) => state.config = Some(Ok(cfg)),
                    Err(e) => state.config = Some(Err(e)),
                }
            }
            _ => {}
        },
    }
}

fn handle_wizard_form_key(k: KeyEvent, form: &mut crate::app::ProviderFormModal) {
    use crate::app::FormField;
    match k.code {
        KeyCode::Tab => form.focused = form.focused.next(),
        KeyCode::BackTab => form.focused = form.focused.prev(),
        KeyCode::Char(c) => match form.focused {
            FormField::Kind => form.kind = form.kind.cycle_next(),
            FormField::AuthKind => form.auth_kind = form.auth_kind.cycle(),
            FormField::AuthValue => {
                form.auth_value.push(c);
                form.error = None;
            }
            FormField::Name => {
                form.name.push(c);
                form.error = None;
            }
            FormField::BaseUrl => {
                form.base_url.push(c);
                form.error = None;
            }
        },
        KeyCode::Backspace => match form.focused {
            FormField::Kind | FormField::AuthKind => {}
            FormField::Name => { form.name.pop(); form.error = None; }
            FormField::AuthValue => { form.auth_value.pop(); form.error = None; }
            FormField::BaseUrl => { form.base_url.pop(); form.error = None; }
        },
        _ => {}
    }
}

fn build_wizard_config(form: &crate::app::ProviderFormModal) -> Option<proxy_admin_api::ConfigPayload> {
    if form.name.trim().is_empty() {
        // At least require a name
        return None;
    }
    let auth = match form.auth_kind {
        crate::app::AuthInputKind::Passthrough => proxy_admin_api::AuthPayload::Passthrough,
        crate::app::AuthInputKind::ApiKey => proxy_admin_api::AuthPayload::ApiKey {
            value: form.auth_value.clone(),
        },
        crate::app::AuthInputKind::Bearer => proxy_admin_api::AuthPayload::Bearer {
            value: form.auth_value.clone(),
        },
    };
    Some(proxy_admin_api::ConfigPayload {
        port: 8787,
        providers: vec![proxy_admin_api::ProviderPayload {
            name: form.name.clone(),
            kind: format!("{:?}", form.kind).to_lowercase(),
            auth,
            base_url: if form.base_url.is_empty() { None } else { Some(form.base_url.clone()) },
            openai_base_url: None,
        }],
        routing: vec![],
        quota: vec![],
        affinity: proxy_admin_api::AffinityPayload {
            enabled: true,
            headers: vec![
                "x-session-id".into(),
                "x-request-id".into(),
                "x-api-key".into(),
            ],
        },
        proxy_db: None,
        pricing_db: None,
    })
}

fn minimal_config() -> proxy_admin_api::ConfigPayload {
    proxy_admin_api::ConfigPayload {
        port: 8787,
        providers: vec![],
        routing: vec![],
        quota: vec![],
        affinity: proxy_admin_api::AffinityPayload {
            enabled: true,
            headers: vec![
                "x-session-id".into(),
                "x-request-id".into(),
                "x-api-key".into(),
            ],
        },
        proxy_db: None,
        pricing_db: None,
    }
}
```

- [ ] **Step 4: Update `handle_key` to dispatch to wizard**

At the top of `handle_key`, before any other handling:

```rust
fn handle_key(k: KeyEvent, client: &AdminClient, state: &mut AppState, term_area: Rect) {
    if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
        state.should_quit = true;
        return;
    }

    // Wizard takes over all key handling.
    if state.wizard.is_some() {
        handle_wizard_key(k, state);
        return;
    }

    // ... rest of handle_key unchanged ...
}
```

- [ ] **Step 5: Add Config-section key handling**

In `handle_key`, replace the `View::Providers` specific key bindings with `View::Config` section-aware bindings:

```rust
// Inside the "On all other tabs" section of handle_key:
KeyCode::Char('2') => state.set_view(View::Config),
KeyCode::Char('3') => state.set_view(View::Requests),
KeyCode::Char('4') => state.set_view(View::Usage),
KeyCode::Char('5') => state.set_view(View::Account),
```

Add section switching when on Config tab:

```rust
// Config-tab-specific section switching
if state.view == View::Config {
    match k.code {
        KeyCode::Char('[') | KeyCode::Left => {
            state.config_section = state.config_section.prev();
            return;
        }
        KeyCode::Char(']') | KeyCode::Right => {
            state.config_section = state.config_section.next();
            return;
        }
        KeyCode::Down | KeyCode::Char('j') => {
            match state.config_section {
                ConfigSection::Providers => state.move_selection_down(),
                ConfigSection::Routing => {
                    if let Some(Ok(cfg)) = &state.config {
                        state.routing_selected =
                            (state.routing_selected + 1).min(cfg.routing.len().saturating_sub(1));
                    }
                }
                ConfigSection::Quotas => {
                    if let Some(Ok(cfg)) = &state.config {
                        state.quota_selected =
                            (state.quota_selected + 1).min(cfg.quota.len().saturating_sub(1));
                    }
                }
                ConfigSection::Settings => {}
            }
            return;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            match state.config_section {
                ConfigSection::Providers => state.move_selection_up(),
                ConfigSection::Routing => {
                    state.routing_selected = state.routing_selected.saturating_sub(1);
                }
                ConfigSection::Quotas => {
                    state.quota_selected = state.quota_selected.saturating_sub(1);
                }
                ConfigSection::Settings => {}
            }
            return;
        }
        // Provider actions (when on Providers section)
        KeyCode::Char('a') if state.config_section == ConfigSection::Providers => {
            open_add_modal(state);
            return;
        }
        KeyCode::Char('t') if state.config_section == ConfigSection::Providers => {
            open_test_modal(state);
            return;
        }
        KeyCode::Char('e') if state.config_section == ConfigSection::Providers => {
            open_edit_modal(state);
            return;
        }
        KeyCode::Char('d') if state.config_section == ConfigSection::Providers => {
            open_delete_modal(state);
            return;
        }
        // Routing actions
        KeyCode::Char('a') if state.config_section == ConfigSection::Routing => {
            state.modal = Modal::RoutingForm(RoutingFormModal::new_for_add());
            return;
        }
        KeyCode::Char('e') if state.config_section == ConfigSection::Routing => {
            if let Some(Ok(cfg)) = &state.config {
                if let Some(rule) = cfg.routing.get(state.routing_selected) {
                    let form =
                        RoutingFormModal::from_rule(state.routing_selected, rule);
                    state.modal = Modal::RoutingForm(form);
                }
            }
            return;
        }
        KeyCode::Char('d') if state.config_section == ConfigSection::Routing => {
            // Delete routing rule
            if let Some(Ok(mut cfg)) = state.config.clone() {
                if state.routing_selected < cfg.routing.len() {
                    cfg.routing.remove(state.routing_selected);
                    save_config_state(client, state, cfg);
                }
            }
            return;
        }
        // Quota actions
        KeyCode::Char('a') if state.config_section == ConfigSection::Quotas => {
            state.modal = Modal::QuotaForm(QuotaFormModal::new_for_add());
            return;
        }
        KeyCode::Char('e') if state.config_section == ConfigSection::Quotas => {
            if let Some(Ok(cfg)) = &state.config {
                if let Some(quota) = cfg.quota.get(state.quota_selected) {
                    let form =
                        QuotaFormModal::from_rule(state.quota_selected, quota);
                    state.modal = Modal::QuotaForm(form);
                }
            }
            return;
        }
        KeyCode::Char('d') if state.config_section == ConfigSection::Quotas => {
            if let Some(Ok(mut cfg)) = state.config.clone() {
                if state.quota_selected < cfg.quota.len() {
                    cfg.quota.remove(state.quota_selected);
                    save_config_state(client, state, cfg);
                }
            }
            return;
        }
        // Settings edit
        KeyCode::Char('e') if state.config_section == ConfigSection::Settings => {
            // Open settings form (could reuse QuotaFormModal pattern or a simple inline edit)
            // For now, cycle the port or affinity toggle as a minimal implementation
            if let Some(Ok(mut cfg)) = state.config.clone() {
                cfg.affinity.enabled = !cfg.affinity.enabled;
                save_config_state(client, state, cfg);
            }
            return;
        }
        _ => {}
    }
    return; // Don't fall through to the general handler
}
```

- [ ] **Step 6: Add `save_config_state` helper**

```rust
fn save_config_state(
    client: &AdminClient,
    state: &mut AppState,
    cfg: proxy_admin_api::ConfigPayload,
) {
    match state.mode {
        AppMode::Connected => match client.put_config(&cfg) {
            Ok(updated) => {
                state.config = Some(Ok(updated));
                state.flash("config saved");
            }
            Err(e) => {
                state.flash = Some(format!("save failed: {e}"));
            }
        },
        AppMode::Offline => {
            let path = config_writer::resolved_config_path();
            match config_writer::write_to_file(&cfg, &path) {
                Ok(()) => {
                    state.config = Some(Ok(cfg));
                    state.flash("config saved to file");
                }
                Err(e) => {
                    state.flash = Some(format!("save failed: {e}"));
                }
            }
        }
    }
}
```

- [ ] **Step 7: Update refresh key for offline mode**

In the `KeyCode::Char('r')` handler, check mode:

```rust
KeyCode::Char('r') => {
    match state.mode {
        AppMode::Connected => {
            refresh_view(client, state);
            state.flash("refreshed");
        }
        AppMode::Offline => {
            refresh_offline(state);
        }
    }
}
```

- [ ] **Step 8: Add routing/quota form key handlers**

Add handlers for `Modal::RoutingForm` and `Modal::QuotaForm` in `handle_modal_key`:

```rust
Modal::RoutingForm(ref mut form) => {
    match k.code {
        KeyCode::Tab => form.focused = form.focused.next(),
        KeyCode::BackTab => form.focused = form.focused.prev(),
        KeyCode::Esc => state.modal = Modal::None,
        KeyCode::Enter | KeyCode::Char('s') => {
            submit_routing_form(client, state, form);
        }
        KeyCode::Char(c) => match form.focused {
            RoutingField::MatchModel => { form.match_model.push(c); form.error = None; }
            RoutingField::Provider => { form.provider.push(c); form.error = None; }
            RoutingField::Fallback => { form.fallback.push(c); form.error = None; }
            RoutingField::Strategy => {
                form.strategy = match form.strategy {
                    RoutingStrategyPayload::Failover => RoutingStrategyPayload::RoundRobin,
                    RoutingStrategyPayload::RoundRobin => RoutingStrategyPayload::Failover,
                };
            }
            RoutingField::Priority => { form.priority.push(c); form.error = None; }
        },
        KeyCode::Backspace => match form.focused {
            RoutingField::MatchModel => { form.match_model.pop(); }
            RoutingField::Provider => { form.provider.pop(); }
            RoutingField::Fallback => { form.fallback.pop(); }
            RoutingField::Strategy => {}
            RoutingField::Priority => { form.priority.pop(); }
        },
        _ => {}
    }
}

Modal::QuotaForm(ref mut form) => {
    match k.code {
        KeyCode::Tab => form.focused = form.focused.next(),
        KeyCode::BackTab => form.focused = form.focused.prev(),
        KeyCode::Esc => state.modal = Modal::None,
        KeyCode::Enter | KeyCode::Char('s') => {
            submit_quota_form(client, state, form);
        }
        KeyCode::Char(c) => match form.focused {
            QuotaField::Provider => { form.provider.push(c); form.error = None; }
            QuotaField::Window => { form.window.push(c); form.error = None; }
            QuotaField::MaxRequests => { form.max_requests.push(c); form.error = None; }
            QuotaField::MaxInputTokens => { form.max_input_tokens.push(c); form.error = None; }
            QuotaField::MaxOutputTokens => { form.max_output_tokens.push(c); form.error = None; }
            QuotaField::WarnPct => { form.warn_pct.push(c); form.error = None; }
        },
        KeyCode::Backspace => match form.focused {
            QuotaField::Provider => { form.provider.pop(); }
            QuotaField::Window => { form.window.pop(); }
            QuotaField::MaxRequests => { form.max_requests.pop(); }
            QuotaField::MaxInputTokens => { form.max_input_tokens.pop(); }
            QuotaField::MaxOutputTokens => { form.max_output_tokens.pop(); }
            QuotaField::WarnPct => { form.warn_pct.pop(); }
        },
        _ => {}
    }
}
```

- [ ] **Step 9: Add `submit_routing_form` and `submit_quota_form`**

```rust
fn submit_routing_form(
    client: &AdminClient,
    state: &mut AppState,
    form: &RoutingFormModal,
) {
    if form.provider.trim().is_empty() {
        // Can't set error on borrowed form, so set via modal mutation
        // (This is called from the match arm above where form is &mut)
        return;
    }

    let rule = proxy_admin_api::RoutingRulePayload {
        r#match: proxy_admin_api::MatchPayload {
            model: if form.match_model.is_empty() {
                None
            } else {
                Some(form.match_model.clone())
            },
        },
        provider: form.provider.clone(),
        fallback: form
            .fallback
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        strategy: form.strategy.clone(),
        priority: form.priority.parse::<u32>().ok(),
    };

    if let Some(Ok(mut cfg)) = state.config.clone() {
        match form.mode {
            FormMode::Add => cfg.routing.push(rule),
            FormMode::Edit { original_index, .. } => {
                if original_index < cfg.routing.len() {
                    cfg.routing[original_index] = rule;
                }
            }
        }
        state.modal = Modal::None;
        save_config_state(client, state, cfg);
    }
}

fn submit_quota_form(
    client: &AdminClient,
    state: &mut AppState,
    form: &QuotaFormModal,
) {
    if form.provider.trim().is_empty() || form.window.trim().is_empty() {
        return;
    }

    let quota = proxy_admin_api::QuotaPayload {
        provider: form.provider.clone(),
        window: form.window.clone(),
        max_requests: form.max_requests.parse().ok(),
        max_input_tokens: form.max_input_tokens.parse().ok(),
        max_output_tokens: form.max_output_tokens.parse().ok(),
        warn_pct: form.warn_pct.parse().unwrap_or(80),
    };

    if let Some(Ok(mut cfg)) = state.config.clone() {
        match form.mode {
            FormMode::Add => cfg.quota.push(quota),
            FormMode::Edit { original_index, .. } => {
                if original_index < cfg.quota.len() {
                    cfg.quota[original_index] = quota;
                }
            }
        }
        state.modal = Modal::None;
        save_config_state(client, state, cfg);
    }
}
```

- [ ] **Step 10: Update `handle_mouse` for new tab numbers**

Update `tab_hit` calls to use new tab indices. The tab positions shift because we now have 5 tabs instead of 6. Update any mouse hit-testing that uses hard-coded tab indices.

- [ ] **Step 11: Verify build**

Run: `cargo check -p proxy-tui`
Expected: Compiles.

- [ ] **Step 12: Run all tests**

Run: `cargo test -p proxy-tui`
Expected: All tests pass.

- [ ] **Step 13: Commit**

```bash
git add crates/proxy-tui/src/main.rs
git commit -m "feat(proxy-tui): wire up wizard, dual-mode config, and Config tab key handling"
```

---

## Task 9: Status Bar for Offline Mode

Show the user whether they're in connected or offline mode.

**Files:**
- Modify: `crates/proxy-tui/src/ui.rs`

- [ ] **Step 1: Update `draw_status_line` to show mode**

In the status line drawing function, add an indicator:

```rust
// In draw_status_line, add mode indicator:
let mode_indicator = if state.mode == AppMode::Offline {
    Span::styled(
        " \u{26a0} Offline \u{2014} editing config.toml directly ",
        Style::default().fg(Color::Yellow),
    )
} else {
    Span::raw("")
};
```

Include this span in the status line rendering.

- [ ] **Step 2: Verify build and commit**

Run: `cargo check -p proxy-tui`
Expected: Compiles.

```bash
git add crates/proxy-tui/src/ui.rs
git commit -m "feat(proxy-tui): show offline mode indicator in status bar"
```

---

## Task 10: End-to-End Testing

Manual and automated verification of the complete feature.

**Files:**
- No new files

- [ ] **Step 1: Run all workspace tests**

Run: `cargo test --workspace`
Expected: All tests pass across all crates.

- [ ] **Step 2: Build all binaries**

Run: `cargo build --workspace`
Expected: All binaries compile successfully.

- [ ] **Step 3: Manual test — wizard flow**

1. Temporarily rename `~/.config/cli-router/config.toml` (if it exists)
2. Run `cargo run -p proxy-tui`
3. Verify: wizard welcome screen appears
4. Press Enter → verify: Add Provider form
5. Fill in a provider name, select kind, enter API key
6. Press Enter → verify: "Config saved!" screen
7. Press T → verify: normal TUI opens with the provider visible in Config tab
8. Restore original config file

- [ ] **Step 4: Manual test — offline config editing**

1. Stop the proxy (if running)
2. Run `cargo run -p proxy-tui`
3. Verify: status bar shows "⚠ Offline"
4. Navigate to Config tab → Providers section
5. Add a provider → verify: config file updated on disk
6. Check sections: Routing, Quotas, Settings all display correctly

- [ ] **Step 5: Manual test — connected config editing**

1. Start the proxy
2. Run `cargo run -p proxy-tui`
3. Verify: status bar does NOT show offline
4. Navigate to Config tab
5. Add/edit/delete providers, routing rules, quotas → verify: changes persist through Admin API

- [ ] **Step 6: Final commit**

```bash
git add -A
git commit -m "test: verify TUI config wizard and full config editor end-to-end"
```
