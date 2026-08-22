#!/usr/bin/env bash
# _lib.sh — sourced by service/* scripts to get OS detection + path vars.
# Test by setting UNAME_S before sourcing.

# When sourced, set -e etc. belong to the caller; this library only uses
# parameter expansion so it is safe under any shell settings.

_os_from_uname() {
    case "${UNAME_S:-$(uname -s)}" in
        Darwin) printf '%s' macos ;;
        Linux)  printf '%s' linux ;;
        *)      printf '' ;;
    esac
}

OS="$(_os_from_uname)"
if [ -z "$OS" ]; then
    printf 'error: unsupported OS %q. Supported: macOS (Darwin), Linux.\n' "${UNAME_S:-$(uname -s)}" >&2
    return 1 2>/dev/null || exit 1
fi

# Shared by both platforms
SERVICE_BIN="$HOME/.cargo/bin/cli-router-proxy"

case "$OS" in
    macos)
        SERVICE_LABEL="com.cli-router.proxy"
        SERVICE_UNIT_PATH="$HOME/Library/LaunchAgents/com.cli-router.proxy.plist"
        SERVICE_TARGET="gui/$(id -u)/$SERVICE_LABEL"
        SERVICE_LOG_OUT="$HOME/Library/Logs/cli-router-proxy.log"
        SERVICE_LOG_ERR="$HOME/Library/Logs/cli-router-proxy.err"
        ;;
    linux)
        SERVICE_LABEL="cli-router-proxy"
        SERVICE_UNIT_PATH="$HOME/.config/systemd/user/cli-router-proxy.service"
        SERVICE_TARGET=""  # not used; systemd handles by unit name
        SERVICE_LOG_OUT="" # not used; journalctl -u <unit> serves logs
        SERVICE_LOG_ERR=""
        ;;
esac

# Tell consumers where to put runtime state (logs dir, env file).
PROD_CONFIG="$HOME/.config/cli-router/config.toml"
SERVICE_ENV_FILE="$HOME/.config/cli-router/.env"
