//! The waterfall and the inspector's details (M3 plan, Task 3), built from the
//! fixed fake mesh: `CheckoutFailure` (15 spans, the root and the card-gateway
//! calls erroring, an exception on `POST /charges`), `LateOutOfOrder` (the
//! same tree without the second gateway call, sent leaves first and the root
//! last) and `HugeTrace` (100,000 spans, for the budget).
//!
//! Span IDs are random, so a test names a span by the number it has in the
//! span table of the M1 plan (Task 8) and finds it by its start offset, which
//! is unique in the table.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use loctel_ingest::spans_from_request;
use loctel_meshgen::checkout::{EXCEPTION_MESSAGE, EXCEPTION_STACKTRACE, EXCEPTION_TYPE};
use loctel_meshgen::{Generated, Scenario, generate};
use loctel_model::{
    AttrValue, ExceptionInfo, Resource, Span, SpanEvent, SpanId, SpanKind, StatusCode, TraceId,
};
use loctel_store::{ManualClock, Store, StoreConfig, TraceSnapshot};
use loctel_ui::model::colors::ServiceColors;
use loctel_ui::model::inspector::{SpanDetails, span_details};
use loctel_ui::model::waterfall::{
    MIN_WIDTH_FRAC, MISSING_PARENT, Waterfall, WaterfallRow, initial_selection,
};

/// 2027-01-15T08:00:00Z. The traces start here.
const BASE: u64 = 1_800_000_000_000_000_000;
const MS: u64 = 1_000_000;
const SEED: u64 = 7;

/// The start of each span of the checkout table, in milliseconds after the
/// trace's start. The number of the span is the position plus one. Every one
/// is different, so a start names a span.
const START_MS: [u64; 15] = [
    0, 4, 30, 34, 40, 62, 66, 70, 196, 200, 206, 236, 840, 1794, 1802,
];

/// What the tree of the failing checkout reads top to bottom: the number in
/// the table, the depth, the name and the service. The order is the M1 plan's
/// table walked depth first, children by start.
const TREE: [(usize, u16, &str, &str); 15] = [
    (1, 0, "POST /api/checkout", "web-bff"),
    (2, 1, "validate_token", "web-bff"),
    (3, 1, "POST", "web-bff"),
    (4, 2, "POST /orders", "orders-api"),
    (5, 3, "SELECT carts", "orders-api"),
    (6, 3, "GET", "orders-api"),
    (7, 4, "GET /stock/{sku}", "inventory-api"),
    (8, 5, "MGET", "inventory-api"),
    (9, 3, "POST", "orders-api"),
    (10, 4, "POST /charges", "payments-api"),
    (11, 5, "SELECT payment_methods", "payments-api"),
    (12, 5, "POST", "payments-api"),
    (13, 5, "POST", "payments-api"),
    (14, 3, "order.failed publish", "orders-api"),
    (15, 4, "order.failed process", "notifications-worker"),
];

/// The spans of the erroring ones among them.
const ERRORING: [usize; 7] = [1, 3, 4, 9, 10, 12, 13];

// --- fixtures ----------------------------------------------------------------

fn spans_of(generated: &Generated, batches: std::ops::Range<usize>) -> Vec<Span> {
    let mut spans = Vec::new();
    for batch in &generated.batches[batches] {
        if let Some(request) = &batch.traces {
            let converted = spans_from_request(request.clone());
            assert_eq!(converted.rejected, 0, "generated spans are valid");
            spans.extend(converted.items);
        }
    }
    spans
}

/// The trace as the store hands it out once it holds `spans`: the real
/// snapshot, not one built by hand.
fn snapshot_of(spans: Vec<Span>) -> TraceSnapshot {
    let trace = spans.first().expect("some spans").trace_id;
    let clock = Arc::new(ManualClock::new(BASE));
    let store = Store::new(StoreConfig::default(), clock.clone());
    store.ingest_spans(spans);
    clock.advance(Duration::from_secs(60));
    store.trace(trace).expect("the trace is held")
}

fn failing_spans() -> Vec<Span> {
    let generated = generate(&Scenario::CheckoutFailure, SEED, BASE);
    spans_of(&generated, 0..generated.batches.len())
}

fn failing() -> TraceSnapshot {
    snapshot_of(failing_spans())
}

fn succeeding() -> TraceSnapshot {
    let generated = generate(&Scenario::CheckoutSuccess, SEED, BASE);
    snapshot_of(spans_of(&generated, 0..generated.batches.len()))
}

/// The id of the span with number `n` in the checkout table.
fn id_of(snapshot: &TraceSnapshot, n: usize) -> SpanId {
    let start = BASE + START_MS[n - 1] * MS;
    snapshot
        .spans
        .iter()
        .find(|span| span.start_unix_nano == start)
        .unwrap_or_else(|| panic!("span {n} is in the trace"))
        .span_id
}

/// The number in the checkout table of the span `id`.
fn number_of(snapshot: &TraceSnapshot, id: SpanId) -> usize {
    let span = snapshot
        .spans
        .iter()
        .find(|span| span.span_id == id)
        .expect("the span is in the trace");
    let offset = (span.start_unix_nano - BASE) / MS;
    START_MS
        .iter()
        .position(|&ms| ms == offset)
        .expect("a table span")
        + 1
}

/// The numbers of the rows shown, in order.
fn numbers(waterfall: &Waterfall, snapshot: &TraceSnapshot) -> Vec<usize> {
    waterfall
        .rows()
        .iter()
        .map(|row| number_of(snapshot, row.span_id))
        .collect()
}

fn row_for<'a>(waterfall: &'a Waterfall, snapshot: &TraceSnapshot, n: usize) -> &'a WaterfallRow {
    let id = id_of(snapshot, n);
    waterfall
        .rows()
        .iter()
        .find(|row| row.span_id == id)
        .unwrap_or_else(|| panic!("span {n} has a row"))
}

fn waterfall_of(snapshot: &TraceSnapshot) -> Waterfall {
    Waterfall::new(snapshot, &mut ServiceColors::new())
}

/// A span written out by hand, for the shapes the mesh does not make.
/// `id` and `parent` are small numbers; times are in milliseconds after
/// `BASE`.
fn hand_span(id: u64, parent: Option<u64>, name: &str, start_ms: u64, end_ms: u64) -> Span {
    Span {
        trace_id: TraceId([9; 16]),
        span_id: SpanId(id.to_be_bytes()),
        parent_span_id: parent.map(|p| SpanId(p.to_be_bytes())),
        name: name.to_owned(),
        kind: SpanKind::Internal,
        start_unix_nano: BASE + start_ms * MS,
        end_unix_nano: BASE + end_ms * MS,
        status: StatusCode::Unset,
        status_message: String::new(),
        attributes: Vec::new(),
        events: Vec::new(),
        resource: Arc::new(Resource {
            attributes: vec![("service.name".to_owned(), AttrValue::Str("svc".to_owned()))],
        }),
        scope_name: String::new(),
    }
}

fn hand_id(id: u64) -> SpanId {
    SpanId(id.to_be_bytes())
}

// --- the tree ----------------------------------------------------------------

#[test]
fn rows_in_tree_order_with_depths() {
    let snapshot = failing();
    let waterfall = waterfall_of(&snapshot);
    let rows = waterfall.rows();
    assert_eq!(rows.len(), 15);
    for (row, (n, depth, name, service)) in rows.iter().zip(TREE) {
        assert_eq!(number_of(&snapshot, row.span_id), n, "{name}");
        assert_eq!((row.depth, row.name.as_str()), (depth, name), "span {n}");
        assert_eq!(row.service, service, "span {n}");
    }
}

#[test]
fn rows_say_who_has_children_and_who_errors() {
    let snapshot = failing();
    let waterfall = waterfall_of(&snapshot);
    let parents: HashSet<usize> = [1, 3, 4, 6, 7, 9, 10, 14].into();
    for n in 1..=15 {
        let row = row_for(&waterfall, &snapshot, n);
        assert_eq!(
            row.has_children,
            parents.contains(&n),
            "has_children of {n}"
        );
        assert!(!row.collapsed, "nothing is collapsed at first");
        assert_eq!(row.is_error, ERRORING.contains(&n), "is_error of {n}");
    }
}

#[test]
fn durations_read_as_the_format_says() {
    let snapshot = failing();
    let waterfall = waterfall_of(&snapshot);
    assert_eq!(row_for(&waterfall, &snapshot, 1).duration, "1.84 s");
    assert_eq!(row_for(&waterfall, &snapshot, 10).duration, "1.59 s");
    assert_eq!(row_for(&waterfall, &snapshot, 2).duration, "18 ms");
    assert_eq!(row_for(&waterfall, &snapshot, 12).duration, "600 ms");
    assert_eq!(row_for(&waterfall, &snapshot, 8).duration, "4 ms");
}

#[test]
fn peer_notes_name_the_other_side_of_a_call() {
    let snapshot = failing();
    let waterfall = waterfall_of(&snapshot);
    let notes: Vec<(usize, Option<&str>)> = (1..=15)
        .map(|n| (n, row_for(&waterfall, &snapshot, n).peer_note.as_deref()))
        .collect();
    assert_eq!(
        notes,
        vec![
            (1, None),
            (2, None),
            (3, Some("orders-api")),
            (4, None),
            (5, Some("postgresql")),
            (6, Some("inventory-api")),
            (7, None),
            (8, Some("redis")),
            (9, Some("payments-api")),
            (10, None),
            (11, Some("postgresql")),
            (12, Some("card-gateway")),
            (13, Some("card-gateway")),
            (14, Some("rabbitmq")),
            (15, Some("rabbitmq")),
        ]
    );
}

#[test]
fn services_get_the_first_seen_colours() {
    let snapshot = failing();
    let mut colors = ServiceColors::new();
    let waterfall = Waterfall::new(&snapshot, &mut colors);
    let colour = |n| row_for(&waterfall, &snapshot, n).color;
    assert_eq!(
        [1, 4, 7, 10, 15].map(colour),
        [0, 1, 2, 3, 4],
        "web-bff, orders-api, inventory-api, payments-api, notifications-worker"
    );
    // The same numbers the colours table knows, so a service keeps its colour
    // from the trace table to the waterfall.
    assert_eq!(colors.peek("payments-api"), Some(3));

    // A service the table met first keeps the number it has.
    let mut met_first = ServiceColors::new();
    assert_eq!(met_first.index("payments-api"), 0);
    let waterfall = Waterfall::new(&snapshot, &mut met_first);
    assert_eq!(row_for(&waterfall, &snapshot, 10).color, 0);
    assert_eq!(
        row_for(&waterfall, &snapshot, 1).color,
        1,
        "web-bff is next"
    );
}

// --- the bars ----------------------------------------------------------------

#[test]
fn fractions_match_offsets() {
    let snapshot = failing();
    let waterfall = waterfall_of(&snapshot);
    let near = |a: f32, b: f64| (f64::from(a) - b).abs() <= 0.001;

    let charges = row_for(&waterfall, &snapshot, 10);
    assert!(near(charges.start_frac, 200.0 / 1840.0), "{charges:?}");
    assert!(near(charges.width_frac, 1586.0 / 1840.0), "{charges:?}");

    let root = row_for(&waterfall, &snapshot, 1);
    assert!(near(root.start_frac, 0.0) && near(root.width_frac, 1.0));

    let last = row_for(&waterfall, &snapshot, 15);
    assert!(near(last.start_frac, 1802.0 / 1840.0), "{last:?}");
    assert!(near(last.width_frac, 14.0 / 1840.0), "{last:?}");
}

#[test]
fn a_bar_is_never_thinner_than_the_minimum_and_stays_inside() {
    assert_eq!(MIN_WIDTH_FRAC, 0.003);
    // A trace of 1 s, with a span that has no length in the middle and one at
    // the very end.
    let snapshot = snapshot_of(vec![
        hand_span(1, None, "root", 0, 1_000),
        hand_span(2, Some(1), "instant", 500, 500),
        hand_span(3, Some(1), "at the end", 1_000, 1_000),
        hand_span(4, Some(1), "thin", 100, 101),
    ]);
    let waterfall = waterfall_of(&snapshot);
    let by_name = |name: &str| {
        waterfall
            .rows()
            .iter()
            .find(|row| row.name == name)
            .expect("a row")
    };
    for name in ["instant", "at the end", "thin"] {
        assert!(by_name(name).width_frac >= 0.003, "{name} is visible");
    }
    assert!((by_name("instant").start_frac - 0.5).abs() < 1e-6);
    let end = by_name("at the end");
    assert!(
        end.start_frac + end.width_frac <= 1.0 + 1e-6,
        "the bar stays inside the track: {end:?}"
    );
    // 1 ms of 1 s is 0.001, which is raised to the minimum; a bar that is
    // long enough is not touched.
    assert_eq!(by_name("thin").width_frac, MIN_WIDTH_FRAC);
    assert!((by_name("root").width_frac - 1.0).abs() < 1e-6);
}

#[test]
fn a_trace_with_no_length_gets_slivers_at_the_start() {
    let snapshot = snapshot_of(vec![
        hand_span(1, None, "root", 0, 0),
        hand_span(2, Some(1), "child", 0, 0),
    ]);
    let waterfall = waterfall_of(&snapshot);
    for row in waterfall.rows() {
        assert_eq!(row.start_frac, 0.0, "{row:?}");
        assert_eq!(row.width_frac, 0.003_f32, "{row:?}");
    }
    assert_eq!(
        waterfall.ruler(),
        ["0", "0", "0", "0", "0"].map(String::from)
    );
}

#[test]
fn exception_marker_on_charges() {
    let snapshot = failing();
    let waterfall = waterfall_of(&snapshot);
    let charges = row_for(&waterfall, &snapshot, 10);
    assert_eq!(charges.exception_fracs.len(), 1);
    assert!(
        (f64::from(charges.exception_fracs[0]) - 1782.0 / 1840.0).abs() <= 0.001,
        "{charges:?}"
    );
    // No other span has an exception event.
    for n in (1..=15).filter(|&n| n != 10) {
        assert!(
            row_for(&waterfall, &snapshot, n).exception_fracs.is_empty(),
            "span {n}"
        );
    }
}

#[test]
fn an_exception_without_error_status_still_has_its_marker() {
    // A handled exception (§9.4): the event is there, the status is not Error.
    let mut spans = vec![
        hand_span(1, None, "root", 0, 100),
        hand_span(2, Some(1), "handled", 10, 60),
    ];
    spans[1].events = vec![
        SpanEvent {
            time_unix_nano: BASE + 30 * MS,
            name: "exception".to_owned(),
            attributes: vec![],
        },
        SpanEvent {
            time_unix_nano: BASE + 40 * MS,
            name: "retry".to_owned(),
            attributes: vec![],
        },
        SpanEvent {
            // Before the trace started (clock skew): clamped, not negative.
            time_unix_nano: BASE - 5 * MS,
            name: "exception".to_owned(),
            attributes: vec![],
        },
    ];
    let waterfall = waterfall_of(&snapshot_of(spans));
    let handled = &waterfall.rows()[1];
    assert!(!handled.is_error);
    assert_eq!(handled.exception_fracs.len(), 2, "only `exception` events");
    assert!((handled.exception_fracs[0] - 0.3).abs() < 1e-6);
    assert_eq!(handled.exception_fracs[1], 0.0);
}

#[test]
fn ruler_labels() {
    let waterfall = waterfall_of(&failing());
    assert_eq!(
        waterfall.ruler(),
        ["0", "460 ms", "920 ms", "1.38 s", "1.84 s"].map(String::from)
    );
}

// --- selection ---------------------------------------------------------------

#[test]
fn initial_selection_is_failed_in() {
    let snapshot = failing();
    // The failed-in span is the first gateway call, number 12.
    assert_eq!(initial_selection(&snapshot), Some(id_of(&snapshot, 12)));
    assert_eq!(
        initial_selection(&snapshot),
        snapshot.failed_in.as_ref().map(|failed| failed.span_id)
    );
}

#[test]
fn initial_selection_is_the_root_without_a_failure() {
    let snapshot = succeeding();
    assert!(snapshot.failed_in.is_none());
    assert_eq!(initial_selection(&snapshot), Some(id_of(&snapshot, 1)));
}

#[test]
fn initial_selection_with_no_root_and_no_failure_is_none() {
    let generated = generate(&Scenario::LateOutOfOrder, SEED, BASE);
    let leaves = snapshot_of(spans_of(&generated, 0..1));
    assert!(leaves.root.is_none() && leaves.failed_in.is_none());
    assert_eq!(initial_selection(&leaves), None);
}

#[test]
fn a_new_waterfall_selects_nothing() {
    let waterfall = waterfall_of(&failing());
    assert_eq!(waterfall.selected(), None);
}

#[test]
fn select_expands_ancestors() {
    let snapshot = failing();
    let mut waterfall = waterfall_of(&snapshot);
    waterfall.collapse_all();
    assert_eq!(
        numbers(&waterfall, &snapshot),
        vec![1],
        "only the root shows"
    );

    let card = id_of(&snapshot, 11);
    assert_eq!(waterfall.row_of(card), None, "hidden under 10");
    waterfall.select(card);
    assert_eq!(waterfall.selected(), Some(card));

    // 1, 3, 4, 9 and 10 are open, and so every row of theirs shows; 6 and 14
    // are still collapsed, so their children do not.
    assert_eq!(
        numbers(&waterfall, &snapshot),
        vec![1, 2, 3, 4, 5, 6, 9, 10, 11, 12, 13, 14]
    );
    let row = waterfall.row_of(card).expect("now visible");
    assert_eq!(waterfall.rows()[row].span_id, card);
    assert!(waterfall.rows()[waterfall.row_of(id_of(&snapshot, 6)).unwrap()].collapsed);
}

#[test]
fn selecting_a_span_that_is_not_in_the_trace_changes_nothing() {
    let snapshot = failing();
    let mut waterfall = waterfall_of(&snapshot);
    let charges = id_of(&snapshot, 10);
    waterfall.select(charges);
    waterfall.select(hand_id(0xdead));
    assert_eq!(waterfall.selected(), Some(charges));
    assert_eq!(waterfall.row_of(hand_id(0xdead)), None);
}

#[test]
fn row_of_is_the_position_among_the_rows_shown() {
    let snapshot = failing();
    let mut waterfall = waterfall_of(&snapshot);
    for (position, (n, ..)) in TREE.iter().enumerate() {
        assert_eq!(waterfall.row_of(id_of(&snapshot, *n)), Some(position));
    }
    waterfall.toggle(id_of(&snapshot, 4));
    assert_eq!(waterfall.row_of(id_of(&snapshot, 14)), None, "hidden");
    assert_eq!(waterfall.row_of(id_of(&snapshot, 3)), Some(2));
    assert_eq!(waterfall.row_of(id_of(&snapshot, 4)), Some(3));
}

#[test]
fn a_collapsed_ancestor_hides_the_selection_but_keeps_it() {
    let snapshot = failing();
    let mut waterfall = waterfall_of(&snapshot);
    let card = id_of(&snapshot, 11);
    waterfall.select(card);
    waterfall.toggle(id_of(&snapshot, 10));
    assert_eq!(waterfall.selected(), Some(card), "selection follows the id");
    assert_eq!(waterfall.row_of(card), None);
    waterfall.toggle(id_of(&snapshot, 10));
    assert!(waterfall.row_of(card).is_some());
}

// --- collapse ----------------------------------------------------------------

#[test]
fn collapse_and_expand() {
    let snapshot = failing();
    let mut waterfall = waterfall_of(&snapshot);
    let orders = id_of(&snapshot, 4);

    waterfall.toggle(orders);
    assert_eq!(numbers(&waterfall, &snapshot), vec![1, 2, 3, 4]);
    let row = &waterfall.rows()[3];
    assert!(row.collapsed && row.has_children, "{row:?}");

    waterfall.toggle(orders);
    assert_eq!(waterfall.rows().len(), 15);
    assert!(!waterfall.rows()[3].collapsed);

    // A branch inside a branch: collapsing the outer one and opening it again
    // gives the inner one back as it was left.
    let inventory = id_of(&snapshot, 6);
    waterfall.toggle(inventory);
    waterfall.toggle(orders);
    waterfall.toggle(orders);
    assert_eq!(
        numbers(&waterfall, &snapshot),
        vec![1, 2, 3, 4, 5, 6, 9, 10, 11, 12, 13, 14, 15]
    );

    waterfall.collapse_all();
    assert_eq!(numbers(&waterfall, &snapshot), vec![1]);
    assert!(waterfall.rows()[0].collapsed);
    waterfall.expand_all();
    assert_eq!(waterfall.rows().len(), 15);
    assert!(waterfall.rows().iter().all(|row| !row.collapsed));
}

#[test]
fn toggling_a_leaf_or_a_stranger_does_nothing() {
    let snapshot = failing();
    let mut waterfall = waterfall_of(&snapshot);
    waterfall.toggle(id_of(&snapshot, 5));
    waterfall.toggle(hand_id(0xdead));
    assert_eq!(waterfall.rows().len(), 15);
    assert!(waterfall.rows().iter().all(|row| !row.collapsed));
}

// --- errors path only --------------------------------------------------------

#[test]
fn errors_path_only_shows_errors_and_ancestors() {
    let snapshot = failing();
    let mut waterfall = waterfall_of(&snapshot);
    waterfall.set_errors_path_only(true);
    assert_eq!(numbers(&waterfall, &snapshot), ERRORING.to_vec());
    let depths: Vec<u16> = waterfall.rows().iter().map(|row| row.depth).collect();
    assert_eq!(depths, vec![0, 1, 2, 3, 4, 5, 5], "depths are the tree's");
    assert!(waterfall.rows().iter().all(|row| row.is_error));

    // Switching it off gives the whole tree back.
    waterfall.set_errors_path_only(false);
    assert_eq!(waterfall.rows().len(), 15);
}

#[test]
fn an_ancestor_that_is_not_erroring_is_on_the_path() {
    // Only the gateway calls (12 and 13) error here: the spans above them
    // are on the path to them, and no other branch is.
    let mut spans = failing_spans();
    for span in &mut spans {
        let offset = (span.start_unix_nano - BASE) / MS;
        if ![236, 840].contains(&offset) {
            span.status = StatusCode::Unset;
        }
    }
    let snapshot = snapshot_of(spans);
    let mut waterfall = waterfall_of(&snapshot);
    waterfall.set_errors_path_only(true);
    assert_eq!(numbers(&waterfall, &snapshot), vec![1, 3, 4, 9, 10, 12, 13]);
    let errors: Vec<bool> = waterfall.rows().iter().map(|row| row.is_error).collect();
    assert_eq!(errors, vec![false, false, false, false, false, true, true]);
}

#[test]
fn errors_path_only_with_no_error_shows_nothing() {
    let snapshot = succeeding();
    let mut waterfall = waterfall_of(&snapshot);
    waterfall.set_errors_path_only(true);
    assert!(waterfall.rows().is_empty());
    waterfall.set_errors_path_only(false);
    assert_eq!(waterfall.rows().len(), 14);
}

#[test]
fn errors_path_only_composes_with_collapse() {
    let snapshot = failing();
    let mut waterfall = waterfall_of(&snapshot);
    waterfall.set_errors_path_only(true);
    waterfall.toggle(id_of(&snapshot, 9));
    assert_eq!(numbers(&waterfall, &snapshot), vec![1, 3, 4, 9]);
    waterfall.toggle(id_of(&snapshot, 9));
    assert_eq!(numbers(&waterfall, &snapshot), ERRORING.to_vec());
}

#[test]
fn the_selection_survives_the_errors_filter() {
    let snapshot = failing();
    let mut waterfall = waterfall_of(&snapshot);
    let carts = id_of(&snapshot, 5);
    waterfall.select(carts);
    waterfall.set_errors_path_only(true);
    assert_eq!(waterfall.selected(), Some(carts));
    assert_eq!(waterfall.row_of(carts), None, "not on the path");
    waterfall.set_errors_path_only(false);
    assert_eq!(waterfall.row_of(carts), Some(4));
}

// --- find --------------------------------------------------------------------

#[test]
fn find_matches_name_service_and_attribute_values() {
    let snapshot = failing();
    let waterfall = waterfall_of(&snapshot);
    // A row index is the position among the rows shown; the table is walked
    // top to bottom, so span n is at n - 1.
    let at = |found: Vec<usize>| -> Vec<usize> { found.iter().map(|i| i + 1).collect() };

    // The name.
    assert_eq!(at(waterfall.find("validate")), vec![2]);
    assert_eq!(at(waterfall.find("/charges")), vec![10]);
    // Ignoring case, in the text and in the pattern.
    assert_eq!(at(waterfall.find("VALIDATE_token")), vec![2]);
    assert_eq!(at(waterfall.find("Select")), vec![5, 11]);
    // The service: its four spans, and the call that has it as a
    // `server.address` value.
    assert_eq!(at(waterfall.find("payments-api")), vec![9, 10, 11, 12, 13]);
    // An attribute value, written as the inspector writes it: a string...
    assert_eq!(at(waterfall.find("card-gateway")), vec![12, 13]);
    assert_eq!(at(waterfall.find("postgres")), vec![5, 11]);
    // ... and a number.
    assert_eq!(at(waterfall.find("8812")), vec![10]);
    assert_eq!(
        at(waterfall.find("/api/checkout")),
        vec![1],
        "route and name"
    );
}

#[test]
fn find_looks_at_values_not_keys_or_messages() {
    let snapshot = failing();
    let waterfall = waterfall_of(&snapshot);
    // `error.type` is on span 12 as a key; its value is `timeout`.
    assert!(
        waterfall.find("error.type").is_empty(),
        "keys are not searched"
    );
    assert_eq!(waterfall.find("timeout"), vec![11], "the value is");
    // The status message and the exception are the inspector's, not the
    // find's.
    assert!(waterfall.find("did not respond").is_empty());
    assert!(waterfall.find("HTTP 500").is_empty());
}

#[test]
fn find_with_no_text_finds_nothing() {
    let waterfall = waterfall_of(&failing());
    assert!(waterfall.find("").is_empty());
    assert!(waterfall.find("no such text anywhere").is_empty());
}

#[test]
fn find_only_sees_the_rows_shown() {
    let snapshot = failing();
    let mut waterfall = waterfall_of(&snapshot);
    waterfall.toggle(id_of(&snapshot, 4));
    // Rows now: 1, 2, 3, 4. `orders-api` is the service of 4 and the value
    // of `server.address` on 3; the many spans of orders-api below 4 are
    // hidden, so their rows are not offered.
    assert_eq!(waterfall.find("orders-api"), vec![2, 3]);
    assert!(waterfall.find("payments-api").is_empty(), "all hidden");
    waterfall.toggle(id_of(&snapshot, 4));
    assert_eq!(waterfall.find("payments-api").len(), 5);
}

#[test]
fn find_walks_a_long_value_whole() {
    let mut spans = vec![
        hand_span(1, None, "root", 0, 100),
        hand_span(2, Some(1), "query", 10, 20),
    ];
    let mut statement = "x".repeat(20_000);
    statement.push_str(" the end of it");
    spans[1].attributes = vec![("db.statement".to_owned(), AttrValue::Str(statement))];
    let waterfall = waterfall_of(&snapshot_of(spans));
    assert_eq!(waterfall.find("THE END"), vec![1]);
}

// --- the missing parent placeholder ------------------------------------------

#[test]
fn orphans_under_missing_parent_row() {
    // The first batch of LateOutOfOrder: ten spans, none of them the root nor
    // the middle of the tree. 5, 6, 9 and 14 hang on 4, which has not come.
    let generated = generate(&Scenario::LateOutOfOrder, SEED, BASE);
    let snapshot = snapshot_of(spans_of(&generated, 0..1));
    assert_eq!(snapshot.spans.len(), 10);
    assert_eq!(snapshot.orphans.len(), 4);
    let waterfall = waterfall_of(&snapshot);
    let rows = waterfall.rows();
    assert_eq!(rows.len(), 11, "the placeholder and the ten spans");

    let placeholder = &rows[0];
    assert!(placeholder.is_placeholder());
    assert_eq!(placeholder.name, MISSING_PARENT);
    assert_eq!(placeholder.peer_note.as_deref(), Some("missing parent"));
    assert_eq!((placeholder.depth, placeholder.has_children), (0, true));
    assert!(!placeholder.is_error);
    assert!(rows[1..].iter().all(|row| !row.is_placeholder()));

    // The orphans are one level down, in start order, each with its subtree.
    let shown: Vec<(usize, u16)> = rows[1..]
        .iter()
        .map(|row| (number_of(&snapshot, row.span_id), row.depth))
        .collect();
    assert_eq!(
        shown,
        vec![
            (5, 1),
            (6, 1),
            (7, 2),
            (8, 3),
            (9, 1),
            (10, 2),
            (11, 3),
            (12, 3),
            (14, 1),
            (15, 2)
        ]
    );
}

#[test]
fn the_placeholder_comes_after_the_roots_subtree() {
    // The failing checkout without `POST /orders` (4): the root has 2 and 3,
    // and everything that hung on 4 is an orphan.
    let spans: Vec<Span> = failing_spans()
        .into_iter()
        .filter(|span| span.start_unix_nano != BASE + 34 * MS)
        .collect();
    let snapshot = snapshot_of(spans);
    assert!(snapshot.root.is_some());
    let waterfall = waterfall_of(&snapshot);
    let rows = waterfall.rows();
    let shape: Vec<(Option<usize>, u16)> = rows
        .iter()
        .map(|row| {
            let n = (!row.is_placeholder()).then(|| number_of(&snapshot, row.span_id));
            (n, row.depth)
        })
        .collect();
    assert_eq!(
        shape,
        vec![
            (Some(1), 0),
            (Some(2), 1),
            (Some(3), 1),
            (None, 0),
            (Some(5), 1),
            (Some(6), 1),
            (Some(7), 2),
            (Some(8), 3),
            (Some(9), 1),
            (Some(10), 2),
            (Some(11), 3),
            (Some(12), 3),
            (Some(13), 3),
            (Some(14), 1),
            (Some(15), 2),
        ]
    );
}

#[test]
fn no_placeholder_when_nothing_is_missing() {
    let waterfall = waterfall_of(&failing());
    assert!(waterfall.rows().iter().all(|row| !row.is_placeholder()));
}

#[test]
fn the_placeholder_collapses_like_any_row() {
    let generated = generate(&Scenario::LateOutOfOrder, SEED, BASE);
    let snapshot = snapshot_of(spans_of(&generated, 0..1));
    let mut waterfall = waterfall_of(&snapshot);
    let placeholder = waterfall.rows()[0].span_id;
    waterfall.toggle(placeholder);
    assert_eq!(waterfall.rows().len(), 1);
    assert!(waterfall.rows()[0].collapsed);
    waterfall.toggle(placeholder);
    assert_eq!(waterfall.rows().len(), 11);
}

#[test]
fn the_placeholder_is_on_the_error_path_only_if_an_orphan_errors() {
    // Without 4, the orphans include the erroring 9 (and 10, 12, 13 below it).
    let spans: Vec<Span> = failing_spans()
        .into_iter()
        .filter(|span| span.start_unix_nano != BASE + 34 * MS)
        .collect();
    let snapshot = snapshot_of(spans);
    let mut waterfall = waterfall_of(&snapshot);
    waterfall.set_errors_path_only(true);
    let shape: Vec<Option<usize>> = waterfall
        .rows()
        .iter()
        .map(|row| (!row.is_placeholder()).then(|| number_of(&snapshot, row.span_id)))
        .collect();
    assert_eq!(
        shape,
        vec![
            Some(1),
            Some(3),
            None,
            Some(9),
            Some(10),
            Some(12),
            Some(13)
        ]
    );

    // A leaf-only first batch has no error: the placeholder goes too.
    let generated = generate(&Scenario::LateOutOfOrder, SEED, BASE);
    let leaves = snapshot_of(spans_of(&generated, 0..1));
    let mut waterfall = waterfall_of(&leaves);
    waterfall.set_errors_path_only(true);
    assert!(waterfall.rows().is_empty());
}

// --- trees that are not trees ------------------------------------------------

#[test]
fn two_roots_are_both_at_the_top_in_start_order() {
    let snapshot = snapshot_of(vec![
        hand_span(2, None, "second root", 5, 50),
        hand_span(1, None, "first root", 0, 100),
        hand_span(3, Some(2), "under the second", 6, 7),
    ]);
    let waterfall = waterfall_of(&snapshot);
    let shape: Vec<(&str, u16)> = waterfall
        .rows()
        .iter()
        .map(|row| (row.name.as_str(), row.depth))
        .collect();
    assert_eq!(
        shape,
        vec![
            ("first root", 0),
            ("second root", 0),
            ("under the second", 1)
        ]
    );
}

#[test]
fn siblings_that_start_together_go_by_span_id() {
    let snapshot = snapshot_of(vec![
        hand_span(1, None, "root", 0, 100),
        hand_span(9, Some(1), "nine", 10, 20),
        hand_span(3, Some(1), "three", 10, 20),
        hand_span(5, Some(1), "five", 10, 20),
        hand_span(4, Some(1), "early", 2, 3),
    ]);
    let waterfall = waterfall_of(&snapshot);
    let names: Vec<&str> = waterfall
        .rows()
        .iter()
        .map(|row| row.name.as_str())
        .collect();
    assert_eq!(names, vec!["root", "early", "three", "five", "nine"]);
}

#[test]
fn a_parent_loop_terminates_and_every_span_shows_once() {
    // 2 -> 3 -> 4 -> 2 is a loop with nothing above it (malformed), 5 hangs
    // on 3, 6 is its own parent, and 1 is a proper root.
    let snapshot = snapshot_of(vec![
        hand_span(1, None, "root", 0, 100),
        hand_span(2, Some(4), "loop a", 1, 90),
        hand_span(3, Some(2), "loop b", 2, 80),
        hand_span(4, Some(3), "loop c", 3, 70),
        hand_span(5, Some(3), "hangs on the loop", 4, 5),
        hand_span(6, Some(6), "its own parent", 6, 7),
    ]);
    let waterfall = waterfall_of(&snapshot);
    let mut ids: Vec<SpanId> = waterfall
        .rows()
        .iter()
        .filter(|row| !row.is_placeholder())
        .map(|row| row.span_id)
        .collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 6, "each span once: {:?}", waterfall.rows());
    assert_eq!(waterfall.rows()[0].name, "root");
    // What cannot be reached from a root goes under the placeholder.
    assert!(waterfall.rows().iter().any(WaterfallRow::is_placeholder));
}

#[test]
fn a_very_deep_chain_does_not_overflow_the_stack() {
    // Deeper than a recursive walk could go on a test thread's stack, and
    // deeper than the 16 bits a depth has.
    const DEPTH: u64 = 70_000;
    let spans: Vec<Span> = (1..=DEPTH)
        .map(|n| hand_span(n, (n > 1).then(|| n - 1), "link", n, n + 1))
        .collect();
    let snapshot = snapshot_of(spans);
    let mut waterfall = waterfall_of(&snapshot);
    assert_eq!(waterfall.rows().len(), DEPTH as usize);
    assert_eq!(waterfall.rows()[1_000].depth, 1_000);
    assert_eq!(
        waterfall.rows().last().expect("a row").depth,
        u16::MAX,
        "a depth that does not fit stops at the largest"
    );
    waterfall.toggle(hand_id(2));
    assert_eq!(waterfall.rows().len(), 2);
}

#[test]
fn no_spans_no_rows() {
    let snapshot = TraceSnapshot {
        trace_id: TraceId([1; 16]),
        spans: Vec::new(),
        root: None,
        orphans: Vec::new(),
        complete: false,
        status: loctel_store::TraceStatus::Ok,
        failed_in: None,
    };
    let mut waterfall = waterfall_of(&snapshot);
    assert!(waterfall.rows().is_empty());
    assert_eq!(
        waterfall.ruler(),
        ["0", "0", "0", "0", "0"].map(String::from)
    );
    assert_eq!(initial_selection(&snapshot), None);
    waterfall.collapse_all();
    waterfall.expand_all();
    assert!(waterfall.find("x").is_empty());
}

// --- a trace that is still arriving ------------------------------------------

#[test]
fn rebuild_keeps_selection_and_collapsed_state() {
    // The three batches of LateOutOfOrder: the leaves, the middle, the root.
    let generated = generate(&Scenario::LateOutOfOrder, SEED, BASE);
    let mut colors = ServiceColors::new();
    let first = snapshot_of(spans_of(&generated, 0..1));
    let second = snapshot_of(spans_of(&generated, 0..2));
    let third = snapshot_of(spans_of(&generated, 0..3));
    assert_eq!(
        [first.spans.len(), second.spans.len(), third.spans.len()],
        [10, 13, 14]
    );

    let mut waterfall = Waterfall::new(&first, &mut colors);
    let charges = id_of(&first, 10);
    let inventory = id_of(&first, 6);
    waterfall.select(charges);
    waterfall.toggle(inventory);
    assert_eq!(waterfall.rows().len(), 9, "7 and 8 are folded away");
    let before = waterfall.row_of(charges).expect("shown");

    // The middle arrives: 2 and 3 are orphans now, and the old orphans have a
    // parent.
    waterfall.rebuild(&second, &mut colors);
    assert_eq!(waterfall.selected(), Some(charges));
    assert!(waterfall.rows()[waterfall.row_of(inventory).unwrap()].collapsed);
    let real = |w: &Waterfall| w.rows().iter().filter(|r| !r.is_placeholder()).count();
    assert_eq!(real(&waterfall), 11, "13 spans, 7 and 8 folded away");
    assert!(waterfall.rows()[0].is_placeholder(), "the root is missing");
    let during = waterfall.row_of(charges).expect("still shown");
    assert_ne!(during, before, "it moved down: rows came in above it");

    // The root arrives: no placeholder, rooted at the checkout.
    waterfall.rebuild(&third, &mut colors);
    assert_eq!(waterfall.selected(), Some(charges));
    assert!(waterfall.rows()[waterfall.row_of(inventory).unwrap()].collapsed);
    assert!(waterfall.rows().iter().all(|row| !row.is_placeholder()));
    assert_eq!(waterfall.rows()[0].name, "POST /api/checkout");
    assert_eq!(waterfall.rows()[0].depth, 0);
    assert_eq!(waterfall.rows().len(), 12, "14 spans, 7 and 8 folded away");
    assert_eq!(
        waterfall.row_of(charges).map(|i| waterfall.rows()[i].depth),
        Some(4)
    );

    // Opening it all shows every span, the selection still the same.
    waterfall.expand_all();
    assert_eq!(waterfall.rows().len(), 14);
    assert_eq!(waterfall.selected(), Some(charges));
}

#[test]
fn rebuild_keeps_the_filter_and_forgets_what_is_gone() {
    let snapshot = failing();
    let mut colors = ServiceColors::new();
    let mut waterfall = Waterfall::new(&snapshot, &mut colors);
    waterfall.set_errors_path_only(true);
    let gone = id_of(&snapshot, 12);
    waterfall.select(gone);
    waterfall.toggle(id_of(&snapshot, 10));

    // The same trace without span 12 (the first gateway call).
    let spans: Vec<Span> = failing_spans()
        .into_iter()
        .filter(|span| span.span_id != gone)
        .collect();
    let smaller = snapshot_of(spans);
    waterfall.rebuild(&smaller, &mut colors);

    // The filter is the user's setting and stays. The selected span is not
    // there any more, so nothing is selected rather than something that is
    // not shown.
    assert_eq!(numbers(&waterfall, &smaller), vec![1, 3, 4, 9, 10]);
    assert!(waterfall.rows()[4].collapsed, "10 is still folded");
    assert_eq!(waterfall.selected(), None);
    assert_eq!(waterfall.row_of(gone), None);
}

#[test]
fn rebuild_gives_the_same_rows_as_a_new_waterfall() {
    let snapshot = failing();
    let mut colors = ServiceColors::new();
    let fresh = Waterfall::new(&snapshot, &mut colors);
    let mut rebuilt = Waterfall::new(&succeeding(), &mut colors);
    rebuilt.rebuild(&snapshot, &mut colors);
    assert_eq!(rebuilt.rows(), fresh.rows());
    assert_eq!(rebuilt.ruler(), fresh.ruler());
}

// --- the details -------------------------------------------------------------

fn details(snapshot: &TraceSnapshot, n: usize) -> SpanDetails {
    span_details(snapshot, id_of(snapshot, n)).expect("the span is in the trace")
}

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|&(key, value)| (key.to_owned(), value.to_owned()))
        .collect()
}

#[test]
fn span_details_of_charges() {
    let snapshot = failing();
    let charges = details(&snapshot, 10);
    assert_eq!(charges.name, "POST /charges");
    assert_eq!(charges.service, "payments-api");
    assert_eq!(charges.kind, "Server");
    assert_eq!(charges.duration, "1.59 s");
    assert_eq!(charges.offset, "starts at +200 ms");
    assert_eq!(
        charges.status,
        format!("Error \u{b7} {EXCEPTION_MESSAGE}"),
        "the status and its message"
    );
    assert_eq!(
        charges.exception,
        Some(ExceptionInfo {
            type_name: EXCEPTION_TYPE.to_owned(),
            message: EXCEPTION_MESSAGE.to_owned(),
            stacktrace: Some(EXCEPTION_STACKTRACE.to_owned()),
        })
    );
    assert_eq!(charges.attributes, pairs(&[("order.id", "8812")]));
    assert_eq!(
        charges.resource,
        pairs(&[
            ("deployment.environment", "local"),
            ("service.name", "payments-api"),
            ("service.version", "1.4.0"),
        ])
    );
}

#[test]
fn attributes_are_sorted_by_key_and_values_are_whole() {
    let snapshot = failing();
    // Written `server.address` first, `http.request.resend_count` second.
    let retry = details(&snapshot, 13);
    assert_eq!(
        retry.attributes,
        pairs(&[
            ("http.request.resend_count", "1"),
            ("server.address", "card-gateway"),
        ])
    );
    assert_eq!(retry.kind, "Client");
    assert_eq!(retry.offset, "starts at +840 ms");
    assert_eq!(retry.exception, None);

    // A value is not cut: the view does that. 20,000 characters stay 20,000.
    let mut spans = vec![hand_span(1, None, "root", 0, 100)];
    spans[0].attributes = vec![
        ("zeta".to_owned(), AttrValue::Str("z".repeat(20_000))),
        ("alpha".to_owned(), AttrValue::Bool(true)),
        ("mid".to_owned(), AttrValue::Double(1.5)),
        (
            "list".to_owned(),
            AttrValue::Array(vec![AttrValue::Int(1), AttrValue::Str("two".to_owned())]),
        ),
    ];
    let snapshot = snapshot_of(spans);
    let root = span_details(&snapshot, hand_id(1)).expect("the root");
    let keys: Vec<&str> = root
        .attributes
        .iter()
        .map(|(key, _)| key.as_str())
        .collect();
    assert_eq!(keys, vec!["alpha", "list", "mid", "zeta"]);
    assert_eq!(root.attributes[0].1, "true");
    assert_eq!(root.attributes[1].1, r#"[1,"two"]"#);
    assert_eq!(root.attributes[2].1, "1.5");
    assert_eq!(root.attributes[3].1.len(), 20_000);
}

#[test]
fn status_reads_the_code_and_its_message() {
    let mut spans = vec![
        hand_span(1, None, "unset", 0, 100),
        hand_span(2, Some(1), "ok", 1, 2),
        hand_span(3, Some(1), "error without a word", 3, 4),
        hand_span(4, Some(1), "unset with a word", 5, 6),
    ];
    spans[1].status = StatusCode::Ok;
    spans[2].status = StatusCode::Error;
    spans[3].status_message = "odd".to_owned();
    let snapshot = snapshot_of(spans);
    let status = |id| span_details(&snapshot, hand_id(id)).expect("a span").status;
    assert_eq!(status(1), "Unset");
    assert_eq!(status(2), "OK");
    assert_eq!(status(3), "Error");
    assert_eq!(status(4), "Unset \u{b7} odd");
}

#[test]
fn kinds_read_as_their_names() {
    let kinds = [
        (SpanKind::Unspecified, "Unspecified"),
        (SpanKind::Internal, "Internal"),
        (SpanKind::Server, "Server"),
        (SpanKind::Client, "Client"),
        (SpanKind::Producer, "Producer"),
        (SpanKind::Consumer, "Consumer"),
    ];
    let mut spans = vec![hand_span(1, None, "root", 0, 100)];
    for (i, (kind, _)) in kinds.iter().enumerate() {
        let mut span = hand_span(i as u64 + 2, Some(1), "child", 1, 2);
        span.kind = *kind;
        spans.push(span);
    }
    let snapshot = snapshot_of(spans);
    for (i, (_, name)) in kinds.iter().enumerate() {
        let id = hand_id(i as u64 + 2);
        assert_eq!(span_details(&snapshot, id).expect("a span").kind, *name);
    }
}

#[test]
fn the_root_starts_at_nothing_and_a_late_span_is_not_before_it() {
    let snapshot = failing();
    assert_eq!(details(&snapshot, 1).offset, "starts at +0 ms");
    assert_eq!(details(&snapshot, 1).duration, "1.84 s");
    assert_eq!(details(&snapshot, 15).offset, "starts at +1.80 s");
    assert_eq!(details(&snapshot, 15).duration, "14 ms");
    assert_eq!(details(&snapshot, 8).duration, "4 ms");
    assert_eq!(details(&snapshot, 8).offset, "starts at +70 ms");
}

#[test]
fn details_of_a_span_that_is_not_there_are_none() {
    let snapshot = failing();
    assert_eq!(span_details(&snapshot, hand_id(0xdead)), None);
    // The placeholder is not a span.
    let placeholder =
        waterfall_of(&snapshot_of(vec![hand_span(2, Some(1), "orphan", 0, 1)])).rows()[0].span_id;
    assert_eq!(span_details(&snapshot, placeholder), None);
}

// --- the budget --------------------------------------------------------------

fn huge_snapshot(spans: usize) -> TraceSnapshot {
    let generated = generate(&Scenario::HugeTrace { spans }, SEED, BASE);
    snapshot_of(spans_of(&generated, 0..generated.batches.len()))
}

#[test]
fn waterfall_100k_builds_under_budget() {
    let snapshot = huge_snapshot(100_000);
    assert_eq!(snapshot.spans.len(), 100_000);
    let mut colors = ServiceColors::new();

    let started = Instant::now();
    let mut waterfall = Waterfall::new(&snapshot, &mut colors);
    let built = started.elapsed();
    assert_eq!(waterfall.rows().len(), 100_000);
    assert!(
        built < Duration::from_millis(1_500),
        "building 100,000 rows took {built:?}"
    );

    // A live trace is rebuilt twice a second: the same budget.
    let started = Instant::now();
    waterfall.rebuild(&snapshot, &mut colors);
    let rebuilt = started.elapsed();
    assert_eq!(waterfall.rows().len(), 100_000);
    assert!(
        rebuilt < Duration::from_millis(1_500),
        "rebuilding 100,000 rows took {rebuilt:?}"
    );

    // Every kind of walk works on all of it, and gives the right counts.
    let root = waterfall.rows()[0].span_id;
    assert_eq!(waterfall.rows()[0].depth, 0);
    assert!(waterfall.rows().iter().all(|row| row.depth <= 7));
    waterfall.toggle(root);
    assert_eq!(waterfall.rows().len(), 1);
    waterfall.expand_all();
    assert_eq!(waterfall.rows().len(), 100_000);
    waterfall.collapse_all();
    assert_eq!(waterfall.rows().len(), 1);
    let middle = snapshot.spans[50_000].span_id;
    waterfall.select(middle);
    let at = waterfall.row_of(middle).expect("shown after select");
    assert_eq!(waterfall.rows()[at].span_id, middle);
    waterfall.expand_all();
    let hits = waterfall.find("work item 99999");
    assert_eq!(hits.len(), 1);
    assert_eq!(waterfall.rows()[hits[0]].name, "work item 99999");
}
