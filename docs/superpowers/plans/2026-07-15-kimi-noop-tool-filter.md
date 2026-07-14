# Kimi no-op tool-call filter — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an opt-in, per-provider filter that drops no-op `bash` tool calls from Kimi's Anthropic responses and forces `stop_reason=end_turn` when nothing actionable remains, so opencode stops instead of looping on `$ :`.

**Architecture:** A new provider-agnostic `tool_sanitizer` module transforms Anthropic responses (buffered + streaming SSE). `KimiProvider` gains a `sanitize_empty_tools` flag (default off) that, when set, wraps the upstream response through the sanitizer on the Anthropic `forward` path only. The flag is a per-provider config field persisted in SQLite and editable via the admin API/TUI (hot reload).

**Tech Stack:** Rust (edition 2024), axum, reqwest, serde_json, bytes, futures (stream combinators), rusqlite, wiremock (dev).

## Global Constraints

- Ring dependency rule: `frameworks → adapters → application → domain`. The sanitizer is an adapter; it must not import framework types.
- Per-ring error types; no `anyhow`. Fail **open** on any parse error (return original bytes).
- All crates target edition 2024; prefer let-chains where clippy's `collapsible_if` applies.
- `cargo clippy --workspace -- -D warnings` and `cargo fmt` must pass.
- Config is stored in SQLite as the single source of truth; new config fields need a migration + `DbConfigRepository` read/write + admin-API DTO round-trip.
- New field default: `sanitize_empty_tools = false` (feature OFF unless explicitly enabled).
- No-op command definition (verbatim): a `tool_use` block whose `input.command` is a string and `command.trim()` ∈ `{"", ":", "true"}`.

---

### Task 1: Config field + admin-API DTO + mapping

**Files:**
- Modify: `crates/proxy/src/config.rs:27-46` (`ProviderConfig` struct)
- Modify: `crates/proxy-admin-api/src/lib.rs:114-129` (`ProviderPayload`)
- Modify: `crates/proxy/src/application/use_cases/admin.rs:697-709` (`config_to_payload`) and `:793-795` (`payload_to_config`)
- Modify (mechanical): every `ProviderConfig { … }` literal — the compiler lists them (~22 sites across `config.rs`, `admin.rs`, `builder.rs`, `db_config.rs`, `account_usage/anthropic.rs`, `tests/integration.rs`, `tests/quota_enforcement.rs`)
- Test: `crates/proxy/src/config.rs` tests mod; `crates/proxy/src/application/use_cases/admin.rs` tests mod

**Interfaces:**
- Produces: `ProviderConfig.sanitize_empty_tools: bool` (default `false` via `#[serde(default)]`); `ProviderPayload.sanitize_empty_tools: Option<bool>`.

- [ ] **Step 1: Write the failing test** (config default + serde)

Add to the `#[cfg(test)] mod tests` in `crates/proxy/src/config.rs`:

```rust
#[test]
fn provider_config_defaults_sanitize_empty_tools_to_false() {
    let json = r#"{"name":"moonshot","kind":"kimi"}"#;
    let provider: ProviderConfig = serde_json::from_str(json).unwrap();
    assert!(!provider.sanitize_empty_tools);
}

#[test]
fn provider_config_deserializes_sanitize_empty_tools() {
    let json = r#"{"name":"moonshot","kind":"kimi","sanitize_empty_tools":true}"#;
    let provider: ProviderConfig = serde_json::from_str(json).unwrap();
    assert!(provider.sanitize_empty_tools);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p proxy --lib config::tests::provider_config_defaults_sanitize_empty_tools_to_false`
Expected: FAIL — compile error, no field `sanitize_empty_tools`.

- [ ] **Step 3: Add the field** to `ProviderConfig` (after `max_concurrent`, `crates/proxy/src/config.rs:45`):

```rust
    /// When true, drop no-op `bash` tool calls (empty / `:` / `true` command)
    /// from this provider's Anthropic responses and force `stop_reason=end_turn`
    /// when no real tool call remains. Off by default. Currently honored only by
    /// the Kimi provider on the Anthropic path.
    #[serde(default)]
    pub sanitize_empty_tools: bool,
```

- [ ] **Step 4: Fix all struct-literal sites**

Run: `cargo build -p proxy 2>&1 | rg "missing field"` to list every `ProviderConfig { … }` literal. Add `sanitize_empty_tools: false,` to each (in `config.rs`, `admin.rs`, `builder.rs`, `db_config.rs`, `account_usage/anthropic.rs`). Then in `crates/proxy/tests/integration.rs` and `crates/proxy/tests/quota_enforcement.rs` do the same. Repeat `cargo build` until clean.

- [ ] **Step 5: Add the DTO field** to `ProviderPayload` in `crates/proxy-admin-api/src/lib.rs` (after `max_concurrent`, `:128`):

```rust
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sanitize_empty_tools: Option<bool>,
```

Fix any `ProviderPayload { … }` literals the compiler flags (add `sanitize_empty_tools: None,`).

- [ ] **Step 6: Wire the mapping** in `crates/proxy/src/application/use_cases/admin.rs`.

In `config_to_payload` (the `ProviderPayload { … }` at `:697`), add:

```rust
                sanitize_empty_tools: Some(p.sanitize_empty_tools),
```

In `payload_to_config` (the `ProviderConfig { … }` at `:793`), add:

```rust
                sanitize_empty_tools: pp.sanitize_empty_tools.unwrap_or(false),
```

- [ ] **Step 7: Write the admin round-trip test**

Add to `#[cfg(test)] mod tests` in `admin.rs`:

```rust
#[test]
fn config_payload_preserves_sanitize_empty_tools() {
    let cfg = Config {
        providers: vec![ProviderConfig {
            name: "moonshot".into(),
            kind: crate::config::ProviderKind::Kimi,
            auth: AuthConfig::Passthrough,
            base_url: None,
            openai_base_url: None,
            reasoning_effort: None,
            thinking_mode: crate::config::ThinkingMode::SplitOnly,
            max_concurrent: None,
            sanitize_empty_tools: true,
        }],
        ..Default::default()
    };
    let payload = config_to_payload(&cfg);
    assert_eq!(payload.providers[0].sanitize_empty_tools, Some(true));
    let restored = payload_to_config(&payload).unwrap();
    assert!(restored.providers[0].sanitize_empty_tools);
}
```

(If `Config` has no `Default`, copy the full literal from the neighboring `config_payload_preserves_thinking_mode` test instead of `..Default::default()`.)

- [ ] **Step 8: Run the tests**

Run: `cargo test -p proxy --lib config::tests -- sanitize && cargo test -p proxy --lib admin -- sanitize`
Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add crates/proxy/src/config.rs crates/proxy-admin-api/src/lib.rs crates/proxy/src/application/use_cases/admin.rs crates/proxy/src/adapters crates/proxy/tests
git commit -m "feat(proxy): add per-provider sanitize_empty_tools config flag"
```

---

### Task 2: Storage migration + DbConfigRepository read/write

**Files:**
- Modify: `crates/proxy/src/adapters/storage/schema.rs:5-11` (register `MIGRATION_V7`), `:113` (add const)
- Modify: `crates/proxy/src/adapters/storage/db_config.rs:107-135` (INSERT), `:199-237` (SELECT + row map)
- Test: `crates/proxy/src/adapters/storage/schema.rs` tests mod; `crates/proxy/src/adapters/storage/db_config.rs` tests mod

**Interfaces:**
- Consumes: `ProviderConfig.sanitize_empty_tools` (Task 1).
- Produces: `providers.sanitize_empty_tools` column persisted and restored.

- [ ] **Step 1: Write the failing migration test**

Add to `#[cfg(test)] mod tests` in `schema.rs` (mirror `v5_adds_provider_thinking_mode_column`):

```rust
#[test]
fn v7_adds_provider_sanitize_empty_tools_column() {
    let conn = open_in_memory();
    ensure_current(&conn).unwrap();
    let cols: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('providers')")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(cols.contains(&"sanitize_empty_tools".into()));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p proxy --lib schema::tests::v7_adds_provider_sanitize_empty_tools_column`
Expected: FAIL — column absent.

- [ ] **Step 3: Add the migration**

In `schema.rs`, after `MIGRATION_V6` (`:113`):

```rust
const MIGRATION_V7: &str = r#"
ALTER TABLE providers ADD COLUMN sanitize_empty_tools INTEGER NOT NULL DEFAULT 0;
"#;
```

Register it in the `MIGRATIONS` array (`:5-11`):

```rust
    (7, MIGRATION_V7),
```

- [ ] **Step 4: Run migration test to verify it passes**

Run: `cargo test -p proxy --lib schema::tests::v7_adds_provider_sanitize_empty_tools_column`
Expected: PASS.

- [ ] **Step 5: Write the failing round-trip test**

Add to `#[cfg(test)] mod tests` in `db_config.rs` (find an existing save/load test to copy the harness; it opens a temp DB, saves a `Config`, reloads it):

```rust
#[test]
fn sanitize_empty_tools_survives_save_and_load() {
    let repo = test_repo(); // reuse this crate's existing test-repo helper
    let mut cfg = Config::default();
    cfg.providers.push(ProviderConfig {
        name: "moonshot".into(),
        kind: ProviderKind::Kimi,
        auth: AuthConfig::Passthrough,
        base_url: None,
        openai_base_url: None,
        reasoning_effort: None,
        thinking_mode: ThinkingMode::SplitOnly,
        max_concurrent: None,
        sanitize_empty_tools: true,
    });
    repo.save(&cfg).unwrap();
    let loaded = repo.load().unwrap();
    assert!(loaded.providers.iter().any(|p| p.name == "moonshot" && p.sanitize_empty_tools));
}
```

(Match the exact helper/method names already used by the other `db_config.rs` tests — `test_repo`, `save`, `load` are placeholders for whatever this file already uses.)

- [ ] **Step 6: Run test to verify it fails**

Run: `cargo test -p proxy --lib db_config::tests::sanitize_empty_tools_survives_save_and_load`
Expected: FAIL — value not persisted (defaults to false on load / SQL column-count mismatch).

- [ ] **Step 7: Update the INSERT** in `db_config.rs` (`:107-135`)

Add `sanitize_empty_tools` to the column list and a new `?14` placeholder:

```rust
                    "INSERT INTO providers (name, kind, base_url, openai_base_url, reasoning_effort, thinking_mode, auth_type,
                     auth_api_key, auth_bearer, auth_access_token, auth_refresh_token, auth_expires_at_ms, max_concurrent, sanitize_empty_tools)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
```

Append to the `params!` list (after `p.max_concurrent.map(...)`):

```rust
                    p.sanitize_empty_tools as i64,
```

- [ ] **Step 8: Update the SELECT + row map** in `db_config.rs` (`:199-237`)

Add the column to the query (`:202-205`):

```rust
            "SELECT name, kind, base_url, openai_base_url, reasoning_effort, thinking_mode, auth_type,
                    auth_api_key, auth_bearer, auth_access_token, auth_refresh_token, auth_expires_at_ms,
                    max_concurrent, sanitize_empty_tools
             FROM providers ORDER BY id",
```

Add to the `ProviderConfig { … }` mapping (after `max_concurrent`, `:233`):

```rust
                sanitize_empty_tools: row.get::<_, i64>(13)? != 0,
```

- [ ] **Step 9: Run tests to verify they pass**

Run: `cargo test -p proxy --lib storage`
Expected: PASS.

- [ ] **Step 10: Commit**

```bash
git add crates/proxy/src/adapters/storage
git commit -m "feat(proxy): persist sanitize_empty_tools in providers table"
```

---

### Task 3: `tool_sanitizer` — no-op detection + buffered path

**Files:**
- Create: `crates/proxy/src/adapters/providers/tool_sanitizer.rs`
- Modify: `crates/proxy/src/adapters/providers/mod.rs:11` (add `mod tool_sanitizer;`)
- Test: same file, `#[cfg(test)] mod tests`

**Interfaces:**
- Produces:
  - `pub fn is_noop_tool_use(block: &serde_json::Value) -> bool`
  - `pub fn sanitize_buffered(body: &[u8]) -> bytes::Bytes`

- [ ] **Step 1: Declare the module**

In `crates/proxy/src/adapters/providers/mod.rs`, after line `mod messages_protocol;` (`:11`):

```rust
mod tool_sanitizer;
```

- [ ] **Step 2: Write the failing tests**

Create `crates/proxy/src/adapters/providers/tool_sanitizer.rs` with only the test module + empty stubs:

```rust
//! Provider-agnostic sanitizer for Anthropic responses: drops no-op `bash`
//! tool calls (empty / `:` / `true` command) and forces `stop_reason=end_turn`
//! when no real tool_use remains. Fails open — any parse error returns input
//! unchanged. Currently used only by the Kimi provider on the Anthropic path.

use bytes::Bytes;
use serde_json::{Value, json};

pub fn is_noop_tool_use(_block: &Value) -> bool {
    unimplemented!()
}

pub fn sanitize_buffered(_body: &[u8]) -> Bytes {
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_use(cmd: Value) -> Value {
        json!({"type": "tool_use", "id": "t1", "name": "bash", "input": {"command": cmd}})
    }

    #[test]
    fn noop_detects_empty_whitespace_and_shell_noops() {
        assert!(is_noop_tool_use(&tool_use(json!(""))));
        assert!(is_noop_tool_use(&tool_use(json!("   "))));
        assert!(is_noop_tool_use(&tool_use(json!(":"))));
        assert!(is_noop_tool_use(&tool_use(json!("true"))));
        assert!(is_noop_tool_use(&tool_use(json!(" : "))));
    }

    #[test]
    fn noop_rejects_real_commands_and_non_tool_blocks() {
        assert!(!is_noop_tool_use(&tool_use(json!("ls -la"))));
        assert!(!is_noop_tool_use(&json!({"type": "text", "text": "hi"})));
        assert!(!is_noop_tool_use(&json!({"type": "tool_use", "id": "t", "name": "bash", "input": {}})));
        assert!(!is_noop_tool_use(&json!({"type": "tool_use", "name": "x", "input": {"command": 5}})));
    }

    #[test]
    fn buffered_drops_only_noop_tool_keeps_stop_reason() {
        let body = json!({
            "type": "message", "role": "assistant", "stop_reason": "tool_use",
            "content": [
                {"type": "tool_use", "id": "a", "name": "bash", "input": {"command": "ls"}},
                {"type": "tool_use", "id": "b", "name": "bash", "input": {"command": ":"}}
            ]
        });
        let out: Value = serde_json::from_slice(&sanitize_buffered(body.to_string().as_bytes())).unwrap();
        assert_eq!(out["content"].as_array().unwrap().len(), 1);
        assert_eq!(out["content"][0]["id"], "a");
        assert_eq!(out["stop_reason"], "tool_use"); // a real tool remains
    }

    #[test]
    fn buffered_drops_sole_noop_tool_and_forces_end_turn() {
        let body = json!({
            "type": "message", "role": "assistant", "stop_reason": "tool_use",
            "content": [
                {"type": "text", "text": "All done."},
                {"type": "tool_use", "id": "b", "name": "bash", "input": {"command": ""}}
            ]
        });
        let out: Value = serde_json::from_slice(&sanitize_buffered(body.to_string().as_bytes())).unwrap();
        let content = out["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(out["stop_reason"], "end_turn");
    }

    #[test]
    fn buffered_leaves_valid_tool_untouched() {
        let body = json!({
            "type": "message", "role": "assistant", "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": "a", "name": "bash", "input": {"command": "pwd"}}]
        });
        let out: Value = serde_json::from_slice(&sanitize_buffered(body.to_string().as_bytes())).unwrap();
        assert_eq!(out["content"].as_array().unwrap().len(), 1);
        assert_eq!(out["stop_reason"], "tool_use");
    }

    #[test]
    fn buffered_returns_malformed_input_unchanged() {
        let raw = b"not json at all";
        assert_eq!(sanitize_buffered(raw).as_ref(), raw);
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p proxy --lib tool_sanitizer::tests`
Expected: FAIL — `unimplemented!()` panics.

- [ ] **Step 4: Implement `is_noop_tool_use` and `sanitize_buffered`**

Replace the two stub functions:

```rust
/// True iff `block` is a `tool_use` whose `input.command` is a string that,
/// trimmed, is empty or a shell no-op (`:` / `true`).
pub fn is_noop_tool_use(block: &Value) -> bool {
    if block.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
        return false;
    }
    match block.pointer("/input/command").and_then(|c| c.as_str()) {
        Some(cmd) => matches!(cmd.trim(), "" | ":" | "true"),
        None => false,
    }
}

/// Drop no-op tool_use blocks from an Anthropic message body. If no `tool_use`
/// block survives, force `stop_reason=end_turn`. Fail open: any parse failure
/// or unexpected shape returns the original bytes unchanged.
pub fn sanitize_buffered(body: &[u8]) -> Bytes {
    let Ok(mut value) = serde_json::from_slice::<Value>(body) else {
        return Bytes::copy_from_slice(body);
    };
    let Some(content) = value.get("content").and_then(|c| c.as_array()) else {
        return Bytes::copy_from_slice(body);
    };
    let kept: Vec<Value> = content
        .iter()
        .filter(|b| !is_noop_tool_use(b))
        .cloned()
        .collect();
    let has_tool_use = kept
        .iter()
        .any(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_use"));
    value["content"] = Value::Array(kept);
    if !has_tool_use && value.get("stop_reason").and_then(|s| s.as_str()) == Some("tool_use") {
        value["stop_reason"] = json!("end_turn");
    }
    serde_json::to_vec(&value)
        .map(Bytes::from)
        .unwrap_or_else(|_| Bytes::copy_from_slice(body))
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p proxy --lib tool_sanitizer::tests`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/proxy/src/adapters/providers/tool_sanitizer.rs crates/proxy/src/adapters/providers/mod.rs
git commit -m "feat(proxy): add tool_sanitizer buffered no-op filter"
```

---

### Task 4: `tool_sanitizer` — streaming SSE state machine + wrapper

**Files:**
- Modify: `crates/proxy/src/adapters/providers/tool_sanitizer.rs`
- Test: same file, `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `is_noop_tool_use` (Task 3); `crate::application::ports::BoxedByteStream`.
- Produces:
  - `pub struct AnthropicNoopFilter` with `pub fn new() -> Self` and `pub fn feed(&mut self, event: &str, data: &Value) -> Vec<String>` (each returned string is a complete `event: …\ndata: …\n\n` frame).
  - `pub fn sanitize_stream(upstream: BoxedByteStream) -> BoxedByteStream`.

Design notes for the state machine:
- Buffer a `tool_use` content block (its `content_block_start`, `input_json_delta` frames, and accumulated `partial_json`) until its `content_block_stop`. At stop, parse the accumulated args into `{"command": …}`, build the full block, and check `is_noop_tool_use`. No-op → emit nothing (swallowed). Otherwise → flush the buffered frames verbatim and mark `emitted_real_tool = true`.
- `message_delta` is buffered (it carries `stop_reason` + `usage`) and emitted at `message_stop`. If its `stop_reason == "tool_use"` and no real tool was flushed, rewrite only `stop_reason` to `"end_turn"`, preserving all other fields.
- `message_start`, text `content_block_*`, and any unknown event pass through immediately.
- Fail open: if accumulated args don't parse, flush the buffered tool block unchanged.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` mod in `tool_sanitizer.rs`:

```rust
fn frame(event: &str, data: &Value) -> (String, Value) {
    (event.to_string(), data.clone())
}

fn run(events: &[(String, Value)]) -> Vec<(String, Value)> {
    let mut f = AnthropicNoopFilter::new();
    let mut out = Vec::new();
    for (name, data) in events {
        for s in f.feed(name, data) {
            // parse "event: X\ndata: Y\n\n"
            let mut ev = String::new();
            let mut dt = String::new();
            for line in s.lines() {
                if let Some(n) = line.strip_prefix("event: ") { ev = n.into(); }
                else if let Some(d) = line.strip_prefix("data: ") { dt = d.into(); }
            }
            out.push((ev, serde_json::from_str(&dt).unwrap()));
        }
    }
    out
}

fn tool_stream(cmd_json: &str) -> Vec<(String, Value)> {
    vec![
        frame("message_start", &json!({"type":"message_start","message":{"id":"m","model":"kimi","content":[],"stop_reason":null,"usage":{"input_tokens":1,"output_tokens":0}}})),
        frame("content_block_start", &json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t1","name":"bash","input":{}}})),
        frame("content_block_delta", &json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":cmd_json}})),
        frame("content_block_stop", &json!({"type":"content_block_stop","index":0})),
        frame("message_delta", &json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":3}})),
        frame("message_stop", &json!({"type":"message_stop"})),
    ]
}

#[test]
fn stream_swallows_noop_tool_and_rewrites_stop_reason() {
    let out = run(&tool_stream("{\"command\":\":\"}"));
    let names: Vec<&str> = out.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, &["message_start", "message_delta", "message_stop"]);
    let md = out.iter().find(|(n, _)| n == "message_delta").unwrap();
    assert_eq!(md.1["delta"]["stop_reason"], "end_turn");
    assert_eq!(md.1["usage"]["output_tokens"], 3); // usage preserved
}

#[test]
fn stream_keeps_valid_tool_and_stop_reason() {
    let out = run(&tool_stream("{\"command\":\"ls\"}"));
    let names: Vec<&str> = out.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, &["message_start", "content_block_start", "content_block_delta", "content_block_stop", "message_delta", "message_stop"]);
    let md = out.iter().find(|(n, _)| n == "message_delta").unwrap();
    assert_eq!(md.1["delta"]["stop_reason"], "tool_use");
}

#[test]
fn stream_command_split_across_deltas_is_evaluated_whole() {
    let mut events = vec![
        frame("message_start", &json!({"type":"message_start","message":{"id":"m","model":"k","content":[]}})),
        frame("content_block_start", &json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t1","name":"bash","input":{}}})),
        frame("content_block_delta", &json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"comm"}})),
        frame("content_block_delta", &json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"and\":\"\"}"}})),
        frame("content_block_stop", &json!({"type":"content_block_stop","index":0})),
        frame("message_delta", &json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":2}})),
        frame("message_stop", &json!({"type":"message_stop"})),
    ];
    let out = run(&std::mem::take(&mut events));
    let names: Vec<&str> = out.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, &["message_start", "message_delta", "message_stop"]);
}

#[test]
fn stream_keeps_text_and_drops_noop_tool() {
    let events = vec![
        frame("message_start", &json!({"type":"message_start","message":{"id":"m","model":"k","content":[]}})),
        frame("content_block_start", &json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}})),
        frame("content_block_delta", &json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Done."}})),
        frame("content_block_stop", &json!({"type":"content_block_stop","index":0})),
        frame("content_block_start", &json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"t1","name":"bash","input":{}}})),
        frame("content_block_delta", &json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"command\":\"true\"}"}})),
        frame("content_block_stop", &json!({"type":"content_block_stop","index":1})),
        frame("message_delta", &json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":4}})),
        frame("message_stop", &json!({"type":"message_stop"})),
    ];
    let out = run(&events);
    let names: Vec<&str> = out.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, &["message_start", "content_block_start", "content_block_delta", "content_block_stop", "message_delta", "message_stop"]);
    // the surviving content_block_start is the text block
    let cbs = out.iter().find(|(n, _)| n == "content_block_start").unwrap();
    assert_eq!(cbs.1["content_block"]["type"], "text");
    assert_eq!(out.iter().find(|(n, _)| n == "message_delta").unwrap().1["delta"]["stop_reason"], "end_turn");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p proxy --lib tool_sanitizer::tests::stream`
Expected: FAIL — `AnthropicNoopFilter` not defined.

- [ ] **Step 3: Implement the state machine**

Add to `tool_sanitizer.rs` (above the tests mod):

```rust
use crate::application::ports::BoxedByteStream;
use futures::StreamExt;

fn sse(event: &str, data: &Value) -> String {
    format!("event: {event}\ndata: {}\n\n", data)
}

struct PendingTool {
    frames: Vec<String>, // raw start + delta frames, in order
    args: String,        // accumulated partial_json
}

pub struct AnthropicNoopFilter {
    pending_tool: Option<PendingTool>,
    emitted_real_tool: bool,
    pending_message_delta: Option<Value>,
    closed: bool,
}

impl Default for AnthropicNoopFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl AnthropicNoopFilter {
    pub fn new() -> Self {
        Self {
            pending_tool: None,
            emitted_real_tool: false,
            pending_message_delta: None,
            closed: false,
        }
    }

    /// Flush a buffered tool block unchanged (used on the valid path or when
    /// failing open). Returns its frames and marks a real tool as emitted.
    fn flush_pending_tool(&mut self) -> Vec<String> {
        match self.pending_tool.take() {
            Some(p) => {
                self.emitted_real_tool = true;
                p.frames
            }
            None => Vec::new(),
        }
    }

    pub fn feed(&mut self, event: &str, data: &Value) -> Vec<String> {
        if self.closed {
            return Vec::new();
        }
        match event {
            "content_block_start" => {
                let is_tool = data.pointer("/content_block/type").and_then(|t| t.as_str())
                    == Some("tool_use");
                if is_tool {
                    // Start buffering a fresh tool block. If one was already
                    // pending (missing stop — shouldn't happen), flush it first
                    // so a valid tool is never lost.
                    let flushed = self.flush_pending_tool();
                    self.pending_tool = Some(PendingTool {
                        frames: vec![sse(event, data)],
                        args: String::new(),
                    });
                    flushed
                } else {
                    vec![sse(event, data)]
                }
            }
            "content_block_delta" => {
                let is_json_delta = data.pointer("/delta/type").and_then(|t| t.as_str())
                    == Some("input_json_delta");
                if let Some(p) = self.pending_tool.as_mut()
                    && is_json_delta
                {
                    if let Some(frag) = data.pointer("/delta/partial_json").and_then(|v| v.as_str()) {
                        p.args.push_str(frag);
                    }
                    p.frames.push(sse(event, data));
                    Vec::new()
                } else {
                    vec![sse(event, data)]
                }
            }
            "content_block_stop" => {
                if let Some(mut p) = self.pending_tool.take() {
                    p.frames.push(sse(event, data));
                    // Decide: parse accumulated args; no-op → swallow, else flush.
                    let block = serde_json::from_str::<Value>(&p.args)
                        .map(|input| json!({"type": "tool_use", "input": input}))
                        .ok();
                    let noop = block.as_ref().map(is_noop_tool_use).unwrap_or(false);
                    if noop {
                        Vec::new() // swallow entire block
                    } else {
                        self.emitted_real_tool = true;
                        p.frames
                    }
                } else {
                    vec![sse(event, data)]
                }
            }
            "message_delta" => {
                // Buffer; emit at message_stop (may rewrite stop_reason).
                self.pending_message_delta = Some(data.clone());
                Vec::new()
            }
            "message_stop" => {
                let mut out = Vec::new();
                if let Some(mut md) = self.pending_message_delta.take() {
                    if !self.emitted_real_tool
                        && md.pointer("/delta/stop_reason").and_then(|s| s.as_str())
                            == Some("tool_use")
                    {
                        md["delta"]["stop_reason"] = json!("end_turn");
                    }
                    out.push(sse("message_delta", &md));
                }
                out.push(sse(event, data));
                self.closed = true;
                out
            }
            _ => vec![sse(event, data)],
        }
    }
}
```

- [ ] **Step 4: Add the stream wrapper**

Append to `tool_sanitizer.rs`:

```rust
/// Wrap an upstream Anthropic SSE byte stream, filtering no-op tool calls.
/// Splits on blank-line frame boundaries, feeds each complete `event:/data:`
/// frame through `AnthropicNoopFilter`, and re-emits the filtered frames.
pub fn sanitize_stream(upstream: BoxedByteStream) -> BoxedByteStream {
    let mut filter = AnthropicNoopFilter::new();
    let mut buf = String::new();

    let filtered = upstream.flat_map(move |chunk_result| {
        let mut emit: Vec<Result<Bytes, Box<dyn std::error::Error + Send + Sync>>> = Vec::new();
        match chunk_result {
            Err(e) => emit.push(Err(e)),
            Ok(chunk_bytes) => {
                buf.push_str(&String::from_utf8_lossy(&chunk_bytes));
                while let Some(idx) = buf.find("\n\n") {
                    let frame = buf[..idx].to_string();
                    buf.drain(..idx + 2);
                    if frame.trim().is_empty() {
                        continue;
                    }
                    let mut event = String::new();
                    let mut data_str = String::new();
                    for line in frame.lines() {
                        if let Some(n) = line.strip_prefix("event: ") {
                            event = n.to_string();
                        } else if let Some(d) = line.strip_prefix("data: ") {
                            data_str = d.to_string();
                        }
                    }
                    match serde_json::from_str::<Value>(&data_str) {
                        Ok(data) => {
                            for out_frame in filter.feed(&event, &data) {
                                emit.push(Ok(Bytes::from(out_frame)));
                            }
                        }
                        // Fail open: forward the frame verbatim if data isn't JSON.
                        Err(_) => emit.push(Ok(Bytes::from(format!("{frame}\n\n")))),
                    }
                }
            }
        }
        futures::stream::iter(emit)
    });

    Box::pin(filtered)
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p proxy --lib tool_sanitizer::tests`
Expected: PASS.

- [ ] **Step 6: Clippy + fmt**

Run: `cargo clippy -p proxy --lib -- -D warnings && cargo fmt`
Expected: clean (remove any dead scaffolding flagged).

- [ ] **Step 7: Commit**

```bash
git add crates/proxy/src/adapters/providers/tool_sanitizer.rs
git commit -m "feat(proxy): add tool_sanitizer streaming no-op filter"
```

---

### Task 5: Wire the filter into KimiProvider

**Files:**
- Modify: `crates/proxy/src/adapters/providers/kimi.rs`
- Modify: `crates/proxy/src/adapters/providers/builder.rs:113-118`
- Test: `crates/proxy/src/adapters/providers/kimi.rs` tests mod

**Interfaces:**
- Consumes: `tool_sanitizer::{sanitize_buffered, sanitize_stream}` (Tasks 3–4); `ProviderConfig.sanitize_empty_tools` (Task 1).
- Produces: `KimiProvider::configure(http, base_url, openai_base_url, auth, sanitize_empty_tools: bool)`; `forward` applies the filter when the flag is set.

- [ ] **Step 1: Write the failing constructor test**

In `kimi.rs` tests mod, add:

```rust
#[test]
fn configure_sets_sanitize_flag() {
    let p = KimiProvider::configure(
        reqwest::Client::new(),
        None,
        None,
        AuthHeader::Passthrough,
        true,
    );
    assert!(p.sanitize_empty_tools);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p proxy --lib kimi::tests::configure_sets_sanitize_flag`
Expected: FAIL — `configure` takes 4 args, no field `sanitize_empty_tools`.

- [ ] **Step 3: Add the field + thread it through constructors**

In `kimi.rs`:

- Add field to the struct:
```rust
pub struct KimiProvider {
    base_url: String,
    openai_base_url: Option<String>,
    http: reqwest::Client,
    auth: AuthHeader,
    sanitize_empty_tools: bool,
}
```
- `new` and `with_auth`: pass `false` to `build`.
- `configure`: add a `sanitize_empty_tools: bool` param and forward it to `build`.
- `build`: add the `sanitize_empty_tools: bool` param and store it.

```rust
    pub fn configure(
        http: reqwest::Client,
        base_url: Option<String>,
        openai_base_url: Option<String>,
        auth: AuthHeader,
        sanitize_empty_tools: bool,
    ) -> Self {
        Self::build(
            http,
            base_url.unwrap_or_else(|| DEFAULT_BASE_URL.into()),
            Some(openai_base_url.unwrap_or_else(|| DEFAULT_OPENAI_BASE_URL.into())),
            auth,
            sanitize_empty_tools,
        )
    }

    fn build(
        http: reqwest::Client,
        base_url: String,
        openai_base_url: Option<String>,
        auth: AuthHeader,
        sanitize_empty_tools: bool,
    ) -> Self {
        Self { base_url, openai_base_url, http, auth, sanitize_empty_tools }
    }
```

Update the existing `configure_defaults_to_moonshot_ai` test call to pass `false` as the 5th arg.

- [ ] **Step 4: Apply the filter in `forward`**

Replace the body of `forward` so it wraps the response when the flag is set:

```rust
    async fn forward(
        &self,
        path: &str,
        headers: &HeaderMap,
        body: Bytes,
        streaming: bool,
    ) -> Result<UpstreamResponse, ProxyError> {
        let resp = messages_protocol::forward(
            &self.http,
            &self.base_url,
            &self.auth,
            path,
            headers,
            body,
            streaming,
            self.name(),
        )
        .await?;

        if !self.sanitize_empty_tools {
            return Ok(resp);
        }
        // Only sanitize successful responses; error bodies pass through.
        Ok(match resp {
            UpstreamResponse::Buffered { status, headers, body, provider_id, translation_direction }
                if (200..300).contains(&status) =>
            {
                UpstreamResponse::Buffered {
                    status,
                    headers,
                    body: super::tool_sanitizer::sanitize_buffered(&body),
                    provider_id,
                    translation_direction,
                }
            }
            UpstreamResponse::Streaming { status, headers, body, provider_id, translation_direction }
                if (200..300).contains(&status) =>
            {
                UpstreamResponse::Streaming {
                    status,
                    headers,
                    body: super::tool_sanitizer::sanitize_stream(body),
                    provider_id,
                    translation_direction,
                }
            }
            other => other,
        })
    }
```

- [ ] **Step 5: Update the builder call site** (`builder.rs:113-118`):

```rust
        ProviderKind::Kimi => Arc::new(KimiProvider::configure(
            http,
            p.base_url.clone(),
            p.openai_base_url.clone(),
            auth,
            p.sanitize_empty_tools,
        )),
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p proxy --lib kimi`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/proxy/src/adapters/providers/kimi.rs crates/proxy/src/adapters/providers/builder.rs
git commit -m "feat(proxy): wire no-op tool filter into KimiProvider (opt-in)"
```

---

### Task 6: End-to-end streaming integration test (wiremock)

**Files:**
- Modify: `crates/proxy/src/adapters/providers/kimi.rs` tests mod (add `#[tokio::test]` cases using `wiremock`)

**Interfaces:**
- Consumes: `KimiProvider::configure` with the sanitize flag (Task 5); the `Provider::forward` trait method.

- [ ] **Step 1: Write the failing end-to-end test**

Add to `kimi.rs` tests mod (mirror how other providers set up `wiremock` — `MockServer`, `Mock`, `ResponseTemplate`; import them under `#[cfg(test)]`). Use the mock as `base_url`:

```rust
#[tokio::test]
async fn forward_streaming_drops_noop_tool_and_forces_end_turn() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    let sse = concat!(
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"model\":\"kimi\",\"content\":[]}}\n\n",
        "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"t1\",\"name\":\"bash\",\"input\":{}}}\n\n",
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"command\\\":\\\":\\\"}\"}}\n\n",
        "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":3}}\n\n",
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    );
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(sse))
        .mount(&server)
        .await;

    let provider = KimiProvider::configure(
        reqwest::Client::new(),
        Some(server.uri()),
        None,
        AuthHeader::Passthrough,
        true, // sanitize ON
    );
    let resp = provider
        .forward("/v1/messages", &HeaderMap::new(), Bytes::from("{}"), true)
        .await
        .unwrap();

    let collected = collect_streaming_body(resp).await;
    assert!(!collected.contains("tool_use"), "no-op tool_use must be dropped: {collected}");
    assert!(collected.contains("\"stop_reason\":\"end_turn\""), "stop_reason must be rewritten: {collected}");
}

#[tokio::test]
async fn forward_streaming_passthrough_when_flag_off() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    let sse = concat!(
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"model\":\"kimi\",\"content\":[]}}\n\n",
        "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"t1\",\"name\":\"bash\",\"input\":{}}}\n\n",
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"command\\\":\\\":\\\"}\"}}\n\n",
        "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":3}}\n\n",
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    );
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(sse))
        .mount(&server)
        .await;

    let provider = KimiProvider::configure(
        reqwest::Client::new(),
        Some(server.uri()),
        None,
        AuthHeader::Passthrough,
        false, // sanitize OFF
    );
    let resp = provider
        .forward("/v1/messages", &HeaderMap::new(), Bytes::from("{}"), true)
        .await
        .unwrap();

    let collected = collect_streaming_body(resp).await;
    assert!(collected.contains("tool_use"), "flag off must pass tool_use through");
    assert!(collected.contains("\"stop_reason\":\"tool_use\""), "flag off must keep stop_reason");
}

// Helper: drain a Streaming UpstreamResponse into a String.
async fn collect_streaming_body(resp: UpstreamResponse) -> String {
    use futures::StreamExt;
    match resp {
        UpstreamResponse::Streaming { body, .. } => {
            let mut s = String::new();
            let mut body = body;
            while let Some(chunk) = body.next().await {
                s.push_str(&String::from_utf8_lossy(&chunk.unwrap()));
            }
            s
        }
        UpstreamResponse::Buffered { body, .. } => String::from_utf8_lossy(&body).to_string(),
    }
}
```

- [ ] **Step 2: Run tests to verify they fail then pass**

Run: `cargo test -p proxy --lib kimi::tests::forward_streaming`
Expected: FAIL first if any helper/import is missing; once compiling, both PASS. Fix imports (`UpstreamResponse`, `futures`) as the compiler directs.

- [ ] **Step 3: Full gate**

Run: `cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --check`
Expected: PASS / clean.

- [ ] **Step 4: Commit**

```bash
git add crates/proxy/src/adapters/providers/kimi.rs
git commit -m "test(proxy): end-to-end no-op tool filter streaming tests"
```

---

## Self-Review

**Spec coverage:**
- Config toggle `sanitize_empty_tools` default false → Task 1. ✔
- SQLite column + migration + read/write → Task 2. ✔
- `tool_sanitizer` module, `is_noop_tool_use`, buffered → Task 3. ✔
- Streaming SSE state machine (buffer tool block, defer message_delta, rewrite stop_reason, preserve usage, fail open) → Task 4. ✔
- Wire into KimiProvider Anthropic `forward` only; OpenAI path untouched → Task 5. ✔
- No-op definition `{"", ":", "true"}` after trim → Task 3 (`is_noop_tool_use`). ✔
- Preserve text blocks; force end_turn only when no tool_use remains → Tasks 3 & 4 tests. ✔
- Fail open on parse errors → Tasks 3 (`sanitize_buffered`) & 4 (`sanitize_stream` Err arm). ✔
- Non-2xx responses not sanitized → Task 5 (`(200..300)` guard). ✔
- wiremock integration, flag on & off → Task 6. ✔
- Disable at runtime via admin API → Task 1 mapping (round-trips the flag). ✔

**Placeholder scan:** No TBD/TODO. All code blocks are complete and self-contained.

**Type consistency:** `is_noop_tool_use(&Value) -> bool`, `sanitize_buffered(&[u8]) -> Bytes`, `sanitize_stream(BoxedByteStream) -> BoxedByteStream`, `AnthropicNoopFilter::{new, feed}`, `KimiProvider::configure(.., bool)` — names consistent across Tasks 3–6. `sanitize_empty_tools` field/column/DTO name identical across Tasks 1–5.
