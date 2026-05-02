# Data-shape invariants

These assumptions are baked into every SQL query in `db.rs`. Changing any of them means touching every query — do it deliberately.

## Assistant-only rows

Token and cost fields live in `message.data` JSON and are **only present on `role = 'assistant'` rows**. `Db::where_clause` always prepends:

```sql
json_extract(m.data, '$.role') = 'assistant'
```

Never remove this predicate. User-role rows have no token fields and will silently poison aggregates with NULLs and zeros.

## Timestamps are epoch milliseconds

`$.time.created` is epoch **ms**. Day bucketing divides by 1000 before `datetime(..., 'unixepoch')`:

```sql
date(datetime(json_extract(m.data, '$.time.created') / 1000, 'unixepoch'))
```

When adding new time-based queries, keep the `/ 1000`. When filtering by date range, convert `NaiveDate` to `timestamp_millis()` — see the `date_from`/`date_to` branches in `where_clause`.

## session vs. message metadata

- **Project/directory** lives on `session.directory`. Queries that filter or group by project must `JOIN session s ON m.session_id = s.id`.
- **Model, provider, tokens, cost** live on `message.data` JSON. Read them with `json_extract` directly — do **not** add these columns to the `session` table or denormalize.

Keep the split. Don't add session-derived columns to message aggregates or vice versa.

## COALESCE all aggregates

`json_extract` returns NULL for missing keys on older rows. Every `SUM(...)` is wrapped:

- Integer counts: `COALESCE(SUM(json_extract(...)), 0)`
- Costs: `COALESCE(SUM(json_extract(...)), 0.0)`

Do not rely on SQLite's NULL-is-0 coercion — it isn't reliable across `SUM` of all-NULL groups.

## Parameter binding

`where_clause` returns `(sql_fragment, Vec<Box<dyn ToSql>>)`. Call sites bind via `Self::param_refs(&params)`. Never `format!` user input into SQL — always add a `?N` placeholder and push to `params`.

## Read-only connection

`Db::open` uses `OpenFlags::SQLITE_OPEN_READ_ONLY`. This tool must never write to `opencode.db`. If you need to cache or persist results, use a separate file — do not relax the flag.

## Pricing table (separate DB)

The pricing table lives in a separate writable SQLite file at `~/.local/share/cli-router/pricing.db`.

```sql
CREATE TABLE pricing (
  lookup_key             TEXT PRIMARY KEY,
  model_id               TEXT NOT NULL,
  provider_id            TEXT NOT NULL,
  input_per_token        REAL NOT NULL,
  output_per_token       REAL NOT NULL,
  cache_read_per_token   REAL,
  cache_write_per_token  REAL,
  last_synced_at         INTEGER NOT NULL   -- days since epoch (1970-01-01)
);
CREATE INDEX idx_pricing_model_id ON pricing(model_id);
```

### Two-key matching invariant

A single LiteLLM entry produces up to two rows with different `lookup_key`s:
- raw: the LiteLLM key as-is (e.g. `"claude-opus-4"`)
- composed: `"{litellm_provider}/{raw}"` (e.g. `"anthropic/claude-opus-4"`)

When the raw key already contains `/`, only the raw form is stored.

On lookup from an OpenCode `ModelId`, `ModelId::lookup_keys()` returns the full id and the suffix after the first `/`. The caller picks the first hit. No hit means fall back to OpenCode's `message.data.cost`.

## Aliases (compile-time)

`src/domain/services/aliases.rs` holds a `ALIASES: &[ModelAlias]` const. Each entry maps N source model IDs (exact match) to one canonical name plus per-token pricing. The TUI aggregates usage rows by canonical name before pricing reconciliation; `CompositePricingRepository` layers the alias-derived pricing in front of the LiteLLM-synced pricing.

To add or change aliases: edit the const, rebuild, restart. There is no runtime config file.

Matching is first-wins on duplicate source strings. No pattern matching.
