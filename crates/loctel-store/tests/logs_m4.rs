//! Logs for the Logs screen (M4 plan, Task 1): stable log IDs, keyed search,
//! the level and window filters, the volume histogram and the lines around a
//! log.
//!
//! The plan's eight tests come first. The rest pin the corners the UI relies
//! on: paging with a window, bucket edges, empty buckets and eviction.

mod fixtures;

use fixtures::*;
use loctel_meshgen::Scenario;
use loctel_model::{LogRecord, Severity};
use loctel_store::{
    HistogramBucket, LevelSet, LogFilter, LogId, Page, PageRequest, Store, StoreConfig,
};

const MS: u64 = 1_000_000;
const MINUTE: u64 = 60_000_000_000;

fn first(limit: usize) -> PageRequest {
    PageRequest {
        limit,
        cursor: None,
    }
}

fn page_at(limit: usize, cursor: &Option<String>) -> PageRequest {
    PageRequest {
        limit,
        cursor: cursor.clone(),
    }
}

fn bodies(items: &[(LogId, LogRecord)]) -> Vec<&str> {
    items.iter().map(|(_, l)| l.body.as_str()).collect()
}

fn all_keyed(store: &Store, f: &LogFilter) -> Page<(LogId, LogRecord)> {
    store.search_logs_keyed(f, &first(usize::MAX))
}

fn with_cap(max_log_records: usize) -> Store {
    store_with_config(StoreConfig {
        max_log_records,
        ..StoreConfig::default()
    })
    .0
}

/// Logs `l1`..`l{n}` of one service at 10 ms, 20 ms, ... in the given order
/// of arrival.
fn numbered(n: u64) -> Vec<LogRecord> {
    (1..=n)
        .map(|i| log("orders-api", &format!("l{i}"), i * 10))
        .collect()
}

// --- the plan's tests ---

#[test]
fn keyed_search_matches_unkeyed() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_logs(vec![
        log("orders-api", "order created", 10),
        with_severity(log("payments-api", "charge declined", 20), Severity::Error),
        log("orders-api", "order shipped", 30),
        in_span(log("orders-api", "inside a span", 40), 1, 2),
        // The same time as the line before: a tie goes to the later arrival.
        log("web-bff", "checkout", 40),
        with_severity(log("orders-api", "slow query", 50), Severity::Warn),
    ]);

    let filters = [
        LogFilter::default(),
        LogFilter {
            service: Some("orders-api".into()),
            ..LogFilter::default()
        },
        LogFilter {
            min_severity: Some(Severity::Warn),
            ..LogFilter::default()
        },
        LogFilter {
            contains: Some("ORDER".into()),
            ..LogFilter::default()
        },
        LogFilter {
            trace_id: Some(tid(1)),
            ..LogFilter::default()
        },
        LogFilter {
            since_unix_nano: Some(30 * MS),
            until_unix_nano: Some(50 * MS),
            levels: Some(LevelSet::ALL),
            ..LogFilter::default()
        },
    ];
    for filter in &filters {
        for limit in [1, 2, 3, 100] {
            // Page through both ways at once: the records, the cursors and
            // the order must agree on every page.
            let mut unkeyed = None;
            let mut keyed = None;
            loop {
                let plain = store.search_logs(filter, &page_at(limit, &unkeyed));
                let with_ids = store.search_logs_keyed(filter, &page_at(limit, &keyed));
                let from_keyed: Vec<LogRecord> =
                    with_ids.items.iter().map(|(_, l)| l.clone()).collect();
                assert_eq!(plain.items, from_keyed, "{filter:?} limit {limit}");
                assert_eq!(plain.next_cursor, with_ids.next_cursor);
                // Newest first, so the IDs strictly descend.
                assert!(with_ids.items.windows(2).all(|w| w[0].0 > w[1].0));
                unkeyed = plain.next_cursor;
                keyed = with_ids.next_cursor;
                if unkeyed.is_none() {
                    break;
                }
            }
        }
    }

    // A cursor that was not produced here gives an empty page, keyed too.
    let bad = store.search_logs_keyed(
        &LogFilter::default(),
        &PageRequest {
            limit: 5,
            cursor: Some("nonsense".into()),
        },
    );
    assert!(bad.items.is_empty() && bad.next_cursor.is_none());
    assert!(
        store
            .search_logs_keyed(&LogFilter::default(), &first(0))
            .items
            .is_empty()
    );
}

#[test]
fn log_by_id() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_logs(vec![
        log("orders-api", "first", 10),
        log("payments-api", "second", 20),
    ]);
    let held = all_keyed(&store, &LogFilter::default()).items;
    assert_eq!(bodies(&held), vec!["second", "first"]);

    for (id, record) in &held {
        assert_eq!(store.log(*id).as_ref(), Some(record));
        // The ID carries the time the log is ordered by.
        assert_eq!(id.time_unix_nano, record.time_unix_nano);
    }
    // An ID that was never issued.
    let never = LogId {
        time_unix_nano: 20 * MS,
        seq: 9_999,
    };
    assert_eq!(store.log(never), None);

    // A log without a timestamp is ordered, and so identified, by its
    // observed time.
    let mut unstamped = log("orders-api", "no timestamp", 0);
    unstamped.observed_unix_nano = 77 * MS;
    store.ingest_logs(vec![unstamped.clone()]);
    let newest = store
        .search_logs_keyed(&LogFilter::default(), &first(1))
        .items;
    assert_eq!(bodies(&newest), vec!["no timestamp"]);
    assert_eq!(newest[0].0.time_unix_nano, 77 * MS);
    assert_eq!(store.log(newest[0].0), Some(unstamped));

    // Gone once the log is gone, and never given to a later log.
    let (gone, _) = held[1].clone();
    store.clear();
    assert_eq!(store.log(gone), None);
    store.ingest_logs(vec![log("orders-api", "first", 10)]);
    assert_eq!(store.log(gone), None);
    let again = all_keyed(&store, &LogFilter::default()).items;
    assert_eq!(bodies(&again), vec!["first"]);
    assert_ne!(again[0].0, held[1].0);
}

#[test]
fn until_is_exclusive() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_logs(numbered(5));
    let page = |f: LogFilter| {
        let keyed = store.search_logs_keyed(&f, &first(100)).items;
        keyed
            .into_iter()
            .map(|(_, l)| l.body)
            .collect::<Vec<String>>()
    };

    // A log at exactly `until` is out, one at exactly `since` is in.
    let window = page(LogFilter {
        since_unix_nano: Some(20 * MS),
        until_unix_nano: Some(40 * MS),
        ..LogFilter::default()
    });
    assert_eq!(window, vec!["l3", "l2"]);
    let up_to = page(LogFilter {
        until_unix_nano: Some(30 * MS),
        ..LogFilter::default()
    });
    assert_eq!(up_to, vec!["l2", "l1"]);
    let nothing = page(LogFilter {
        until_unix_nano: Some(10 * MS),
        ..LogFilter::default()
    });
    assert!(nothing.is_empty());
    let empty_window = page(LogFilter {
        since_unix_nano: Some(40 * MS),
        until_unix_nano: Some(40 * MS),
        ..LogFilter::default()
    });
    assert!(empty_window.is_empty());

    // The unkeyed search takes it too, and paging inside a window keeps its
    // edges: the cursor and `until` both bound the next page.
    let filter = LogFilter {
        since_unix_nano: Some(10 * MS),
        until_unix_nano: Some(50 * MS),
        ..LogFilter::default()
    };
    let one = store.search_logs(&filter, &first(2));
    let texts: Vec<&str> = one.items.iter().map(|l| l.body.as_str()).collect();
    assert_eq!(texts, vec!["l4", "l3"]);
    let two = store.search_logs(&filter, &page_at(2, &one.next_cursor));
    let texts: Vec<&str> = two.items.iter().map(|l| l.body.as_str()).collect();
    assert_eq!(texts, vec!["l2", "l1"]);
    assert_eq!(two.next_cursor, None);

    // A cursor newer than `until` does not let newer logs through.
    let newest = store.search_logs(&LogFilter::default(), &first(1));
    let stale = LogFilter {
        until_unix_nano: Some(30 * MS),
        ..LogFilter::default()
    };
    let page = store.search_logs(&stale, &page_at(10, &newest.next_cursor));
    let texts: Vec<&str> = page.items.iter().map(|l| l.body.as_str()).collect();
    assert_eq!(texts, vec!["l2", "l1"]);
}

#[test]
fn level_groups() {
    use Severity::*;
    let members = |set: LevelSet| -> Vec<Severity> {
        [Unspecified, Trace, Debug, Info, Warn, Error, Fatal]
            .into_iter()
            .filter(|&s| set.contains(s))
            .collect()
    };

    // ERR holds error and fatal, and not warn.
    let err = LevelSet::from_groups(true, false, false, false);
    assert_eq!(members(err), vec![Error, Fatal]);
    assert!(!err.contains(Warn));
    // WRN is warn alone.
    assert_eq!(
        members(LevelSet::from_groups(false, true, false, false)),
        vec![Warn]
    );
    // INF includes unspecified.
    let inf = LevelSet::from_groups(false, false, true, false);
    assert_eq!(members(inf), vec![Unspecified, Info]);
    assert!(inf.contains(Unspecified));
    // DBG is debug and trace.
    assert_eq!(
        members(LevelSet::from_groups(false, false, false, true)),
        vec![Trace, Debug]
    );
    // The groups add up; all four is every level, none is no level.
    assert_eq!(LevelSet::from_groups(true, true, true, true), LevelSet::ALL);
    assert_eq!(members(LevelSet::ALL).len(), 7);
    let none = LevelSet::from_groups(false, false, false, false);
    assert_eq!(none, LevelSet::default());
    assert!(members(none).is_empty());
    // `with` adds one level at a time (twice is once) and leaves the
    // original alone.
    let empty = LevelSet::default();
    let built = empty.with(Warn).with(Fatal).with(Warn);
    assert_eq!(members(built), vec![Warn, Fatal]);
    assert!(members(empty).is_empty());
}

#[test]
fn histogram_buckets_and_errors() {
    let (store, _c) = store_with_manual_clock();
    // The fixed checkout failure: 7 logs between 10 ms and 1800 ms after
    // the base, 3 of them errors, 1 a warning.
    let base = 5_000 * MINUTE;
    let logs = scenario_logs(&Scenario::CheckoutFailure, 1, base);
    assert_eq!(logs.len(), 7);
    store.ingest_logs(logs);

    // 15 minutes ending 2 s after the base: every log is in the last bucket
    // (18.75 s wide).
    let until = base + 2_000 * MS;
    let since = until - 15 * MINUTE;
    let histogram = store.log_histogram(&LogFilter::default(), since, until, 48);
    assert_eq!(histogram.len(), 48);
    assert_eq!(
        histogram[47],
        HistogramBucket {
            total: 7,
            errors: 3
        }
    );
    assert!(
        histogram[..47]
            .iter()
            .all(|b| *b == HistogramBucket::default())
    );
}

#[test]
fn histogram_respects_filter() {
    let (store, _c) = store_with_manual_clock();
    let base = 5_000 * MINUTE;
    store.ingest_logs(scenario_logs(&Scenario::CheckoutFailure, 1, base));
    let until = base + 2_000 * MS;
    let since = until - 15 * MINUTE;
    let last = |f: &LogFilter| {
        let h = store.log_histogram(f, since, until, 48);
        assert_eq!(h.len(), 48);
        // Everything is in the last bucket, whatever the filter.
        assert!(h[..47].iter().all(|b| *b == HistogramBucket::default()));
        h[47]
    };

    // By service: orders-api logged 3 lines (1 error), payments-api 4 (2
    // errors) and web-bff none.
    let service = |name: &str| LogFilter {
        service: Some(name.into()),
        ..LogFilter::default()
    };
    assert_eq!(
        last(&service("orders-api")),
        HistogramBucket {
            total: 3,
            errors: 1
        }
    );
    assert_eq!(
        last(&service("payments-api")),
        HistogramBucket {
            total: 4,
            errors: 2
        }
    );
    assert_eq!(last(&service("web-bff")), HistogramBucket::default());

    // By level group: errors-only keeps the 3 errors (all of them errors),
    // warnings only has the one warning and no errors.
    let err_only = LogFilter {
        levels: Some(LevelSet::from_groups(true, false, false, false)),
        ..LogFilter::default()
    };
    assert_eq!(
        last(&err_only),
        HistogramBucket {
            total: 3,
            errors: 3
        }
    );
    let wrn_only = LogFilter {
        levels: Some(LevelSet::from_groups(false, true, false, false)),
        ..LogFilter::default()
    };
    assert_eq!(
        last(&wrn_only),
        HistogramBucket {
            total: 1,
            errors: 0
        }
    );
    // No level at all matches nothing.
    let no_levels = LogFilter {
        levels: Some(LevelSet::default()),
        ..LogFilter::default()
    };
    assert_eq!(last(&no_levels), HistogramBucket::default());

    // By text, ignoring case: the card gateway is named in 3 lines, 2 of
    // them errors.
    let text = LogFilter {
        contains: Some("CARD-GATEWAY".into()),
        ..LogFilter::default()
    };
    assert_eq!(
        last(&text),
        HistogramBucket {
            total: 3,
            errors: 2
        }
    );

    // The filter's own time fields are ignored: the window is the
    // `since`/`until` arguments, even if the filter says otherwise.
    let ignoring = LogFilter {
        since_unix_nano: Some(until + MINUTE),
        until_unix_nano: Some(since),
        ..LogFilter::default()
    };
    assert_eq!(
        last(&ignoring),
        HistogramBucket {
            total: 7,
            errors: 3
        }
    );
}

#[test]
fn logs_around_target_in_middle() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_logs(numbered(9));
    let ids: Vec<LogId> = all_keyed(&store, &LogFilter::default())
        .items
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    // Newest first: l9 is ids[0], l5 is ids[4].
    let around = store.logs_around(ids[4], 3, 3);
    assert_eq!(
        bodies(&around.lines),
        vec!["l2", "l3", "l4", "l5", "l6", "l7", "l8"]
    );
    assert_eq!(around.target, Some(3));
    assert_eq!(around.lines[3].0, ids[4]);
    // Oldest first, and each line comes with its own ID.
    assert!(around.lines.windows(2).all(|w| w[0].0 < w[1].0));
    for (id, record) in &around.lines {
        assert_eq!(store.log(*id).as_ref(), Some(record));
    }

    // Near the edges there are fewer lines, and the target moves with them.
    let at_oldest = store.logs_around(ids[8], 3, 2);
    assert_eq!(bodies(&at_oldest.lines), vec!["l1", "l2", "l3"]);
    assert_eq!(at_oldest.target, Some(0));
    let at_newest = store.logs_around(ids[0], 2, 3);
    assert_eq!(bodies(&at_newest.lines), vec!["l7", "l8", "l9"]);
    assert_eq!(at_newest.target, Some(2));
    // Asking for none on a side gives none.
    let alone = store.logs_around(ids[4], 0, 0);
    assert_eq!(bodies(&alone.lines), vec!["l5"]);
    assert_eq!(alone.target, Some(0));
}

#[test]
fn logs_around_evicted_target() {
    // The target itself was evicted: what remains is the lines nearest to
    // its time, and no target.
    let store = with_cap(5);
    store.ingest_logs(numbered(5));
    let ids: Vec<LogId> = all_keyed(&store, &LogFilter::default())
        .items
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    let (l1, l3) = (ids[4], ids[2]);
    store.ingest_logs(
        (6..=10)
            .map(|i| log("orders-api", &format!("l{i}"), i * 10))
            .collect(),
    );
    // l1..l5 are gone, l6..l10 are held.
    assert_eq!(store.log(l1), None);
    let around = store.logs_around(l1, 3, 3);
    assert_eq!(bodies(&around.lines), vec!["l6", "l7", "l8"]);
    assert_eq!(around.target, None);
    let around = store.logs_around(l3, 2, 1);
    assert_eq!(bodies(&around.lines), vec!["l6"]);
    assert_eq!(around.target, None);

    // Everything gone: an empty result, no target, no panic.
    store.clear();
    let around = store.logs_around(l1, 3, 3);
    assert!(around.lines.is_empty());
    assert_eq!(around.target, None);

    // The target removed from the middle: the lines on both sides of where
    // it was, nothing at its place.
    let (store, _c) = store_with_manual_clock();
    let mut logs = numbered(9);
    logs[4] = in_span(logs[4].clone(), 7, 7);
    store.ingest_logs(logs);
    let target = store
        .search_logs_keyed(
            &LogFilter {
                trace_id: Some(tid(7)),
                ..LogFilter::default()
            },
            &first(1),
        )
        .items[0]
        .0;
    assert_eq!(store.remove_trace(tid(7)).logs, 1);
    let around = store.logs_around(target, 3, 3);
    assert_eq!(
        bodies(&around.lines),
        vec!["l2", "l3", "l4", "l6", "l7", "l8"]
    );
    assert_eq!(around.target, None);

    // The neighbours evicted but the target held: it is the oldest line.
    let store = with_cap(3);
    store.ingest_logs(numbered(6));
    let oldest = store
        .search_logs_keyed(&LogFilter::default(), &first(100))
        .items
        .last()
        .unwrap()
        .0;
    let around = store.logs_around(oldest, 3, 1);
    assert_eq!(bodies(&around.lines), vec!["l4", "l5"]);
    assert_eq!(around.target, Some(0));
}

// --- the corners ---

#[test]
fn levels_filter_combines_with_the_other_conditions() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_logs(vec![
        with_severity(log("a", "debug line", 10), Severity::Debug),
        with_severity(log("a", "trace line", 20), Severity::Trace),
        with_severity(log("a", "info line", 30), Severity::Info),
        with_severity(log("b", "unlabelled line", 40), Severity::Unspecified),
        with_severity(log("a", "warn line", 50), Severity::Warn),
        with_severity(log("b", "error line", 60), Severity::Error),
        with_severity(log("a", "fatal line", 70), Severity::Fatal),
    ]);
    let search = |f: LogFilter| -> Vec<String> {
        store
            .search_logs_keyed(&f, &first(100))
            .items
            .into_iter()
            .map(|(_, l)| l.body)
            .collect()
    };

    // `None` is every level, `ALL` is the same, and a set picks out levels
    // that `min_severity` (a threshold) cannot, like "debug but not info".
    assert_eq!(search(LogFilter::default()).len(), 7);
    assert_eq!(
        search(LogFilter {
            levels: Some(LevelSet::ALL),
            ..LogFilter::default()
        })
        .len(),
        7
    );
    assert_eq!(
        search(LogFilter {
            levels: Some(LevelSet::from_groups(true, false, false, true)),
            ..LogFilter::default()
        }),
        vec!["fatal line", "error line", "trace line", "debug line"]
    );
    assert_eq!(
        search(LogFilter {
            levels: Some(LevelSet::from_groups(false, false, true, false)),
            ..LogFilter::default()
        }),
        vec!["unlabelled line", "info line"]
    );
    // Every condition must hold: levels AND min_severity AND service.
    assert_eq!(
        search(LogFilter {
            levels: Some(LevelSet::from_groups(true, true, true, true)),
            min_severity: Some(Severity::Warn),
            service: Some("a".into()),
            ..LogFilter::default()
        }),
        vec!["fatal line", "warn line"]
    );
    // An empty set of levels matches nothing.
    assert!(
        search(LogFilter {
            levels: Some(LevelSet::default()),
            ..LogFilter::default()
        })
        .is_empty()
    );
}

#[test]
fn log_ids_survive_late_arrivals_and_eviction_of_others() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_logs(vec![
        log("a", "newest", 30),
        log("a", "oldest", 10),
        log("a", "middle", 20),
    ]);
    let before = all_keyed(&store, &LogFilter::default()).items;
    assert_eq!(bodies(&before), vec!["newest", "middle", "oldest"]);

    // A log that arrives later but happened in between does not change the
    // IDs of the held ones.
    store.ingest_logs(vec![log("a", "late", 25)]);
    let now = all_keyed(&store, &LogFilter::default()).items;
    assert_eq!(bodies(&now), vec!["newest", "late", "middle", "oldest"]);
    for (id, record) in &before {
        assert_eq!(store.log(*id).as_ref(), Some(record));
    }

    // Logs with the same time are told apart by arrival.
    store.ingest_logs(vec![log("b", "tie", 30)]);
    let ties: Vec<LogId> = all_keyed(&store, &LogFilter::default())
        .items
        .into_iter()
        .filter(|(id, _)| id.time_unix_nano == 30 * MS)
        .map(|(id, _)| id)
        .collect();
    assert_eq!(ties.len(), 2);
    assert_ne!(ties[0], ties[1]);
    assert_eq!(ties[0].time_unix_nano, ties[1].time_unix_nano);
    assert!(ties[0].seq > ties[1].seq, "the later arrival is newer");
}

#[test]
fn histogram_edges_and_empty_inputs() {
    let (store, _c) = store_with_manual_clock();
    // Ten buckets of 10 ms over [100 ms, 200 ms).
    store.ingest_logs(vec![
        log("a", "before the window", 99),
        log("a", "first edge", 100),
        log("a", "still the first", 109),
        log("a", "second edge", 110),
        with_severity(log("a", "mid", 155), Severity::Error),
        with_severity(log("a", "last", 199), Severity::Fatal),
        log("a", "until is exclusive", 200),
    ]);
    let h = store.log_histogram(&LogFilter::default(), 100 * MS, 200 * MS, 10);
    let totals: Vec<u64> = h.iter().map(|b| b.total).collect();
    assert_eq!(totals, vec![2, 1, 0, 0, 0, 1, 0, 0, 0, 1]);
    let errors: Vec<u64> = h.iter().map(|b| b.errors).collect();
    assert_eq!(errors, vec![0, 0, 0, 0, 0, 1, 0, 0, 0, 1]);

    // Buckets that do not divide the window evenly still tile it: every log
    // in it is counted once.
    let h = store.log_histogram(&LogFilter::default(), 100 * MS, 200 * MS, 7);
    assert_eq!(h.len(), 7);
    assert_eq!(h.iter().map(|b| b.total).sum::<u64>(), 5);

    // An empty or backwards window, or no buckets, is still exactly
    // `buckets` long (or empty), and counts nothing.
    let f = LogFilter::default();
    assert!(store.log_histogram(&f, 100 * MS, 200 * MS, 0).is_empty());
    for (since, until) in [(150 * MS, 150 * MS), (200 * MS, 100 * MS)] {
        let h = store.log_histogram(&f, since, until, 12);
        assert_eq!(h, vec![HistogramBucket::default(); 12]);
    }
    // A window wider than u64 nanoseconds can multiply does not overflow.
    let h = store.log_histogram(&f, 0, u64::MAX, 48);
    assert_eq!(h.len(), 48);
    assert_eq!(h.iter().map(|b| b.total).sum::<u64>(), 7);
    // A one-nanosecond window per bucket and more buckets than nanoseconds.
    let h = store.log_histogram(&f, 100 * MS, 100 * MS + 3, 10);
    assert_eq!(h.len(), 10);
    assert_eq!(h.iter().map(|b| b.total).sum::<u64>(), 1);
}

#[test]
fn histogram_counts_unstamped_logs_by_observed_time() {
    let (store, _c) = store_with_manual_clock();
    let mut unstamped = log("a", "no timestamp", 0);
    unstamped.observed_unix_nano = 150 * MS;
    store.ingest_logs(vec![unstamped]);
    let h = store.log_histogram(&LogFilter::default(), 100 * MS, 200 * MS, 2);
    assert_eq!(h.iter().map(|b| b.total).collect::<Vec<_>>(), vec![0, 1]);
}
