//! Store limits and timings.

use std::time::Duration;

/// Limits and timings for a [`Store`](crate::Store).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreConfig {
    /// Most traces kept. Past this, the oldest trace (by first arrival) is
    /// evicted whole.
    pub max_traces: usize,
    /// Most log records kept. Past this, the oldest are evicted. (Enforced
    /// once logs are stored.)
    pub max_log_records: usize,
    /// How long a trace must go without a new span to count as complete.
    pub completeness_quiet: Duration,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            max_traces: 200_000,
            max_log_records: 5_000_000,
            completeness_quiet: Duration::from_secs(10),
        }
    }
}
