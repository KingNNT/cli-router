# Proxy TUI Config UI polish design

## Goal

Improve the `proxy-tui` Config tab so it is easier to understand and operate while preserving the current navigation model. This is a focused usability polish pass with small workflow improvements only, not a full redesign.

The existing structure remains:

```text
Config
Providers | Routing | Quotas | Settings
<section content>
```

## Layout model

Each Config section should use a consistent layout:

```text
<section tabs>
<context summary>
<action toolbar>
<main content table/list>
<selected item hint or detail footer>
```

Example Providers layout:

```text
 Providers  Routing  Quotas  Settings
 Providers: 5 total · 2 OAuth · 2 API key · 1 passthrough
 [a] Add  [e/Enter] Edit  [d] Delete  [t] Test  [r] Refresh

 Name        Kind       Auth                 Base URL
 anthropic   anthropic  OAuth: user@example  https://...
 openai      openai     Bearer: ...abcd      https://...

 Selected: anthropic · anthropic · https://api.anthropic.com · [t] test
```

For tight terminal heights, the content should degrade gracefully. Preserve the tabs, toolbar, and table first. The summary and selected footer may be omitted when there is not enough space.

## Providers section

### Summary

Render a compact inventory line:

```text
Providers: 5 total · 2 OAuth · 2 API key · 1 passthrough
```

If enabled/disabled status is not present in `ConfigPayload`, do not invent it or add backend fields for this polish pass.

### Table labels

Use user-facing title labels instead of lowercase/internal labels:

```text
Name | Kind | Auth | Base URL
```

### Auth display

Keep redaction behavior. Use readable labels such as:

- `API key: ...abcd`
- `Bearer: ...abcd`
- `OAuth: user@example.com`
- `Passthrough`
- `Codex auto`

### Empty state

Render:

```text
No providers configured. Press [a] to add your first provider.
```

### Selected footer

Render a one-line selected provider summary when a provider is selected:

```text
Selected: anthropic · anthropic · https://api.anthropic.com · [t] test
```

If there is no base URL, omit that segment.

## Routing section

### Summary

Render:

```text
Routing: 3 rules · evaluated top to bottom · default match: *
```

This makes the rule-order behavior visible.

### Table labels

Use:

```text
# | Model Match | Primary | Fallbacks | Strategy
```

### Fallbacks

If a route has no fallbacks, render `—` instead of an empty cell.

### Empty state

Render:

```text
No routing rules configured. Press [a] to add one.
```

### Selected footer

Render:

```text
Selected: rule 2 · claude-* → anthropic · fallback: openai, deepseek
```

If there are no fallbacks, render `fallback: —`.

## Quotas section

### Summary

Render a compact summary such as:

```text
Quotas: 4 rules · 2 request limits · 3 token limits
```

Request-limit count means rules with `max_requests` set. Token-limit count means rules with either `max_input_tokens` or `max_output_tokens` set.

### Table labels

Use:

```text
# | Provider | Window | Requests | Input Tok | Output Tok | Warn
```

### Number formatting

Format large numbers with comma separators for readability while preserving precision:

```text
1000000 -> 1,000,000
```

### Warning percent

Render warning thresholds with a percent suffix:

```text
80%
```

### Empty state

Render:

```text
No quota rules configured. Press [a] to add one.
```

### Selected footer

Render:

```text
Selected: anthropic · 1d window · 1,000 req · 2,000,000 input tok · warn 80%
```

Omit unset limit segments instead of showing noisy placeholders.

## Settings section

Group settings instead of rendering a flat key/value list:

```text
Runtime
  Port:       3456
  Proxy DB:   default
  Pricing DB: default

Affinity
  Status:     enabled    [e] Toggle
  Headers:    anthropic-conversation-id, x-session-id
```

Use title-style labels (`Proxy DB`, not `proxy_db`). Show empty headers as `—`. Keep `[e] Toggle` only next to affinity because it is the only editable setting currently exposed in this view.

## Toolbar consistency

Use consistent action labels across sections:

```text
[a] Add  [e/Enter] Edit  [d] Delete
```

Providers includes additional actions:

```text
[a] Add  [e/Enter] Edit  [d] Delete  [t] Test  [r] Refresh
```

The implementation can continue using `PROVIDER_TOOLBAR` for behavior, but rendered labels should be consistent and readable.

## Navigation hints

Add a lightweight footer inside Config when space allows:

```text
←/→ section · ↑/↓ select · Enter/e edit · a add · d delete · ? help
```

Do not let the hint reduce the core table below a useful size. If height is constrained, omit the summary and hint before omitting the table.

## Responsive behavior

For narrow terminals:

- Keep the single-table layout.
- Truncate long auth strings and base URLs.
- Prefer the selected footer over a side panel.

For wide terminals, a right-side details panel may be added later, but it is not required for this implementation. The selected footer provides most of the usability benefit with less complexity.

## Loading and error states

Keep the existing loading and error rendering pattern, but make section empty states action-oriented as described above. Do not promise retry behavior unless the relevant key actually reloads the config.

## Tests

Add TUI rendering tests that assert:

- Providers empty state includes the `[a]` add hint.
- Routing empty fallback cells render `—`.
- Quota warning thresholds render with `%`.
- Quota large numbers render with comma separators.
- Settings renders grouped headings `Runtime` and `Affinity`.
- Config section tabs continue to render.

## Out of scope

- Backend API changes.
- A new Config dashboard landing page.
- Route reordering.
- A wide-screen details side panel, unless it is trivial after the footer implementation.
- Editing settings beyond the currently exposed affinity toggle.
