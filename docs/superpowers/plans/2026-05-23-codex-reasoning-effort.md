# Codex Reasoning Effort Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a TUI-editable, SQLite-backed default Codex reasoning effort that is applied only when requests do not specify `reasoning_effort`.

**Architecture:** Store the setting as nullable `providers.reasoning_effort`, expose it through the shared admin API provider payload, carry it in `ProviderConfig`, and pass it into `CodexProvider`. The TUI structured provider editor is the only supported user-facing editing path; non-Codex providers save `None`.

**Tech Stack:** Rust 2024, serde, rusqlite migrations, axum admin DTOs, Ratatui TUI, existing `cargo test`/`cargo clippy` workflow.

---

## File Structure

- Modify `crates/proxy-admin-api/src/lib.rs`
  - Add `ProviderPayload.reasoning_effort: Option<String>` with serde default.
  - Update payload construction tests.
- Modify `crates/proxy/src/config.rs`
  - Add `ProviderConfig.reasoning_effort: Option<String>` with serde default.
  - Update tests/fixtures that construct `ProviderConfig`.
- Modify `crates/proxy/src/adapters/storage/schema.rs`
  - Add migration V4 for nullable `providers.reasoning_effort`.
  - Add migration test for the new column.
- Modify `crates/proxy/src/adapters/storage/db_config.rs`
  - Persist and load `reasoning_effort`.
  - Update save/load tests.
- Modify `crates/proxy/src/application/use_cases/admin.rs`
  - Map `ProviderConfig.reasoning_effort` to/from `ProviderPayload.reasoning_effort`.
  - Validate persisted values for provider config updates.
- Modify `crates/proxy/src/adapters/providers/codex.rs`
  - Add optional default reasoning effort to `CodexProvider`.
  - Add a translator variant that accepts provider default.
  - Preserve request-level override behavior.
- Modify `crates/proxy/src/adapters/providers/builder.rs`
  - Pass `ProviderConfig.reasoning_effort` into `CodexProvider` only.
- Modify `crates/proxy-tui/src/app.rs`
  - Add `ReasoningEffortInput` enum and `ProviderFormModal.reasoning_effort`.
  - Add `FormField::ReasoningEffort` and include it only for Codex providers.
- Modify `crates/proxy-tui/src/validate.rs`
  - Accept the optional reasoning effort input and write it into `ProviderPayload` only for Codex providers.
- Modify `crates/proxy-tui/src/main.rs`
  - Wire field traversal, cycling, and submit paths.
- Modify `crates/proxy-tui/src/ui.rs`
  - Render the field for Codex providers only.
- Modify `crates/proxy-tui/src/wizard.rs`
  - Preserve the field if the wizard builds a Codex provider payload.

---

### Task 1: Add Shared DTO and Config Field

**Files:**
- Modify: `crates/proxy-admin-api/src/lib.rs`
- Modify: `crates/proxy/src/config.rs`

- [ ] **Step 1: Add the failing DTO/config expectation tests**

In `crates/proxy-admin-api/src/lib.rs`, update the existing `config_payload_round_trips_through_toml` test provider to include the field:

```rust
providers: vec![ProviderPayload {
    name: "anthropic".into(),
    kind: "anthropic".into(),
    auth: AuthPayload::Passthrough,
    base_url: None,
    openai_base_url: None,
    reasoning_effort: Some("high".into()),
}],
```

Then add this assertion after parsing:

```rust
assert_eq!(
    parsed.providers[0].reasoning_effort.as_deref(),
    Some("high")
);
```

In `crates/proxy/src/config.rs`, add a unit test near the existing config tests:

```rust
#[test]
fn provider_config_deserializes_reasoning_effort() {
    let toml = r#"
name = "codex-main"
kind = "codex"
reasoning_effort = "high"
"#;
    let provider: ProviderConfig = toml::from_str(toml).unwrap();
    assert_eq!(provider.reasoning_effort.as_deref(), Some("high"));
}
```

- [ ] **Step 2: Run the targeted tests and verify failure**

Run:

```bash
cargo test -p proxy-admin-api config_payload_round_trips_through_toml
cargo test -p proxy provider_config_deserializes_reasoning_effort
```

Expected: compile failure mentioning missing field `reasoning_effort` on `ProviderPayload` and `ProviderConfig`.

- [ ] **Step 3: Add the DTO/config fields**

In `crates/proxy-admin-api/src/lib.rs`, change `ProviderPayload` to:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ProviderPayload {
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub auth: AuthPayload,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub openai_base_url: Option<String>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
}
```

In `crates/proxy/src/config.rs`, change `ProviderConfig` to:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub name: String,
    pub kind: ProviderKind,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub openai_base_url: Option<String>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
}
```

Update every `ProviderPayload { ... }` and `ProviderConfig { ... }` literal that fails to compile by adding:

```rust
reasoning_effort: None,
```

- [ ] **Step 4: Run targeted tests and verify pass**

Run:

```bash
cargo test -p proxy-admin-api config_payload_round_trips_through_toml
cargo test -p proxy provider_config_deserializes_reasoning_effort
```

Expected: both commands pass.

- [ ] **Step 5: Commit**

```bash
git add crates/proxy-admin-api/src/lib.rs crates/proxy/src/config.rs
git commit -m "feat: add reasoning effort config field"
```

---

### Task 2: Persist Reasoning Effort in SQLite Config

**Files:**
- Modify: `crates/proxy/src/adapters/storage/schema.rs`
- Modify: `crates/proxy/src/adapters/storage/db_config.rs`

- [ ] **Step 1: Write failing schema and repository tests**

In `crates/proxy/src/adapters/storage/schema.rs`, change migrations to include V4:

```rust
const MIGRATIONS: &[(i32, &str)] = &[
    (1, MIGRATION_V1),
    (2, MIGRATION_V2),
    (3, MIGRATION_V3),
    (4, MIGRATION_V4),
];
```

Add after `MIGRATION_V3`:

```rust
const MIGRATION_V4: &str = r#"
ALTER TABLE providers ADD COLUMN reasoning_effort TEXT;
"#;
```

Add this test in the schema test module:

```rust
#[test]
fn v4_adds_provider_reasoning_effort_column() {
    let conn = open_in_memory();
    ensure_current(&conn).unwrap();
    let cols: Vec<String> = conn
        .prepare("PRAGMA table_info(providers)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert!(cols.contains(&"reasoning_effort".into()));
}
```

In `crates/proxy/src/adapters/storage/db_config.rs`, update `save_and_load_roundtrip` provider fixture to set:

```rust
reasoning_effort: Some("high".into()),
```

Add an assertion after loading:

```rust
let provider = loaded.providers.iter().find(|p| p.name == "test").unwrap();
assert_eq!(provider.reasoning_effort.as_deref(), Some("high"));
```

- [ ] **Step 2: Run targeted tests and verify failure**

Run:

```bash
cargo test -p proxy v4_adds_provider_reasoning_effort_column save_and_load_roundtrip
```

Expected: repository roundtrip fails or compile fails until load/save SQL includes the new column.

- [ ] **Step 3: Update DB save/load SQL**

In `crates/proxy/src/adapters/storage/db_config.rs`, change the insert statement from:

```rust
"INSERT INTO providers (name, kind, base_url, openai_base_url, auth_type,
 auth_api_key, auth_bearer, auth_access_token, auth_refresh_token, auth_expires_at_ms)
 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)"
```

to:

```rust
"INSERT INTO providers (name, kind, base_url, openai_base_url, reasoning_effort, auth_type,
 auth_api_key, auth_bearer, auth_access_token, auth_refresh_token, auth_expires_at_ms)
 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"
```

Update the params to include `p.reasoning_effort` between `p.openai_base_url` and `auth_type`:

```rust
stmt.execute(rusqlite::params![
    p.name,
    kind_str,
    p.base_url,
    p.openai_base_url,
    p.reasoning_effort,
    auth_type,
    ak,
    bearer,
    at,
    rt,
    exp
])
```

Change the select SQL from:

```rust
"SELECT name, kind, base_url, openai_base_url, auth_type,
        auth_api_key, auth_bearer, auth_access_token, auth_refresh_token, auth_expires_at_ms
 FROM providers ORDER BY id"
```

to:

```rust
"SELECT name, kind, base_url, openai_base_url, reasoning_effort, auth_type,
        auth_api_key, auth_bearer, auth_access_token, auth_refresh_token, auth_expires_at_ms
 FROM providers ORDER BY id"
```

Update row indexes in the mapper:

```rust
let auth_type_str: String = row.get(5)?;
Ok(ProviderConfig {
    name: row.get(0)?,
    kind: parse_kind(&kind_str),
    base_url: row.get(2)?,
    openai_base_url: row.get(3)?,
    reasoning_effort: row.get(4)?,
    auth: columns_to_auth(
        &auth_type_str,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
    ),
})
```

- [ ] **Step 4: Run targeted tests and verify pass**

Run:

```bash
cargo test -p proxy v4_adds_provider_reasoning_effort_column save_and_load_roundtrip
```

Expected: both tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/proxy/src/adapters/storage/schema.rs crates/proxy/src/adapters/storage/db_config.rs
git commit -m "feat: persist provider reasoning effort"
```

---

### Task 3: Map and Validate Reasoning Effort Through Admin Config

**Files:**
- Modify: `crates/proxy/src/application/use_cases/admin.rs`

- [ ] **Step 1: Write failing admin mapping tests**

In `crates/proxy/src/application/use_cases/admin.rs`, update the config-to-payload roundtrip test fixture, or add this test in the existing test module:

```rust
#[test]
fn config_payload_preserves_reasoning_effort() {
    let cfg = Config {
        port: 8787,
        proxy_db: PathBuf::from("/tmp/proxy.db"),
        pricing_db: PathBuf::from("/tmp/pricing.db"),
        providers: vec![ProviderConfig {
            name: "codex-main".into(),
            kind: ProviderKind::Codex,
            auth: AuthConfig::CodexAuto,
            base_url: None,
            openai_base_url: None,
            reasoning_effort: Some("high".into()),
        }],
        routing: vec![],
        affinity: AffinityConfig::default(),
        quotas: vec![],
        docs_enabled: false,
        docs_port: None,
    };

    let payload = config_to_payload(&cfg);
    assert_eq!(payload.providers[0].reasoning_effort.as_deref(), Some("high"));

    let restored = payload_to_config(payload).unwrap();
    assert_eq!(restored.providers[0].reasoning_effort.as_deref(), Some("high"));
}
```

Add an invalid value test:

```rust
#[test]
fn payload_to_config_rejects_invalid_reasoning_effort() {
    let p = ConfigPayload {
        port: 8787,
        providers: vec![ProviderPayload {
            name: "codex-main".into(),
            kind: "codex".into(),
            auth: AuthPayload::CodexAuto,
            base_url: None,
            openai_base_url: None,
            reasoning_effort: Some("extreme".into()),
        }],
        routing: vec![],
        quota: vec![],
        affinity: AffinityPayload::default(),
        proxy_db: None,
        pricing_db: None,
    };

    let err = payload_to_config(p).unwrap_err();
    assert!(err.to_string().contains("invalid reasoning_effort"));
}
```

- [ ] **Step 2: Run targeted tests and verify failure**

Run:

```bash
cargo test -p proxy config_payload_preserves_reasoning_effort payload_to_config_rejects_invalid_reasoning_effort
```

Expected: preservation fails until mappings are added; invalid-value test fails until validation is added.

- [ ] **Step 3: Add mapping and validation**

In `config_to_payload`, add:

```rust
reasoning_effort: p.reasoning_effort.clone(),
```

In `payload_to_config`, before creating `ProviderConfig`, validate and normalize:

```rust
let reasoning_effort = match pp.reasoning_effort.as_deref().map(str::trim) {
    None | Some("") => None,
    Some("low" | "medium" | "high") => pp.reasoning_effort.map(|s| s.trim().to_string()),
    Some(other) => {
        return Err(ProxyError::BadRequest(format!(
            "invalid reasoning_effort '{other}' for provider '{}'",
            pp.name
        )));
    }
};
```

Then set:

```rust
reasoning_effort,
```

inside the `ProviderConfig` literal.

- [ ] **Step 4: Run targeted tests and verify pass**

Run:

```bash
cargo test -p proxy config_payload_preserves_reasoning_effort payload_to_config_rejects_invalid_reasoning_effort
```

Expected: both tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/proxy/src/application/use_cases/admin.rs
git commit -m "feat: validate reasoning effort admin config"
```

---

### Task 4: Apply Codex Provider Default During Request Translation

**Files:**
- Modify: `crates/proxy/src/adapters/providers/codex.rs`
- Modify: `crates/proxy/src/adapters/providers/builder.rs`

- [ ] **Step 1: Write failing Codex translation tests**

In `crates/proxy/src/adapters/providers/codex.rs`, add tests near `translate_reasoning_effort`:

```rust
#[test]
fn translate_uses_default_reasoning_effort_when_request_omits_it() {
    let chat = json!({
        "model": "codex-mini",
        "messages": [{"role": "user", "content": "Hello"}]
    });
    let result = translate_request_with_default_reasoning_effort(&chat, Some("high")).unwrap();
    assert_eq!(result["reasoning"]["effort"], "high");
}

#[test]
fn translate_request_reasoning_effort_overrides_provider_default() {
    let chat = json!({
        "model": "codex-mini",
        "messages": [{"role": "user", "content": "Hello"}],
        "reasoning_effort": "low"
    });
    let result = translate_request_with_default_reasoning_effort(&chat, Some("high")).unwrap();
    assert_eq!(result["reasoning"]["effort"], "low");
}

#[test]
fn translate_without_default_keeps_reasoning_absent() {
    let chat = json!({
        "model": "codex-mini",
        "messages": [{"role": "user", "content": "Hello"}]
    });
    let result = translate_request_with_default_reasoning_effort(&chat, None).unwrap();
    assert!(result.get("reasoning").is_none());
}
```

- [ ] **Step 2: Run targeted tests and verify failure**

Run:

```bash
cargo test -p proxy translate_uses_default_reasoning_effort_when_request_omits_it translate_request_reasoning_effort_overrides_provider_default translate_without_default_keeps_reasoning_absent
```

Expected: compile failure because `translate_request_with_default_reasoning_effort` does not exist.

- [ ] **Step 3: Add CodexProvider default field and translator helper**

In `CodexProvider`, add the field:

```rust
pub struct CodexProvider {
    base_url: String,
    http: reqwest::Client,
    auth: AuthHeader,
    default_reasoning_effort: Option<String>,
}
```

Update constructors:

```rust
pub fn configure(http: reqwest::Client, base_url: Option<String>, auth: AuthHeader) -> Self {
    Self::configure_with_reasoning_effort(http, base_url, auth, None)
}

pub fn configure_with_reasoning_effort(
    http: reqwest::Client,
    base_url: Option<String>,
    auth: AuthHeader,
    default_reasoning_effort: Option<String>,
) -> Self {
    Self::build(
        http,
        base_url.unwrap_or_else(|| DEFAULT_BASE_URL.into()),
        auth,
        default_reasoning_effort,
    )
}

fn build(
    http: reqwest::Client,
    base_url: String,
    auth: AuthHeader,
    default_reasoning_effort: Option<String>,
) -> Self {
    Self {
        base_url,
        http,
        auth,
        default_reasoning_effort,
    }
}
```

Update `new`, `with_base_url`, and `with_auth` to call `Self::build(..., None)`.

In `forward_openai`, replace:

```rust
let responses_body = translate_request(&chat_body).map_err(|e| {
```

with:

```rust
let responses_body = translate_request_with_default_reasoning_effort(
    &chat_body,
    self.default_reasoning_effort.as_deref(),
)
.map_err(|e| {
```

Change `translate_request` into a wrapper:

```rust
fn translate_request(chat: &Value) -> Result<Value, String> {
    translate_request_with_default_reasoning_effort(chat, None)
}

fn translate_request_with_default_reasoning_effort(
    chat: &Value,
    default_reasoning_effort: Option<&str>,
) -> Result<Value, String> {
    // move the current translate_request body here
}
```

Inside the moved body, replace the reasoning block with:

```rust
if let Some(effort) = chat.get("reasoning_effort") {
    out.insert("reasoning".into(), json!({ "effort": effort }));
} else if let Some(effort) = default_reasoning_effort {
    out.insert("reasoning".into(), json!({ "effort": effort }));
}
```

- [ ] **Step 4: Pass config default from builder**

In `crates/proxy/src/adapters/providers/builder.rs`, change the Codex branch to:

```rust
ProviderKind::Codex => Arc::new(CodexProvider::configure_with_reasoning_effort(
    http,
    p.base_url.clone(),
    auth,
    p.reasoning_effort.clone(),
)),
```

- [ ] **Step 5: Run targeted tests and verify pass**

Run:

```bash
cargo test -p proxy translate_uses_default_reasoning_effort_when_request_omits_it translate_request_reasoning_effort_overrides_provider_default translate_without_default_keeps_reasoning_absent
```

Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/proxy/src/adapters/providers/codex.rs crates/proxy/src/adapters/providers/builder.rs
git commit -m "feat: apply codex reasoning effort default"
```

---

### Task 5: Add TUI Structured Field and Validation

**Files:**
- Modify: `crates/proxy-tui/src/app.rs`
- Modify: `crates/proxy-tui/src/validate.rs`
- Modify: `crates/proxy-tui/src/main.rs`
- Modify: `crates/proxy-tui/src/ui.rs`
- Modify: `crates/proxy-tui/src/wizard.rs`

- [ ] **Step 1: Add failing validation tests**

In `crates/proxy-tui/src/validate.rs`, add `reasoning_effort` to the test helper `inputs` with default `None`, then add tests:

```rust
#[test]
fn codex_provider_preserves_reasoning_effort() {
    let auth = AuthPayload::CodexAuto;
    let cfg = empty_config();
    let input = FormInputs {
        name: "codex-main",
        kind: "codex",
        base_url: None,
        openai_base_url: None,
        reasoning_effort: Some("high"),
        auth: &auth,
        editing_index: None,
        original_name: None,
    };

    let provider = validate_provider_form(&input, &cfg).unwrap();
    assert_eq!(provider.reasoning_effort.as_deref(), Some("high"));
}

#[test]
fn non_codex_provider_drops_reasoning_effort() {
    let auth = AuthPayload::Passthrough;
    let cfg = empty_config();
    let input = FormInputs {
        name: "anthropic-main",
        kind: "anthropic",
        base_url: None,
        openai_base_url: None,
        reasoning_effort: Some("high"),
        auth: &auth,
        editing_index: None,
        original_name: None,
    };

    let provider = validate_provider_form(&input, &cfg).unwrap();
    assert_eq!(provider.reasoning_effort, None);
}
```

- [ ] **Step 2: Run validation tests and verify failure**

Run:

```bash
cargo test -p proxy-tui codex_provider_preserves_reasoning_effort non_codex_provider_drops_reasoning_effort
```

Expected: compile failure because `FormInputs.reasoning_effort` does not exist.

- [ ] **Step 3: Add TUI state enum and form field**

In `crates/proxy-tui/src/app.rs`, add this enum near `ProviderKind`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningEffortInput {
    Unset,
    Low,
    Medium,
    High,
}

impl ReasoningEffortInput {
    pub fn label(self) -> &'static str {
        match self {
            ReasoningEffortInput::Unset => "unset",
            ReasoningEffortInput::Low => "low",
            ReasoningEffortInput::Medium => "medium",
            ReasoningEffortInput::High => "high",
        }
    }

    pub fn as_option(self) -> Option<&'static str> {
        match self {
            ReasoningEffortInput::Unset => None,
            ReasoningEffortInput::Low => Some("low"),
            ReasoningEffortInput::Medium => Some("medium"),
            ReasoningEffortInput::High => Some("high"),
        }
    }

    pub fn from_option(value: Option<&str>) -> Self {
        match value {
            Some("low") => ReasoningEffortInput::Low,
            Some("medium") => ReasoningEffortInput::Medium,
            Some("high") => ReasoningEffortInput::High,
            _ => ReasoningEffortInput::Unset,
        }
    }

    pub fn cycle_next(self) -> Self {
        match self {
            ReasoningEffortInput::Unset => ReasoningEffortInput::Low,
            ReasoningEffortInput::Low => ReasoningEffortInput::Medium,
            ReasoningEffortInput::Medium => ReasoningEffortInput::High,
            ReasoningEffortInput::High => ReasoningEffortInput::Unset,
        }
    }

    pub fn cycle_prev(self) -> Self {
        match self {
            ReasoningEffortInput::Unset => ReasoningEffortInput::High,
            ReasoningEffortInput::Low => ReasoningEffortInput::Unset,
            ReasoningEffortInput::Medium => ReasoningEffortInput::Low,
            ReasoningEffortInput::High => ReasoningEffortInput::Medium,
        }
    }
}
```

Add a form field:

```rust
ReasoningEffort,
```

Add a modal field:

```rust
pub reasoning_effort: ReasoningEffortInput,
```

Set it in `new_for_add`:

```rust
reasoning_effort: ReasoningEffortInput::Unset,
```

Set it in `from_provider`:

```rust
reasoning_effort: ReasoningEffortInput::from_option(p.reasoning_effort.as_deref()),
```

Change `FormField::next/prev` and `field_order` to accept both auth kind and provider kind:

```rust
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
```

Implement `field_order` with a `Vec<FormField>` so Codex can include the extra field:

```rust
fn field_order(auth_kind: AuthInputKind, provider_kind: ProviderKind) -> Vec<FormField> {
    let mut order = vec![
        FormField::Name,
        FormField::Kind,
        FormField::BaseUrl,
        FormField::OpenaiBaseUrl,
    ];
    if provider_kind == ProviderKind::Codex {
        order.push(FormField::ReasoningEffort);
    }
    order.push(FormField::AuthKind);
    if matches!(auth_kind, AuthInputKind::ApiKey | AuthInputKind::Bearer) {
        order.push(FormField::AuthValue);
    }
    order.push(FormField::Save);
    order
}
```

- [ ] **Step 4: Wire validation input and save behavior**

In `crates/proxy-tui/src/validate.rs`, add to `FormInputs`:

```rust
pub reasoning_effort: Option<&'a str>,
```

In `validate_provider_form`, set the provider payload field:

```rust
reasoning_effort: if input.kind == "codex" {
    input
        .reasoning_effort
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
} else {
    None
},
```

Update all `FormInputs { ... }` literals in tests and production code to include:

```rust
reasoning_effort: m.reasoning_effort.as_option(),
```

For test helper defaults, use:

```rust
reasoning_effort: None,
```

- [ ] **Step 5: Wire key handling and text editing**

In `crates/proxy-tui/src/main.rs`, update Tab handlers:

```rust
m.focused = m.focused.next(m.auth_kind, m.kind);
```

and Shift+Tab handlers:

```rust
m.focused = m.focused.prev(m.auth_kind, m.kind);
```

In `cycle_field_value`, add:

```rust
FormField::ReasoningEffort => {
    m.reasoning_effort = if forward {
        m.reasoning_effort.cycle_next()
    } else {
        m.reasoning_effort.cycle_prev()
    };
}
```

When `FormField::Kind` changes away from Codex, reset the setting:

```rust
if m.kind != crate::app::ProviderKind::Codex {
    m.reasoning_effort = crate::app::ReasoningEffortInput::Unset;
}
```

In `edit_focused_text`, do not add `ReasoningEffort`; it is cycle-only.

- [ ] **Step 6: Render the TUI field**

In `crates/proxy-tui/src/ui.rs`, add `ReasoningEffortInput` to the `use crate::app::{...}` list if needed.

In `draw_form_modal`, after OpenAI Base URL and before Auth Kind, add:

```rust
if m.kind == crate::app::ProviderKind::Codex {
    lines.push(row(
        FormField::ReasoningEffort,
        "Reasoning Effort:",
        format!("< {} >    [←/→ to cycle]", m.reasoning_effort.label()),
    ));
}
```

- [ ] **Step 7: Preserve the field in wizard payloads**

In `crates/proxy-tui/src/wizard.rs`, add to the `ProviderPayload` literal:

```rust
reasoning_effort: if form.kind == crate::app::ProviderKind::Codex {
    form.reasoning_effort.as_option().map(str::to_string)
} else {
    None
},
```

- [ ] **Step 8: Run targeted TUI tests and verify pass**

Run:

```bash
cargo test -p proxy-tui codex_provider_preserves_reasoning_effort non_codex_provider_drops_reasoning_effort
```

Expected: both tests pass.

- [ ] **Step 9: Commit**

```bash
git add crates/proxy-tui/src/app.rs crates/proxy-tui/src/validate.rs crates/proxy-tui/src/main.rs crates/proxy-tui/src/ui.rs crates/proxy-tui/src/wizard.rs
git commit -m "feat: edit codex reasoning effort in tui"
```

---

### Task 6: Workspace Verification and Documentation Check

**Files:**
- Modify only if verification finds compile errors caused by missed struct literals.

- [ ] **Step 1: Run full tests**

Run:

```bash
cargo test --workspace
```

Expected: all tests pass.

- [ ] **Step 2: Run clippy**

Run:

```bash
cargo clippy --workspace -- -D warnings
```

Expected: no warnings.

- [ ] **Step 3: Fix any missed struct literal compile errors**

If Rust reports missing `reasoning_effort` fields in `ProviderPayload` or `ProviderConfig`, add:

```rust
reasoning_effort: None,
```

to that literal unless the test specifically needs `Some("low" | "medium" | "high")`.

- [ ] **Step 4: Re-run verification after fixes**

Run:

```bash
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

Expected: all tests pass and clippy reports no warnings.

- [ ] **Step 5: Commit verification fixes if any**

If Step 3 changed files, commit them:

```bash
git add crates
git commit -m "fix: complete reasoning effort wiring"
```

If no files changed, do not create an empty commit.

---

## Self-Review Notes

- Spec coverage: DB migration, admin DTO, provider config, Codex translation fallback, TUI-only user editing path, validation, and tests are all mapped to tasks.
- Placeholder scan: no TBD/TODO/fill-in placeholders remain; all code-changing steps include concrete snippets.
- Type consistency: the plan uses `reasoning_effort` consistently as `Option<String>` in persisted/shared DTOs and `ReasoningEffortInput` only inside TUI form state.
