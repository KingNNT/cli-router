//! Quota domain types and the window-string parser.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalendarUnit {
    Minute,
    Hour,
    Day,
}

impl CalendarUnit {
    pub fn duration_ms(self) -> u64 {
        match self {
            CalendarUnit::Minute => 60 * 1_000,
            CalendarUnit::Hour => 60 * 60 * 1_000,
            CalendarUnit::Day => 24 * 60 * 60 * 1_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaWindow {
    Rolling { duration_ms: u64 },
    Calendar { unit: CalendarUnit },
}

impl QuotaWindow {
    pub fn duration_ms(&self) -> u64 {
        match *self {
            QuotaWindow::Rolling { duration_ms } => duration_ms,
            QuotaWindow::Calendar { unit } => unit.duration_ms(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct QuotaConfig {
    pub provider: String,
    pub window: QuotaWindow,
    pub max_requests: Option<u64>,
    pub max_input_tokens: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub warn_pct: u8, // 0..=100
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuotaCheck {
    Ok,
    Warn {
        metric: &'static str,
        used_pct: u8,
    },
    Reject {
        metric: &'static str,
        retry_after_ms: u64,
    },
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WindowParseError {
    #[error("missing colon: window must be like 'rolling:5h' or 'calendar:day'")]
    MissingColon,
    #[error("unknown window kind '{0}': expected 'rolling' or 'calendar'")]
    UnknownKind(String),
    #[error("invalid duration '{0}': accepted suffixes are s, m, h, d (e.g. '5h')")]
    InvalidDuration(String),
    #[error("invalid calendar unit '{0}': accepted are 'minute', 'hour', 'day'")]
    InvalidUnit(String),
}

pub fn parse_window(s: &str) -> Result<QuotaWindow, WindowParseError> {
    let (kind, rest) = s.split_once(':').ok_or(WindowParseError::MissingColon)?;
    match kind {
        "rolling" => parse_duration(rest)
            .ok_or_else(|| WindowParseError::InvalidDuration(rest.into()))
            .map(|duration_ms| QuotaWindow::Rolling { duration_ms }),
        "calendar" => match rest {
            "minute" => Ok(QuotaWindow::Calendar {
                unit: CalendarUnit::Minute,
            }),
            "hour" => Ok(QuotaWindow::Calendar {
                unit: CalendarUnit::Hour,
            }),
            "day" => Ok(QuotaWindow::Calendar {
                unit: CalendarUnit::Day,
            }),
            other => Err(WindowParseError::InvalidUnit(other.into())),
        },
        other => Err(WindowParseError::UnknownKind(other.into())),
    }
}

fn parse_duration(s: &str) -> Option<u64> {
    if s.len() < 2 {
        return None;
    }
    let (num, suffix) = s.split_at(s.len() - 1);
    let n: u64 = num.parse().ok()?;
    let mult_ms: u64 = match suffix {
        "s" => 1_000,
        "m" => 60 * 1_000,
        "h" => 60 * 60 * 1_000,
        "d" => 24 * 60 * 60 * 1_000,
        _ => return None,
    };
    n.checked_mul(mult_ms)
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Bucket {
    pub bucket_idx_ms: u64, // floor(timestamp / 60_000) * 60_000
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    initialized: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Totals {
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

const BUCKET_GRANULARITY_MS: u64 = 60_000;

#[derive(Debug)]
pub struct RingBuffer {
    duration_ms: u64,
    buckets: Vec<Bucket>,
}

impl RingBuffer {
    pub fn new(duration_ms: u64) -> Self {
        let n = duration_ms.div_ceil(BUCKET_GRANULARITY_MS) as usize;
        let n = n.max(1);
        Self {
            duration_ms,
            buckets: vec![Bucket::default(); n],
        }
    }

    pub fn add(&mut self, now_ms: u64, requests: u64, input_tokens: u64, output_tokens: u64) {
        let bidx = (now_ms / BUCKET_GRANULARITY_MS) * BUCKET_GRANULARITY_MS;
        let slot = ((now_ms / BUCKET_GRANULARITY_MS) as usize) % self.buckets.len();
        let b = &mut self.buckets[slot];
        if !b.initialized || b.bucket_idx_ms != bidx {
            *b = Bucket::default();
            b.bucket_idx_ms = bidx;
            b.initialized = true;
        }
        b.requests += requests;
        b.input_tokens += input_tokens;
        b.output_tokens += output_tokens;
    }

    pub fn totals(&self, now_ms: u64) -> Totals {
        let cutoff_ms = now_ms.saturating_sub(self.duration_ms);
        let mut t = Totals::default();
        for b in &self.buckets {
            if !b.initialized {
                continue;
            }
            if b.bucket_idx_ms + BUCKET_GRANULARITY_MS <= cutoff_ms {
                continue;
            }
            t.requests += b.requests;
            t.input_tokens += b.input_tokens;
            t.output_tokens += b.output_tokens;
        }
        t
    }

    /// Milliseconds until the oldest currently-counted bucket falls out of window.
    pub fn next_boundary_ms(&self, now_ms: u64) -> u64 {
        let cutoff_ms = now_ms.saturating_sub(self.duration_ms);
        let mut oldest = u64::MAX;
        for b in &self.buckets {
            if !b.initialized {
                continue;
            }
            if b.bucket_idx_ms + BUCKET_GRANULARITY_MS <= cutoff_ms {
                continue;
            }
            if b.bucket_idx_ms < oldest {
                oldest = b.bucket_idx_ms;
            }
        }
        if oldest == u64::MAX {
            return 0;
        }
        oldest
            .saturating_add(BUCKET_GRANULARITY_MS)
            .saturating_add(self.duration_ms)
            .saturating_sub(now_ms)
    }
}

#[derive(Debug, Default)]
pub struct CalendarCounter {
    pub totals: Totals,
    pub reset_at_ms: u64,
}

impl CalendarCounter {
    pub fn new(unit: CalendarUnit, now_ms: u64) -> Self {
        Self {
            totals: Totals::default(),
            reset_at_ms: next_calendar_boundary_ms(unit, now_ms),
        }
    }

    pub fn add(
        &mut self,
        now_ms: u64,
        unit: CalendarUnit,
        requests: u64,
        input_tokens: u64,
        output_tokens: u64,
    ) {
        if now_ms >= self.reset_at_ms {
            self.totals = Totals::default();
            self.reset_at_ms = next_calendar_boundary_ms(unit, now_ms);
        }
        self.totals.requests += requests;
        self.totals.input_tokens += input_tokens;
        self.totals.output_tokens += output_tokens;
    }

    pub fn totals(&self, now_ms: u64, _unit: CalendarUnit) -> Totals {
        if now_ms >= self.reset_at_ms {
            Totals::default()
        } else {
            self.totals
        }
    }

    pub fn next_boundary_ms(&self, now_ms: u64) -> u64 {
        self.reset_at_ms.saturating_sub(now_ms)
    }
}

fn next_calendar_boundary_ms(unit: CalendarUnit, now_ms: u64) -> u64 {
    let unit_ms = unit.duration_ms();
    ((now_ms / unit_ms) + 1) * unit_ms
}

/// Run the warn/reject check logic against an arbitrary totals snapshot.
/// `next_boundary_ms` is the ms-until-window-rolls (used in `Reject.retry_after_ms`).
pub fn evaluate(cfg: &QuotaConfig, totals: Totals, next_boundary_ms: u64) -> QuotaCheck {
    let metrics: [(&'static str, u64, Option<u64>); 3] = [
        ("requests", totals.requests, cfg.max_requests),
        ("input_tokens", totals.input_tokens, cfg.max_input_tokens),
        ("output_tokens", totals.output_tokens, cfg.max_output_tokens),
    ];

    // Hard reject — first metric over the limit wins.
    for (metric, used, max) in metrics {
        if let Some(max) = max
            && max > 0
            && used >= max
        {
            return QuotaCheck::Reject {
                metric,
                retry_after_ms: next_boundary_ms.max(1),
            };
        }
    }

    // Warn — first metric at/above warn threshold wins.
    for (metric, used, max) in metrics {
        if let Some(max) = max
            && max > 0
        {
            let used_pct = ((used as u128 * 100) / max as u128).min(100) as u8;
            if used_pct >= cfg.warn_pct {
                return QuotaCheck::Warn { metric, used_pct };
            }
        }
    }

    QuotaCheck::Ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rolling_durations() {
        assert_eq!(
            parse_window("rolling:30s").unwrap(),
            QuotaWindow::Rolling {
                duration_ms: 30_000
            }
        );
        assert_eq!(
            parse_window("rolling:1m").unwrap(),
            QuotaWindow::Rolling {
                duration_ms: 60_000
            }
        );
        assert_eq!(
            parse_window("rolling:5h").unwrap(),
            QuotaWindow::Rolling {
                duration_ms: 5 * 60 * 60 * 1000
            }
        );
        assert_eq!(
            parse_window("rolling:2d").unwrap(),
            QuotaWindow::Rolling {
                duration_ms: 2 * 24 * 60 * 60 * 1000
            }
        );
    }

    #[test]
    fn parse_calendar_units() {
        assert_eq!(
            parse_window("calendar:minute").unwrap(),
            QuotaWindow::Calendar {
                unit: CalendarUnit::Minute
            }
        );
        assert_eq!(
            parse_window("calendar:hour").unwrap(),
            QuotaWindow::Calendar {
                unit: CalendarUnit::Hour
            }
        );
        assert_eq!(
            parse_window("calendar:day").unwrap(),
            QuotaWindow::Calendar {
                unit: CalendarUnit::Day
            }
        );
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(matches!(
            parse_window("nope"),
            Err(WindowParseError::MissingColon)
        ));
        assert!(matches!(
            parse_window("rolling"),
            Err(WindowParseError::MissingColon)
        ));
        assert!(matches!(
            parse_window("rolling:5x"),
            Err(WindowParseError::InvalidDuration(_))
        ));
        assert!(matches!(
            parse_window("calendar:year"),
            Err(WindowParseError::InvalidUnit(_))
        ));
        assert!(matches!(
            parse_window("foo:bar"),
            Err(WindowParseError::UnknownKind(_))
        ));
    }

    #[test]
    fn duration_ms_returns_correct_value() {
        assert_eq!(
            QuotaWindow::Rolling { duration_ms: 1234 }.duration_ms(),
            1234
        );
        assert_eq!(
            QuotaWindow::Calendar {
                unit: CalendarUnit::Hour
            }
            .duration_ms(),
            3_600_000
        );
    }

    #[test]
    fn ring_add_and_total_in_same_minute() {
        let mut rb = RingBuffer::new(60 * 60 * 1000); // 1h
        rb.add(120_000, 1, 100, 50);
        rb.add(125_000, 2, 200, 80);
        let t = rb.totals(125_000);
        assert_eq!(t.requests, 3);
        assert_eq!(t.input_tokens, 300);
        assert_eq!(t.output_tokens, 130);
    }

    #[test]
    fn ring_drops_old_buckets_outside_window() {
        let mut rb = RingBuffer::new(60_000); // 1 minute window
        rb.add(0, 5, 0, 0);
        let later = 125_000; // > 60s after bucket 0
        let t = rb.totals(later);
        assert_eq!(t.requests, 0);
    }

    #[test]
    fn ring_buffer_size_at_least_one() {
        let rb = RingBuffer::new(0);
        assert_eq!(rb.buckets.len(), 1);
    }

    #[test]
    fn ring_next_boundary_ms_rolls_oldest_out() {
        let mut rb = RingBuffer::new(60 * 60 * 1000); // 1h
        rb.add(0, 1, 0, 0);
        let now = 30 * 60 * 1000; // 30 min in
        // Oldest bucket (bucket_idx_ms=0) falls out at now+30min.
        assert!(rb.next_boundary_ms(now) > 29 * 60 * 1000);
        assert!(rb.next_boundary_ms(now) <= 31 * 60 * 1000);
    }

    #[test]
    fn ring_handles_index_collision_after_full_rotation() {
        let mut rb = RingBuffer::new(60_000); // 1 bucket
        rb.add(0, 1, 0, 0);
        rb.add(300_000, 1, 0, 0); // 5 minutes later — same slot index, different bucket_idx_ms
        let t = rb.totals(300_000);
        assert_eq!(t.requests, 1, "old bucket should be replaced not summed");
    }

    fn cfg_max(reqs: Option<u64>, input: Option<u64>, output: Option<u64>) -> QuotaConfig {
        QuotaConfig {
            provider: "p".into(),
            window: QuotaWindow::Rolling {
                duration_ms: 60_000,
            },
            max_requests: reqs,
            max_input_tokens: input,
            max_output_tokens: output,
            warn_pct: 80,
        }
    }

    #[test]
    fn evaluate_ok_when_no_limits_configured() {
        let cfg = cfg_max(None, None, None);
        let t = Totals {
            requests: 999_999,
            input_tokens: 999_999,
            output_tokens: 999_999,
        };
        assert_eq!(evaluate(&cfg, t, 0), QuotaCheck::Ok);
    }

    #[test]
    fn evaluate_rejects_when_requests_at_max() {
        let cfg = cfg_max(Some(10), None, None);
        let t = Totals {
            requests: 10,
            ..Default::default()
        };
        assert_eq!(
            evaluate(&cfg, t, 5_000),
            QuotaCheck::Reject {
                metric: "requests",
                retry_after_ms: 5_000
            }
        );
    }

    #[test]
    fn evaluate_warn_at_or_above_warn_pct() {
        let cfg = cfg_max(Some(10), None, None);
        let t = Totals {
            requests: 8,
            ..Default::default()
        }; // exactly 80%
        assert_eq!(
            evaluate(&cfg, t, 0),
            QuotaCheck::Warn {
                metric: "requests",
                used_pct: 80
            }
        );
    }

    #[test]
    fn evaluate_reject_takes_priority_over_warn() {
        let cfg = QuotaConfig {
            provider: "p".into(),
            window: QuotaWindow::Rolling {
                duration_ms: 60_000,
            },
            max_requests: Some(10),
            max_input_tokens: Some(100),
            max_output_tokens: None,
            warn_pct: 80,
        };
        let t = Totals {
            requests: 100,
            input_tokens: 80,
            ..Default::default()
        };
        let r = evaluate(&cfg, t, 1_000);
        assert!(matches!(r, QuotaCheck::Reject { .. }));
    }

    #[test]
    fn calendar_resets_at_boundary() {
        let unit = CalendarUnit::Hour;
        let mut c = CalendarCounter::new(unit, 0);
        c.add(0, unit, 5, 0, 0);
        assert_eq!(c.totals(0, unit).requests, 5);
        let after_hour = unit.duration_ms();
        c.add(after_hour, unit, 1, 0, 0);
        assert_eq!(
            c.totals(after_hour, unit).requests,
            1,
            "must reset, not accumulate"
        );
    }

    #[test]
    fn calendar_next_boundary_decreases() {
        let c = CalendarCounter::new(CalendarUnit::Hour, 0);
        let early = c.next_boundary_ms(0);
        let later = c.next_boundary_ms(30 * 60 * 1000); // 30 min in
        assert!(later < early);
    }
}
