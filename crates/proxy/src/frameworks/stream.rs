//! Tees an upstream byte stream to a UsageParser while forwarding bytes downstream.
//!
//! The teed stream finalises usage on either path:
//!   * stream EOF — `on_finish(usage, true)`
//!   * the consumer drops the stream before EOF — `on_finish(usage, false)`
//!
//! The Drop guard is what lets the proxy mark a row `errored` with
//! `'client disconnected'` when axum drops the response body mid-flight.

use crate::application::ports::{BoxedError, UsageParser};
use crate::domain::UsageRecord;
use bytes::Bytes;
use futures::stream::Stream;
use std::pin::Pin;
use std::task::{Context, Poll};

type FinishCb = Box<dyn FnOnce(UsageRecord, bool) + Send>;

/// Wraps an upstream stream so each chunk is fed to a `UsageParser`. The
/// finalising callback fires exactly once — on EOF (`normal=true`) or on
/// Drop before EOF (`normal=false`).
pub struct TeedStream<S> {
    upstream: Pin<Box<S>>,
    parser: Option<Box<dyn UsageParser>>,
    on_finish: Option<FinishCb>,
    finished_normally: bool,
}

impl<S> TeedStream<S>
where
    S: Stream<Item = Result<Bytes, BoxedError>> + Send,
{
    /// Build a `TeedStream` that feeds each upstream chunk to `parser` and forwards
    /// it downstream unchanged. `on_finish` runs once when the stream terminates,
    /// receiving the parser-finalised `UsageRecord` and a `bool` that is `true` on
    /// stream EOF and `false` if the stream was dropped before EOF.
    pub fn new<F>(upstream: S, parser: Box<dyn UsageParser>, on_finish: F) -> Self
    where
        F: FnOnce(UsageRecord, bool) + Send + 'static,
    {
        Self {
            upstream: Box::pin(upstream),
            parser: Some(parser),
            on_finish: Some(Box::new(on_finish)),
            finished_normally: false,
        }
    }
}

impl<S> TeedStream<S> {
    fn fire_finish(&mut self, normal: bool) {
        if let (Some(parser), Some(cb)) = (self.parser.take(), self.on_finish.take()) {
            cb(parser.finish(), normal);
        }
    }
}

impl<S> Stream for TeedStream<S>
where
    S: Stream<Item = Result<Bytes, BoxedError>> + Send,
{
    type Item = Result<Bytes, BoxedError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.upstream.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(chunk))) => {
                if let Some(p) = self.parser.as_mut() {
                    p.feed(&chunk);
                }
                Poll::Ready(Some(Ok(chunk)))
            }
            Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(e))),
            Poll::Ready(None) => {
                self.finished_normally = true;
                self.fire_finish(true);
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<S> Drop for TeedStream<S> {
    fn drop(&mut self) {
        if !self.finished_normally {
            self.fire_finish(false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::usage::anthropic_sse::AnthropicSseParser;
    use futures::stream::StreamExt;
    use std::sync::{Arc, Mutex};

    struct TestParser(AnthropicSseParser);
    impl UsageParser for TestParser {
        fn feed(&mut self, chunk: &[u8]) {
            self.0.feed(chunk);
        }
        fn finish(self: Box<Self>) -> UsageRecord {
            UsageRecord::from(self.0.state)
        }
    }

    #[tokio::test]
    async fn forwards_chunks_and_finalises_usage() {
        let chunks: Vec<Result<Bytes, BoxedError>> = vec![
            Ok(Bytes::from_static(b"event: message_start\ndata: {\"message\":{\"usage\":{\"input_tokens\":11,\"output_tokens\":1}}}\n\n")),
            Ok(Bytes::from_static(b"event: message_delta\ndata: {\"usage\":{\"output_tokens\":22}}\n\n")),
        ];
        let upstream = futures::stream::iter(chunks);

        let captured: Arc<Mutex<Option<(UsageRecord, bool)>>> = Arc::new(Mutex::new(None));
        let captured_clone = captured.clone();

        let parser: Box<dyn UsageParser> = Box::new(TestParser(AnthropicSseParser::new()));
        let mut s = TeedStream::new(upstream, parser, move |rec, normal| {
            *captured_clone.lock().unwrap() = Some((rec, normal));
        });

        let mut forwarded = Vec::new();
        while let Some(item) = s.next().await {
            forwarded.extend_from_slice(&item.unwrap());
        }
        assert!(forwarded.starts_with(b"event: message_start"));
        let (rec, normal) = captured.lock().unwrap().clone().unwrap();
        assert_eq!(rec.input_tokens, Some(11));
        assert_eq!(rec.output_tokens, Some(22));
        assert!(normal, "callback should report normal termination on EOF");
    }

    #[tokio::test]
    async fn finalises_usage_when_dropped_before_eof() {
        // A stream that yields one chunk then hangs forever (never EOFs).
        let first: Vec<Result<Bytes, BoxedError>> = vec![Ok(Bytes::from_static(
            b"event: message_start\ndata: {\"message\":{\"usage\":{\"input_tokens\":11}}}\n\n",
        ))];
        let upstream = futures::stream::iter(first).chain(futures::stream::pending());

        let captured: Arc<Mutex<Option<(UsageRecord, bool)>>> = Arc::new(Mutex::new(None));
        let captured_clone = captured.clone();

        let parser: Box<dyn UsageParser> = Box::new(TestParser(AnthropicSseParser::new()));
        let mut s = TeedStream::new(upstream, parser, move |rec, normal| {
            *captured_clone.lock().unwrap() = Some((rec, normal));
        });

        // Read the first chunk, then drop the stream simulating a client disconnect.
        let _ = s.next().await.unwrap().unwrap();
        drop(s);

        let (rec, normal) = captured
            .lock()
            .unwrap()
            .clone()
            .expect("on_finish should have fired on Drop");
        assert_eq!(rec.input_tokens, Some(11));
        assert!(
            !normal,
            "callback should report abnormal termination on drop"
        );
    }
}
