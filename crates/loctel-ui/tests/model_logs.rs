//! The Logs screen's models (M4 plan, Task 3): the rows of the list, the
//! expanded line's details, the LogQL bar's states, the query `Open in Logs`
//! writes, and the volume histogram's bars. Built from the fixed fake mesh:
//! `CheckoutFailure` has 7 logs (3 INF, 1 WRN and 3 ERR) of `orders-api` and
//! `payments-api` on two spans of one trace.

use std::sync::Arc;

use loctel_ingest::{logs_from_request, spans_from_request};
use loctel_meshgen::{Scenario, generate};
use loctel_model::{AttrValue, LogRecord, Resource, Severity, SpanId, TraceId};
use loctel_query::logql;
use loctel_store::{
    HistogramBucket, LevelSet, LogFilter, LogId, ManualClock, PageRequest, Store, StoreConfig,
};
use loctel_ui::model::colors::ServiceColors;
use loctel_ui::model::histogram::{self, Bars};
use loctel_ui::model::logs::{
    BACK_TO_SEARCH, CAPPED_FOOTER, EXAMPLE_QUERY, LevelBadge, LogQueryState, LogRow, MAX_ROWS,
    NEW_LINES_PILL, PAGE_SIZE, PREFETCH_ROWS, context_banner, context_count_line, count_line,
    details, error_line, footer, level_is_on, levels_with, merge_head, rows, span_logs_query,
    validate,
};

/// 2027-01-15T08:00:00Z. The failing checkout starts here.
const BASE: u64 = 1_800_000_000_000_000_000;
const MS: u64 = 1_000_000;
const MINUTE: u64 = 60_000 * MS;
const SEED: u64 = 7;

/// The failing checkout in a store: its 7 logs newest first with their IDs
/// (as the Logs screen reads them), and the trace they belong to.
struct Fixture {
    store: Store,
    logs: Vec<(LogId, LogRecord)>,
    trace_id: TraceId,
}

fn fixture() -> Fixture {
    let clock = Arc::new(ManualClock::new(BASE));
    let store = Store::new(StoreConfig::default(), clock);
    let generated = generate(&Scenario::CheckoutFailure, SEED, BASE);
    for batch in &generated.batches {
        if let Some(request) = &batch.traces {
            store.ingest_spans(spans_from_request(request.clone()).items);
        }
        if let Some(request) = &batch.logs {
            store.ingest_logs(logs_from_request(request.clone()).items);
        }
    }
    let page = store.search_logs_keyed(
        &LogFilter::default(),
        &PageRequest {
            limit: 100,
            cursor: None,
        },
    );
    assert_eq!(page.items.len(), 7, "the failing checkout has 7 logs");
    Fixture {
        store,
        logs: page.items,
        trace_id: generated.trace_ids[0],
    }
}

/// A log with just what a row reads; the rest is empty.
fn log(severity: Severity, body: &str) -> LogRecord {
    LogRecord {
        time_unix_nano: BASE + 5 * MS,
        observed_unix_nano: BASE + 9 * MS,
        severity,
        severity_text: String::new(),
        body: body.to_owned(),
        attributes: Vec::new(),
        trace_id: None,
        span_id: None,
        resource: Arc::new(Resource::default()),
        scope_name: String::new(),
    }
}

fn id_of(record: &LogRecord) -> LogId {
    LogId {
        time_unix_nano: record.time_unix_nano,
        seq: 1,
    }
}

fn one_row(record: &LogRecord) -> LogRow {
    let mut colors = ServiceColors::new();
    rows(&[(id_of(record), record.clone())], &mut colors, 0).remove(0)
}

// --- the rows -----------------------------------------------------------------

#[test]
fn rows_of_the_failed_checkout() {
    let Fixture {
        store,
        logs,
        trace_id,
    } = fixture();
    let mut colors = ServiceColors::new();

    let rows = rows(&logs, &mut colors, 0);

    // The store gives newest first, and the rows keep its order.
    let seen: Vec<(&str, &str, &str, &str, usize)> = rows
        .iter()
        .map(|row| {
            (
                row.time.as_str(),
                row.level.label(),
                row.service.as_str(),
                row.message.as_str(),
                row.color,
            )
        })
        .collect();
    assert_eq!(
        seen,
        vec![
            (
                "08:00:01.800",
                "ERR",
                "orders-api",
                "Checkout failed for order 8812: payments-api returned 500",
                0
            ),
            (
                "08:00:01.782",
                "ERR",
                "payments-api",
                "Charge failed for order 8812: CardGatewayTimeoutException: card-gateway did not respond after 2 attempts",
                1
            ),
            (
                "08:00:01.440",
                "ERR",
                "payments-api",
                "card-gateway did not answer within 600 ms",
                1
            ),
            (
                "08:00:00.836",
                "WRN",
                "payments-api",
                "card-gateway did not answer within 600 ms, retrying (attempt 2 of 2)",
                1
            ),
            (
                "08:00:00.210",
                "INF",
                "payments-api",
                "Charging card for order 8812, amount 129.00 EUR",
                1
            ),
            (
                "08:00:00.190",
                "INF",
                "orders-api",
                "Requesting payment for order 8812",
                0
            ),
            (
                "08:00:00.010",
                "INF",
                "orders-api",
                "Created order 8812 for cart c-5521",
                0
            ),
        ]
    );

    // Every row is the link to the trace, with the first 8 hex digits shown.
    let short = trace_id.to_string()[..8].to_owned();
    for row in &rows {
        assert_eq!(row.trace_id, Some(trace_id));
        assert_eq!(row.trace_short.as_deref(), Some(short.as_str()));
        assert_eq!(row.trace_short.as_deref().map(str::len), Some(8));
    }

    // And to its span: the payments logs are on `POST /charges`, the orders
    // ones on `POST /orders`.
    let snapshot = store.trace(trace_id).expect("the trace is held");
    let span = |name: &str| {
        snapshot
            .spans
            .iter()
            .find(|span| span.name == name)
            .unwrap_or_else(|| panic!("no span {name}"))
            .span_id
    };
    assert_eq!(rows[0].span_id, Some(span("POST /orders")));
    assert_eq!(rows[1].span_id, Some(span("POST /charges")));

    // The row's ID is the log's: the store gives the same line back.
    for (row, (id, record)) in rows.iter().zip(&logs) {
        assert_eq!(row.id, *id);
        assert_eq!(store.log(row.id).as_ref(), Some(record));
    }
}

#[test]
fn service_colours_follow_first_asked_not_the_list() {
    let Fixture { logs, .. } = fixture();
    let mut colors = ServiceColors::new();
    // The window has seen payments-api before it reads these rows.
    assert_eq!(colors.index("payments-api"), 0);

    let rows = rows(&logs, &mut colors, 0);

    assert_eq!(rows[0].service, "orders-api");
    assert_eq!(rows[0].color, 1);
    assert_eq!(rows[1].service, "payments-api");
    assert_eq!(rows[1].color, 0);
}

#[test]
fn rows_of_nothing_are_nothing() {
    let mut colors = ServiceColors::new();
    assert!(rows(&[], &mut colors, 0).is_empty());
}

#[test]
fn the_time_is_in_the_given_zone() {
    let record = log(Severity::Info, "x");
    let mut colors = ServiceColors::new();
    let at_plus_one = rows(&[(id_of(&record), record.clone())], &mut colors, 3_600);
    assert_eq!(at_plus_one[0].time, "09:00:00.005");
}

#[test]
fn a_log_without_a_time_shows_its_observed_time() {
    let mut record = log(Severity::Info, "x");
    record.time_unix_nano = 0;
    assert_eq!(one_row(&record).time, "08:00:00.009");
}

#[test]
fn level_badges_group_the_severities_like_the_toggles() {
    let expected = [
        (Severity::Fatal, LevelBadge::Err, "ERR"),
        (Severity::Error, LevelBadge::Err, "ERR"),
        (Severity::Warn, LevelBadge::Wrn, "WRN"),
        (Severity::Info, LevelBadge::Inf, "INF"),
        (Severity::Unspecified, LevelBadge::Inf, "INF"),
        (Severity::Debug, LevelBadge::Dbg, "DBG"),
        (Severity::Trace, LevelBadge::Dbg, "DBG"),
    ];
    for (severity, badge, label) in expected {
        let row = one_row(&log(severity, "x"));
        assert_eq!(row.level, badge, "{severity:?}");
        assert_eq!(row.level.label(), label, "{severity:?}");
    }
}

#[test]
fn a_multi_line_body_shows_on_one_line() {
    // A stack trace in a log message is one row: every line break is a ` ⏎ `.
    let row = one_row(&log(
        Severity::Error,
        "Charge failed\n  at Gateway.charge(Gateway.cs:41)\n  at Api.post(Api.cs:12)",
    ));
    assert_eq!(
        row.message,
        "Charge failed \u{23ce}   at Gateway.charge(Gateway.cs:41) \u{23ce}   at Api.post(Api.cs:12)"
    );
}

#[test]
fn a_windows_line_break_is_one_mark_and_a_trailing_one_is_none() {
    assert_eq!(
        one_row(&log(Severity::Info, "one\r\ntwo\rthree")).message,
        "one \u{23ce} two \u{23ce} three"
    );
    // Nothing follows a trailing break, so it says nothing.
    assert_eq!(one_row(&log(Severity::Info, "done\n")).message, "done");
    assert_eq!(
        one_row(&log(Severity::Info, "done\r\n\r\n")).message,
        "done"
    );
    assert_eq!(one_row(&log(Severity::Info, "")).message, "");
}

#[test]
fn a_log_outside_a_trace_has_no_link() {
    let row = one_row(&log(Severity::Info, "no trace"));
    assert_eq!(row.trace_id, None);
    assert_eq!(row.trace_short, None);
    assert_eq!(row.span_id, None);
}

#[test]
fn a_log_with_a_trace_but_no_span_links_to_the_trace_only() {
    let mut record = log(Severity::Info, "in a trace");
    let trace = TraceId::from_hex("0af7651916cd43dd8448eb211c80319c").unwrap();
    record.trace_id = Some(trace);
    let row = one_row(&record);
    assert_eq!(row.trace_id, Some(trace));
    assert_eq!(row.trace_short.as_deref(), Some("0af76519"));
    assert_eq!(row.span_id, None);
}

// --- the expanded line --------------------------------------------------------

#[test]
fn details_start_with_the_whole_body_then_the_labels() {
    let Fixture { logs, trace_id, .. } = fixture();
    // The ERR "Charge failed…" line: payments-api, on span POST /charges.
    let (_, record) = &logs[1];

    let details = details(record);

    assert_eq!(
        details[0],
        ("body".to_owned(), record.body.clone()),
        "the body comes first, whole"
    );
    // The rest is the label set LogQL sees, in its order.
    assert_eq!(details[1..], logql::labels(record)[..]);
    let get = |name: &str| {
        details
            .iter()
            .find(|(label, _)| label == name)
            .map(|(_, value)| value.as_str())
    };
    assert_eq!(get("service_name"), Some("payments-api"));
    assert_eq!(get("level"), Some("error"));
    assert_eq!(get("trace_id"), Some(trace_id.to_string().as_str()));
    assert!(get("span_id").is_some());
    assert_eq!(get("order_id"), None, "only what the log carries is shown");
}

#[test]
fn details_keep_the_body_whole_even_when_it_is_long_and_multi_line() {
    let body = format!("first\n{}\nlast", "x".repeat(5_000));
    let record = log(Severity::Warn, &body);
    assert_eq!(details(&record)[0], ("body".to_owned(), body));
}

#[test]
fn details_show_attributes_with_loki_label_names() {
    let mut record = log(Severity::Info, "x");
    record.attributes = vec![(
        "http.route".to_owned(),
        AttrValue::Str("/orders".to_owned()),
    )];
    let details = details(&record);
    assert!(
        details.contains(&("http_route".to_owned(), "/orders".to_owned())),
        "{details:?}"
    );
}

// --- the LogQL bar --------------------------------------------------------------

#[test]
fn an_empty_bar_is_no_filter() {
    assert_eq!(validate(""), LogQueryState::Empty);
    assert_eq!(validate("  \t\n "), LogQueryState::Empty);
}

#[test]
fn a_good_query_is_parsed_once_and_shared() {
    let text = r#"{service_name="payments-api"} | level = "error""#;
    let LogQueryState::Valid(query) = validate(text) else {
        panic!("the query is valid");
    };
    assert_eq!(*query, logql::parse(text).unwrap());
}

#[test]
fn a_bad_query_says_where_counting_from_one() {
    // `{service_name=` is 14 characters: the problem is at the end.
    let LogQueryState::Invalid { column, message } = validate("{service_name=") else {
        panic!("the query is invalid");
    };
    assert_eq!(column, 15);
    assert!(!message.is_empty());
    assert!(
        !message.contains("column"),
        "the message has no column: {message}"
    );

    // Blanks the user typed count.
    let LogQueryState::Invalid { column, .. } = validate("   {}") else {
        panic!("an empty selector is an error");
    };
    assert_eq!(column, logql::parse("   {}").unwrap_err().position + 1);
}

#[test]
fn an_unsupported_feature_is_an_error_not_a_silent_mismatch() {
    let LogQueryState::Invalid { message, .. } = validate(r#"sum(rate({service_name="a"}[5m]))"#)
    else {
        panic!("metric queries are not supported");
    };
    assert!(message.contains("not supported yet"), "{message}");
}

// --- Open in Logs ---------------------------------------------------------------

#[test]
fn the_span_logs_query_names_the_service_and_the_span() {
    let span = SpanId::from_hex("00f067aa0ba902b7").unwrap();
    assert_eq!(
        span_logs_query("payments-api", span),
        r#"{service_name="payments-api"} | span_id = "00f067aa0ba902b7""#
    );
}

#[test]
fn the_span_logs_query_escapes_the_service_name() {
    let span = SpanId::from_hex("00f067aa0ba902b7").unwrap();
    let query = span_logs_query(r#"we"ird\name"#, span);
    assert_eq!(
        query,
        r#"{service_name="we\"ird\\name"} | span_id = "00f067aa0ba902b7""#
    );
    assert!(
        matches!(validate(&query), LogQueryState::Valid(_)),
        "{query}"
    );
}

#[test]
fn the_span_logs_query_finds_exactly_the_logs_of_the_span() {
    let Fixture { logs, .. } = fixture();
    // The payments logs are on one span, `POST /charges`; the span of the ERR
    // "Charge failed…" line.
    let span = logs[1].1.span_id.expect("the log is on a span");

    let text = span_logs_query("payments-api", span);
    let query = logql::parse(&text).expect("it is a LogQL query");

    let matched: Vec<&str> = logs
        .iter()
        .filter(|(_, record)| query.matches(record))
        .map(|(_, record)| record.body.as_str())
        .collect();
    assert_eq!(
        matched.len(),
        4,
        "the four logs of POST /charges: {matched:?}"
    );
    assert!(matched.contains(&"card-gateway did not answer within 600 ms"));
    assert!(logs.iter().filter(|(_, r)| r.span_id == Some(span)).count() == 4);

    // The same span under another service matches nothing.
    let other = logql::parse(&span_logs_query("orders-api", span)).unwrap();
    assert!(logs.iter().all(|(_, record)| !other.matches(record)));
}

// --- the histogram --------------------------------------------------------------

fn bucket(total: u64, errors: u64) -> HistogramBucket {
    HistogramBucket { total, errors }
}

#[test]
fn bars_are_fractions_of_the_tallest() {
    let buckets = [bucket(2, 0), bucket(4, 1), bucket(0, 0), bucket(1, 1)];

    let Bars { bars, .. } = histogram::bars(&buckets, BASE, BASE + 15 * MINUTE, 0);

    assert_eq!(
        bars,
        vec![(0.5, 0.0), (1.0, 0.25), (0.0, 0.0), (0.25, 0.25)]
    );
}

#[test]
fn the_error_share_is_on_the_same_scale_as_the_total() {
    // 1 error in a bar of 3, when the tallest is 6: the error part is 1/6 of
    // the height of the tallest, not 1/3 of its own bar.
    let bars = histogram::bars(&[bucket(6, 0), bucket(3, 1)], BASE, BASE + MINUTE, 0).bars;
    assert_eq!(bars[1].0, 0.5);
    assert!((bars[1].1 - 1.0 / 6.0).abs() < f32::EPSILON);
}

#[test]
fn a_bar_never_shows_more_errors_than_logs() {
    // The store cannot say this; the model still keeps the stack inside the bar.
    let bars = histogram::bars(&[bucket(2, 9)], BASE, BASE + MINUTE, 0).bars;
    assert_eq!(bars, vec![(1.0, 1.0)]);
}

#[test]
fn no_logs_at_all_is_flat_bars_not_a_division_by_zero() {
    let bars = histogram::bars(&[bucket(0, 0); 48], BASE, BASE + 15 * MINUTE, 0).bars;
    assert_eq!(bars.len(), 48);
    assert!(bars.iter().all(|&bar| bar == (0.0, 0.0)));
}

#[test]
fn no_buckets_are_no_bars_but_the_labels_stay() {
    let bars = histogram::bars(&[], BASE, BASE + 15 * MINUTE, 0);
    assert!(bars.bars.is_empty());
    assert_eq!(bars.labels[0], "08:00:00.000");
}

#[test]
fn four_labels_at_the_start_a_third_two_thirds_and_the_end() {
    let bars = histogram::bars(&[bucket(1, 0)], BASE, BASE + 15 * MINUTE, 0);
    assert_eq!(
        bars.labels,
        [
            "08:00:00.000".to_owned(),
            "08:05:00.000".to_owned(),
            "08:10:00.000".to_owned(),
            "08:15:00.000".to_owned(),
        ]
    );
}

#[test]
fn labels_are_in_the_given_zone_and_cut_not_rounded() {
    // A thirds of a second: 333.33 ms and 666.66 ms, cut to whole ms by the
    // clock, so the label never runs ahead of the instant.
    let bars = histogram::bars(&[bucket(1, 0)], BASE, BASE + 1_000 * MS, 3_600);
    assert_eq!(
        bars.labels,
        [
            "09:00:00.000".to_owned(),
            "09:00:00.333".to_owned(),
            "09:00:00.666".to_owned(),
            "09:00:01.000".to_owned(),
        ]
    );
}

#[test]
fn an_empty_or_reversed_window_puts_every_label_at_the_start() {
    for until in [BASE, BASE - MINUTE] {
        let bars = histogram::bars(&[bucket(1, 0)], BASE, until, 0);
        assert!(bars.labels.iter().all(|label| label == "08:00:00.000"));
    }
}

#[test]
fn the_failed_checkout_is_one_tall_bar_of_the_store_histogram() {
    let Fixture { store, .. } = fixture();
    // A range of 15 minutes that began 10 minutes before the checkout.
    let since = BASE - 10 * MINUTE;
    let until = since + 15 * MINUTE;
    // 48 buckets of 18.75 s: the checkout's logs (all within 1.8 s of BASE)
    // are in the bucket that holds BASE.
    let buckets = store.log_histogram(&LogFilter::default(), since, until, 48);

    let Bars { bars, labels } = histogram::bars(&buckets, since, until, 0);

    assert_eq!(bars.len(), 48);
    // BASE is 10 minutes after `since`: bucket 32. All 7 logs, 3 of them errors.
    assert_eq!(bars[32], (1.0, 3.0 / 7.0));
    assert!(
        bars.iter()
            .enumerate()
            .all(|(i, &bar)| i == 32 || bar == (0.0, 0.0))
    );
    assert_eq!(labels[0], "07:50:00.000");
    assert_eq!(labels[2], "08:00:00.000", "two thirds in is where BASE is");
    assert_eq!(labels[3], "08:05:00.000");
}

// --- the bar's hint and error line ------------------------------------------------

#[test]
fn the_example_query_is_a_query_the_bar_accepts() {
    // The placeholder must never teach a syntax the bar rejects.
    assert!(
        matches!(validate(EXAMPLE_QUERY), LogQueryState::Valid(_)),
        "{EXAMPLE_QUERY}"
    );
}

#[test]
fn the_error_line_says_where_and_what() {
    assert_eq!(error_line(&LogQueryState::Empty), None);
    assert_eq!(error_line(&validate(r#"{service_name="a"}"#)), None);

    let LogQueryState::Invalid { column, message } = validate(r#"{service_name="a""#) else {
        panic!("an unclosed selector is an error");
    };
    assert_eq!(
        error_line(&validate(r#"{service_name="a""#)),
        Some(format!("at column {column}: {message}"))
    );
}

// --- the count, the footer, the banner and the pill --------------------------------

#[test]
fn the_paging_numbers_are_the_plans() {
    assert_eq!(PAGE_SIZE, 500);
    assert_eq!(MAX_ROWS, 20_000);
    assert_eq!(PREFETCH_ROWS, 50);
}

#[test]
fn the_count_line_counts_loaded_lines_and_marks_more_to_come() {
    // Everything that matches is loaded: an exact count, newest first.
    assert_eq!(count_line(0, false, false), "0 lines \u{b7} newest first");
    assert_eq!(count_line(1, false, false), "1 line \u{b7} newest first");
    assert_eq!(
        count_line(1_204, false, false),
        "1,204 lines \u{b7} newest first"
    );
    // More pages wait: the count is what is loaded, and it says so.
    assert_eq!(
        count_line(500, true, false),
        "500+ lines \u{b7} newest first"
    );
    // At the cap the plan's text, with no ordering.
    assert_eq!(count_line(20_000, true, true), "20,000+ lines");
}

#[test]
fn the_lines_around_a_log_are_counted_oldest_first() {
    assert_eq!(context_count_line(51), "51 lines \u{b7} oldest first");
    assert_eq!(context_count_line(1), "1 line \u{b7} oldest first");
}

#[test]
fn the_footer_is_only_there_at_the_cap() {
    assert_eq!(CAPPED_FOOTER, "Showing the newest 20,000 matching lines");
    assert_eq!(footer(false), None);
    assert_eq!(footer(true), Some(CAPPED_FOOTER));
}

#[test]
fn the_banner_names_the_time_or_says_the_line_is_gone() {
    let id = LogId {
        time_unix_nano: BASE + 9 * 1_000 * MS + 194 * MS,
        seq: 3,
    };
    assert_eq!(context_banner(id, true, 0), "Lines around 08:00:09.194");
    assert_eq!(
        context_banner(id, true, 3_600),
        "Lines around 09:00:09.194",
        "in the zone given"
    );
    // The time is in the ID, so an evicted line still has a banner; it says
    // what the plan says.
    assert_eq!(context_banner(id, false, 0), "This line is no longer held");
    assert_eq!(BACK_TO_SEARCH, "Back to search");
}

#[test]
fn the_pill_text_is_the_plans() {
    assert_eq!(NEW_LINES_PILL, "New lines \u{b7} Jump to newest");
}

// --- how the live tail joins the newest page to the rows held ------------------------

fn ids(times: &[u64]) -> Vec<LogId> {
    // Newest first, as the store gives them; the arrival number follows the time.
    times
        .iter()
        .map(|&t| LogId {
            time_unix_nano: t,
            seq: t,
        })
        .collect()
}

/// The list the model builds: the fresh page, then the held rows it did not
/// supersede.
fn joined(held: &[LogId], fresh: &[LogId], keep_from: usize) -> Vec<LogId> {
    fresh.iter().chain(&held[keep_from..]).copied().collect()
}

#[test]
fn a_page_that_reaches_the_held_rows_is_joined_without_a_gap() {
    let held = ids(&[50, 40, 30, 20, 10]);
    // 70 and 60 are new; 50 and 40 are rows we hold.
    let fresh = ids(&[70, 60, 50, 40]);

    let keep_from = merge_head(&held, |id| *id, &fresh).expect("the page overlaps");

    assert_eq!(keep_from, 2, "50 and 40 are superseded by the page");
    assert_eq!(
        joined(&held, &fresh, keep_from),
        ids(&[70, 60, 50, 40, 30, 20, 10])
    );
}

#[test]
fn a_page_with_nothing_new_changes_nothing() {
    let held = ids(&[50, 40, 30]);
    let fresh = ids(&[50, 40, 30]);
    let keep_from = merge_head(&held, |id| *id, &fresh).expect("the page overlaps");
    assert_eq!(joined(&held, &fresh, keep_from), held);
}

#[test]
fn a_late_line_inside_the_page_is_picked_up_once() {
    // 45 arrived late: older than the newest held row (50), newer than 40.
    let held = ids(&[50, 40, 30, 20]);
    let fresh = ids(&[60, 50, 45, 40, 30]);
    let keep_from = merge_head(&held, |id| *id, &fresh).expect("the page overlaps");
    assert_eq!(keep_from, 3);
    assert_eq!(
        joined(&held, &fresh, keep_from),
        ids(&[60, 50, 45, 40, 30, 20])
    );
}

#[test]
fn a_page_that_ends_above_the_held_rows_is_a_gap_and_starts_over() {
    // A flood: more lines arrived than a page holds, so the page ends newer
    // than the newest row held and lines in between were never read.
    let held = ids(&[50, 40, 30]);
    let fresh = ids(&[900, 800, 700]);
    assert_eq!(merge_head(&held, |id| *id, &fresh), None);
}

#[test]
fn an_empty_page_means_nothing_matches_so_nothing_held_is_kept() {
    // The store was cleared, or the logs held were all evicted.
    assert_eq!(merge_head(&ids(&[50, 40]), |id| *id, &[]), None);
    assert_eq!(merge_head(&[] as &[LogId], |id| *id, &[]), None);
}

#[test]
fn a_page_joins_an_empty_list_whole() {
    let fresh = ids(&[30, 20]);
    assert_eq!(merge_head(&[] as &[LogId], |id| *id, &fresh), Some(0));
}

#[test]
fn a_page_that_ends_on_the_newest_held_row_overlaps_by_that_row() {
    let held = ids(&[50, 40]);
    let fresh = ids(&[70, 60, 50]);
    let keep_from = merge_head(&held, |id| *id, &fresh).expect("the page overlaps");
    assert_eq!(joined(&held, &fresh, keep_from), ids(&[70, 60, 50, 40]));
}

#[test]
fn whatever_arrives_the_joined_list_is_strictly_newest_first() {
    // The invariant the list stands on, over many shapes of page: a joined
    // list has no line twice and no line out of order.
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = move |below: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % below
    };
    for _ in 0..2_000 {
        // The store: distinct times 1..=200, a random subset.
        let mut store: Vec<u64> = (1..=200).filter(|_| next(3) != 0).collect();
        store.sort_unstable_by(|a, b| b.cmp(a));
        // Held: a run of the store that was the newest page some time ago.
        let start = next(store.len() as u64 + 1) as usize;
        let len = next(40) as usize;
        let held: Vec<u64> = store.iter().copied().skip(start).take(len).collect();
        // Fresh: the newest `page` rows of the store now.
        let page = 1 + next(60) as usize;
        let fresh: Vec<u64> = store.iter().copied().take(page).collect();
        let (held, fresh) = (ids(&held), ids(&fresh));

        if let Some(keep_from) = merge_head(&held, |id| *id, &fresh) {
            let list = joined(&held, &fresh, keep_from);
            assert!(
                list.windows(2).all(|pair| pair[0] > pair[1]),
                "held {held:?} fresh {fresh:?} gave {list:?}"
            );
        }
    }
}

// --- the level toggles ---------------------------------------------------------------

#[test]
fn all_four_toggles_start_on_and_each_flips_its_own_group() {
    let all = LevelSet::ALL;
    for badge in LevelBadge::ALL {
        assert!(level_is_on(all, badge), "{badge:?} is on in the full set");
    }

    // Switching one group off leaves the other three on.
    for off in LevelBadge::ALL {
        let set = levels_with(all, off, false);
        for badge in LevelBadge::ALL {
            assert_eq!(
                level_is_on(set, badge),
                badge != off,
                "{off:?} off, {badge:?}"
            );
        }
        // And on again gives the full set back.
        assert_eq!(levels_with(set, off, true), all);
    }
}

#[test]
fn a_toggle_covers_the_severities_of_its_group() {
    let only_err = levels_with(
        levels_with(
            levels_with(LevelSet::ALL, LevelBadge::Wrn, false),
            LevelBadge::Inf,
            false,
        ),
        LevelBadge::Dbg,
        false,
    );
    assert_eq!(only_err, LevelSet::from_groups(true, false, false, false));
    assert!(only_err.contains(Severity::Error) && only_err.contains(Severity::Fatal));
    assert!(!only_err.contains(Severity::Warn));

    // INF takes the unlabelled logs with it, as the store's groups say.
    let inf = levels_with(LevelSet::default(), LevelBadge::Inf, true);
    assert!(inf.contains(Severity::Info) && inf.contains(Severity::Unspecified));
    assert!(!inf.contains(Severity::Debug));
}
