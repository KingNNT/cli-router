# Codex Reasoning Effort Provider Setting Design

## Goal

Allow users to set a default Codex reasoning effort from the `proxy-tui` structured provider editor. The value is persisted in the proxy SQLite database and applied by the Codex provider when incoming OpenAI Chat Completions requests do not already include `reasoning_effort`.

SQLite remains the single source of truth. Users should not need a separate config-file workflow to change this setting.

## User-Facing Behavior

- In `proxy-tui`, Codex providers expose a reasoning effort field.
- The field supports one of four states:
  - unset: do not inject a default reasoning effort
  - `low`
  - `medium`
  - `high`
- The field is shown only for `kind = "codex"` providers in structured provider editing.
- Saving the provider writes through the existing admin config API and persists to SQLite.
- Proxy hot reload picks up the changed provider config without daemon restart.

Request precedence:

1. If the incoming OpenAI Chat Completions request contains `reasoning_effort`, the request value wins.
2. Otherwise, if the selected Codex provider has a configured reasoning effort, the proxy injects `reasoning.effort` into the Codex Responses API request.
3. Otherwise, the proxy omits `reasoning`, preserving current behavior.

Non-Codex providers do not use this setting.

## Data Flow

```text
proxy-tui provider editor
  -> admin API ConfigPayload
  -> proxy UpdateConfig
  -> DbConfigRepository::save()
  -> providers.reasoning_effort
  -> DbConfigRepository::load()
  -> provider builder creates CodexProvider
  -> Codex request translator injects default reasoning.effort when needed
```

## Data Model

### Runtime Config

Add an optional field to `ProviderConfig`:

```rust
pub reasoning_effort: Option<String>
```

The field is generic at the config DTO level because providers are stored in a shared table and passed through a shared admin API. It is semantically meaningful only for Codex providers.

### SQLite

Add a nullable column to the `providers` table:

```sql
ALTER TABLE providers ADD COLUMN reasoning_effort TEXT;
```

This requires a new schema migration after the current version. Existing databases receive `NULL`, preserving current behavior.

### Admin API DTOs

Add the optional field to provider config payloads shared by the proxy and TUI. This keeps TUI edits and daemon config in sync through the existing DB-backed admin config flow.

## Provider Behavior

`CodexProvider` stores an optional default reasoning effort. During Chat Completions to Responses API translation:

- map request `reasoning_effort` to `reasoning.effort` exactly as today;
- if request `reasoning_effort` is absent and the provider default is set, insert `reasoning: { "effort": <default> }`;
- if neither exists, do not insert `reasoning`.

The builder passes `ProviderConfig.reasoning_effort` only when constructing `CodexProvider`. Other provider builders ignore it.

## TUI Design

The structured provider editor is the only user-facing way to change this setting.

For Codex providers, add a reasoning effort control with values:

```text
unset -> low -> medium -> high
```

The existing save path should include the selected value in the provider DTO. If the selected value is `unset`, send `None`/`null` so the DB column stores `NULL`.

For non-Codex providers, the control is hidden or disabled and saves `None`.

## Validation Rules

Accepted persisted values are:

- `low`
- `medium`
- `high`
- null/unset

Validation should happen before saving TUI/provider config if an existing validation path exists. At minimum, the TUI structured editor should only generate valid values.

## Error Handling

- Existing DB rows migrate with `reasoning_effort = NULL`.
- Invalid values should not be produced by the TUI.
- If a malformed value somehow reaches the proxy config, prefer rejecting the admin config update over silently sending an unknown reasoning effort upstream.
- Request-provided `reasoning_effort` remains trusted as caller intent and continues to pass through the existing translation path.

## Testing

Add or update tests for:

1. Codex request translation:
   - configured default applies when request omits `reasoning_effort`;
   - request `reasoning_effort` overrides configured default;
   - unset default preserves current no-`reasoning` behavior.
2. SQLite config repository:
   - save/load preserves `ProviderConfig.reasoning_effort`.
3. Schema migration:
   - migrated DB contains `providers.reasoning_effort`.
4. TUI/config DTO integration:
   - Codex provider editor can set unset/low/medium/high and save through the existing config payload shape, using existing test patterns where available.

## Out of Scope

- No hardcoded global Codex reasoning effort default.
- No routing-rule or model-specific reasoning effort defaults.
- No separate config-file user workflow. TOML seed/import support may pass through the field naturally via `ProviderConfig`, but SQLite and the TUI remain the supported source/editing path.
- No changes to how non-Codex providers handle requests.
