# Migrate from direnv to mise

**Date:** 2026-06-12
**Status:** Approved

## Motivation

The project uses direnv (`.envrc` + `.envrc.local`) for shell environment variable auto-loading on `cd`. mise provides the same capability with `.mise.toml` + `.mise.local.toml`, and is already installed on the machine. Removing direnv eliminates a dependency and consolidates tooling.

## Scope

- **In scope:** Replace direnv env var management with mise env management.
- **Out of scope:** Rust toolchain management (stays with `rust-toolchain.toml`, which mise reads natively), Makefile changes, `.env` file mechanics.

## Decisions

1. **One-to-one migration** — `.mise.toml` replaces `.envrc`, `.mise.local.toml` replaces `.envrc.local`.
2. **Clean cut** — Delete `.envrc` and `.envrc.local` immediately. No deprecation period.
3. **Only env var management** — mise is not used for Rust toolchain pinning, task running, or anything beyond `[env]`.

## File changes

| Action | File | Notes |
|--------|------|-------|
| Create | `.mise.toml` | Committed. Comment-only, points contributors to `.mise.local.toml`. |
| Create | `.mise.local.toml` | Gitignored. Active env vars from `.envrc.local` in mise `[env]` TOML format. |
| Delete | `.envrc` | The one-liner that sourced `.envrc.local`. |
| Delete | `.envrc.local` | Replaced by `.mise.local.toml`. |
| Edit | `.gitignore` | Remove `.envrc.local`, add `.mise.local.toml`. |

## `.mise.toml` (committed)

```toml
# cli-router — mise config
# Put personal dev overrides in .mise.local.toml (gitignored).
# mise auto-loads .mise.local.toml if it exists.
```

Empty aside from comments. Mirrors the intent of `.envrc` (just a redirect to the local file).

## `.mise.local.toml` (gitignored)

Contains all currently-active (uncommented) env vars from `.envrc.local`, converted to mise `[env]` TOML format:

```toml
[env]
ANTHROPIC_BASE_URL = "http://127.0.0.1:8787"
ANTHROPIC_AUTH_TOKEN = "sk_live_1234"
ANTHROPIC_MODEL = "zai/glm-5-turbo"
ANTHROPIC_DEFAULT_HAIKU_MODEL = "zai/glm-4.5-air"
ANTHROPIC_DEFAULT_SONNET_MODEL = "zai/glm-5-turbo"
ANTHROPIC_DEFAULT_OPUS_MODEL = "zai/glm-5.1"
CLAUDE_CODE_SUBAGENT_MODEL = "zai/glm-4.7"
OPENCODE_LARGE_MODEL = "cli-router/minimax/minimax-m3"
OPENCODE_MEDIUM_MODEL = "cli-router/minimax/minimax-m3"
OPENCODE_SMALL_MODEL = "cli-router/minimax/minimax-m3"
```

Commented-out alternatives from `.envrc.local` are dropped. Re-add as needed.

## Unchanged files

- **`.env`** — secrets (ZAI_API_KEY, DEEPSEEK_API_KEY) and CLI_ROUTER_PROFILE. Still sourced by Makefile dev recipes via `LOAD_DOTENV`. Not loaded by mise.
- **`rust-toolchain.toml`** — mise reads this natively, no duplication.
- **`Makefile`** / `makefiles/*`** — no changes.
- **`prek.toml`** — no changes.

## Activation

mise auto-activates when `cd`-ing into a directory with `.mise.toml` (requires `eval "$(mise activate bash)"` or equivalent in shell config). Already configured on this machine.
