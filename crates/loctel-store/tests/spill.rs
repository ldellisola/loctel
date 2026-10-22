//! Spilling traces to segment files and bringing them back (Milestone 5,
//! Task 2): the trace index, cold reads, rehydration by a late span, and the
//! settings that make the store spill.
//!
//! The other test files run their bodies in memory and spilling through
//! `modes::each_mode`, which pins that the two modes answer alike. These tests
//! look *at* the spill: they build the stores themselves, as a [`Pair`] (one
//! in memory, one spilling, fed the same data on the same clock), and say what
//! is on disk.

mod fixtures;
mod modes;

use std::fs::{self, OpenOptions};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use fixtures::*;
use loctel_meshgen::Scenario;
use loctel_model::{Span, SpanId, TraceId};
use loctel_query::traceql;
use loctel_store::{
    Clock, LogFilter, ManualClock, Page, PageRequest, Store, StoreConfig, TimeWindow, TraceFilter,
    TraceSnapshot, TraceStatus, TraceSummary,
};
use tempfile::TempDir;

const BASE: u64 = 2_000_000_000_000;
const SEC: u64 = 1_000_000_000;
const MS: u64 = 1_000_000;

/// Longer than the 10 s quiet period of the default config: every trace the
/// store holds is complete after it.
const QUIET: Duration = Duration::from_secs(11);

/// The same data in two stores: `memory` never spills, `spill` spills as soon
/// as a trace is complete (`StoreConfig::spilling_for_tests`). Both read the
/// same kind of manual clock, and [`Pair::advance`] moves them together.
struct Pair {
    memory: Store,
    memory_clock: Arc<ManualClock>,
    spill: Store,
    spill_clock: Arc<ManualClock>,
    dir: TempDir,
}

impl Pair {
    fn new() -> Pair {
        Pair::with(StoreConfig::default())
    }

    /// `limits` gives `max_traces`, `max_log_records` and `completeness_quiet`
    /// to both stores.
    fn with(limits: StoreConfig) -> Pair {
        let dir = TempDir::new().unwrap();
        let spilling = StoreConfig {
            max_traces: limits.max_traces,
            max_log_records: limits.max_log_records,
            completeness_quiet: limits.completeness_quiet,
            ..StoreConfig::spilling_for_tests(dir.path().to_path_buf())
        };
        let (memory, memory_clock) = store_with_config(limits);
        let (spill, spill_clock) = store_with_config(spilling);
        Pair {
            memory,
            memory_clock,
            spill,
            spill_clock,
            dir,
        }
    }

    fn ingest_spans(&self, spans: &[Span]) {
        self.memory.ingest_spans(spans.to_vec());
        self.spill.ingest_spans(spans.to_vec());
    }

    fn ingest_logs(&self, logs: &[loctel_model::LogRecord]) {
        self.memory.ingest_logs(logs.to_vec());
        self.spill.ingest_logs(logs.to_vec());
    }

    /// Moves both clocks on, and gives the spilling store a call in which to
    /// notice (the store spills ahead of every call, at most once a second).
    fn advance(&self, by: Duration) {
        self.memory_clock.advance(by);
        self.spill_clock.advance(by);
        self.spill.stats();
    }

    /// Lets every trace go quiet, which spills them.
    fn settle(&self) {
        self.advance(QUIET);
    }

    fn segment_files(&self) -> Vec<String> {
        segment_files(self.dir.path())
    }
}

fn segment_files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.ends_with(".seg"))
        .collect();
    names.sort();
    names
}

/// A store that spills as soon as a trace is complete, on its own manual
/// clock.
fn spilling_store(dir: &Path) -> (Store, Arc<ManualClock>) {
    store_with_config(StoreConfig::spilling_for_tests(dir.to_path_buf()))
}

fn page_all() -> PageRequest {
    PageRequest {
        limit: usize::MAX,
        cursor: None,
    }
}

/// Both snapshots say the same about a trace, field by field.
fn assert_same_snapshot(want: &TraceSnapshot, got: &TraceSnapshot) {
    assert_eq!(want.trace_id, got.trace_id);
    assert_eq!(want.spans, got.spans, "spans");
    assert_eq!(want.root, got.root, "root");
    assert_eq!(want.orphans, got.orphans, "orphans");
    assert_eq!(want.complete, got.complete, "complete");
    assert_eq!(want.status, got.status, "status");
    assert_eq!(want.failed_in, got.failed_in, "failed_in");
}

fn trace_id_of(spans: &[Span]) -> TraceId {
    spans[0].trace_id
}

// ---- configuration ------------------------------------------------------------

#[test]
fn the_harness_clock_starts_where_the_fixtures_does() {
    assert_eq!(modes::CLOCK_START, CLOCK_START);
    let runs = std::cell::Cell::new(0);
    modes::each_mode(|store, clock| {
        runs.set(runs.get() + 1);
        assert_eq!(clock.now_unix_nano(), CLOCK_START);
        assert_eq!(store.now_unix_nano(), CLOCK_START);
    });
    assert_eq!(runs.get(), 2, "in memory, then spilling");
}

#[test]
fn spill_defaults_match_the_plan() {
    let config = StoreConfig::default();
    assert_eq!(config.spill_dir, None, "spilling is something to opt in to");
    assert_eq!(config.spill_after, Duration::from_secs(30));
    assert_eq!(config.hot_max_spans, 2_000_000);
    assert_eq!(config.segment_max_bytes, 64 * 1024 * 1024);
    assert_eq!(config.disk_cap_bytes, 4 * 1024 * 1024 * 1024);
    // The Milestone 1 to 4 defaults did not move.
    assert_eq!(config.max_traces, 200_000);
    assert_eq!(config.max_log_records, 5_000_000);
    assert_eq!(config.completeness_quiet, Duration::from_secs(10));
}

#[test]
fn spilling_for_tests_exercises_every_cold_path() {
    let config = StoreConfig::spilling_for_tests(PathBuf::from("/some/dir"));
    assert_eq!(config.spill_dir, Some(PathBuf::from("/some/dir")));
    assert_eq!(config.spill_after, Duration::ZERO);
    assert_eq!(config.segment_max_bytes, 4 * 1024);
    assert_eq!(config.hot_max_spans, 16);
    // Everything else is the default.
    assert_eq!(config.max_traces, StoreConfig::default().max_traces);
    assert_eq!(config.completeness_quiet, Duration::from_secs(10));
    assert_eq!(config.disk_cap_bytes, StoreConfig::default().disk_cap_bytes);
}

#[test]
fn a_store_without_a_spill_dir_never_spills() {
    let (store, clock) = store_with_manual_clock();
    store.ingest_spans(scenario_spans(&Scenario::CheckoutFailure, 1, BASE));
    clock.advance(Duration::from_secs(3_600));
    store.ingest_spans(scenario_spans(&Scenario::CheckoutSuccess, 2, BASE + SEC));
    assert_eq!(store.spilled_traces(), 0);
    assert_eq!(store.stats().traces, 2);
}

// ---- spilling and reading back ---------------------------------------------------

#[test]
fn complete_trace_spills_and_reads_back_equal() {
    let pair = Pair::new();
    let spans = scenario_spans(&Scenario::CheckoutFailure, 1, BASE);
    assert_eq!(spans.len(), 15);
    let id = trace_id_of(&spans);
    pair.ingest_spans(&spans);

    // Still being written: nothing leaves memory.
    pair.advance(Duration::from_secs(5));
    assert_eq!(pair.spill.spilled_traces(), 0);
    assert!(pair.segment_files().is_empty());

    // Quiet for the quiet period: complete, and spilled with it.
    pair.advance(Duration::from_secs(6));
    assert_eq!(pair.spill.spilled_traces(), 1, "the quiet trace spilled");
    assert_eq!(pair.memory.spilled_traces(), 0);
    assert_eq!(pair.segment_files(), vec!["traces-000000.seg".to_string()]);
    assert!(pair.spill.spill_disk_bytes() > 0);
    assert_eq!(pair.spill.spill_dead_bytes(), 0);
    assert_eq!(pair.spill.stats(), pair.memory.stats(), "same counts");

    // Everything a reader can ask for reads back the same.
    let decodes = pair.spill.trace_decodes();
    let want = pair.memory.trace(id).unwrap();
    let got = pair.spill.trace(id).unwrap();
    assert!(
        pair.spill.trace_decodes() > decodes,
        "the read came from the segment"
    );
    assert_same_snapshot(&want, &got);
    assert!(got.complete);
    assert_eq!(got.status, TraceStatus::Failed);

    assert_eq!(pair.memory.summary(id), pair.spill.summary(id));
    assert_eq!(
        pair.memory.with_trace_spans(id, |spans| spans.to_vec()),
        pair.spill.with_trace_spans(id, |spans| spans.to_vec())
    );
    let scan = |store: &Store| {
        let mut seen = Vec::new();
        store.scan_traces(0, |id, spans, status| {
            seen.push((id, spans.to_vec(), status));
        });
        seen
    };
    assert_eq!(scan(&pair.memory), scan(&pair.spill));
    assert_eq!(
        pair.memory
            .search_traces(&TraceFilter::default(), &page_all()),
        pair.spill
            .search_traces(&TraceFilter::default(), &page_all())
    );
    assert_eq!(
        pair.memory.oldest_start_unix_nano(),
        pair.spill.oldest_start_unix_nano()
    );
}

#[test]
fn an_unfinished_trace_stays_in_memory_beside_a_finished_one() {
    let pair = Pair::new();
    let old = scenario_spans(&Scenario::CheckoutFailure, 1, BASE);
    let new = scenario_spans(&Scenario::CheckoutSuccess, 2, BASE + 20 * SEC);
    pair.ingest_spans(&old);
    pair.settle();
    pair.ingest_spans(&new);
    pair.advance(Duration::from_secs(2));
    assert_eq!(pair.spill.spilled_traces(), 1, "only the quiet one");
    let (old_id, new_id) = (trace_id_of(&old), trace_id_of(&new));
    assert!(pair.spill.trace(old_id).unwrap().complete);
    assert!(!pair.spill.trace(new_id).unwrap().complete);
    // Asking about the unfinished one did not move it either.
    assert_eq!(pair.spill.spilled_traces(), 1);
}

#[test]
fn hot_span_budget_spills_the_oldest_complete_traces_early() {
    // Quiet for 10 s makes a trace complete, but it only spills 30 s later.
    // Past `hot_max_spans`, the oldest complete traces go without waiting.
    let dir = TempDir::new().unwrap();
    let (store, clock) = store_with_config(StoreConfig {
        spill_after: Duration::from_secs(30),
        ..StoreConfig::spilling_for_tests(dir.path().to_path_buf())
    });
    let first = scenario_spans(&Scenario::CheckoutFailure, 1, BASE);
    let second = scenario_spans(&Scenario::CheckoutSuccess, 2, BASE + SEC);
    store.ingest_spans(first.clone());
    clock.advance(Duration::from_secs(1));
    store.ingest_spans(second.clone());
    // 29 hot spans, over the budget of 16, but nothing is complete yet (the
    // first trace has been quiet for 9 s, the second for 8 s).
    clock.advance(Duration::from_secs(8));
    store.stats();
    assert_eq!(store.spilled_traces(), 0, "nothing complete, nothing moves");

    // The first trace is complete (10 s quiet), the second is not (9 s).
    clock.advance(Duration::from_secs(1));
    store.stats();
    assert_eq!(store.spilled_traces(), 1, "the oldest complete one, early");
    assert!(store.trace(trace_id_of(&first)).unwrap().complete);

    // Now 14 hot spans fit the budget: the second one stays until it is past
    // its 30 s.
    clock.advance(Duration::from_secs(5));
    store.stats();
    assert_eq!(store.spilled_traces(), 1, "within budget, nothing is early");
    clock.advance(Duration::from_secs(30));
    store.stats();
    assert_eq!(store.spilled_traces(), 2, "past spill_after, it goes too");
}

#[test]
fn segments_roll_at_their_size_and_clear_removes_them() {
    let pair = Pair::new();
    // 40 traces of 15 spans: far more than the 4 KiB a test segment holds.
    for seed in 0..40u64 {
        pair.ingest_spans(&scenario_spans(
            &Scenario::CheckoutFailure,
            100 + seed,
            BASE + seed * SEC,
        ));
    }
    pair.settle();
    assert_eq!(pair.spill.spilled_traces(), 40);
    let files = pair.segment_files();
    assert!(files.len() >= 3, "rolled several times: {files:?}");
    assert_eq!(files[0], "traces-000000.seg");
    assert_eq!(files[1], "traces-000001.seg");
    // The size of the files is what the store says it holds.
    let on_disk: u64 = files
        .iter()
        .map(|name| fs::metadata(pair.dir.path().join(name)).unwrap().len())
        .sum();
    assert_eq!(pair.spill.spill_disk_bytes(), on_disk);
    // A trace is never split over two segments, so the files are each a bit
    // over the size they roll at, not under half of it.
    assert!(on_disk >= 4 * 1024 * (files.len() as u64 - 1));

    assert_eq!(pair.spill.stats(), pair.memory.stats());
    pair.spill.clear();
    assert!(pair.segment_files().is_empty(), "clear deletes the files");
    assert_eq!(pair.spill.spilled_traces(), 0);
    assert_eq!(pair.spill.spill_disk_bytes(), 0);
    assert_eq!(pair.spill.stats().traces, 0);

    // The store goes on spilling after a clear, into segments of new numbers.
    pair.spill
        .ingest_spans(scenario_spans(&Scenario::CheckoutFailure, 7, BASE));
    pair.spill_clock.advance(QUIET);
    pair.spill.stats();
    assert_eq!(pair.spill.spilled_traces(), 1);
    let after = pair.segment_files();
    assert_eq!(after.len(), 1);
    assert!(after[0] > files[files.len() - 1], "numbers are not reused");
}

#[test]
fn dropping_the_store_deletes_its_segments() {
    let dir = TempDir::new().unwrap();
    let (store, clock) = spilling_store(dir.path());
    store.ingest_spans(scenario_spans(&Scenario::CheckoutFailure, 1, BASE));
    clock.advance(QUIET);
    store.stats();
    assert_eq!(store.spilled_traces(), 1);
    assert_eq!(segment_files(dir.path()).len(), 1);
    let clone = store.clone();
    drop(store);
    assert_eq!(segment_files(dir.path()).len(), 1, "a clone still holds it");
    drop(clone);
    assert!(segment_files(dir.path()).is_empty());
}

#[test]
fn reads_see_records_appended_after_an_earlier_read() {
    // One big segment: the store reads the file it is still writing, and has
    // to notice that it grew.
    let dir = TempDir::new().unwrap();
    let (store, clock) = store_with_config(StoreConfig {
        segment_max_bytes: 64 * 1024 * 1024,
        ..StoreConfig::spilling_for_tests(dir.path().to_path_buf())
    });
    let mut ids = Vec::new();
    for seed in 0..6u64 {
        let spans = scenario_spans(&Scenario::CheckoutSuccess, 50 + seed, BASE + seed * SEC);
        ids.push((trace_id_of(&spans), spans.len()));
        store.ingest_spans(spans);
        clock.advance(QUIET);
        store.stats();
        assert_eq!(store.spilled_traces(), seed as usize + 1);
        // Every trace so far reads back, the new one included.
        for &(id, len) in &ids {
            assert_eq!(store.trace(id).unwrap().spans.len(), len);
        }
    }
    assert_eq!(segment_files(dir.path()).len(), 1, "all in one segment");
}

// ---- late spans -----------------------------------------------------------------

/// A span of the trace of `spans`, as a slow service would send it, under
/// that trace's root.
fn late_span(spans: &[Span], id: u8) -> Span {
    let root = spans
        .iter()
        .find(|span| span.parent_span_id.is_none())
        .unwrap();
    let mut late = root.clone();
    late.span_id = SpanId([id; 8]);
    late.parent_span_id = Some(root.span_id);
    late.name = format!("late {id}");
    late.start_unix_nano = root.start_unix_nano + 5 * MS;
    late.end_unix_nano = late.start_unix_nano + MS;
    late
}

#[test]
fn late_span_rehydrates_cold_trace() {
    let pair = Pair::new();
    let spans = scenario_spans(&Scenario::CheckoutFailure, 1, BASE);
    let id = trace_id_of(&spans);
    pair.ingest_spans(&spans);
    pair.settle();
    assert_eq!(pair.spill.spilled_traces(), 1);
    assert_eq!(pair.spill.spill_dead_bytes(), 0);
    let first_blob = pair.spill.spill_disk_bytes();

    // A slow service sends one more span of the finished trace.
    let late = late_span(&spans, 0xEE);
    pair.ingest_spans(std::slice::from_ref(&late));

    assert_eq!(pair.spill.spilled_traces(), 0, "the trace is hot again");
    let got = pair.spill.trace(id).unwrap();
    assert_eq!(got.spans.len(), 16);
    assert!(got.spans.iter().any(|span| span.span_id == late.span_id));
    assert!(!got.complete, "a new span resets completeness");
    assert_same_snapshot(&pair.memory.trace(id).unwrap(), &got);
    // The blob it came from is dead, and counted as disk until its segment
    // goes; the trace is not counted twice.
    assert_eq!(pair.spill.spill_dead_bytes(), first_blob);
    assert_eq!(pair.spill.spill_disk_bytes(), first_blob);
    let stats = pair.spill.stats();
    assert_eq!(stats, pair.memory.stats());
    assert_eq!((stats.traces, stats.spans), (1, 16));
    assert_eq!(stats.spans_received, 16);

    // It goes quiet and spills again, the late span with it.
    pair.settle();
    assert_eq!(pair.spill.spilled_traces(), 1);
    let again = pair.spill.trace(id).unwrap();
    assert_eq!(again.spans.len(), 16);
    assert!(again.complete);
    assert_same_snapshot(&pair.memory.trace(id).unwrap(), &again);
    assert_eq!(
        pair.spill.spill_dead_bytes(),
        first_blob,
        "only the first blob is dead; the second is live"
    );
    assert!(pair.spill.spill_disk_bytes() > first_blob);
    assert_eq!(pair.spill.stats().spans, 16);
}

#[test]
fn a_retry_of_a_span_the_cold_trace_holds_changes_nothing() {
    let pair = Pair::new();
    let spans = scenario_spans(&Scenario::CheckoutFailure, 1, BASE);
    pair.ingest_spans(&spans);
    pair.settle();
    let disk = pair.spill.spill_disk_bytes();

    // An exporter resends a batch the store has already taken.
    pair.ingest_spans(&spans[..5]);
    assert_eq!(
        pair.spill.spilled_traces(),
        1,
        "a duplicate is not a reason"
    );
    assert_eq!(pair.spill.spill_dead_bytes(), 0);
    assert_eq!(pair.spill.spill_disk_bytes(), disk);
    assert_eq!(pair.spill.stats(), pair.memory.stats());
    assert_eq!(pair.spill.stats().spans_received, 15);
    assert!(pair.spill.trace(trace_id_of(&spans)).unwrap().complete);
}

#[test]
fn a_late_span_with_an_earlier_start_moves_the_trace_in_the_order() {
    let pair = Pair::new();
    let a = scenario_spans(&Scenario::CheckoutSuccess, 1, BASE);
    let b = scenario_spans(&Scenario::CheckoutSuccess, 2, BASE + 10 * SEC);
    pair.ingest_spans(&a);
    pair.ingest_spans(&b);
    pair.settle();
    assert_eq!(pair.spill.spilled_traces(), 2);
    let list = |store: &Store| -> Vec<TraceId> {
        store
            .search_traces(&TraceFilter::default(), &page_all())
            .items
            .iter()
            .map(|summary| summary.trace_id)
            .collect()
    };
    assert_eq!(list(&pair.spill), vec![trace_id_of(&b), trace_id_of(&a)]);

    // Trace `b` gets a span that started long before it: it now starts
    // first, and the newest-first order follows.
    let mut early = late_span(&b, 0xAB);
    early.start_unix_nano = BASE - 5 * SEC;
    early.end_unix_nano = early.start_unix_nano + MS;
    pair.ingest_spans(std::slice::from_ref(&early));
    assert_eq!(list(&pair.spill), list(&pair.memory));
    assert_eq!(list(&pair.spill), vec![trace_id_of(&a), trace_id_of(&b)]);
    assert_eq!(
        pair.spill.oldest_start_unix_nano(),
        pair.memory.oldest_start_unix_nano()
    );
    assert_eq!(pair.spill.oldest_start_unix_nano(), Some(BASE - 5 * SEC));
}

// ---- searching ---------------------------------------------------------------

/// 60 small traces over three services, some failing, with starts that tie,
/// ingested in two waves so that the first 40 go cold and the last 20 stay hot.
fn mixed_pair() -> Pair {
    let pair = Pair::new();
    let wave = |range: std::ops::RangeInclusive<u8>| {
        let mut spans = Vec::new();
        for n in range {
            // Starts tie in fours: traces n and n + 20 start together.
            let start = 1_000 + u64::from(n % 20) * 10;
            let long = 5 + u64::from(n % 9);
            let root = span(n, 1, None, "gateway", "GET /order", start, long + 20);
            let root = if n % 11 == 0 { error(root) } else { root };
            let orders = span(n, 2, Some(1), "orders", "load", start + 1, long);
            let orders = if n % 7 == 0 { error(orders) } else { orders };
            spans.push(root);
            spans.push(orders);
            if n % 3 == 0 {
                spans.push(span(n, 3, Some(2), "payments", "charge", start + 2, 3));
            }
        }
        spans
    };
    pair.ingest_spans(&wave(1..=40));
    pair.settle();
    pair.ingest_spans(&wave(41..=60));
    pair.advance(Duration::from_secs(2));
    assert_eq!(pair.spill.spilled_traces(), 40, "40 cold, 20 hot");
    pair
}

/// Every page of `filter`, `limit` items at a time.
fn walk(store: &Store, filter: &TraceFilter, limit: usize) -> Vec<Page<TraceSummary>> {
    let mut pages = Vec::new();
    let mut cursor = None;
    loop {
        let page = store.search_traces(filter, &PageRequest { limit, cursor });
        cursor = page.next_cursor.clone();
        pages.push(page);
        if cursor.is_none() {
            return pages;
        }
        assert!(pages.len() < 1_000, "a walk that never ends");
    }
}

#[test]
fn search_order_and_cursor_identical_across_modes() {
    let pair = mixed_pair();
    let query = |text: &str| Some(Arc::new(traceql::parse(text).unwrap()));
    let filters = [
        ("everything", TraceFilter::default()),
        (
            "errors only",
            TraceFilter {
                errors_only: true,
                ..TraceFilter::default()
            },
        ),
        (
            "service",
            TraceFilter {
                service: Some("payments".to_string()),
                ..TraceFilter::default()
            },
        ),
        (
            "since",
            TraceFilter {
                since_unix_nano: Some(1_100 * MS),
                ..TraceFilter::default()
            },
        ),
        (
            "min duration",
            TraceFilter {
                min_duration_nanos: Some(30 * MS),
                ..TraceFilter::default()
            },
        ),
        (
            "query",
            TraceFilter {
                query: query("{ resource.service.name = \"payments\" && name = \"charge\" }"),
                ..TraceFilter::default()
            },
        ),
        (
            "everything at once",
            TraceFilter {
                since_unix_nano: Some(1_050 * MS),
                service: Some("orders".to_string()),
                errors_only: true,
                min_duration_nanos: Some(20 * MS),
                query: query("{ status = error }"),
            },
        ),
    ];
    for (label, filter) in &filters {
        for limit in [1, 7, 25, 1_000] {
            let want = walk(&pair.memory, filter, limit);
            let got = walk(&pair.spill, filter, limit);
            assert_eq!(want, got, "{label}, pages of {limit}");
        }
        let total: usize = walk(&pair.memory, filter, 7)
            .iter()
            .map(|page| page.items.len())
            .sum();
        // The filters are not all vacuous: each keeps something.
        assert!(total > 0, "{label} kept nothing");
    }
    assert_eq!(
        walk(&pair.memory, &TraceFilter::default(), 1_000)[0]
            .items
            .len(),
        60
    );

    // A cursor that one store made is one the other understands, since the
    // position it names is the same.
    let page = pair.memory.search_traces(
        &TraceFilter::default(),
        &PageRequest {
            limit: 13,
            cursor: None,
        },
    );
    let next = PageRequest {
        limit: 13,
        cursor: page.next_cursor,
    };
    assert_eq!(
        pair.memory.search_traces(&TraceFilter::default(), &next),
        pair.spill.search_traces(&TraceFilter::default(), &next)
    );
    // And so is a garbage one: an empty page.
    let bad = PageRequest {
        limit: 5,
        cursor: Some("nonsense".to_string()),
    };
    assert!(
        pair.spill
            .search_traces(&TraceFilter::default(), &bad)
            .items
            .is_empty()
    );
}

/// A trace id for the big searches: `n` in the low bytes.
fn big_tid(n: u32) -> TraceId {
    let mut bytes = [0u8; 16];
    bytes[12..].copy_from_slice(&n.to_be_bytes());
    TraceId(bytes)
}

/// Trace `n`: a root and one child named `child`. It starts `n` ms in, so a
/// higher `n` is newer.
fn tiny_trace(n: u32, child: &str) -> Vec<Span> {
    let mut root = span(1, 1, None, "gateway", "GET /x", u64::from(n), 5);
    let mut leaf = span(1, 2, Some(1), "orders", child, u64::from(n) + 1, 2);
    for span in [&mut root, &mut leaf] {
        span.trace_id = big_tid(n);
    }
    vec![root, leaf]
}

#[test]
fn traceql_search_decodes_newest_first_and_stops_at_page() {
    let pair = Pair::new();
    // 300 traces; the 26 newest even ones hold a span called "needle".
    let mut all = Vec::new();
    for n in 1..=300u32 {
        let child = if n >= 250 && n % 2 == 0 {
            "needle"
        } else {
            "hay"
        };
        all.extend(tiny_trace(n, child));
    }
    pair.ingest_spans(&all);
    pair.settle();
    assert_eq!(pair.spill.spilled_traces(), 300);

    let filter = TraceFilter {
        query: Some(Arc::new(traceql::parse("{ name = \"needle\" }").unwrap())),
        ..TraceFilter::default()
    };
    let before = pair.spill.trace_decodes();
    let page = pair.spill.search_traces(
        &filter,
        &PageRequest {
            limit: 20,
            cursor: None,
        },
    );
    let decodes = pair.spill.trace_decodes() - before;
    assert_eq!(page.items.len(), 20);
    assert!(page.next_cursor.is_some());
    // Newest first: 300, 298, ... 262.
    let ids: Vec<TraceId> = page.items.iter().map(|item| item.trace_id).collect();
    let want: Vec<TraceId> = (0..20).map(|k| big_tid(300 - 2 * k)).collect();
    assert_eq!(ids, want);
    assert!(
        decodes < 100,
        "a page of 20 near the top decoded {decodes} of 300 traces"
    );
    assert!(decodes >= 20, "it cannot decode fewer than it returns");
    // The summaries came from the index, the matches from the decoded spans.
    assert_eq!(page.items[0].span_count, 2);
    assert_eq!(page.items[0].matched_span_ids.len(), 1);

    // The second page has the other six matches and has to look at everything
    // older to know there is no more.
    let rest = pair.spill.search_traces(
        &filter,
        &PageRequest {
            limit: 20,
            cursor: page.next_cursor.clone(),
        },
    );
    assert_eq!(rest.items.len(), 6);
    assert_eq!(rest.next_cursor, None);
    // Both modes agree.
    let want = walk(&pair.memory, &filter, 20);
    assert_eq!(walk(&pair.spill, &filter, 20), want);
    assert_eq!(want.len(), 2);

    // A search that needs no spans touches none.
    let before = pair.spill.trace_decodes();
    let plain = pair.spill.search_traces(
        &TraceFilter {
            errors_only: false,
            service: Some("orders".to_string()),
            ..TraceFilter::default()
        },
        &PageRequest {
            limit: 50,
            cursor: None,
        },
    );
    assert_eq!(plain.items.len(), 50);
    assert_eq!(
        pair.spill.trace_decodes(),
        before,
        "metadata is enough for time, service, errors and duration"
    );
}

// ---- bounded work on search (Milestone 5, Task 5) ------------------------------------

/// Trace `n` of [`tiny_trace`], failing at the root when `failed`, else with an
/// error inside (the child span) when `inside`.
fn graded_trace(n: u32, failed: bool, inside: bool) -> Vec<Span> {
    let mut spans = tiny_trace(n, "work");
    if failed {
        spans[0] = error(spans[0].clone());
    } else if inside {
        spans[1] = error(spans[1].clone());
    }
    spans
}

/// 400 traces in both modes, all cold in `spill`: every 10th fails at the root
/// and every 7th of the others has an error inside.
fn graded_pair() -> Pair {
    let pair = Pair::new();
    let mut all = Vec::new();
    for n in 1..=400u32 {
        all.extend(graded_trace(n, n % 10 == 0, n % 7 == 0));
    }
    pair.ingest_spans(&all);
    pair.settle();
    assert_eq!(pair.spill.spilled_traces(), 400);
    pair
}

fn first_page(limit: usize) -> PageRequest {
    PageRequest {
        limit,
        cursor: None,
    }
}

#[test]
fn search_first_page_builds_at_most_limit_plus_one() {
    let pair = graded_pair();
    let filters = [
        ("everything", TraceFilter::default()),
        (
            "errors only",
            TraceFilter {
                errors_only: true,
                ..TraceFilter::default()
            },
        ),
        (
            "a service",
            TraceFilter {
                service: Some("orders".to_string()),
                ..TraceFilter::default()
            },
        ),
        (
            "a duration",
            TraceFilter {
                min_duration_nanos: Some(5 * MS),
                ..TraceFilter::default()
            },
        ),
        (
            "a query",
            TraceFilter {
                query: Some(Arc::new(traceql::parse("{ status = error }").unwrap())),
                ..TraceFilter::default()
            },
        ),
    ];
    for (mode, store) in [("memory", &pair.memory), ("spilling", &pair.spill)] {
        for (name, filter) in &filters {
            for limit in [1, 5, 20] {
                let before = store.summaries_built();
                let page = store.search_traces(filter, &first_page(limit));
                let built = store.summaries_built() - before;
                assert_eq!(page.items.len(), limit, "{mode}, {name}");
                assert!(page.next_cursor.is_some(), "{mode}, {name}: more follow");
                assert!(
                    built <= limit as u64 + 1,
                    "{mode}, {name}: a page of {limit} built {built} summaries of 400 traces"
                );
            }
        }
    }
}

#[test]
fn a_search_builds_a_summary_only_for_what_it_returns() {
    // The traces that do not pass (here the 300-odd that did not fail) cost the
    // index lookup that rejects them, and no summary: in memory a summary is a
    // walk of the trace's spans, and cold it is a copy of the index's.
    let pair = graded_pair();
    let errors_only = TraceFilter {
        errors_only: true,
        ..TraceFilter::default()
    };
    for (mode, store) in [("memory", &pair.memory), ("spilling", &pair.spill)] {
        let before = store.summaries_built();
        let pages = walk(store, &errors_only, 25);
        let returned: usize = pages.iter().map(|page| page.items.len()).sum();
        // 40 fail at the root, and 52 more (every 7th that did not) have an
        // error inside.
        assert_eq!(returned, 92, "{mode}: the traces with errors");
        assert_eq!(
            store.summaries_built() - before,
            returned as u64,
            "{mode}: one summary for each trace returned, none for the rest"
        );
    }
}

/// What `recent_failures` must say, worked out the slow way: every trace with
/// errors by a full search, the first `limit` of each status.
fn failures_by_search(
    store: &Store,
    since: Option<u64>,
    limit: usize,
) -> (Vec<TraceSummary>, Vec<TraceSummary>) {
    let filter = TraceFilter {
        since_unix_nano: since,
        errors_only: true,
        ..TraceFilter::default()
    };
    let all = store.search_traces(&filter, &page_all()).items;
    let by_status = |status: TraceStatus| -> Vec<TraceSummary> {
        all.iter()
            .filter(|summary| summary.status == status)
            .take(limit)
            .cloned()
            .collect()
    };
    (
        by_status(TraceStatus::Failed),
        by_status(TraceStatus::ErrorsInside),
    )
}

#[test]
fn recent_failures_one_pass_equals_a_full_search() {
    let pair = mixed_pair();
    for (mode, store) in [("memory", &pair.memory), ("spilling", &pair.spill)] {
        // Cold and hot traces, failing at the root (5 of them) and inside (8).
        let (all_failed, all_inside) = failures_by_search(store, None, usize::MAX);
        assert_eq!((all_failed.len(), all_inside.len()), (5, 8), "{mode}");
        for since in [None, Some(1_050 * MS), Some(1_150 * MS), Some(9_999 * MS)] {
            for limit in [1, 2, 3, 5, 8, 20] {
                let want = failures_by_search(store, since, limit);
                let before = store.summaries_built();
                let got = store.recent_failures(since, limit);
                let built = store.summaries_built() - before;
                assert_eq!(
                    (&got.failed, &got.errors_inside),
                    (&want.0, &want.1),
                    "{mode}, since {since:?}, limit {limit}"
                );
                assert_eq!(
                    built,
                    (got.failed.len() + got.errors_inside.len()) as u64,
                    "{mode}: a summary for each failure it returns, none for the rest"
                );
            }
        }
        assert!(
            store.recent_failures(None, 0).failed.is_empty(),
            "a limit of none"
        );
    }
}

#[test]
fn recent_failures_builds_nothing_for_a_list_that_is_full() {
    let pair = Pair::new();
    // 300 failed traces and two with an error inside, the oldest ones: the
    // second list can never fill, so the pass goes to the end of the index, and
    // it must not build the summaries of the 295 failures it has no room for.
    let mut all = Vec::new();
    for n in 1..=302u32 {
        all.extend(graded_trace(n, n > 2, n <= 2));
    }
    pair.ingest_spans(&all);
    pair.settle();
    for (mode, store) in [("memory", &pair.memory), ("spilling", &pair.spill)] {
        let before = store.summaries_built();
        let found = store.recent_failures(None, 5);
        assert_eq!(found.failed.len(), 5, "{mode}");
        assert_eq!(found.errors_inside.len(), 2, "{mode}");
        let newest: Vec<TraceId> = (298..=302).rev().map(big_tid).collect();
        let got: Vec<TraceId> = found.failed.iter().map(|item| item.trace_id).collect();
        assert_eq!(got, newest, "{mode}: the newest failures, newest first");
        assert_eq!(
            store.summaries_built() - before,
            7,
            "{mode}: five and two, of 302 traces"
        );
    }
}

#[test]
fn services_reads_only_the_traces_that_reach_the_window() {
    let pair = Pair::new();
    let mut all = Vec::new();
    for n in 1..=300u32 {
        all.extend(tiny_trace(n, "hay"));
    }
    pair.ingest_spans(&all);
    pair.settle();
    assert_eq!(pair.spill.spilled_traces(), 300);

    // Trace n lasts from n ms to n + 5 ms, so the traces that reach 290 ms are
    // 285 to 300.
    let window = TimeWindow {
        since_unix_nano: 290 * MS,
    };
    let before = pair.spill.trace_decodes();
    let built = pair.spill.summaries_built();
    let services = pair.spill.services(window);
    assert_eq!(services, pair.memory.services(window));
    assert!(!services.is_empty());
    let decodes = pair.spill.trace_decodes() - before;
    assert!(
        decodes <= 16,
        "decoded {decodes} of 300 traces for a window that 16 reach"
    );
    assert_eq!(
        pair.spill.summaries_built(),
        built,
        "a services list is counted from spans, not from summaries"
    );
}

// ---- the service map -----------------------------------------------------------

#[test]
fn service_map_identical_across_modes() {
    let pair = Pair::new();
    let failure = scenario_spans(&Scenario::CheckoutFailure, 1, BASE);
    let success = scenario_spans(&Scenario::CheckoutSuccess, 2, BASE + 10 * SEC);
    let health = scenario_spans(&Scenario::HealthChecks { count: 3 }, 3, BASE + 20 * SEC);
    let straddles_since = BASE + 100 * MS;
    // Before anything is quiet: dependencies that need a complete trace are
    // not there yet, in either mode.
    pair.ingest_spans(&failure);
    pair.ingest_spans(&success);
    pair.ingest_spans(&health);
    let windows = [
        TimeWindow::default(),
        TimeWindow {
            since_unix_nano: BASE + 10 * SEC,
        },
        // Cuts through the failing checkout: some of its spans are in the
        // window and some are not.
        TimeWindow {
            since_unix_nano: straddles_since,
        },
        TimeWindow {
            since_unix_nano: BASE + 3_600 * SEC,
        },
    ];
    for window in windows {
        assert_eq!(
            pair.memory.service_map(window),
            pair.spill.service_map(window),
            "{window:?}, hot"
        );
        assert_eq!(pair.memory.services(window), pair.spill.services(window));
    }

    pair.settle();
    assert_eq!(
        pair.spill.spilled_traces(),
        pair.spill.stats().traces,
        "all of them cold"
    );
    for window in windows {
        let want = pair.memory.service_map(window);
        let got = pair.spill.service_map(window);
        assert_eq!(want, got, "{window:?}, cold");
        assert_eq!(pair.memory.services(window), pair.spill.services(window));
        if window == TimeWindow::default() {
            assert!(want.nodes.len() >= 5, "a real map: {want:?}");
            assert!(!want.edges.is_empty());
        }
    }
}

// ---- removal --------------------------------------------------------------------

#[test]
fn remove_trace_on_cold_trace() {
    let pair = Pair::new();
    let failure = scenario_spans(&Scenario::CheckoutFailure, 1, BASE);
    let success = scenario_spans(&Scenario::CheckoutSuccess, 2, BASE + 10 * SEC);
    let third = scenario_spans(&Scenario::CheckoutSuccess, 3, BASE + 20 * SEC);
    pair.ingest_spans(&failure);
    pair.ingest_spans(&success);
    pair.ingest_spans(&third);
    pair.ingest_logs(&scenario_logs(&Scenario::CheckoutFailure, 1, BASE));
    pair.ingest_logs(&scenario_logs(
        &Scenario::CheckoutSuccess,
        2,
        BASE + 10 * SEC,
    ));
    pair.settle();
    assert_eq!(pair.spill.spilled_traces(), 3);
    assert_eq!(pair.spill.spill_dead_bytes(), 0);
    let id = trace_id_of(&failure);

    let removed = pair.spill.remove_trace(id);
    assert_eq!(removed, pair.memory.remove_trace(id));
    assert_eq!((removed.spans, removed.logs), (15, 7));

    // Gone from the index and from every way of reading it.
    assert!(pair.spill.trace(id).is_none());
    assert!(pair.spill.summary(id).is_none());
    assert!(pair.spill.with_trace_spans(id, |_| ()).is_none());
    let listed: Vec<TraceId> = pair
        .spill
        .search_traces(&TraceFilter::default(), &page_all())
        .items
        .iter()
        .map(|item| item.trace_id)
        .collect();
    assert_eq!(listed, vec![trace_id_of(&third), trace_id_of(&success)]);
    let mut scanned = 0;
    pair.spill.scan_traces(0, |_, _, _| scanned += 1);
    assert_eq!(scanned, 2);
    assert_eq!(
        pair.spill.oldest_start_unix_nano(),
        pair.memory.oldest_start_unix_nano()
    );
    // The counts are right: the trace left them, the other two stay cold, and
    // its blob is dead (still on disk until its segment goes).
    let stats = pair.spill.stats();
    assert_eq!(stats, pair.memory.stats());
    assert_eq!((stats.traces, stats.spans), (2, 28));
    assert_eq!(pair.spill.spilled_traces(), 2);
    assert!(pair.spill.spill_dead_bytes() > 0);
    assert!(pair.spill.spill_dead_bytes() < pair.spill.spill_disk_bytes());
    // Its logs went with it, in both modes.
    let logs = |store: &Store| store.search_logs(&LogFilter::default(), &page_all()).items;
    assert_eq!(logs(&pair.spill), logs(&pair.memory));
    assert!(logs(&pair.spill).iter().all(|log| log.trace_id != Some(id)));

    // A second removal finds nothing, and the dead bytes do not count twice.
    let dead = pair.spill.spill_dead_bytes();
    assert_eq!(pair.spill.remove_trace(id), Default::default());
    assert_eq!(pair.spill.spill_dead_bytes(), dead);

    // What was removed is forgotten: its spans come in again as new ones.
    pair.ingest_spans(&failure[..3]);
    assert_eq!(pair.spill.stats(), pair.memory.stats());
    assert_eq!(pair.spill.trace(id).unwrap().spans.len(), 3);
    assert_eq!(pair.spill.spilled_traces(), 2);
}

#[test]
fn remove_spans_on_cold_trace_keeps_it_complete() {
    let pair = Pair::new();
    let spans = scenario_spans(&Scenario::CheckoutFailure, 1, BASE);
    let id = trace_id_of(&spans);
    pair.ingest_spans(&spans);
    pair.settle();
    assert_eq!(pair.spill.spilled_traces(), 1);

    // Two spans that are not there are ignored, and the trace stays cold.
    let nothing = [SpanId([0xF0; 8]), SpanId([0xF1; 8])];
    assert_eq!(pair.spill.remove_spans(id, &nothing), Default::default());
    assert_eq!(pair.spill.spilled_traces(), 1);
    assert_eq!(pair.spill.spill_dead_bytes(), 0);

    let doomed = [spans[3].span_id, spans[4].span_id];
    let removed = pair.spill.remove_spans(id, &doomed);
    assert_eq!(removed, pair.memory.remove_spans(id, &doomed));
    assert_eq!(removed.spans, 2);
    // Taking spans out is not a new arrival: the trace stays complete.
    let got = pair.spill.trace(id).unwrap();
    assert!(got.complete);
    assert_eq!(got.spans.len(), 13);
    assert_same_snapshot(&pair.memory.trace(id).unwrap(), &got);
    assert_eq!(pair.spill.stats(), pair.memory.stats());
    assert_eq!(pair.memory.summary(id), pair.spill.summary(id));

    // It spills again, as it was before, and then loses its last spans.
    pair.settle();
    assert_eq!(pair.spill.spilled_traces(), 1);
    let rest: Vec<SpanId> = got.spans.iter().map(|span| span.span_id).collect();
    let removed = pair.spill.remove_spans(id, &rest);
    assert_eq!(removed, pair.memory.remove_spans(id, &rest));
    assert_eq!(removed.spans, 13);
    assert!(
        pair.spill.trace(id).is_none(),
        "the last span took the trace"
    );
    assert_eq!(pair.spill.spilled_traces(), 0);
    assert_eq!(pair.spill.stats(), pair.memory.stats());
    assert_eq!(pair.spill.stats().traces, 0);
}

#[test]
fn max_traces_evicts_cold_traces_whole() {
    let pair = Pair::with(StoreConfig {
        max_traces: 3,
        ..StoreConfig::default()
    });
    let first = scenario_spans(&Scenario::CheckoutFailure, 1, BASE);
    pair.ingest_spans(&first);
    pair.ingest_spans(&scenario_spans(&Scenario::CheckoutSuccess, 2, BASE + SEC));
    pair.ingest_spans(&scenario_spans(
        &Scenario::CheckoutSuccess,
        3,
        BASE + 2 * SEC,
    ));
    pair.settle();
    assert_eq!(pair.spill.spilled_traces(), 3);

    // A fourth trace pushes the oldest, which is cold, out.
    pair.ingest_spans(&scenario_spans(
        &Scenario::CheckoutSuccess,
        4,
        BASE + 3 * SEC,
    ));
    assert!(pair.spill.trace(trace_id_of(&first)).is_none());
    assert_eq!(pair.spill.stats(), pair.memory.stats());
    assert_eq!(pair.spill.stats().traces, 3);
    assert_eq!(
        pair.spill.spilled_traces(),
        2,
        "three held, one of them hot"
    );
    assert!(pair.spill.spill_dead_bytes() > 0, "its blob is dead");
}

#[test]
fn a_failing_write_leaves_traces_in_memory_and_is_retried() {
    let dir = TempDir::new().unwrap();
    let (store, clock) = spilling_store(dir.path());
    assert_eq!(store.spill_error(), None);

    // The directory goes away under the store, so no segment can be made.
    fs::remove_dir_all(dir.path()).unwrap();
    let first = scenario_spans(&Scenario::CheckoutFailure, 1, BASE);
    store.ingest_spans(first.clone());
    clock.advance(QUIET);
    store.stats();
    assert_eq!(store.spilled_traces(), 0, "nothing could be written");
    assert!(store.spill_error().is_some(), "and the store says why");
    let id = trace_id_of(&first);
    assert_eq!(
        store.trace(id).unwrap().spans.len(),
        15,
        "the trace is still held"
    );

    // Ingest goes on, and the store never panics over it.
    store.ingest_spans(scenario_spans(&Scenario::CheckoutSuccess, 2, BASE + SEC));
    assert_eq!(store.stats().traces, 2);
    assert_eq!(store.spilled_traces(), 0);

    // Space comes back: the next step writes both, and the error is gone.
    fs::create_dir(dir.path()).unwrap();
    clock.advance(QUIET);
    store.stats();
    assert_eq!(store.spilled_traces(), 2);
    assert_eq!(store.spill_error(), None);
    assert_eq!(store.trace(id).unwrap().spans.len(), 15);
    assert_eq!(store.stats().spans, 15 + 14);
}

// ---- damage ------------------------------------------------------------------------

#[test]
fn a_damaged_blob_is_skipped_not_fatal() {
    let dir = TempDir::new().unwrap();
    let (store, clock) = store_with_config(StoreConfig {
        segment_max_bytes: 64 * 1024 * 1024,
        ..StoreConfig::spilling_for_tests(dir.path().to_path_buf())
    });
    let first = scenario_spans(&Scenario::CheckoutFailure, 1, BASE);
    let second = scenario_spans(&Scenario::CheckoutSuccess, 2, BASE + 10 * SEC);
    store.ingest_spans(first.clone());
    clock.advance(QUIET);
    store.ingest_spans(second.clone());
    clock.advance(QUIET);
    store.stats();
    assert_eq!(store.spilled_traces(), 2);

    // Flip a byte inside the first record, which is the first trace's blob,
    // before anything has read it. Its checksum no longer matches.
    let segment = dir.path().join("traces-000000.seg");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&segment)
        .unwrap();
    let mut byte = [0u8; 1];
    file.read_at(&mut byte, 20).unwrap();
    file.write_at(&[byte[0] ^ 0xFF], 20).unwrap();

    // The damaged trace cannot be read; nothing panics, and the other trace
    // and the summaries (which live in the index) are fine.
    let (first_id, second_id) = (trace_id_of(&first), trace_id_of(&second));
    assert!(store.trace(first_id).is_none());
    assert!(store.with_trace_spans(first_id, |_| ()).is_none());
    assert_eq!(store.trace(second_id).unwrap().spans.len(), second.len());
    assert!(store.summary(first_id).is_some());
    let page = store.search_traces(&TraceFilter::default(), &page_all());
    assert_eq!(page.items.len(), 2);
    let mut scanned = Vec::new();
    store.scan_traces(0, |id, _, _| scanned.push(id));
    assert_eq!(
        scanned,
        vec![second_id],
        "an unreadable trace is not scanned"
    );
    // A query needs the spans, so it cannot match the damaged trace.
    let query = TraceFilter {
        query: Some(Arc::new(traceql::parse("{ status = error }").unwrap())),
        ..TraceFilter::default()
    };
    assert!(store.search_traces(&query, &page_all()).items.is_empty());
    let map = store.service_map(TimeWindow::default());
    assert!(!map.nodes.is_empty());

    // A late span for it cannot be joined to what was lost: the trace starts
    // over, from that span, and what could not be read is no longer counted.
    let late = late_span(&first, 0xEE);
    store.ingest_spans(vec![late]);
    assert_eq!(store.trace(first_id).unwrap().spans.len(), 1);
    let stats = store.stats();
    assert_eq!(stats.traces, 2);
    assert_eq!(stats.spans, 1 + second.len());
}

// ---- concurrency -------------------------------------------------------------------

#[test]
fn readers_and_a_writer_share_a_spilling_store() {
    let dir = TempDir::new().unwrap();
    let (store, clock) = spilling_store(dir.path());
    let writer = {
        let store = store.clone();
        let clock = clock.clone();
        std::thread::spawn(move || {
            for seed in 0..30u64 {
                store.ingest_spans(scenario_spans(
                    &Scenario::CheckoutSuccess,
                    200 + seed,
                    BASE + seed * SEC,
                ));
                clock.advance(Duration::from_secs(6));
            }
        })
    };
    let readers: Vec<_> = (0..3)
        .map(|_| {
            let store = store.clone();
            std::thread::spawn(move || {
                for _ in 0..200 {
                    let page = store.search_traces(
                        &TraceFilter::default(),
                        &PageRequest {
                            limit: 10,
                            cursor: None,
                        },
                    );
                    for item in &page.items {
                        // A listed trace is either still held or just gone,
                        // never half there.
                        if let Some(trace) = store.trace(item.trace_id) {
                            assert_eq!(trace.spans.len(), item.span_count);
                        }
                    }
                    store.service_map(TimeWindow::default());
                }
            })
        })
        .collect();
    writer.join().unwrap();
    for reader in readers {
        reader.join().unwrap();
    }
    let stats = store.stats();
    assert_eq!(stats.traces, 30);
    assert_eq!(stats.spans, 30 * 14);
    assert!(store.spilled_traces() > 0);
}
