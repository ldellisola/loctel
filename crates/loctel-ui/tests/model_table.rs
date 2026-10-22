//! The trace table's rows, the service colours and the query bar's states
//! (M3 plan, Task 1), built from the fixed fake mesh: `CheckoutFailure` (15
//! spans, the root and the card-gateway calls erroring) and `CheckoutSuccess`
//! (14 spans, nothing erroring) in a store on a manual clock.

use std::sync::Arc;
use std::time::Duration;

use loctel_ingest::{logs_from_request, spans_from_request};
use loctel_meshgen::{Generated, SERVICES, Scenario, generate};
use loctel_model::{SpanId, TraceId};
use loctel_query::traceql;
use loctel_store::{
    FailedIn, ManualClock, PageRequest, Store, StoreConfig, TraceFilter, TraceStatus, TraceSummary,
};
use loctel_ui::model::colors::ServiceColors;
use loctel_ui::model::query_bar::{QueryState, service_filter, validate};
use loctel_ui::model::trace_table::{self, TraceRow, failed_in_text};

/// 2027-01-15T08:00:00Z. The failing checkout starts here.
const BASE: u64 = 1_800_000_000_000_000_000;
const MS: u64 = 1_000_000;
const SECOND: u64 = 1_000 * MS;
const SEED: u64 = 7;

const FAILED_IN: &str = "payments-api \u{b7} POST card-gateway +1";

/// A store holding the failing checkout (at `BASE`) and the successful one
/// (five seconds later, so it is the newer), and the clock that dates them.
struct Fixture {
    store: Store,
    clock: Arc<ManualClock>,
    failure: TraceId,
    success: TraceId,
}

fn feed(store: &Store, generated: &Generated, batches: std::ops::Range<usize>) {
    for batch in &generated.batches[batches] {
        if let Some(request) = &batch.traces {
            let converted = spans_from_request(request.clone());
            assert_eq!(converted.rejected, 0, "generated spans are valid");
            store.ingest_spans(converted.items);
        }
        if let Some(request) = &batch.logs {
            let converted = logs_from_request(request.clone());
            assert_eq!(converted.rejected, 0, "generated logs are valid");
            store.ingest_logs(converted.items);
        }
    }
}

fn new_store() -> (Store, Arc<ManualClock>) {
    let clock = Arc::new(ManualClock::new(BASE));
    let store = Store::new(StoreConfig::default(), clock.clone());
    (store, clock)
}

fn fixture() -> Fixture {
    let (store, clock) = new_store();
    let failure = generate(&Scenario::CheckoutFailure, SEED, BASE);
    let success = generate(&Scenario::CheckoutSuccess, SEED, BASE + 5 * SECOND);
    feed(&store, &failure, 0..failure.batches.len());
    feed(&store, &success, 0..success.batches.len());
    Fixture {
        store,
        clock,
        failure: failure.trace_ids[0],
        success: success.trace_ids[0],
    }
}

impl Fixture {
    /// Moves the clock past the 10 s quiet period: every trace is complete.
    fn settle(&self) {
        self.clock.advance(Duration::from_secs(60));
    }

    /// What the table is built from: the store's summaries, newest first.
    fn summaries(&self) -> Vec<TraceSummary> {
        self.store
            .search_traces(
                &TraceFilter::default(),
                &PageRequest {
                    limit: 100,
                    cursor: None,
                },
            )
            .items
    }

    fn summary(&self, id: TraceId) -> TraceSummary {
        self.store.summary(id).expect("the trace is held")
    }
}

fn row_of(rows: &[TraceRow], id: TraceId) -> &TraceRow {
    rows.iter()
        .find(|row| row.trace_id == id)
        .expect("the trace has a row")
}

fn names(row: &TraceRow) -> Vec<&str> {
    row.services.iter().map(|(name, _)| name.as_str()).collect()
}

fn indexes(row: &TraceRow) -> Vec<usize> {
    row.services.iter().map(|&(_, index)| index).collect()
}

// --- rows -------------------------------------------------------------------

#[test]
fn failed_trace_row() {
    let f = fixture();
    f.settle();
    let summaries = f.summaries();
    let rows = trace_table::rows(&summaries, &mut ServiceColors::new(), 0);

    let row = row_of(&rows, f.failure);
    assert_eq!(row.failed_in.as_deref(), Some(FAILED_IN));
    assert!(!row.errors_inside);

    // The rest of the row, as the mockup's first line reads.
    assert_eq!(row.root, "POST /api/checkout");
    assert_eq!(row.started, "08:00:00.000");
    assert_eq!(row.duration, "1.84 s");
    assert_eq!(row.span_count, 15);
    assert!(row.complete);
}

#[test]
fn ok_trace_row_has_no_failure_text() {
    let f = fixture();
    f.settle();
    let summaries = f.summaries();
    let rows = trace_table::rows(&summaries, &mut ServiceColors::new(), 0);

    let row = row_of(&rows, f.success);
    assert_eq!(row.failed_in, None);
    assert!(!row.errors_inside);
    assert_eq!(row.root, "POST /api/checkout");
    assert_eq!(row.started, "08:00:05.000");
    assert_eq!(row.span_count, 14);
}

#[test]
fn errors_inside_is_amber_and_not_failed() {
    let f = fixture();
    // The root is fine and a child erroring: the store gives such a trace the
    // status `ErrorsInside` and still names the lowest error in `failed_in`.
    let mut summary = f.summary(f.failure);
    summary.status = TraceStatus::ErrorsInside;
    assert!(summary.failed_in.is_some());

    let rows = trace_table::rows(&[summary], &mut ServiceColors::new(), 0);
    assert_eq!(
        rows[0].failed_in, None,
        "the red `Failed in` is for failures"
    );
    assert!(rows[0].errors_inside);
}

#[test]
fn started_follows_the_zone_offset() {
    let f = fixture();
    let summaries = vec![f.summary(f.failure)];
    let utc = trace_table::rows(&summaries, &mut ServiceColors::new(), 0);
    let plus_two = trace_table::rows(&summaries, &mut ServiceColors::new(), 2 * 3_600);
    assert_eq!(utc[0].started, "08:00:00.000");
    assert_eq!(plus_two[0].started, "10:00:00.000");
}

#[test]
fn complete_follows_the_store() {
    let f = fixture();
    let before = trace_table::rows(&f.summaries(), &mut ServiceColors::new(), 0);
    assert!(before.iter().all(|row| !row.complete), "just arrived");
    f.settle();
    let after = trace_table::rows(&f.summaries(), &mut ServiceColors::new(), 0);
    assert!(after.iter().all(|row| row.complete), "quiet for a while");
}

#[test]
fn root_not_arrived_reads_a_placeholder() {
    // `LateOutOfOrder` sends the leaves first and the root last. Only the
    // first batch has arrived: no root yet.
    let (store, _clock) = new_store();
    let late = generate(&Scenario::LateOutOfOrder, SEED, BASE);
    assert_eq!(late.batches.len(), 3);
    feed(&store, &late, 0..1);
    let summary = store.summary(late.trace_ids[0]).expect("the trace is held");
    assert_eq!(summary.root_name, None);

    let rows = trace_table::rows(&[summary], &mut ServiceColors::new(), 0);
    assert_eq!(rows[0].root, "(root not arrived)");

    // The root arrives and the row names it.
    feed(&store, &late, 1..3);
    let summary = store.summary(late.trace_ids[0]).expect("the trace is held");
    let rows = trace_table::rows(&[summary], &mut ServiceColors::new(), 0);
    assert_eq!(rows[0].root, "POST /api/checkout");
}

#[test]
fn rows_keep_the_order_of_the_summaries() {
    let f = fixture();
    let summaries = f.summaries();
    assert_eq!(summaries[0].trace_id, f.success, "newest first");
    let rows = trace_table::rows(&summaries, &mut ServiceColors::new(), 0);
    let ids: Vec<TraceId> = rows.iter().map(|row| row.trace_id).collect();
    assert_eq!(ids, vec![f.success, f.failure]);
    assert!(trace_table::rows(&[], &mut ServiceColors::new(), 0).is_empty());
}

// --- service colours ---------------------------------------------------------

#[test]
fn services_in_first_seen_order_with_stable_colors() {
    let f = fixture();
    f.settle();
    let summaries = f.summaries();
    let mut colors = ServiceColors::new();

    let first = trace_table::rows(&summaries, &mut colors, 0);
    for row in &first {
        // Each trace lists its services in the order of their first span:
        // the mesh's order, so `web-bff` is 0 and `orders-api` 1.
        assert_eq!(names(row), SERVICES, "{}", row.trace_id);
        assert_eq!(indexes(row), vec![0, 1, 2, 3, 4], "{}", row.trace_id);
    }
    assert_eq!(colors.peek("web-bff"), Some(0));
    assert_eq!(colors.peek("orders-api"), Some(1));
    assert_eq!(colors.peek("notifications-worker"), Some(4));

    // The same again from the same colours, and from fresh ones: nothing
    // moves.
    assert_eq!(trace_table::rows(&summaries, &mut colors, 0), first);
    assert_eq!(
        trace_table::rows(&summaries, &mut ServiceColors::new(), 0),
        first
    );
}

#[test]
fn colors_follow_the_order_across_rows() {
    let f = fixture();
    let mut one = f.summary(f.failure);
    one.services = vec!["b".to_owned(), "a".to_owned()];
    let mut two = f.summary(f.success);
    two.services = vec!["a".to_owned(), "c".to_owned(), "b".to_owned()];
    let mut colors = ServiceColors::new();

    let rows = trace_table::rows(&[one, two], &mut colors, 0);
    assert_eq!(
        rows[0].services,
        vec![("b".to_owned(), 0), ("a".to_owned(), 1)]
    );
    assert_eq!(
        rows[1].services,
        vec![
            ("a".to_owned(), 1),
            ("c".to_owned(), 2),
            ("b".to_owned(), 0)
        ]
    );
}

#[test]
fn service_colors_api() {
    let mut colors = ServiceColors::new();
    assert_eq!(colors.peek("web-bff"), None);
    // Looking does not assign.
    assert_eq!(colors.peek("web-bff"), None);

    assert_eq!(colors.index("web-bff"), 0);
    assert_eq!(colors.index("orders-api"), 1);
    assert_eq!(colors.index("web-bff"), 0, "stable");
    assert_eq!(colors.peek("orders-api"), Some(1));
    assert_eq!(colors.peek("payments-api"), None);

    // The index counts on past the palette's five colours; the palette slot
    // is `index % 5`, so the sixth service shares the first one's colour.
    for (expected, name) in ["c", "d", "e", "f"].iter().enumerate() {
        assert_eq!(colors.index(name), expected + 2);
    }
    assert_eq!(colors.index("f"), 5);
    assert_eq!(colors.index("f") % 5, colors.index("web-bff"));

    // Names are exact: no trimming, no case folding.
    assert_eq!(colors.peek("WEB-BFF"), None);
    assert_eq!(colors.peek("web-bff "), None);
}

// --- duration bar ------------------------------------------------------------

#[test]
fn duration_fraction_relative_to_longest() {
    let f = fixture();
    let longest = f.summary(f.failure);
    assert_eq!(longest.duration_nanos, 1_840 * MS);
    let mut quarter = f.summary(f.success);
    quarter.duration_nanos = 460 * MS;
    let mut instant = f.summary(f.success);
    instant.duration_nanos = 0;

    let rows = trace_table::rows(
        &[quarter.clone(), longest, instant],
        &mut ServiceColors::new(),
        0,
    );
    assert_eq!(rows[1].duration_fraction, 1.0, "the longest fills the bar");
    assert_eq!(rows[0].duration_fraction, 0.25);
    assert_eq!(rows[2].duration_fraction, 0.0);
    assert_eq!(rows[0].duration, "460 ms");
    assert_eq!(rows[2].duration, "0 \u{b5}s");

    // The longest is the longest of the rows given, not of the store: leave
    // the long one out and the next one fills the bar.
    let rows = trace_table::rows(&[quarter], &mut ServiceColors::new(), 0);
    assert_eq!(rows[0].duration_fraction, 1.0);

    // The real mesh: both checkouts last 1.84 s, so both fill the bar.
    let summaries = f.summaries();
    let rows = trace_table::rows(&summaries, &mut ServiceColors::new(), 0);
    assert!(rows.iter().all(|row| row.duration_fraction == 1.0));
}

#[test]
fn duration_fraction_with_nothing_to_divide_by() {
    let f = fixture();
    let mut a = f.summary(f.failure);
    a.duration_nanos = 0;
    let mut b = f.summary(f.success);
    b.duration_nanos = 0;
    let rows = trace_table::rows(&[a, b], &mut ServiceColors::new(), 0);
    for row in rows {
        // Never NaN: a bar of no length.
        assert_eq!(row.duration_fraction, 0.0);
    }
}

// --- failed-in text ----------------------------------------------------------

#[test]
fn failed_in_text_cases() {
    let failed = |service: &str, span_name: &str, peer: Option<&str>, more: usize| FailedIn {
        span_id: SpanId::from_hex("00000000000000a1").unwrap(),
        service: service.to_owned(),
        span_name: span_name.to_owned(),
        peer: peer.map(str::to_owned),
        more,
    };

    // The checkout: a client span with a peer, and one more like it.
    assert_eq!(
        failed_in_text(&failed("payments-api", "POST", Some("card-gateway"), 1)),
        FAILED_IN
    );
    assert_eq!(
        failed_in_text(&failed("payments-api", "POST", Some("card-gateway"), 0)),
        "payments-api \u{b7} POST card-gateway"
    );
    // A server or internal span has no peer.
    assert_eq!(
        failed_in_text(&failed("payments-api", "POST /charges", None, 0)),
        "payments-api \u{b7} POST /charges"
    );
    assert_eq!(
        failed_in_text(&failed("payments-api", "POST /charges", None, 2)),
        "payments-api \u{b7} POST /charges +2"
    );

    // And as the store works it out for the real trace.
    let f = fixture();
    let found = f.summary(f.failure).failed_in.expect("the trace failed");
    assert_eq!(failed_in_text(&found), FAILED_IN);
}

// --- query bar ---------------------------------------------------------------

#[test]
fn query_bar_states() {
    assert_eq!(validate(""), QueryState::Empty);

    match validate("{ status = error }") {
        QueryState::Valid(query) => {
            assert_eq!(*query, traceql::parse("{ status = error }").unwrap());
        }
        other => panic!("expected Valid, got {other:?}"),
    }

    // The text stops where a value was due: the 12th column, one past the
    // 11 characters typed.
    match validate("{ status = ") {
        QueryState::Invalid { column, message } => {
            assert_eq!(column, 12);
            assert!(!message.is_empty());
            assert!(
                !message.contains("at column"),
                "the column is a field of its own: {message}"
            );
        }
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[test]
fn query_bar_whitespace_is_empty() {
    for text in [" ", "   ", "\t", "\n", " \r\n\t "] {
        assert_eq!(validate(text), QueryState::Empty, "{text:?}");
    }
}

#[test]
fn query_bar_columns_count_characters_in_the_text_as_typed() {
    // Leading blanks are part of what the user typed, so the column counts
    // them.
    match validate("  { status = ") {
        QueryState::Invalid { column, .. } => assert_eq!(column, 14),
        other => panic!("expected Invalid, got {other:?}"),
    }
    // Characters, not bytes: `é` and `日` are one column each.
    let text = "{ name = \"é日\" && ";
    match validate(text) {
        QueryState::Invalid { column, .. } => assert_eq!(column, text.chars().count() + 1),
        other => panic!("expected Invalid, got {other:?}"),
    }
    // A feature loctel does not have yet is an error with its column, never a
    // silent mismatch.
    match validate("{ status = error } | select(span.foo)") {
        QueryState::Invalid { column, message } => {
            assert_eq!(column, 22);
            assert_eq!(message, "select is not supported yet");
        }
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[test]
fn service_filter_escapes() {
    assert_eq!(
        service_filter("web-bff"),
        r#"{ resource.service.name = "web-bff" }"#
    );
    assert_eq!(
        service_filter(r#"say "hi""#),
        r#"{ resource.service.name = "say \"hi\"" }"#
    );
    assert_eq!(
        service_filter(r"a\b"),
        r#"{ resource.service.name = "a\\b" }"#
    );
    // A backslash before a quote: both are escaped, the backslash first.
    assert_eq!(
        service_filter(r#"a\"b"#),
        r#"{ resource.service.name = "a\\\"b" }"#
    );
    assert_eq!(service_filter(""), r#"{ resource.service.name = "" }"#);
}

#[test]
fn service_filter_is_a_query_that_means_that_service() {
    let f = fixture();
    let search = |query: &str| -> Vec<TraceId> {
        let QueryState::Valid(query) = validate(query) else {
            panic!("not a valid query: {query}");
        };
        f.store
            .search_traces(
                &TraceFilter {
                    query: Some(query),
                    ..TraceFilter::default()
                },
                &PageRequest {
                    limit: 100,
                    cursor: None,
                },
            )
            .items
            .into_iter()
            .map(|summary| summary.trace_id)
            .collect()
    };

    // Every service of the mesh takes part in both checkouts.
    for service in SERVICES {
        let found = search(&service_filter(service));
        assert_eq!(found.len(), 2, "{service}");
    }
    // A service that is not there matches nothing.
    assert!(search(&service_filter("billing")).is_empty());

    // Names that need escaping still parse, and still mean exactly that name.
    for odd in [
        r#"say "hi""#,
        r"a\b",
        r#"a\"b"#,
        "two\nlines",
        "tab\tand {braces}",
        "日本語",
    ] {
        let query = service_filter(odd);
        assert!(
            matches!(validate(&query), QueryState::Valid(_)),
            "{query} should be valid"
        );
        assert!(search(&query).is_empty(), "{odd:?} is not a service here");
    }
}
