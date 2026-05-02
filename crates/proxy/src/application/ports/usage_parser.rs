//! UsageParser trait — feeds bytes incrementally, returns usage on finish.

use crate::domain::UsageRecord;

pub trait UsageParser: Send {
    /// Tolerates events that arrive split across multiple `feed` calls.
    fn feed(&mut self, chunk: &[u8]);
    fn finish(self: Box<Self>) -> UsageRecord;
}
