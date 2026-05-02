use chrono::NaiveDate;

pub trait Clock: Send + Sync {
    fn today(&self) -> NaiveDate;

    /// Current wall-clock time as epoch milliseconds.
    ///
    /// Defaults to `chrono::Utc::now().timestamp_millis()`. Test fakes can
    /// keep using the default; only call sites that need deterministic
    /// timestamps need to override it.
    fn now_ms(&self) -> i64 {
        chrono::Utc::now().timestamp_millis()
    }
}
