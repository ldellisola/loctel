//! Building the data the budgets read: a store on a clock that follows the
//! data (so that traces complete, spill and seal chunks the way they do in a
//! long run, in the seconds a test has), filled from the bulk generators.
//!
//! The data goes in through the real OTLP converter
//! (`loctel_ingest::{spans_from_request, logs_from_request}`), so the store
//! sees what a receiver would give it. A thread makes and converts the next
//! batch while the store takes the last: building ten million logs is most of
//! a run.

use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::sync_channel;
use std::thread;
use std::time::Duration;

use anyhow::{Result, bail};
use loctel_ingest::{logs_from_request, spans_from_request};
use loctel_meshgen::{BULK_START_UNIX_NANO, BulkLogs, BulkTraces, Generated};
use loctel_model::{LogRecord, Span};
use loctel_store::{Clock, ManualClock, Store, StoreConfig};

use crate::progress::say;

/// The seed of the bulk data. One run is the same bytes as the last.
pub const SEED: u64 = 1;

/// How many converted batches wait for the store.
const QUEUE: usize = 8;

/// The store config of the big-data budgets: the defaults, spilling into `dir`,
/// with the limits raised past what the budgets load (the app's defaults are
/// 200,000 traces and 5,000,000 logs, and a budget over ten million logs must
/// not measure a store that evicted half of them) and the disk cap past what
/// ten million of each take.
pub fn store_config(dir: &Path) -> StoreConfig {
    StoreConfig {
        spill_dir: Some(dir.to_path_buf()),
        max_traces: 20_000_000,
        max_log_records: 100_000_000,
        disk_cap_bytes: 64 * 1024 * 1024 * 1024,
        ..StoreConfig::default()
    }
}

/// A store spilling into `dir`, on a clock that starts where the bulk data
/// does.
pub fn new_store(dir: &Path) -> (Store, Arc<ManualClock>) {
    let clock = Arc::new(ManualClock::new(BULK_START_UNIX_NANO));
    (Store::new(store_config(dir), clock.clone()), clock)
}

/// Moves `clock` to `at` if that is later than it is: data arrives in time
/// order, so the clock only goes forward.
fn follow(clock: &ManualClock, at: u64) {
    if at > clock.now_unix_nano() {
        clock.set(at);
    }
}

/// Makes the batches of `batches` into `T`s on a thread of its own and hands
/// them to `consume` on this one, in order.
fn pipelined<I, T>(batches: I, convert: impl Fn(I::Item) -> T + Send, mut consume: impl FnMut(T))
where
    I: Iterator + Send,
    T: Send,
{
    thread::scope(|scope| {
        let (send, receive) = sync_channel::<T>(QUEUE);
        scope.spawn(move || {
            for batch in batches {
                if send.send(convert(batch)).is_err() {
                    break;
                }
            }
        });
        for converted in receive {
            consume(converted);
        }
    });
}

/// Ingests every log of `logs`, the clock following their time.
pub fn feed_logs(store: &Store, clock: &ManualClock, logs: BulkLogs) {
    let total = logs.len();
    let mut batches = 0usize;
    pipelined(
        logs,
        |batch| {
            let converted = logs_from_request(batch.logs.expect("a logs batch"));
            debug_assert_eq!(converted.rejected, 0, "generated logs are valid");
            converted.items
        },
        |logs: Vec<LogRecord>| {
            if let Some(latest) = logs.iter().map(|log| log.time_unix_nano).max() {
                follow(clock, latest);
            }
            store.ingest_logs(logs);
            batches += 1;
            if batches.is_multiple_of(2_000) {
                say(format!("  {batches} of {total} batches of logs"));
            }
        },
    );
}

/// Ingests every span of `traces`, the clock following their start.
pub fn feed_spans(store: &Store, clock: &ManualClock, traces: BulkTraces) {
    pipelined(
        traces,
        |batch| {
            let converted = spans_from_request(batch.traces.expect("a traces batch"));
            debug_assert_eq!(converted.rejected, 0, "generated spans are valid");
            converted.items
        },
        |spans: Vec<Span>| {
            if let Some(latest) = spans.iter().map(|span| span.start_unix_nano).max() {
                follow(clock, latest);
            }
            store.ingest_spans(spans);
        },
    );
}

/// Ingests a generated scenario (a huge trace), whole.
pub fn feed_generated(store: &Store, generated: &Generated) {
    for batch in &generated.batches {
        if let Some(request) = &batch.traces {
            let converted = spans_from_request(request.clone());
            store.ingest_spans(converted.items);
        }
        if let Some(request) = &batch.logs {
            let converted = logs_from_request(request.clone());
            store.ingest_logs(converted.items);
        }
    }
}

/// Lets a minute and a half go by on `clock`, which is more than a trace takes
/// to be complete (10 s) and then spilled (30 s), and runs the store's spill
/// step, so that what can be on disk is.
pub fn settle(store: &Store, clock: &ManualClock) {
    clock.advance(Duration::from_secs(90));
    // Any read runs the step (at most once a second of the store's clock).
    let _ = store.stats();
}

/// Fails unless the store holds what was loaded: a budget measured over a store
/// that dropped half the data would look better than the truth.
pub fn expect_held(store: &Store, spans: usize, logs: usize) -> Result<()> {
    let stats = store.stats();
    if stats.spans != spans || stats.logs != logs {
        bail!(
            "the store holds {} spans and {} logs, not the {spans} and {logs} that were loaded \
             (disk {} bytes, spill error {:?})",
            stats.spans,
            stats.logs,
            stats.disk_bytes,
            stats.spill_error
        );
    }
    Ok(())
}
