# Routing form: provider/fallback dropdowns design

## Goal

Replace the free-text **Provider** and **Fallback** inputs on the routing
rule form modal in `proxy-tui` with dropdown-style selectors sourced from the
currently configured providers list. Eliminates typos, makes the field labels
discoverable, and reuses the existing "cycle" UI pattern (already used for
`Kind:`, `Auth Kind:`, and `Strategy:`).

Scope is `proxy-tui` only. No changes to `proxy-admin-api`, the daemon, or the
on-disk config shape.

## Background

The routing form modal (`crates/proxy-tui/src/app.rs`, `RoutingFormModal`)
currently holds two free-text fields whose values must match configured
provider names:

- `provider: String` — single provider name
- `fallback: String` — comma-separated list of provider names

Both are rendered as plain text inputs (`crates/proxy-tui/src/ui.rs`,
`draw_routing_form_modal`). Typing a name that does not match a configured
provider silently produces a rule that fails to route at runtime.

The codebase already has an established "cycle dropdown" pattern that renders
as `< value >    [←/→ to cycle]` and cycles through a known set of options on
`KeyCode::Left` / `KeyCode::Right` / `KeyCode::Enter`. This pattern is used for
`Kind:`, `Auth Kind:`, and `Strategy:` rows. We extend it to **Provider** and
introduce a small variation for **Fallback** (multi-select via toggle).

## Approach

### Provider field — single-select cycle

`Provider` becomes an index into a snapshot of provider names captured at
modal-open time. `←` / `→` / `Enter` cycle the index through the snapshot.

### Fallback field — multi-select via toggle

`Fallback` becomes an ordered `Vec<String>` of selected names plus a cursor
into the same provider snapshot. `←` / `→` move the cursor. `Enter` toggles
the cursor's provider in/out of the selection. `Backspace` removes the
last-added entry (quick undo).

### Empty providers list

If `state.config.providers` is empty when the modal opens, both rows render
`(no providers configured)` and Save is blocked with a clear error message.

### Stale references

When editing an existing rule whose `provider` or `fallback` entries reference
providers that have since been deleted:

- The provider dropdown cursor defaults to index 0 and the caller flashes a
  warning on modal open (`⚠ original provider 'foo' no longer exists`).
  Detection is done by the caller, not the constructor (see "Construction"
  below).
- Stale fallback entries are filtered out silently. They would be runtime
  no-ops anyway, and a single flash for the provider is enough signal.

## Data model

### `RoutingFormModal` (in `crates/proxy-tui/src/app.rs`)

```rust
pub struct RoutingFormModal {
    pub mode: FormMode,
    pub focused: RoutingField,
    pub match_model: String,

    // CHANGED: was `provider: String`
    pub available_providers: Vec<String>,  // snapshot taken at modal open
    pub provider_index: usize,

    // CHANGED: was `fallback: String`
    pub fallback: Vec<String>,            // ordered selection
    pub fallback_cursor: usize,           // index into available_providers

    pub strategy: proxy_admin_api::RoutingStrategyPayload,
    pub priority: String,
    pub error: Option<String>,
}
```

### Construction (`new_for_add`, `from_rule`)

Signatures change to take the providers snapshot:

```rust
pub fn new_for_add(available_providers: Vec<String>) -> Self
pub fn from_rule(
    index: usize,
    rule: &proxy_admin_api::RoutingRulePayload,
    available_providers: Vec<String>,
) -> Self
```

The snapshot is sourced in **config order** (the order providers appear on the
Providers tab), not alphabetically sorted, so users see the same ordering as
elsewhere.

`new_for_add` initializes `provider_index = 0`, `fallback = vec![]`,
`fallback_cursor = 0`.

`from_rule` resolves the existing `rule.provider` against the snapshot:

- If found → `provider_index` = its position.
- If not found → `provider_index = 0`.

The caller (`open_routing_edit_modal` in `main.rs`) detects staleness directly
by checking `!available_providers.contains(&rule.provider)` *before* calling
`from_rule`, and sets `state.flash(...)` if so. The constructor itself stays
infallible and free of side effects on `AppState`. Stale fallback entries are
dropped silently inside `from_rule` (no flash for those — they are runtime
no-ops, and a single flash for the provider is enough signal).

`fallback` is filtered to entries present in `available_providers`, preserving
order.

### Validation invariants (on Save)

1. `match_model` non-empty (unchanged).
2. `available_providers` non-empty — else error: `"no providers configured; add a provider first"`.
3. `provider_index < available_providers.len()` (defensive; always true via UI).
4. `fallback` must not contain the primary provider — else error: `"fallback must not contain the primary provider"`.
5. Every `fallback` entry is in `available_providers` (always true by construction).

## UI rendering

### Provider row

```text
Provider:         < anthropic >    [←/→ or Enter to cycle]
```

Empty list:

```text
Provider:         (no providers configured)
```

### Fallback row

```text
Fallback:         < zai > | selected: [zai, openai]    [Enter toggles, Backspace removes last]
```

No selection:

```text
Fallback:         < zai > | selected: (none)    [Enter toggles, Backspace removes last]
```

### Help line

Updated to:

```text
↑/↓: move  ←/→/Enter on Provider/Fallback: cycle  Enter on Fallback: toggle  Enter on Strategy: cycle  Enter on Save: submit  Esc: cancel
```

### Unchanged rows

`Match Model`, `Strategy`, `Priority` keep their existing render and edit
behavior.

## Key handling

### When `m.focused == RoutingField::Provider`

| Key            | Action                                                  |
| -------------- | ------------------------------------------------------- |
| `Left`         | Cycle `provider_index` backward (wraps to end)          |
| `Right`        | Cycle `provider_index` forward (wraps to start)         |
| `Enter`        | Cycle `provider_index` forward (matches Strategy row)   |
| `Char(c)`      | No-op (field is no longer free-text)                    |
| `Backspace`    | No-op                                                   |

### When `m.focused == RoutingField::Fallback`

| Key            | Action                                                                  |
| -------------- | ----------------------------------------------------------------------- |
| `Left`         | Cycle `fallback_cursor` backward (wraps)                                |
| `Right`        | Cycle `fallback_cursor` forward (wraps)                                 |
| `Enter`        | Toggle `available_providers[fallback_cursor]` in `m.fallback`           |
| `Backspace`    | Remove last entry from `m.fallback`                                     |
| `Char(c)`      | No-op                                                                   |

Toggle semantics: if the cursor's provider is already in `fallback`, remove
it (preserving the order of the remaining entries). Otherwise, append it.
Selection order = add order = failover order.

### Save behavior

Validation runs the five invariants above. On success, the
`RoutingRulePayload` is built from `available_providers[provider_index]` and
`m.fallback.clone()` directly — no more `split(',')` parsing.

### `edit_routing_text`

Loses its `Provider` and `Fallback` arms. Only `MatchModel` and `Priority`
remain text-editable.

## Compatibility

- **On-disk config**: unchanged. `RoutingRulePayload` still serializes
  `provider: String` and `fallback: Vec<String>`.
- **Backward compatibility**: existing rules load and edit correctly. Stale
  references are surfaced via flash + filtered fallback (see above).
- **API/daemon**: untouched. The change is fully contained in `proxy-tui`.
- **Rollback**: revert the commit; text inputs return. No migrations.

## Files touched

- `crates/proxy-tui/src/app.rs` — `RoutingFormModal` struct fields, `new_for_add`
  /`from_rule` signatures and bodies.
- `crates/proxy-tui/src/ui.rs` — `draw_routing_form_modal` Provider/Fallback
  rows and help line.
- `crates/proxy-tui/src/main.rs` — `handle_routing_form_key`, `edit_routing_text`,
  `open_routing_edit_modal`, `handle_config_key` call sites.

## Tests

### Updated existing tests (in `crates/proxy-tui/src/main.rs::mod tests`)

- `routing_form_provider_validation` (line ~2036) — switch from setting
  `form.provider` to setting `available_providers` and `provider_index`.
- `routing_form_renders_save_row_and_arrow_help` (line ~2066) — pass
  `available_providers` in the constructor.
- Other call sites of `RoutingFormModal::new_for_add` / `from_rule` in tests.

### Updated `crates/proxy-tui/src/app.rs::form_field_tests`

- `routing_field_navigation_includes_save` — unaffected (navigation logic
  unchanged), but if it constructs a modal it must pass the new arg.

### New tests (`crates/proxy-tui/src/main.rs::mod tests`)

1. `routing_provider_cycles_with_left_right` — index advances on `Right`,
   wraps, reverses on `Left`.
2. `routing_provider_enter_cycles_forward` — Enter on Provider cycles forward.
3. `routing_provider_no_providers_configured_blocks_save` — empty snapshot →
   Save returns error modal.
4. `routing_fallback_enter_toggles_add_then_remove` — Enter adds, Enter again
   removes.
5. `routing_fallback_appends_in_order` — toggle providers[2] then providers[0]
   → `fallback == [providers[2], providers[0]]`.
6. `routing_fallback_backspace_removes_last` — `[a, b]` + Backspace → `[a]`.
7. `routing_fallback_cycles_cursor_independently_of_selection` — cursor
   movement does not mutate `fallback`.
8. `routing_save_blocks_when_fallback_contains_primary` — Save returns error.
9. `routing_from_rule_filters_stale_fallback` — stale entries dropped.
10. `routing_from_rule_provider_not_in_snapshot_defaults_to_zero` —
    `provider_index == 0` for unknown name.

### New tests (`crates/proxy-tui/src/ui.rs::mod tests`)

11. `routing_form_renders_provider_as_cycle` — output contains `< name >` and
    `←/→` hint.
12. `routing_form_renders_fallback_with_selected_list` — output contains
    `selected: [a, b]`.

### New unit test (`crates/proxy-tui/src/app.rs::form_field_tests`)

13. `routing_form_construction_preserves_providers_order` — `new_for_add`
    keeps the passed-in order.

## Implementation sequence

For TDD-friendly incremental verification:

1. Update `RoutingFormModal` struct and constructors (`app.rs`).
2. Update `draw_routing_form_modal` (`ui.rs`).
3. Update `handle_routing_form_key` + `edit_routing_text` (`main.rs`).
4. Update existing tests that break due to signature changes.
5. Add new tests (cycling, toggling, validation, stale handling).
6. Add stale-provider flash in `open_routing_edit_modal`.
7. Update call sites in `handle_config_key`.

### Verification gates

- `cargo check -p proxy-tui` after step 1.
- `cargo test -p proxy-tui` after step 4 (existing tests green).
- `cargo test -p proxy-tui` after step 5 (new tests green).
- `cargo clippy -p proxy-tui -- -D warnings` after step 7.
- Manual smoke test via `mise run dev:proxy-tui`.

## Out of scope

- Sub-modal multi-select UI for Fallback (popup checklist). Considered in
  Approach 2 and rejected as over-engineering for the typical fallback list
  size.
- Any change to how routing rules are evaluated at runtime.
- Any change to the wire format or daemon.
