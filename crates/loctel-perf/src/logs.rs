//! The budgets over the store of ten million logs: `rss_10m` (the memory the
//! store takes once it holds them), `logs_count`, `logs_at`, `histogram` and
//! `logql_first_page`. They share one store, which is most of their cost to
//! build; the resident size is read first, before a query has filled the cache
//! of decoded chunks.

use std::sync::Arc;
use std::time::Instant;

use anyhow::{Result, bail};
use loctel_meshgen::{BULK_START_UNIX_NANO, bulk_logs, bulk_traces};
use loctel_model::Severity;
use loctel_store::{LevelSet, LogFilter, PageRequest, Store};

use crate::budget::{HISTOGRAM, LOGQL_FIRST_PAGE, LOGS_AT, LOGS_COUNT, Measurement, RSS_10M};
use crate::load::{SEED, expect_held, feed_logs, feed_spans, new_store};
use crate::progress::say;
use crate::rss::resident_bytes;
use crate::scale::Scale;
use crate::stats::{median, percentile};
use crate::wants::Wants;

/// The logs of the bulk data are this far apart (`meshgen::bulk_logs`).
const LOG_SPACING_NANOS: u64 = 20_000;

const SECOND: u64 = 1_000_000_000;

/// The buckets the Logs screen's histogram has.
const BUCKETS: usize = 48;

/// What the Logs screen's level toggles and service picker make of a filter:
/// errors and warnings of one service. Indexed (`LogFilter::is_indexed`), so
/// the store answers it from what it keeps about each chunk.
fn levels_and_service() -> LogFilter {
    LogFilter {
        levels: Some(LevelSet::from_groups(true, true, false, false)),
        service: Some("payments-api".to_owned()),
        ..LogFilter::default()
    }
}

/// Times `run` `runs` times and gives the median in milliseconds.
fn median_millis(runs: usize, mut run: impl FnMut()) -> f64 {
    let mut times: Vec<f64> = (0..runs)
        .map(|_| {
            let started = Instant::now();
            run();
            started.elapsed().as_secs_f64() * 1_000.0
        })
        .collect();
    median(&mut times)
}

/// A deterministic stream of numbers, for the offsets (splitmix64).
struct Offsets(u64);

impl Offsets {
    fn next_below(&mut self, bound: u64) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut x = self.0;
        x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        (x ^ (x >> 31)) % bound.max(1)
    }
}

pub fn run(scale: &Scale, wants: &Wants) -> Result<Vec<Measurement>> {
    let dir = tempfile::tempdir()?;
    let (store, clock) = new_store(dir.path());

    say(format!(
        "logs: building {} spans and {} logs",
        scale.spans_beside_logs, scale.logs
    ));
    // The spans first: the logs that follow carry the clock far enough for them
    // to be complete and spilled, as in a long run.
    feed_spans(
        &store,
        &clock,
        bulk_traces(scale.spans_beside_logs, SEED).starting_at(BULK_START_UNIX_NANO),
    );
    feed_logs(&store, &clock, bulk_logs(scale.logs, SEED));
    expect_held(&store, scale.spans_beside_logs, scale.logs)?;
    let stats = store.stats();
    say(format!(
        "logs: built; {} traces on disk of {}, {} MB of segment files",
        stats.spilled_traces,
        stats.traces,
        stats.disk_bytes / 1_000_000
    ));

    let mut out = Vec::new();
    if wants.has(&RSS_10M) {
        out.push(rss(&store));
    }
    if wants.has(&LOGS_COUNT) {
        out.push(logs_count(&store, scale)?);
    }
    if wants.has(&LOGS_AT) {
        out.push(logs_at(&store, scale)?);
    }
    if wants.has(&HISTOGRAM) {
        out.push(histogram(&store, scale)?);
    }
    if wants.has(&LOGQL_FIRST_PAGE) {
        out.push(logql_first_page(&store, scale)?);
    }
    Ok(out)
}

fn rss(store: &Store) -> Measurement {
    // The store is alive here, so what it holds is in the number.
    let _keep = store.stats();
    match resident_bytes() {
        Some(bytes) => {
            say(format!("rss_10m: {} MB resident", bytes / 1_000_000));
            Measurement::value(&RSS_10M, bytes as f64)
        }
        None => Measurement::not_measured(
            &RSS_10M,
            "the resident size is read from /proc, which only Linux has",
        ),
    }
}

fn logs_count(store: &Store, scale: &Scale) -> Result<Measurement> {
    let filter = levels_and_service();
    let Some(counted) = store.log_count(&filter) else {
        bail!("log_count answered None for an indexed filter");
    };
    if counted == 0 {
        bail!("the filter of logs_count matches no log; the data has no errors of payments-api");
    }
    let millis = median_millis(scale.runs, || {
        let _ = store.log_count(&filter);
    });
    say(format!("logs_count: {counted} lines, {millis:.2} ms"));
    Ok(Measurement::value(&LOGS_COUNT, millis))
}

fn logs_at(store: &Store, scale: &Scale) -> Result<Measurement> {
    let filter = LogFilter::default();
    let total = store
        .log_count(&filter)
        .ok_or_else(|| anyhow::anyhow!("log_count answered None for the empty filter"))?;
    let mut offsets = Offsets(SEED);
    let mut times = Vec::with_capacity(scale.random_offsets);
    for _ in 0..scale.random_offsets {
        // Leave room for the 50 rows.
        let offset = offsets.next_below(total.saturating_sub(50));
        let started = Instant::now();
        let rows = store.logs_at(&filter, offset, 50);
        times.push(started.elapsed().as_secs_f64() * 1_000.0);
        if rows.len() != 50 {
            bail!(
                "logs_at({offset}, 50) returned {} rows of {total}",
                rows.len()
            );
        }
    }
    let p95 = percentile(&mut times, 95.0);
    say(format!(
        "logs_at: p50 {:.2} ms, p95 {p95:.2} ms",
        percentile(&mut times, 50.0)
    ));
    Ok(Measurement::value(&LOGS_AT, p95))
}

/// The window the Logs screen asks for: from a whole second, in buckets of a
/// whole number of seconds, so that no bucket edge falls inside a second the
/// store has only counted (decision 0046).
fn histogram_window(first_unix_nano: u64, logs: usize) -> (u64, u64) {
    let since = first_unix_nano / SECOND * SECOND;
    let last = first_unix_nano + logs as u64 * LOG_SPACING_NANOS;
    let seconds = (last - since).div_ceil(SECOND);
    let width = seconds.div_ceil(BUCKETS as u64).max(1);
    (since, since + BUCKETS as u64 * width * SECOND)
}

fn histogram(store: &Store, scale: &Scale) -> Result<Measurement> {
    let filter = levels_and_service();
    let first = store
        .oldest_log_unix_nano()
        .ok_or_else(|| anyhow::anyhow!("the store holds no log"))?;
    let (since, until) = histogram_window(first, scale.logs);
    let buckets = store.log_histogram(&filter, since, until, BUCKETS);
    let counted: u64 = buckets.iter().map(|bucket| bucket.total).sum();
    let expected = store
        .log_count(&filter)
        .ok_or_else(|| anyhow::anyhow!("log_count answered None for an indexed filter"))?;
    if buckets.len() != BUCKETS || counted != expected {
        bail!(
            "the histogram has {} buckets and {counted} logs; log_count says {expected}",
            buckets.len()
        );
    }
    let millis = median_millis(scale.runs, || {
        let _ = store.log_histogram(&filter, since, until, BUCKETS);
    });
    say(format!("histogram: {millis:.2} ms"));
    Ok(Measurement::value(&HISTOGRAM, millis))
}

fn logql_first_page(store: &Store, scale: &Scale) -> Result<Measurement> {
    let query = loctel_query::logql::parse(r#"{service_name="payments-api"} |= "timeout""#)
        .map_err(|error| {
            anyhow::anyhow!("the LogQL query of the budget does not parse: {error}")
        })?;
    let filter = LogFilter {
        query: Some(Arc::new(query)),
        ..LogFilter::default()
    };
    let page = PageRequest {
        limit: 50,
        cursor: None,
    };
    let first = store.search_logs_keyed(&filter, &page);
    if first.items.len() != 50 {
        bail!(
            "the first page of the LogQL budget has {} lines, not 50",
            first.items.len()
        );
    }
    if !first.items.iter().all(|(_, log)| {
        log.service() == "payments-api"
            && log.body.contains("timeout")
            && matches!(log.severity, Severity::Warn | Severity::Error)
    }) {
        bail!("the first page of the LogQL budget has a line that is not a payments-api timeout");
    }
    let millis = median_millis(scale.runs, || {
        let _ = store.search_logs_keyed(&filter, &page);
    });
    say(format!("logql_first_page: {millis:.2} ms"));
    Ok(Measurement::value(&LOGQL_FIRST_PAGE, millis))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_histogram_window_is_whole_seconds_and_covers_the_data() {
        let first = BULK_START_UNIX_NANO + 123_456;

        let (since, until) = histogram_window(first, 10_000_000);

        assert_eq!(since % SECOND, 0);
        assert!(since <= first);
        let width = (until - since) / BUCKETS as u64;
        assert_eq!(width % SECOND, 0, "a bucket is whole seconds");
        let last = first + 10_000_000 * LOG_SPACING_NANOS;
        assert!(until >= last, "the window reaches the newest log");
        // Rounding each bucket up to a whole second costs less than a second a
        // bucket, so the window is never much longer than the data.
        assert!(
            until - last < BUCKETS as u64 * SECOND,
            "and no longer than that"
        );
    }

    #[test]
    fn offsets_are_deterministic_and_inside_the_bound() {
        let mut a = Offsets(1);
        let mut b = Offsets(1);
        for _ in 0..1_000 {
            let n = a.next_below(77);
            assert_eq!(n, b.next_below(77));
            assert!(n < 77);
        }
        assert_eq!(Offsets(1).next_below(0), 0);
    }
}
