//! Trace assembly, completeness, dedup and eviction (plan Task 3).
//!
//! The review-focus items owned by this task are `duplicate_span_is_ignored`
//! (an exporter retry resends identical spans: stored once) and
//! `skewed_child_still_assembles` (clock skew between containers: no panic,
//! the child still sits under its parent).

mod fixtures;
mod modes;

use std::time::Duration;

use fixtures::*;
use loctel_store::StoreConfig;
use modes::{each_mode, each_mode_with};

#[test]
fn late_child_before_parent_assembles() {
    each_mode(|store, _clock| {
        store.ingest_spans(vec![span(
            1,
            2,
            Some(1),
            "orders-api",
            "POST /orders",
            5,
            10,
        )]);
        assert_eq!(store.trace(tid(1)).unwrap().orphans, vec![sid(2)]);
        store.ingest_spans(vec![span(
            1,
            1,
            None,
            "web-bff",
            "POST /api/checkout",
            0,
            20,
        )]);
        let t = store.trace(tid(1)).unwrap();
        assert_eq!(t.spans.len(), 2);
        assert_eq!(t.root, Some(sid(1)));
        assert!(t.orphans.is_empty());
    });
}

#[test]
fn complete_after_quiet_period() {
    each_mode(|store, clock| {
        // quiet = 10s
        store.ingest_spans(vec![span(1, 1, None, "a", "x", 0, 1)]);
        clock.advance(Duration::from_secs(9));
        assert!(!store.trace(tid(1)).unwrap().complete);
        clock.advance(Duration::from_secs(1));
        assert!(store.trace(tid(1)).unwrap().complete);
        store.ingest_spans(vec![span(1, 2, Some(1), "a", "y", 0, 1)]);
        assert!(!store.trace(tid(1)).unwrap().complete);
    });
}

#[test]
fn duplicate_span_is_ignored() {
    each_mode(|store, _c| {
        let s = span(1, 1, None, "a", "x", 0, 1);
        store.ingest_spans(vec![s.clone()]);
        store.ingest_spans(vec![s]);
        assert_eq!(store.trace(tid(1)).unwrap().spans.len(), 1);
        assert_eq!(store.stats().spans, 1);
    });
}

#[test]
fn skewed_child_still_assembles() {
    each_mode(|store, _c| {
        // The child starts before its parent, and its end is before its own
        // start: both effects of clock skew between containers.
        let parent = span(1, 1, None, "web-bff", "POST /api/checkout", 100, 20);
        let mut child = span(1, 2, Some(1), "orders-api", "POST /orders", 50, 0);
        child.end_unix_nano = child.start_unix_nano - 10_000_000;
        store.ingest_spans(vec![child, parent]);

        let t = store.trace(tid(1)).unwrap();
        assert_eq!(t.root, Some(sid(1)));
        assert!(t.orphans.is_empty());
        let kids: Vec<_> = t.children(sid(1)).collect();
        assert_eq!(kids.len(), 1);
        assert_eq!(kids[0].span_id, sid(2));
        assert_eq!(kids[0].duration_nanos(), 0);
        // Sorted by start time, the child comes first even though it is the
        // parent's child.
        assert_eq!(t.spans[0].span_id, sid(2));
    });
}

#[test]
fn evicts_oldest_whole_traces() {
    each_mode_with(
        StoreConfig {
            max_traces: 2,
            ..StoreConfig::default()
        },
        |store, _c| {
            for trace in 1..=3 {
                store.ingest_spans(vec![
                    span(trace, 1, None, "a", "root", 0, 10),
                    span(trace, 2, Some(1), "a", "child", 1, 5),
                ]);
            }
            assert!(store.trace(tid(1)).is_none());
            assert_eq!(store.trace(tid(2)).unwrap().spans.len(), 2);
            assert_eq!(store.trace(tid(3)).unwrap().spans.len(), 2);
            let stats = store.stats();
            assert_eq!(stats.traces, 2);
            assert_eq!(stats.spans, 4);
        },
    );
}

#[test]
fn clear_empties_store() {
    each_mode(|store, _c| {
        store.ingest_spans(vec![
            span(1, 1, None, "a", "x", 0, 1),
            span(2, 1, None, "a", "y", 0, 1),
            span(2, 2, Some(1), "a", "z", 0, 1),
        ]);
        assert_eq!(store.stats().traces, 2);
        store.clear();
        let stats = store.stats();
        assert_eq!(stats.traces, 0);
        assert_eq!(stats.spans, 0);
        assert_eq!(stats.logs, 0);
        // The received counters are lifetime totals, so clearing keeps them.
        assert_eq!(stats.spans_received, 3);
        assert_eq!(stats.logs_received, 0);
        assert!(store.trace(tid(1)).is_none());
        assert!(store.trace(tid(2)).is_none());
    });
}
