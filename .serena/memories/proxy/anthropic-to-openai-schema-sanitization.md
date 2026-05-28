# Anthropic → OpenAI Tool Schema Sanitization

## Problem

When Claude Code sends requests through the proxy to Codex/OpenAI providers, tool schemas (MCP tools like `AskUserQuestion`, Playwright tools) fail with 400 errors because OpenAI's strict mode validates JSON Schema much more strictly than Anthropic.

## Root Causes (discovered incrementally)

### 1. `required` doesn't include all `properties` keys
- **Error**: `"Invalid schema ... 'required' is required to be supplied and to be an array including every key in properties. Missing 'button'."`
- **Cause**: Anthropic tools have optional params (e.g. Playwright's `button` with default) omitted from `required`. OpenAI strict mode requires every key in `properties` to appear in `required`.
- **Fix**: `strip_schema_keywords` synthesises `required` from `properties` keys at every level (recursive).

### 2. Unsupported JSON Schema keywords
- **Error**: `"Invalid schema ... 'propertyNames' is not permitted."`
- **Cause**: Anthropic accepts full JSON Schema keywords (`propertyNames`, `title`, `default`, `pattern`, `format`, `minLength`, etc.) but OpenAI only supports a narrow subset.
- **Fix**: Blocklist of ~35 unsupported keywords, recursively stripped.

### 3. Phantom `required` keys from `patternProperties`
- **Error**: `"Extra required key 'annotations' supplied."`
- **Cause**: Anthropic schema has `required: ["questions", "annotations"]` where `annotations` is defined via `patternProperties` (not `properties`). After stripping `patternProperties`, `annotations` becomes a phantom key.
- **Fix**: Always overwrite `required` = exactly `properties` keys. Remove `required` entirely if no `properties`.

### 4. `required` not synthesised at nested levels
- **Error**: `"In context=('properties','annotations','additionalProperties'), 'required' is required … Missing 'notes'."`
- **Cause**: `required` synthesis only ran at top level, not recursively into nested objects like `additionalProperties`, `items`, `anyOf` entries.
- **Fix**: Moved synthesis into `strip_schema_keywords` itself (runs at every level).

### 5. `additionalProperties` with object schema value
- **Error**: `"Extra required key 'annotations' supplied."` (same message, different cause)
- **Cause**: Codex Responses API only accepts `additionalProperties: false` (boolean). Anthropic schemas use `additionalProperties: {"type": "string"}` or `additionalProperties: {"type": "object", "properties": {...}}` to define map/dict types. These cause rejection.
- **Fix**: Collapse all non-boolean `additionalProperties` to `false`. Inject empty `properties: {}` + `required: []` for objects that lose their shape.

## Architecture: Where Sanitization Happens

```
Claude Code (Anthropic format)
  → anthropic_to_openai::request::translate()
    → translate_tool_def()
      → strip_schema_keywords() ← ALL sanitization here
  → Chat Completions body (clean)
    → CodexProvider::forward_openai()
      → translate_tool() ← unwraps envelope only, NO re-sanitization
  → Responses API body
    → Codex upstream
```

## Key Files

- `crates/proxy/src/adapters/translation/anthropic_to_openai/request.rs` — `strip_schema_keywords()`, `translate_tool_def()`
- `crates/proxy/src/adapters/providers/codex.rs` — `translate_tool()`, `schema_looks_strict()`

## Error Response Translation

When upstream returns errors (400, etc.), the response body also needs translation:
- OpenAI error `{"error": {"message": "...", "type": "..."}}` → Anthropic error `{"type": "error", "error": {...}}`
- Fixed in `anthropic_to_openai/response.rs` and `openai_to_anthropic/response.rs`
