//! SSE event parser for OpenAI-format streams. Frames byte chunks line-by-line
//! and pulls the `usage` block out of any `data: {...}` JSON payload that
//! carries one. Last `usage` wins, matching OpenAI's convention of putting the
//! full counts on the final chunk when `stream_options: {"include_usage": true}`
//! is set on the request.

use crate::domain::UsageRecord;
use serde::Deserialize;

#[derive(Deserialize)]
struct ChunkEnvelope {
    #[serde(default)]
    usage: Option<ChunkUsage>,
}

#[derive(Deserialize)]
struct ChunkUsage {
    #[serde(default)]
    prompt_tokens: Option<u64>,
    #[serde(default)]
    completion_tokens: Option<u64>,
    #[serde(default)]
    prompt_tokens_details: Option<PromptTokensDetails>,
}

#[derive(Deserialize)]
struct PromptTokensDetails {
    #[serde(default)]
    cached_tokens: Option<u64>,
}

/// Streaming usage state captured from an OpenAI SSE response.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct OpenAiSseState {
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub cached_tokens: Option<u64>,
}

impl From<OpenAiSseState> for UsageRecord {
    fn from(s: OpenAiSseState) -> Self {
        UsageRecord {
            input_tokens: s.prompt_tokens,
            output_tokens: s.completion_tokens,
            cache_read_tokens: s.cached_tokens,
            cache_creation_tokens: None,
        }
    }
}

/// OpenAI-flavoured SSE parser. Splits incoming bytes into lines and extracts
/// the `usage` block from each `data: {...}` payload. `[DONE]` sentinels and
/// malformed lines are skipped.
pub struct OpenAiSseParser {
    buf: Vec<u8>,
    pub state: OpenAiSseState,
}

impl Default for OpenAiSseParser {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenAiSseParser {
    pub fn new() -> Self {
        Self {
            buf: Vec::with_capacity(4096),
            state: OpenAiSseState::default(),
        }
    }

    pub fn feed(&mut self, chunk: &[u8]) {
        self.buf.extend_from_slice(chunk);
        // Split on `\n` boundaries; keep the trailing partial line in `buf`.
        while let Some(pos) = self.buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..pos + 1).collect();
            // Strip the trailing newline (and any \r before it).
            let mut end = line.len() - 1;
            if end > 0 && line[end - 1] == b'\r' {
                end -= 1;
            }
            self.consume_line(&line[..end]);
        }
    }

    pub fn state(&self) -> &OpenAiSseState {
        &self.state
    }

    fn consume_line(&mut self, line: &[u8]) {
        let Some(rest) = line.strip_prefix(b"data:") else {
            return;
        };
        let payload = trim_ascii(rest);
        if payload.is_empty() {
            return;
        }
        if payload == b"[DONE]" {
            return;
        }
        let Ok(env) = serde_json::from_slice::<ChunkEnvelope>(payload) else {
            return;
        };
        let Some(u) = env.usage else {
            return;
        };
        // Last chunk to carry usage wins. Only overwrite when present so a
        // partial chunk (rare) doesn't clobber a previous full reading.
        if let Some(p) = u.prompt_tokens {
            self.state.prompt_tokens = Some(p);
        }
        if let Some(c) = u.completion_tokens {
            self.state.completion_tokens = Some(c);
        }
        if let Some(d) = u.prompt_tokens_details
            && let Some(cached) = d.cached_tokens
        {
            self.state.cached_tokens = Some(cached);
        }
    }
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

    #[test]
    fn captures_usage_from_final_chunk() {
        let stream = b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\
                       data: {\"choices\":[],\"usage\":{\"prompt_tokens\":6,\"completion_tokens\":10,\"total_tokens\":16,\"prompt_tokens_details\":{\"cached_tokens\":3}}}\n\
                       data: [DONE]\n";
        let mut p = OpenAiSseParser::new();
        p.feed(stream);
        assert_eq!(p.state().prompt_tokens, Some(6));
        assert_eq!(p.state().completion_tokens, Some(10));
        assert_eq!(p.state().cached_tokens, Some(3));
    }

    #[test]
    fn stream_without_usage_yields_all_none() {
        let stream = b"data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\
                       data: {\"choices\":[{\"delta\":{\"content\":\" world\"}}]}\n\
                       data: [DONE]\n";
        let mut p = OpenAiSseParser::new();
        p.feed(stream);
        assert_eq!(p.state(), &OpenAiSseState::default());
    }

    #[test]
    fn last_usage_chunk_wins() {
        let stream = b"data: {\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2}}\n\
                       data: {\"usage\":{\"prompt_tokens\":100,\"completion_tokens\":200,\"prompt_tokens_details\":{\"cached_tokens\":7}}}\n";
        let mut p = OpenAiSseParser::new();
        p.feed(stream);
        assert_eq!(p.state().prompt_tokens, Some(100));
        assert_eq!(p.state().completion_tokens, Some(200));
        assert_eq!(p.state().cached_tokens, Some(7));
    }

    #[test]
    fn malformed_data_line_is_skipped() {
        let stream = b"data: not-json\n\
                       data: {\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":6}}\n";
        let mut p = OpenAiSseParser::new();
        p.feed(stream);
        assert_eq!(p.state().prompt_tokens, Some(5));
        assert_eq!(p.state().completion_tokens, Some(6));
    }

    #[test]
    fn done_sentinel_is_handled_cleanly() {
        let mut p = OpenAiSseParser::new();
        p.feed(b"data: [DONE]\n");
        assert_eq!(p.state(), &OpenAiSseState::default());
    }

    #[test]
    fn splits_at_arbitrary_byte_boundaries() {
        let stream = b"data: {\"usage\":{\"prompt_tokens\":11,\"completion_tokens\":22,\"prompt_tokens_details\":{\"cached_tokens\":4}}}\ndata: [DONE]\n";
        let mut p = OpenAiSseParser::new();
        for b in stream.iter() {
            p.feed(std::slice::from_ref(b));
        }
        assert_eq!(p.state().prompt_tokens, Some(11));
        assert_eq!(p.state().completion_tokens, Some(22));
        assert_eq!(p.state().cached_tokens, Some(4));
    }

    #[test]
    fn handles_crlf_line_endings() {
        let stream =
            b"data: {\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":4}}\r\ndata: [DONE]\r\n";
        let mut p = OpenAiSseParser::new();
        p.feed(stream);
        assert_eq!(p.state().prompt_tokens, Some(3));
        assert_eq!(p.state().completion_tokens, Some(4));
    }

    #[test]
    fn from_state_into_usage_record_maps_correctly() {
        let state = OpenAiSseState {
            prompt_tokens: Some(10),
            completion_tokens: Some(20),
            cached_tokens: Some(5),
        };
        let rec: UsageRecord = state.into();
        assert_eq!(rec.input_tokens, Some(10));
        assert_eq!(rec.output_tokens, Some(20));
        assert_eq!(rec.cache_read_tokens, Some(5));
        assert_eq!(rec.cache_creation_tokens, None);
    }
}
