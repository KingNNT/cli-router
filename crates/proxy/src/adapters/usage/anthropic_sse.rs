//! SSE event parser. Frames byte chunks into `event: ... \n data: ... \n\n` records,
//! exposed via a streaming API so the caller can feed bytes as they arrive.

use crate::domain::UsageState;
use serde::Deserialize;

#[derive(Deserialize)]
struct MessageStartEvent {
    message: MessageStartInner,
}

#[derive(Deserialize)]
struct MessageStartInner {
    usage: MessageStartUsage,
}

#[derive(Deserialize)]
struct MessageStartUsage {
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
    #[serde(default)]
    cache_creation_input_tokens: Option<u64>,
    #[serde(default)]
    cache_read_input_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct MessageDeltaEvent {
    #[serde(default)]
    usage: Option<MessageDeltaUsage>,
}

#[derive(Deserialize)]
struct MessageDeltaUsage {
    #[serde(default)]
    output_tokens: Option<u64>,
}

/// Anthropic-flavoured SSE parser. Maintains a partial-line buffer and decodes
/// `message_start` / `message_delta` events.
pub struct AnthropicSseParser {
    buf: Vec<u8>,
    pub state: UsageState,
}

impl Default for AnthropicSseParser {
    fn default() -> Self {
        Self::new()
    }
}

impl AnthropicSseParser {
    pub fn new() -> Self {
        Self {
            buf: Vec::with_capacity(4096),
            state: UsageState::default(),
        }
    }

    pub fn feed(&mut self, chunk: &[u8]) {
        self.buf.extend_from_slice(chunk);
        // Split on `\n\n` event boundary; keep tail.
        while let Some(pos) = find_double_newline(&self.buf) {
            let event_bytes: Vec<u8> = self.buf.drain(..pos + 2).collect();
            self.consume_event(&event_bytes[..event_bytes.len() - 2]);
        }
    }

    fn consume_event(&mut self, event_block: &[u8]) {
        // Each event has lines like `event: name` and `data: {...}` separated by \n.
        // We only care about the data line for usage extraction.
        let mut event_name: Option<&str> = None;
        let mut data_json: Option<&[u8]> = None;
        for line in event_block.split(|b| *b == b'\n') {
            if let Some(rest) = line.strip_prefix(b"event:") {
                event_name = std::str::from_utf8(trim_ascii(rest)).ok();
            } else if let Some(rest) = line.strip_prefix(b"data:") {
                data_json = Some(trim_ascii(rest));
            }
        }
        let (Some(name), Some(json)) = (event_name, data_json) else {
            return;
        };
        match name {
            "message_start" => self.consume_message_start(json),
            "message_delta" => self.consume_message_delta(json),
            _ => {}
        }
    }

    fn consume_message_start(&mut self, data: &[u8]) {
        let Ok(ev) = serde_json::from_slice::<MessageStartEvent>(data) else {
            return;
        };
        self.state.input_tokens = ev.message.usage.input_tokens;
        self.state.output_tokens = ev.message.usage.output_tokens;
        self.state.cache_creation_tokens = ev.message.usage.cache_creation_input_tokens;
        self.state.cache_read_tokens = ev.message.usage.cache_read_input_tokens;
    }

    fn consume_message_delta(&mut self, data: &[u8]) {
        let Ok(ev) = serde_json::from_slice::<MessageDeltaEvent>(data) else {
            return;
        };
        if let Some(u) = ev.usage
            && let Some(out) = u.output_tokens
        {
            self.state.output_tokens = Some(out);
        }
    }
}

fn find_double_newline(buf: &[u8]) -> Option<usize> {
    buf.windows(2).position(|w| w == b"\n\n")
}

fn trim_ascii(b: &[u8]) -> &[u8] {
    let start = b
        .iter()
        .position(|c| !c.is_ascii_whitespace())
        .unwrap_or(b.len());
    let end = b
        .iter()
        .rposition(|c| !c.is_ascii_whitespace())
        .map(|i| i + 1)
        .unwrap_or(start);
    &b[start..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    const STREAM: &[u8] = b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"x\",\"usage\":{\"input_tokens\":100,\"cache_creation_input_tokens\":50,\"cache_read_input_tokens\":25,\"output_tokens\":1}}}\n\nevent: ping\ndata: {\"type\":\"ping\"}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"text\":\"hi\"}}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":42}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";

    #[test]
    fn parses_full_stream_in_one_chunk() {
        let mut p = AnthropicSseParser::new();
        p.feed(STREAM);
        assert_eq!(p.state.input_tokens, Some(100));
        assert_eq!(p.state.output_tokens, Some(42));
        assert_eq!(p.state.cache_creation_tokens, Some(50));
        assert_eq!(p.state.cache_read_tokens, Some(25));
    }

    #[test]
    fn parses_stream_split_at_arbitrary_byte_boundaries() {
        let mut p = AnthropicSseParser::new();
        // Feed one byte at a time.
        for b in STREAM.iter() {
            p.feed(std::slice::from_ref(b));
        }
        assert_eq!(p.state.input_tokens, Some(100));
        assert_eq!(p.state.output_tokens, Some(42));
    }

    #[test]
    fn ignores_unknown_events() {
        let mut p = AnthropicSseParser::new();
        p.feed(b"event: unknown\ndata: {\"foo\":1}\n\n");
        assert_eq!(p.state, UsageState::default());
    }

    #[test]
    fn tolerates_missing_data_line() {
        let mut p = AnthropicSseParser::new();
        p.feed(b"event: message_start\n\n");
        assert_eq!(p.state, UsageState::default());
    }
}
