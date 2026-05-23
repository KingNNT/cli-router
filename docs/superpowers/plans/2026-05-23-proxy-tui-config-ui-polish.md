# Proxy TUI Config UI Polish Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Polish the `proxy-tui` Config tab with clearer section summaries, consistent toolbars, readable labels/formatting, action-oriented empty states, and selected-item hints.

**Architecture:** Keep the existing `crates/proxy-tui/src/ui.rs` rendering structure and add small helper functions near the existing Config rendering functions. No backend/admin API changes. The implementation remains a Ratatui render-only change plus focused buffer-content tests.

**Tech Stack:** Rust 2024, Ratatui, `proxy-admin-api` DTOs, existing `cargo test -p proxy-tui` test harness with `TestBackend`.

---

## File structure

- Modify `crates/proxy-tui/src/ui.rs`
  - Update Config section rendering functions: `draw_config`, `draw_providers_content`, `draw_routing_content`, `draw_quotas_content`, `draw_settings_content`, `draw_provider_toolbar`, `draw_routing_toolbar`, `draw_quotas_toolbar`, and `auth_summary`.
  - Add focused render helpers in the same file near the Config functions:
    - `draw_config_toolbar`
    - `draw_config_hint`
    - `provider_summary`
    - `routing_summary`
    - `quota_summary`
    - `selected_provider_footer`
    - `selected_route_footer`
    - `selected_quota_footer`
    - `format_count`
    - `auth_kind_label`
  - Add tests in the existing `#[cfg(test)] mod tests` in `ui.rs`.
- Read-only reference: `docs/superpowers/specs/2026-05-23-proxy-tui-config-ui-design.md`.

## Baseline verification

Baseline already run before writing this plan:

```bash
cargo test -p proxy-tui
```

Expected/current result:

```text
test result: ok. 44 passed; 0 failed; 0 ignored
```

---

### Task 1: Providers section polish

**Files:**
- Modify: `crates/proxy-tui/src/ui.rs`

- [ ] **Step 1: Add failing Providers render tests**

Add these imports to the existing `#[cfg(test)] mod tests` import block in `crates/proxy-tui/src/ui.rs`:

```rust
use proxy_admin_api::{
    AffinityPayload, AffinityStatus, AuthPayload, ConfigPayload, ProviderPayload, StatusResponse,
};
```

Replace the current single-line import:

```rust
use proxy_admin_api::{AffinityStatus, StatusResponse};
```

with the expanded import above.

Then add these helper functions inside `mod tests`, after `sample_status()`:

```rust
fn empty_config() -> ConfigPayload {
    ConfigPayload {
        port: 8787,
        providers: vec![],
        routing: vec![],
        quota: vec![],
        affinity: AffinityPayload {
            enabled: true,
            headers: vec!["x-session-id".into(), "anthropic-conversation-id".into()],
        },
        proxy_db: None,
        pricing_db: None,
        docs_enabled: false,
        docs_port: 8788,
    }
}

fn config_state(config: ConfigPayload) -> AppState {
    let mut state = AppState::new();
    state.view = View::Config;
    state.mode = AppMode::Online;
    state.config = Some(Ok(config));
    state
}
```

Add these tests in the same test module:

```rust
#[test]
fn config_providers_empty_state_suggests_add_action() {
    let state = config_state(empty_config());

    let rendered = render_state(&state, 120, 28);

    assert!(rendered.contains("No providers configured. Press [a] to add your first provider."));
    assert!(rendered.contains("[a] Add"));
}

#[test]
fn config_providers_render_summary_readable_headers_and_selected_footer() {
    let mut config = empty_config();
    config.providers = vec![
        ProviderPayload {
            name: "anthropic".into(),
            kind: "anthropic".into(),
            auth: AuthPayload::AnthropicOAuth {
                access_token: "access".into(),
                refresh_token: "refresh".into(),
                expires_at_ms: 9_999,
            },
            base_url: Some("https://api.anthropic.com".into()),
            openai_base_url: None,
        },
        ProviderPayload {
            name: "openai".into(),
            kind: "openai".into(),
            auth: AuthPayload::ApiKey {
                value: "sk-test-secret-value".into(),
            },
            base_url: None,
            openai_base_url: Some("https://api.openai.com/v1".into()),
        },
    ];
    let mut state = config_state(config);
    state.providers_selected = 0;

    let rendered = render_state(&state, 140, 30);

    assert!(rendered.contains("Providers: 2 total"));
    assert!(rendered.contains("1 OAuth"));
    assert!(rendered.contains("1 API key"));
    assert!(rendered.contains("Name"));
    assert!(rendered.contains("Kind"));
    assert!(rendered.contains("Auth"));
    assert!(rendered.contains("Base URL"));
    assert!(rendered.contains("OAuth"));
    assert!(rendered.contains("API key:"));
    assert!(rendered.contains("Selected: anthropic"));
    assert!(rendered.contains("[t] test"));
}
```

- [ ] **Step 2: Run Providers tests and verify they fail**

Run:

```bash
cargo test -p proxy-tui config_providers -- --nocapture
```

Expected: both new tests fail because the current UI still renders lowercase headers, old toolbar labels, and the old empty state.

- [ ] **Step 3: Implement Providers helper functions and rendering**

In `crates/proxy-tui/src/ui.rs`, add these helper functions near `draw_config_section_tabs`:

```rust
fn draw_config_toolbar(f: &mut Frame, area: Rect, actions: &[&str]) {
    let mut spans: Vec<Span> = Vec::new();
    for (i, label) in actions.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(
            *label,
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_config_hint(f: &mut Frame, area: Rect, text: String) {
    f.render_widget(
        Paragraph::new(text).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

fn auth_kind_label(auth: &AuthPayload) -> &'static str {
    match auth {
        AuthPayload::Passthrough => "passthrough",
        AuthPayload::ApiKey { .. } => "API key",
        AuthPayload::Bearer { .. } => "Bearer",
        AuthPayload::AnthropicOAuth { .. } | AuthPayload::OpenAiOAuth { .. } => "OAuth",
        AuthPayload::CodexAuto => "Codex auto",
    }
}

fn provider_summary(cfg: &proxy_admin_api::ConfigPayload) -> String {
    let mut oauth = 0usize;
    let mut api_key = 0usize;
    let mut bearer = 0usize;
    let mut passthrough = 0usize;
    let mut codex_auto = 0usize;

    for provider in &cfg.providers {
        match provider.auth {
            AuthPayload::AnthropicOAuth { .. } | AuthPayload::OpenAiOAuth { .. } => oauth += 1,
            AuthPayload::ApiKey { .. } => api_key += 1,
            AuthPayload::Bearer { .. } => bearer += 1,
            AuthPayload::Passthrough => passthrough += 1,
            AuthPayload::CodexAuto => codex_auto += 1,
        }
    }

    let mut parts = vec![format!("Providers: {} total", cfg.providers.len())];
    if oauth > 0 {
        parts.push(format!("{} OAuth", oauth));
    }
    if api_key > 0 {
        parts.push(format!("{} API key", api_key));
    }
    if bearer > 0 {
        parts.push(format!("{} Bearer", bearer));
    }
    if passthrough > 0 {
        parts.push(format!("{} passthrough", passthrough));
    }
    if codex_auto > 0 {
        parts.push(format!("{} Codex auto", codex_auto));
    }
    parts.join(" · ")
}

fn selected_provider_footer(state: &AppState, cfg: &proxy_admin_api::ConfigPayload) -> Option<String> {
    let provider = cfg.providers.get(state.providers_selected)?;
    let mut parts = vec![format!("Selected: {}", provider.name), provider.kind.clone()];
    if let Some(base_url) = provider
        .base_url
        .as_deref()
        .or(provider.openai_base_url.as_deref())
        .filter(|s| !s.is_empty())
    {
        parts.push(base_url.to_string());
    }
    parts.push("[t] test".into());
    Some(parts.join(" · "))
}
```

Change `draw_provider_toolbar` to:

```rust
fn draw_provider_toolbar(f: &mut Frame, area: Rect) {
    draw_config_toolbar(
        f,
        area,
        &["[a] Add", "[e/Enter] Edit", "[d] Delete", "[t] Test", "[r] Refresh"],
    );
}
```

Change `auth_summary` to:

```rust
fn auth_summary(a: &AuthPayload) -> String {
    match a {
        AuthPayload::Passthrough => "Passthrough".into(),
        AuthPayload::ApiKey { value } => format!("API key: {}", redact(value)),
        AuthPayload::Bearer { value } => format!("Bearer: {}", redact(value)),
        AuthPayload::AnthropicOAuth { .. } => "OAuth".into(),
        AuthPayload::OpenAiOAuth { .. } => "OAuth".into(),
        AuthPayload::CodexAuto => "Codex auto".into(),
    }
}
```

Update `draw_providers_content` so its body uses summary, toolbar, table, and footer. Replace the current function body with:

```rust
fn draw_providers_content(f: &mut Frame, area: Rect, state: &AppState) {
    if area.height == 0 {
        return;
    }

    let cfg = match &state.config {
        None => return draw_message(f, area, "Loading config…"),
        Some(Err(e)) => return draw_error(f, area, e),
        Some(Ok(c)) => c,
    };

    let summary_area = Rect::new(area.x, area.y, area.width, 1);
    f.render_widget(Paragraph::new(provider_summary(cfg)), summary_area);

    if area.height < 2 {
        return;
    }
    let toolbar_area = Rect::new(area.x, area.y + 1, area.width, 1);
    draw_provider_toolbar(f, toolbar_area);

    if area.height < 4 {
        return;
    }
    let footer_height = if area.height >= 6 && !cfg.providers.is_empty() { 1 } else { 0 };
    let table_height = area.height.saturating_sub(3 + footer_height);
    let table_area = Rect::new(area.x, area.y + 3, area.width, table_height);

    if cfg.providers.is_empty() {
        return draw_message(
            f,
            table_area,
            "No providers configured. Press [a] to add your first provider.",
        );
    }

    let header = Row::new(vec![
        Cell::from("Name"),
        Cell::from("Kind"),
        Cell::from("Auth"),
        Cell::from("Base URL"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    let rows: Vec<Row> = cfg
        .providers
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let style = if i == state.providers_selected {
                Style::default().fg(Color::Black).bg(Color::Yellow)
            } else {
                Style::default()
            };
            Row::new(vec![
                Cell::from(p.name.clone()),
                Cell::from(p.kind.clone()),
                Cell::from(auth_summary(&p.auth)),
                Cell::from(
                    p.base_url
                        .as_deref()
                        .or(p.openai_base_url.as_deref())
                        .unwrap_or_default()
                        .to_string(),
                ),
            ])
            .style(style)
        })
        .collect();

    let widths = [
        Constraint::Length(20),
        Constraint::Length(14),
        Constraint::Length(28),
        Constraint::Min(20),
    ];
    let table = Table::new(rows, widths).header(header);
    f.render_widget(table, table_area);

    if footer_height > 0
        && let Some(footer) = selected_provider_footer(state, cfg)
    {
        let footer_area = Rect::new(area.x, area.y + area.height - 1, area.width, 1);
        draw_config_hint(f, footer_area, footer);
    }
}
```

- [ ] **Step 4: Run Providers tests and verify they pass**

Run:

```bash
cargo test -p proxy-tui config_providers -- --nocapture
```

Expected: both `config_providers_*` tests pass.

- [ ] **Step 5: Run all proxy-tui tests**

Run:

```bash
cargo test -p proxy-tui
```

Expected: all tests pass.

- [ ] **Step 6: Commit Providers polish**

Run:

```bash
git add crates/proxy-tui/src/ui.rs
git commit -m "feat(tui): polish config providers section"
```

---

### Task 2: Routing section polish

**Files:**
- Modify: `crates/proxy-tui/src/ui.rs`

- [ ] **Step 1: Add failing Routing render tests**

Expand the test import from `proxy_admin_api` to include routing DTOs:

```rust
use proxy_admin_api::{
    AffinityPayload, AffinityStatus, AuthPayload, ConfigPayload, MatchPayload, ProviderPayload,
    RoutingRulePayload, RoutingStrategyPayload, StatusResponse,
};
```

Add these tests in `mod tests`:

```rust
#[test]
fn config_routing_empty_state_suggests_add_action() {
    let mut state = config_state(empty_config());
    state.config_section = ConfigSection::Routing;

    let rendered = render_state(&state, 120, 28);

    assert!(rendered.contains("Routing: 0 rules"));
    assert!(rendered.contains("No routing rules configured. Press [a] to add one."));
    assert!(rendered.contains("[a] Add"));
}

#[test]
fn config_routing_renders_summary_headers_fallback_dash_and_selected_footer() {
    let mut config = empty_config();
    config.routing = vec![
        RoutingRulePayload {
            r#match: MatchPayload {
                model: Some("claude-*".into()),
            },
            provider: "anthropic".into(),
            fallback: vec![],
            strategy: RoutingStrategyPayload::Failover,
            priority: None,
        },
        RoutingRulePayload {
            r#match: MatchPayload { model: None },
            provider: "openai".into(),
            fallback: vec!["deepseek".into()],
            strategy: RoutingStrategyPayload::RoundRobin,
            priority: None,
        },
    ];
    let mut state = config_state(config);
    state.config_section = ConfigSection::Routing;
    state.routing_selected = 0;

    let rendered = render_state(&state, 140, 30);

    assert!(rendered.contains("Routing: 2 rules"));
    assert!(rendered.contains("evaluated top to bottom"));
    assert!(rendered.contains("Model Match"));
    assert!(rendered.contains("Primary"));
    assert!(rendered.contains("Fallbacks"));
    assert!(rendered.contains("—"));
    assert!(rendered.contains("Selected: rule 1"));
    assert!(rendered.contains("claude-* → anthropic"));
    assert!(rendered.contains("fallback: —"));
}
```

- [ ] **Step 2: Run Routing tests and verify they fail**

Run:

```bash
cargo test -p proxy-tui config_routing -- --nocapture
```

Expected: the new tests fail because the current Routing UI uses old labels and empty fallback cells.

- [ ] **Step 3: Implement Routing helpers and rendering**

Add these helper functions near the Config helpers:

```rust
fn routing_summary(cfg: &proxy_admin_api::ConfigPayload) -> String {
    format!(
        "Routing: {} rules · evaluated top to bottom · default match: *",
        cfg.routing.len()
    )
}

fn selected_route_footer(state: &AppState, cfg: &proxy_admin_api::ConfigPayload) -> Option<String> {
    let route = cfg.routing.get(state.routing_selected)?;
    let model = route.r#match.model.as_deref().unwrap_or("*");
    let fallback = if route.fallback.is_empty() {
        "—".to_string()
    } else {
        route.fallback.join(", ")
    };
    Some(format!(
        "Selected: rule {} · {} → {} · fallback: {}",
        state.routing_selected + 1,
        model,
        route.provider,
        fallback
    ))
}
```

Change `draw_routing_toolbar` to:

```rust
fn draw_routing_toolbar(f: &mut Frame, area: Rect) {
    draw_config_toolbar(f, area, &["[a] Add", "[e/Enter] Edit", "[d] Delete"]);
}
```

Replace `draw_routing_content` with:

```rust
fn draw_routing_content(f: &mut Frame, area: Rect, state: &AppState) {
    if area.height == 0 {
        return;
    }

    let cfg = match &state.config {
        None => return draw_message(f, area, "Loading config…"),
        Some(Err(e)) => return draw_error(f, area, e),
        Some(Ok(c)) => c,
    };

    let summary_area = Rect::new(area.x, area.y, area.width, 1);
    f.render_widget(Paragraph::new(routing_summary(cfg)), summary_area);

    if area.height < 2 {
        return;
    }
    let toolbar_area = Rect::new(area.x, area.y + 1, area.width, 1);
    draw_routing_toolbar(f, toolbar_area);

    if area.height < 4 {
        return;
    }
    let footer_height = if area.height >= 6 && !cfg.routing.is_empty() { 1 } else { 0 };
    let table_height = area.height.saturating_sub(3 + footer_height);
    let table_area = Rect::new(area.x, area.y + 3, area.width, table_height);

    if cfg.routing.is_empty() {
        return draw_message(f, table_area, "No routing rules configured. Press [a] to add one.");
    }

    let header = Row::new(vec![
        Cell::from("#"),
        Cell::from("Model Match"),
        Cell::from("Primary"),
        Cell::from("Fallbacks"),
        Cell::from("Strategy"),
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
            let strategy_str = crate::wizard::strategy_label(&r.strategy);
            let fallback = if r.fallback.is_empty() {
                "—".to_string()
            } else {
                r.fallback.join(", ")
            };
            Row::new(vec![
                Cell::from(format!("{}", i + 1)),
                Cell::from(r.r#match.model.clone().unwrap_or_else(|| "*".into())),
                Cell::from(r.provider.clone()),
                Cell::from(fallback),
                Cell::from(strategy_str),
            ])
            .style(style)
        })
        .collect();

    let widths = [
        Constraint::Length(4),
        Constraint::Length(20),
        Constraint::Length(20),
        Constraint::Length(24),
        Constraint::Min(12),
    ];
    let table = Table::new(rows, widths).header(header);
    f.render_widget(table, table_area);

    if footer_height > 0
        && let Some(footer) = selected_route_footer(state, cfg)
    {
        let footer_area = Rect::new(area.x, area.y + area.height - 1, area.width, 1);
        draw_config_hint(f, footer_area, footer);
    }
}
```

- [ ] **Step 4: Run Routing tests and verify they pass**

Run:

```bash
cargo test -p proxy-tui config_routing -- --nocapture
```

Expected: both `config_routing_*` tests pass.

- [ ] **Step 5: Run all proxy-tui tests**

Run:

```bash
cargo test -p proxy-tui
```

Expected: all tests pass.

- [ ] **Step 6: Commit Routing polish**

Run:

```bash
git add crates/proxy-tui/src/ui.rs
git commit -m "feat(tui): polish config routing section"
```

---

### Task 3: Quotas section polish

**Files:**
- Modify: `crates/proxy-tui/src/ui.rs`

- [ ] **Step 1: Add failing Quotas render tests**

Expand the test import from `proxy_admin_api` to include `QuotaPayload`:

```rust
use proxy_admin_api::{
    AffinityPayload, AffinityStatus, AuthPayload, ConfigPayload, MatchPayload, ProviderPayload,
    QuotaPayload, RoutingRulePayload, RoutingStrategyPayload, StatusResponse,
};
```

Add these tests in `mod tests`:

```rust
#[test]
fn config_quotas_empty_state_suggests_add_action() {
    let mut state = config_state(empty_config());
    state.config_section = ConfigSection::Quotas;

    let rendered = render_state(&state, 120, 28);

    assert!(rendered.contains("Quotas: 0 rules"));
    assert!(rendered.contains("No quota rules configured. Press [a] to add one."));
    assert!(rendered.contains("[a] Add"));
}

#[test]
fn config_quotas_render_summary_readable_numbers_percent_and_selected_footer() {
    let mut config = empty_config();
    config.quota = vec![QuotaPayload {
        provider: "anthropic".into(),
        window: "1d".into(),
        max_requests: Some(1_000),
        max_input_tokens: Some(2_000_000),
        max_output_tokens: None,
        warn_pct: 80,
    }];
    let mut state = config_state(config);
    state.config_section = ConfigSection::Quotas;
    state.quota_selected = 0;

    let rendered = render_state(&state, 140, 30);

    assert!(rendered.contains("Quotas: 1 rules"));
    assert!(rendered.contains("1 request limits"));
    assert!(rendered.contains("1 token limits"));
    assert!(rendered.contains("Provider"));
    assert!(rendered.contains("Requests"));
    assert!(rendered.contains("Input Tok"));
    assert!(rendered.contains("Output Tok"));
    assert!(rendered.contains("1,000"));
    assert!(rendered.contains("2,000,000"));
    assert!(rendered.contains("80%"));
    assert!(rendered.contains("Selected: anthropic"));
    assert!(rendered.contains("1d window"));
    assert!(rendered.contains("warn 80%"));
}
```

- [ ] **Step 2: Run Quotas tests and verify they fail**

Run:

```bash
cargo test -p proxy-tui config_quotas -- --nocapture
```

Expected: the new tests fail because the current UI uses old labels, unformatted numbers, and no `%` suffix.

- [ ] **Step 3: Implement Quotas helpers and rendering**

Add these helper functions near the Config helpers:

```rust
fn format_count(value: u64) -> String {
    let s = value.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

fn format_optional_count(value: Option<u64>) -> String {
    value.map(format_count).unwrap_or_else(|| "—".into())
}

fn quota_summary(cfg: &proxy_admin_api::ConfigPayload) -> String {
    let request_limits = cfg.quota.iter().filter(|q| q.max_requests.is_some()).count();
    let token_limits = cfg
        .quota
        .iter()
        .filter(|q| q.max_input_tokens.is_some() || q.max_output_tokens.is_some())
        .count();
    format!(
        "Quotas: {} rules · {} request limits · {} token limits",
        cfg.quota.len(),
        request_limits,
        token_limits
    )
}

fn selected_quota_footer(state: &AppState, cfg: &proxy_admin_api::ConfigPayload) -> Option<String> {
    let quota = cfg.quota.get(state.quota_selected)?;
    let mut parts = vec![
        format!("Selected: {}", quota.provider),
        format!("{} window", quota.window),
    ];
    if let Some(max_requests) = quota.max_requests {
        parts.push(format!("{} req", format_count(max_requests)));
    }
    if let Some(max_input_tokens) = quota.max_input_tokens {
        parts.push(format!("{} input tok", format_count(max_input_tokens)));
    }
    if let Some(max_output_tokens) = quota.max_output_tokens {
        parts.push(format!("{} output tok", format_count(max_output_tokens)));
    }
    parts.push(format!("warn {}%", quota.warn_pct));
    Some(parts.join(" · "))
}
```

Change `draw_quotas_toolbar` to:

```rust
fn draw_quotas_toolbar(f: &mut Frame, area: Rect) {
    draw_config_toolbar(f, area, &["[a] Add", "[e/Enter] Edit", "[d] Delete"]);
}
```

Replace `draw_quotas_content` with:

```rust
fn draw_quotas_content(f: &mut Frame, area: Rect, state: &AppState) {
    if area.height == 0 {
        return;
    }

    let cfg = match &state.config {
        None => return draw_message(f, area, "Loading config…"),
        Some(Err(e)) => return draw_error(f, area, e),
        Some(Ok(c)) => c,
    };

    let summary_area = Rect::new(area.x, area.y, area.width, 1);
    f.render_widget(Paragraph::new(quota_summary(cfg)), summary_area);

    if area.height < 2 {
        return;
    }
    let toolbar_area = Rect::new(area.x, area.y + 1, area.width, 1);
    draw_quotas_toolbar(f, toolbar_area);

    if area.height < 4 {
        return;
    }
    let footer_height = if area.height >= 6 && !cfg.quota.is_empty() { 1 } else { 0 };
    let table_height = area.height.saturating_sub(3 + footer_height);
    let table_area = Rect::new(area.x, area.y + 3, area.width, table_height);

    if cfg.quota.is_empty() {
        return draw_message(f, table_area, "No quota rules configured. Press [a] to add one.");
    }

    let header = Row::new(vec![
        Cell::from("#"),
        Cell::from("Provider"),
        Cell::from("Window"),
        Cell::from("Requests"),
        Cell::from("Input Tok"),
        Cell::from("Output Tok"),
        Cell::from("Warn"),
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
                Cell::from(format_optional_count(q.max_requests)),
                Cell::from(format_optional_count(q.max_input_tokens)),
                Cell::from(format_optional_count(q.max_output_tokens)),
                Cell::from(format!("{}%", q.warn_pct)),
            ])
            .style(style)
        })
        .collect();

    let widths = [
        Constraint::Length(4),
        Constraint::Length(16),
        Constraint::Length(12),
        Constraint::Length(12),
        Constraint::Length(14),
        Constraint::Length(14),
        Constraint::Min(6),
    ];
    let table = Table::new(rows, widths).header(header);
    f.render_widget(table, table_area);

    if footer_height > 0
        && let Some(footer) = selected_quota_footer(state, cfg)
    {
        let footer_area = Rect::new(area.x, area.y + area.height - 1, area.width, 1);
        draw_config_hint(f, footer_area, footer);
    }
}
```

- [ ] **Step 4: Run Quotas tests and verify they pass**

Run:

```bash
cargo test -p proxy-tui config_quotas -- --nocapture
```

Expected: both `config_quotas_*` tests pass.

- [ ] **Step 5: Run all proxy-tui tests**

Run:

```bash
cargo test -p proxy-tui
```

Expected: all tests pass.

- [ ] **Step 6: Commit Quotas polish**

Run:

```bash
git add crates/proxy-tui/src/ui.rs
git commit -m "feat(tui): polish config quotas section"
```

---

### Task 4: Settings grouping and shared Config navigation hint

**Files:**
- Modify: `crates/proxy-tui/src/ui.rs`

- [ ] **Step 1: Add failing Settings and Config hint tests**

Add these tests in `mod tests`:

```rust
#[test]
fn config_settings_renders_grouped_runtime_and_affinity_sections() {
    let mut config = empty_config();
    config.proxy_db = None;
    config.pricing_db = Some("/tmp/pricing.db".into());
    config.affinity.headers = vec![];
    let mut state = config_state(config);
    state.config_section = ConfigSection::Settings;

    let rendered = render_state(&state, 120, 28);

    assert!(rendered.contains("Runtime"));
    assert!(rendered.contains("Port:"));
    assert!(rendered.contains("Proxy DB:"));
    assert!(rendered.contains("Pricing DB:"));
    assert!(rendered.contains("Affinity"));
    assert!(rendered.contains("Status:"));
    assert!(rendered.contains("[e] Toggle"));
    assert!(rendered.contains("Headers:"));
    assert!(rendered.contains("—"));
}

#[test]
fn config_tab_renders_navigation_hint_when_space_allows() {
    let state = config_state(empty_config());

    let rendered = render_state(&state, 120, 28);

    assert!(rendered.contains("←/→ section"));
    assert!(rendered.contains("↑/↓ select"));
    assert!(rendered.contains("Enter/e edit"));
}
```

- [ ] **Step 2: Run Settings/hint tests and verify they fail**

Run:

```bash
cargo test -p proxy-tui 'config_settings|config_tab' -- --nocapture
```

If the shell treats the pattern literally and no tests run, use:

```bash
cargo test -p proxy-tui config_settings -- --nocapture
cargo test -p proxy-tui config_tab -- --nocapture
```

Expected: the new tests fail because Settings is currently flat and there is no Config navigation hint.

- [ ] **Step 3: Update `draw_config` to reserve a bottom navigation hint**

Replace the content-area calculation in `draw_config` with this logic:

```rust
    let hint_height = if inner.height >= 8 { 1 } else { 0 };
    let content_height = inner.height.saturating_sub(1 + hint_height);
    if content_height == 0 {
        return;
    }
    let content_area = Rect::new(inner.x, inner.y + 1, inner.width, content_height);

    match state.config_section {
        ConfigSection::Providers => draw_providers_content(f, content_area, state),
        ConfigSection::Routing => draw_routing_content(f, content_area, state),
        ConfigSection::Quotas => draw_quotas_content(f, content_area, state),
        ConfigSection::Settings => draw_settings_content(f, content_area, state),
    }

    if hint_height > 0 {
        let hint_area = Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1);
        draw_config_hint(
            f,
            hint_area,
            "←/→ section · ↑/↓ select · Enter/e edit · a add · d delete · ? help".into(),
        );
    }
```

Keep the existing section-tabs rendering before this block.

- [ ] **Step 4: Replace Settings rendering with grouped layout**

Replace `draw_settings_content` with:

```rust
fn draw_settings_content(f: &mut Frame, area: Rect, state: &AppState) {
    let cfg = match &state.config {
        None => return draw_message(f, area, "Loading config…"),
        Some(Err(e)) => return draw_error(f, area, e),
        Some(Ok(c)) => c,
    };

    let affinity_str = if cfg.affinity.enabled {
        "enabled"
    } else {
        "disabled"
    };
    let proxy_db_str = cfg.proxy_db.as_deref().unwrap_or("default");
    let pricing_db_str = cfg.pricing_db.as_deref().unwrap_or("default");
    let headers_str = if cfg.affinity.headers.is_empty() {
        "—".to_string()
    } else {
        cfg.affinity.headers.join(", ")
    };

    let lines = vec![
        Line::from(Span::styled(
            "Runtime",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled("  Port:       ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(cfg.port.to_string()),
        ]),
        Line::from(vec![
            Span::styled("  Proxy DB:   ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(proxy_db_str),
        ]),
        Line::from(vec![
            Span::styled("  Pricing DB: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(pricing_db_str),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Affinity",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled("  Status:     ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(affinity_str),
            Span::raw("    "),
            Span::styled(
                "[e] Toggle",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("  Headers:    ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(headers_str),
        ]),
    ];
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}
```

- [ ] **Step 5: Run Settings/hint tests and verify they pass**

Run:

```bash
cargo test -p proxy-tui config_settings -- --nocapture
cargo test -p proxy-tui config_tab -- --nocapture
```

Expected: both tests pass.

- [ ] **Step 6: Run all proxy-tui tests**

Run:

```bash
cargo test -p proxy-tui
```

Expected: all tests pass.

- [ ] **Step 7: Commit Settings and navigation hint polish**

Run:

```bash
git add crates/proxy-tui/src/ui.rs
git commit -m "feat(tui): group config settings"
```

---

### Task 5: Final verification and cleanup

**Files:**
- Modify if needed: `crates/proxy-tui/src/ui.rs`
- Already created: `docs/superpowers/specs/2026-05-23-proxy-tui-config-ui-design.md`
- Already created: `docs/superpowers/plans/2026-05-23-proxy-tui-config-ui-polish.md`

- [ ] **Step 1: Run formatting check**

Run:

```bash
cargo fmt --check
```

Expected: exits successfully. If it fails, run `cargo fmt`, inspect the diff, and continue.

- [ ] **Step 2: Run full proxy-tui tests**

Run:

```bash
cargo test -p proxy-tui
```

Expected: all tests pass.

- [ ] **Step 3: Run workspace tests if time permits**

Run:

```bash
cargo test --workspace
```

Expected: all workspace tests pass. If unrelated long-running or network-sensitive tests fail, capture the failing test names and output before deciding whether to investigate.

- [ ] **Step 4: Inspect final diff**

Run:

```bash
git diff --stat
git diff -- crates/proxy-tui/src/ui.rs docs/superpowers/specs/2026-05-23-proxy-tui-config-ui-design.md docs/superpowers/plans/2026-05-23-proxy-tui-config-ui-polish.md
```

Expected: diff only contains Config UI polish, tests, and the spec/plan docs.

- [ ] **Step 5: Commit docs if they were not committed earlier**

Run:

```bash
git status --short
git add docs/superpowers/specs/2026-05-23-proxy-tui-config-ui-design.md docs/superpowers/plans/2026-05-23-proxy-tui-config-ui-polish.md
git commit -m "docs: plan proxy tui config polish"
```

Expected: docs are committed. If there are no staged docs because they were already committed, skip this commit.

---

## Self-review

- Spec coverage: Providers, Routing, Quotas, Settings, toolbar consistency, navigation hint, responsive omission behavior, loading/empty states, and test expectations are each covered by tasks above.
- Placeholder scan: This plan contains no `TBD`, `TODO`, or undefined implementation steps.
- Type consistency: The plan uses existing DTO names from `proxy_admin_api`: `ConfigPayload`, `ProviderPayload`, `RoutingRulePayload`, `MatchPayload`, `RoutingStrategyPayload`, `QuotaPayload`, `AffinityPayload`, and `AuthPayload`.
