//! How big a run is. The budgets name their sizes (10M logs, 10M spans, a
//! 100,000-span trace, 20 seconds of ingest); [`Scale::FULL`] is those, and
//! [`Scale::SMOKE`] is a tiny run that only shows the scenarios work (the
//! tests run it in debug builds, and `--smoke` runs it by hand): its numbers
//! mean nothing.

use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scale {
    /// Logs in the store the log budgets read (`logs_count`, `logs_at`,
    /// `histogram`, `logql_first_page`, `rss_10m`).
    pub logs: usize,
    /// Spans in that same store, held beside the logs (`rss_10m`).
    pub spans_beside_logs: usize,
    /// Spans in the store `traceql_first_page` searches.
    pub searched_spans: usize,
    /// Spans in the trace the `trace_open_*` budgets open.
    pub big_trace_spans: usize,
    /// How long `ingest` sends.
    pub ingest: Duration,
    /// The offsets `logs_at` is asked for.
    pub random_offsets: usize,
    /// Runs a timing is the median of.
    pub runs: usize,
}

impl Scale {
    /// The sizes of the budgets.
    pub const FULL: Scale = Scale {
        logs: 10_000_000,
        spans_beside_logs: 1_000_000,
        searched_spans: 10_000_000,
        big_trace_spans: 100_000,
        ingest: Duration::from_secs(20),
        random_offsets: 100,
        runs: 5,
    };

    /// Small enough to run in a debug build in seconds.
    pub const SMOKE: Scale = Scale {
        logs: 200_000,
        spans_beside_logs: 20_000,
        searched_spans: 100_000,
        big_trace_spans: 5_000,
        ingest: Duration::from_secs(1),
        random_offsets: 20,
        runs: 3,
    };
}
