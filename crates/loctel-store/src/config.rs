//! Store limits and timings.

use std::path::PathBuf;
use std::time::Duration;

use crate::log_chunks::{LOG_CHUNK_AGE, LOG_CHUNK_MAX};

/// Limits and timings for a [`Store`](crate::Store).
///
/// The first three fields are what the store always had. The rest are about
/// spilling (Milestone 5): with a [`spill_dir`](Self::spill_dir) set, complete
/// traces move out of memory into segment files in that directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreConfig {
    /// Most traces kept. Past this, the oldest trace (by first arrival) is
    /// evicted whole.
    pub max_traces: usize,
    /// Most log records kept. Past this, the oldest (by log time) are
    /// evicted.
    pub max_log_records: usize,
    /// How long a trace must go without a new span to count as complete.
    pub completeness_quiet: Duration,
    /// Where segment files go. `None` keeps everything in memory (tests, and
    /// runs that choose to). The app passes the directory of its run
    /// ([`RunDir`](crate::run_dir::RunDir)), which it deletes when it quits;
    /// the store creates it when it is missing and removes its own files from
    /// it when the last clone of the store is dropped.
    pub spill_dir: Option<PathBuf>,
    /// How long a trace stays in memory after it is complete, before it is
    /// moved to a segment: the complete trace a user is still looking at, or
    /// that a slow service still adds to, stays cheap to read.
    pub spill_after: Duration,
    /// How many spans of traces in memory the store holds before it moves the
    /// oldest complete traces out early, without waiting for `spill_after`.
    pub hot_max_spans: usize,
    /// The size at which a segment file is finished and the next trace goes
    /// into a new one. A trace is never split, so a file is a little over it.
    pub segment_max_bytes: u64,
    /// The most disk the segment files may use: the oldest segments go whole
    /// when they would take more. Not enforced yet; the disk cap is the next
    /// task of the milestone.
    pub disk_cap_bytes: u64,
    /// How many logs the open chunk takes before it is sealed (see
    /// "Log chunks" in `docs/areas/store.md`). A sealed chunk is written to a
    /// segment file when the store has one, and read back through a small cache
    /// of decoded chunks.
    pub log_chunk_max: usize,
    /// How long the open chunk may stay open, on the store's clock from its
    /// first log, before it is sealed whatever its size: so that a store that
    /// is no longer written to gives its last logs back to the disk too.
    pub log_chunk_age: Duration,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            max_traces: 200_000,
            max_log_records: 5_000_000,
            completeness_quiet: Duration::from_secs(10),
            spill_dir: None,
            spill_after: Duration::from_secs(30),
            hot_max_spans: 2_000_000,
            segment_max_bytes: 64 * 1024 * 1024,
            disk_cap_bytes: 4 * 1024 * 1024 * 1024,
            log_chunk_max: LOG_CHUNK_MAX,
            log_chunk_age: LOG_CHUNK_AGE,
        }
    }
}

impl StoreConfig {
    /// A config that spills into `dir` as eagerly as it can: a trace goes to a
    /// segment the moment it is complete (`spill_after` is zero), a segment
    /// rolls every 4 KiB, the oldest complete traces go early past 16 hot
    /// spans and a chunk of logs is sealed every 7 logs, so a test that lets the
    /// clock pass the quiet period meets every cold path: decoding on a read,
    /// rehydrating on a late span, a trace spread over many segments, logs
    /// spread over many chunks that a late log overlaps. Everything else is the
    /// default.
    ///
    /// Public only for the tests of this crate and of the crates that test
    /// through the store.
    #[doc(hidden)]
    pub fn spilling_for_tests(dir: PathBuf) -> StoreConfig {
        StoreConfig {
            spill_dir: Some(dir),
            spill_after: Duration::ZERO,
            segment_max_bytes: 4 * 1024,
            hot_max_spans: 16,
            log_chunk_max: 7,
            ..StoreConfig::default()
        }
    }
}
