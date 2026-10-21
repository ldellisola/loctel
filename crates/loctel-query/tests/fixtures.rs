//! Shared builders for the query crate's integration tests.
//!
//! Every test file that needs them does `mod fixtures;` and uses the subset it
//! wants, so unused items are expected here. (Cargo also compiles this file as
//! a test binary of its own, with no tests in it.)
#![allow(dead_code)]

use std::collections::BTreeSet;
use std::sync::Arc;

use loctel_ingest::{logs_from_request, spans_from_request};
use loctel_meshgen::{Scenario, generate};
use loctel_model::{
    AttrValue, LogRecord, Resource, Severity, Span, SpanId, SpanKind, StatusCode, TraceId,
};
use loctel_query::traceql::{self, TraceView};

/// The base time the fixtures generate at (1 s after the epoch, in
/// nanoseconds), so a span's start offset in milliseconds is easy to read back.
pub const BASE_UNIX_NANO: u64 = 1_000_000_000;

/// The seed the fixtures generate with.
pub const SEED: u64 = 1;

/// The spans of every trace export in `scenario`, converted by the real OTLP
/// conversion code, in the order the batches carry them.
pub fn scenario_spans(scenario: &Scenario) -> Vec<Span> {
    generate(scenario, SEED, BASE_UNIX_NANO)
        .batches
        .into_iter()
        .filter_map(|batch| batch.traces)
        .flat_map(|request| spans_from_request(request).items)
        .collect()
}

/// The 15 spans of the fixed `CheckoutFailure` trace (the table in Task 8 of
/// the M1 plan; `number` maps a span back to its row).
pub fn checkout_failure() -> Vec<Span> {
    scenario_spans(&Scenario::CheckoutFailure)
}

/// The indices (into `spans`) that `query` matches in the trace made of
/// `spans`, as `evaluate` gives them. Panics if the query does not parse.
pub fn matched_indices(query: &str, spans: &[Span]) -> Vec<usize> {
    let parsed = traceql::parse(query).unwrap_or_else(|err| panic!("{query}: {err}"));
    parsed.evaluate(&TraceView::new(spans))
}

/// The `(service, name)` pair of each span `query` matches.
pub fn matched(query: &str, spans: &[Span]) -> BTreeSet<(String, String)> {
    matched_indices(query, spans)
        .into_iter()
        .map(|i| (spans[i].service().to_owned(), spans[i].name.clone()))
        .collect()
}

/// The row numbers (1 to 15, as in the plan's table) of the spans `query`
/// matches in the `CheckoutFailure` trace.
pub fn matched_numbers(query: &str, spans: &[Span]) -> BTreeSet<usize> {
    matched_indices(query, spans)
        .into_iter()
        .map(|i| number(&spans[i]))
        .collect()
}

/// The row number (1 to 15) of a `CheckoutFailure` span in the plan's table,
/// found by its start offset (the 15 offsets are all different). Panics for a
/// span that is not from that trace.
pub fn number(span: &Span) -> usize {
    let offset_ms = (span.start_unix_nano - BASE_UNIX_NANO) / 1_000_000;
    match offset_ms {
        0 => 1,
        4 => 2,
        30 => 3,
        34 => 4,
        40 => 5,
        62 => 6,
        66 => 7,
        70 => 8,
        196 => 9,
        200 => 10,
        206 => 11,
        236 => 12,
        840 => 13,
        1794 => 14,
        1802 => 15,
        other => panic!("no CheckoutFailure span starts at {other} ms"),
    }
}

/// A span ID whose 8 bytes all equal `n`.
pub fn sid(n: u8) -> SpanId {
    SpanId([n; 8])
}

/// A hand-made span of one fixed trace: ID `id`, under `parent` (`None` for
/// no parent), starting at `start_ms` and lasting `dur_ms`. It is `Internal`,
/// status `Unset`, with no attributes and an unnamed service.
pub fn mk(id: u8, parent: Option<u8>, name: &str, start_ms: u64, dur_ms: u64) -> Span {
    Span {
        trace_id: TraceId([7; 16]),
        span_id: sid(id),
        parent_span_id: parent.map(sid),
        name: name.to_owned(),
        kind: SpanKind::Internal,
        start_unix_nano: BASE_UNIX_NANO + start_ms * 1_000_000,
        end_unix_nano: BASE_UNIX_NANO + (start_ms + dur_ms) * 1_000_000,
        status: StatusCode::Unset,
        status_message: String::new(),
        attributes: Vec::new(),
        events: Vec::new(),
        resource: Arc::new(Resource::default()),
        scope_name: String::new(),
    }
}

/// `span` with one more span attribute.
pub fn with_attr(mut span: Span, key: &str, value: AttrValue) -> Span {
    span.attributes.push((key.to_owned(), value));
    span
}

/// `span` with one more attribute on its resource.
pub fn with_resource_attr(mut span: Span, key: &str, value: AttrValue) -> Span {
    let mut resource = (*span.resource).clone();
    resource.attributes.push((key.to_owned(), value));
    span.resource = Arc::new(resource);
    span
}

/// `span` emitted by `service`.
pub fn in_service(span: Span, service: &str) -> Span {
    with_resource_attr(span, "service.name", AttrValue::Str(service.to_owned()))
}

// ---- logs ------------------------------------------------------------------

/// The log records of every logs export in `scenario`, converted by the real
/// OTLP conversion code, in the order the batches carry them.
pub fn scenario_logs(scenario: &Scenario) -> Vec<LogRecord> {
    generate(scenario, SEED, BASE_UNIX_NANO)
        .batches
        .into_iter()
        .filter_map(|batch| batch.logs)
        .flat_map(|request| logs_from_request(request).items)
        .collect()
}

/// The 7 log records of the fixed `CheckoutFailure` scenario (the log table in
/// Task 8 of the M1 plan), in time order: 3 from `orders-api` (on span 4), 4
/// from `payments-api` (on span 10).
pub fn checkout_failure_logs() -> Vec<LogRecord> {
    let mut logs = scenario_logs(&Scenario::CheckoutFailure);
    logs.sort_by_key(|log| log.time_unix_nano);
    logs
}

/// A hand-made `Info` log with `body`: no attributes, no trace context, an
/// unnamed service and an empty scope name.
pub fn mk_log(body: &str) -> LogRecord {
    LogRecord {
        time_unix_nano: BASE_UNIX_NANO,
        observed_unix_nano: BASE_UNIX_NANO,
        severity: Severity::Info,
        severity_text: "INFO".to_owned(),
        body: body.to_owned(),
        attributes: Vec::new(),
        trace_id: None,
        span_id: None,
        resource: Arc::new(Resource::default()),
        scope_name: String::new(),
    }
}

/// `log` with one more log attribute.
pub fn log_with_attr(mut log: LogRecord, key: &str, value: AttrValue) -> LogRecord {
    log.attributes.push((key.to_owned(), value));
    log
}

/// `log` with one more attribute on its resource.
pub fn log_with_resource_attr(mut log: LogRecord, key: &str, value: AttrValue) -> LogRecord {
    let mut resource = (*log.resource).clone();
    resource.attributes.push((key.to_owned(), value));
    log.resource = Arc::new(resource);
    log
}

/// `log` emitted by `service`.
pub fn log_in_service(log: LogRecord, service: &str) -> LogRecord {
    log_with_resource_attr(log, "service.name", AttrValue::Str(service.to_owned()))
}

/// How many of `logs` the LogQL `query` matches. Panics if it does not parse.
pub fn count_logs(query: &str, logs: &[LogRecord]) -> usize {
    let parsed = loctel_query::logql::parse(query).unwrap_or_else(|err| panic!("{query}: {err}"));
    logs.iter().filter(|log| parsed.matches(log)).count()
}
