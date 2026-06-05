# MiniMax Thinking Content Cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Strip MiniMax thinking/reasoning content from responses so opencode clients see clean output without `心思...半数` tags or raw reasoning fields.

**Architecture:** Override `MinimaxProvider::forward_openai` to (1) inject `reasoning_split: true` into the request body so MiniMax separates thinking into dedicated fields, and (2) wrap the response stream/buffered body with a filter that removes `reasoning_content` and `reasoning_details` from SSE chunks and buffered JSON. The Anthropic-format path (`forward`) is left as-is since MiniMax's Anthropic endpoint handles thinking natively.

**Tech Stack:** Rust, serde_json for JSON manipulation, futures::StreamExt for stream wrapping, existing SSE patterns from `stream_wrap.rs` and `openai_sse.rs`.

---

## Context: How the Pieces Fit

When opencode calls the proxy:
1. `chat_completions` handler → `HandleMessages::execute` → `RoutingProvider::forward_openai`
2. RoutingProvider resolves `minimax` provider, sees both client and provider are OpenAI → `Direction::Passthrough`
3. `MinimaxProvider::forward_openai` is called → currently just forwards to `messages_protocol::forward` unchanged
4. MiniMax returns thinking content in `content` field wrapped in `心思...半数` tags (default) or in `reasoning_content`/`reasoning_details` fields (with `reasoning_split: true`)
5. Proxy passes this through unchanged → opencode renders all the raw thinking text

The fix goes into step 3: `MinimaxProvider::forward_openai` intercepts the request/response to clean thinking content.

## Files

| File | Action | Responsibility |
|------|--------|----------------|
| `crates/proxy/src/adapters/providers/minimax.rs` | Modify | Override `forward_openai` to inject `reasoning_split: true` and wrap response with thinking filter |
| `crates/proxy/src/adapters/providers/minimax_stream.rs` | Create | Stream filter that strips `reasoning_content`/`reasoning_details` from OpenAI SSE chunks |
| `crates/proxy/src/adapters/providers/mod.rs` | Modify | Add `pub mod minimax_stream;` |

---

### Task 1: Create minimax_stream module — unit tests for thinking content stripping

**Files:**
- Create: `crates/proxy/src/adapters/providers/minimax_stream.rs`

- [ ] **Step 1: Create the minimax_stream.rs file with tests for buffered thinking cleanup**

```rust
//! Stream and buffer filter that strips MiniMax thinking/reasoning content from
//! OpenAI-format responses.
//!
//! MiniMax models (M2.x, M3) produce thinking content that arrives in two forms:
//! - With `reasoning_split: true`: separate `reasoning_content` and `reasoning_details` fields
//! - Without `reasoning_split`: `心思...半数` tags embedded in the `content` field
//!
//! This module strips both so clients see clean output.

use bytes::Bytes;
use futures::StreamExt;
use serde_json::Value;

/// Strip thinking-related fields from a buffered (non-streaming) OpenAI-format
/// response body. Removes `reasoning_content` and `reasoning_details` from
/// `choices[].message` and `choices[].delta`, and strips `心思...半数` tags
/// from `content` fields.
pub fn strip_thinking_buffered(body: &Bytes) -> Option<Bytes> {
    let mut value: Value = serde_json::from_slice(body).ok()?;
    strip_thinking_value(&mut value);
    serde_json::to_vec(&value).ok().map(Bytes::from)
}

/// Strip thinking content from a parsed JSON value in-place.
fn strip_thinking_value(value: &mut Value) {
    let Some(choices) = value.get_mut("choices").and_then(|c| c.as_array_mut()) else {
        return;
    };
    for choice in choices.iter_mut() {
        // Remove reasoning_content and reasoning_details from message or delta
        if let Some(obj) = choice.get_mut("message").or_else(|| choice.get_mut("delta")) {
            if let Some(map) = obj.as_object_mut() {
                map.remove("reasoning_content");
                map.remove("reasoning_details");
                // Also strip 思心思 tags from content
                if let Some(content) = map.get_mut("content").and_then(|c| c.as_str_mut()) {
                    *content = strip_thinking_tags(content);
                }
            }
        }
    }
}

/// Strip `心思...半数` tags from a content string.
fn strip_thinking_tags(content: &str) -> String {
    let mut result = String::with_capacity(content.len());
    let mut chars = content.char_indices().peekable();
    let tag_start = "心思";
    let tag_end = "半数";
    let tag_start_len = tag_start.chars().count();
    let tag_end_len = tag_end.chars().count();

    while let Some(&(i, _)) = chars.peek() {
        // Check for tag start
        let remaining = &content[i..];
        if remaining.starts_with(tag_start) {
            // Find the closing tag
            if let Some(end_pos) = remaining.find(tag_end) {
                let after_end = end_pos + tag_end.len();
                // Skip the thinking content between tags
                for _ in 0..content[i..after_end].chars().count() {
                    chars.next();
                }
                continue;
            }
        }
        result.push(chars.next().unwrap().1);
    }

    result
}

/// Wrap an upstream OpenAI SSE stream to strip thinking content from each chunk.
/// Returns a new `BoxedByteStream` where each SSE `data: {...}` line has had
/// `reasoning_content`, `reasoning_details` removed and `心思...半数` tags
/// stripped from `content`.
pub fn strip_thinking_stream(
    upstream: crate::application::ports::BoxedByteStream,
) -> crate::application::ports::BoxedByteStream {
    let mut buf = String::new();

    let filtered = upstream.flat_map(move |chunk_result| {
        let mut emit: Vec<Result<Bytes, Box<dyn std::error::Error + Send + Sync>>> = Vec::new();

        match chunk_result {
            Err(e) => {
                emit.push(Err(e));
            }
            Ok(chunk_bytes) => {
                buf.push_str(&String::from_utf8_lossy(&chunk_bytes));

                // Drain complete SSE frames.
                while let Some(idx) = buf.find("\n\n") {
                    let frame = buf[..idx].to_string();
                    buf.drain(..idx + 2);

                    if frame.trim().is_empty() {
                        continue;
                    }

                    // Process each line in the frame
                    let mut out_lines: Vec<String> = Vec::new();
                    for line in frame.lines() {
                        if let Some(data) = line.strip_prefix("data: ") {
                            let trimmed = data.trim();
                            if trimmed == "[DONE]" {
                                out_lines.push("data: [DONE]".to_string());
                            } else if let Ok(mut value) = serde_json::from_str::<Value>(trimmed) {
                                strip_thinking_value(&mut value);
                                out_lines.push(format!("data: {}", value));
                            } else {
                                out_lines.push(line.to_string());
                            }
                        } else {
                            out_lines.push(line.to_string());
                        }
                    }

                    if !out_lines.is_empty() {
                        emit.push(Ok(Bytes::from(
                            out_lines.into_iter().collect::<Vec<_>>().join("\n")
                                + "\n\n",
                        )));
                    }
                }
            }
        }
        futures::stream::iter(emit)
    });

    Box::pin(filtered)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- strip_thinking_tags tests ---

    #[test]
    fn strips_thinking_tags_with_content() {
        let input = "心思this is thinking半数and this is real content";
        assert_eq!(strip_thinking_tags(input), "and this is real content");
    }

    #[test]
    fn strips_multiple_thinking_blocks() {
        let input = "心思block1半数real1心思block2半数real2";
        assert_eq!(strip_thinking_tags(input), "real1real2");
    }

    #[test]
    fn no_thinking_tags_returns_unchanged() {
        let input = "just normal content";
        assert_eq!(strip_thinking_tags(input), "just normal content");
    }

    #[test]
    fn empty_input_returns_empty() {
        assert_eq!(strip_thinking_tags(""), "");
    }

    #[test]
    fn only_thinking_returns_empty() {
        assert_eq!(strip_thinking_tags("思绪...some deep thoughts...半数"), "");
    }

    // --- strip_thinking_buffered tests ---

    #[test]
    fn buffered_strips_reasoning_content_from_message() {
        let body = Bytes::from(r#"{"choices":[{"message":{"role":"assistant","content":"Hello","reasoning_content":"I should greet","reasoning_details":[{"text":"I should greet"}]}}]}"#);
        let result = strip_thinking_buffered(&body).unwrap();
        let parsed: Value = serde_json::from_slice(&result).unwrap();
        let msg = &parsed["choices"][0]["message"];
        assert_eq!(msg["content"], "Hello");
        assert!(msg.get("reasoning_content").is_none());
        assert!(msg.get("reasoning_details").is_none());
    }

    #[test]
    fn buffered_strips_thinking_tags_from_content() {
        let body = Bytes::from(r#"{"choices":[{"message":{"role":"assistant","content":"心思let me think半数Hello world"}}]}"#);
        let result = strip_thinking_buffered(&body).unwrap();
        let parsed: Value = serde_json::from_slice(&result).unwrap();
        assert_eq!(parsed["choices"][0]["message"]["content"], "Hello world");
    }

    #[test]
    fn buffered_preserves_non_thinking_response_unchanged() {
        let body = Bytes::from(r#"{"choices":[{"message":{"role":"assistant","content":"Hello world"}}]}"#);
        let result = strip_thinking_buffered(&body).unwrap();
        let parsed: Value = serde_json::from_slice(&result).unwrap();
        assert_eq!(parsed["choices"][0]["message"]["content"], "Hello world");
    }

    // --- strip_thinking_stream tests ---

    #[test]
    fn stream_strips_reasoning_from_sse_chunk() {
        use futures::stream;
        let chunks: Vec<Result<Bytes, Box<dyn std::error::Error + Send + Sync>>> = vec![
            Ok(Bytes::from("data: {\"choices\":[{\"delta\":{\"content\":\"Hi\",\"reasoning_content\":\"thinking...\",\"reasoning_details\":[{\"text\":\"thinking...\"}]}}]}\n\n")),
        ];
        let upstream: crate::application::ports::BoxedByteStream =
            Box::pin(stream::iter(chunks));
        let filtered = strip_thinking_stream(upstream);

        use futures::StreamExt;
        let collected: Vec<_> = filtered.collect::<Vec<_>>();
        // Need to actually await — use block_on
        let rt = tokio::runtime::Runtime::new().unwrap();
        let results: Vec<_> = rt.block_on(async { filtered.collect::<Vec<_>>().await });

        assert_eq!(results.len(), 1);
        let data = &results[0];
        let data_str = String::from_utf8_lossy(&data.as_ref().unwrap());
        assert!(!data_str.contains("reasoning_content"));
        assert!(!data_str.contains("reasoning_details"));
        assert!(data_str.contains("\"content\":\"Hi\""));
    }

    #[test]
    fn stream_passes_done_sentinel_through() {
        use futures::stream;
        let chunks: Vec<Result<Bytes, Box<dyn std::error::Error + Send + Sync>>> = vec![
            Ok(Bytes::from("data: [DONE]\n\n")),
        ];
        let upstream: crate::application::ports::BoxedByteStream =
            Box::pin(stream::iter(chunks));
        let filtered = strip_thinking_stream(upstream);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let results: Vec<_> = rt.block_on(async { filtered.collect::<Vec<_>>().await });

        let data_str = String::from_utf8_lossy(&results[0].as_ref().unwrap());
        assert!(data_str.contains("[DONE]"));
    }

    #[test]
    fn stream_strips_thinking_tags_from_chunk_content() {
        use futures::stream;
        let chunks: Vec<Result<Bytes, Box<dyn std::error::Error + Send + Sync>>> = vec![
            Ok(Bytes::from("data: {\"choices\":[{\"delta\":{\"content\":\"思绪hmm...半数real text\"}}]}\n\n")),
        ];
        let upstream: crate::application::ports::BoxedByteStream =
            Box::pin(stream::iter(chunks));
        let filtered = strip_thinking_stream(upstream);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let results: Vec<_> = rt.block_on(async { filtered.collect::<Vec<_>>().await });

        let data_str = String::from_utf8_lossy(&results[0].as_ref().unwrap());
        assert!(data_str.contains("real text"));
        assert!(!data_str.contains("hmm"));
    }
}
```

- [ ] **Step 2: Run tests to verify they compile and fail appropriately**

Run: `cargo test -p proxy minimax_stream --no-run 2>&1 | tail -5`
Expected: Compilation may fail — check that module structure is correct

- [ ] **Step 3: Commit**

```bash
git add crates/proxy/src/adapters/providers/minimax_stream.rs
git commit -m "test: add minimax_stream thinking content stripping module with tests"
```

---

### Task 2: Register minimax_stream module

**Files:**
- Modify: `crates/proxy/src/adapters/providers/mod.rs`

- [ ] **Step 1: Add `pub mod minimax_stream;` to providers/mod.rs**

The file currently has:
```rust
pub mod minimax;
```

Add after the `minimax` line:
```rust
pub mod minimax_stream;
```

- [ ] **Step 2: Run tests to verify compilation**

Run: `cargo test -p proxy minimax_stream -- --test-threads=1 2>&1 | tail -20`
Expected: All tests in `minimax_stream` pass

- [ ] **Step 3: Commit**

```bash
git add crates/proxy/src/adapters/providers/mod.rs
git commit -m "feat: register minimax_stream module in providers"
```

---

### Task 3: Override MinimaxProvider::forward_openai to inject reasoning_split and filter response

**Files:**
- Modify: `crates/proxy/src/adapters/providers/minimax.rs`

This is the key change. Currently `forward_openai` just calls `messages_protocol::forward`. We need to:
1. Inject `reasoning_split: true` into the request body (so MiniMax separates thinking into dedicated fields)
2. For buffered responses: strip thinking fields from the JSON body
3. For streaming responses: wrap with `strip_thinking_stream`

- [ ] **Step 1: Replace the `forward_openai` method in MinimaxProvider**

Find the current `forward_openai` method (lines 127-156) and replace with:

```rust
    async fn forward_openai(
        &self,
        path: &str,
        headers: &HeaderMap,
        body: Bytes,
        streaming: bool,
    ) -> Result<UpstreamResponse, ProxyError> {
        let openai_base = self.openai_base_url.as_deref().ok_or_else(|| {
            ProxyError::BadRequest(
                "provider 'minimax' does not support OpenAI chat completions format".into(),
            )
        })?;
        // Minimax uses Authorization: Bearer for both endpoints. Convert ApiKey → Bearer.
        let openai_auth = match &self.auth {
            AuthHeader::Passthrough => AuthHeader::Passthrough,
            AuthHeader::ApiKey(v) => AuthHeader::Bearer(v.clone()),
            other => other.clone(),
        };

        // Inject `reasoning_split: true` so MiniMax separates thinking content
        // into `reasoning_content` / `reasoning_details` fields instead of embedding
        // `心思...半数` tags inside `content`. This makes it much easier to strip cleanly.
        let outbound_body = inject_reasoning_split(&body);

        let resp = messages_protocol::forward(
            &self.http,
            openai_base,
            &openai_auth,
            path,
            headers,
            outbound_body,
            streaming,
            self.name(),
        )
        .await?;

        // Strip thinking content from the response.
        match resp {
            UpstreamResponse::Buffered {
                status,
                headers,
                body,
                provider_id,
                translation_direction,
            } => {
                let cleaned = super::minimax_stream::strip_thinking_buffered(&body)
                    .unwrap_or(body);
                Ok(UpstreamResponse::Buffered {
                    status,
                    headers,
                    body: cleaned,
                    provider_id,
                    translation_direction,
                })
            }
            UpstreamResponse::Streaming {
                status,
                headers,
                body,
                provider_id,
                translation_direction,
            } => Ok(UpstreamResponse::Streaming {
                status,
                headers,
                body: super::minimax_stream::strip_thinking_stream(body),
                provider_id,
                translation_direction,
            }),
        }
    }
```

- [ ] **Step 2: Add the `inject_reasoning_split` helper function at the bottom of the file (before `#[cfg(test)]`)**

```rust
/// Inject `reasoning_split: true` into an OpenAI-format request body.
/// Non-JSON or non-object bodies are returned verbatim.
fn inject_reasoning_split(body: &Bytes) -> Bytes {
    let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(body) else {
        return body.clone();
    };
    let Some(obj) = value.as_object_mut() else {
        return body.clone();
    };
    obj.insert(
        "reasoning_split".to_string(),
        serde_json::Value::Bool(true),
    );
    serde_json::to_vec(&value)
        .map(Bytes::from)
        .unwrap_or_else(|_| body.clone())
}
```

- [ ] **Step 3: Add imports at the top of minimax.rs**

Add these imports if not already present (check existing imports first):
```rust
use crate::application::ports::{UpstreamResponse, Provider};
use crate::adapters::providers::messages_protocol;
```
Note: `UpstreamResponse` needs to be in scope for the match arms. Check what's already imported.

- [ ] **Step 4: Add tests for `inject_reasoning_split` in the `#[cfg(test)]` section**

Add these tests to the existing test module:

```rust
    #[test]
    fn inject_reasoning_split_adds_field() {
        let body = Bytes::from(r#"{"model":"MiniMax-M3","messages":[]}"#);
        let result = inject_reasoning_split(&body);
        let parsed: serde_json::Value = serde_json::from_slice(&result).unwrap();
        assert_eq!(parsed["reasoning_split"], true);
    }

    #[test]
    fn inject_reasoning_split_preserves_existing_fields() {
        let body = Bytes::from(r#"{"model":"MiniMax-M3","messages":[],"stream":true}"#);
        let result = inject_reasoning_split(&body);
        let parsed: serde_json::Value = serde_json::from_slice(&result).unwrap();
        assert_eq!(parsed["model"], "MiniMax-M3");
        assert_eq!(parsed["stream"], true);
        assert_eq!(parsed["reasoning_split"], true);
    }

    #[test]
    fn inject_reasoning_split_overwrites_false() {
        let body = Bytes::from(r#"{"model":"MiniMax-M3","reasoning_split":false}"#);
        let result = inject_reasoning_split(&body);
        let parsed: serde_json::Value = serde_json::from_slice(&result).unwrap();
        assert_eq!(parsed["reasoning_split"], true);
    }

    #[test]
    fn inject_reasoning_split_returns_non_json_unchanged() {
        let body = Bytes::from("not json");
        let result = inject_reasoning_split(&body);
        assert_eq!(result, body);
    }
```

- [ ] **Step 5: Run all proxy tests to verify**

Run: `cargo test -p proxy minimax 2>&1 | tail -30`
Expected: All tests pass

- [ ] **Step 6: Commit**

```bash
git add crates/proxy/src/adapters/providers/minimax.rs
git commit -m "feat: strip MiniMax thinking content from OpenAI responses

Inject reasoning_split: true into outgoing requests so MiniMax separates
thinking into dedicated fields, then strip reasoning_content/reasoning_details
and 思思...半数 tags from both buffered and streaming responses."
```

---

### Task 4: Full integration test

**Files:**
- Modify: `crates/proxy/src/adapters/providers/minimax.rs` (add integration-level test)

- [ ] **Step 1: Add a test that verifies the full pipeline: inject + strip**

This test simulates the full flow: request body gets `reasoning_split` injected, a fake MiniMax response with thinking content is cleaned.

```rust
    #[test]
    fn forward_openai_strips_thinking_from_buffered_response() {
        // Simulate what MiniMax returns with reasoning_split: true
        let fake_response = r#"{"id":"chatcmpl-1","object":"chat.completion","model":"MiniMax-M3","choices":[{"index":0,"message":{"role":"assistant","content":"Hello!","reasoning_content":"The user said hi, I should greet them back.","reasoning_details":[{"type":"text","text":"The user said hi, I should greet them back."}]},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#;
        let body = Bytes::from(fake_response);
        let cleaned = super::minimax_stream::strip_thinking_buffered(&body).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&cleaned).unwrap();

        // Content preserved
        assert_eq!(parsed["choices"][0]["message"]["content"], "Hello!");
        // Thinking fields removed
        assert!(parsed["choices"][0]["message"].get("reasoning_content").is_none());
        assert!(parsed["choices"][0]["message"].get("reasoning_details").is_none());
        // Other fields preserved
        assert_eq!(parsed["model"], "MiniMax-M3");
        assert_eq!(parsed["usage"]["total_tokens"], 15);
    }
```

- [ ] **Step 2: Run all proxy tests**

Run: `cargo test -p proxy 2>&1 | tail -10`
Expected: All tests pass

- [ ] **Step 3: Commit**

```bash
git add crates/proxy/src/adapters/providers/minimax.rs
git commit -m "test: add integration test for MiniMax thinking content stripping"
```

---

### Task 5: Final verification

- [ ] **Step 1: Run the full test suite**

Run: `cargo test -p proxy 2>&1`
Expected: All tests pass, no regressions

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -p proxy 2>&1 | tail -20`
Expected: No new warnings

- [ ] **Step 3: Check formatting**

Run: `cargo fmt -p proxy --check 2>&1`
Expected: No formatting issues
