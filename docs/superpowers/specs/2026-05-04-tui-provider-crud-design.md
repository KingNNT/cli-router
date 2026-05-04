# TUI Provider CRUD — Design

**Status:** approved
**Date:** 2026-05-04
**Scope:** `crates/proxy-tui` only. No new admin endpoints, no new shared crate types.

## Problem

The `proxy-tui` admin client lets a user **edit auth** on an existing provider but cannot **add**, **delete**, or edit non-auth fields (`name`, `kind`, `base_url`, `openai_base_url`). Users currently have to hand-edit `~/.config/cli-router/config.toml` and refresh — and that bypasses the TUI's value as a control plane.

This spec adds full Create / Edit / Delete provider operations to the Providers view.

## Goals

- One unified form modal that handles both **Add** and **Edit** of every provider field.
- A safe **Delete** that refuses to break routing rules.
- Reuse the existing OAuth (Anthropic) paste flow for both Add and Edit.
- Persist via the existing `PUT /admin/config` endpoint — no new admin surface.
- Keep all logic on the TUI side; no daemon changes.

## Non-goals

- Editing routing rules (separate concern, separate view).
- Force-delete that rewrites routing rules.
- Multi-user / concurrent-edit safety (ETag, optimistic locking). The admin API is loopback-only and assumed single-user.
- Free-form `kind` strings. Only `anthropic` and `zai` are supported by the daemon today; new kinds become a separate, deliberate addition.

## UX

### Keybinds (Providers view only)

| Key | Action |
|---|---|
| `a` | Open Add modal |
| `e` | Open Edit modal (prefilled from selected provider) |
| `d` | Open Delete confirm modal for selected provider |
| `t` | Test selected provider (existing) |
| `j` / `↓` | Move selection down (existing) |
| `k` / `↑` | Move selection up (existing) |
| `r` | Refresh config (existing) |

Footer hint when `view == Providers`:

```
[a] add  [e] edit  [d] delete  [t] test  [j/k] move  [r] refresh  [q] quit
```

### Add / Edit form modal

A single `ProviderFormModal` renders a vertical form. Fields, in order:

```
Name:            ____________________
Kind:            < anthropic >          (←/→ to cycle)
Base URL:        ____________________  (optional)
OpenAI Base URL: ____________________  (optional, only meaningful for kind=zai)
Auth Kind:       < passthrough >       (←/→ to cycle: passthrough | api_key | bearer | oauth (anthropic))
Auth Value:      ********************  (masked; only shown for api_key/bearer; not shown for oauth or passthrough)

[ Save ]                               (Enter to submit)
```

The `Auth Value` row is **hidden entirely** when `auth_kind` is `passthrough` or `oauth (anthropic)`. For OAuth the tokens are obtained by the paste flow on Save, not via this field. For the visible cases (`api_key`, `bearer`), characters render as `*` so secrets do not appear on screen.

Navigation:
- `Tab` / `Shift+Tab` move focus between fields, wrapping past `[ Save ]` back to `Name`.
- On enum fields (`Kind`, `Auth Kind`), `←` and `→` cycle values.
- On text fields, type to edit; `Backspace` deletes.
- `Enter` on `[ Save ]` submits.
- `Esc` at any time closes the modal without saving.

### Delete confirm modal

```
Delete provider 'anthropic-a'?

[y] yes   [n / Esc] no
```

If the provider is referenced by any routing rule, the modal instead shows:

```
Cannot delete 'anthropic-a' — referenced by:
  - rule #1: match=claude-opus-* provider=anthropic-a
  - rule #3: match=*           fallback=[anthropic-b, anthropic-a]

Resolve routing rules first.

[Esc] close
```

No `[y]` option in this state.

## State machine

### `Modal` enum (replaces existing variants on the form side)

```rust
pub enum Modal {
    None,
    TestProvider(TestProviderModal),       // unchanged
    ProviderForm(ProviderFormModal),       // replaces EditAuth
    DeleteConfirm(DeleteConfirmModal),     // new
}

pub struct ProviderFormModal {
    pub mode: FormMode,
    pub focused: FormField,
    pub name: String,
    pub kind: ProviderKind,
    pub base_url: String,
    pub openai_base_url: String,
    pub auth_kind: AuthInputKind,          // existing enum
    pub auth_value: String,
    pub state: FormState,
    pub error: Option<String>,             // rendered red at top of modal
}

pub enum FormMode {
    Add,
    Edit { original_index: usize, original_name: String },
}

pub enum FormField { Name, Kind, BaseUrl, OpenaiBaseUrl, AuthKind, AuthValue, Save }

pub enum ProviderKind { Anthropic, Zai }   // cycle: Anthropic ↔ Zai

pub enum FormState {
    Editing,
    Saving,
    OAuthAwaitingCode { authorization_url: String, state_id: String, code_input: String },
    OAuthExchanging,
    Failed(String),
}

pub struct DeleteConfirmModal {
    pub provider_index: usize,
    pub provider_name: String,
    pub blocking_rules: Vec<String>,       // empty → safe to delete
}
```

The existing `EditAuthModal` and `EditState` are removed. OAuth substates are folded into `FormState` with identical semantics.

### Save flow

The daemon's OAuth-complete handler **looks up the provider by name** in the live config and mutates its auth in place (see `crates/proxy/src/application/use_cases/admin.rs::CompleteAnthropicOAuth`). The provider must therefore exist in the daemon's config before `oauth_start` is called. Add + OAuth handles this by PUT-ing the new provider first with placeholder auth, then running the OAuth dance.

**Non-OAuth save (Add or Edit, auth_kind ∈ {Passthrough, ApiKey, Bearer}):**

```
Editing
  └─ Enter on [Save]
       └─ validate_provider_form(form, &cfg)
             ├─ Err(e) → state=Editing, error=Some(e)  [stay open]
             └─ Ok(provider) →
                   state=Saving
                   mutate local cfg:
                     Add  → cfg.providers.push(provider)
                     Edit → cfg.providers[original_index] = provider
                   client.put_config(&cfg)
                     ├─ Err(msg) → state=Failed(msg)  [stay open]
                     └─ Ok       → close modal, flash "added/updated <name>", refresh()
```

**Edit + OAuth (provider already exists in daemon):**

```
Editing
  └─ Enter on [Save] with auth_kind=OAuthAnthropic
       └─ validate_provider_form(form, &cfg)
             ├─ Err(e) → state=Editing, error=Some(e)
             └─ Ok(_)  →
                   client.oauth_start(name) → state=OAuthAwaitingCode{..}
                   user pastes code → state=OAuthExchanging
                   client.oauth_complete(state_id, code, name)
                     ├─ Err(msg) → state=Failed(msg)
                     └─ Ok       → daemon already wrote AnthropicOAuth auth + persisted.
                                    refetch config → close modal → flash "updated <name>"
```

If the user changed non-auth fields (`name`, `kind`, `base_url`, `openai_base_url`) at the same time as switching to OAuth, those are PUT first (with the *old* auth preserved), then the OAuth dance runs against the renamed/updated provider. This keeps the daemon's name lookup consistent with what we send.

**Add + OAuth (provider does not yet exist):**

```
Editing
  └─ Enter on [Save] with auth_kind=OAuthAnthropic and mode=Add
       └─ validate_provider_form(form, &cfg)
             ├─ Err(e) → state=Editing, error=Some(e)
             └─ Ok(_)  →
                   state=Saving
                   mutate local cfg:
                     cfg.providers.push(<new provider with auth=Passthrough as placeholder>)
                   client.put_config(&cfg)
                     ├─ Err(msg) → state=Failed(msg), rollback local cfg push
                     └─ Ok       →
                          client.oauth_start(name) → state=OAuthAwaitingCode{..}
                          user pastes code → state=OAuthExchanging
                          client.oauth_complete(state_id, code, name)
                            ├─ Err(msg) → state=Failed(msg)
                                          (provider exists with Passthrough auth;
                                           user can Esc, then Edit to retry OAuth
                                           or change to a different auth kind)
                            └─ Ok       → refetch config → close modal → flash "added <name>"
```

Documented quirk: an aborted Add+OAuth leaves the new provider in config with `auth=passthrough`. The user can finish via Edit or Delete it. We do **not** auto-rollback because the user may want to keep the provider record and just retry auth later.

### Delete flow

```
on 'd' key in Providers view:
  selected = state.selected_provider()?
  blocking = rules_referencing(&selected.name, &cfg)
  open Modal::DeleteConfirm { provider_name, blocking_rules: blocking }

on 'y' in DeleteConfirm (only when blocking_rules empty):
  cfg.providers.remove(provider_index)
  client.put_config(&cfg)
    ├─ Err(msg) → state.flash(format!("delete failed: {msg}"))
    └─ Ok       → state.flash("deleted <name>"), refresh()
```

## Validation

A new `app::validate` module exposes pure functions:

```rust
pub fn validate_provider_form(
    form: &ProviderFormModal,
    cfg: &ConfigPayload,
) -> Result<ProviderPayload, FormError>;

pub fn rules_referencing(name: &str, cfg: &ConfigPayload) -> Vec<String>;
```

`FormError`:

| Variant | Trigger |
|---|---|
| `EmptyName` | `name.trim().is_empty()` |
| `InvalidNameChars` | `name` contains anything outside `[A-Za-z0-9_-]` |
| `DuplicateName` | Add: name already exists. Edit: name exists at a different index |
| `EmptyAuthValue` | `auth_kind ∈ {ApiKey, Bearer}` and `auth_value.trim().is_empty()` |
| `RenameBlockedBy(Vec<String>)` | Edit: `name != original_name` and routing rules reference `original_name` |

`Display` impl renders each variant to the inline red error string in the modal.

`rules_referencing` walks `cfg.routing` and collects rule descriptions where `rule.provider == name` or `rule.fallback.contains(&name.to_string())`. Format: `"rule #{idx}: match={model_pat} {role}=…"`.

## Persistence

All operations follow the same shape:

1. Mutate the in-memory `ConfigPayload` cache that the TUI already holds (`AppState::config`).
2. Call `client.put_config(&cfg)` (existing method).
3. On success, refresh state from the daemon to confirm hot-reload landed.

This is identical to today's Edit-Auth flow. No new admin endpoints.

## Error handling

- **Validation errors** render inline at the top of the modal; modal stays open; field focus is preserved.
- **OAuth start/complete failure** → `FormState::Failed(msg)`. `Enter` retries the OAuth handshake; `Esc` discards.
- **PUT failure** (network, 4xx, 5xx) → `FormState::Failed(msg)` for form modal; `flash` for delete (no modal to keep open).
- **Stale config** (config.toml changed on disk between fetch and PUT) is **not handled**. Single-user assumption documented in non-goals.

## Module boundaries

The TUI is not split into clean-architecture rings (it's a thin admin client), but the design respects existing seams:

- `app.rs` — modal state, form data, `validate` submodule (pure logic, unit tests live here).
- `ui.rs` — render the form modal + delete confirm. Pure projection of `Modal` → ratatui widgets.
- `main.rs` — keybind dispatch, side-effecting calls to `client`, OAuth glue.
- `client.rs` — unchanged.

No changes to `crates/proxy/`, `crates/proxy-admin-api/`, `crates/shared/`.

## Testing

### Unit tests (in `app.rs`)

- `validate_provider_form` table-driven: empty name, whitespace-only name, invalid chars (`/`, space, `.`), duplicate on Add, duplicate on Edit (different index), valid Add of each auth kind, Edit allows same name at same index, rename blocked by routing reference.
- `rules_referencing`: empty config, name in `provider`, name in `fallback`, name in both, name nowhere, multiple rules.
- `ProviderFormModal::cycle_field` (Tab/Shift+Tab wrap correctly).
- `ProviderKind::cycle`, `AuthInputKind::cycle` (existing test gets one new case if missing).
- `auth_value` row visibility: hidden for `Passthrough` and `OAuthAnthropic`, shown (masked) for `ApiKey` and `Bearer`.

### State-machine tests

- Add → Save (api_key) → success path mutates cfg and closes modal.
- Add → Save (oauth) → transitions through `OAuthAwaitingCode` → `OAuthExchanging` → `Saving` → success.
- Edit rename blocked → modal stays Editing, error populated.
- Delete blocked by routing rule → confirm modal lists rules, `y` is ignored.

### Manual smoke

- Add an Anthropic provider with API key → restart proxy with the same config file → request still routes correctly.
- Delete a provider that nothing references → success.
- Try to delete a provider referenced by routing → blocked with rule list.

## Out of scope (explicit deferrals)

- Editing routing rules in the TUI.
- ETag / If-Match concurrency control on `PUT /admin/config`.
- Importing/exporting provider blocks as TOML snippets.
- Force-delete with automatic routing-rule rewrite.
- New provider kinds beyond `anthropic` and `zai`.
