# Provider Namespace Routing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Allow clients to target a specific provider by sending `model` as `provider-name/model-id` (e.g. `zai/glm-5`), with the proxy stripping the prefix before forwarding.

**Architecture:** Extend `RoutingProvider` to hold a name→provider leaf map. In `forward()`, check for a `/` in the model name — if found, look up the provider by name, rewrite the body, and forward directly. Otherwise, existing glob routing applies.

**Tech Stack:** Rust, serde_json, globset, async_trait

---

## File Structure

| File | Change | Responsibility |
|------|--------|----------------|
| `crates/proxy/src/adapters/providers/routing.rs` | Modify | Add `leaves` field, namespace parsing, body rewrite, updated `forward()` |
| `crates/proxy/src/adapters/providers/builder.rs` | Modify | Pass leaves map into `RoutingProviderBuilder` |

---

### Task 1: Add `split_namespace` helper and tests

**Files:**
- Modify: `crates/proxy/src/adapters/providers/routing.rs` (new function + tests)

- [ ] **Step 1: Write the failing tests**

Add these tests inside the `#[cfg(test)] mod tests` block at the end of `routing.rs`:

```rust
#[test]
fn split_namespace_returns_some_for_slash_separated() {
    assert_eq!(split_namespace("zai/glm-5"), Some(("zai", "glm-5")));
}

#[test]
fn split_namespace_returns_none_for_no_slash() {
    assert_eq!(split_namespace("glm-5"), None);
}

#[test]
fn split_namespace_splits_on_first_slash_only() {
    assert_eq!(split_namespace("zai/glm-5/extra"), Some(("zai", "glm-5/extra")));
}

#[test]
fn split_namespace_empty_after_slash() {
    assert_eq!(split_namespace("zai/"), Some(("zai", "")));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p proxy -- routing::tests::split_namespace`
Expected: compile error — `split_namespace` does not exist

- [ ] **Step 3: Write the implementation**

Add this function in `routing.rs`, just above `impl RoutingProvider` (around line 100):

```rust
/// Split `model` on the first `/`. Returns `Some((namespace, bare_model))`
/// if a `/` is present, `None` otherwise.
fn split_namespace(model: &str) -> Option<(&str, &str)> {
    let idx = model.find('/')?;
    Some((&model[..idx], &model[idx + 1..]))
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p proxy -- routing::tests::split_namespace`
Expected: 4 passing tests

- [ ] **Step 5: Commit**

```bash
git add crates/proxy/src/adapters/providers/routing.rs
git commit -m "feat(proxy): add split_namespace helper for provider namespace routing"
```

---

### Task 2: Add `rewrite_model_in_body` helper and tests

**Files:**
- Modify: `crates/proxy/src/adapters/providers/routing.rs` (new function + tests)

- [ ] **Step 1: Write the failing tests**

Add these tests inside the `#[cfg(test)] mod tests` block:

```rust
#[test]
fn rewrite_model_in_body_replaces_model_field() {
    let body = br#"{"model":"zai/glm-5","messages":[{"role":"user","content":"hi"}]}"#;
    let result = rewrite_model_in_body(body, "glm-5").unwrap();
    let v: serde_json::Value = serde_json::from_slice(&result).unwrap();
    assert_eq!(v["model"].as_str().unwrap(), "glm-5");
}

#[test]
fn rewrite_model_in_body_preserves_other_fields() {
    let body = br#"{"model":"zai/glm-5","max_tokens":50,"stream":true}"#;
    let result = rewrite_model_in_body(body, "glm-5").unwrap();
    let v: serde_json::Value = serde_json::from_slice(&result).unwrap();
    assert_eq!(v["model"].as_str().unwrap(), "glm-5");
    assert_eq!(v["max_tokens"].as_u64().unwrap(), 50);
    assert_eq!(v["stream"].as_bool().unwrap(), true);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p proxy -- routing::tests::rewrite_model_in_body`
Expected: compile error — `rewrite_model_in_body` does not exist

- [ ] **Step 3: Write the implementation**

Add this function in `routing.rs`, next to `split_namespace`:

```rust
/// Replace the `"model"` field in a JSON body with `new_model`, returning
/// the re-serialized body bytes.
fn rewrite_model_in_body(body: &[u8], new_model: &str) -> Result<Vec<u8>, String> {
    let mut value: serde_json::Value =
        serde_json::from_slice(body).map_err(|e| format!("invalid json: {e}"))?;
    value["model"] = serde_json::Value::String(new_model.to_string());
    serde_json::to_vec(&value).map_err(|e| format!("json serialize: {e}"))
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p proxy -- routing::tests::rewrite_model_in_body`
Expected: 2 passing tests

- [ ] **Step 5: Commit**

```bash
git add crates/proxy/src/adapters/providers/routing.rs
git commit -m "feat(proxy): add rewrite_model_in_body helper for namespace stripping"
```

---

### Task 3: Add `leaves` field to `RoutingProvider` and update builder

**Files:**
- Modify: `crates/proxy/src/adapters/providers/routing.rs` (struct + builder)
- Modify: `crates/proxy/src/adapters/providers/builder.rs` (pass leaves into builder)

- [ ] **Step 1: Write the failing test**

Add this test:

```rust
#[test]
fn forward_with_namespace_routes_to_named_provider() {
    use crate::adapters::providers::ZaiProvider;

    let zai: Arc<dyn Provider> = Arc::new(ZaiProvider::new(reqwest::Client::new()));
    let anthropic: Arc<dyn Provider> = Arc::new(AnthropicProvider::new(reqwest::Client::new()));

    let mut leaves = std::collections::HashMap::new();
    leaves.insert("zai".to_string(), zai.clone());
    leaves.insert("anthropic".to_string(), anthropic.clone());

    // Catch-all route points to anthropic
    let router = RoutingProvider::builder()
        .rule("*", RoutingStrategy::Failover, anthropic.clone(), vec![])
        .unwrap()
        .with_leaves(leaves)
        .build();

    // "zai/glm-5" should route to the zai provider (namespace wins over glob rule)
    let body = Bytes::from_static(br#"{"model":"zai/glm-5","messages":[]}"#);
    // We can't easily test forward() without a real upstream, but we can test select_with_namespace
    let resolved = router.resolve_provider("zai/glm-5");
    assert!(resolved.is_some(), "namespace should resolve provider");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p proxy -- routing::tests::forward_with_namespace`
Expected: compile error — `with_leaves` / `resolve_provider` / `leaves` do not exist

- [ ] **Step 3: Update `RoutingProvider` struct**

In `routing.rs`, add `leaves` field to `RoutingProvider`:

```rust
pub struct RoutingProvider {
    rules: Vec<Route>,
    /// Counter for round-robin rotation. Incremented per request.
    rr_counter: AtomicUsize,
    /// Name → provider map for namespace routing (e.g. "zai" → ZaiProvider).
    leaves: std::collections::HashMap<String, Arc<dyn Provider>>,
}
```

Update `RoutingProviderBuilder` to accept leaves:

```rust
#[derive(Default)]
pub struct RoutingProviderBuilder {
    rules: Vec<Route>,
    leaves: std::collections::HashMap<String, Arc<dyn Provider>>,
}

impl RoutingProviderBuilder {
    pub fn leaves(mut self, leaves: std::collections::HashMap<String, Arc<dyn Provider>>) -> Self {
        self.leaves = leaves;
        self
    }

    // ... existing rule() method unchanged ...

    pub fn build(self) -> RoutingProvider {
        RoutingProvider {
            rules: self.rules,
            rr_counter: AtomicUsize::new(0),
            leaves: self.leaves,
        }
    }
}
```

Add `resolve_provider` method to `RoutingProvider`:

```rust
impl RoutingProvider {
    pub fn builder() -> RoutingProviderBuilder {
        RoutingProviderBuilder::default()
    }

    fn select(&self, model: &str) -> Option<&Route> {
        self.rules.iter().find(|r| r.matcher.is_match(model))
    }

    /// If `model` contains a namespace prefix (e.g. "zai/glm-5"), look up
    /// the provider by name. Returns the provider and the bare model name.
    fn resolve_provider(&self, model: &str) -> Option<(&Arc<dyn Provider>, &str)> {
        let (namespace, bare_model) = split_namespace(model)?;
        let provider = self.leaves.get(namespace)?;
        Some((provider, bare_model))
    }
}
```

- [ ] **Step 4: Update `builder.rs` to pass leaves**

In `builder.rs`, update `build_routing_provider` to pass the leaves map:

```rust
pub fn build_routing_provider(
    cfg: &Config,
    leaves: &HashMap<String, Arc<dyn Provider>>,
) -> Result<Arc<dyn Provider>, BuildError> {
    // ... existing indexed/sorting logic unchanged ...

    let mut builder = RoutingProvider::builder();
    for (_pri, rule) in indexed {
        // ... existing rule-building logic unchanged ...
    }
    Ok(Arc::new(builder.leaves(leaves.clone()).build()))
}
```

- [ ] **Step 5: Fix existing tests that call `.build()`**

The existing tests call `.build()` on the builder — they still work because `leaves` defaults to an empty `HashMap`. Verify:

Run: `cargo test -p proxy -- routing::tests`
Expected: All existing tests still pass + the new `forward_with_namespace` test passes

- [ ] **Step 6: Commit**

```bash
git add crates/proxy/src/adapters/providers/routing.rs crates/proxy/src/adapters/providers/builder.rs
git commit -m "feat(proxy): add leaves map and resolve_provider for namespace routing"
```

---

### Task 4: Wire namespace routing into `forward()` and add error handling

**Files:**
- Modify: `crates/proxy/src/adapters/providers/routing.rs` (forward method + tests)

- [ ] **Step 1: Write the failing test for namespace error**

```rust
#[tokio::test]
async fn forward_with_unknown_namespace_returns_400() {
    let router = RoutingProvider::builder()
        .rule("*", RoutingStrategy::Failover, dummy(), vec![])
        .unwrap()
        .build();

    let body = Bytes::from_static(br#"{"model":"nonexistent/glm-5","messages":[]}"#);
    let result = router.forward("/v1/messages", &HeaderMap::new(), body, false).await;
    match result {
        Err(ProxyError::BadRequest(msg)) => {
            assert!(msg.contains("nonexistent"), "error should mention the namespace");
        }
        other => panic!("expected BadRequest, got {:?}", other),
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p proxy -- routing::tests::forward_with_unknown_namespace`
Expected: test runs but gets `no routing rule matches model 'nonexistent/glm-5'` instead of the namespace-specific error

- [ ] **Step 3: Update `forward()` in `Provider` impl**

Replace the existing `forward()` method in the `Provider for RoutingProvider` impl:

```rust
async fn forward(
    &self,
    path: &str,
    headers: &HeaderMap,
    body: Bytes,
    streaming: bool,
) -> Result<UpstreamResponse, ProxyError> {
    let model = messages_protocol::parse_model(&body).map_err(ProxyError::BadRequest)?;

    // Namespace routing: if model contains "/", extract namespace and route
    // directly to the named provider.
    if let Some((provider, bare_model)) = self.resolve_provider(&model) {
        let rewritten = rewrite_model_in_body(&body, bare_model)
            .map_err(ProxyError::BadRequest)?;
        let rewritten_body = Bytes::from(rewritten);
        return provider.forward(path, headers, rewritten_body, streaming).await;
    }

    // If model contains "/" but we couldn't resolve, it's an unknown namespace.
    if split_namespace(&model).is_some() {
        let (ns, _) = split_namespace(&model).unwrap();
        return Err(ProxyError::BadRequest(format!(
            "unknown provider namespace '{ns}'"
        )));
    }

    // No namespace — use existing glob-based routing.
    let route = self.select(&model).ok_or_else(|| {
        ProxyError::BadRequest(format!("no routing rule matches model '{model}'"))
    })?;

    match route.strategy {
        RoutingStrategy::Failover => {
            self.forward_failover(route, path, headers, body, streaming).await
        }
        RoutingStrategy::RoundRobin => {
            self.forward_round_robin(route, path, headers, body, streaming).await
        }
    }
}
```

- [ ] **Step 4: Run all routing tests**

Run: `cargo test -p proxy -- routing::tests`
Expected: All tests pass, including `forward_with_unknown_namespace_returns_400`

- [ ] **Step 5: Commit**

```bash
git add crates/proxy/src/adapters/providers/routing.rs
git commit -m "feat(proxy): wire namespace routing into forward with body rewrite and error handling"
```

---

### Task 5: Update docs

**Files:**
- Modify: `docs/proxy-usage.md`

- [ ] **Step 1: Add Namespace Routing section**

Add a new section after the "Priority" section (after line ~211) and before "OAuth Setup":

```markdown
---

## Namespace Routing

Override routing rules by prefixing the model name with a provider name and `/`:

```
<provider-name>/<model>
```

| Client sends | Routed to | Upstream receives |
|---|---|---|
| `zai/glm-5` | provider `zai` | `{"model":"glm-5"}` |
| `anthropic-work/claude-sonnet-4` | provider `anthropic-work` | `{"model":"claude-sonnet-4"}` |
| `glm-5` | normal routing rules | `{"model":"glm-5"}` |

**Namespace always wins** — it bypasses glob-based routing rules entirely. The proxy strips the prefix before forwarding, so the upstream only sees the bare model name.

If the namespace doesn't match any configured provider name, the proxy returns:

```
400 Bad Request: unknown provider namespace 'nonexistent'
```

This is useful when you have multiple providers of the same kind and want explicit control over which one handles a request, or when testing a specific provider without changing routing rules.
```

- [ ] **Step 2: Commit**

```bash
git add docs/proxy-usage.md
git commit -m "docs: add namespace routing section to proxy usage guide"
```

---

### Task 6: Verify everything works end-to-end

- [ ] **Step 1: Run full test suite**

Run: `cargo test --workspace`
Expected: All tests pass

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --workspace -- -D warnings`
Expected: No warnings

- [ ] **Step 3: Manual smoke test (if proxy is running)**

```bash
# Should route to zai provider, strip namespace, forward "glm-5"
curl http://127.0.0.1:8787/v1/messages \
  -H "content-type: application/json" \
  -H "x-api-key: dummy" \
  -H "anthropic-version: 2023-06-01" \
  -d '{"model":"zai/glm-5","max_tokens":50,"messages":[{"role":"user","content":"Say hello"}]}'
```

Expected: Successful response from Z.ai with `model` field showing `glm-5.x`.

- [ ] **Step 4: Test unknown namespace**

```bash
curl -s http://127.0.0.1:8787/v1/messages \
  -H "content-type: application/json" \
  -H "x-api-key: dummy" \
  -H "anthropic-version: 2023-06-01" \
  -d '{"model":"nonexistent/glm-5","max_tokens":50,"messages":[{"role":"user","content":"hi"}]}'
```

Expected: `400 Bad Request: unknown provider namespace 'nonexistent'`
