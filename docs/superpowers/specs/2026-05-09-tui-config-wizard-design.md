# TUI Config Wizard & Full Config Editor

**Date:** 2026-05-09
**Status:** Draft

## Problem

Setting up cli-router requires manually creating `~/.config/cli-router/config.toml` with providers, routing rules, quotas, and other settings. The TUI can edit providers and view routing rules, but only when the proxy daemon is running (via Admin API). There is no first-run experience, and several config sections (quotas, affinity, settings) are not editable in the TUI at all.

## Decision

- **Keep TOML as the source of truth.** The config file is human-readable, diffable, follows XDG conventions, and supports `${ENV}` interpolation for secrets. SQLite is not used for config.
- **Add a first-run wizard** that activates when no config file exists, writing TOML directly to disk.
- **Upgrade the TUI to a full config editor** with CRUD for all config sections (providers, routing, quotas, settings).
- **Dual-mode editing:** connected to proxy → Admin API; not connected → direct TOML file read/write.

## Design

### 1. First-Run Wizard

The wizard runs inside the same Ratatui terminal when the TUI starts and the config file doesn't exist.

**Steps:**

1. **Welcome** — explains what will be configured, shows config file path, press Enter to start.
2. **Add Provider** — reuses the `ProviderFormModal` pattern (name, kind, auth type, API key, base URL). A "Skip" option saves a minimal config with zero providers.
3. **Done** — confirms config saved, shows path, offers two buttons:
   - **Exit** — quit the TUI
   - **Open TUI** — restart into the normal TUI flow (config now exists)

The wizard writes the TOML file directly using the `toml` crate. No proxy connection needed.

**Key details:**
- The wizard reuses existing form component patterns (`ProviderFormModal`) — fields, focus cycling, validation via `validate.rs`.
- "Skip" on step 2 creates a config with empty providers and all defaults. The proxy starts fine with no providers; users can add later via the TUI.
- The wizard is intentionally minimal — just enough to get started. All further configuration happens in the Config tab.

### 2. Full Config Editing — Enhanced TUI Tabs

**Tab changes:**

Current: `Status | Providers | Routing | Requests | Usage | Account`

New: `Status | Config | Requests | Usage | Account`

The existing `Providers` and `Routing` tabs merge into a single `Config` tab with inner section tabs:

```
┌─ Config ──────────────────────────────────────┐
│ [Providers] [Routing] [Quotas] [Settings]     │  ← section tabs
│───────────────────────────────────────────────│
│  name           kind      auth      base_url  │
│  my-anthropic   Anthropic sk-ant••  (default) │
│  my-zai         Zai       •••••••   (default) │
│                                               │
│  [a]dd [e]dit [d]elete [t]est                 │  ← toolbar
└───────────────────────────────────────────────┘
```

**Sections:**

| Section | Behavior |
|---------|----------|
| Providers | Identical to current Providers tab — table, toolbar (add/edit/delete/test), `ProviderFormModal` |
| Routing | Upgraded from read-only to full CRUD — table of rules, `[a]dd [e]dit [d]elete` toolbar, `RoutingFormModal` (match, provider, fallback, strategy, priority) |
| Quotas | New — table of quota rules, `[a]dd [e]dit [d]elete` toolbar, `QuotaFormModal` (provider, window, limits, warn_pct) |
| Settings | New — key-value form for `port`, `proxy_db`, `pricing_db`; affinity toggle + headers list; `[Save]` button |

All form modals follow the existing `ProviderFormModal` pattern: focused field, Tab to cycle, Enter to save, Esc to cancel, inline validation errors.

### 3. Dual-Mode Editing

The TUI detects its mode on startup:

```
TUI starts
  → config file exists?
      YES → try connect to Admin API
              success → AppMode::Connected (edit via Admin API)
              fail    → AppMode::Offline (edit config.toml directly)
      NO  → enter wizard (forced offline)
              wizard completes → write config.toml → offer "Open TUI"
              user picks "Open TUI" → restart into normal flow
```

Write logic:

```rust
fn save_config(&self, config: &Config) -> Result<(), String> {
    match self.mode {
        AppMode::Connected => self.client.put_config(config)?,
        AppMode::Offline => {
            let path = Config::resolved_path();
            let toml_str = toml::to_string_pretty(config)
                .map_err(|e| e.to_string())?;
            std::fs::write(&path, toml_str)
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}
```

Status bar in offline mode: `⚠ Offline mode — editing config.toml directly`

### 4. Architecture & Code Changes

**New files in `proxy-tui`:**

```
crates/proxy-tui/src/
├── wizard.rs          ← wizard state (WizardStep, WizardState) + rendering
├── config_writer.rs   ← dual-mode write logic
├── app.rs             ← MODIFIED: View enum, AppMode, ConfigSection, wizard state
├── ui.rs              ← MODIFIED: wizard screens, Config tab sections, new form modals
├── client.rs          ← UNCHANGED
├── views/
│   ├── mod.rs         ← MODIFIED: RoutingFormModal, QuotaFormModal, SettingsState
│   ├── account.rs     ← UNCHANGED
│   └── usage.rs       ← UNCHANGED
└── validate.rs        ← UNCHANGED: validation reused by wizard and new forms
```

**Key data structures:**

```rust
// wizard.rs
enum WizardStep {
    Welcome,
    AddProvider,
    Done,
}

struct WizardState {
    step: WizardStep,
    form: ProviderFormModal,
    saved_path: Option<PathBuf>,
}
```

```rust
// app.rs additions
enum AppMode {
    Connected,   // proxy reachable → Admin API
    Offline,     // proxy not running → direct TOML
}

enum ConfigSection {
    Providers,
    Routing,
    Quotas,
    Settings,
}

// View changes: Providers + Routing → Config
enum View {
    Status,
    Config,     // replaces Providers + Routing
    Requests,
    Usage,
    Account,
}

struct AppState {
    // ... existing fields ...
    mode: AppMode,
    config_section: ConfigSection,
    wizard: Option<WizardState>,
}
```

**Moving Config types to `proxy-admin-api`:**

The `Config` struct and its sub-types (`ProviderConfig`, `AuthConfig`, `ProviderKind`, `RoutingRule`, `RoutingStrategy`, `MatchSpec`, `QuotaRule`, `AffinityConfig`) move from `crates/proxy/src/config.rs` to `crates/proxy-admin-api/src/`. Rationale:

- They are pure data + serde — the crate's stated purpose ("wire DTOs, pure data + serde, zero logic").
- Both `proxy` and `proxy-tui` already depend on `proxy-admin-api`.
- Validation logic stays in `proxy` (business logic, not a DTO).
- Helper functions (`default_port`, `default_proxy_db`, etc.) move with the structs.
- `from_env`, `resolved_path`, `interpolate_env`, `validate` stay in `proxy` — they are business logic.

### 5. Error Handling

| Scenario | Behavior |
|----------|----------|
| Wizard can't write file | Show error in wizard screen, let user retry |
| Offline config has validation errors | Show inline, same as current provider form errors |
| Admin API unreachable but config exists | Show status bar warning, switch to offline mode |
| TOML parse error in existing config | Show error screen with file path and parse error details |
| File permission denied | Show error with path, suggest checking permissions |

### 6. Testing

- **Unit tests** for `config_writer` module: TOML round-trip (`Config → TOML string → Config` equals original).
- **Unit tests** for wizard state transitions: step forward, step back, cancel, skip.
- **Unit tests** for new form modals (`RoutingFormModal`, `QuotaFormModal`): field cycling, validation.
- Existing TUI tests are unaffected — `ProviderFormModal`, `AuthInputKind`, etc. don't change.

### 7. Scope Exclusions

- No SQLite storage for config.
- No new secrets management — `${ENV}` interpolation remains the mechanism.
- No proxy auto-start from TUI — user starts proxy separately.
- No config file watcher in TUI — TUI reads config once on load, writes on save.
- The `LiveProvider` file-watcher in the proxy is unchanged — it picks up changes made by the TUI's offline writes.
