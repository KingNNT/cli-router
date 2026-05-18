# CodexAuto Auth — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `AuthConfig::CodexAuto` — an auth type that auto-reads tokens from `~/.codex/auth.json` with no manual token management.

**Architecture:** New `CodexAuto` variant in `AuthConfig` and `AuthPayload`. At startup, `build_leaf()` reads `~/.codex/auth.json` and resolves to an `AuthHeader::OAuth`. The background `refresh_expiring` task re-reads the file on each sweep and refreshes in-memory if needed. Never writes to `~/.codex/auth.json` or `config.toml`.

**Tech Stack:** Rust, serde, reqwest, existing `oauth::openai` module

**Spec:** `docs/superpowers/specs/2026-05-18-codex-auto-auth-design.md`

---

## File Structure

| Action | File | Responsibility |
|--------|------|----------------|
| Modify | `crates/proxy/src/config.rs` | +`AuthConfig::CodexAuto` variant, serde, Debug, interpolate_env arm |
| Modify | `crates/proxy-admin-api/src/lib.rs` | +`AuthPayload::CodexAuto` variant |
| Modify | `crates/proxy/src/adapters/providers/builder.rs` | +`CodexAuto` arm in `build_leaf()` + `resolve_auth_token()` |
| Modify | `crates/proxy/src/adapters/providers/token_refresh.rs` | +`CodexAuto` handling in `refresh_expiring` |
| Modify | `crates/proxy/src/application/use_cases/admin.rs` | +mapping arms for `CodexAuto` |

---

### Task 1: Add `AuthConfig::CodexAuto` to config

**Files:**
- Modify: `crates/proxy/src/config.rs`
- Test: inline `#[cfg(test)] mod tests`

- [ ] **Step 1: Write the failing tests**

Add tests at the bottom of the `tests` module in `config.rs`:

```rust
    #[test]
    fn toml_parses_codex_auto_auth() {
        let toml_str = r#"
            [[providers]]
            name = "openai"
            kind = "openai"
            auth = { type = "codex_auto" }

            [[routing]]
            match = { model = "*" }
            provider = "openai"
        "#;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        assert!(matches!(cfg.providers[0].auth, AuthConfig::CodexAuto));
    }

    #[test]
    fn codex_auto_auth_debug_does_not_leak() {
        let auth = AuthConfig::CodexAuto;
        let s = format!("{auth:?}");
        assert_eq!(s, "CodexAuto");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p proxy -- toml_parses_codex_auto_auth codex_auto_auth_debug_does_not_leak`
Expected: compile errors — `AuthConfig::CodexAuto` doesn't exist yet.

- [ ] **Step 3: Add `CodexAuto` variant to `AuthConfig`**

In `config.rs`, add after `OpenAiOAuth` in the `AuthConfig` enum (line ~95):

```rust
    /// Auto-read tokens from `~/.codex/auth.json` (Codex CLI cache).
    /// The proxy reads the file at startup and during background refresh.
    /// No tokens are stored in config.toml.
    #[serde(rename = "codex_auto")]
    CodexAuto,
```

- [ ] **Step 4: Add `CodexAuto` arm to `Debug` impl**

In the `Debug for AuthConfig` impl (line ~101), add after the `OpenAiOAuth` arm:

```rust
            AuthConfig::CodexAuto => f.debug_tuple("CodexAuto").finish(),
```

- [ ] **Step 5: Add `CodexAuto` arm to `interpolate_env`**

In the `interpolate_env` method (line ~328), add after the `OpenAiOAuth` arm:

```rust
                AuthConfig::CodexAuto => {
                    // Tokens come from ~/.codex/auth.json, not env vars.
                }
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p proxy -- toml_parses_codex_auto_auth codex_auto_auth_debug_does_not_leak`
Expected: 2 PASS

- [ ] **Step 7: Run full workspace tests**

Run: `cargo test --workspace`
Expected: compile errors in `builder.rs`, `admin.rs`, `token_refresh.rs` — match arms are now non-exhaustive. That's expected, we fix them in subsequent tasks.

- [ ] **Step 8: Commit**

```
feat(config): add AuthConfig::CodexAuto variant
```

---

### Task 2: Add `AuthPayload::CodexAuto` to proxy-admin-api

**Files:**
- Modify: `crates/proxy-admin-api/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Add test in the test module in `lib.rs`:

```rust
    #[test]
    fn codex_auto_payload_round_trips() {
        let auth = AuthPayload::CodexAuto;
        let json = serde_json::to_string(&auth).unwrap();
        assert!(json.contains("\"type\":\"codex_auto\""));
        let back: AuthPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(auth, back);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p proxy-admin-api -- codex_auto_payload_round_trips`
Expected: compile error — `AuthPayload::CodexAuto` doesn't exist.

- [ ] **Step 3: Add the variant**

In the `AuthPayload` enum, add after `OpenAiOAuth`:

```rust
    #[serde(rename = "codex_auto")]
    CodexAuto,
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p proxy-admin-api -- codex_auto_payload_round_trips`
Expected: PASS

- [ ] **Step 5: Commit**

```
feat(admin-api): add AuthPayload::CodexAuto DTO
```

---

### Task 3: Add `CodexAuto` to builder + resolve at startup

**Files:**
- Modify: `crates/proxy/src/adapters/providers/builder.rs`

- [ ] **Step 1: Add `CodexAuto` arm to `build_leaf`**

In `build_leaf()` (line ~22), add after the `OpenAiOAuth` arm:

```rust
        AuthConfig::CodexAuto => {
            let tokens = crate::adapters::oauth::openai::read_auth_json()
                .map_err(|e| BuildError::AuthResolve(format!(
                    "provider '{}': CodexAuto requires ~/.codex/auth.json. Run 'codex login' first. ({e})",
                    p.name
                )))?;
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let expires_at_ms = now_ms + tokens.expires_in.unwrap_or(3600) * 1000;
            AuthHeader::OAuth {
                access_token: tokens.access_token,
                refresh_token: tokens.refresh_token.unwrap_or_default(),
                expires_at_ms,
            }
        }
```

- [ ] **Step 2: Add `AuthResolve` variant to `BuildError`**

In the `BuildError` enum, add:

```rust
    #[error("{0}")]
    AuthResolve(String),
```

- [ ] **Step 3: Add `CodexAuto` arm to `resolve_auth_token`**

In `resolve_auth_token()` (line ~183), add before `Passthrough`:

```rust
        AuthConfig::CodexAuto => String::new(), // resolved at build time, no static token
```

- [ ] **Step 4: Run builder tests**

Run: `cargo test -p proxy -- builders`
Expected: all existing tests PASS (no tests reference `CodexAuto` yet)

- [ ] **Step 5: Commit**

```
feat(proxy): resolve CodexAuto auth from ~/.codex/auth.json at startup
```

---

### Task 4: Add `CodexAuto` to background token refresh

**Files:**
- Modify: `crates/proxy/src/adapters/providers/token_refresh.rs`

- [ ] **Step 1: Add `CodexAuto` to `OAuthKind` enum**

Change:

```rust
enum OAuthKind {
    Anthropic,
    OpenAi,
}
```

To:

```rust
enum OAuthKind {
    Anthropic,
    OpenAi,
    CodexAuto,
}
```

- [ ] **Step 2: Collect `CodexAuto` providers in `refresh_expiring`**

In the `filter_map` that collects providers needing refresh (line ~53), add after the `OpenAiOAuth` arm:

```rust
                AuthConfig::CodexAuto => {
                    // Always re-read the file. If it's fresh, we skip refresh below.
                    Some((p.name.clone(), String::new(), OAuthKind::CodexAuto))
                }
```

- [ ] **Step 3: Handle `CodexAuto` in the refresh loop**

Replace the `for (name, old_rt, kind) in to_refresh` loop body. The key change: for `CodexAuto`, first re-read the file. If the file's tokens are fresh, just update in-memory and skip OAuth refresh. If the file's tokens are also expired, do an OAuth refresh using the file's refresh_token.

Find the block that starts with:

```rust
    for (name, old_rt, kind) in to_refresh {
```

Add a `CodexAuto` arm in the match that resolves tokens:

```rust
        let (access_token, refresh_token, expires_in) = match kind {
            OAuthKind::Anthropic => {
                let t = crate::adapters::oauth::anthropic::refresh_token(http, &old_rt)
                    .await
                    .map_err(|e| format!("refresh {name}: {e}"))?;
                (t.access_token, t.refresh_token, t.expires_in)
            }
            OAuthKind::OpenAi => {
                let t = crate::adapters::oauth::openai::refresh_token(http, &old_rt)
                    .await
                    .map_err(|e| format!("refresh {name}: {e}"))?;
                (t.access_token, t.refresh_token, t.expires_in)
            }
            OAuthKind::CodexAuto => {
                // Re-read ~/.codex/auth.json first — Codex CLI may have refreshed it.
                match crate::adapters::oauth::openai::read_auth_json() {
                    Ok(file_tokens) => {
                        let file_expires_in = file_tokens.expires_in.unwrap_or(0);
                        if file_expires_in > 300 {
                            // File tokens are fresh — use them, skip OAuth refresh.
                            tracing::info!(provider = %name, "CodexAuto: re-read fresh tokens from ~/.codex/auth.json");
                            (
                                file_tokens.access_token,
                                file_tokens.refresh_token,
                                file_tokens.expires_in,
                            )
                        } else {
                            // File tokens also expired — refresh using the file's refresh_token.
                            let rt = file_tokens.refresh_token.as_deref().unwrap_or("");
                            if rt.is_empty() {
                                tracing::warn!(provider = %name, "CodexAuto: ~/.codex/auth.json tokens expired and no refresh_token available");
                                continue;
                            }
                            tracing::info!(provider = %name, "CodexAuto: ~/.codex/auth.json tokens expired, refreshing");
                            let t = crate::adapters::oauth::openai::refresh_token(http, rt)
                                .await
                                .map_err(|e| format!("CodexAuto refresh {name}: {e}"))?;
                            (t.access_token, t.refresh_token, t.expires_in)
                        }
                    }
                    Err(e) => {
                        tracing::warn!(provider = %name, error = %e, "CodexAuto: failed to read ~/.codex/auth.json during refresh");
                        continue;
                    }
                }
            }
        };
```

- [ ] **Step 4: Update in-memory config write-back for `CodexAuto`**

In the section that writes back to in-memory config (the `if let Some(prov) = cfg.providers.iter_mut().find(...)` block), add a `CodexAuto` arm. For `CodexAuto`, update the in-memory auth to the resolved `OpenAiOAuth` tokens (but do NOT persist to config.toml — skip the `toml::to_string_pretty` and `std::fs::write` for `CodexAuto`).

Change the block after `// Update in-memory config.` to be wrapped:

```rust
        // Update in-memory config.
        let new_cfg = {
            let mut cfg = config.write().map_err(|e| format!("config write: {e}"))?;
            if let Some(prov) = cfg.providers.iter_mut().find(|p| p.name == name) {
                prov.auth = match kind {
                    OAuthKind::Anthropic => AuthConfig::AnthropicOAuth {
                        access_token,
                        refresh_token: refresh_token.clone().unwrap_or(old_rt),
                        expires_at_ms,
                    },
                    OAuthKind::OpenAi => AuthConfig::OpenAiOAuth {
                        access_token,
                        refresh_token: refresh_token.clone().unwrap_or(old_rt),
                        expires_at_ms,
                    },
                    OAuthKind::CodexAuto => AuthConfig::OpenAiOAuth {
                        access_token,
                        refresh_token: refresh_token.clone().unwrap_or_default(),
                        expires_at_ms,
                    },
                };
            }
            cfg.clone()
        }; // Write lock dropped here.
```

Then wrap the persist-to-disk section to skip for `CodexAuto`:

```rust
        // Persist to disk and rebuild provider tree — skip for CodexAuto
        // (CodexAuto never writes tokens to config.toml).
        if !matches!(kind, OAuthKind::CodexAuto) {
            if let Some(parent) = config_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let toml_str = toml::to_string_pretty(&new_cfg).map_err(|e| format!("serialize: {e}"))?;
            std::fs::write(config_path, toml_str).map_err(|e| format!("write: {e}"))?;
        }

        live.reload(&new_cfg, http.clone())
            .map_err(|e| format!("reload: {e}"))?;

        tracing::info!(provider = %name, "background refresh: OAuth token refreshed");
```

Note: The `old_rt` variable is used in `refresh_token.clone().unwrap_or(old_rt)`. For `CodexAuto`, `old_rt` is `String::new()` (we passed `String::new()` in the collection step). This is fine because `CodexAuto` always reads the refresh_token from the file or from the OAuth response.

- [ ] **Step 5: Run token_refresh tests**

Run: `cargo test -p proxy -- token_refresh`
Expected: all existing tests PASS

- [ ] **Step 6: Commit**

```
feat(proxy): handle CodexAuto in background token refresh
```

---

### Task 5: Add `CodexAuto` mapping arms to admin use cases

**Files:**
- Modify: `crates/proxy/src/application/use_cases/admin.rs`

- [ ] **Step 1: Add `CodexAuto` arm to `auth_to_payload`**

In `auth_to_payload()` (line ~854), add after the `OpenAiOAuth` arm:

```rust
        AuthConfig::CodexAuto => AuthPayload::CodexAuto,
```

- [ ] **Step 2: Add `CodexAuto` arm to `payload_to_auth`**

In `payload_to_auth()` (line ~884), add after the `OpenAiOAuth` arm (before the closing `}`):

```rust
        AuthPayload::CodexAuto => AuthConfig::CodexAuto,
```

- [ ] **Step 3: Run workspace tests**

Run: `cargo test --workspace`
Expected: all PASS

- [ ] **Step 4: Commit**

```
feat(admin): add CodexAuto mapping arms
```

---

### Task 6: Run full verification

- [ ] **Step 1: Run all tests**

Run: `cargo test --workspace`
Expected: all PASS

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --workspace -- -D warnings`
Expected: no warnings

- [ ] **Step 3: Build release**

Run: `cargo build --release --workspace`
Expected: clean build

- [ ] **Step 4: Final commit (if any fixups needed)**

```
chore: fix clippy warnings from CodexAuto integration
```
