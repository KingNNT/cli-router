# Mise Tasks Replace Makefile

**Date:** 2026-07-14

## Context

The project currently uses a `Makefile` that includes three partial makefiles under `makefiles/`:

- `makefiles/install.mk` — build and install production binaries
- `makefiles/service.mk` — macOS LaunchAgent service management for the proxy
- `makefiles/dev.mk` — development workflows against a cloned dev DB

The project already has a `.mise.toml` (currently empty except for comments) and a gitignored `.mise.local.toml` for personal environment overrides. This spec migrates every Makefile target to native [mise tasks](https://mise.jdx.dev/tasks/), removing the Makefile entirely.

## Goals

1. Provide every existing `make <target>` command as `mise run <task>`.
2. Organize tasks by environment using file-task directories and `:` namespacing.
3. Keep the repo-level `.mise.toml` minimal; task logic lives in executable scripts.
4. Delete `Makefile` and the `makefiles/` directory.
5. Update project documentation (`CLAUDE.md`, `README.md`) to reference mise tasks.

## Non-goals

- No new build or deployment behavior. Each task reproduces the existing Makefile recipe.
- No cross-platform abstraction for service tasks; they remain macOS-specific.
- No new developer onboarding workflow beyond what the Makefile already provided.

## Design

### File-task layout

File tasks are placed in `.mise/tasks/` and grouped into environment subdirectories. mise auto-discovers executable files in this directory and automatically prefixes subdirectory names with `:`.

```text
.mise/tasks/
├── dev/
│   ├── proxy
│   ├── proxy-tui
│   ├── init
│   ├── reset
│   ├── paths
│   └── seed-requests
├── install/
│   ├── proxy
│   ├── proxy-tui
│   ├── analysis
│   ├── all
│   ├── prod
│   ├── uninstall-proxy
│   ├── uninstall-proxy-tui
│   └── uninstall-analysis
└── service/
    ├── install
    ├── uninstall
    ├── restart
    ├── status
    └── logs
```

Rendered task names:

| Task file | Task name |
|---|---|
| `.mise/tasks/dev/proxy` | `dev:proxy` |
| `.mise/tasks/dev/proxy-tui` | `dev:proxy-tui` |
| `.mise/tasks/dev/init` | `dev:init` |
| `.mise/tasks/dev/reset` | `dev:reset` |
| `.mise/tasks/dev/paths` | `dev:paths` |
| `.mise/tasks/dev/seed-requests` | `dev:seed-requests` |
| `.mise/tasks/install/proxy` | `install:proxy` |
| `.mise/tasks/install/proxy-tui` | `install:proxy-tui` |
| `.mise/tasks/install/analysis` | `install:analysis` |
| `.mise/tasks/install/all` | `install:all` |
| `.mise/tasks/install/prod` | `install:prod` |
| `.mise/tasks/install/uninstall-proxy` | `install:uninstall-proxy` |
| `.mise/tasks/install/uninstall-proxy-tui` | `install:uninstall-proxy-tui` |
| `.mise/tasks/install/uninstall-analysis` | `install:uninstall-analysis` |
| `.mise/tasks/service/install` | `service:install` |
| `.mise/tasks/service/uninstall` | `service:uninstall` |
| `.mise/tasks/service/restart` | `service:restart` |
| `.mise/tasks/service/status` | `service:status` |
| `.mise/tasks/service/logs` | `service:logs` |

### `.mise.toml`

File tasks are auto-discovered from `.mise/tasks/`, so `.mise.toml` only needs the existing header comment:

```toml
# cli-router — mise config
# Put personal dev overrides in .mise.local.toml (gitignored).
# mise auto-loads .mise.local.toml if it exists.
```

### Task behavior

#### Install tasks

- `install:proxy`
  - Run `cargo install --locked --force --path crates/proxy`.
  - If `~/Library/LaunchAgents/com.cli-router.proxy.plist` exists, restart the service so the new binary is picked up.
  - Description: "Install the proxy binary into ~/.cargo/bin (restarts service if loaded)".
  - Alias: `ip`.

- `install:proxy-tui`
  - Run `cargo install --locked --force --path crates/proxy-tui`.
  - Description: "Install the proxy-tui binary into ~/.cargo/bin".
  - Alias: `ipt`.

- `install:analysis`
  - Run `cargo install --locked --force --path crates/analysis`.
  - Description: "Install the analysis binary into ~/.cargo/bin".
  - Alias: `ia`.

- `install:all`
  - Depends on `install:proxy`, `install:proxy-tui`, and `install:analysis`.
  - Description: "Install all cli-router binaries globally".
  - Alias: `iall`.

- `install:prod`
  - Depends on `install:proxy-tui`, `install:analysis`, and `service:install`.
  - This reproduces the original `prod` target semantics: install every binary and register the proxy service. The proxy binary is installed by `service:install`, avoiding a redundant `install:proxy` step.
  - Description: "Ship to production: install all binaries and register the proxy service".
  - Alias: `prod`.

- `install:uninstall-proxy`
  - Run `cargo uninstall proxy`.
  - Description: "Uninstall the proxy binary".
  - Alias: `unp`.

- `install:uninstall-proxy-tui`
  - Run `cargo uninstall proxy-tui`.
  - Description: "Uninstall the proxy-tui binary".
  - Alias: `unpt`.

- `install:uninstall-analysis`
  - Run `cargo uninstall analysis` (ignore errors to match the original Makefile behavior).
  - Description: "Uninstall the analysis binary".
  - Alias: `una`.

#### Service tasks (macOS only)

- `service:install`
  - Install the proxy binary (`cargo install --locked --force --path crates/proxy`).
  - Generate `~/Library/LaunchAgents/com.cli-router.proxy.plist` with the same environment-variable resolution logic as the original Makefile.
  - Bootstrap the service if not loaded, otherwise kickstart it atomically.
  - Clear the stderr log file after loading.
  - Description: "Build proxy, install it, and run it as a LaunchAgent (one-shot deploy)".
  - Alias: `si`.

- `service:uninstall`
  - Run `launchctl bootout` if the plist exists.
  - Remove the plist file.
  - Description: "Stop and remove the proxy LaunchAgent".
  - Alias: `su`.

- `service:restart`
  - Validate the plist exists; error if not.
  - Kickstart if loaded, otherwise bootstrap.
  - Clear the stderr log file.
  - Description: "Restart the proxy service (atomic kickstart)".
  - Alias: `sr`.

- `service:status`
  - Run `launchctl list` filtered to `com.cli-router.proxy`.
  - Print "(not loaded)" if absent.
  - Description: "Show launchd status for the proxy service".
  - Alias: `ss`.

- `service:logs`
  - Touch the stdout and stderr log files, then `tail -f` both.
  - Description: "Tail proxy service stdout + stderr logs".
  - Alias: `sl`.

#### Dev tasks

- `dev:proxy`
  - Source `./.env` if present.
  - Error if the dev DB (`~/.local/share/cli-router/dev/proxy.db`) does not exist.
  - Run `cargo run -p proxy -- --db <dev DB>`.
  - Description: "Run proxy in foreground against the dev DB (port 8788)".
  - Alias: `dp`.

- `dev:proxy-tui`
  - Source `./.env` if present.
  - Set `CLI_ROUTER_PROXY_URL` to `http://127.0.0.1:8788` if not already set.
  - Run `cargo run -p proxy-tui`.
  - Description: "Run proxy-tui pointed at the dev proxy".
  - Alias: `dpt`.

- `dev:init`
  - Error if the dev DB already exists; error if the prod DB does not exist.
  - Copy `~/.local/share/cli-router/proxy.db` to `~/.local/share/cli-router/dev/proxy.db`.
  - Override `settings` rows: `port=8788`, `proxy_db=<dev DB>`, `pricing_db=<dev data dir>/pricing.db`.
  - Delete `requests` and `api_keys` from the cloned dev DB.
  - Print the dev port and data dir.
  - Description: "Clone prod DB into dev with port override".
  - Alias: `di`.

- `dev:reset`
  - Remove `~/.local/share/cli-router/dev/`.
  - Run `dev:init`.
  - Description: "Delete dev DBs, then re-clone from prod".
  - Alias: `dr`.

- `dev:paths`
  - Print resolved dev and prod DB paths, ports, and service status.
  - Description: "Print resolved dev paths and ports".
  - Alias: `dpaths`.

- `dev:seed-requests`
  - Error if the dev DB does not exist.
  - Print the request count before seeding.
  - Run `sqlite3 <dev DB> < scripts/seed-dev-requests.sql`.
  - Print the request count after seeding.
  - Description: "Insert mock requests into the dev DB".
  - Alias: `dseed`.

### Common constants

Each script is self-contained and declares the constants it needs. The following paths and labels are repeated across multiple scripts:

```bash
LAUNCH_AGENT_DIR="$HOME/Library/LaunchAgents"
SERVICE_LABEL="com.cli-router.proxy"
SERVICE_PLIST="$LAUNCH_AGENT_DIR/$SERVICE_LABEL.plist"
SERVICE_BIN="$HOME/.cargo/bin/cli-router-proxy"
SERVICE_LOG_DIR="$HOME/Library/Logs"
SERVICE_LOG_OUT="$SERVICE_LOG_DIR/cli-router-proxy.log"
SERVICE_LOG_ERR="$SERVICE_LOG_DIR/cli-router-proxy.err"
SERVICE_DOMAIN="gui/$(id -u)"
SERVICE_TARGET="$SERVICE_DOMAIN/$SERVICE_LABEL"
PROD_CONFIG="$HOME/.config/cli-router/config.toml"
SERVICE_ENV_FILE="$HOME/.config/cli-router/.env"

DEV_PORT=8788
DEV_DATA_DIR="$HOME/.local/share/cli-router/dev"
DEV_DB="$DEV_DATA_DIR/proxy.db"
PROD_DB="$HOME/.local/share/cli-router/proxy.db"
```

If the repetition becomes awkward, a shared `.mise/tasks/lib/common.sh` can be sourced by scripts. For the initial migration, scripts will inline these values to keep each file readable without indirection.

## Implementation plan

1. Create `.mise/tasks/` directory and environment subdirectories.
2. Create each executable task script with the appropriate `#MISE` metadata (description, alias).
3. Update `.mise.toml` to keep the minimal header comment.
4. Delete `Makefile`, `makefiles/install.mk`, `makefiles/service.mk`, `makefiles/dev.mk`, and the empty `makefiles/` directory.
5. Update `CLAUDE.md` to replace the `make` command examples with `mise run` equivalents.
6. Update `README.md` to reference mise tasks instead of Makefile targets.
7. Run `mise tasks` to verify all tasks are discovered and named correctly.
8. Run selected tasks (e.g., `mise run dev:paths`, `mise run install:analysis --help`) to verify basic behavior.

## Success criteria

- `mise tasks` lists all 19 tasks with the names and descriptions above.
- `make` no longer exists in the repository root.
- `CLAUDE.md` and `README.md` refer to `mise run <task>` instead of `make <target>`.
- Each task script is executable and runs without syntax errors when invoked.

## References

- [mise Tasks documentation](https://mise.jdx.dev/tasks/)
- [mise File Tasks documentation](https://mise.jdx.dev/tasks/file-tasks.html)
- [mise Running Tasks documentation](https://mise.jdx.dev/tasks/running-tasks.html)
- Existing Makefile targets in `makefiles/{install,service,dev}.mk`
