//! Logs: ingest, dedup, eviction, search and the span link (plan Task 5).
//!
//! The plan's six tests come first. The rest pin the corners that the MCP
//! and UI layers rely on: paging, late arrivals, counting and clearing.

mod fixtures;

use std::thread;

use fixtures::*;
use loctel_model::{LogRecord, Severity};
use loctel_store::{LogFilter, Page, PageRequest, Store, StoreConfig};

const MS: u64 = 1_000_000;

fn first(limit: usize) -> PageRequest {
    PageRequest {
        limit,
        cursor: None,
    }
}

fn everything(store: &Store) -> Page<LogRecord> {
    store.search_logs(&LogFilter::default(), &first(usize::MAX))
}

fn bodies(items: &[LogRecord]) -> Vec<&str> {
    items.iter().map(|l| l.body.as_str()).collect()
}

fn with_cap(max_log_records: usize) -> (Store, std::sync::Arc<loctel_store::ManualClock>) {
    store_with_config(StoreConfig {
        max_log_records,
        ..StoreConfig::default()
    })
}

// --- the plan's tests ---

#[test]
fn search_logs_newest_first_with_filters() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_logs(vec![
        log("orders-api", "order created", 10),
        with_severity(log("payments-api", "charge declined", 20), Severity::Error),
        log("orders-api", "order shipped", 30),
        in_span(log("orders-api", "inside a span", 40), 1, 2),
        log("web-bff", "checkout", 50),
    ]);

    // No filter: everything, newest first.
    let all = everything(&store);
    assert_eq!(
        bodies(&all.items),
        vec![
            "checkout",
            "inside a span",
            "order shipped",
            "charge declined",
            "order created"
        ]
    );
    assert_eq!(all.next_cursor, None);

    // Each filter on its own.
    let page = |f: LogFilter| store.search_logs(&f, &first(10)).items;
    let since = page(LogFilter {
        since_unix_nano: Some(30 * MS),
        ..LogFilter::default()
    });
    assert_eq!(
        bodies(&since),
        vec!["checkout", "inside a span", "order shipped"]
    );
    let service = page(LogFilter {
        service: Some("orders-api".into()),
        ..LogFilter::default()
    });
    assert_eq!(
        bodies(&service),
        vec!["inside a span", "order shipped", "order created"]
    );
    let severity = page(LogFilter {
        min_severity: Some(Severity::Error),
        ..LogFilter::default()
    });
    assert_eq!(bodies(&severity), vec!["charge declined"]);
    let text = page(LogFilter {
        contains: Some("order".into()),
        ..LogFilter::default()
    });
    assert_eq!(bodies(&text), vec!["order shipped", "order created"]);
    let trace = page(LogFilter {
        trace_id: Some(tid(1)),
        ..LogFilter::default()
    });
    assert_eq!(bodies(&trace), vec!["inside a span"]);
    let span_only = page(LogFilter {
        span_id: Some(sid(2)),
        ..LogFilter::default()
    });
    assert_eq!(bodies(&span_only), vec!["inside a span"]);

    // Filters combine with AND.
    let combined = page(LogFilter {
        since_unix_nano: Some(20 * MS),
        service: Some("orders-api".into()),
        contains: Some("ORDER".into()),
        ..LogFilter::default()
    });
    assert_eq!(bodies(&combined), vec!["order shipped"]);
}

#[test]
fn contains_is_case_insensitive() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_logs(vec![
        log("a", "Connection Refused by upstream", 1),
        log("a", "all good", 2),
    ]);
    for needle in ["connection refused", "CONNECTION REFUSED", "nEcTiOn rEfU"] {
        let found = store.search_logs(
            &LogFilter {
                contains: Some(needle.into()),
                ..LogFilter::default()
            },
            &first(10),
        );
        assert_eq!(
            bodies(&found.items),
            vec!["Connection Refused by upstream"],
            "needle {needle:?}"
        );
    }
}

#[test]
fn min_severity_filters_lower_levels() {
    let (store, _c) = store_with_manual_clock();
    let levels = [
        ("unspecified", Severity::Unspecified),
        ("trace", Severity::Trace),
        ("debug", Severity::Debug),
        ("info", Severity::Info),
        ("warn", Severity::Warn),
        ("error", Severity::Error),
        ("fatal", Severity::Fatal),
    ];
    store.ingest_logs(
        levels
            .iter()
            .enumerate()
            .map(|(i, (body, level))| with_severity(log("a", body, i as u64 + 1), *level))
            .collect(),
    );
    let at_least = |min: Severity| {
        let page = store.search_logs(
            &LogFilter {
                min_severity: Some(min),
                ..LogFilter::default()
            },
            &first(10),
        );
        let mut found: Vec<String> = page.items.iter().map(|l| l.body.clone()).collect();
        found.reverse(); // oldest first, to read like the table above
        found
    };
    assert_eq!(
        at_least(Severity::Warn),
        vec!["warn", "error", "fatal"],
        "warn and above"
    );
    assert_eq!(at_least(Severity::Fatal), vec!["fatal"]);
    assert_eq!(
        at_least(Severity::Info),
        vec!["info", "warn", "error", "fatal"],
        "an unspecified level is below info"
    );
    assert_eq!(
        at_least(Severity::Unspecified).len(),
        7,
        "the lowest level keeps everything"
    );
}

#[test]
fn logs_for_span_links_by_ids() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_logs(vec![
        in_span(log("orders-api", "second", 20), 1, 2),
        in_span(log("orders-api", "first", 10), 1, 2),
        in_span(log("orders-api", "same span id, other trace", 15), 9, 2),
        in_span(log("orders-api", "other span", 12), 1, 3),
        in_trace(log("orders-api", "trace only", 11), 1),
        log("orders-api", "no context", 13),
        in_span(log("payments-api", "third", 30), 1, 2),
    ]);
    let found = store.logs_for_span(tid(1), sid(2));
    assert_eq!(bodies(&found), vec!["first", "second", "third"]);
    assert!(found.iter().all(|l| l.trace_id == Some(tid(1))));
    assert!(found.iter().all(|l| l.span_id == Some(sid(2))));
    assert_eq!(store.logs_for_span(tid(9), sid(2)).len(), 1);
    assert!(store.logs_for_span(tid(1), sid(99)).is_empty());
    assert!(store.logs_for_span(tid(77), sid(2)).is_empty());
}

#[test]
fn duplicate_log_is_ignored() {
    let (store, _c) = store_with_manual_clock();
    let line = in_span(log("orders-api", "retry me", 10), 1, 2);
    store.ingest_logs(vec![line.clone()]);
    store.ingest_logs(vec![line.clone(), line]);
    assert_eq!(everything(&store).items.len(), 1);
    assert_eq!(store.logs_for_span(tid(1), sid(2)).len(), 1);
    let stats = store.stats();
    assert_eq!(stats.logs, 1);
    assert_eq!(stats.logs_received, 1, "a retry is counted once");
}

#[test]
fn log_eviction_drops_oldest() {
    let (store, _c) = with_cap(3);
    store.ingest_logs(vec![
        log("a", "t1", 1),
        log("a", "t2", 2),
        log("a", "t3", 3),
    ]);
    assert_eq!(store.stats().logs, 3);
    store.ingest_logs(vec![log("a", "t4", 4)]);
    assert_eq!(bodies(&everything(&store).items), vec!["t4", "t3", "t2"]);
    store.ingest_logs(vec![log("a", "t5", 5), log("a", "t6", 6)]);
    assert_eq!(bodies(&everything(&store).items), vec!["t6", "t5", "t4"]);
    let stats = store.stats();
    assert_eq!(stats.logs, 3);
    assert_eq!(
        stats.logs_received, 6,
        "evicted logs still count as received"
    );
}

// --- paging ---

#[test]
fn cursor_pages_logs_without_duplicates() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_logs((1..=5).map(|i| log("a", &format!("l{i}"), i)).collect());
    let filter = LogFilter::default();

    let p1 = store.search_logs(&filter, &first(2));
    assert_eq!(bodies(&p1.items), vec!["l5", "l4"]);
    assert!(p1.next_cursor.is_some());

    let p2 = store.search_logs(
        &filter,
        &PageRequest {
            limit: 2,
            cursor: p1.next_cursor,
        },
    );
    assert_eq!(bodies(&p2.items), vec!["l3", "l2"]);
    assert!(p2.next_cursor.is_some());

    let p3 = store.search_logs(
        &filter,
        &PageRequest {
            limit: 2,
            cursor: p2.next_cursor,
        },
    );
    assert_eq!(bodies(&p3.items), vec!["l1"]);
    assert_eq!(p3.next_cursor, None);
}

#[test]
fn a_page_that_ends_exactly_at_the_last_log_has_no_cursor() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_logs(vec![log("a", "x", 1), log("a", "y", 2)]);
    let page = store.search_logs(&LogFilter::default(), &first(2));
    assert_eq!(page.items.len(), 2);
    assert_eq!(page.next_cursor, None);
}

#[test]
fn paging_applies_the_filter_and_does_not_count_skipped_logs() {
    let (store, _c) = store_with_manual_clock();
    // Matching and non-matching logs interleave; every page must still be
    // full of matches, and no match may be lost or repeated.
    store.ingest_logs(
        (1..=9)
            .map(|i| {
                let service = if i % 3 == 0 { "wanted" } else { "other" };
                log(service, &format!("l{i}"), i)
            })
            .collect(),
    );
    let filter = LogFilter {
        service: Some("wanted".into()),
        ..LogFilter::default()
    };
    let p1 = store.search_logs(&filter, &first(2));
    assert_eq!(bodies(&p1.items), vec!["l9", "l6"]);
    let p2 = store.search_logs(
        &filter,
        &PageRequest {
            limit: 2,
            cursor: p1.next_cursor,
        },
    );
    assert_eq!(bodies(&p2.items), vec!["l3"]);
    assert_eq!(p2.next_cursor, None);
}

#[test]
fn logs_with_equal_timestamps_page_without_loss_and_newest_arrival_first() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_logs(vec![log("a", "first", 5), log("a", "second", 5)]);
    store.ingest_logs(vec![log("a", "third", 5), log("a", "later", 6)]);
    assert_eq!(
        bodies(&everything(&store).items),
        vec!["later", "third", "second", "first"]
    );
    let mut seen = Vec::new();
    let mut cursor = None;
    loop {
        let page = store.search_logs(&LogFilter::default(), &PageRequest { limit: 1, cursor });
        seen.extend(page.items.iter().map(|l| l.body.clone()));
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(seen, vec!["later", "third", "second", "first"]);
}

#[test]
fn limit_zero_and_garbage_cursors_give_an_empty_page() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_logs(vec![log("a", "x", 1), log("a", "y", 2)]);
    let empty = store.search_logs(&LogFilter::default(), &first(0));
    assert!(empty.items.is_empty());
    assert_eq!(empty.next_cursor, None);
    for garbage in [
        "",
        "nope",
        "1",
        "1-",
        "-1",
        "+1-2",
        "1-2-3",
        "a-b",
        "99999999999999999999-1",
    ] {
        let page = store.search_logs(
            &LogFilter::default(),
            &PageRequest {
                limit: 10,
                cursor: Some(garbage.into()),
            },
        );
        assert!(page.items.is_empty(), "cursor {garbage:?}");
        assert_eq!(page.next_cursor, None, "cursor {garbage:?}");
    }
}

#[test]
fn a_cursor_still_works_after_older_logs_were_evicted() {
    let (store, _c) = with_cap(4);
    store.ingest_logs((1..=4).map(|i| log("a", &format!("l{i}"), i)).collect());
    let p1 = store.search_logs(&LogFilter::default(), &first(2));
    assert_eq!(bodies(&p1.items), vec!["l4", "l3"]);
    // Newer logs arrive, and evict l1 and l2: the next page starts after l3.
    store.ingest_logs(vec![log("a", "l5", 5), log("a", "l6", 6)]);
    let p2 = store.search_logs(
        &LogFilter::default(),
        &PageRequest {
            limit: 10,
            cursor: p1.next_cursor,
        },
    );
    assert!(p2.items.is_empty(), "l2 and l1 are gone, nothing is older");
    assert_eq!(p2.next_cursor, None);
}

// --- ordering ---

#[test]
fn a_late_arrival_takes_its_place_in_time_order() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_logs(vec![log("a", "t10", 10), log("a", "t30", 30)]);
    store.ingest_logs(vec![log("b", "t20", 20), log("b", "t05", 5)]);
    assert_eq!(
        bodies(&everything(&store).items),
        vec!["t30", "t20", "t10", "t05"]
    );
}

#[test]
fn since_is_inclusive() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_logs(vec![
        log("a", "old", 9),
        log("a", "edge", 10),
        log("a", "new", 11),
    ]);
    let found = store.search_logs(
        &LogFilter {
            since_unix_nano: Some(10 * MS),
            ..LogFilter::default()
        },
        &first(10),
    );
    assert_eq!(bodies(&found.items), vec!["new", "edge"]);
}

#[test]
fn a_log_without_a_timestamp_is_placed_by_its_observed_time() {
    let (store, _c) = store_with_manual_clock();
    let mut untimed = log("a", "untimed", 0);
    untimed.time_unix_nano = 0;
    untimed.observed_unix_nano = 25 * MS;
    store.ingest_logs(vec![log("a", "t10", 10), untimed, log("a", "t40", 40)]);
    assert_eq!(
        bodies(&everything(&store).items),
        vec!["t40", "untimed", "t10"]
    );
    let recent = store.search_logs(
        &LogFilter {
            since_unix_nano: Some(20 * MS),
            ..LogFilter::default()
        },
        &first(10),
    );
    assert_eq!(bodies(&recent.items), vec!["t40", "untimed"]);
}

// --- filters ---

#[test]
fn service_filter_is_an_exact_match() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_logs(vec![
        log("orders", "a", 1),
        log("orders-api", "b", 2),
        log("Orders", "c", 3),
    ]);
    let found = store.search_logs(
        &LogFilter {
            service: Some("orders".into()),
            ..LogFilter::default()
        },
        &first(10),
    );
    assert_eq!(bodies(&found.items), vec!["a"]);
}

#[test]
fn a_log_without_service_name_is_listed_as_unknown_service() {
    let (store, _c) = store_with_manual_clock();
    let mut nameless = log("x", "who am i", 1);
    nameless.resource = std::sync::Arc::new(loctel_model::Resource { attributes: vec![] });
    store.ingest_logs(vec![nameless]);
    let found = store.search_logs(
        &LogFilter {
            service: Some("unknown_service".into()),
            ..LogFilter::default()
        },
        &first(10),
    );
    assert_eq!(bodies(&found.items), vec!["who am i"]);
}

#[test]
fn contains_matches_the_body_only_and_an_empty_needle_matches_everything() {
    let (store, _c) = store_with_manual_clock();
    let mut tagged = log("needle-service", "plain body", 1);
    tagged.attributes.push((
        "note".into(),
        loctel_model::AttrValue::Str("needle in an attribute".into()),
    ));
    store.ingest_logs(vec![tagged, log("a", "a needle here", 2)]);
    let search = |needle: &str| {
        store
            .search_logs(
                &LogFilter {
                    contains: Some(needle.into()),
                    ..LogFilter::default()
                },
                &first(10),
            )
            .items
    };
    assert_eq!(bodies(&search("needle")), vec!["a needle here"]);
    assert_eq!(search("").len(), 2);
    assert!(search("absent").is_empty());
}

#[test]
fn contains_folds_case_for_non_ascii_text_too() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_logs(vec![
        log("a", "ÉCHEC de la connexion", 1),
        log("a", "Straße blocked", 2),
        log("a", "plain ascii", 3),
    ]);
    let search = |needle: &str| {
        let page = store.search_logs(
            &LogFilter {
                contains: Some(needle.into()),
                ..LogFilter::default()
            },
            &first(10),
        );
        page.items
            .iter()
            .map(|l| l.body.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(search("échec"), vec!["ÉCHEC de la connexion"]);
    assert_eq!(search("ÉCHEC"), vec!["ÉCHEC de la connexion"]);
    assert_eq!(search("STRAßE"), vec!["Straße blocked"]);
    // An ASCII needle still finds ASCII text in a body that has other
    // characters in it.
    assert_eq!(search("CONNEXION"), vec!["ÉCHEC de la connexion"]);
}

#[test]
fn trace_and_span_filters_need_the_ids_to_be_set_on_the_log() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_logs(vec![
        log("a", "no context", 1),
        in_trace(log("a", "trace only", 2), 1),
        in_span(log("a", "trace and span", 3), 1, 2),
        in_span(log("a", "other trace", 4), 2, 2),
    ]);
    let search = |f: LogFilter| {
        store
            .search_logs(&f, &first(10))
            .items
            .into_iter()
            .map(|l| l.body)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        search(LogFilter {
            trace_id: Some(tid(1)),
            ..LogFilter::default()
        }),
        vec!["trace and span", "trace only"]
    );
    assert_eq!(
        search(LogFilter {
            trace_id: Some(tid(1)),
            span_id: Some(sid(2)),
            ..LogFilter::default()
        }),
        vec!["trace and span"]
    );
    assert_eq!(
        search(LogFilter {
            span_id: Some(sid(2)),
            ..LogFilter::default()
        }),
        vec!["other trace", "trace and span"],
        "a span ID on its own matches in every trace"
    );
}

#[test]
fn search_returns_whole_records() {
    let (store, _c) = store_with_manual_clock();
    let line = with_severity(in_span(log("orders-api", "boom", 7), 1, 2), Severity::Error);
    store.ingest_logs(vec![line.clone()]);
    assert_eq!(everything(&store).items, vec![line.clone()]);
    assert_eq!(store.logs_for_span(tid(1), sid(2)), vec![line]);
}

// --- the span link ---

#[test]
fn logs_for_span_is_oldest_first_whatever_the_arrival_order() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_logs(vec![
        in_span(log("a", "t30", 30), 1, 2),
        in_span(log("a", "t10", 10), 1, 2),
    ]);
    store.ingest_logs(vec![
        in_span(log("a", "t20", 20), 1, 2),
        in_span(log("a", "t05", 5), 1, 2),
    ]);
    assert_eq!(
        bodies(&store.logs_for_span(tid(1), sid(2))),
        vec!["t05", "t10", "t20", "t30"]
    );
}

#[test]
fn logs_for_span_needs_no_span_to_be_stored() {
    // Logs and spans are stored independently: the link is by ID only, so a
    // log is found before its span arrives (or after the span's trace was
    // evicted).
    let (store, _c) = store_with_manual_clock();
    store.ingest_logs(vec![in_span(log("a", "early log", 1), 1, 1)]);
    assert_eq!(store.logs_for_span(tid(1), sid(1)).len(), 1);
    store.ingest_spans(vec![span(1, 1, None, "a", "root", 0, 5)]);
    assert_eq!(store.logs_for_span(tid(1), sid(1)).len(), 1);
}

#[test]
fn evicting_a_log_unlinks_it_from_its_span() {
    let (store, _c) = with_cap(2);
    store.ingest_logs(vec![
        in_span(log("a", "s2-old", 1), 1, 2),
        in_span(log("a", "s3", 2), 1, 3),
        in_span(log("a", "s2-new", 3), 1, 2),
    ]);
    // "s2-old" was the oldest and is gone, from search and from the span.
    assert_eq!(bodies(&store.logs_for_span(tid(1), sid(2))), vec!["s2-new"]);
    assert_eq!(bodies(&store.logs_for_span(tid(1), sid(3))), vec!["s3"]);
    store.ingest_logs(vec![log("a", "newer", 4), log("a", "newest", 5)]);
    assert!(store.logs_for_span(tid(1), sid(2)).is_empty());
    assert!(store.logs_for_span(tid(1), sid(3)).is_empty());
    assert_eq!(bodies(&everything(&store).items), vec!["newest", "newer"]);
}

// --- eviction ---

#[test]
fn eviction_keeps_the_newest_by_time_whatever_the_arrival_order() {
    let (store, _c) = with_cap(2);
    store.ingest_logs(vec![
        log("a", "t30", 30),
        log("a", "t10", 10),
        log("a", "t20", 20),
        log("a", "t40", 40),
    ]);
    assert_eq!(bodies(&everything(&store).items), vec!["t40", "t30"]);
    assert_eq!(store.stats().logs, 2);
}

#[test]
fn a_batch_is_applied_in_full_before_eviction() {
    // Evicting in the middle of a batch would forget the first log, then
    // accept its retry (later in the same batch) as new.
    let (store, _c) = with_cap(2);
    let a = log("a", "a", 1);
    store.ingest_logs(vec![a.clone(), log("a", "b", 2), log("a", "c", 3), a]);
    assert_eq!(bodies(&everything(&store).items), vec!["c", "b"]);
    assert_eq!(store.stats().logs_received, 3);
}

#[test]
fn a_late_log_older_than_everything_held_is_dropped_at_the_cap() {
    let (store, _c) = with_cap(2);
    store.ingest_logs(vec![log("a", "t20", 20), log("a", "t30", 30)]);
    store.ingest_logs(vec![log("a", "ancient", 1)]);
    assert_eq!(bodies(&everything(&store).items), vec!["t30", "t20"]);
}

#[test]
fn a_zero_cap_holds_nothing_but_still_counts() {
    let (store, _c) = with_cap(0);
    store.ingest_logs(vec![log("a", "x", 1), log("a", "y", 2)]);
    let stats = store.stats();
    assert_eq!(stats.logs, 0);
    assert_eq!(stats.logs_received, 2);
    assert!(everything(&store).items.is_empty());
}

#[test]
fn a_retry_of_an_evicted_log_is_taken_as_new_and_is_the_first_to_go_again() {
    // Dedup remembers only what the store still holds, so memory stays
    // bounded. A retry that arrives after its original was evicted is
    // accepted like a new log (and counted), but it is the oldest, so it is
    // evicted again at once.
    let (store, _c) = with_cap(2);
    let original = log("a", "original", 1);
    store.ingest_logs(vec![original.clone(), log("a", "t2", 2), log("a", "t3", 3)]);
    assert_eq!(bodies(&everything(&store).items), vec!["t3", "t2"]);
    store.ingest_logs(vec![original]);
    assert_eq!(bodies(&everything(&store).items), vec!["t3", "t2"]);
    assert_eq!(store.stats().logs_received, 4);
}

// --- counting and clearing ---

#[test]
fn stats_count_held_and_received_logs() {
    let (store, _c) = store_with_manual_clock();
    assert_eq!(store.stats().logs, 0);
    store.ingest_logs(vec![log("a", "x", 1), log("a", "y", 2)]);
    store.ingest_logs(vec![log("a", "x", 1)]);
    let stats = store.stats();
    assert_eq!((stats.logs, stats.logs_received), (2, 2));
    assert_eq!(stats.spans, 0, "logs do not count as spans");
}

#[test]
fn an_empty_batch_changes_nothing() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_logs(vec![]);
    assert_eq!(store.stats(), loctel_store::StoreStats::default());
}

#[test]
fn clear_empties_logs_and_keeps_the_received_total() {
    let (store, _c) = store_with_manual_clock();
    let line = in_span(log("a", "x", 1), 1, 2);
    store.ingest_logs(vec![line.clone(), log("a", "y", 2)]);
    store.clear();
    let stats = store.stats();
    assert_eq!(stats.logs, 0);
    assert_eq!(stats.logs_received, 2);
    assert!(everything(&store).items.is_empty());
    assert!(store.logs_for_span(tid(1), sid(2)).is_empty());
    // The dedup memory is cleared too: the same line is accepted again.
    store.ingest_logs(vec![line]);
    assert_eq!(store.stats().logs, 1);
    assert_eq!(store.stats().logs_received, 3);
}

#[test]
fn clear_drops_spans_and_logs_together() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_spans(vec![span(1, 1, None, "a", "x", 0, 1)]);
    store.ingest_logs(vec![log("a", "x", 1)]);
    store.clear();
    let stats = store.stats();
    assert_eq!((stats.traces, stats.spans, stats.logs), (0, 0, 0));
    assert_eq!((stats.spans_received, stats.logs_received), (1, 1));
}

#[test]
fn logs_and_spans_do_not_disturb_each_other() {
    let (store, _c) = store_with_config(StoreConfig {
        max_traces: 1,
        max_log_records: 1,
        ..StoreConfig::default()
    });
    store.ingest_spans(vec![span(1, 1, None, "a", "x", 0, 1)]);
    store.ingest_logs(vec![log("a", "l1", 1)]);
    store.ingest_spans(vec![span(2, 1, None, "a", "y", 0, 1)]);
    // Span eviction left the log alone, and vice versa.
    assert_eq!(store.stats().logs, 1);
    assert!(store.trace(tid(1)).is_none());
    store.ingest_logs(vec![log("a", "l2", 2)]);
    assert!(store.trace(tid(2)).is_some());
    assert_eq!(bodies(&everything(&store).items), vec!["l2"]);
}

#[test]
fn clones_share_logs_across_threads() {
    let (store, _c) = store_with_manual_clock();
    let writers: Vec<_> = (0..4u64)
        .map(|worker| {
            let store = store.clone();
            thread::spawn(move || {
                for i in 0..100u64 {
                    store.ingest_logs(vec![log("a", &format!("w{worker}-{i}"), worker * 1000 + i)]);
                }
            })
        })
        .collect();
    for writer in writers {
        writer.join().unwrap();
    }
    let stats = store.stats();
    assert_eq!((stats.logs, stats.logs_received), (400, 400));
    assert_eq!(everything(&store).items.len(), 400);
}
