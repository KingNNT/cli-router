//! Async stream wrappers that apply bidirectional translation on-the-fly.
//!
//! - `wrap_openai_to_anthropic`: upstream speaks OpenAI SSE → client expects Anthropic SSE.
//! - `wrap_anthropic_to_openai`: upstream speaks Anthropic SSE → client expects OpenAI chunks.

use crate::adapters::translation::anthropic_to_openai::stream::OpenAiToAnthropicStream;
use crate::adapters::translation::openai_to_anthropic::stream::AnthropicToOpenAiStream;
use crate::application::ports::{BoxedByteStream, BoxedError};
use bytes::Bytes;
use futures::StreamExt;

/// Find the byte offset of the first `\n\n` SSE frame terminator in `buf`.
fn find_frame_end(buf: &[u8]) -> Option<usize> {
    buf.windows(2).position(|w| w == b"\n\n")
}

/// Wrap an upstream OpenAI SSE stream so the client receives Anthropic SSE events.
///
/// The wrapper buffers partial SSE frames (separated by `\n\n`), parses the
/// `data:` line, feeds each complete JSON chunk to `OpenAiToAnthropicStream`,
/// and emits the translated Anthropic event strings.
pub fn wrap_openai_to_anthropic(upstream: BoxedByteStream) -> BoxedByteStream {
    let mut fsm = OpenAiToAnthropicStream::new();
    // Byte buffer, not String: an upstream chunk boundary can fall inside a
    // multi-byte UTF-8 character, so we must not lossy-decode partial bytes.
    let mut buf: Vec<u8> = Vec::new();
    let mut done = false;

    // Append a synthetic end-of-stream marker so we can flush `fsm.finish()`
    // even when the upstream closes without a `data: [DONE]` sentinel — MiniMax
    // terminates with a `finish_reason` chunk and simply closes the connection.
    let terminated = upstream
        .map(Some)
        .chain(futures::stream::once(async { None }));

    let translated = terminated.flat_map(move |maybe_chunk| {
        let mut emit: Vec<Result<Bytes, BoxedError>> = Vec::new();
        if done {
            return futures::stream::iter(emit);
        }
        match maybe_chunk {
            // Upstream ended: flush any pending terminator events.
            None => {
                for s in fsm.finish() {
                    emit.push(Ok(Bytes::from(s.into_bytes())));
                }
                done = true;
            }
            Some(Err(e)) => {
                emit.push(Err(e));
            }
            Some(Ok(chunk_bytes)) => {
                buf.extend_from_slice(&chunk_bytes);

                // Drain complete SSE frames (terminated by "\n\n"). A frame is
                // whole, so decoding it is UTF-8-safe (a char can't straddle the
                // `\n\n` delimiter — 0x0A never appears mid-character).
                while let Some(idx) = find_frame_end(&buf) {
                    let frame_bytes: Vec<u8> = buf.drain(..idx + 2).collect();
                    let frame = String::from_utf8_lossy(&frame_bytes[..idx]);

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

                    match serde_json::from_str::<serde_json::Value>(data) {
                        Ok(value) => {
                            for s in fsm.feed_chunk(&value) {
                                emit.push(Ok(Bytes::from(s.into_bytes())));
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                target: "translation",
                                error = %e,
                                data = %data,
                                "failed to parse SSE chunk; dropping"
                            );
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
    // Byte buffer, not String: guard against a multi-byte UTF-8 character being
    // split across upstream chunk boundaries.
    let mut buf: Vec<u8> = Vec::new();
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
                buf.extend_from_slice(&chunk_bytes);

                // Drain complete SSE frames.
                while let Some(idx) = find_frame_end(&buf) {
                    let frame_bytes: Vec<u8> = buf.drain(..idx + 2).collect();
                    let frame = String::from_utf8_lossy(&frame_bytes[..idx]);

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

                    match serde_json::from_str::<serde_json::Value>(&data_str) {
                        Ok(value) => {
                            let chunks = fsm.feed_event(&event_name, &value);
                            for s in chunks {
                                if s == "data: [DONE]\n\n" {
                                    done = true;
                                }
                                emit.push(Ok(Bytes::from(s.into_bytes())));
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                target: "translation",
                                error = %e,
                                data = %data_str,
                                "failed to parse SSE chunk; dropping"
                            );
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

#[cfg(test)]
mod done_sentinel_tests {
    use super::*;

    fn collect(chunks: Vec<&'static str>) -> String {
        let items: Vec<Result<Bytes, BoxedError>> =
            chunks.into_iter().map(|s| Ok(Bytes::from(s))).collect();
        let upstream: BoxedByteStream = Box::pin(futures::stream::iter(items));
        let wrapped = wrap_openai_to_anthropic(upstream);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let out: Vec<_> = rt.block_on(async { wrapped.collect::<Vec<_>>().await });
        out.into_iter()
            .filter_map(|r| r.ok())
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .collect()
    }

    /// MiniMax terminates its OpenAI SSE stream with a `finish_reason` chunk and
    /// then simply closes the connection — it never sends `data: [DONE]`.
    #[test]
    fn stream_without_done_sentinel_still_terminates() {
        let s = collect(vec![
            "data: {\"id\":\"abc\",\"model\":\"MiniMax-M2\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hello\"}}]}\n\n",
            "data: {\"id\":\"abc\",\"model\":\"MiniMax-M2\",\"choices\":[{\"finish_reason\":\"stop\",\"index\":0,\"delta\":{\"content\":\" world\"}}]}\n\n",
        ]);

        assert!(s.contains("Hello"), "text missing: {s}");
        assert!(s.contains(" world"), "tail text missing: {s}");
        assert!(
            s.contains("content_block_stop"),
            "no content_block_stop: {s}"
        );
        assert!(s.contains("message_delta"), "no message_delta: {s}");
        assert!(s.contains("message_stop"), "no message_stop: {s}");
    }

    /// A TCP/reqwest chunk boundary can fall inside a multi-byte UTF-8 character.
    /// The wrapper must not destroy that character.
    #[test]
    fn split_multibyte_char_across_chunks_is_not_corrupted() {
        let frame = "data: {\"id\":\"a\",\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hà Nội\"}}]}\n\ndata: [DONE]\n\n";
        let bytes = frame.as_bytes();
        // Split inside the 2-byte 'à' (0xC3 0xA0) which follows "...content\":\"H".
        let cut = frame.find('à').unwrap() + 1;
        let items: Vec<Result<Bytes, BoxedError>> = vec![
            Ok(Bytes::copy_from_slice(&bytes[..cut])),
            Ok(Bytes::copy_from_slice(&bytes[cut..])),
        ];
        let upstream: BoxedByteStream = Box::pin(futures::stream::iter(items));
        let wrapped = wrap_openai_to_anthropic(upstream);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let out: Vec<_> = rt.block_on(async { wrapped.collect::<Vec<_>>().await });
        let s: String = out
            .into_iter()
            .filter_map(|r| r.ok())
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .collect();

        assert!(!s.contains('\u{FFFD}'), "character destroyed: {s}");
        assert!(s.contains("Hà Nội"), "text corrupted: {s}");
    }

    /// Control: with `[DONE]` present the terminator events are emitted today.
    #[test]
    fn stream_with_done_sentinel_terminates() {
        let s = collect(vec![
            "data: {\"id\":\"abc\",\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hi\"}}]}\n\n",
            "data: [DONE]\n\n",
        ]);
        assert!(s.contains("message_stop"), "no message_stop: {s}");
    }
}
