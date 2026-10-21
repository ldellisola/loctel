//! TraceQL and LogQL queries as store filters: which traces and logs a query
//! keeps, and the spans of a trace that it matched.
//!
//! The data is the fixed fake mesh (the `CheckoutFailure` table in the M1
//! plan), converted by the real OTLP code.

mod fixtures;

use std::sync::Arc;

use fixtures::*;
use loctel_meshgen::Scenario;
use loctel_model::{Severity, SpanId, StatusCode};
use loctel_query::{logql, traceql};
use loctel_store::{LogFilter, PageRequest, Store, TraceFilter};

/// Where the failing checkout starts; the successful one starts 10 s later.
const FAILURE_BASE: u64 = 2_000_000_000_000;
const SUCCESS_BASE: u64 = FAILURE_BASE + 10_000_000_000;

fn all() -> PageRequest {
    PageRequest {
        limit: usize::MAX,
        cursor: None,
    }
}

fn trace_filter(query: &str) -> TraceFilter {
    TraceFilter {
        query: Some(Arc::new(
            traceql::parse(query).unwrap_or_else(|err| panic!("{query}: {err}")),
        )),
        ..TraceFilter::default()
    }
}

fn log_filter(query: &str) -> LogFilter {
    LogFilter {
        query: Some(Arc::new(
            logql::parse(query).unwrap_or_else(|err| panic!("{query}: {err}")),
        )),
        ..LogFilter::default()
    }
}

/// A store holding a failing checkout (seed 1, trace failing) and a
/// successful one (seed 2, 10 s later), with their logs. Returns the two
/// traces' spans as they were ingested.
struct Mesh {
    store: Store,
    failure: Vec<loctel_model::Span>,
    success: Vec<loctel_model::Span>,
}

fn mesh() -> Mesh {
    let (store, _clock) = store_with_manual_clock();
    let failure = scenario_spans(&Scenario::CheckoutFailure, 1, FAILURE_BASE);
    let success = scenario_spans(&Scenario::CheckoutSuccess, 2, SUCCESS_BASE);
    assert_eq!(failure.len(), 15);
    assert_eq!(success.len(), 14);
    store.ingest_spans(failure.clone());
    store.ingest_spans(success.clone());
    store.ingest_logs(scenario_logs(&Scenario::CheckoutFailure, 1, FAILURE_BASE));
    store.ingest_logs(scenario_logs(&Scenario::CheckoutSuccess, 2, SUCCESS_BASE));
    Mesh {
        store,
        failure,
        success,
    }
}

#[test]
fn traceql_query_filters_traces_and_reports_matched_spans() {
    let Mesh { store, failure, .. } = mesh();

    let page = store.search_traces(&trace_filter("{ status = error }"), &all());

    // Only the failing checkout has erroring spans.
    assert_eq!(page.items.len(), 1);
    let summary = &page.items[0];
    assert_eq!(summary.trace_id, failure[0].trace_id);
    assert_eq!(summary.matched_span_ids.len(), 7);

    // They are the 7 erroring spans (rows 1, 3, 4, 9, 10, 12 and 13 of the
    // table), in the order the store holds the spans, which is evaluate's
    // order: ascending positions in the held spans.
    let expected: Vec<SpanId> = failure
        .iter()
        .filter(|span| span.status == StatusCode::Error)
        .map(|span| span.span_id)
        .collect();
    assert_eq!(expected.len(), 7);
    assert_eq!(summary.matched_span_ids, expected);

    // The rest of the summary is the ordinary one.
    assert_eq!(summary.span_count, 15);
    assert_eq!(page.next_cursor, None);

    // Without a query nothing is matched, and every trace is listed.
    let plain = store.search_traces(&TraceFilter::default(), &all());
    assert_eq!(plain.items.len(), 2);
    assert!(plain.items.iter().all(|s| s.matched_span_ids.is_empty()));
    assert!(
        store
            .summary(failure[0].trace_id)
            .unwrap()
            .matched_span_ids
            .is_empty()
    );
}

#[test]
fn structural_query_matches_through_the_trace_view() {
    // `>>` needs the whole trace, so this goes through TraceView, not one span.
    let Mesh {
        store,
        failure,
        success,
    } = mesh();

    let page = store.search_traces(
        &trace_filter(r#"{ resource.service.name = "web-bff" } >> { name = "POST /charges" }"#),
        &all(),
    );

    // Both checkouts call payments-api from web-bff, so both match, newest
    // first, each with its own `POST /charges` span.
    let charges = |spans: &[loctel_model::Span]| {
        spans
            .iter()
            .find(|span| span.name == "POST /charges")
            .unwrap()
            .span_id
    };
    assert_eq!(page.items.len(), 2);
    assert_eq!(page.items[0].trace_id, success[0].trace_id);
    assert_eq!(page.items[0].matched_span_ids, vec![charges(&success)]);
    assert_eq!(page.items[1].trace_id, failure[0].trace_id);
    assert_eq!(page.items[1].matched_span_ids, vec![charges(&failure)]);

    // A structural query that only the failing trace satisfies.
    let page = store.search_traces(
        &trace_filter(
            r#"{ resource.service.name = "web-bff" } >> { status = error && kind = server }"#,
        ),
        &all(),
    );
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].trace_id, failure[0].trace_id);
}

#[test]
fn query_that_matches_nothing_gives_an_empty_page() {
    let Mesh { store, .. } = mesh();

    let page = store.search_traces(&trace_filter(r#"{ name = "no such span" }"#), &all());

    assert!(page.items.is_empty());
    assert_eq!(page.next_cursor, None);
}

#[test]
fn query_combines_with_other_conditions() {
    let Mesh {
        store,
        failure,
        success,
    } = mesh();
    let failure_id = failure[0].trace_id;
    let success_id = success[0].trace_id;
    let ids = |filter: &TraceFilter| -> Vec<_> {
        store
            .search_traces(filter, &all())
            .items
            .iter()
            .map(|summary| summary.trace_id)
            .collect()
    };

    // `{ kind = client }` is in both traces, newest first.
    let client = trace_filter("{ kind = client }");
    assert_eq!(ids(&client), vec![success_id, failure_id]);

    // Each other condition narrows it further.
    let with = |change: &dyn Fn(&mut TraceFilter)| {
        let mut filter = client.clone();
        change(&mut filter);
        filter
    };
    assert_eq!(ids(&with(&|f| f.errors_only = true)), vec![failure_id]);
    assert_eq!(
        ids(&with(
            &|f| f.since_unix_nano = Some(FAILURE_BASE + 5_000_000_000)
        )),
        vec![success_id]
    );
    // Both checkouts last 1,840 ms, and the bound is inclusive.
    assert_eq!(
        ids(&with(&|f| f.min_duration_nanos = Some(1_840_000_000))),
        vec![success_id, failure_id]
    );
    assert!(ids(&with(&|f| f.min_duration_nanos = Some(1_840_000_001))).is_empty());
    assert_eq!(
        ids(&with(&|f| f.service = Some("card-gateway".to_owned()))),
        Vec::<loctel_model::TraceId>::new(),
        "no span of either trace belongs to card-gateway"
    );
    assert_eq!(
        ids(&with(&|f| f.service = Some("payments-api".to_owned()))),
        vec![success_id, failure_id]
    );

    // The query is a condition too: the other conditions alone keep a trace
    // that the query rejects out of the result.
    let errors = trace_filter("{ status = error }");
    assert_eq!(ids(&errors), vec![failure_id]);
    let mut late = errors.clone();
    late.since_unix_nano = Some(SUCCESS_BASE);
    assert!(
        ids(&late).is_empty(),
        "the failure is too old, the success has no errors"
    );
    let mut other_service = errors.clone();
    other_service.service = Some("payments-api".to_owned());
    assert_eq!(ids(&other_service), vec![failure_id]);

    // The matched spans come from the query alone, not from the other
    // conditions.
    let mut narrowed = client.clone();
    narrowed.errors_only = true;
    let page = store.search_traces(&narrowed, &all());
    let clients = failure
        .iter()
        .filter(|span| span.kind == loctel_model::SpanKind::Client)
        .count();
    assert_eq!(page.items[0].matched_span_ids.len(), clients);
}

#[test]
fn query_paging_continues_past_traces_the_query_rejects() {
    // Three traces, newest first: success B (matches), the failure (its
    // producer span is named `order.failed publish`, so it does not match) and
    // success A (matches). A page of one must still find the next match.
    let (store, _clock) = store_with_manual_clock();
    let a = scenario_spans(&Scenario::CheckoutSuccess, 3, FAILURE_BASE);
    let failure = scenario_spans(&Scenario::CheckoutFailure, 1, FAILURE_BASE + 1_000_000_000);
    let b = scenario_spans(&Scenario::CheckoutSuccess, 4, FAILURE_BASE + 2_000_000_000);
    let (a_id, b_id) = (a[0].trace_id, b[0].trace_id);
    store.ingest_spans(a);
    store.ingest_spans(failure);
    store.ingest_spans(b);
    let filter = trace_filter(r#"{ name = "order.created publish" }"#);

    let first = store.search_traces(
        &filter,
        &PageRequest {
            limit: 1,
            cursor: None,
        },
    );
    assert_eq!(
        first.items.iter().map(|s| s.trace_id).collect::<Vec<_>>(),
        vec![b_id]
    );
    assert!(
        first.next_cursor.is_some(),
        "one more trace matches, behind the one the query rejects"
    );

    let second = store.search_traces(
        &filter,
        &PageRequest {
            limit: 1,
            cursor: first.next_cursor,
        },
    );
    assert_eq!(
        second.items.iter().map(|s| s.trace_id).collect::<Vec<_>>(),
        vec![a_id]
    );
    assert_eq!(second.next_cursor, None);
}

#[test]
fn logql_query_filters_logs() {
    let Mesh { store, .. } = mesh();

    let page = store.search_logs(
        &log_filter(r#"{service_name="payments-api"} | level = "error""#),
        &all(),
    );

    // payments-api logs INF, WRN, ERR, ERR in the failing checkout.
    assert_eq!(page.items.len(), 2);
    assert!(page.items.iter().all(|log| log.service() == "payments-api"));
    assert!(page.items.iter().all(|log| log.severity == Severity::Error));
    assert_eq!(page.next_cursor, None);
}

#[test]
fn logql_query_corners() {
    let Mesh { store, .. } = mesh();
    let count = |filter: &LogFilter| store.search_logs(filter, &all()).items.len();

    // orders-api logs 3 lines in the failing checkout and 4 in the successful
    // one. An exact service selector is the one the store uses to skip services.
    assert_eq!(count(&log_filter(r#"{service_name="orders-api"}"#)), 3 + 4);
    // A regex selector has no such hint, and must give the same answer.
    assert_eq!(count(&log_filter(r#"{service_name=~"orders-.*"}"#)), 3 + 4);
    // A line filter, unanchored.
    assert_eq!(
        count(&log_filter(
            r#"{service_name="payments-api"} |= "retrying""#
        )),
        1
    );
    // Nothing matches a service that does not exist.
    assert_eq!(count(&log_filter(r#"{service_name="nope"}"#)), 0);
    // A negated matcher gives no hint, and is still correct: both checkouts
    // log "Charging card" from payments-api.
    assert_eq!(
        count(&log_filter(r#"{service_name!="orders-api"} |= "Charging""#)),
        2
    );
}

#[test]
fn logql_query_combines_with_other_conditions() {
    let Mesh { store, failure, .. } = mesh();
    // payments-api logs 4 lines in the failing checkout and 2 in the other.
    let query = log_filter(r#"{service_name="payments-api"}"#);
    let count = |filter: &LogFilter| store.search_logs(filter, &all()).items.len();
    assert_eq!(count(&query), 6);

    let mut errors = query.clone();
    errors.min_severity = Some(Severity::Error);
    assert_eq!(count(&errors), 2);

    let mut in_trace = query.clone();
    in_trace.trace_id = Some(failure[0].trace_id);
    assert_eq!(count(&in_trace), 4);

    let mut contains = query.clone();
    contains.contains = Some("CHARGE".to_owned());
    assert_eq!(count(&contains), 2, "'Charge failed' and 'Charge accepted'");

    let mut other_service = query.clone();
    other_service.service = Some("orders-api".to_owned());
    assert_eq!(count(&other_service), 0, "the two conditions contradict");

    // Paging works with a query.
    let first = store.search_logs(
        &query,
        &PageRequest {
            limit: 3,
            cursor: None,
        },
    );
    assert_eq!(first.items.len(), 3);
    let second = store.search_logs(
        &query,
        &PageRequest {
            limit: 3,
            cursor: first.next_cursor,
        },
    );
    assert_eq!(second.items.len(), 3);
    assert_eq!(second.next_cursor, None);
}

#[test]
fn matched_span_ids_capped_at_20() {
    let (store, _clock) = store_with_manual_clock();
    let spans = scenario_spans(&Scenario::HugeTrace { spans: 500 }, 1, FAILURE_BASE);
    assert_eq!(spans.len(), 500);
    store.ingest_spans(spans.clone());

    let page = store.search_traces(&trace_filter("{ }"), &all());

    assert_eq!(page.items.len(), 1);
    let summary = &page.items[0];
    assert_eq!(summary.span_count, 500);
    assert_eq!(summary.matched_span_ids.len(), 20);
    // The first 20 matched spans in evaluate order: the first 20 held.
    let first_20: Vec<SpanId> = spans.iter().take(20).map(|span| span.span_id).collect();
    assert_eq!(summary.matched_span_ids, first_20);
}

#[test]
fn matched_span_ids_cap_is_exact() {
    // 20 matches give 20, and 19 give 19: the cap is not off by one.
    let (store, _clock) = store_with_manual_clock();
    for (trace, count) in [(1u8, 19u8), (2, 20), (3, 21)] {
        let mut spans = vec![span(trace, 100, None, "svc", "root", 0, 10)];
        for i in 0..count {
            spans.push(span(trace, i + 1, Some(100), "svc", "leaf", 1, 1));
        }
        store.ingest_spans(spans);
    }

    let page = store.search_traces(&trace_filter(r#"{ name = "leaf" }"#), &all());

    let matched = |trace: u8| -> usize {
        page.items
            .iter()
            .find(|summary| summary.trace_id == tid(trace))
            .unwrap()
            .matched_span_ids
            .len()
    };
    assert_eq!(matched(1), 19);
    assert_eq!(matched(2), 20);
    assert_eq!(matched(3), 20);
}

#[test]
fn matched_span_ids_serialize_only_when_present() {
    let (store, _clock) = store_with_manual_clock();
    store.ingest_spans(vec![
        span(1, 1, None, "web", "root", 0, 10),
        span(1, 2, Some(1), "web", "child", 1, 2),
    ]);

    // No query: the JSON of a summary is what it was before queries existed.
    let plain = serde_json::to_value(store.summary(tid(1)).unwrap()).unwrap();
    assert!(plain.get("matched_span_ids").is_none());

    // With a query: the matched IDs, as 16 lowercase hex characters each.
    let page = store.search_traces(&trace_filter(r#"{ name = "child" }"#), &all());
    let json = serde_json::to_value(&page.items[0]).unwrap();
    assert_eq!(
        json["matched_span_ids"],
        serde_json::json!([sid(2).to_string()])
    );
    assert_eq!(sid(2).to_string(), "0202020202020202");
}

#[test]
fn filters_with_queries_compare_by_query() {
    // The filters are `PartialEq` (not `Eq`: a query holds floats).
    let a = trace_filter("{ status = error }");
    assert_eq!(a, a.clone());
    assert_eq!(a, trace_filter("{ status = error }"));
    assert_ne!(a, trace_filter("{ status = ok }"));
    assert_ne!(a, TraceFilter::default());

    let l = log_filter(r#"{service_name="x"}"#);
    assert_eq!(l, l.clone());
    assert_eq!(l, log_filter(r#"{service_name="x"}"#));
    assert_ne!(l, log_filter(r#"{service_name="y"}"#));
    assert_ne!(l, LogFilter::default());
}
