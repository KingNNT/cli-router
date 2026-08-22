#!/usr/bin/env bash
# Tests for .mise/tasks/_lib.sh — OS detection + path setup.
# Run via `mise run test` (or `bash .mise/tasks/tests/test_lib.sh`).

set -uo pipefail

TESTS=0
FAILS=0

assert() {
    local label="$1" actual="$2" expected="$3"
    TESTS=$((TESTS + 1))
    if [ "$actual" = "$expected" ]; then
        printf '  ok   - %s\n' "$label"
    else
        printf '  FAIL - %s (expected %q, got %q)\n' "$label" "$expected" "$actual"
        FAILS=$((FAILS + 1))
    fi
}

# Locate library relative to this script.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
LIB="$ROOT/_lib.sh"

if [ ! -f "$LIB" ]; then
    printf 'FATAL: missing %s\n' "$LIB" >&2
    exit 1
fi

# Source $LIB with UNAME_S=$1 set in the *current* shell.
# Caller is responsible for resetting variables between cases (set -u guard).
source_with_uname() {
    local u="$1"
    # Reset known vars so each case is isolated even though we re-source.
    unset OS SERVICE_LABEL SERVICE_UNIT_PATH SERVICE_TARGET \
          SERVICE_LOG_OUT SERVICE_LOG_ERR SERVICE_BIN PROD_CONFIG SERVICE_ENV_FILE 2>/dev/null || true
    UNAME_S="$u"
    # shellcheck disable=SC1090,SC1091
    . "$LIB"
}

# --- macOS detection ------------------------------------------------
source_with_uname Darwin
assert "OS=macos when UNAME_S=Darwin"          "$OS"                "macos"
assert "macOS SERVICE_LABEL is launchd label"   "$SERVICE_LABEL"     "com.cli-router.proxy"
assert "macOS SERVICE_UNIT_PATH is plist"       "$SERVICE_UNIT_PATH" "$HOME/Library/LaunchAgents/com.cli-router.proxy.plist"
assert "macOS SERVICE_TARGET has gui/<uid>"     "${SERVICE_TARGET#*/}" "$(id -u)/com.cli-router.proxy"
assert "macOS SERVICE_LOG_OUT points to Library/Logs" "$SERVICE_LOG_OUT" "$HOME/Library/Logs/cli-router-proxy.log"
assert "macOS SERVICE_LOG_ERR points to Library/Logs" "$SERVICE_LOG_ERR" "$HOME/Library/Logs/cli-router-proxy.err"
assert "macOS SERVICE_BIN is in cargo bin"      "$SERVICE_BIN"       "$HOME/.cargo/bin/cli-router-proxy"

# --- Linux detection ------------------------------------------------
source_with_uname Linux
assert "OS=linux when UNAME_S=Linux"            "$OS"                "linux"
assert "linux SERVICE_LABEL is unit name"       "$SERVICE_LABEL"     "cli-router-proxy"
assert "linux SERVICE_UNIT_PATH is systemd unit" "$SERVICE_UNIT_PATH" "$HOME/.config/systemd/user/cli-router-proxy.service"
assert "linux SERVICE_BIN is in cargo bin"       "$SERVICE_BIN"       "$HOME/.cargo/bin/cli-router-proxy"

# --- Unsupported OS sets OS empty (caller decides how to error) -----
# The library exits (or returns 1) on unknown OS; the assignment still happens.
(
    set +e
    UNAME_S=Windows
    UNSUPPORTED_OS="$(UNAME_S=Windows bash -c '. "'"$LIB"'"; printf %s "$OS"')"
    # Note: the above won't actually fail; _lib.sh exits 1, but printf
    # in the test subshell still runs first. We capture what's left.
    assert "unsupported OS detection returns empty" "$UNSUPPORTED_OS" ""
)

# --- Shared config paths (both OS) ---------------------------------
assert "PROD_CONFIG is config.toml"             "$PROD_CONFIG"       "$HOME/.config/cli-router/config.toml"
assert "SERVICE_ENV_FILE is .env"                "$SERVICE_ENV_FILE"  "$HOME/.config/cli-router/.env"

# --- Linux service logs are empty (use journalctl) -----------------
source_with_uname Linux
assert "linux SERVICE_LOG_OUT is empty"         "$SERVICE_LOG_OUT"   ""
assert "linux SERVICE_LOG_ERR is empty"         "$SERVICE_LOG_ERR"   ""

printf '\n%s assertions, %s failed\n' "$TESTS" "$FAILS"
exit "$FAILS"
