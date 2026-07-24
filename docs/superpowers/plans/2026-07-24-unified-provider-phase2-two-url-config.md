# Unified Provider — Phase 2: Two explicit URLs, end-to-end

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the overloaded `base_url` field with two explicit, independent fields — `anthropic_base_url` and `openai_base_url` — across config, SQLite, admin API, admin use cases, and the TUI, with a per-kind backfill migration. **No routing behavior change** (the old provider structs still build and still pin one format via `native_format()`).

**Architecture:** `base_url` currently means the Anthropic endpoint for the `anthropic`/`minimax`/`zai`/`kimi` kinds and the OpenAI endpoint for `deepseek`/`openai`/`codex`. This phase renames it to `anthropic_base_url` and moves the OpenAI-kinds' URL into `openai_base_url`, both in the type and in the stored data (migration). The builder maps the two fields back onto each existing provider struct's constructor so behavior is byte-for-byte unchanged. Editing stays coherent because the DTO and TUI move in the same phase.

**Tech Stack:** Rust (edition 2024), rusqlite (bundled), axum, utoipa, ratatui, `cargo test`.

## Global Constraints

- Edition 2024; prefer let-chains. No new dependencies.
- Hand-rolled migrations in `schema.rs` keyed by `PRAGMA user_version`; each new
  migration is a numbered `MIGRATION_Vn` const appended to the `MIGRATIONS`
  array. Never edit a released migration; only append.
- Treat an empty-string URL as "not configured" (normalize to `None` on load).
- `cargo test --workspace` and `cargo clippy --workspace -- -D warnings` green
  before every commit. Pre-commit hook runs the full suite (~5 min).
- Conventional Commits; no `Co-Authored-By`. Branch, not `develop`.

---

## File Structure

- `crates/proxy/src/config.rs` — `ProviderConfig`: rename `base_url` →
  `anthropic_base_url`.
- `crates/proxy/src/adapters/storage/schema.rs` — add `MIGRATION_V9`.
- `crates/proxy/src/adapters/storage/db_config.rs` — read/write the new column;
  normalize empty→None.
- `crates/proxy/src/adapters/providers/builder.rs` — map the two fields to each
  existing struct constructor per kind.
- `crates/proxy/src/application/use_cases/admin.rs` — `ProviderPayload` ↔
  `ProviderConfig` mapping + tests.
- `crates/proxy-admin-api/src/lib.rs` — `ProviderPayload`: rename `base_url` →
  `anthropic_base_url`.
- `crates/proxy-tui/src/*` — provider create/edit modal gains an
  Anthropic-URL field alongside the existing OpenAI-URL field.

---

### Task 1: Rename the config field and re-map the builder (compiles, behavior identical)

**Files:**
- Modify: `crates/proxy/src/config.rs`
- Modify: `crates/proxy/src/adapters/providers/builder.rs`
- Modify: `crates/proxy/src/adapters/storage/db_config.rs`
- Modify: `crates/proxy/src/application/use_cases/admin.rs`

**Interfaces:**
- Produces: `ProviderConfig.anthropic_base_url: Option<String>` (was `base_url`);
  `ProviderConfig.openai_base_url: Option<String>` unchanged.

- [ ] **Step 1: Rename the field**

In `crates/proxy/src/config.rs`, in `struct ProviderConfig`, rename
`pub base_url: Option<String>` to `pub anthropic_base_url: Option<String>`.

- [ ] **Step 2: Update the builder mapping per kind**

In `crates/proxy/src/adapters/providers/builder.rs`, `build_leaf`, feed each
existing constructor the correct field so behavior is unchanged. The rule: the
Anthropic-native kinds read `anthropic_base_url`; the OpenAI-native kinds read
`openai_base_url` (previously they read `base_url`).

```rust
    Ok(match p.kind {
        ProviderKind::Anthropic => Arc::new(AnthropicProvider::configure_with_effort(
            http, p.anthropic_base_url.clone(), auth, p.reasoning_effort.clone(),
        )),
        ProviderKind::Zai => Arc::new(ZaiProvider::configure(
            http, p.anthropic_base_url.clone(), p.openai_base_url.clone(), auth,
        )),
        ProviderKind::DeepSeek => Arc::new(DeepSeekProvider::configure(
            http, p.openai_base_url.clone(), auth,
        )),
        ProviderKind::OpenAi => Arc::new(OpenAiProvider::configure(
            http, p.openai_base_url.clone(), auth,
        )),
        ProviderKind::Codex => Arc::new(CodexProvider::configure_with_reasoning_effort(
            http, p.openai_base_url.clone(), auth, p.reasoning_effort.clone(),
        )),
        ProviderKind::Minimax => {
            let mode = /* unchanged thinking_mode match */;
            Arc::new(MinimaxProvider::configure(
                http, p.anthropic_base_url.clone(), p.openai_base_url.clone(), auth, mode,
            ))
        }
        ProviderKind::Kimi => Arc::new(KimiProvider::configure(
            http, p.anthropic_base_url.clone(), p.openai_base_url.clone(), auth,
            p.sanitize_empty_tools,
        )),
    })
```

Keep the existing `thinking_mode` match block verbatim.

- [ ] **Step 3: Fix the remaining references the compiler flags**

Run `cargo build -p proxy` and update every remaining `base_url:` /
`.base_url` on `ProviderConfig` to `anthropic_base_url` — these are in
`db_config.rs` (`load_providers` construction at ~line 218, save binding at
~line 120) and `admin.rs` (payload→config mapping at ~line 793, config→payload
at ~line 702, and the many test fixtures constructing `ProviderConfig { … }`).
For the config→payload direction in `admin.rs`, map
`base_url: p.anthropic_base_url.clone()` for now (the DTO field is renamed in
Task 5). `openai_base_url` stays as-is everywhere.

- [ ] **Step 4: Build and test**

Run: `cargo test -p proxy`
Expected: PASS — this is a pure rename; no behavior changed. If a test fails,
it is a missed rename site, not a logic change.

- [ ] **Step 5: Commit**

```bash
git add crates/proxy/src/config.rs crates/proxy/src/adapters/providers/builder.rs \
        crates/proxy/src/adapters/storage/db_config.rs \
        crates/proxy/src/application/use_cases/admin.rs
git commit -m "refactor(proxy): rename ProviderConfig.base_url to anthropic_base_url"
```

---

### Task 2: SQLite migration V9 — add column + per-kind backfill

**Files:**
- Modify: `crates/proxy/src/adapters/storage/schema.rs`

**Interfaces:**
- Produces: a `providers.anthropic_base_url TEXT` column, populated from the old
  `base_url` by kind. `base_url` remains in the table but is no longer read.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` in `schema.rs` (mirror the existing
`PRAGMA table_info(providers)` tests near line 258):

```rust
#[test]
fn migration_adds_anthropic_base_url_and_backfills_by_kind() {
    let conn = Connection::open_in_memory().unwrap();
    // Bring the schema up to V8 first, then seed rows the way older code stored them.
    ensure_current(&conn).unwrap();
    conn.execute_batch(
        "INSERT INTO providers (name, kind, base_url, openai_base_url, auth_type) VALUES
           ('mm','minimax','https://api.minimax.io/anthropic','https://api.minimax.io/v1','bearer'),
           ('ds','deepseek','https://api.deepseek.com',NULL,'bearer'),
           ('an','anthropic','https://api.anthropic.com',NULL,'anthropic_oauth');",
    ).unwrap();

    // Re-running ensure_current is idempotent and V9 is already applied here;
    // assert the backfill landed.
    let anthropic_of = |name: &str| -> Option<String> {
        conn.query_row(
            "SELECT anthropic_base_url FROM providers WHERE name=?1",
            [name], |r| r.get(0),
        ).unwrap()
    };
    let openai_of = |name: &str| -> Option<String> {
        conn.query_row(
            "SELECT openai_base_url FROM providers WHERE name=?1",
            [name], |r| r.get(0),
        ).unwrap()
    };

    assert_eq!(anthropic_of("mm").as_deref(), Some("https://api.minimax.io/anthropic"));
    assert_eq!(openai_of("mm").as_deref(), Some("https://api.minimax.io/v1"));
    assert_eq!(anthropic_of("an").as_deref(), Some("https://api.anthropic.com"));
    assert_eq!(openai_of("ds").as_deref(), Some("https://api.deepseek.com"));
    assert_eq!(anthropic_of("ds"), None);
}
```

Note: because `ensure_current` seeds through V9 before the INSERT above runs,
the INSERT sets `anthropic_base_url` to NULL implicitly and the test would then
need the backfill to have run against *existing* rows. To exercise the backfill
against pre-existing data, the test must insert rows **before** V9 exists. Use
this ordering instead: build the table only up to V8 by running migrations V1–V8
manually (copy the `MIGRATIONS` slice up to V8), INSERT, then apply
`MIGRATION_V9` alone and assert. If replicating the runner is awkward, assert
the simpler property that a fresh `ensure_current` + INSERT with explicit
`anthropic_base_url` round-trips, and cover the backfill with a dedicated
`conn.execute_batch(MIGRATION_V9)` call on a hand-built V8 table.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p proxy --lib migration_adds_anthropic_base_url`
Expected: FAIL — column `anthropic_base_url` does not exist.

- [ ] **Step 3: Add the migration**

After `MIGRATION_V8` in `schema.rs`:

```rust
const MIGRATION_V9: &str = r#"
ALTER TABLE providers ADD COLUMN anthropic_base_url TEXT;

UPDATE providers
   SET anthropic_base_url = base_url
 WHERE kind IN ('anthropic','minimax','zai','kimi')
   AND base_url IS NOT NULL AND base_url <> '';

UPDATE providers
   SET openai_base_url = base_url
 WHERE kind IN ('deepseek','openai','codex')
   AND base_url IS NOT NULL AND base_url <> ''
   AND (openai_base_url IS NULL OR openai_base_url = '');
"#;
```

Append `(9, MIGRATION_V9)` to the `MIGRATIONS` array (find the array literal that
already lists `(8, MIGRATION_V8)`).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p proxy --lib migration`
Expected: PASS (including the existing migration idempotency test).

- [ ] **Step 5: Commit**

```bash
git add crates/proxy/src/adapters/storage/schema.rs
git commit -m "feat(proxy): migration V9 adds anthropic_base_url with per-kind backfill"
```

---

### Task 3: `db_config` reads/writes the new column, normalizing empty→None

**Files:**
- Modify: `crates/proxy/src/adapters/storage/db_config.rs`

**Interfaces:**
- Consumes: `ProviderConfig.anthropic_base_url` (Task 1), the V9 column (Task 2).
- Produces: round-trip persistence of both URL fields; empty strings load as
  `None`.

- [ ] **Step 1: Write the failing test**

Extend the existing round-trip test in `db_config.rs` (the one around line 460
that pushes a `ProviderConfig` and reloads). Add a second provider that sets
`anthropic_base_url` and assert both fields survive:

```rust
#[test]
fn provider_two_urls_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("c.db");
    let repo = DbConfigRepository::new(path.clone());
    let mut cfg = repo.load().unwrap();
    cfg.providers.push(ProviderConfig {
        name: "mm".into(),
        kind: ProviderKind::Minimax,
        enabled: true,
        auth: AuthConfig::Bearer { value: "k".into() },
        anthropic_base_url: Some("https://a/anthropic".into()),
        openai_base_url: Some("https://a/v1".into()),
        reasoning_effort: None,
        thinking_mode: ThinkingMode::SplitOnly,
        max_concurrent: None,
        sanitize_empty_tools: false,
    });
    repo.save(&cfg).unwrap();
    let loaded = repo.load().unwrap();
    let p = loaded.providers.iter().find(|p| p.name == "mm").unwrap();
    assert_eq!(p.anthropic_base_url.as_deref(), Some("https://a/anthropic"));
    assert_eq!(p.openai_base_url.as_deref(), Some("https://a/v1"));
}
```

Match the exact `DbConfigRepository` constructor/method names already used by
neighboring tests in this file.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p proxy --lib provider_two_urls_round_trip`
Expected: FAIL — the save/load SQL does not include `anthropic_base_url`.

- [ ] **Step 3: Update the SQL and mapping**

In the save `INSERT` (line ~109), add `anthropic_base_url` to the column list
and a matching `?N` placeholder, and bind `p.anthropic_base_url` in the params
list (place it right after `kind` or at the end — keep column order and params
order consistent).

In `load_providers` `SELECT` (line ~204), add `anthropic_base_url` to the
column list, then set the field in the constructed `ProviderConfig`. Normalize
empty strings to `None` for **both** URL fields with a small helper:

```rust
fn none_if_empty(v: Option<String>) -> Option<String> {
    v.filter(|s| !s.is_empty())
}
```

Apply it: `anthropic_base_url: none_if_empty(row.get(N)?)`,
`openai_base_url: none_if_empty(row.get(3)?)`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p proxy --lib db_config`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/proxy/src/adapters/storage/db_config.rs
git commit -m "feat(proxy): persist anthropic_base_url; treat empty URLs as unset"
```

---

### Task 4: Admin API DTO + use-case mapping expose both URLs

**Files:**
- Modify: `crates/proxy-admin-api/src/lib.rs`
- Modify: `crates/proxy/src/application/use_cases/admin.rs`

**Interfaces:**
- Produces: `ProviderPayload.anthropic_base_url: Option<String>` (renamed from
  `base_url`); `openai_base_url` unchanged.

- [ ] **Step 1: Rename the DTO field**

In `crates/proxy-admin-api/src/lib.rs`, `struct ProviderPayload`, rename
`pub base_url: Option<String>` to `pub anthropic_base_url: Option<String>`
(keep the `#[serde(default)]`).

- [ ] **Step 2: Update the mapping and fix fixtures**

In `admin.rs`, the config→payload map (~line 702) becomes
`anthropic_base_url: p.anthropic_base_url.clone()`, and the payload→config map
(~line 793) becomes `anthropic_base_url: pp.anthropic_base_url`. Run
`cargo build -p proxy` and rename `base_url:` to `anthropic_base_url:` in every
`ProviderPayload { … }` test fixture the compiler flags.

- [ ] **Step 3: Build and test**

Run: `cargo test -p proxy && cargo test -p proxy-admin-api`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/proxy-admin-api/src/lib.rs crates/proxy/src/application/use_cases/admin.rs
git commit -m "refactor(admin-api): rename ProviderPayload.base_url to anthropic_base_url"
```

---

### Task 5: TUI provider modal — Anthropic URL field beside OpenAI URL

**Files:**
- Modify: `crates/proxy-tui/src/*` (the provider create/edit modal — locate with
  the grep in Step 1)

**Interfaces:**
- Consumes: `ProviderPayload.anthropic_base_url` (Task 4).
- Produces: a UI field bound to `anthropic_base_url`; the existing
  `openai_base_url` field is retained.

- [ ] **Step 1: Locate the modal**

Run: `grep -rn "base_url\|openai_base_url" crates/proxy-tui/src`
Expected: references in the provider edit modal (fields list, input handling,
render, and payload construction). Note every occurrence of `base_url`.

- [ ] **Step 2: Rename and add the field**

- Rename the payload construction site's `base_url:` to `anthropic_base_url:`.
- Wherever the modal enumerates editable fields (an enum/array of field ids and
  their labels) add an "Anthropic URL" entry mirroring the existing
  "OpenAI URL" entry: label, read from / write to `anthropic_base_url`, same
  validation (optional URL). Keep the OpenAI URL field.
- If the modal has a field-count/index (for cursor navigation), increment it to
  cover the new field.

Do not add preset prefill yet — that arrives with `preset(kind)` in Phase 3/4.
For now both URL fields are free text, optional.

- [ ] **Step 3: Build and test**

Run: `cargo test -p proxy-tui && cargo build -p proxy-tui`
Expected: PASS / builds. If `validate.rs` has URL validation tests, ensure the
Anthropic URL uses the same validator as the OpenAI URL.

- [ ] **Step 4: Commit**

```bash
git add crates/proxy-tui/src
git commit -m "feat(proxy-tui): edit both anthropic_base_url and openai_base_url"
```

---

## Self-Review

- **Spec coverage:** spec §"Config schema and migration" (Tasks 1–3),
  §"Admin API + TUI" URL fields (Tasks 4–5). No routing/capability change here
  (that is Phases 3–4); `native_format()` untouched.
- **Placeholder scan:** migration SQL, DTO field, and mapping are shown in full;
  mechanical rename sites are found via `cargo build` errors (a standard
  compiler-guided refactor, not a vague "handle the rest").
- **Type consistency:** `anthropic_base_url` used identically in
  `ProviderConfig`, the V9 column, `db_config` SQL, `ProviderPayload`, the
  admin mapping, and the TUI. `none_if_empty` normalization applied to both URL
  fields on load.

## After this phase

Phase 3 adds `Quirks`, `preset(kind)`, and the generic `UpstreamProvider`
(reading the two URL fields for `supported_formats()`), unit-tested but not yet
wired. Phase 4 cuts `build_leaf` over to `UpstreamProvider` and deletes the
seven structs — dual-for-all turns on. Phase 5 removes `native_format()` and
adds preset-driven prefill to the TUI.
