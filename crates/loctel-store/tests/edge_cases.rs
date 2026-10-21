//! Edge cases around the plan's Task 3 interfaces. The plan's own tests live
//! in `assembly.rs`; these pin the behaviour later tasks and crates rely on.

mod fixtures;

use std::thread;
use std::time::Duration;

use fixtures::*;
use loctel_model::StatusCode;
use loctel_store::{FailedIn, Store, StoreConfig, TraceStatus};

fn ids(spans: &[loctel_model::Span]) -> Vec<loctel_model::SpanId> {
    spans.iter().map(|s| s.span_id).collect()
}

// --- lookup and ordering ---

#[test]
fn unknown_trace_is_none() {
    let (store, _c) = store_with_manual_clock();
    assert!(store.trace(tid(9)).is_none());
}

#[test]
fn spans_sorted_by_start_then_span_id() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_spans(vec![
        span(1, 5, Some(1), "a", "late", 30, 1),
        span(1, 4, Some(1), "a", "tie-high-id", 10, 1),
        span(1, 1, None, "a", "root", 0, 50),
        span(1, 3, Some(1), "a", "tie-low-id", 10, 1),
    ]);
    let t = store.trace(tid(1)).unwrap();
    assert_eq!(ids(&t.spans), vec![sid(1), sid(3), sid(4), sid(5)]);
}

#[test]
fn snapshot_is_a_copy_that_later_ingest_does_not_change() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_spans(vec![span(1, 1, None, "a", "x", 0, 1)]);
    let before = store.trace(tid(1)).unwrap();
    store.ingest_spans(vec![span(1, 2, Some(1), "a", "y", 0, 1)]);
    assert_eq!(before.spans.len(), 1);
    assert_eq!(store.trace(tid(1)).unwrap().spans.len(), 2);
}

#[test]
fn trace_snapshot_carries_its_id() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_spans(vec![span(7, 1, None, "a", "x", 0, 1)]);
    assert_eq!(store.trace(tid(7)).unwrap().trace_id, tid(7));
}

#[test]
fn spans_of_different_traces_stay_apart() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_spans(vec![
        span(1, 1, None, "a", "one", 0, 1),
        span(2, 1, None, "a", "two", 0, 1),
    ]);
    assert_eq!(store.trace(tid(1)).unwrap().spans[0].name, "one");
    assert_eq!(store.trace(tid(2)).unwrap().spans[0].name, "two");
    assert_eq!(store.stats().traces, 2);
    assert_eq!(store.stats().spans, 2);
}

// --- root, orphans, children ---

#[test]
fn earliest_root_wins_when_several() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_spans(vec![
        span(1, 3, None, "a", "later root", 20, 5),
        span(1, 2, None, "a", "earliest root", 10, 5),
        span(1, 4, Some(2), "a", "child", 11, 1),
    ]);
    assert_eq!(store.trace(tid(1)).unwrap().root, Some(sid(2)));
}

#[test]
fn equal_start_roots_pick_the_lower_span_id() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_spans(vec![
        span(1, 9, None, "a", "high", 10, 5),
        span(1, 2, None, "a", "low", 10, 5),
    ]);
    assert_eq!(store.trace(tid(1)).unwrap().root, Some(sid(2)));
}

#[test]
fn trace_without_a_root_has_none_and_lists_orphans_in_order() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_spans(vec![
        span(1, 3, Some(1), "a", "b", 20, 1),
        span(1, 2, Some(1), "a", "a", 10, 1),
        span(1, 4, Some(2), "a", "grandchild of the missing root", 11, 1),
    ]);
    let t = store.trace(tid(1)).unwrap();
    assert_eq!(t.root, None);
    // Span 4's parent (2) is present, so it is not an orphan.
    assert_eq!(t.orphans, vec![sid(2), sid(3)]);
}

#[test]
fn orphan_is_cleared_when_its_parent_arrives_even_in_the_same_batch() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_spans(vec![
        span(1, 2, Some(1), "a", "child", 5, 1),
        span(1, 1, None, "a", "root", 0, 10),
    ]);
    assert!(store.trace(tid(1)).unwrap().orphans.is_empty());
}

#[test]
fn children_lists_direct_children_in_snapshot_order() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_spans(vec![
        span(1, 1, None, "a", "root", 0, 50),
        span(1, 3, Some(1), "a", "second", 20, 5),
        span(1, 2, Some(1), "a", "first", 10, 5),
        span(1, 4, Some(2), "a", "grandchild", 11, 1),
    ]);
    let t = store.trace(tid(1)).unwrap();
    let of_root: Vec<_> = t.children(sid(1)).map(|s| s.span_id).collect();
    assert_eq!(of_root, vec![sid(2), sid(3)]);
    let of_first: Vec<_> = t.children(sid(2)).map(|s| s.span_id).collect();
    assert_eq!(of_first, vec![sid(4)]);
    assert_eq!(t.children(sid(4)).count(), 0);
    assert_eq!(t.children(sid(99)).count(), 0);
}

#[test]
fn span_that_is_its_own_parent_does_not_panic() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_spans(vec![span(1, 1, Some(1), "a", "loop", 0, 1)]);
    let t = store.trace(tid(1)).unwrap();
    assert_eq!(t.spans.len(), 1);
    assert_eq!(t.root, None);
    assert!(t.orphans.is_empty());
}

// --- dedup ---

#[test]
fn duplicate_within_one_batch_is_ignored() {
    let (store, _c) = store_with_manual_clock();
    let s = span(1, 1, None, "a", "x", 0, 1);
    store.ingest_spans(vec![s.clone(), s.clone(), s]);
    assert_eq!(store.trace(tid(1)).unwrap().spans.len(), 1);
    assert_eq!(store.stats().spans, 1);
}

#[test]
fn first_copy_of_a_duplicate_wins() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_spans(vec![span(1, 1, None, "a", "first", 0, 1)]);
    store.ingest_spans(vec![span(1, 1, None, "a", "second", 0, 1)]);
    assert_eq!(store.trace(tid(1)).unwrap().spans[0].name, "first");
}

#[test]
fn duplicate_is_counted_once() {
    let (store, _c) = store_with_manual_clock();
    let s = span(1, 1, None, "a", "x", 0, 1);
    store.ingest_spans(vec![s.clone()]);
    store.ingest_spans(vec![s]);
    assert_eq!(store.stats().spans_received, 1);
}

#[test]
fn same_span_id_in_two_traces_is_not_a_duplicate() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_spans(vec![
        span(1, 1, None, "a", "x", 0, 1),
        span(2, 1, None, "a", "x", 0, 1),
    ]);
    assert_eq!(store.stats().spans, 2);
    assert_eq!(store.stats().spans_received, 2);
}

#[test]
fn duplicate_does_not_make_a_complete_trace_incomplete() {
    let (store, clock) = store_with_manual_clock();
    let s = span(1, 1, None, "a", "x", 0, 1);
    store.ingest_spans(vec![s.clone()]);
    clock.advance(Duration::from_secs(10));
    assert!(store.trace(tid(1)).unwrap().complete);
    // An exporter retry arrives late: no new span, so still complete.
    store.ingest_spans(vec![s]);
    assert!(store.trace(tid(1)).unwrap().complete);
}

// --- completeness ---

#[test]
fn completeness_counts_from_the_last_new_span() {
    let (store, clock) = store_with_manual_clock();
    store.ingest_spans(vec![span(1, 1, None, "a", "x", 0, 1)]);
    clock.advance(Duration::from_secs(8));
    store.ingest_spans(vec![span(1, 2, Some(1), "a", "y", 0, 1)]);
    clock.advance(Duration::from_secs(8));
    // 16 s since the first span, but only 8 s since the last one.
    assert!(!store.trace(tid(1)).unwrap().complete);
    clock.advance(Duration::from_secs(2));
    assert!(store.trace(tid(1)).unwrap().complete);
}

#[test]
fn completeness_is_tracked_per_trace() {
    let (store, clock) = store_with_manual_clock();
    store.ingest_spans(vec![span(1, 1, None, "a", "x", 0, 1)]);
    clock.advance(Duration::from_secs(6));
    store.ingest_spans(vec![span(2, 1, None, "a", "y", 0, 1)]);
    clock.advance(Duration::from_secs(5));
    assert!(store.trace(tid(1)).unwrap().complete);
    assert!(!store.trace(tid(2)).unwrap().complete);
}

#[test]
fn completeness_quiet_period_is_configurable() {
    let (store, clock) = store_with_config(StoreConfig {
        completeness_quiet: Duration::from_millis(50),
        ..StoreConfig::default()
    });
    store.ingest_spans(vec![span(1, 1, None, "a", "x", 0, 1)]);
    clock.advance(Duration::from_millis(49));
    assert!(!store.trace(tid(1)).unwrap().complete);
    clock.advance(Duration::from_millis(1));
    assert!(store.trace(tid(1)).unwrap().complete);
}

#[test]
fn completeness_uses_arrival_time_not_span_timestamps() {
    let (store, _c) = store_with_manual_clock();
    // A span that "happened" long before the store's clock now.
    store.ingest_spans(vec![span(1, 1, None, "a", "ancient", 0, 1)]);
    assert!(!store.trace(tid(1)).unwrap().complete);
}

#[test]
fn clock_stepping_backwards_never_panics_and_is_not_complete() {
    let (store, clock) = store_with_manual_clock();
    store.ingest_spans(vec![span(1, 1, None, "a", "x", 0, 1)]);
    clock.set(CLOCK_START - 1_000);
    assert!(!store.trace(tid(1)).unwrap().complete);
}

// --- placeholders for Task 4 ---

#[test]
fn status_and_failed_in_types_are_declared() {
    // Task 4 computes these on read. Task 3 only declares them, so a
    // snapshot always carries a value of each type.
    let (store, _c) = store_with_manual_clock();
    let mut erroring = span(1, 1, None, "a", "x", 0, 1);
    erroring.status = StatusCode::Error;
    store.ingest_spans(vec![erroring]);
    let t = store.trace(tid(1)).unwrap();
    let _status: TraceStatus = t.status;
    let _failed_in: Option<FailedIn> = t.failed_in;
}

// --- eviction ---

fn two_trace_store() -> (Store, std::sync::Arc<loctel_store::ManualClock>) {
    store_with_config(StoreConfig {
        max_traces: 2,
        ..StoreConfig::default()
    })
}

#[test]
fn eviction_is_by_first_arrival_not_last_activity() {
    let (store, _c) = two_trace_store();
    store.ingest_spans(vec![span(1, 1, None, "a", "x", 0, 1)]);
    store.ingest_spans(vec![span(2, 1, None, "a", "x", 0, 1)]);
    // Trace 1 is the busiest and most recent, but it arrived first.
    store.ingest_spans(vec![span(1, 2, Some(1), "a", "y", 0, 1)]);
    store.ingest_spans(vec![span(3, 1, None, "a", "x", 0, 1)]);
    assert!(store.trace(tid(1)).is_none());
    assert!(store.trace(tid(2)).is_some());
    assert!(store.trace(tid(3)).is_some());
}

#[test]
fn batch_is_ingested_before_eviction_so_no_fragments_survive() {
    let (store, _c) = two_trace_store();
    // Trace 1 spans the whole batch. Evicting while the batch is still being
    // applied would drop trace 1 and then resurrect it as a fragment.
    store.ingest_spans(vec![
        span(1, 1, None, "a", "x", 0, 1),
        span(2, 1, None, "a", "x", 0, 1),
        span(3, 1, None, "a", "x", 0, 1),
        span(1, 2, Some(1), "a", "y", 0, 1),
    ]);
    assert!(store.trace(tid(1)).is_none());
    assert_eq!(store.trace(tid(2)).unwrap().spans.len(), 1);
    assert_eq!(store.trace(tid(3)).unwrap().spans.len(), 1);
    assert_eq!(store.stats().traces, 2);
    assert_eq!(store.stats().spans, 2);
}

#[test]
fn eviction_keeps_span_count_in_step() {
    let (store, _c) = two_trace_store();
    store.ingest_spans(vec![
        span(1, 1, None, "a", "x", 0, 1),
        span(1, 2, Some(1), "a", "y", 0, 1),
        span(1, 3, Some(1), "a", "z", 0, 1),
        span(2, 1, None, "a", "x", 0, 1),
        span(3, 1, None, "a", "x", 0, 1),
    ]);
    // Trace 1 (3 spans) is gone; traces 2 and 3 (1 span each) remain.
    assert_eq!(store.stats().spans, 2);
    // Received counts everything accepted, evicted or not.
    assert_eq!(store.stats().spans_received, 5);
}

#[test]
fn late_span_of_an_evicted_trace_starts_a_fresh_trace() {
    let (store, _c) = two_trace_store();
    store.ingest_spans(vec![span(1, 1, None, "a", "x", 0, 1)]);
    store.ingest_spans(vec![span(2, 1, None, "a", "x", 0, 1)]);
    store.ingest_spans(vec![span(3, 1, None, "a", "x", 0, 1)]);
    assert!(store.trace(tid(1)).is_none());
    // The straggler cannot be matched to the dropped spans. It forms a new
    // (partial) trace, which in turn evicts the now-oldest trace.
    store.ingest_spans(vec![span(1, 2, Some(1), "a", "late", 0, 1)]);
    let t = store.trace(tid(1)).unwrap();
    assert_eq!(t.spans.len(), 1);
    assert_eq!(t.orphans, vec![sid(2)]);
    assert!(store.trace(tid(2)).is_none());
    assert_eq!(store.stats().traces, 2);
}

#[test]
fn nothing_is_evicted_at_the_limit() {
    let (store, _c) = two_trace_store();
    store.ingest_spans(vec![
        span(1, 1, None, "a", "x", 0, 1),
        span(2, 1, None, "a", "x", 0, 1),
    ]);
    assert_eq!(store.stats().traces, 2);
    assert!(store.trace(tid(1)).is_some());
}

#[test]
fn many_traces_stay_within_the_cap() {
    let (store, _c) = store_with_config(StoreConfig {
        max_traces: 10,
        ..StoreConfig::default()
    });
    for n in 1..=200u8 {
        store.ingest_spans(vec![span(n, 1, None, "a", "x", 0, 1)]);
    }
    assert_eq!(store.stats().traces, 10);
    assert_eq!(store.stats().spans, 10);
    // The ten newest (191 to 200) survive.
    assert!(store.trace(tid(200)).is_some());
    assert!(store.trace(tid(191)).is_some());
    assert!(store.trace(tid(190)).is_none());
}

// --- clear and reuse ---

#[test]
fn store_works_after_clear() {
    let (store, _c) = two_trace_store();
    store.ingest_spans(vec![span(1, 1, None, "a", "x", 0, 1)]);
    store.clear();
    store.ingest_spans(vec![span(1, 1, None, "a", "x", 0, 1)]);
    assert_eq!(store.stats().spans, 1);
    assert_eq!(store.stats().spans_received, 2);
    assert!(store.trace(tid(1)).is_some());
}

#[test]
fn clear_resets_eviction_order() {
    let (store, _c) = two_trace_store();
    store.ingest_spans(vec![span(1, 1, None, "a", "x", 0, 1)]);
    store.clear();
    store.ingest_spans(vec![span(2, 1, None, "a", "x", 0, 1)]);
    store.ingest_spans(vec![span(3, 1, None, "a", "x", 0, 1)]);
    // Nothing stale in the arrival queue: no eviction yet.
    assert!(store.trace(tid(2)).is_some());
    assert!(store.trace(tid(3)).is_some());
}

#[test]
fn empty_batch_is_a_no_op() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_spans(vec![]);
    let stats = store.stats();
    assert_eq!(stats.traces, 0);
    assert_eq!(stats.spans_received, 0);
}

// --- sharing ---

#[test]
fn store_is_cheap_to_clone_and_shares_state() {
    let (store, _c) = store_with_manual_clock();
    let other = store.clone();
    store.ingest_spans(vec![span(1, 1, None, "a", "x", 0, 1)]);
    assert_eq!(other.stats().spans, 1);
    other.clear();
    assert_eq!(store.stats().spans, 0);
}

#[test]
fn concurrent_ingest_from_many_threads() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Store>();

    let (store, _c) = store_with_manual_clock();
    let handles: Vec<_> = (0..4u8)
        .map(|worker| {
            let store = store.clone();
            thread::spawn(move || {
                for n in 0..50u8 {
                    // Every worker ingests the same trace IDs with
                    // worker-specific span IDs, plus one shared duplicate.
                    store.ingest_spans(vec![
                        span(n, 100 + worker, None, "a", "x", 0, 1),
                        span(n, 200, None, "a", "shared", 0, 1),
                    ]);
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    let stats = store.stats();
    assert_eq!(stats.traces, 50);
    // Per trace: 4 worker spans plus the one shared span (deduplicated).
    assert_eq!(stats.spans, 50 * 5);
    assert_eq!(stats.spans_received, 50 * 5);
    assert_eq!(store.trace(tid(7)).unwrap().spans.len(), 5);
}
