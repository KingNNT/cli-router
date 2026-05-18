# DB-as-Single-Source-of-Truth — Design Spec

**Date:** 2026-05-18
**Status:** Approved

## Problem

cli-router uses `~/.config/cli-router/config.toml` as its source of truth for all
configuration: providers, routing, auth tokens, quota, affinity, port, DB paths.
This means:

- Secrets (API keys, OAuth tokens) live in a plaintext file
- Config file mutations require TOML serialization and file I/O scattered across 4+ places
- The TUI/admin API must read-modify-write the TOML file, risking race conditions
- Two sources of truth (file + in-memory `Arc<RwLock<Config>>`) can drift

## Solution

**Eliminate config.toml entirely.** SQLite becomes the single source of truth for
all configuration. The proxy starts with a CLI flag `--db <path>` (default:
`~/.local/share/cli-router/proxy.db`). A `ConfigRepository` trait (clean architecture
port) abstracts persistence. `DbConfigRepository` (adapter) reads/writes normalized
tables.

## ConfigRepository Port

```rust
// crates/proxy/src/application/ports/config.rs

pub trait ConfigRepository: Send + Sync {
    fn load(&self) -> Result<Config, ConfigError>;
    fn save(&self, config: &Config) -> Result<(), ConfigError>;
}
```

## DB Schema (Migration V3)

```sql
-- Server settings (port, proxy_db, pricing_db, affinity)
CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- Providers
CREATE TABLE providers (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    name              TEXT NOT NULL UNIQUE,
    kind              TEXT NOT NULL,
    base_url          TEXT,
    openai_base_url   TEXT,
    auth_type         TEXT NOT NULL,
    auth_api_key      TEXT,
    auth_bearer       TEXT,
    auth_access_token TEXT,
    auth_refresh_token TEXT,
    auth_expires_at_ms INTEGER,
    created_at        INTEGER NOT NULL DEFAULT (CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)),
    updated_at        INTEGER NOT NULL DEFAULT (CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER))
);

-- Routing rules (ordered by priority, lower = higher priority)
CREATE TABLE routing_rules (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    priority   INTEGER NOT NULL DEFAULT 0,
    provider   TEXT NOT NULL,
    model_glob TEXT NOT NULL,
    strategy   TEXT NOT NULL DEFAULT 'failover',
    fallback   TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL DEFAULT (CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER))
);

CREATE INDEX idx_routing_rules_priority ON routing_rules(priority);

-- Quota rules
CREATE TABLE quota_rules (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    provider          TEXT NOT NULL,
    window            TEXT NOT NULL,
    max_requests      INTEGER,
    max_input_tokens  INTEGER,
    max_output_tokens INTEGER,
    warn_pct          INTEGER NOT NULL DEFAULT 80
);
```

### Auth column mapping

| auth_type | auth_api_key | auth_bearer | auth_access_token | auth_refresh_token | auth_expires_at_ms |
|-----------|-------------|-------------|-------------------|--------------------|--------------------|
| passthrough | NULL | NULL | NULL | NULL | NULL |
| api_key | value | NULL | NULL | NULL | NULL |
| bearer | NULL | value | NULL | NULL | NULL |
| anthropic_oauth | NULL | NULL | access_token | refresh_token | expires_at_ms |
| openai_oauth | NULL | NULL | access_token | refresh_token | expires_at_ms |
| codex_auto | NULL | NULL | NULL | NULL | NULL |

### Settings key-value map

| key | value | default |
|-----|-------|---------|
| port | "8787" | "8787" |
| proxy_db | path string | CLI flag value |
| pricing_db | path string | `~/.local/share/cli-router/pricing.db` |
| affinity_enabled | "true" / "false" | "true" |
| affinity_headers | JSON array of strings | `["x-session-id","anthropic-session-id"]` |

## Startup Flow

```
1. Parse CLI flags: --db (default: ~/.local/share/cli-router/proxy.db)
2. Open SQLite connection
3. Run migrations (ensure_current — adds v3 tables)
4. Create DbConfigRepository
5. config_repo.load() → Config
   - If DB is empty (first launch): seed settings with defaults, return empty Config
6. Build provider tree from Config
7. Start server on port from settings
```

## Dependency Flow

```
main.rs
  ├── parses CLI flags (--db)
  ├── opens SQLite
  ├── runs migrations
  ├── creates DbConfigRepository (implements ConfigRepository)
  ├── config_repo.load() → Config
  ├── builds LiveProvider from Config
  └── wires AdminState { config_repo, config, live, ... }

AdminState
  ├── UpdateConfig { config_repo, config, live, http }
  ├── CompleteAnthropicOAuth { config_repo, config, live, http }
  ├── CompleteOpenAiOAuth { config_repo, config, live, http }
  └── token_refresh::spawn { config_repo, config, live, http }
```

## DbConfigRepository Implementation

### load()

1. `SELECT key, value FROM settings` → build settings map
2. `SELECT * FROM providers ORDER BY id` → build `Vec<ProviderConfig>`
   - Map auth columns to `AuthConfig` enum based on `auth_type`
3. `SELECT * FROM routing_rules ORDER BY priority` → build `Vec<RoutingRule>`
   - Parse `fallback` (comma-separated) into `Vec<String>`
4. `SELECT * FROM quota_rules` → build `Vec<QuotaRule>`
5. Assemble `Config` struct

### save()

1. Begin SQLite transaction
2. `DELETE FROM settings` → `INSERT` all key-value pairs
3. `DELETE FROM providers` → `INSERT` all providers (auto-increment IDs)
4. `DELETE FROM routing_rules` → `INSERT` all rules with priority
5. `DELETE FROM quota_rules` → `INSERT` all quota rules
6. Commit transaction

Full-replace strategy: no incremental diff. The Config struct is the source of truth
in memory; `save()` writes the full state. This matches the existing behavior where
`UpdateConfig` serializes the entire config.

## Files Changed

### New files

| File | Responsibility |
|------|----------------|
| `crates/proxy/src/application/ports/config.rs` | `ConfigRepository` trait |
| `crates/proxy/src/adapters/storage/db_config.rs` | `DbConfigRepository` implementation |

### Modified files

| File | What changes |
|------|-------------|
| `crates/proxy/src/adapters/storage/schema.rs` | +migration v3 |
| `crates/proxy/src/adapters/storage/mod.rs` | +pub mod db_config |
| `crates/proxy/src/application/ports/mod.rs` | +pub mod config |
| `crates/proxy/src/main.rs` | CLI flag parsing, `config_repo` wiring |
| `crates/proxy/src/application/use_cases/admin.rs` | Replace `config_path: PathBuf` with `config_repo: Arc<dyn ConfigRepository>` |
| `crates/proxy/src/frameworks/admin.rs` | `AdminState` holds `Arc<dyn ConfigRepository>` |
| `crates/proxy/src/adapters/providers/token_refresh.rs` | `spawn()` takes `Arc<dyn ConfigRepository>` |
| `crates/proxy/src/config.rs` | Remove TOML/file logic, keep domain types |
| `crates/proxy/Cargo.toml` | +clap dependency for CLI flag parsing |

### Removed behavior

| What | Why |
|------|-----|
| `Config::from_env()` | Replaced by `config_repo.load()` |
| `Config::resolved_path()` | No config file |
| `Config::legacy_default()` | Seeding handled by `DbConfigRepository` |
| `Config::interpolate_env()` | No `${VAR}` in DB |
| `interpolate()`, `read_service_env_var()` | No env var interpolation needed |
| `config_filename_for_profile()`, `config_dir()` | No config file |
| `CLI_ROUTER_CONFIG`, `CLI_ROUTER_PROFILE` env vars | Replaced by `--db` CLI flag |

## Error Handling

| Scenario | Behavior |
|----------|----------|
| `--db` path doesn't exist | Create it (with parent dirs) |
| DB is empty (first launch) | Seed settings with defaults, return empty providers/routing |
| DB migration fails | Proxy won't start, clear error message |
| `save()` fails mid-transaction | SQLite rolls back, in-memory config unchanged |

## Scope Exclusions

- No migration tool for existing config.toml → DB (users re-enter providers via TUI)
- No TUI changes (TUI already uses admin API for CRUD)
- No admin API changes (payloads stay the same, backend swaps TOML write for DB write)
- No changes to proxy-tui crate (it talks to admin API, unaffected)
