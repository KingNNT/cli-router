//! Stream and buffer filter that strips MiniMax thinking/reasoning content from
//! OpenAI-format responses.
//!
//! MiniMax models (M2.x, M3) produce thinking content that arrives in two forms:
//! - With `reasoning_split: true`: separate `reasoning_content` and `reasoning_details` fields
//! - Without `reasoning_split`: 思绪...半数 tags embedded in the `content` field
//!
//! This module strips both so clients see clean output.

use bytes::Bytes;
use futures::StreamExt;
use serde_json::Value;

/// Strip thinking-related fields from a buffered (non-streaming) OpenAI-format
/// response body. Removes `reasoning_content` and `reasoning_details` from
/// `choices[].message` and `choices[].delta`, and strips 思绪...半数 tags
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
        let target_key = if choice.get("message").is_some() {
            "message"
        } else if choice.get("delta").is_some() {
            "delta"
        } else {
            continue;
        };
        if let Some(obj) = choice.get_mut(target_key) {
            if let Some(map) = obj.as_object_mut() {
                map.remove("reasoning_content");
                map.remove("reasoning_details");
                if let Some(content) = map.get("content").and_then(|c| c.as_str()) {
                    let stripped = strip_thinking_tags(content);
                    map.insert("content".to_string(), Value::String(stripped));
                }
            }
        }
    }
}

/// Strip 思绪...半数 tags from a content string.
fn strip_thinking_tags(content: &str) -> String {
    let mut result = String::with_capacity(content.len());
    let mut chars = content.chars().peekable();
    let tag_start: Vec<char> = "思绪".chars().collect();
    let tag_end: Vec<char> = "半数".chars().collect();

    while chars.peek().is_some() {
        // Check if we're at a tag_start
        if starts_with_chars(&chars, &tag_start) {
            // Skip the tag_start chars
            for _ in 0..tag_start.len() {
                chars.next();
            }
            // Find and skip up to tag_end
            let end_len = skip_until_end_tag(&mut chars, &tag_end);
            if end_len > 0 {
                // tag_end was found and skipped, continue processing
                continue;
            }
            // tag_end was not found; the start tag chars are already consumed
            // (they are part of the thinking content that we want to strip anyway)
            continue;
        }
        result.push(chars.next().unwrap());
    }

    result
}

/// Check if the peekable iterator starts with the given char sequence.
fn starts_with_chars<I: Iterator<Item = char> + Clone>(chars: &std::iter::Peekable<I>, prefix: &[char]) -> bool {
    let cloned = chars.clone();
    let mut count = 0;
    for (actual, expected) in cloned.zip(prefix.iter()) {
        if actual != *expected {
            return false;
        }
        count += 1;
    }
    count == prefix.len()
}

/// Skip characters until the end tag sequence is found. Returns the number of
/// end tag chars consumed if found, or 0 if the end of string is reached without
/// finding the tag.
fn skip_until_end_tag<I: Iterator<Item = char> + Clone>(chars: &mut std::iter::Peekable<I>, tag_end: &[char]) -> usize {
    let end_len = tag_end.len();
    let mut buffer: std::collections::VecDeque<char> = std::collections::VecDeque::with_capacity(end_len);

    while let Some(c) = chars.next() {
        buffer.push_back(c);
        if buffer.len() > end_len {
            // Characters that don't form the end tag are discarded (they're thinking content)
            buffer.pop_front();
        }
        if buffer.len() == end_len && buffer.iter().eq(tag_end.iter()) {
            return end_len;
        }
    }
    0
}

/// Wrap an upstream OpenAI SSE stream to strip thinking content from each chunk.
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

                while let Some(idx) = buf.find("\n\n") {
                    let frame = buf[..idx].to_string();
                    buf.drain(..idx + 2);

                    if frame.trim().is_empty() {
                        continue;
                    }

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
                            out_lines.into_iter().collect::<Vec<_>>().join("\n") + "\n\n",
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

    #[test]
    fn strips_thinking_tags_with_content() {
        let input = "思绪this is thinking半数and this is real content";
        assert_eq!(strip_thinking_tags(input), "and this is real content");
    }

    #[test]
    fn strips_multiple_thinking_blocks() {
        let input = "思绪block1半数real1思绪block2半数real2";
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

    #[test]
    fn buffered_strips_reasoning_content_from_message() {
        let body = Bytes::from(
            r#"{"choices":[{"message":{"role":"assistant","content":"Hello","reasoning_content":"I should greet","reasoning_details":[{"text":"I should greet"}]}}]}"#,
        );
        let result = strip_thinking_buffered(&body).unwrap();
        let parsed: Value = serde_json::from_slice(&result).unwrap();
        let msg = &parsed["choices"][0]["message"];
        assert_eq!(msg["content"], "Hello");
        assert!(msg.get("reasoning_content").is_none());
        assert!(msg.get("reasoning_details").is_none());
    }

    #[test]
    fn buffered_strips_thinking_tags_from_content() {
        let body = Bytes::from(
            r#"{"choices":[{"message":{"role":"assistant","content":"思绪let me think半数Hello world"}}]}"#,
        );
        let result = strip_thinking_buffered(&body).unwrap();
        let parsed: Value = serde_json::from_slice(&result).unwrap();
        assert_eq!(parsed["choices"][0]["message"]["content"], "Hello world");
    }

    #[test]
    fn buffered_preserves_non_thinking_response_unchanged() {
        let body = Bytes::from(
            r#"{"choices":[{"message":{"role":"assistant","content":"Hello world"}}]}"#,
        );
        let result = strip_thinking_buffered(&body).unwrap();
        let parsed: Value = serde_json::from_slice(&result).unwrap();
        assert_eq!(parsed["choices"][0]["message"]["content"], "Hello world");
    }

    #[test]
    fn stream_strips_reasoning_from_sse_chunk() {
        use futures::stream;
        let chunks: Vec<Result<Bytes, Box<dyn std::error::Error + Send + Sync>>> = vec![
            Ok(Bytes::from("data: {\"choices\":[{\"delta\":{\"content\":\"Hi\",\"reasoning_content\":\"thinking...\",\"reasoning_details\":[{\"text\":\"thinking...\"}]}}]}\n\n")),
        ];
        let upstream: crate::application::ports::BoxedByteStream =
            Box::pin(stream::iter(chunks));
        let filtered = strip_thinking_stream(upstream);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let results: Vec<_> = rt.block_on(async { filtered.collect::<Vec<_>>().await });

        assert_eq!(results.len(), 1);
        let data_str = String::from_utf8_lossy(&results[0].as_ref().unwrap());
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
