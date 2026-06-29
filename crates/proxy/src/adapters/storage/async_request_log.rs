//! Non-blocking `RequestLogPort` that moves all request-log writes off the
//! request hot path.
//!
//! The previous design called `rusqlite` directly inside the async request
//! handler behind a shared `Mutex<Connection>` — so every request serialized
//! on one lock and paid an `INSERT`/`UPDATE` *before* the upstream call could
//! even start (`insert_started`) and again on completion. Under concurrency
//! that lock is the proxy's single biggest serialization point.
//!
//! `AsyncRequestLog` replaces that with a bounded channel: the hot-path
//! methods only `try_send` a command (no lock, no DB I/O) and a single drain
//! task owns the underlying writer and applies the writes serially. Ordering
//! is preserved because the channel is FIFO and there is exactly one consumer,
//! so a `complete`/`fail` is always applied after its `insert_started`.
//!
//! Logging is best-effort: if the queue is full the event is dropped with a
//! warning rather than blocking or failing the request. A dropped `started`
//! simply means the later `complete` updates no row — the request still
//! succeeds.

use crate::application::errors::ProxyError;
use crate::application::ports::RequestLogPort;
use crate::domain::{RequestStart, RequestUsage};
use std::sync::Arc;
use tokio::sync::mpsc;

/// One queued request-log mutation.
#[derive(Debug)]
pub enum LogCommand {
    Started(RequestStart),
    Complete {
        id: String,
        finished_at: i64,
        usage: RequestUsage,
    },
    Fail {
        id: String,
        finished_at: i64,
        error_message: String,
        usage: RequestUsage,
    },
}

/// Channel-backed `RequestLogPort`. Cheap to clone-wrap in an `Arc`.
pub struct AsyncRequestLog {
    tx: mpsc::Sender<LogCommand>,
    /// Underlying synchronous writer, retained only for `sweep_stale` (a rare,
    /// off-hot-path maintenance write invoked by the background sweeper).
    inner: Arc<dyn RequestLogPort>,
}

impl AsyncRequestLog {
    /// Build the wrapper and hand back the receiver without spawning a drain
    /// task. Used by tests to drive draining deterministically.
    pub fn new(
        inner: Arc<dyn RequestLogPort>,
        capacity: usize,
    ) -> (Self, mpsc::Receiver<LogCommand>) {
        let (tx, rx) = mpsc::channel(capacity);
        (Self { tx, inner }, rx)
    }

    /// Build the wrapper and spawn the drain task that applies queued writes to
    /// `inner` until the channel closes (all senders dropped).
    pub fn spawn(inner: Arc<dyn RequestLogPort>, capacity: usize) -> Self {
        let (me, rx) = Self::new(inner.clone(), capacity);
        tokio::spawn(run_drain(rx, inner));
        me
    }

    fn enqueue(&self, cmd: LogCommand) {
        if let Err(e) = self.tx.try_send(cmd) {
            // Full or closed: drop the event. Never block or fail the request.
            tracing::warn!(error = %e, "request-log queue unavailable; dropping log event");
        }
    }
}

/// Drain loop: apply each queued command to the underlying writer. Errors are
/// logged but never propagated — the client has already been served.
pub async fn run_drain(mut rx: mpsc::Receiver<LogCommand>, inner: Arc<dyn RequestLogPort>) {
    while let Some(cmd) = rx.recv().await {
        let result = match cmd {
            LogCommand::Started(start) => inner.insert_started(&start),
            LogCommand::Complete {
                id,
                finished_at,
                usage,
            } => inner.complete(&id, finished_at, &usage),
            LogCommand::Fail {
                id,
                finished_at,
                error_message,
                usage,
            } => inner.fail(&id, finished_at, &error_message, &usage),
        };
        if let Err(e) = result {
            tracing::error!(error = %e, "failed to persist queued request-log event");
        }
    }
}

impl RequestLogPort for AsyncRequestLog {
    fn insert_started(&self, start: &RequestStart) -> Result<(), ProxyError> {
        self.enqueue(LogCommand::Started(start.clone()));
        Ok(())
    }

    fn complete(&self, id: &str, finished_at: i64, usage: &RequestUsage) -> Result<(), ProxyError> {
        self.enqueue(LogCommand::Complete {
            id: id.to_string(),
            finished_at,
            usage: usage.clone(),
        });
        Ok(())
    }

    fn fail(
        &self,
        id: &str,
        finished_at: i64,
        error_message: &str,
        usage: &RequestUsage,
    ) -> Result<(), ProxyError> {
        self.enqueue(LogCommand::Fail {
            id: id.to_string(),
            finished_at,
            error_message: error_message.to_string(),
            usage: usage.clone(),
        });
        Ok(())
    }

    fn sweep_stale(&self, cutoff_ms: i64) -> Result<u64, ProxyError> {
        // Off the hot path; delegate straight through to the real writer.
        self.inner.sweep_stale(cutoff_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::storage::{SqliteRequestLogRepository, ensure_current};
    use crate::application::ports::RequestLogReadPort;
    use rusqlite::Connection;
    use std::sync::Mutex;

    fn repo() -> SqliteRequestLogRepository {
        let conn = Connection::open_in_memory().unwrap();
        ensure_current(&conn).unwrap();
        SqliteRequestLogRepository::new(Arc::new(Mutex::new(conn)))
    }

    fn start(id: &str) -> RequestStart {
        RequestStart {
            id: id.to_string(),
            user_id: 1,
            provider: "anthropic".into(),
            model: "claude-x".into(),
            started_at: 1_000,
        }
    }

    #[tokio::test]
    async fn drains_started_then_complete_to_underlying_writer() {
        let repo = Arc::new(repo());
        let writer: Arc<dyn RequestLogPort> = repo.clone();
        let (log, rx) = AsyncRequestLog::new(writer.clone(), 100);

        log.insert_started(&start("req-1")).unwrap();
        let usage = RequestUsage {
            output_tokens: Some(42),
            ..Default::default()
        };
        log.complete("req-1", 2_000, &usage).unwrap();

        drop(log); // close the channel so the drain loop terminates
        run_drain(rx, writer).await;

        let by_status = repo.count_by_status().unwrap();
        assert_eq!(by_status.get("completed").copied(), Some(1));
    }

    #[tokio::test]
    async fn full_queue_drops_event_without_error() {
        let writer: Arc<dyn RequestLogPort> = Arc::new(repo());
        // Capacity 1, no drain task: the second send finds the queue full.
        let (log, _rx) = AsyncRequestLog::new(writer, 1);

        assert!(log.insert_started(&start("a")).is_ok()); // queued
        assert!(log.insert_started(&start("b")).is_ok()); // dropped, still Ok
    }
}
