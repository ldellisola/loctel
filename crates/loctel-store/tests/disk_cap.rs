//! The disk cap, the way a failed write is handled and what the store says
//! about its disk (Milestone 5, Task 4).
//!
//! The cap deletes the oldest segment file whole, with the traces and the log
//! chunks that are in it, until the files take no more than the cap. These
//! tests fill a spilling store with 4 KiB segments (so that a few dozen traces
//! make many files), change the cap, and look at the files, at what the store
//! still holds and at its counters, which must say the same thing.

mod fixtures;

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use fixtures::*;
use loctel_meshgen::Scenario;
use loctel_model::TraceId;
use loctel_store::{LogFilter, ManualClock, PageRequest, Store, StoreConfig, TraceFilter};
use tempfile::TempDir;

const BASE: u64 = 2_000_000_000_000;
const SEC: u64 = 1_000_000_000;

/// Longer than the 10 s quiet period of the default config: every trace the
/// store holds is complete after it, and (the test config spills at once) cold.
const QUIET: Duration = Duration::from_secs(11);

/// The spans of a `CheckoutSuccess` trace.
const SPANS: usize = 14;

fn page_all() -> PageRequest {
    PageRequest {
        limit: usize::MAX,
        cursor: None,
    }
}

/// The segment files of `dir` with their sizes, oldest first (by the number in
/// the name, which counts up over both kinds).
fn segments(dir: &Path) -> Vec<(String, u64)> {
    let mut files: Vec<(String, u64)> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().into_string().unwrap(),
                entry.metadata().unwrap().len(),
            )
        })
        .filter(|(name, _)| name.ends_with(".seg"))
        .collect();
    files.sort_by_key(|(name, _)| {
        name.trim_end_matches(".seg")
            .rsplit('-')
            .next()
            .and_then(|number| number.parse::<u64>().ok())
    });
    files
}

fn bytes_of(files: &[(String, u64)]) -> u64 {
    files.iter().map(|(_, bytes)| bytes).sum()
}

fn spilling_store(dir: &Path) -> (Store, Arc<ManualClock>) {
    store_with_config(StoreConfig::spilling_for_tests(dir.to_path_buf()))
}

/// Feeds `count` `CheckoutSuccess` traces to `store`, each left to go quiet
/// before the next: they spill one at a time, in the order of the returned IDs,
/// so the segments hold them oldest first.
fn fill_traces(store: &Store, clock: &ManualClock, count: u64) -> Vec<TraceId> {
    let mut ids = Vec::new();
    for n in 0..count {
        let spans = scenario_spans(&Scenario::CheckoutSuccess, 1_000 + n, BASE + n * SEC);
        assert_eq!(spans.len(), SPANS);
        ids.push(spans[0].trace_id);
        store.ingest_spans(spans);
        clock.advance(QUIET);
    }
    // The last one goes quiet and spills.
    store.stats();
    ids
}

/// The log lines `from..to`: distinct bodies (so that nothing is a retry of
/// another) one millisecond apart, so that "line n" is the n-th oldest.
fn lines(from: u64, to: u64) -> Vec<loctel_model::LogRecord> {
    (from..to)
        .map(|n| {
            log(
                "worker",
                &format!(
                    "line {n} checksum {}",
                    n.wrapping_mul(2_654_435_761) % 100_003
                ),
                1_700_000_000_000 + n,
            )
        })
        .collect()
}

/// The numbers `n` of the held "line n" logs, newest first.
fn held_lines(store: &Store) -> Vec<u64> {
    store
        .search_logs(&LogFilter::default(), &page_all())
        .items
        .iter()
        .map(|record| {
            record
                .body
                .split(' ')
                .nth(1)
                .and_then(|n| n.parse().ok())
                .unwrap_or_else(|| panic!("not one of our lines: {:?}", record.body))
        })
        .collect()
}

/// Everything the store says about what it holds agrees with what it holds and
/// with the files on disk.
fn assert_consistent(store: &Store, dir: &Path) {
    let stats = store.stats();
    let files = segments(dir);
    assert_eq!(
        stats.disk_bytes,
        bytes_of(&files),
        "the disk bytes are what the files take"
    );
    assert_eq!(store.disk_bytes(), stats.disk_bytes);

    let traces = store.search_traces(&TraceFilter::default(), &page_all());
    assert_eq!(traces.items.len(), stats.traces, "traces");
    assert_eq!(
        traces.items.iter().map(|t| t.span_count).sum::<usize>(),
        stats.spans,
        "spans"
    );
    for item in &traces.items {
        let trace = store
            .trace(item.trace_id)
            .expect("a trace the search lists can be read");
        assert_eq!(trace.spans.len(), item.span_count);
    }
    let logs = store.search_logs(&LogFilter::default(), &page_all());
    assert_eq!(logs.items.len(), stats.logs, "logs");
    assert_eq!(stats.spilled_traces, store.spilled_traces());
    assert!(stats.spilled_traces <= stats.traces);
}

// ---- the cap --------------------------------------------------------------------

#[test]
fn the_default_cap_is_4_gib_and_the_retry_a_minute() {
    let config = StoreConfig::default();
    assert_eq!(config.disk_cap_bytes, 4 * 1024 * 1024 * 1024);
    assert_eq!(config.spill_retry, Duration::from_secs(60));
    let (store, _) = store_with_manual_clock();
    assert_eq!(store.disk_cap_bytes(), 4 * 1024 * 1024 * 1024);
    store.set_disk_cap(7);
    assert_eq!(store.disk_cap_bytes(), 7);
}

#[test]
fn cap_evicts_oldest_segments_whole() {
    let dir = TempDir::new().unwrap();
    let (store, clock) = spilling_store(dir.path());
    let ids = fill_traces(&store, &clock, 40);
    assert_eq!(store.spilled_traces(), 40);
    let before = segments(dir.path());
    assert!(before.len() >= 6, "many small segments: {before:?}");
    let total = bytes_of(&before);
    assert_eq!(store.disk_bytes(), total);

    // A cap that keeps about the newest half of the files.
    let cap = total / 2;
    store.set_disk_cap(cap);

    let after = segments(dir.path());
    let kept = after.len();
    assert!(
        0 < kept && kept < before.len(),
        "some went, some stayed: {after:?}"
    );
    assert_eq!(
        after,
        before[before.len() - kept..],
        "the oldest files went, whole, and no other"
    );
    let held = bytes_of(&after);
    assert!(held <= cap, "under the cap: {held} of {cap}");
    assert_eq!(store.disk_bytes(), held);
    let youngest_gone = before[before.len() - kept - 1].1;
    assert!(
        held + youngest_gone > cap,
        "it stopped as soon as it was under the cap"
    );

    // The traces of the deleted files are gone, whole; the others read back
    // whole, and there is no gap between them.
    let alive: Vec<bool> = ids.iter().map(|id| store.trace(*id).is_some()).collect();
    let first_alive = alive.iter().position(|alive| *alive).expect("some stay");
    assert!(first_alive > 0, "some went");
    assert!(
        alive[first_alive..].iter().all(|alive| *alive),
        "the newest traces stay and the oldest go: {alive:?}"
    );
    for id in &ids[first_alive..] {
        assert_eq!(store.trace(*id).unwrap().spans.len(), SPANS);
    }
    let stats = store.stats();
    assert_eq!(stats.traces, 40 - first_alive);
    assert_eq!(stats.spans, SPANS * (40 - first_alive));
    assert_eq!(stats.spilled_traces, stats.traces);
    assert_eq!(
        stats.spans_received,
        (40 * SPANS) as u64,
        "what was evicted was received all the same"
    );
    assert_consistent(&store, dir.path());

    // A late span of an evicted trace starts a new, partial one, as for a trace
    // the count limit evicted.
    store.ingest_spans(vec![late_span(&scenario_spans(
        &Scenario::CheckoutSuccess,
        1_000,
        BASE,
    ))]);
    assert_eq!(store.trace(ids[0]).unwrap().spans.len(), 1);
}

/// A span that joins the trace of `spans[0]` with an ID the trace does not have.
fn late_span(spans: &[loctel_model::Span]) -> loctel_model::Span {
    let mut span = spans[0].clone();
    span.span_id = sid(0xEE);
    span
}

#[test]
fn the_cap_holds_while_data_arrives() {
    let dir = TempDir::new().unwrap();
    let cap = 24 * 1024;
    let (store, clock) = store_with_config(StoreConfig {
        disk_cap_bytes: cap,
        ..StoreConfig::spilling_for_tests(dir.path().to_path_buf())
    });
    let mut ids = Vec::new();
    for n in 0..80u64 {
        let spans = scenario_spans(&Scenario::CheckoutSuccess, 2_000 + n, BASE + n * SEC);
        ids.push(spans[0].trace_id);
        store.ingest_spans(spans);
        store.ingest_logs(lines(n * 10, n * 10 + 10));
        clock.advance(QUIET);
        let disk = store.stats().disk_bytes;
        assert!(disk <= cap, "after round {n}: {disk} bytes of {cap}");
        assert_eq!(disk, bytes_of(&segments(dir.path())));
    }
    assert!(
        store.stats().traces < 80,
        "the oldest traces were evicted to stay under the cap"
    );
    assert!(store.stats().traces > 0, "and the newest stay");
    assert!(
        store.trace(*ids.last().unwrap()).is_some(),
        "the newest trace is there"
    );
    assert_consistent(&store, dir.path());
}

#[test]
fn log_segments_go_with_their_chunks() {
    let dir = TempDir::new().unwrap();
    let (store, _clock) = spilling_store(dir.path());
    // 700 lines in chunks of 7: 100 sealed chunks, all in segment files.
    for batch in 0..14 {
        store.ingest_logs(lines(batch * 50, batch * 50 + 50));
    }
    let info = store.log_chunks();
    assert_eq!((info.sealed.len(), info.on_disk, info.open), (100, 100, 0));
    let before = segments(dir.path());
    assert!(before.len() >= 6, "{before:?}");
    assert!(before.iter().all(|(name, _)| name.starts_with("logs-")));
    assert_eq!(held_lines(&store), (0..700).rev().collect::<Vec<_>>());

    let cap = bytes_of(&before) / 2;
    store.set_disk_cap(cap);

    let after = segments(dir.path());
    let kept = after.len();
    assert!(0 < kept && kept < before.len(), "{after:?}");
    assert_eq!(after, before[before.len() - kept..], "the oldest files");
    assert!(bytes_of(&after) <= cap);
    // The newest logs stay, in order, and the oldest are gone: no gap.
    let held = held_lines(&store);
    assert!(!held.is_empty() && held.len() < 700, "{} held", held.len());
    assert_eq!(held[0], 699, "the newest log is still there");
    let oldest = *held.last().unwrap();
    assert_eq!(held, (oldest..700).rev().collect::<Vec<_>>(), "no gap");
    assert_eq!(oldest as usize % 7, 0, "whole chunks went");
    let stats = store.stats();
    assert_eq!(stats.logs, held.len());
    assert_eq!(stats.logs_received, 700);
    let info = store.log_chunks();
    assert_eq!(info.sealed.len(), info.on_disk);
    assert_eq!(info.sealed.iter().sum::<usize>(), held.len());
    assert_consistent(&store, dir.path());

    // What was evicted is forgotten: a retry of it is taken as new, and a retry
    // of a log still held is still ignored.
    store.ingest_logs(lines(0, 1));
    assert_eq!(store.stats().logs_received, 701);
    store.ingest_logs(lines(699, 700));
    assert_eq!(store.stats().logs_received, 701);
}

#[test]
fn lowering_cap_evicts_now_and_keeps_stats_consistent() {
    let dir = TempDir::new().unwrap();
    let (store, clock) = spilling_store(dir.path());
    // Traces and logs interleaved, so that segments of both kinds are born in
    // turn and the oldest file is sometimes one and sometimes the other.
    let mut ids = Vec::new();
    for n in 0..30u64 {
        let spans = scenario_spans(&Scenario::CheckoutSuccess, 3_000 + n, BASE + n * SEC);
        ids.push(spans[0].trace_id);
        store.ingest_spans(spans);
        store.ingest_logs(lines(n * 30, n * 30 + 30));
        clock.advance(QUIET);
    }
    store.stats();
    let kinds: Vec<_> = segments(dir.path())
        .iter()
        .map(|(name, _)| name.starts_with("traces-"))
        .collect();
    assert!(
        kinds.contains(&true) && kinds.contains(&false),
        "both kinds of file: {kinds:?}"
    );
    assert_consistent(&store, dir.path());
    let first = store.stats();
    assert_eq!(first.traces, 30);
    assert_eq!(first.logs, 900);
    let total = first.disk_bytes;

    let (mut traces, mut logs) = (first.traces, first.logs);
    for cap in [total * 3 / 4, total / 2, total / 4, 1] {
        store.set_disk_cap(cap);
        // At once: no later call is needed for the files to go.
        let files = segments(dir.path());
        assert!(
            bytes_of(&files) <= cap,
            "cap {cap}: the files take {}",
            bytes_of(&files)
        );
        assert_consistent(&store, dir.path());
        let stats = store.stats();
        assert_eq!(stats.spans_received, (30 * SPANS) as u64, "cap {cap}");
        assert_eq!(stats.logs_received, 900, "cap {cap}");
        // A lower cap only ever takes more away.
        assert!(stats.traces <= traces && stats.logs <= logs, "cap {cap}");
        (traces, logs) = (stats.traces, stats.logs);
    }
    assert!(traces < 30 && logs < 900, "the caps took data away");

    // Under a cap of 1 byte no record fits: nothing is cold and no file is
    // left. What was never written stays: the log lines still in the open
    // chunk (its logs are not in a segment yet).
    let stats = store.stats();
    assert_eq!(stats.disk_bytes, 0);
    assert_eq!(stats.spilled_traces, 0);
    assert!(segments(dir.path()).is_empty());
    assert_eq!(store.log_chunks().on_disk, 0);

    // The store goes on working once the cap is raised: new data spills.
    store.set_disk_cap(u64::MAX);
    let spans = scenario_spans(&Scenario::CheckoutSuccess, 3_999, BASE + 99 * SEC);
    let id = spans[0].trace_id;
    store.ingest_spans(spans);
    clock.advance(QUIET);
    store.stats();
    assert_eq!(store.spilled_traces(), 1);
    assert_eq!(store.trace(id).unwrap().spans.len(), SPANS);
    assert!(store.stats().disk_bytes > 0);
    assert_consistent(&store, dir.path());

    // `clear` deletes the files, and the cap is still what it was set to.
    store.set_disk_cap(1_000_000);
    store.clear();
    assert!(segments(dir.path()).is_empty());
    assert_eq!(store.stats().disk_bytes, 0);
    assert_eq!(store.disk_cap_bytes(), 1_000_000);
}

#[test]
fn a_cap_smaller_than_a_record_keeps_nothing_on_disk_and_breaks_nothing() {
    let dir = TempDir::new().unwrap();
    let (store, clock) = store_with_config(StoreConfig {
        disk_cap_bytes: 1,
        ..StoreConfig::spilling_for_tests(dir.path().to_path_buf())
    });
    let ids = fill_traces(&store, &clock, 5);
    store.ingest_logs(lines(0, 100));
    store.stats();

    // Every record that was written went again at once, with its segment.
    assert!(segments(dir.path()).is_empty());
    let stats = store.stats();
    assert_eq!((stats.disk_bytes, stats.spilled_traces), (0, 0));
    assert_eq!(stats.traces, 0, "the traces were written and evicted");
    assert!(ids.iter().all(|id| store.trace(*id).is_none()));
    assert_eq!(stats.spans, 0);
    assert_eq!(stats.spans_received, (5 * SPANS) as u64);
    // The 100 lines are 14 sealed chunks of 7 (deleted) and two lines open.
    assert_eq!(stats.logs, 2);
    assert_eq!(stats.logs_received, 100);
    assert_consistent(&store, dir.path());
}

#[test]
fn a_store_in_memory_has_no_disk_and_ignores_the_cap_but_keeps_it() {
    let (store, clock) = store_with_manual_clock();
    store.ingest_spans(scenario_spans(&Scenario::CheckoutFailure, 1, BASE));
    clock.advance(Duration::from_secs(3_600));
    store.set_disk_cap(1);
    let stats = store.stats();
    assert_eq!(stats.disk_bytes, 0);
    assert_eq!(stats.spilled_traces, 0);
    assert_eq!(stats.spill_error, None);
    assert_eq!(stats.traces, 1, "the cap is about disk: memory is not cut");
    assert_eq!(store.disk_bytes(), 0);
    assert_eq!(store.disk_cap_bytes(), 1);
}

#[test]
fn stats_say_how_much_is_on_disk() {
    let dir = TempDir::new().unwrap();
    let (store, clock) = spilling_store(dir.path());
    assert_eq!(store.stats().disk_bytes, 0);
    fill_traces(&store, &clock, 6);
    let stats = store.stats();
    assert_eq!(stats.spilled_traces, 6);
    assert!(stats.disk_bytes > 0);
    assert_eq!(stats.disk_bytes, bytes_of(&segments(dir.path())));
    assert_eq!(stats.spill_error, None);
    assert_consistent(&store, dir.path());
}

// ---- a failing disk -------------------------------------------------------------------

/// The failure `inject_write_failure` makes: what a full volume says.
const FULL: &str = "No space left on device (os error 28)";

#[test]
fn spill_write_failure_keeps_ingesting_and_reports() {
    let dir = TempDir::new().unwrap();
    let (store, clock) = store_with_config(StoreConfig {
        max_traces: 6,
        max_log_records: 30,
        // The real retry: once a minute.
        spill_retry: Duration::from_secs(60),
        ..StoreConfig::spilling_for_tests(dir.path().to_path_buf())
    });

    // The disk works at first: one trace is written.
    let first = scenario_spans(&Scenario::CheckoutSuccess, 4_000, BASE);
    let first_id = first[0].trace_id;
    store.ingest_spans(first);
    clock.advance(QUIET);
    store.stats();
    assert_eq!(store.spilled_traces(), 1);
    assert_eq!(store.stats().spill_error, None);
    let disk_before = store.stats().disk_bytes;
    assert!(disk_before > 0);

    // Then the volume is full. The next trace goes quiet, the step tries to
    // write it and cannot: it stays in memory, and the store says why.
    store.inject_write_failure(true);
    let second = scenario_spans(&Scenario::CheckoutSuccess, 4_001, BASE + SEC);
    let second_id = second[0].trace_id;
    store.ingest_spans(second);
    clock.advance(QUIET);
    let stats = store.stats();
    assert_eq!(stats.spill_error.as_deref(), Some(FULL));
    assert_eq!(stats.spilled_traces, 1, "nothing more was written");
    assert_eq!(stats.disk_bytes, disk_before);
    assert_eq!(stats.traces, 2);
    assert_eq!(store.trace(second_id).unwrap().spans.len(), SPANS);
    let failed = store.failed_write_attempts();
    assert_eq!(failed, 1, "one attempt, and then it waits");

    // Ingest goes on, with no panic and no more attempts: traces beyond the
    // limit evict the oldest whole, as they did before spilling existed, and so
    // do log lines (these are 14 chunks' worth of lines that cannot be written).
    for n in 0..8u64 {
        store.ingest_spans(scenario_spans(
            &Scenario::CheckoutSuccess,
            4_010 + n,
            BASE + (10 + n) * SEC,
        ));
    }
    store.ingest_logs(lines(0, 14));
    store.ingest_logs(lines(14, 44));
    let stats = store.stats();
    assert_eq!(stats.traces, 6, "held up to max_traces");
    assert_eq!(stats.logs, 30, "held up to max_log_records");
    assert_eq!(
        stats.spill_error.as_deref(),
        Some(FULL),
        "still the same error"
    );
    assert!(store.trace(first_id).is_none(), "the oldest was evicted");
    assert_eq!(store.failed_write_attempts(), failed, "no hammering");
    assert_eq!(held_lines(&store), (14..44).rev().collect::<Vec<_>>());
    let info = store.log_chunks();
    assert_eq!(info.on_disk, 0, "the chunks that could not be written");
    assert!(!info.sealed.is_empty());

    // Half a minute on: still waiting. A minute on: one more attempt, which
    // fails as well.
    clock.advance(Duration::from_secs(30));
    store.stats();
    assert_eq!(store.failed_write_attempts(), failed, "not yet");
    clock.advance(Duration::from_secs(31));
    store.stats();
    assert_eq!(store.failed_write_attempts(), failed + 1, "a minute later");
    assert_eq!(store.stats().spill_error.as_deref(), Some(FULL));
    assert_eq!(store.stats().traces, 6);

    // The operator frees space. The store does not notice at once (it tries
    // once a minute) ...
    store.inject_write_failure(false);
    clock.advance(Duration::from_secs(30));
    assert_eq!(store.stats().spill_error.as_deref(), Some(FULL));
    assert_eq!(store.spilled_traces(), 0, "the first was evicted");
    // ... and at the next try the traces that waited are written, and the
    // error is gone.
    clock.advance(Duration::from_secs(31));
    let stats = store.stats();
    assert_eq!(stats.spill_error, None);
    assert_eq!(stats.spilled_traces, 6, "every trace is cold now");
    assert!(stats.disk_bytes > disk_before);
    assert!(store.trace(second_id).is_none(), "evicted by max_traces");
    // The chunks of logs that could not be written stay in memory (they are
    // held up to max_log_records, which is what bounds them), and are read as
    // before; the ones that come now go to disk.
    let info = store.log_chunks();
    assert_eq!(info.on_disk, 0);
    assert_eq!(stats.logs, 30);
    assert_eq!(held_lines(&store), (14..44).rev().collect::<Vec<_>>());
    store.ingest_logs(lines(44, 58));
    let info = store.log_chunks();
    assert_eq!(info.on_disk, 2, "the two chunks sealed since");
    assert_eq!(store.stats().spill_error, None);
    assert_eq!(held_lines(&store), (28..58).rev().collect::<Vec<_>>());
    assert_consistent(&store, dir.path());
}

#[test]
fn logs_that_cannot_be_written_are_held_up_to_the_memory_limit() {
    // `max_log_records` of a store that spills is set for logs in segments
    // (20 million in the app, decision 0053). A log the store could not write
    // costs far more memory than that, so while it holds any, it keeps no more
    // than `max_log_records_in_memory` logs in all.
    let dir = TempDir::new().unwrap();
    let (store, _clock) = store_with_config(StoreConfig {
        max_log_records: 40,
        max_log_records_in_memory: 20,
        ..StoreConfig::spilling_for_tests(dir.path().to_path_buf())
    });
    // On disk: five chunks of seven, held up to `max_log_records`.
    store.ingest_logs(lines(0, 35));
    assert_eq!(store.stats().logs, 35);
    assert_eq!(store.log_chunks().on_disk, 5);

    // The volume is full: two chunks stay in memory, and the oldest logs go
    // until the store holds the memory limit.
    store.inject_write_failure(true);
    store.ingest_logs(lines(35, 49));
    let stats = store.stats();
    assert_eq!(stats.spill_error.as_deref(), Some(FULL));
    assert_eq!(stats.logs, 20, "held up to max_log_records_in_memory");
    assert_eq!(held_lines(&store), (29..49).rev().collect::<Vec<_>>());

    // Space comes back. New chunks go to disk, but the two in memory stay, and
    // so does the limit, until they are evicted in their turn.
    store.inject_write_failure(false);
    store.ingest_logs(lines(49, 56));
    assert_eq!(store.stats().spill_error, None);
    assert_eq!(held_lines(&store), (36..56).rev().collect::<Vec<_>>());
    store.ingest_logs(lines(56, 84));
    assert_eq!(held_lines(&store), (64..84).rev().collect::<Vec<_>>());

    // They are gone: the store keeps `max_log_records` again.
    store.ingest_logs(lines(84, 98));
    assert_eq!(store.stats().logs, 34);
    assert_eq!(held_lines(&store), (64..98).rev().collect::<Vec<_>>());
    assert_consistent(&store, dir.path());
}

#[test]
fn a_failing_write_does_not_stop_the_cap_from_working() {
    // The cap deletes files, which a full volume does not prevent: it is how
    // the space comes back.
    let dir = TempDir::new().unwrap();
    let (store, clock) = spilling_store(dir.path());
    let ids = fill_traces(&store, &clock, 12);
    store.inject_write_failure(true);
    let spans = scenario_spans(&Scenario::CheckoutSuccess, 5_000, BASE + 50 * SEC);
    store.ingest_spans(spans);
    clock.advance(QUIET);
    assert!(store.stats().spill_error.is_some());

    store.set_disk_cap(store.disk_bytes() / 2);

    assert!(store.disk_bytes() <= store.disk_cap_bytes());
    assert!(store.trace(ids[0]).is_none());
    assert!(store.trace(ids[11]).is_some());
    assert!(
        store.stats().spill_error.is_some(),
        "the error stays until a write works"
    );
    assert_consistent(&store, dir.path());
}
