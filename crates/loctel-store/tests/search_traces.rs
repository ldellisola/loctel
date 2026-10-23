//! Trace summaries and trace search (plan Task 4, design 3.2 and 9.4).
//!
//! The plan's own tests come first (`newest_first` to
//! `cursor_pages_without_duplicates`), then paging corners and the summary
//! fields.

mod fixtures;
mod modes;

use std::time::Duration;

use fixtures::*;
use loctel_model::TraceId;
use loctel_store::{Page, PageRequest, Store, StoreConfig, TraceFilter, TraceStatus, TraceSummary};
use modes::{each_mode, each_mode_with};

const MS: u64 = 1_000_000;

fn page(limit: usize) -> PageRequest {
    PageRequest {
        limit,
        cursor: None,
    }
}

fn search(store: &Store, filter: &TraceFilter, limit: usize) -> Page<TraceSummary> {
    store.search_traces(filter, &page(limit))
}

fn ids(page: &Page<TraceSummary>) -> Vec<TraceId> {
    page.items.iter().map(|t| t.trace_id).collect()
}

/// One root span per trace, `(trace, start_ms, dur_ms)`.
fn ingest_roots(store: &Store, traces: &[(u8, u64, u64)]) {
    for &(trace, start_ms, dur_ms) in traces {
        store.ingest_spans(vec![span(
            trace, 1, None, "web-bff", "GET /", start_ms, dur_ms,
        )]);
    }
}

// --- the plan's tests ---

#[test]
fn newest_first() {
    each_mode(|store, _c| {
        ingest_roots(&store, &[(1, 0, 10), (2, 100, 10), (3, 50, 10)]);
        let result = search(&store, &TraceFilter::default(), 10);
        assert_eq!(ids(&result), vec![tid(2), tid(3), tid(1)]);
        assert_eq!(result.next_cursor, None);
    });
}

#[test]
fn errors_only_includes_both_kinds() {
    each_mode(|store, _c| {
        // 1: fine. 2: the root is erroring (failed). 3: the root is fine but a
        // child is erroring (errors inside).
        store.ingest_spans(vec![span(1, 1, None, "web-bff", "GET /", 0, 10)]);
        store.ingest_spans(vec![
            error(span(2, 1, None, "web-bff", "GET /", 100, 10)),
            error(span(2, 2, Some(1), "payments-api", "POST /charges", 101, 5)),
        ]);
        store.ingest_spans(vec![
            span(3, 1, None, "web-bff", "GET /", 200, 10),
            error(span(3, 2, Some(1), "payments-api", "POST /charges", 201, 5)),
        ]);
        let filter = TraceFilter {
            errors_only: true,
            ..TraceFilter::default()
        };
        let result = search(&store, &filter, 10);
        assert_eq!(ids(&result), vec![tid(3), tid(2)]);
        assert_eq!(result.items[0].status, TraceStatus::ErrorsInside);
        assert_eq!(result.items[1].status, TraceStatus::Failed);
    });
}

#[test]
fn service_filter_matches_any_span() {
    each_mode(|store, _c| {
        store.ingest_spans(vec![
            span(1, 1, None, "web-bff", "GET /", 0, 10),
            span(1, 2, Some(1), "payments-api", "POST /charges", 1, 5),
        ]);
        store.ingest_spans(vec![span(2, 1, None, "web-bff", "GET /", 100, 10)]);
        let filter = TraceFilter {
            service: Some("payments-api".to_string()),
            ..TraceFilter::default()
        };
        // Trace 1 matches through a child span, although its root is web-bff.
        assert_eq!(ids(&search(&store, &filter, 10)), vec![tid(1)]);

        let nobody = TraceFilter {
            service: Some("nope".to_string()),
            ..TraceFilter::default()
        };
        assert!(search(&store, &nobody, 10).items.is_empty());
    });
}

#[test]
fn min_duration_filter() {
    each_mode(|store, _c| {
        ingest_roots(&store, &[(1, 0, 10), (2, 100, 100), (3, 200, 1000)]);
        let filter = TraceFilter {
            min_duration_nanos: Some(100 * MS),
            ..TraceFilter::default()
        };
        // The bound is inclusive: exactly 100 ms is kept.
        assert_eq!(ids(&search(&store, &filter, 10)), vec![tid(3), tid(2)]);
    });
}

#[test]
fn since_filter_excludes_older() {
    each_mode(|store, _c| {
        ingest_roots(&store, &[(1, 0, 10), (2, 100, 10), (3, 200, 10)]);
        let filter = TraceFilter {
            since_unix_nano: Some(100 * MS),
            ..TraceFilter::default()
        };
        // The bound is inclusive: a trace starting exactly at `since` is kept.
        assert_eq!(ids(&search(&store, &filter, 10)), vec![tid(3), tid(2)]);
    });
}

#[test]
fn cursor_pages_without_duplicates() {
    each_mode(|store, _c| {
        ingest_roots(
            &store,
            &[(1, 10, 1), (2, 20, 1), (3, 30, 1), (4, 40, 1), (5, 50, 1)],
        );
        let filter = TraceFilter::default();

        let first = store.search_traces(&filter, &page(2));
        assert_eq!(ids(&first), vec![tid(5), tid(4)]);
        assert!(first.next_cursor.is_some());

        let second = store.search_traces(
            &filter,
            &PageRequest {
                limit: 2,
                cursor: first.next_cursor,
            },
        );
        assert_eq!(ids(&second), vec![tid(3), tid(2)]);
        assert!(second.next_cursor.is_some());

        let third = store.search_traces(
            &filter,
            &PageRequest {
                limit: 2,
                cursor: second.next_cursor,
            },
        );
        assert_eq!(ids(&third), vec![tid(1)]);
        assert_eq!(third.next_cursor, None);
    });
}

// --- paging corners ---

#[test]
fn cursor_is_start_and_trace_id_of_the_last_item() {
    each_mode(|store, _c| {
        ingest_roots(&store, &[(1, 10, 1), (2, 20, 1), (3, 30, 1)]);
        let first = search(&store, &TraceFilter::default(), 2);
        assert_eq!(
            first.next_cursor,
            Some(format!("{}-{}", 20 * MS, tid(2))),
            "the cursor is \"{{start_unix_nano}}-{{trace_id}}\" of the last item"
        );
    });
}

#[test]
fn last_full_page_has_no_cursor() {
    each_mode(|store, _c| {
        ingest_roots(&store, &[(1, 10, 1), (2, 20, 1), (3, 30, 1), (4, 40, 1)]);
        let first = search(&store, &TraceFilter::default(), 2);
        let second = store.search_traces(
            &TraceFilter::default(),
            &PageRequest {
                limit: 2,
                cursor: first.next_cursor,
            },
        );
        assert_eq!(ids(&second), vec![tid(2), tid(1)]);
        assert_eq!(second.next_cursor, None, "nothing follows the last trace");
    });
}

#[test]
fn limit_above_the_count_returns_everything() {
    each_mode(|store, _c| {
        ingest_roots(&store, &[(1, 10, 1), (2, 20, 1)]);
        let result = search(&store, &TraceFilter::default(), usize::MAX);
        assert_eq!(result.items.len(), 2);
        assert_eq!(result.next_cursor, None);
    });
}

#[test]
fn limit_zero_is_an_empty_page_without_a_cursor() {
    each_mode(|store, _c| {
        ingest_roots(&store, &[(1, 10, 1)]);
        let result = search(&store, &TraceFilter::default(), 0);
        assert!(result.items.is_empty());
        assert_eq!(result.next_cursor, None);
    });
}

#[test]
fn empty_store_is_an_empty_page() {
    each_mode(|store, _c| {
        let result = search(&store, &TraceFilter::default(), 10);
        assert!(result.items.is_empty());
        assert_eq!(result.next_cursor, None);
    });
}

#[test]
fn traces_starting_together_page_by_trace_id_without_loss() {
    each_mode(|store, _c| {
        ingest_roots(&store, &[(1, 10, 1), (2, 10, 1), (3, 10, 1), (4, 10, 1)]);
        let filter = TraceFilter::default();
        let mut seen = Vec::new();
        let mut cursor = None;
        loop {
            let result = store.search_traces(&filter, &PageRequest { limit: 1, cursor });
            seen.extend(ids(&result));
            match result.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        // Equal starts sort by trace ID, descending, and every trace shows once.
        assert_eq!(seen, vec![tid(4), tid(3), tid(2), tid(1)]);
    });
}

#[test]
fn paging_survives_new_traces_arriving_between_pages() {
    each_mode(|store, _c| {
        ingest_roots(&store, &[(1, 10, 1), (2, 20, 1), (3, 30, 1), (4, 40, 1)]);
        let filter = TraceFilter::default();
        let first = store.search_traces(&filter, &page(2));
        assert_eq!(ids(&first), vec![tid(4), tid(3)]);

        // A newer trace appears while the reader is on page one. A keyset cursor
        // does not shift: page two still starts right after trace 3.
        ingest_roots(&store, &[(9, 90, 1)]);
        let second = store.search_traces(
            &filter,
            &PageRequest {
                limit: 2,
                cursor: first.next_cursor,
            },
        );
        assert_eq!(ids(&second), vec![tid(2), tid(1)]);
    });
}

#[test]
fn cursor_applies_on_top_of_the_filters() {
    each_mode(|store, _c| {
        // Even traces fail, odd ones are fine.
        for n in 1u8..=6 {
            let root = span(n, 1, None, "web-bff", "GET /", u64::from(n) * 10, 1);
            store.ingest_spans(vec![if n % 2 == 0 { error(root) } else { root }]);
        }
        let filter = TraceFilter {
            errors_only: true,
            ..TraceFilter::default()
        };
        let first = store.search_traces(&filter, &page(2));
        assert_eq!(ids(&first), vec![tid(6), tid(4)]);
        let second = store.search_traces(
            &filter,
            &PageRequest {
                limit: 2,
                cursor: first.next_cursor,
            },
        );
        assert_eq!(ids(&second), vec![tid(2)]);
        assert_eq!(second.next_cursor, None);
    });
}

#[test]
fn garbage_cursor_is_an_empty_page() {
    // Restarting from the top would send a paging agent round in circles, so
    // a cursor that is not ours ends the listing.
    each_mode(|store, _c| {
        ingest_roots(&store, &[(1, 10, 1), (2, 20, 1)]);
        for cursor in [
            "",
            "garbage",
            "-",
            "12-",
            "abc-0101",
            "12-xyz",
            "12-0102",
            "99999999999999999999999-01010101010101010101010101010101",
        ] {
            let result = store.search_traces(
                &TraceFilter::default(),
                &PageRequest {
                    limit: 10,
                    cursor: Some(cursor.to_string()),
                },
            );
            assert!(result.items.is_empty(), "cursor {cursor:?}");
            assert_eq!(result.next_cursor, None, "cursor {cursor:?}");
        }
    });
}

#[test]
fn filters_combine() {
    each_mode(|store, _c| {
        // 1: payments, failed, slow. 2: payments, ok, slow. 3: no payments,
        // failed, slow. 4: payments, failed, fast.
        store.ingest_spans(vec![
            error(span(1, 1, None, "web-bff", "GET /", 10, 500)),
            span(1, 2, Some(1), "payments-api", "POST", 11, 5),
        ]);
        store.ingest_spans(vec![
            span(2, 1, None, "web-bff", "GET /", 20, 500),
            span(2, 2, Some(1), "payments-api", "POST", 21, 5),
        ]);
        store.ingest_spans(vec![error(span(3, 1, None, "web-bff", "GET /", 30, 500))]);
        store.ingest_spans(vec![
            error(span(4, 1, None, "web-bff", "GET /", 40, 5)),
            span(4, 2, Some(1), "payments-api", "POST", 41, 1),
        ]);
        let filter = TraceFilter {
            service: Some("payments-api".to_string()),
            errors_only: true,
            min_duration_nanos: Some(100 * MS),
            since_unix_nano: Some(10 * MS),
            query: None,
        };
        assert_eq!(ids(&search(&store, &filter, 10)), vec![tid(1)]);
    });
}

#[test]
fn evicted_traces_are_not_found() {
    each_mode_with(
        StoreConfig {
            max_traces: 2,
            ..StoreConfig::default()
        },
        |store, _c| {
            ingest_roots(&store, &[(1, 10, 1), (2, 20, 1), (3, 30, 1)]);
            let result = search(&store, &TraceFilter::default(), 10);
            assert_eq!(ids(&result), vec![tid(3), tid(2)]);
            assert!(store.summary(tid(1)).is_none());
        },
    );
}

// --- the summary ---

/// The summary of this shape is checked field by field:
///
/// ```text
/// web-bff        POST /api/checkout   0..500 ms    (root, erroring)
///   orders-api     POST /orders       50..450      (erroring, has an exception)
///     payments-api POST /charges      100..150     (erroring leaf)
/// ```
fn checkout_trace() -> Vec<loctel_model::Span> {
    vec![
        error(span(1, 1, None, "web-bff", "POST /api/checkout", 0, 500)),
        error(with_exception(
            span(1, 2, Some(1), "orders-api", "POST /orders", 50, 400),
            "OrdersError",
            "order failed",
        )),
        error(span(
            1,
            3,
            Some(2),
            "payments-api",
            "POST /charges",
            100,
            50,
        )),
    ]
}

#[test]
fn summary_has_every_field() {
    each_mode(|store, clock| {
        // Spans arrive leaf first, as they do in real life: the first service
        // seen by start time is still the entry point.
        let mut spans = checkout_trace();
        spans.reverse();
        store.ingest_spans(spans);

        let s = store.summary(tid(1)).unwrap();
        assert_eq!(s.trace_id, tid(1));
        assert_eq!(s.root_name.as_deref(), Some("POST /api/checkout"));
        assert_eq!(s.root_service.as_deref(), Some("web-bff"));
        assert_eq!(s.start_unix_nano, 0);
        assert_eq!(s.duration_nanos, 500 * MS);
        assert_eq!(s.span_count, 3);
        assert_eq!(s.services, vec!["web-bff", "orders-api", "payments-api"]);
        assert_eq!(s.status, TraceStatus::Failed);
        let failed_in = s.failed_in.as_ref().unwrap();
        assert_eq!(failed_in.service, "payments-api");
        assert_eq!(failed_in.more, 0);
        // The failed-in span has no exception; its parent does.
        assert_eq!(s.first_exception.as_ref().unwrap().type_name, "OrdersError");
        assert!(!s.complete);

        clock.advance(Duration::from_secs(10));
        assert!(store.summary(tid(1)).unwrap().complete);
    });
}

#[test]
fn summary_agrees_with_the_snapshot() {
    each_mode(|store, _c| {
        store.ingest_spans(checkout_trace());
        let t = store.trace(tid(1)).unwrap();
        let s = store.summary(tid(1)).unwrap();
        assert_eq!(s.status, t.status);
        assert_eq!(s.failed_in, t.failed_in);
        assert_eq!(s.complete, t.complete);
        assert_eq!(s.span_count, t.spans.len());
        assert_eq!(s.first_exception, loctel_store::trace_exception(&t));
    });
}

#[test]
fn search_items_equal_store_summary() {
    each_mode(|store, _c| {
        store.ingest_spans(checkout_trace());
        let from_search = search(&store, &TraceFilter::default(), 10).items;
        assert_eq!(from_search, vec![store.summary(tid(1)).unwrap()]);
    });
}

#[test]
fn summary_of_unknown_trace_is_none() {
    each_mode(|store, _c| {
        assert!(store.summary(tid(9)).is_none());
    });
}

#[test]
fn summary_without_a_root_has_no_root_fields() {
    each_mode(|store, _c| {
        store.ingest_spans(vec![
            span(1, 3, Some(2), "payments-api", "POST /charges", 100, 50),
            span(1, 2, Some(1), "orders-api", "POST /orders", 50, 200),
        ]);
        let s = store.summary(tid(1)).unwrap();
        assert_eq!(s.root_name, None);
        assert_eq!(s.root_service, None);
        // Start and duration cover the spans that are here.
        assert_eq!(s.start_unix_nano, 50 * MS);
        assert_eq!(s.duration_nanos, 200 * MS);
        assert_eq!(s.status, TraceStatus::Ok);
    });
}

#[test]
fn services_are_listed_once_in_order_of_first_start() {
    each_mode(|store, _c| {
        store.ingest_spans(vec![
            span(1, 4, Some(1), "orders-api", "second visit", 300, 10),
            span(1, 3, Some(1), "payments-api", "POST", 200, 10),
            span(1, 2, Some(1), "orders-api", "first visit", 100, 10),
            span(1, 1, None, "web-bff", "GET /", 0, 400),
        ]);
        let s = store.summary(tid(1)).unwrap();
        assert_eq!(s.services, vec!["web-bff", "orders-api", "payments-api"]);
    });
}

#[test]
fn summary_duration_covers_the_whole_trace_and_clamps_skew() {
    each_mode(|store, _c| {
        let mut skewed = span(1, 3, Some(1), "orders-api", "skewed", 40, 0);
        skewed.end_unix_nano = skewed.start_unix_nano - 10 * MS; // ends before it starts
        store.ingest_spans(vec![
            span(1, 1, None, "web-bff", "GET /", 100, 50),
            span(1, 2, Some(1), "orders-api", "late child", 140, 100),
            skewed,
        ]);
        let s = store.summary(tid(1)).unwrap();
        // The trace starts with the skewed span, and ends where the late child
        // ends (240 ms); the skewed span's own end counts as its start.
        assert_eq!(s.start_unix_nano, 40 * MS);
        assert_eq!(s.duration_nanos, 200 * MS);
    });
}

#[test]
fn all_spans_skewed_gives_zero_duration() {
    each_mode(|store, _c| {
        let mut skewed = span(1, 1, None, "web-bff", "GET /", 100, 0);
        skewed.end_unix_nano = 50 * MS;
        store.ingest_spans(vec![skewed]);
        let s = store.summary(tid(1)).unwrap();
        assert_eq!(s.start_unix_nano, 100 * MS);
        assert_eq!(s.duration_nanos, 0);
    });
}

#[test]
fn summary_of_trace_with_only_handled_exceptions_is_ok_with_first_exception() {
    each_mode(|store, _c| {
        store.ingest_spans(vec![
            span(1, 1, None, "web-bff", "GET /", 0, 100),
            with_exception(
                span(1, 2, Some(1), "orders-api", "retry", 10, 10),
                "Retried",
                "then succeeded",
            ),
        ]);
        let s = store.summary(tid(1)).unwrap();
        assert_eq!(s.status, TraceStatus::Ok);
        assert_eq!(s.failed_in, None);
        assert_eq!(s.first_exception.unwrap().type_name, "Retried");
    });
}

#[test]
fn summary_serializes_with_snake_case_status_and_hex_ids() {
    each_mode(|store, _c| {
        store.ingest_spans(checkout_trace());
        let json = serde_json::to_value(store.summary(tid(1)).unwrap()).unwrap();
        assert_eq!(json["status"], "failed");
        assert_eq!(json["trace_id"], tid(1).to_string());
        assert_eq!(json["failed_in"]["service"], "payments-api");
    });
}

#[test]
fn late_span_changes_the_summary_but_not_older_summaries() {
    each_mode(|store, _c| {
        store.ingest_spans(vec![span(1, 1, None, "web-bff", "GET /", 0, 100)]);
        let before = store.summary(tid(1)).unwrap();
        assert_eq!(before.status, TraceStatus::Ok);
        store.ingest_spans(vec![error(span(
            1,
            2,
            Some(1),
            "orders-api",
            "POST",
            10,
            10,
        ))]);
        let after = store.summary(tid(1)).unwrap();
        assert_eq!(after.status, TraceStatus::ErrorsInside);
        assert_eq!(after.span_count, 2);
        assert_eq!(before.span_count, 1);
        assert_eq!(before.status, TraceStatus::Ok);
    });
}
