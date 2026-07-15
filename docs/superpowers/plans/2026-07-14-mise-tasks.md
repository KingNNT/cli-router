> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the existing `Makefile` and `makefiles/` directory with native mise file tasks under `mise-tasks/`, organized by environment.

**Architecture:** Move every Makefile recipe into an executable bash script inside `mise-tasks/{dev,install,service}/`. mise auto-discovers these files and applies `:` namespace prefixes from subdirectory names. Composite tasks (e.g., `install:all`, `install:prod`, `dev:reset`) use mise `depends` so they reuse smaller tasks instead of duplicating code. The repo-level `.mise.toml` stays minimal because file tasks are auto-discovered.

**Tech Stack:** mise, bash, cargo, sqlite3, launchctl (macOS service tasks only).

---

# Mise Tasks Replace Makefile — Implementation Plan

## File map

| File | Responsibility |
|------|--------------|
| `mise-tasks/install/proxy` | Install proxy binary; restart service if already loaded |
| `mise-tasks/install/proxy-tui` | Install proxy-tui binary |
| `mise-tasks/install/analysis` | Install analysis binary |
| `mise-tasks/install/all` | Composite: depends on all three install tasks |
| `mise-tasks/install/prod` | Composite: production deploy (proxy-tui + analysis + service install) |
| `mise-tasks/install/uninstall-proxy` | Uninstall proxy package |
| `mise-tasks/install/uninstall-proxy-tui` | Uninstall proxy-tui package |
| `mise-tasks/install/uninstall-analysis` | Uninstall analysis package (ignore errors) |
| `mise-tasks/service/install` | Install proxy, generate LaunchAgent plist, bootstrap/kickstart |
| `mise-tasks/service/uninstall` | Stop and remove LaunchAgent |
| `mise-tasks/service/restart` | Atomic kickstart or bootstrap |
| `mise-tasks/service/status` | Show launchd status |
| `mise-tasks/service/logs` | Tail stdout + stderr logs |
| `mise-tasks/dev/proxy` | Run proxy against dev DB |
| `mise-tasks/dev/proxy-tui` | Run proxy-tui against dev proxy |
| `mise-tasks/dev/init` | Clone prod DB into dev with port override |
| `mise-tasks/dev/reset` | Remove dev DB and re-clone from prod |
| `mise-tasks/dev/paths` | Print resolved paths |
| `mise-tasks/dev/seed-requests` | Insert mock requests into dev DB |
| `.mise.toml` | Existing header comment; no task definitions needed |
| `Makefile` | Delete |
| `makefiles/install.mk` | Delete |
| `makefiles/service.mk` | Delete |
| `makefiles/dev.mk` | Delete |
| `CLAUDE.md` | Add a "Mise tasks" section |
| `README.md` | Add a "Task runner" section |

---

### Task 1: Create directory structure

**Files:**
- Create: `mise-tasks/dev/`
- Create: `mise-tasks/install/`
- Create: `mise-tasks/service/`

- [ ] **Step 1: Create the three environment subdirectories**

Run:

```bash
mkdir -p mise-tasks/{dev,install,service}
```

Expected: Three directories exist under `mise-tasks/`.

- [ ] **Step 2: Commit the skeleton**

```bash
git add mise-tasks
git commit -m "chore: add mise-tasks directory structure"
```

---

### Task 2: Create install tasks

**Files:**
- Create: `mise-tasks/install/proxy`
- Create: `mise-tasks/install/proxy-tui`
- Create: `mise-tasks/install/analysis`
- Create: `mise-tasks/install/all`
- Create: `mise-tasks/install/prod`
- Create: `mise-tasks/install/uninstall-proxy`
- Create: `mise-tasks/install/uninstall-proxy-tui`
- Create: `mise-tasks/install/uninstall-analysis`

- [ ] **Step 1: Write `mise-tasks/install/proxy`**

```bash
#!/usr/bin/env bash
#MISE description="Install the proxy binary into ~/.cargo/bin (restarts service if loaded)"
#MISE alias="ip"

set -euo pipefail

CARGO="${CARGO:-cargo}"
SERVICE_PLIST="$HOME/Library/LaunchAgents/com.cli-router.proxy.plist"

"$CARGO" install --locked --force --path crates/proxy

if [ -f "$SERVICE_PLIST" ]; then
    echo "service plist detected — restarting to pick up new binary"
    mise run service:restart
fi
```

- [ ] **Step 2: Write `mise-tasks/install/proxy-tui`**

```bash
#!/usr/bin/env bash
#MISE description="Install the proxy-tui binary into ~/.cargo/bin"
#MISE alias="ipt"

set -euo pipefail

CARGO="${CARGO:-cargo}"
"$CARGO" install --locked --force --path crates/proxy-tui
```

- [ ] **Step 3: Write `mise-tasks/install/analysis`**

```bash
#!/usr/bin/env bash
#MISE description="Install the analysis binary into ~/.cargo/bin"
#MISE alias="ia"

set -euo pipefail

CARGO="${CARGO:-cargo}"
"$CARGO" install --locked --force --path crates/analysis
```

- [ ] **Step 4: Write `mise-tasks/install/all`**

```bash
#!/usr/bin/env bash
#MISE description="Install all cli-router binaries globally"
#MISE alias="iall"
#MISE depends=["install:proxy", "install:proxy-tui", "install:analysis"]

set -euo pipefail

echo "All cli-router binaries installed."
```

- [ ] **Step 5: Write `mise-tasks/install/prod`**

```bash
#!/usr/bin/env bash
#MISE description="Ship to production: install all binaries and register the proxy service"
#MISE alias="prod"
#MISE depends=["install:proxy-tui", "install:analysis", "service:install"]

set -euo pipefail

echo "Production deployment complete."
```

- [ ] **Step 6: Write `mise-tasks/install/uninstall-proxy`**

```bash
#!/usr/bin/env bash
#MISE description="Uninstall the proxy binary"
#MISE alias="unp"

set -euo pipefail

CARGO="${CARGO:-cargo}"
"$CARGO" uninstall proxy
```

- [ ] **Step 7: Write `mise-tasks/install/uninstall-proxy-tui`**

```bash
#!/usr/bin/env bash
#MISE description="Uninstall the proxy-tui binary"
#MISE alias="unpt"

set -euo pipefail

CARGO="${CARGO:-cargo}"
"$CARGO" uninstall proxy-tui
```

- [ ] **Step 8: Write `mise-tasks/install/uninstall-analysis`**

```bash
#!/usr/bin/env bash
#MISE description="Uninstall the analysis binary"
#MISE alias="una"

set -euo pipefail

CARGO="${CARGO:-cargo}"
"$CARGO" uninstall analysis || true
```

- [ ] **Step 9: Make install task files executable**

Run:

```bash
chmod +x mise-tasks/install/*
```

Expected: All eight files in `mise-tasks/install/` are executable.

- [ ] **Step 10: Verify install tasks are discovered**

Run:

```bash
mise tasks | grep '^install:'
```

Expected output includes:

```
install:all
install:analysis
install:prod
install:proxy
install:proxy-tui
install:uninstall-analysis
install:uninstall-proxy
install:uninstall-proxy-tui
```

- [ ] **Step 11: Commit install tasks**

```bash
git add mise-tasks/install
git commit -m "feat(tasks): add mise install tasks"
```

---

### Task 3: Create service tasks

**Files:**
- Create: `mise-tasks/service/install`
- Create: `mise-tasks/service/uninstall`
- Create: `mise-tasks/service/restart`
- Create: `mise-tasks/service/status`
- Create: `mise-tasks/service/logs`

- [ ] **Step 1: Write `mise-tasks/service/install`**

```bash
#!/usr/bin/env bash
#MISE description="Build proxy, install it, and run it as a LaunchAgent (one-shot deploy)"
#MISE alias="si"

set -euo pipefail

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

CARGO="${CARGO:-cargo}"
"$CARGO" install --locked --force --path crates/proxy

mkdir -p "$LAUNCH_AGENT_DIR" "$SERVICE_LOG_DIR"

{
    printf '%s\n' \
        '<?xml version="1.0" encoding="UTF-8"?>' \
        '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">' \
        '<plist version="1.0">' \
        '<dict>' \
        '  <key>Label</key><string>'"$SERVICE_LABEL"'</string>' \
        '  <key>ProgramArguments</key>' \
        '  <array><string>'"$SERVICE_BIN"'</string></array>' \
        '  <key>RunAtLoad</key><true/>' \
        '  <key>KeepAlive</key><true/>' \
        '  <key>StandardOutPath</key><string>'"$SERVICE_LOG_OUT"'</string>' \
        '  <key>StandardErrorPath</key><string>'"$SERVICE_LOG_ERR"'</string>'

    count=0
    if [ -f "$PROD_CONFIG" ]; then
        var_names=$(grep -oE '\$\{[A-Za-z_][A-Za-z0-9_]*\}' "$PROD_CONFIG" 2>/dev/null \
            | sed 's/\${//;s/}//' | sort -u || true)
        for var in $var_names; do
            val=""
            source=""
            if [ -f "$SERVICE_ENV_FILE" ]; then
                val=$(grep -E "^$var=" "$SERVICE_ENV_FILE" 2>/dev/null | head -1 | cut -d= -f2-)
            fi
            if [ -n "$val" ]; then
                source=".env"
            else
                eval "val=\${$var:-}"
                if [ -n "$val" ]; then source="env"; fi
            fi
            if [ -n "$val" ]; then
                if [ "$count" -eq 0 ]; then
                    printf '  <key>EnvironmentVariables</key>\n  <dict>\n'
                fi
                printf '    <key>%s</key><string>%s</string>\n' "$var" "$val"
                echo "  $var <- $source" >&2
                count=$((count + 1))
            else
                echo "  warning: $var referenced in config but not set (neither .env nor env)" >&2
            fi
        done
    fi
    if [ "$count" -gt 0 ]; then
        printf '  </dict>\n'
    fi

    printf '%s\n' \
        '</dict>' \
        '</plist>'
} > "$SERVICE_PLIST"

if launchctl print "$SERVICE_TARGET" >/dev/null 2>&1; then
    echo "service already loaded — kickstarting (atomic respawn, no AddrInUse race)"
    launchctl kickstart -k "$SERVICE_TARGET"
else
    echo "first-time install — bootstrapping"
    launchctl bootstrap "$SERVICE_DOMAIN" "$SERVICE_PLIST"
fi

: > "$SERVICE_LOG_ERR"
echo "service installed: $SERVICE_PLIST"
echo "logs: $SERVICE_LOG_OUT"
echo "      $SERVICE_LOG_ERR"
```

- [ ] **Step 2: Write `mise-tasks/service/uninstall`**

```bash
#!/usr/bin/env bash
#MISE description="Stop and remove the proxy LaunchAgent"
#MISE alias="su"

set -euo pipefail

SERVICE_LABEL="com.cli-router.proxy"
SERVICE_PLIST="$HOME/Library/LaunchAgents/$SERVICE_LABEL.plist"
SERVICE_DOMAIN="gui/$(id -u)"
SERVICE_TARGET="$SERVICE_DOMAIN/$SERVICE_LABEL"

if [ -f "$SERVICE_PLIST" ]; then
    launchctl bootout "$SERVICE_TARGET" 2>/dev/null || true
    rm -f "$SERVICE_PLIST"
    echo "service uninstalled"
else
    echo "service not installed"
fi
```

- [ ] **Step 3: Write `mise-tasks/service/restart`**

```bash
#!/usr/bin/env bash
#MISE description="Restart the proxy service (atomic kickstart)"
#MISE alias="sr"

set -euo pipefail

SERVICE_LABEL="com.cli-router.proxy"
SERVICE_PLIST="$HOME/Library/LaunchAgents/$SERVICE_LABEL.plist"
SERVICE_DOMAIN="gui/$(id -u)"
SERVICE_TARGET="$SERVICE_DOMAIN/$SERVICE_LABEL"
SERVICE_LOG_ERR="$HOME/Library/Logs/cli-router-proxy.err"

if [ ! -f "$SERVICE_PLIST" ]; then
    echo "error: service not installed. Run 'mise run service:install' first."
    exit 1
fi

if launchctl print "$SERVICE_TARGET" >/dev/null 2>&1; then
    launchctl kickstart -k "$SERVICE_TARGET"
else
    launchctl bootstrap "$SERVICE_DOMAIN" "$SERVICE_PLIST"
fi

: > "$SERVICE_LOG_ERR"
echo "service restarted"
```

- [ ] **Step 4: Write `mise-tasks/service/status`**

```bash
#!/usr/bin/env bash
#MISE description="Show launchd status for the proxy service"
#MISE alias="ss"

set -euo pipefail

SERVICE_LABEL="com.cli-router.proxy"

output=$(launchctl list | awk -v label="$SERVICE_LABEL" 'NR==1 || $0 ~ label')
header=$(echo "$output" | head -1)
lines=$(echo "$output" | tail -n +2)

echo "$header"
if [ -z "$lines" ]; then
    echo "  (not loaded)"
else
    echo "$lines"
fi
```

- [ ] **Step 5: Write `mise-tasks/service/logs`**

```bash
#!/usr/bin/env bash
#MISE description="Tail proxy service stdout + stderr logs"
#MISE alias="sl"

set -euo pipefail

SERVICE_LOG_OUT="$HOME/Library/Logs/cli-router-proxy.log"
SERVICE_LOG_ERR="$HOME/Library/Logs/cli-router-proxy.err"

touch "$SERVICE_LOG_OUT" "$SERVICE_LOG_ERR"
tail -f "$SERVICE_LOG_OUT" "$SERVICE_LOG_ERR"
```

- [ ] **Step 6: Make service task files executable**

Run:

```bash
chmod +x mise-tasks/service/*
```

Expected: All five files in `mise-tasks/service/` are executable.

- [ ] **Step 7: Verify service tasks are discovered**

Run:

```bash
mise tasks | grep '^service:'
```

Expected output includes:

```
service:install
service:logs
service:restart
service:status
service:uninstall
```

- [ ] **Step 8: Commit service tasks**

```bash
git add mise-tasks/service
git commit -m "feat(tasks): add mise service tasks"
```

---

### Task 4: Create dev tasks

**Files:**
- Create: `mise-tasks/dev/proxy`
- Create: `mise-tasks/dev/proxy-tui`
- Create: `mise-tasks/dev/init`
- Create: `mise-tasks/dev/reset`
- Create: `mise-tasks/dev/paths`
- Create: `mise-tasks/dev/seed-requests`

- [ ] **Step 1: Write `mise-tasks/dev/proxy`**

```bash
#!/usr/bin/env bash
#MISE description="Run proxy in foreground against the dev DB (port 8788)"
#MISE alias="dp"

set -euo pipefail

DEV_DB="$HOME/.local/share/cli-router/dev/proxy.db"

if [ ! -f "$DEV_DB" ]; then
    echo "no dev DB at $DEV_DB. Run 'mise run dev:init' first."
    exit 1
fi

if [ -f .env ]; then
    set -a
    . ./.env
    set +a
fi

cargo run -p proxy -- --db "$DEV_DB"
```

- [ ] **Step 2: Write `mise-tasks/dev/proxy-tui`**

```bash
#!/usr/bin/env bash
#MISE description="Run proxy-tui pointed at the dev proxy"
#MISE alias="dpt"

set -euo pipefail

if [ -f .env ]; then
    set -a
    . ./.env
    set +a
fi

: "${CLI_ROUTER_PROXY_URL:=http://127.0.0.1:8788}"
export CLI_ROUTER_PROXY_URL

cargo run -p proxy-tui
```

- [ ] **Step 3: Write `mise-tasks/dev/init`**

```bash
#!/usr/bin/env bash
#MISE description="Clone prod DB into dev with port override"
#MISE alias="di"

set -euo pipefail

DEV_PORT=8788
DEV_DATA_DIR="$HOME/.local/share/cli-router/dev"
DEV_DB="$DEV_DATA_DIR/proxy.db"
PROD_DB="$HOME/.local/share/cli-router/proxy.db"

if [ -f "$DEV_DB" ]; then
    echo "error: $DEV_DB already exists. Run 'mise run dev:reset' to recreate."
    exit 1
fi

if [ ! -f "$PROD_DB" ]; then
    echo "error: no prod DB at $PROD_DB."
    exit 1
fi

mkdir -p "$DEV_DATA_DIR"
cp "$PROD_DB" "$DEV_DB"

sqlite3 "$DEV_DB" <<EOF
INSERT OR REPLACE INTO settings (key, value) VALUES ('port', '$DEV_PORT');
INSERT OR REPLACE INTO settings (key, value) VALUES ('proxy_db', '$DEV_DB');
INSERT OR REPLACE INTO settings (key, value) VALUES ('pricing_db', '$DEV_DATA_DIR/pricing.db');
DELETE FROM requests;
DELETE FROM api_keys;
EOF

echo "cloned prod DB -> $DEV_DB"
echo "  port:     $DEV_PORT"
echo "  data dir: $DEV_DATA_DIR"
echo ""
echo "providers/routing were copied from prod — edit via the admin API to change them."
```

- [ ] **Step 4: Write `mise-tasks/dev/reset`**

```bash
#!/usr/bin/env bash
#MISE description="Delete dev DBs, then re-clone from prod"
#MISE alias="dr"

set -euo pipefail

DEV_DATA_DIR="$HOME/.local/share/cli-router/dev"

rm -rf "$DEV_DATA_DIR"
mise run dev:init
```

- [ ] **Step 5: Write `mise-tasks/dev/paths`**

```bash
#!/usr/bin/env bash
#MISE description="Print resolved dev paths and ports"
#MISE alias="dpaths"

set -euo pipefail

DEV_DATA_DIR="$HOME/.local/share/cli-router/dev"
DEV_DB="$DEV_DATA_DIR/proxy.db"
PROD_DB="$HOME/.local/share/cli-router/proxy.db"

echo "dev DB:       $DEV_DB"
echo "dev data dir: $DEV_DATA_DIR"
echo "dev port:     8788"
echo "prod DB:      $PROD_DB"
echo "prod port:    8787 (managed by launchd via mise run service:install)"
```

- [ ] **Step 6: Write `mise-tasks/dev/seed-requests`**

```bash
#!/usr/bin/env bash
#MISE description="Insert mock requests into the dev DB"
#MISE alias="dseed"

set -euo pipefail

DEV_DB="$HOME/.local/share/cli-router/dev/proxy.db"

if [ ! -f "$DEV_DB" ]; then
    echo "error: $DEV_DB not found. Run 'mise run dev:init' first."
    exit 1
fi

count_before=$(sqlite3 "$DEV_DB" "SELECT COUNT(*) FROM requests;" 2>/dev/null || echo 0)
echo "requests before seed: $count_before"

sqlite3 "$DEV_DB" < scripts/seed-dev-requests.sql

count_after=$(sqlite3 "$DEV_DB" "SELECT COUNT(*) FROM requests;")
echo "requests after seed:  $count_after"
```

- [ ] **Step 7: Make dev task files executable**

Run:

```bash
chmod +x mise-tasks/dev/*
```

Expected: All six files in `mise-tasks/dev/` are executable.

- [ ] **Step 8: Verify dev tasks are discovered**

Run:

```bash
mise tasks | grep '^dev:'
```

Expected output includes:

```
dev:init
dev:paths
dev:proxy
dev:proxy-tui
dev:reset
dev:seed-requests
```

- [ ] **Step 9: Commit dev tasks**

```bash
git add mise-tasks/dev
git commit -m "feat(tasks): add mise dev tasks"
```

---

### Task 5: Remove Makefile and makefiles/

**Files:**
- Delete: `Makefile`
- Delete: `makefiles/install.mk`
- Delete: `makefiles/service.mk`
- Delete: `makefiles/dev.mk`
- Delete: `makefiles/` (after files removed)

- [ ] **Step 1: Delete the Makefile and included makefiles**

Run:

```bash
rm -f Makefile makefiles/install.mk makefiles/service.mk makefiles/dev.mk
rmdir makefiles 2>/dev/null || true
```

- [ ] **Step 2: Verify no Makefile or makefiles directory remains**

Run:

```bash
ls -la Makefile 2>/dev/null || echo "Makefile removed"
ls -la makefiles 2>/dev/null || echo "makefiles/ removed"
```

Expected: Both are gone.

- [ ] **Step 3: Commit the removal**

```bash
git add -u
git commit -m "chore: remove Makefile and makefiles directory"
```

---

### Task 6: Update `.mise.toml`

**Files:**
- Modify: `.mise.toml`

- [ ] **Step 1: Ensure `.mise.toml` contains the minimal header comment**

The file should read exactly:

```toml
# cli-router — mise config
# Put personal dev overrides in .mise.local.toml (gitignored).
# mise auto-loads .mise.local.toml if it exists.
```

If it already does, no change is needed. If not, overwrite it with the content above.

- [ ] **Step 2: Commit `.mise.toml`**

```bash
git add .mise.toml
git commit -m "chore(tasks): ensure .mise.toml header is present"
```

---

### Task 7: Update `CLAUDE.md`

**Files:**
- Modify: `CLAUDE.md` (add a "Mise tasks" section after the existing "Commands" section)

- [ ] **Step 1: Insert the new section after the existing commands block**

Find this block in `CLAUDE.md`:

```markdown
# Coverage (requires `cargo install cargo-llvm-cov`)
cargo coverage                # summary in terminal
cargo coverage-html           # open HTML report
```

Release binaries land at `target/release/{cli-router-proxy,cli-router-analysis,cli-router-proxy-tui}` after `cargo build --release --workspace`.
```

Insert the following immediately after it (before the next `### Architecture` or similar heading):

```markdown
## Mise tasks

Development, install, and service workflows are exposed as [mise](https://mise.jdx.dev/) file tasks in `mise-tasks/`:

```bash
# Development
mise run dev:init          # clone prod DB into dev with port override
mise run dev:proxy         # run proxy in foreground against dev DB
mise run dev:proxy-tui     # run proxy-tui pointed at dev proxy
mise run dev:reset         # delete dev DBs and re-clone from prod
mise run dev:paths         # print resolved dev paths and ports
mise run dev:seed-requests # insert mock requests into dev DB

# Install
mise run install:all       # install proxy, proxy-tui, and analysis
mise run install:prod      # install all binaries and register launchd service
mise run install:proxy     # install proxy binary (restarts service if loaded)

# Service (macOS)
mise run service:install   # build proxy, install, and run as LaunchAgent
mise run service:uninstall # stop and remove the LaunchAgent
mise run service:restart   # restart the proxy service
mise run service:status    # show launchd status
mise run service:logs      # tail service stdout + stderr logs
```

Run `mise tasks` for the full list and short aliases (e.g., `mise run dp` for `dev:proxy`).
```

- [ ] **Step 2: Verify the section renders correctly**

Run:

```bash
grep -A 5 "## Mise tasks" CLAUDE.md
```

Expected: The new section and command block are present.

- [ ] **Step 3: Commit the CLAUDE.md update**

```bash
git add CLAUDE.md
git commit -m "docs: document mise tasks in CLAUDE.md"
```

---

### Task 8: Update `README.md`

**Files:**
- Modify: `README.md` (add a "Task runner" section after "Build & run")

- [ ] **Step 1: Insert the new section after the "Build & run" block**

Find this block in `README.md`:

```markdown
Release binaries: `target/release/cli-router-proxy`, `target/release/cli-router-proxy-tui`, and `target/release/cli-router-analysis` after `cargo build --release --workspace`.
```

Insert the following immediately after it:

```markdown
## Task runner

The project uses [mise](https://mise.jdx.dev/) for development, install, and service tasks:

```bash
mise run dev:proxy          # Run proxy in foreground against dev DB
mise run dev:proxy-tui      # Run proxy-tui against dev proxy
mise run dev:init           # Clone prod DB into dev
mise run dev:reset          # Delete dev DBs and re-clone from prod
mise run install:prod       # Install binaries and register launchd service
mise run service:status     # Show launchd service status
mise run service:logs       # Tail service logs
```

Run `mise tasks` for the full list.
```

- [ ] **Step 2: Verify the section renders correctly**

Run:

```bash
grep -A 5 "## Task runner" README.md
```

Expected: The new section and command block are present.

- [ ] **Step 3: Commit the README.md update**

```bash
git add README.md
git commit -m "docs: document mise tasks in README.md"
```

---

### Task 9: Verify tasks end-to-end

**Files:**
- All `mise-tasks/` scripts (indirectly)

- [ ] **Step 1: List all tasks and confirm 19 tasks are present**

Run:

```bash
mise tasks
```

Expected: 19 tasks with names `dev:*`, `install:*`, and `service:*` plus short aliases.

- [ ] **Step 2: Run a read-only task to verify execution**

Run:

```bash
mise run dev:paths
```

Expected: Prints dev DB, data dir, port, and prod DB paths without errors.

- [ ] **Step 3: Verify a composite task resolves dependencies**

Run:

```bash
mise run install:all --help
```

Expected: mise prints help for the task and shows its dependencies (or dry-run output). Alternatively run:

```bash
mise tasks deps install:all
```

Expected: Lists `install:proxy`, `install:proxy-tui`, and `install:analysis` as dependencies.

- [ ] **Step 4: Verify task files are executable**

Run:

```bash
find mise-tasks -type f -perm +111
```

Expected: Lists all 19 task files (some platforms may need `-executable` instead of `-perm +111`).

- [ ] **Step 5: Commit any verification fixes**

If any of the above steps required changes, commit them:

```bash
git add -A
git commit -m "fix(tasks): verification fixes for mise tasks"
```

---

## Spec coverage review

| Spec requirement | Implementing task |
|--------------------|-----------------|
| Provide every `make` target as `mise run <task>` | Tasks 2, 3, 4 |
| Organize tasks by environment with `:` namespacing | Task 1, 2, 3, 4 |
| Keep `.mise.toml` minimal | Task 6 |
| Delete `Makefile` and `makefiles/` | Task 5 |
| Update `CLAUDE.md` | Task 7 |
| Update `README.md` | Task 8 |
| Verify discovery and execution | Task 9 |

## Placeholder scan

- No "TBD", "TODO", or "implement later" entries.
- Every script file includes complete, runnable bash.
- Every command step includes expected output.
- No references to undefined functions or variables.

## Type / naming consistency

- All task files use `#!/usr/bin/env bash`, `set -euo pipefail`, and `#MISE` metadata.
- All task names follow the `env:action` or `env:action-subject` convention.
- Aliases are unique and match the design spec.
- Composite tasks use `depends` with the exact task names defined in the plan.

## Execution handoff

**Plan complete and saved to `docs/superpowers/plans/2026-07-14-mise-tasks.md`. Two execution options:**

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — Execute tasks in this session using `executing-plans`, batch execution with checkpoints.

Which approach would you like?
