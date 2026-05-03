//! Async stream wrappers that apply bidirectional translation on-the-fly.
//!
//! - `wrap_openai_to_anthropic`: upstream speaks OpenAI SSE → client expects Anthropic SSE.
//! - `wrap_anthropic_to_openai`: upstream speaks Anthropic SSE → client expects OpenAI chunks.

use crate::adapters::translation::anthropic_to_openai::stream::OpenAiToAnthropicStream;
use crate::adapters::translation::openai_to_anthropic::stream::AnthropicToOpenAiStream;
use crate::application::ports::{BoxedByteStream, BoxedError};
use bytes::Bytes;
use futures::StreamExt;

/// Wrap an upstream OpenAI SSE stream so the client receives Anthropic SSE events.
///
/// The wrapper buffers partial SSE frames (separated by `\n\n`), parses the
/// `data:` line, feeds each complete JSON chunk to `OpenAiToAnthropicStream`,
/// and emits the translated Anthropic event strings.
pub fn wrap_openai_to_anthropic(upstream: BoxedByteStream) -> BoxedByteStream {
    let mut fsm = OpenAiToAnthropicStream::new();
    let mut buf = String::new();
    let mut done = false;

    let translated = upstream.flat_map(move |chunk_result| {
        let mut emit: Vec<Result<Bytes, BoxedError>> = Vec::new();
        if done {
            return futures::stream::iter(emit);
        }
        match chunk_result {
            Err(e) => {
                emit.push(Err(e));
            }
            Ok(chunk_bytes) => {
                buf.push_str(&String::from_utf8_lossy(&chunk_bytes));

                // Drain complete SSE frames (terminated by "\n\n").
                while let Some(idx) = buf.find("\n\n") {
                    let frame = buf[..idx].to_string();
                    buf.drain(..idx + 2);

                    // Skip empty frames.
                    if frame.trim().is_empty() {
                        continue;
                    }

                    // Find the `data:` line.
                    let data_line = frame
                        .lines()
                        .find_map(|l| l.strip_prefix("data: ").map(str::trim));
                    let Some(data) = data_line else { continue };

                    if data == "[DONE]" {
                        for s in fsm.finish() {
                            emit.push(Ok(Bytes::from(s.into_bytes())));
                        }
                        done = true;
                        break;
                    }

                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(data) {
                        for s in fsm.feed_chunk(&value) {
                            emit.push(Ok(Bytes::from(s.into_bytes())));
                        }
                    }
                }
            }
        }
        futures::stream::iter(emit)
    });

    Box::pin(translated)
}

/// Wrap an upstream Anthropic SSE stream so the client receives OpenAI chat.completion.chunk SSE.
///
/// The wrapper buffers partial SSE frames (separated by `\n\n`), parses the
/// `event:` and `data:` lines, feeds each event to `AnthropicToOpenAiStream`,
/// and emits the translated OpenAI `data: {...}\n\n` strings.
pub fn wrap_anthropic_to_openai(upstream: BoxedByteStream) -> BoxedByteStream {
    let mut fsm = AnthropicToOpenAiStream::new();
    let mut buf = String::new();
    let mut done = false;

    let translated = upstream.flat_map(move |chunk_result| {
        let mut emit: Vec<Result<Bytes, BoxedError>> = Vec::new();
        if done {
            return futures::stream::iter(emit);
        }
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

                    // Parse event name and data from frame lines.
                    let mut event_name = String::new();
                    let mut data_str = String::new();
                    for line in frame.lines() {
                        if let Some(ev) = line.strip_prefix("event: ") {
                            event_name = ev.trim().to_string();
                        } else if let Some(d) = line.strip_prefix("data: ") {
                            data_str = d.trim().to_string();
                        }
                    }

                    if event_name.is_empty() || data_str.is_empty() {
                        continue;
                    }

                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&data_str) {
                        let chunks = fsm.feed_event(&event_name, &value);
                        for s in chunks {
                            if s == "data: [DONE]\n\n" {
                                done = true;
                            }
                            emit.push(Ok(Bytes::from(s.into_bytes())));
                        }
                    }

                    if done {
                        break;
                    }
                }
            }
        }
        futures::stream::iter(emit)
    });

    Box::pin(translated)
}
