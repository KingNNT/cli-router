# Per-provider thinking level — design

**Status:** approved, ready for implementation plan
**Date:** 2026-07-25
**Base branch:** `fix/minimax-format-mode` (PR #18). The migration numbering below assumes V11 from that branch already exists.

## Problem

Only two provider kinds can influence how much the upstream model thinks:

| kind | Field | Where it lands |
|---|---|---|
| `anthropic` | `reasoning_effort` | `output_config.effort`, injected in `upstream.rs` `inject_effort` |
| `codex` | `reasoning_effort` | `reasoning.effort` of the Responses API, in `codex.rs` |

`zai`, `deepseek`, `openai`, `kimi`, and `minimax` have no control at all, and the admin API rejects `reasoning_effort` for them outright. MiniMax's `thinking_mode` looks like a thinking control but is a *response filter* — it strips reasoning content from what the client sees and never touches the request.

Two further gaps make the current state worse than it looks:

- The translation layer drops thinking parameters entirely. `grep` across `crates/proxy/src/adapters/translation/` finds no reference to `thinking`, `reasoning_effort`, or `budget_tokens`, so a client's thinking configuration is lost whenever a request is translated between formats.
- Every provider exposes a differently-shaped knob, so a single shared scale cannot express them faithfully.

## Upstream reality

Researched 2026-07-25. This table is the source of truth for the preset catalog.

| kind | Wire knob | Values that exist |
|---|---|---|
| `anthropic` | `output_config.effort`, `thinking.type` | `low` `medium` `high` `xhigh` `max`; thinking disabled via `thinking.type="disabled"`. Opus 5 returns 400 for disabled thinking at effort `xhigh`/`max` |
| `codex` | `reasoning.effort` (Responses API) | `none` `minimal` `low` `medium` `high` `xhigh` |
| `openai` | `reasoning_effort` | `none` `minimal` `low` `medium` `high` `xhigh` |
| `zai` | `reasoning_effort`, `thinking.type` | `high` `max` (`max` is the default); disabled via `thinking.type="disabled"` |
| `deepseek` | `reasoning_effort` | `high` `max`. `low` and `medium` are accepted but mapped to `high` server-side, so they are not offered |
| `kimi` | `reasoning_effort` (K3) | `low` `high` `max` (`max` default). K2.x rejects a request carrying both `thinking` and `reasoning_effort` |
| `minimax` | `thinking.type` | `adaptive` `disabled`. M3 honors disabled; M2.x accepts the value but keeps reasoning on |

## Design

### Configuration surface

One field for every kind, plus one flag:

```rust
pub enum ThinkingLevel {
    Unset, Off, Minimal, Low, Medium, High, XHigh, Max, Adaptive,
}

pub struct ProviderConfig {
    pub thinking_level: ThinkingLevel,  // default Unset — proxy injects nothing
    pub thinking_force: bool,           // default false
    // ...
}
```

`reasoning_effort` is removed from `ProviderConfig` and from the admin API; existing values migrate into `thinking_level`.

`thinking_mode` stays exactly as it is. It filters MiniMax reasoning content out of responses and is orthogonal to how hard the model thinks.

Each kind offers only the levels its upstream actually has. One table drives the TUI dropdown and the admin API validation, so the two can never disagree:

| kind | Levels offered |
|---|---|
| `anthropic` | off · low · medium · high · xhigh · max |
| `codex` | off · minimal · low · medium · high · xhigh |
| `openai` | off · minimal · low · medium · high · xhigh |
| `zai` | off · high · max |
| `deepseek` | high · max |
| `kimi` | low · high · max |
| `minimax` | off · adaptive |

`Unset` is valid for every kind and means the proxy sends nothing, leaving the upstream default in force.

### Preset catalog

A pure data table in `crates/proxy/src/adapters/providers/thinking.rs`, sitting beside `default_urls` and `quirks_for` in the same module family:

```rust
pub struct ThinkingPatch {
    pub anthropic: Option<serde_json::Value>,
    pub openai: Option<serde_json::Value>,
}

pub fn thinking_patch(kind: ProviderKind, level: ThinkingLevel) -> Option<ThinkingPatch>;
pub fn thinking_levels(kind: ProviderKind) -> &'static [ThinkingLevel];
```

Each entry carries both wire representations, because a provider may serve either format:

| (kind, level) | anthropic branch | openai branch |
|---|---|---|
| (anthropic, high) | `{"output_config":{"effort":"high"}}` | — |
| (anthropic, off) | `{"thinking":{"type":"disabled"}}` | — |
| (codex, off) | — | `{"reasoning_effort":"none"}` |
| (zai, max) | `{"reasoning_effort":"max"}` | `{"reasoning_effort":"max"}` |
| (zai, off) | `{"thinking":{"type":"disabled"}}` | `{"thinking":{"type":"disabled"}}` |
| (deepseek, high) | — | `{"reasoning_effort":"high"}` |
| (kimi, low) | `{"reasoning_effort":"low"}` | `{"reasoning_effort":"low"}` |
| (minimax, adaptive) | `{"thinking":{"type":"adaptive"}}` | `{"thinking":{"type":"adaptive"}}` |
| (minimax, off) | `{"thinking":{"type":"disabled"}}` | `{"thinking":{"type":"disabled"}}` |

`None` for a branch means that kind has no endpoint of that format in `default_urls`, so the branch is never consulted. The `anthropic` kind is Anthropic-format only and the `codex`/`deepseek`/`openai` kinds are OpenAI-format only, which is why each carries a single branch. A provider configured with an endpoint outside its kind's default shape simply gets no patch on that path.

The `anthropic` + `off` entry deliberately carries no `effort` key. Sending `thinking.type="disabled"` alone leaves effort at the account default (`high`), which is a combination Opus 5 accepts; adding `xhigh` or `max` alongside it would return 400.

### Applying the patch

The proxy merges the branch matching the format actually sent upstream, after `format_mode` and any translation have decided what that format is:

- `UpstreamProvider::apply_anthropic_request_quirks` merges the `anthropic` branch
- `UpstreamProvider::apply_openai_request_quirks` merges the `openai` branch
- `CodexProvider` merges the `openai` branch into the chat body *before* translating to the Responses API, where the existing `reasoning_effort` → `reasoning.effort` mapping picks it up

Merging follows **RFC 7396 JSON Merge Patch**: objects merge recursively, scalars replace, and `null` deletes a key. This preserves a client's `output_config.format` when the patch only sets `output_config.effort` — a plain top-level key replacement would silently break structured outputs.

`thinking_force` decides precedence:

- **off** (default) — every leaf in the patch whose path already exists in the request body is pruned before merging. The configured level acts as a default and the client keeps control per request.
- **on** — the patch merges unchanged, so the configured level wins over whatever the client sent.

Because the patch is applied to the outgoing body rather than the incoming one, the translation layer's loss of thinking parameters stops mattering for providers that set a level: the correct value is re-injected on the way out. Fixing translation itself is out of scope here.

### Storage and migration

Migration **V12** adds two columns and backfills from `reasoning_effort`:

```sql
ALTER TABLE providers ADD COLUMN thinking_level TEXT NOT NULL DEFAULT 'unset';
ALTER TABLE providers ADD COLUMN thinking_force INTEGER NOT NULL DEFAULT 0;

UPDATE providers SET thinking_level = reasoning_effort
 WHERE kind = 'anthropic' AND COALESCE(reasoning_effort, '') <> '';

UPDATE providers SET thinking_level =
    CASE reasoning_effort WHEN 'none' THEN 'off' ELSE reasoning_effort END
 WHERE kind = 'codex' AND COALESCE(reasoning_effort, '') <> '';
```

The `reasoning_effort` column is left in place and no longer read, matching how `base_url` was retired in V9. Dropping it would break an older binary pointed at the same file for no benefit.

### Admin API

`ProviderPayload` drops `reasoning_effort` and gains:

```rust
pub thinking_level: Option<String>,  // "unset" | "off" | "minimal" | ... | "adaptive"
pub thinking_force: Option<bool>,
```

`payload_to_config` rejects a level that is not in the kind's list with a 400 naming the valid levels — for example `max` on `kimi`, or `low` on `deepseek`. `None` and `""` both resolve to `Unset`.

### TUI

`ThinkingLevelInput` follows the existing `ReasoningEffortInput` shape: a cycle widget whose value list comes from `thinking_levels(kind)`, so changing the provider kind re-scopes the dropdown. Two form rows replace the current `Reasoning Effort` row:

```
Thinking:      < high >         [←/→ to cycle]
Force:         < off >          [←/→ to toggle]
```

`Force` is hidden when `Thinking` is `unset`, since there is nothing to force.

## Testing

- `thinking_patch` returns the documented JSON for every `(kind, level)` pair in the catalog, and `None` for pairs outside it
- `thinking_levels` matches the offered-levels table for each kind
- RFC 7396 merge keeps a sibling `output_config.format` while setting `output_config.effort`
- `null` in a patch deletes the key
- `thinking_force = false` leaves a client-supplied value untouched; `true` overwrites it
- Codex applies the patch before translation, so `reasoning_effort` reaches `reasoning.effort`
- Migration V12 backfills `anthropic` effort verbatim and maps codex `none` → `off`; rows without `reasoning_effort` land on `unset`
- Admin API rejects an out-of-range level per kind and accepts every in-range one
- TUI cycles only through the kind's levels and hides `Force` at `unset`

## Out of scope

- Teaching the translation layer to carry `thinking` / `reasoning_effort` across formats. Provider-level patching covers the practical need; translation fidelity is its own change.
- Renaming or reworking MiniMax's `thinking_mode` response filter.
- Per-model levels. The level is per provider; a provider serving several models applies one level to all of them.
