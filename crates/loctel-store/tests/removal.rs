//! Removing traces, spans and their logs, reading held spans in place, and
//! scanning what the store holds.

mod fixtures;
mod modes;

use fixtures::*;
use loctel_meshgen::Scenario;
use loctel_model::SpanId;
use loctel_store::{LogFilter, PageRequest, Removed, StoreConfig, TraceFilter, TraceStatus};
use modes::{each_mode, each_mode_with};

const BASE: u64 = 2_000_000_000_000;

fn all() -> PageRequest {
    PageRequest {
        limit: usize::MAX,
        cursor: None,
    }
}

fn logs_of_trace(store: &loctel_store::Store, trace: u8) -> usize {
    store
        .search_logs(
            &LogFilter {
                trace_id: Some(tid(trace)),
                ..LogFilter::default()
            },
            &all(),
        )
        .items
        .len()
}

#[test]
fn remove_trace_removes_spans_and_logs() {
    each_mode(|store, _clock| {
        let failure = scenario_spans(&Scenario::CheckoutFailure, 1, BASE);
        let success = scenario_spans(&Scenario::CheckoutSuccess, 2, BASE + 10_000_000_000);
        let failure_id = failure[0].trace_id;
        let success_id = success[0].trace_id;
        store.ingest_spans(failure);
        store.ingest_spans(success);
        store.ingest_logs(scenario_logs(&Scenario::CheckoutFailure, 1, BASE));
        store.ingest_logs(scenario_logs(
            &Scenario::CheckoutSuccess,
            2,
            BASE + 10_000_000_000,
        ));
        // A log with no trace context belongs to no trace.
        store.ingest_logs(vec![log("lonely", "no trace here", 5)]);
        let before = store.stats();
        assert_eq!(before.traces, 2);
        assert_eq!(before.spans, 29);
        let success_logs = store
            .search_logs(
                &LogFilter {
                    trace_id: Some(success_id),
                    ..LogFilter::default()
                },
                &all(),
            )
            .items
            .len();
        assert!(success_logs > 0);

        let removed = store.remove_trace(failure_id);

        assert_eq!(removed, Removed { spans: 15, logs: 7 });
        assert!(store.trace(failure_id).is_none());
        assert!(store.summary(failure_id).is_none());
        let logs = store.search_logs(
            &LogFilter {
                trace_id: Some(failure_id),
                ..LogFilter::default()
            },
            &all(),
        );
        assert!(logs.items.is_empty());

        // Stats follow: what is held goes down, what was received does not.
        let after = store.stats();
        assert_eq!(after.traces, 1);
        assert_eq!(after.spans, 14);
        assert_eq!(after.logs, before.logs - 7);
        assert_eq!(after.spans_received, before.spans_received);
        assert_eq!(after.logs_received, before.logs_received);

        // Everything else is untouched.
        assert_eq!(store.trace(success_id).unwrap().spans.len(), 14);
        assert_eq!(
            store
                .search_logs(
                    &LogFilter {
                        trace_id: Some(success_id),
                        ..LogFilter::default()
                    },
                    &all()
                )
                .items
                .len(),
            success_logs
        );
        assert_eq!(
            store
                .search_logs(&LogFilter::default(), &all())
                .items
                .iter()
                .filter(|log| log.service() == "lonely")
                .count(),
            1
        );

        // A second removal finds nothing.
        assert_eq!(store.remove_trace(failure_id), Removed::default());
    });
}

#[test]
fn remove_trace_also_removes_the_per_span_log_index() {
    // `logs_for_span` reads a per-span index; it must not keep pointing at the
    // removed logs, and a later log for the same span must be found.
    each_mode(|store, _clock| {
        store.ingest_spans(vec![span(1, 1, None, "svc", "root", 0, 10)]);
        store.ingest_logs(vec![in_span(log("svc", "first", 1), 1, 1)]);

        assert_eq!(store.remove_trace(tid(1)), Removed { spans: 1, logs: 1 });
        assert!(store.logs_for_span(tid(1), sid(1)).is_empty());

        store.ingest_logs(vec![in_span(log("svc", "second", 2), 1, 1)]);
        let lines = store.logs_for_span(tid(1), sid(1));
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].body, "second");
    });
}

#[test]
fn remove_trace_removes_logs_with_no_span_and_logs_without_spans_held() {
    each_mode(|store, _clock| {
        // Logs only: no span of trace 1 ever arrived. One has a span ID, one has
        // only the trace ID.
        store.ingest_logs(vec![
            in_span(log("svc", "with span", 1), 1, 7),
            in_trace(log("svc", "trace only", 2), 1),
            in_trace(log("svc", "other trace", 3), 2),
        ]);

        let removed = store.remove_trace(tid(1));

        assert_eq!(removed, Removed { spans: 0, logs: 2 });
        assert_eq!(logs_of_trace(&store, 1), 0);
        assert_eq!(logs_of_trace(&store, 2), 1);
        assert_eq!(store.stats().logs, 1);
    });
}

#[test]
fn removed_logs_are_forgotten_by_dedup() {
    // Dedup remembers the logs the store holds, so a removed log can arrive
    // again (and is counted as received again).
    each_mode(|store, _clock| {
        let line = in_span(log("svc", "again", 1), 1, 1);
        store.ingest_logs(vec![line.clone()]);
        store.remove_trace(tid(1));

        store.ingest_logs(vec![line]);

        assert_eq!(store.stats().logs, 1);
        assert_eq!(store.stats().logs_received, 2);
    });
}

#[test]
fn remove_spans_removes_their_logs() {
    each_mode(|store, _clock| {
        store.ingest_spans(vec![
            span(1, 1, None, "web", "root", 0, 100),
            span(1, 2, Some(1), "web", "child a", 10, 20),
            span(1, 3, Some(1), "web", "child b", 40, 20),
            span(2, 1, None, "web", "other trace, same span id", 0, 10),
        ]);
        store.ingest_logs(vec![
            in_span(log("web", "a1", 11), 1, 2),
            in_span(log("web", "a2", 12), 1, 2),
            in_span(log("web", "b1", 41), 1, 3),
            in_span(log("web", "root", 1), 1, 1),
            in_trace(log("web", "trace only", 2), 1),
            // Same span ID, other trace: must stay.
            in_span(log("web", "other", 3), 2, 2),
        ]);
        let before = store.stats();

        let removed = store.remove_spans(tid(1), &[sid(2)]);

        assert_eq!(removed, Removed { spans: 1, logs: 2 });
        let snapshot = store.trace(tid(1)).unwrap();
        let names: Vec<&str> = snapshot.spans.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["root", "child b"]);
        assert!(store.logs_for_span(tid(1), sid(2)).is_empty());
        assert_eq!(store.logs_for_span(tid(1), sid(3)).len(), 1);
        assert_eq!(store.logs_for_span(tid(1), sid(1)).len(), 1);
        // The trace-only log and the other trace's log are not the span's logs.
        assert_eq!(logs_of_trace(&store, 1), 3);
        assert_eq!(logs_of_trace(&store, 2), 1);
        assert_eq!(store.trace(tid(2)).unwrap().spans.len(), 1);

        let after = store.stats();
        assert_eq!(after.spans, before.spans - 1);
        assert_eq!(after.logs, before.logs - 2);
        assert_eq!(after.spans_received, before.spans_received);
        assert_eq!(after.logs_received, before.logs_received);
        assert_eq!(after.traces, 2);
    });
}

#[test]
fn remove_spans_keeps_the_trace_consistent() {
    each_mode(|store, _clock| {
        store.ingest_spans(vec![
            span(1, 1, None, "web", "root", 5, 100),
            span(1, 2, Some(1), "web", "early", 0, 10), // clock skew: starts first
            span(1, 3, Some(2), "web", "grandchild", 6, 3),
            span(1, 4, Some(1), "web", "late", 50, 500),
        ]);
        let summary = store.summary(tid(1)).unwrap();
        assert_eq!(summary.start_unix_nano, 0);
        assert_eq!(summary.duration_nanos, 550 * 1_000_000);

        // Removing the earliest start and the latest end moves both ends.
        let removed = store.remove_spans(tid(1), &[sid(2), sid(4)]);

        assert_eq!(removed, Removed { spans: 2, logs: 0 });
        let summary = store.summary(tid(1)).unwrap();
        assert_eq!(summary.span_count, 2);
        assert_eq!(summary.start_unix_nano, 5 * 1_000_000);
        assert_eq!(summary.duration_nanos, 100 * 1_000_000);
        // The grandchild's parent is gone, so it is an orphan.
        let snapshot = store.trace(tid(1)).unwrap();
        assert_eq!(snapshot.orphans, vec![sid(3)]);
        assert_eq!(snapshot.root, Some(sid(1)));
        // Search by time uses the updated start.
        let found = store.search_traces(
            &TraceFilter {
                since_unix_nano: Some(5 * 1_000_000),
                ..TraceFilter::default()
            },
            &all(),
        );
        assert_eq!(found.items.len(), 1);

        // Dedup still works on what is held (positions were rebuilt) ...
        let received = store.stats().spans_received;
        store.ingest_spans(vec![span(1, 3, Some(2), "web", "grandchild", 6, 3)]);
        assert_eq!(store.stats().spans, 2);
        assert_eq!(store.stats().spans_received, received);
        // ... and a removed span's ID is free again.
        store.ingest_spans(vec![span(1, 2, Some(1), "web", "early", 0, 10)]);
        assert_eq!(store.stats().spans, 3);
        assert_eq!(store.trace(tid(1)).unwrap().orphans, Vec::<SpanId>::new());
    });
}

#[test]
fn remove_spans_ignores_unknown_and_repeated_ids() {
    each_mode(|store, _clock| {
        store.ingest_spans(vec![
            span(1, 1, None, "web", "root", 0, 10),
            span(1, 2, Some(1), "web", "child", 1, 2),
        ]);
        store.ingest_logs(vec![in_span(log("web", "line", 1), 1, 2)]);

        // 9 is not a span of the trace, 2 is listed twice, and trace 5 is unknown.
        let removed = store.remove_spans(tid(1), &[sid(9), sid(2), sid(2)]);
        assert_eq!(removed, Removed { spans: 1, logs: 1 });
        assert_eq!(store.stats().spans, 1);
        assert_eq!(store.remove_spans(tid(5), &[sid(1)]), Removed::default());
        assert_eq!(store.remove_spans(tid(1), &[]), Removed::default());
        assert_eq!(store.stats().spans, 1);
    });
}

#[test]
fn remove_spans_removes_the_logs_of_spans_that_are_not_held() {
    // Logs and spans are stored independently: a span can be gone (or never
    // arrived) while its logs stay. Asking to remove the span removes them.
    each_mode(|store, _clock| {
        store.ingest_spans(vec![span(1, 1, None, "web", "root", 0, 10)]);
        store.ingest_logs(vec![
            in_span(log("web", "orphan log", 1), 1, 8),
            in_span(log("web", "root log", 2), 1, 1),
        ]);

        let removed = store.remove_spans(tid(1), &[sid(8)]);

        assert_eq!(removed, Removed { spans: 0, logs: 1 });
        assert!(store.logs_for_span(tid(1), sid(8)).is_empty());
        assert_eq!(store.logs_for_span(tid(1), sid(1)).len(), 1);
        assert_eq!(store.stats().spans, 1);
    });
}

#[test]
fn removing_last_span_removes_trace() {
    each_mode(|store, _clock| {
        store.ingest_spans(vec![
            span(1, 1, None, "web", "root", 0, 10),
            span(2, 1, None, "web", "other", 0, 10),
        ]);
        store.ingest_logs(vec![
            in_span(log("web", "root log", 1), 1, 1),
            in_trace(log("web", "trace-level log", 2), 1),
        ]);

        let removed = store.remove_spans(tid(1), &[sid(1)]);

        // The span's logs go with it. The log that names the trace but no span is
        // not a span's log, so it stays.
        assert_eq!(removed, Removed { spans: 1, logs: 1 });
        assert!(store.trace(tid(1)).is_none());
        assert!(store.summary(tid(1)).is_none());
        assert_eq!(store.stats().traces, 1);
        assert_eq!(store.stats().spans, 1);
        assert_eq!(logs_of_trace(&store, 1), 1);
        // The trace is gone from searches too.
        let found = store.search_traces(&TraceFilter::default(), &all());
        assert_eq!(found.items.len(), 1);
        assert_eq!(found.items[0].trace_id, tid(2));
    });
}

#[test]
fn removing_every_span_of_a_trace_in_one_call_removes_it() {
    each_mode(|store, _clock| {
        store.ingest_spans(vec![
            span(1, 1, None, "web", "root", 0, 10),
            span(1, 2, Some(1), "web", "child", 1, 2),
            span(1, 3, Some(1), "web", "child", 4, 2),
        ]);

        let removed = store.remove_spans(tid(1), &[sid(1), sid(2), sid(3)]);

        assert_eq!(removed.spans, 3);
        assert!(store.trace(tid(1)).is_none());
        assert_eq!(store.stats().traces, 0);
        assert_eq!(store.stats().spans, 0);
    });
}

#[test]
fn eviction_after_removal_still_drops_the_oldest_held_trace() {
    let config = StoreConfig {
        max_traces: 2,
        ..StoreConfig::default()
    };
    each_mode_with(config, |store, _clock| {
        let root = |trace: u8| span(trace, 1, None, "web", "root", 0, 10);

        store.ingest_spans(vec![root(1)]);
        store.ingest_spans(vec![root(2)]);
        store.remove_trace(tid(1));
        // Held: 2. Adding 3 and 4 goes over the cap by one, so the oldest held
        // trace (2, not the removed 1) is evicted.
        store.ingest_spans(vec![root(3)]);
        store.ingest_spans(vec![root(4)]);

        assert!(store.trace(tid(2)).is_none());
        assert!(store.trace(tid(3)).is_some());
        assert!(store.trace(tid(4)).is_some());
        assert_eq!(store.stats().traces, 2);
    });
}

#[test]
fn a_removed_trace_that_comes_back_is_the_newest() {
    let config = StoreConfig {
        max_traces: 2,
        ..StoreConfig::default()
    };
    each_mode_with(config, |store, _clock| {
        let root = |trace: u8| span(trace, 1, None, "web", "root", 0, 10);

        store.ingest_spans(vec![root(1)]);
        store.ingest_spans(vec![root(2)]);
        store.remove_trace(tid(1));
        store.ingest_spans(vec![root(1)]); // back, now newer than 2
        store.ingest_spans(vec![root(3)]); // over the cap: 2 is the oldest

        assert!(store.trace(tid(2)).is_none());
        assert!(store.trace(tid(1)).is_some());
        assert!(store.trace(tid(3)).is_some());
    });
}

#[test]
fn log_eviction_still_works_after_removal() {
    // Eviction takes the oldest log and expects it at the front of its span's
    // and trace's index: removal must leave those indexes in order.
    let config = StoreConfig {
        max_log_records: 4,
        ..StoreConfig::default()
    };
    each_mode_with(config, |store, _clock| {
        store.ingest_logs(vec![
            in_span(log("web", "a1", 1), 1, 1),
            in_span(log("web", "b1", 2), 1, 2),
            in_span(log("web", "a2", 3), 1, 1),
            in_span(log("web", "b2", 4), 1, 2),
        ]);
        store.remove_spans(tid(1), &[sid(1)]);
        assert_eq!(store.stats().logs, 2);

        // Four more logs in the same trace push out the older ones.
        store.ingest_logs(vec![
            in_span(log("web", "b3", 5), 1, 2),
            in_span(log("web", "c1", 6), 1, 3),
            in_trace(log("web", "t1", 7), 1),
            in_span(log("web", "c2", 8), 1, 3),
        ]);

        assert_eq!(store.stats().logs, 4);
        let bodies: Vec<String> = store
            .search_logs(&LogFilter::default(), &all())
            .items
            .into_iter()
            .map(|log| log.body)
            .collect();
        assert_eq!(bodies, vec!["c2", "t1", "c1", "b3"]);
        assert_eq!(store.logs_for_span(tid(1), sid(2)).len(), 1);
        assert_eq!(logs_of_trace(&store, 1), 4);
        // And the trace index is gone once its logs are.
        store.remove_trace(tid(1));
        assert_eq!(store.stats().logs, 0);
    });
}

#[test]
fn with_trace_spans_sees_held_spans() {
    each_mode(|store, _clock| {
        store.ingest_spans(vec![
            span(1, 3, Some(1), "web", "later", 20, 5),
            span(1, 1, None, "web", "root", 0, 100),
            span(1, 2, Some(1), "web", "earlier", 10, 5),
        ]);

        // The spans as held, without a copy: arrival order, not start order.
        let names = store.with_trace_spans(tid(1), |spans| {
            spans.iter().map(|s| s.name.clone()).collect::<Vec<_>>()
        });
        assert_eq!(
            names,
            Some(vec![
                "later".to_owned(),
                "root".to_owned(),
                "earlier".to_owned()
            ])
        );

        // Its result is whatever the closure returns.
        assert_eq!(store.with_trace_spans(tid(1), |spans| spans.len()), Some(3));

        // An unknown trace: `None`, and the closure is not run.
        let mut ran = false;
        let missing = store.with_trace_spans(tid(9), |_| {
            ran = true;
        });
        assert_eq!(missing, None);
        assert!(!ran);

        // It sees removals.
        store.remove_spans(tid(1), &[sid(2)]);
        assert_eq!(store.with_trace_spans(tid(1), |spans| spans.len()), Some(2));
        store.remove_trace(tid(1));
        assert_eq!(store.with_trace_spans(tid(1), |spans| spans.len()), None);
    });
}

#[test]
fn scan_traces_respects_since() {
    each_mode(|store, _clock| {
        // Traces starting at 100 ms, 200 ms, 300 ms (the second one failing),
        // ingested out of start order.
        store.ingest_spans(vec![span(3, 1, None, "web", "third", 300, 10)]);
        store.ingest_spans(vec![error(span(2, 1, None, "web", "second", 200, 10))]);
        store.ingest_spans(vec![
            span(1, 1, None, "web", "first", 100, 10),
            span(1, 2, Some(1), "web", "child", 105, 2),
        ]);

        let scan = |since_ms: u64| {
            let mut seen: Vec<(u8, usize, TraceStatus)> = Vec::new();
            store.scan_traces(since_ms * 1_000_000, |id, spans, status| {
                assert!(spans.iter().all(|span| span.trace_id == id));
                seen.push((id.0[0], spans.len(), status));
            });
            seen
        };

        // Everything from time 0, in the order the traces first arrived.
        assert_eq!(
            scan(0),
            vec![
                (3, 1, TraceStatus::Ok),
                (2, 1, TraceStatus::Failed),
                (1, 2, TraceStatus::Ok),
            ]
        );
        // `since` is inclusive and is about the trace's start.
        let ids = |since_ms: u64| scan(since_ms).iter().map(|s| s.0).collect::<Vec<_>>();
        assert_eq!(ids(100), vec![3, 2, 1]);
        assert_eq!(ids(101), vec![3, 2]);
        assert_eq!(ids(200), vec![3, 2]);
        assert_eq!(ids(201), vec![3]);
        assert_eq!(ids(300), vec![3]);
        assert!(ids(301).is_empty());
    });
}

#[test]
fn scan_traces_status_follows_the_failure_rules() {
    each_mode(|store, _clock| {
        let failure = scenario_spans(&Scenario::CheckoutFailure, 1, BASE);
        let success = scenario_spans(&Scenario::CheckoutSuccess, 2, BASE);
        let (failure_id, success_id) = (failure[0].trace_id, success[0].trace_id);
        store.ingest_spans(failure);
        store.ingest_spans(success);

        let mut statuses = Vec::new();
        store.scan_traces(0, |id, _spans, status| statuses.push((id, status)));

        statuses.sort_by_key(|(id, _)| *id);
        let mut expected = vec![
            (failure_id, TraceStatus::Failed),
            (success_id, TraceStatus::Ok),
        ];
        expected.sort_by_key(|(id, _)| *id);
        assert_eq!(statuses, expected);
    });
}

#[test]
fn scan_logs_respects_since() {
    each_mode(|store, _clock| {
        // Out of time order, one with no timestamp (so its observed time counts).
        store.ingest_logs(vec![
            log("web", "at 300", 300),
            log("web", "at 100", 100),
            log("web", "at 200", 200),
        ]);
        let mut untimed = log("web", "observed at 250", 250);
        untimed.time_unix_nano = 0;
        store.ingest_logs(vec![untimed]);

        let scan = |since_ms: u64| {
            let mut bodies: Vec<String> = Vec::new();
            store.scan_logs(since_ms * 1_000_000, |log| bodies.push(log.body.clone()));
            bodies
        };

        // Oldest first.
        assert_eq!(
            scan(0),
            vec!["at 100", "at 200", "observed at 250", "at 300"]
        );
        // Inclusive.
        assert_eq!(scan(200), vec!["at 200", "observed at 250", "at 300"]);
        assert_eq!(scan(201), vec!["observed at 250", "at 300"]);
        assert_eq!(scan(300), vec!["at 300"]);
        assert!(scan(301).is_empty());
    });
}

#[test]
fn scan_logs_sees_removals() {
    each_mode(|store, _clock| {
        store.ingest_logs(vec![
            in_span(log("web", "gone", 1), 1, 1),
            log("web", "kept", 2),
        ]);
        store.remove_trace(tid(1));

        let mut bodies: Vec<String> = Vec::new();
        store.scan_logs(0, |log| bodies.push(log.body.clone()));

        assert_eq!(bodies, vec!["kept"]);
    });
}
