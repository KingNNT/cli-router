# Active/Disabled Provider Toggle

## Status

Approved design — awaiting implementation plan.

## Context

The proxy currently builds every configured provider as a leaf and every routing rule must reference a configured provider. There is no way to temporarily take a provider out of service without deleting it and losing its authentication/settings. Operators want a simple active/disabled toggle that keeps the provider config intact while removing it from the request path.

## Goals

- Allow a provider to be marked `enabled` or `disabled`.
- Disabled providers are not used by the routing layer.
- Routing rules may not reference a disabled provider.
- proxy-tui supports both a form field and a hotkey toggle.
- Provider disabled in the list is visually distinguishable (gray).
- Existing databases continue to work with all providers active by default.

## Non-goals

- This change does not add per-rule enablement; rules are still controlled by their own routing config.
- Disabled providers are not deleted; their auth secrets and settings remain in the DB.

## Data model

### `ProviderConfig` (`crates/proxy/src/config.rs`)

```rust
pub struct ProviderConfig {
    pub name: String,
    pub kind: ProviderKind,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub openai_base_url: Option<String>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub thinking_mode: ThinkingMode,
    #[serde(default)]
    pub max_concurrent: Option<usize>,
    #[serde(default)]
    pub sanitize_empty_tools: bool,
}
```

`default_true` is an existing helper in the crate.

### `ProviderPayload` (`crates/proxy-admin-api/src/lib.rs`)

```rust
pub struct ProviderPayload {
    pub name: String,
    pub kind: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub auth: AuthPayload,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub openai_base_url: Option<String>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub thinking_mode: Option<String>,
    #[serde(default)]
    pub max_concurrent: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sanitize_empty_tools: Option<bool>,
}
```

### `ProviderFormModal` (`crates/proxy-tui/src/app.rs`)

```rust
pub struct ProviderFormModal {
    pub mode: FormMode,
    pub focused: FormField,
    pub name: String,
    pub kind: ProviderKind,
    pub enabled: bool,
    pub base_url: String,
    pub openai_base_url: String,
    pub reasoning_effort: ReasoningEffortInput,
    pub thinking_mode: ThinkingModeInput,
    pub sanitize_empty_tools: bool,
    pub auth_kind: AuthInputKind,
    pub auth_value: String,
    pub max_concurrent: Option<usize>,
    pub state: FormState,
    pub error: Option<String>,
}
```

### Database schema

Migration V8 adds an `enabled` column to `providers`:

```sql
ALTER TABLE providers ADD COLUMN enabled INTEGER NOT NULL DEFAULT 1;
```

`load_providers` and `save` in `crates/proxy/src/adapters/storage/db_config.rs` read and write this column.

## Server-side behavior

### Config validation

`Config::validate` in `crates/proxy/src/config.rs` gains two new checks after the existing unknown-provider checks:

1. The primary provider of every routing rule must be enabled.
2. Every fallback provider of every routing rule must be enabled.

Error messages:

- `routing rule {i} references disabled provider '{name}'`
- `routing rule {i} fallback references disabled provider '{name}'`

### Provider building

`build_leaves` in `crates/proxy/src/adapters/providers/builder.rs` skips providers where `enabled == false`. Disabled providers therefore do not:

- consume an HTTP client,
- participate in load balancing,
- perform background OAuth token refresh,
- appear in the leaves map passed to `RoutingProvider`.

`build_routing_provider` already fails if a rule references a provider not present in the leaves map, which is consistent with the new validation.

### Admin API conversions

`config_to_payload` and `payload_to_config` in `crates/proxy/src/application/use_cases/admin.rs` copy `enabled` between `ProviderConfig` and `ProviderPayload`.

## proxy-tui behavior

### Provider form

- Add `FormField::Enabled` to the `FormField` enum.
- `field_order` always includes `FormField::Enabled`, placed immediately before `FormField::Save`.
- `draw_form_modal` renders a row: `Active: [ Yes / No ]` (Yes = enabled, No = disabled).
- `ProviderFormModal::new_for_add` initializes `enabled` to `true`.
- `ProviderFormModal::from_provider` copies `enabled` from `ProviderPayload`.

### Provider list

- Disabled providers are rendered with a gray style (`Color::DarkGray`).
- The provider list footer hints at the new hotkey (`z` toggle enable/disable).
- Pressing `d` on the selected provider flips its `enabled` flag.

### Disable-with-routing-references confirmation

When `d` would disable a provider that is referenced by one or more routing rules (primary or fallback), the TUI shows a new modal:

```text
Provider 'anthropic' is referenced by 2 routing rule(s).
Delete those rules when disabling?

[ Yes ]   [ No ]
```

- **Yes**: disable the provider and remove all routing rules that reference it, then submit the updated config to the admin API.
- **No**: close the modal without changes.

The modal is implemented as a new `Modal` variant rather than overloading `DeleteConfirmModal` so that the action and copy are unambiguous.

### Admin API client

No client-side change is needed beyond the existing `ConfigPayload` serialization because `ProviderPayload` carries the new `enabled` field.

## OpenAPI / Swagger

`ProviderPayload` is part of the utoipa schema through `utoipa::ToSchema`. Adding the `enabled` field updates the generated schema automatically.

## Tests

### Unit tests in `crates/proxy/src/config.rs`

- `provider_config_defaults_enabled_to_true`
- `validate_rejects_disabled_provider_in_routing`
- `validate_rejects_disabled_provider_in_fallback`

### Unit tests in `crates/proxy/src/adapters/providers/builder.rs`

- `build_leaves_excludes_disabled_provider`

### Unit tests in `crates/proxy/src/adapters/storage/db_config.rs`

- `enabled_flag_survives_save_and_load`

### Unit tests in `crates/proxy/src/application/use_cases/admin.rs`

- `payload_to_config_parses_enabled`
- `config_to_payload_round_trips_enabled`

### Unit tests in `crates/proxy-tui/src/app.rs`

- `field_order_includes_enabled`

### Unit tests in `crates/proxy-tui/src/ui.rs`

- `disabled_provider_rendered_in_gray`
- `provider_form_shows_active_toggle`

## Backward compatibility

- Existing `providers` rows get `enabled = 1` via the migration default.
- JSON config payloads that omit `enabled` default to `true`.
- Existing routing rules remain valid because all legacy providers are active.

## Error messages

All validation errors are user-facing strings returned by the admin API and surfaced by the TUI:

- `routing rule 0 references disabled provider 'anthropic'`
- `routing rule 1 fallback references disabled provider 'zai'`

## Dependencies

This change touches:

- `crates/proxy/src/config.rs`
- `crates/proxy/src/adapters/storage/schema.rs`
- `crates/proxy/src/adapters/storage/db_config.rs`
- `crates/proxy/src/adapters/providers/builder.rs`
- `crates/proxy/src/application/use_cases/admin.rs`
- `crates/proxy-admin-api/src/lib.rs`
- `crates/proxy-tui/src/app.rs`
- `crates/proxy-tui/src/ui.rs`
- `crates/proxy-tui/src/main.rs`
