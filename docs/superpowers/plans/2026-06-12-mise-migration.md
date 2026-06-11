# Migrate from direnv to mise — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace direnv with mise for project environment variable management.

**Architecture:** One-to-one file migration — `.envrc` → `.mise.toml`, `.envrc.local` → `.mise.local.toml`. No code changes, no test changes.

**Tech Stack:** mise (shell env management), TOML config format

---

### Task 1: Create `.mise.toml` (committed)

**Files:**
- Create: `.mise.toml`

- [ ] **Step 1: Create `.mise.toml`**

```toml
# cli-router — mise config
# Put personal dev overrides in .mise.local.toml (gitignored).
# mise auto-loads .mise.local.toml if it exists.
```

- [ ] **Step 2: Verify mise picks it up**

Run: `mise ls`
Expected: No errors. Shows mise recognizes the config.

---

### Task 2: Create `.mise.local.toml` (gitignored)

**Files:**
- Create: `.mise.local.toml`

- [ ] **Step 1: Create `.mise.local.toml`**

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

- [ ] **Step 2: Verify env vars are loaded**

Run: `mise env`
Expected: All 10 variables listed with their values.

---

### Task 3: Update `.gitignore`**

**Files:**
- Modify: `.gitignore`

- [ ] **Step 1: Replace `.envrc.local` with `.mise.local.toml`**

In `.gitignore`, change:
```
.envrc.local
```
to:
```
.mise.local.toml
```

- [ ] **Step 2: Verify gitignore works**

Run: `git status --short`
Expected: `.mise.local.toml` is NOT listed (gitignored). `.mise.toml` IS listed (new untracked file).

---

### Task 4: Delete old direnv files

**Files:**
- Delete: `.envrc`
- Delete: `.envrc.local`

- [ ] **Step 1: Delete the files**

Run: `rm .envrc .envrc.local`

- [ ] **Step 2: Verify they're gone and git sees the deletion**

Run: `git status --short`
Expected: `.envrc` shows as deleted (was tracked). `.envrc.local` does NOT appear (was already gitignored).

---

### Task 5: Commit and verify

**Files:** None (git operations only)

- [ ] **Step 1: Stage all changes**

Run: `git add .mise.toml .gitignore && git rm .envrc`

Note: `.mise.local.toml` is gitignored so it won't be staged. `.envrc.local` was gitignored so `git rm` doesn't apply to it.

- [ ] **Step 2: Review staged diff**

Run: `git diff --cached`
Expected:
- `.mise.toml` added (3 comment lines)
- `.gitignore` modified (`.envrc.local` → `.mise.local.toml`)
- `.envrc` deleted

- [ ] **Step 3: Commit**

```bash
git commit -m "chore: migrate from direnv to mise for env management"
```

- [ ] **Step 4: End-to-end verification**

Open a new shell (or re-enter the directory) and run:
```bash
echo $ANTHROPIC_BASE_URL
```
Expected: `http://127.0.0.1:8787`

Run: `direnv export bash 2>&1 || true`
Expected: Error or no output (direnv no longer has config).
