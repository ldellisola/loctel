//! What the trace view says outside the waterfall (M3 plan, Task 8): the
//! header, the find box's count, the rows of a span's logs, the inspector's
//! cut values and what its buttons copy, and the query "traces through this
//! span name" writes. All plain strings over the fixed fake mesh
//! (`CheckoutFailure`: 15 spans, 5 services, 1.84 s, a failed trace;
//! `CheckoutSuccess`; `LateOutOfOrder`'s first batch, which has no root).

use std::sync::Arc;
use std::time::Duration;

use loctel_ingest::{logs_from_request, spans_from_request};
use loctel_meshgen::checkout::{EXCEPTION_MESSAGE, EXCEPTION_STACKTRACE, EXCEPTION_TYPE};
use loctel_meshgen::{Generated, Scenario, generate};
use loctel_model::{ExceptionInfo, LogRecord, Severity, Span, SpanId};
use loctel_store::{ManualClock, Store, StoreConfig, TraceSnapshot};
use loctel_ui::model::inspector::{
    VALUE_PREVIEW_CHARS, exception_text, span_details, span_json, value_preview,
};
use loctel_ui::model::query_bar::{self, QueryState, span_name_filter};
use loctel_ui::model::trace_view::{
    COMPLETE, Failure, RECEIVING, find_status, header, level_badge, log_lines, logs_count,
    logs_title,
};

/// 2027-01-15T08:00:00Z. The traces start here.
const BASE: u64 = 1_800_000_000_000_000_000;
const MS: u64 = 1_000_000;
const SEED: u64 = 7;

/// A store on its own manual clock with the first `batches` of `scenario`
/// ingested at `BASE`.
fn store_with(
    scenario: &Scenario,
    batches: std::ops::Range<usize>,
) -> (Store, Arc<ManualClock>, Generated) {
    let clock = Arc::new(ManualClock::new(BASE));
    let store = Store::new(StoreConfig::default(), clock.clone());
    let generated = generate(scenario, SEED, BASE);
    for batch in &generated.batches[batches] {
        if let Some(request) = &batch.traces {
            store.ingest_spans(spans_from_request(request.clone()).items);
        }
        if let Some(request) = &batch.logs {
            store.ingest_logs(logs_from_request(request.clone()).items);
        }
    }
    (store, clock, generated)
}

/// The failing checkout, quiet for a minute (so complete): the store's own
/// snapshot of it.
fn failing() -> TraceSnapshot {
    let (store, clock, generated) = store_with(&Scenario::CheckoutFailure, 0..1);
    clock.advance(Duration::from_secs(60));
    store
        .trace(generated.trace_ids[0])
        .expect("the trace is held")
}

fn span_named<'a>(snapshot: &'a TraceSnapshot, name: &str, start_ms: u64) -> &'a Span {
    snapshot
        .spans
        .iter()
        .find(|span| span.name == name && span.start_unix_nano == BASE + start_ms * MS)
        .unwrap_or_else(|| panic!("no span {name} at {start_ms} ms"))
}

// --- the header ----------------------------------------------------------------

#[test]
fn header_of_the_failed_checkout() {
    let snapshot = failing();
    let header = header(&snapshot, 0);

    assert_eq!(header.root, "POST /api/checkout");
    assert!(header.root_arrived);
    assert_eq!(
        header.failure,
        Failure::FailedIn("payments-api \u{b7} POST card-gateway +1".to_owned())
    );
    assert_eq!(
        header.failure.badge().as_deref(),
        Some("Failed in payments-api \u{b7} POST card-gateway +1")
    );
    assert_eq!(
        header.summary,
        "1.84 s \u{b7} 15 spans \u{b7} 5 services \u{b7} 08:00:00.000"
    );
    assert_eq!(header.trace_id.len(), 32);
    assert_eq!(header.trace_id, snapshot.trace_id.to_string());
    assert!(header.complete);
    assert_eq!(header.completeness(), COMPLETE);
    assert_eq!(COMPLETE, "Complete");
    assert_eq!(RECEIVING, "Receiving spans\u{2026}");
}

#[test]
fn header_start_time_is_in_the_given_zone() {
    let snapshot = failing();
    // Two hours east of UTC.
    let header = header(&snapshot, 2 * 3_600);
    assert!(
        header.summary.ends_with("10:00:00.000"),
        "{}",
        header.summary
    );
}

#[test]
fn header_of_a_successful_trace_has_no_badge() {
    let (store, _, generated) = store_with(&Scenario::CheckoutSuccess, 0..1);
    let snapshot = store.trace(generated.trace_ids[0]).unwrap();
    let header = header(&snapshot, 0);
    assert_eq!(header.failure, Failure::None);
    assert_eq!(header.failure.badge(), None);
    assert!(
        header
            .summary
            .starts_with("1.84 s \u{b7} 14 spans \u{b7} 5 services")
    );
}

#[test]
fn a_trace_without_its_root_reads_root_not_arrived() {
    // LateOutOfOrder's first batch: the ten spans at depth 3 and deeper.
    let (store, _, generated) = store_with(&Scenario::LateOutOfOrder, 0..1);
    let snapshot = store.trace(generated.trace_ids[0]).unwrap();
    let header = header(&snapshot, 0);
    assert_eq!(header.root, "(root not arrived)");
    assert!(!header.root_arrived);
    assert!(header.summary.contains("10 spans"), "{}", header.summary);
    // Nothing is quiet yet: the indicator says spans are still coming.
    assert!(!header.complete);
    assert_eq!(header.completeness(), RECEIVING);
}

#[test]
fn errors_inside_is_the_amber_badge() {
    let (store, _, generated) = store_with(&Scenario::CheckoutFailure, 0..1);
    let mut spans = store.trace(generated.trace_ids[0]).unwrap().spans;
    // The root is fine, a span below it is erroring: errors inside.
    for span in &mut spans {
        if span.name == "POST /api/checkout" {
            span.status = loctel_model::StatusCode::Unset;
        }
    }
    let fresh = Store::new(StoreConfig::default(), Arc::new(ManualClock::new(BASE)));
    fresh.ingest_spans(spans);
    let snapshot = fresh.trace(generated.trace_ids[0]).unwrap();
    let header = header(&snapshot, 0);
    assert_eq!(header.failure, Failure::ErrorsInside);
    assert_eq!(header.failure.badge().as_deref(), Some("Errors inside"));
}

#[test]
fn one_span_and_one_service_are_singular() {
    let (store, _, generated) = store_with(&Scenario::HealthChecks { count: 1 }, 0..1);
    let trace = generated.trace_ids[0];
    let snapshot = store.trace(trace).unwrap();
    let header = header(&snapshot, 0);
    assert!(
        header.summary.contains("1 span \u{b7} 1 service \u{b7}"),
        "{}",
        header.summary
    );
}

#[test]
fn big_counts_have_a_thousands_separator() {
    let (store, _, generated) = store_with(&Scenario::HugeTrace { spans: 1_500 }, 0..1);
    let snapshot = store.trace(generated.trace_ids[0]).unwrap();
    let header = header(&snapshot, 0);
    assert!(header.summary.contains("1,500 spans"), "{}", header.summary);
}

// --- the find box ----------------------------------------------------------------

#[test]
fn find_status_says_how_many_and_which() {
    assert_eq!(find_status(true, 0, None), "", "an empty box says nothing");
    assert_eq!(find_status(true, 7, Some(2)), "");
    assert_eq!(find_status(false, 0, None), "No matches");
    assert_eq!(find_status(false, 0, Some(0)), "No matches");
    assert_eq!(find_status(false, 1, None), "1 match");
    assert_eq!(find_status(false, 5, None), "5 matches");
    assert_eq!(find_status(false, 5, Some(1)), "2 of 5");
    assert_eq!(find_status(false, 1, Some(0)), "1 of 1");
    assert_eq!(find_status(false, 12_345, None), "12,345 matches");
    assert_eq!(find_status(false, 12_345, Some(1_233)), "1,234 of 12,345");
}

// --- the span's logs ------------------------------------------------------------

/// The four logs of `POST /charges` (the span with number 10 in the M1 plan's
/// table) from the failing checkout.
fn charges_logs() -> Vec<LogRecord> {
    let (store, _, generated) = store_with(&Scenario::CheckoutFailure, 0..1);
    let snapshot = store.trace(generated.trace_ids[0]).unwrap();
    let charges = span_named(&snapshot, "POST /charges", 200).span_id;
    store.logs_for_span(snapshot.trace_id, charges)
}

#[test]
fn log_lines_of_charges() {
    let logs = charges_logs();
    let lines = log_lines(&logs, 0);
    assert_eq!(lines.len(), 4);
    let shown: Vec<(&str, &str)> = lines
        .iter()
        .map(|line| (line.time.as_str(), line.level))
        .collect();
    assert_eq!(
        shown,
        [
            ("08:00:00.210", "INF"),
            ("08:00:00.836", "WRN"),
            ("08:00:01.440", "ERR"),
            ("08:00:01.782", "ERR"),
        ]
    );
    assert_eq!(
        lines[0].message,
        "Charging card for order 8812, amount 129.00 EUR"
    );
    assert_eq!(lines[1].severity, Severity::Warn);
    assert_eq!(lines[3].severity, Severity::Error);
}

#[test]
fn log_lines_keep_one_line_and_fall_back_to_observed_time() {
    let logs = charges_logs();
    let mut log = logs[0].clone();
    log.body = "first\r\nsecond\nthird".to_owned();
    log.time_unix_nano = 0;
    log.observed_unix_nano = BASE + 1_500 * MS;
    let lines = log_lines(&[log], 0);
    assert_eq!(lines[0].message, "first  second third");
    assert_eq!(lines[0].time, "08:00:01.500", "the observed time");
}

#[test]
fn level_badges_are_three_letters() {
    let badges: Vec<&str> = [
        Severity::Unspecified,
        Severity::Trace,
        Severity::Debug,
        Severity::Info,
        Severity::Warn,
        Severity::Error,
        Severity::Fatal,
    ]
    .into_iter()
    .map(level_badge)
    .collect();
    assert_eq!(badges, ["LOG", "TRC", "DBG", "INF", "WRN", "ERR", "FTL"]);
}

#[test]
fn logs_title_and_count() {
    assert_eq!(logs_title("POST /charges"), "Logs from POST /charges");
    assert_eq!(logs_count(0), "0 logs");
    assert_eq!(logs_count(1), "1 log");
    assert_eq!(logs_count(4), "4 logs");
}

// --- the inspector -----------------------------------------------------------------

#[test]
fn value_preview_cuts_at_200_characters() {
    assert_eq!(VALUE_PREVIEW_CHARS, 200);
    assert_eq!(value_preview(""), None);
    assert_eq!(value_preview("short"), None);
    let exactly = "a".repeat(200);
    assert_eq!(value_preview(&exactly), None, "200 fit");
    let over = "a".repeat(201);
    assert_eq!(value_preview(&over), Some(exactly.as_str()), "201 are cut");
    let huge = "b".repeat(20_000);
    assert_eq!(value_preview(&huge).map(str::len), Some(200));
}

#[test]
fn value_preview_counts_characters_not_bytes() {
    // 200 two-byte characters fit; 201 are cut after the 200th.
    let fits = "\u{e9}".repeat(200);
    assert_eq!(value_preview(&fits), None);
    let over = format!("{fits}\u{e9}");
    assert_eq!(value_preview(&over), Some(fits.as_str()));
}

#[test]
fn exception_text_is_the_type_message_and_trace() {
    let exception = ExceptionInfo {
        type_name: EXCEPTION_TYPE.to_owned(),
        message: EXCEPTION_MESSAGE.to_owned(),
        stacktrace: Some(EXCEPTION_STACKTRACE.to_owned()),
    };
    assert_eq!(
        exception_text(&exception),
        format!("{EXCEPTION_TYPE}: {EXCEPTION_MESSAGE}\n{EXCEPTION_STACKTRACE}")
    );

    let bare = ExceptionInfo {
        type_name: String::new(),
        message: "boom".to_owned(),
        stacktrace: None,
    };
    assert_eq!(exception_text(&bare), "boom");
    let typed = ExceptionInfo {
        type_name: "Boom".to_owned(),
        message: String::new(),
        stacktrace: Some(String::new()),
    };
    assert_eq!(
        exception_text(&typed),
        "Boom",
        "an empty trace adds nothing"
    );
}

#[test]
fn span_json_is_the_span_pretty_printed() {
    let snapshot = failing();
    let charges = span_named(&snapshot, "POST /charges", 200);
    let json = span_json(&snapshot, charges.span_id).expect("the span is in the trace");

    let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
    assert_eq!(value["name"], "POST /charges");
    assert_eq!(value["span_id"], charges.span_id.to_string());
    assert_eq!(value["trace_id"], snapshot.trace_id.to_string());
    assert_eq!(value["attributes"]["order.id"], 8812);
    assert!(json.contains('\n'), "pretty-printed");
    assert!(
        span_json(&snapshot, SpanId([0xee; 8])).is_none(),
        "a span the trace does not have"
    );
}

#[test]
fn details_of_the_failed_in_span_for_the_inspector() {
    // The span the trace view opens on: payments-api's first call to the card
    // gateway (number 12 in the table, starting at 236 ms).
    let snapshot = failing();
    let span = span_named(&snapshot, "POST", 236);
    let details = span_details(&snapshot, span.span_id).expect("details");
    assert_eq!(details.service, "payments-api");
    assert_eq!(details.kind, "Client");
    assert_eq!(details.offset, "starts at +236 ms");
    assert!(details.status.starts_with("Error"), "{}", details.status);
    assert!(details.exception.is_none(), "the exception is on /charges");
}

// --- traces through this span name ----------------------------------------------------

#[test]
fn span_name_filter_writes_a_name_query() {
    assert_eq!(
        span_name_filter("POST /charges"),
        "{ name = \"POST /charges\" }"
    );
    assert_eq!(span_name_filter(""), "{ name = \"\" }");
}

#[test]
fn span_name_filter_escapes_quotes_and_backslashes() {
    assert_eq!(
        span_name_filter(r#"say "hi" \o/"#),
        r#"{ name = "say \"hi\" \\o/" }"#
    );
}

#[test]
fn span_name_filter_is_a_query_that_parses_for_any_name() {
    for name in [
        "POST /charges",
        "GET /stock/{sku}",
        r#"quote " and \ slash"#,
        "multi\nline",
        "ends with backslash \\",
        &"x".repeat(300),
    ] {
        let state = query_bar::validate(&span_name_filter(name));
        assert!(
            matches!(state, QueryState::Valid(_)),
            "`{name}` gives {state:?}"
        );
    }
}

#[test]
fn span_name_filter_matches_exactly_the_traces_with_that_span() {
    use loctel_store::{PageRequest, TraceFilter};

    let (store, _, _) = store_with(&Scenario::CheckoutFailure, 0..1);
    let QueryState::Valid(query) = query_bar::validate(&span_name_filter("POST /charges")) else {
        panic!("the query parses");
    };
    let page = store.search_traces(
        &TraceFilter {
            query: Some(query),
            ..TraceFilter::default()
        },
        &PageRequest {
            limit: 10,
            cursor: None,
        },
    );
    assert_eq!(page.items.len(), 1);

    let QueryState::Valid(query) = query_bar::validate(&span_name_filter("no such span")) else {
        panic!("the query parses");
    };
    let page = store.search_traces(
        &TraceFilter {
            query: Some(query),
            ..TraceFilter::default()
        },
        &PageRequest {
            limit: 10,
            cursor: None,
        },
    );
    assert!(page.items.is_empty());
}
