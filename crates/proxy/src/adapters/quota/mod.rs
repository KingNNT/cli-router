//! InMemoryQuota — QuotaPort backed by in-process ring buffers / calendar
//! counters. Seeded from the SQLite request log on startup so counters
//! survive proxy restarts (within the window).

use crate::application::ports::{QuotaSeedRow, QuotaPort};
use crate::domain::quota::{
    CalendarCounter, CalendarUnit, QuotaCheck, QuotaConfig, QuotaWindow, RingBuffer, evaluate,
};
use crate::domain::RequestUsage;
use std::sync::Mutex;

fn now_epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

enum Counter {
    Rolling(RingBuffer),
    Calendar { counter: CalendarCounter, unit: CalendarUnit },
}

impl Counter {
    fn new(window: QuotaWindow) -> Self {
        match window {
            QuotaWindow::Rolling { duration_ms } => Counter::Rolling(RingBuffer::new(duration_ms)),
            QuotaWindow::Calendar { unit } => Counter::Calendar {
                counter: CalendarCounter::new(unit, now_epoch_ms()),
                unit,
            },
        }
    }

    fn add(&mut self, now_ms: u64, requests: u64, input_tokens: u64, output_tokens: u64) {
        match self {
            Counter::Rolling(rb) => rb.add(now_ms, requests, input_tokens, output_tokens),
            Counter::Calendar { counter, unit } => {
                counter.add(now_ms, *unit, requests, input_tokens, output_tokens)
            }
        }
    }

    fn totals(&self, now_ms: u64) -> crate::domain::quota::Totals {
        match self {
            Counter::Rolling(rb) => rb.totals(now_ms),
            Counter::Calendar { counter, unit } => counter.totals(now_ms, *unit),
        }
    }

    fn next_boundary_ms(&self, now_ms: u64) -> u64 {
        match self {
            Counter::Rolling(rb) => rb.next_boundary_ms(now_ms),
            Counter::Calendar { counter, .. } => counter.next_boundary_ms(now_ms),
        }
    }
}

struct Entry {
    config: QuotaConfig,
    counter: Counter,
}

impl Entry {
    fn new(config: QuotaConfig) -> Self {
        let window = config.window;
        Self { config, counter: Counter::new(window) }
    }

    fn add(&mut self, now_ms: u64, requests: u64, input_tokens: u64, output_tokens: u64) {
        self.counter.add(now_ms, requests, input_tokens, output_tokens);
    }
}

pub struct InMemoryQuota {
    entries: Mutex<Vec<Entry>>,
}

/// Point-in-time snapshot of one configured quota.
#[derive(Debug, Clone)]
pub struct QuotaSnapshot {
    pub config: QuotaConfig,
    pub totals: crate::domain::quota::Totals,
    pub next_boundary_ms: u64,
}

impl InMemoryQuota {
    pub fn new(configs: Vec<QuotaConfig>) -> Self {
        let entries = configs.into_iter().map(Entry::new).collect();
        Self { entries: Mutex::new(entries) }
    }

    /// Returns a point-in-time snapshot of all configured quotas and their
    /// current counters. Used by the admin endpoint to surface live status.
    pub fn snapshot(&self) -> Vec<QuotaSnapshot> {
        let now_ms = now_epoch_ms();
        let entries = self.entries.lock().expect("quota mutex poisoned");
        entries
            .iter()
            .map(|e| QuotaSnapshot {
                config: e.config.clone(),
                totals: e.counter.totals(now_ms),
                next_boundary_ms: e.counter.next_boundary_ms(now_ms),
            })
            .collect()
    }

    /// Replay historical rows from the request log into the counters. Call
    /// once at startup after constructing the adapter.
    pub fn seed(&self, rows: Vec<QuotaSeedRow>) {
        let mut entries = self.entries.lock().expect("quota mutex poisoned");
        for row in rows {
            let now_ms = row.started_at_ms.max(0) as u64;
            let input = row.input_tokens.unwrap_or(0);
            let output = row.output_tokens.unwrap_or(0);
            for entry in entries.iter_mut() {
                if entry.config.provider == row.provider {
                    entry.add(now_ms, 1, input, output);
                }
            }
        }
    }
}

impl QuotaPort for InMemoryQuota {
    fn check(&self, provider: &str) -> QuotaCheck {
        let now_ms = now_epoch_ms();
        let entries = self.entries.lock().expect("quota mutex poisoned");
        for entry in entries.iter() {
            if entry.config.provider == provider {
                let totals = entry.counter.totals(now_ms);
                let next_boundary = entry.counter.next_boundary_ms(now_ms);
                return evaluate(&entry.config, totals, next_boundary);
            }
        }
        QuotaCheck::Ok
    }

    fn record(&self, provider: &str, usage: &RequestUsage) {
        let now_ms = now_epoch_ms();
        let mut entries = self.entries.lock().expect("quota mutex poisoned");
        for entry in entries.iter_mut() {
            if entry.config.provider == provider {
                let input = usage.input_tokens.unwrap_or(0);
                let output = usage.output_tokens.unwrap_or(0);
                entry.add(now_ms, 1, input, output);
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::quota::{QuotaCheck, QuotaWindow};

    fn cfg(provider: &str, max_requests: u64) -> QuotaConfig {
        QuotaConfig {
            provider: provider.into(),
            window: QuotaWindow::Rolling { duration_ms: 60 * 60 * 1_000 }, // 1h
            max_requests: Some(max_requests),
            max_input_tokens: None,
            max_output_tokens: None,
            warn_pct: 80,
        }
    }

    fn usage(input: u64, output: u64) -> RequestUsage {
        RequestUsage {
            input_tokens: Some(input),
            output_tokens: Some(output),
            ..Default::default()
        }
    }

    #[test]
    fn check_returns_ok_when_provider_unconfigured() {
        let quota = InMemoryQuota::new(vec![cfg("anthropic", 100)]);
        assert_eq!(quota.check("zai"), QuotaCheck::Ok);
    }

    #[test]
    fn record_increments_counter_for_configured_provider() {
        let quota = InMemoryQuota::new(vec![cfg("anthropic", 100)]);
        // First record does not trip warn (80 out of 100) yet
        for _ in 0..5 {
            quota.record("anthropic", &usage(10, 5));
        }
        // 5 requests → 5%, well below 80% warn threshold
        assert_eq!(quota.check("anthropic"), QuotaCheck::Ok);
    }

    #[test]
    fn record_ignores_unconfigured_provider() {
        let quota = InMemoryQuota::new(vec![cfg("anthropic", 2)]);
        // Recording for an unknown provider must not panic or affect anthropic.
        quota.record("zai", &usage(1_000_000, 1_000_000));
        assert_eq!(quota.check("anthropic"), QuotaCheck::Ok);
    }

    #[test]
    fn seed_replays_history() {
        let quota = InMemoryQuota::new(vec![cfg("anthropic", 10)]);
        // Seed 8 rows (80% of 10) — should tip the counter into Warn territory.
        let now_ms = now_epoch_ms();
        let rows: Vec<QuotaSeedRow> = (0..8)
            .map(|_| QuotaSeedRow {
                provider: "anthropic".into(),
                started_at_ms: now_ms as i64,
                input_tokens: Some(10),
                output_tokens: Some(5),
            })
            .collect();
        quota.seed(rows);
        let result = quota.check("anthropic");
        assert!(
            matches!(result, QuotaCheck::Warn { .. }),
            "expected Warn after seeding 80%, got {result:?}"
        );
    }

    #[test]
    fn check_rejects_at_max() {
        let quota = InMemoryQuota::new(vec![cfg("anthropic", 3)]);
        quota.record("anthropic", &usage(10, 5));
        quota.record("anthropic", &usage(10, 5));
        quota.record("anthropic", &usage(10, 5));
        let result = quota.check("anthropic");
        assert!(
            matches!(result, QuotaCheck::Reject { metric: "requests", .. }),
            "expected Reject at max, got {result:?}"
        );
    }
}
