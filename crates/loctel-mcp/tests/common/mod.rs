//! Shared by `tools.rs` and `http.rs`: a store fed from the fake mesh, a fake
//! status source and a few hand-built spans and logs for the cases the mesh
//! scenarios do not cover.
//!
//! Each test crate uses a different part of this, so unused items are fine.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use loctel_ingest::{ListenerState, logs_from_request, spans_from_request};
use loctel_mcp::{ListenerDto, StatusDto, StatusSource};
use loctel_meshgen::{Scenario, generate};
use loctel_model::{
    AttrValue, LogRecord, Resource, Severity, Span, SpanEvent, SpanId, SpanKind, StatusCode,
    TraceId,
};
use loctel_store::{ManualClock, Store, StoreConfig};

/// 2027-01-15T08:00:00Z. Scenario data is generated at this base unless a test
/// says otherwise, and the manual clock starts here.
pub const BASE: u64 = 1_800_000_000_000_000_000;
pub const MS: u64 = 1_000_000;
pub const SECOND: u64 = 1_000 * MS;

/// A store on a manual clock reading [`BASE`], with the default config (10 s
/// to complete).
pub fn manual_store() -> (Store, Arc<ManualClock>) {
    let clock = Arc::new(ManualClock::new(BASE));
    let store = Store::new(StoreConfig::default(), clock.clone());
    (store, clock)
}

/// Generates `scenario` and feeds it to the store through the real converter,
/// the way the receivers do. Returns the scenario's trace IDs in the order
/// they were made.
pub fn load(store: &Store, scenario: &Scenario, seed: u64, base: u64) -> Vec<TraceId> {
    let generated = generate(scenario, seed, base);
    for batch in &generated.batches {
        if let Some(traces) = &batch.traces {
            let converted = spans_from_request(traces.clone());
            assert_eq!(converted.rejected, 0, "generated spans are valid");
            store.ingest_spans(converted.items);
        }
        if let Some(logs) = &batch.logs {
            let converted = logs_from_request(logs.clone());
            assert_eq!(converted.rejected, 0, "generated logs are valid");
            store.ingest_logs(converted.items);
        }
    }
    generated.trace_ids
}

/// Moves the manual clock past the 10 s quiet period, so every trace loaded
/// so far is complete.
pub fn settle(clock: &ManualClock) {
    clock.advance(Duration::from_secs(11));
}

/// A status source that always says the same thing.
pub struct FakeStatus(pub StatusDto);

impl StatusSource for FakeStatus {
    fn status(&self) -> StatusDto {
        self.0.clone()
    }
}

pub fn sample_status() -> StatusDto {
    StatusDto {
        paused: true,
        listeners: vec![
            ListenerDto {
                name: "otlp-grpc".to_owned(),
                address: "127.0.0.1:4317".to_owned(),
                state: "listening".to_owned(),
                error: None,
            },
            ListenerDto {
                name: "otlp-http".to_owned(),
                address: "127.0.0.1:4318".to_owned(),
                state: "failed".to_owned(),
                error: Some("Address already in use (os error 98)".to_owned()),
            },
        ],
        spans_per_sec: 12.5,
        logs_per_sec: 3.0,
        traces_held: 4,
        spans_held: 60,
        logs_held: 28,
        rules: Vec::new(),
        rules_file_error: None,
        dropped_fraction: 0.0,
        disk_bytes: 0,
        disk_cap_bytes: 4 * 1024 * 1024 * 1024,
        spill_error: None,
    }
}

pub fn fake_status() -> Arc<dyn StatusSource> {
    Arc::new(FakeStatus(sample_status()))
}

/// The address a listener is bound to.
pub fn bound_address(state: &ListenerState) -> std::net::SocketAddr {
    match state {
        ListenerState::Listening(address) => *address,
        ListenerState::Failed(reason) => panic!("listener failed: {reason}"),
    }
}

// --- hand-built telemetry -----------------------------------------------------

pub fn trace_id(n: u8) -> TraceId {
    let mut bytes = [0u8; 16];
    bytes[15] = n;
    TraceId(bytes)
}

pub fn span_id(n: u8) -> SpanId {
    let mut bytes = [0u8; 8];
    bytes[7] = n;
    SpanId(bytes)
}

pub fn resource(service: &str) -> Arc<Resource> {
    Arc::new(Resource {
        attributes: vec![(
            "service.name".to_owned(),
            AttrValue::Str(service.to_owned()),
        )],
    })
}

/// A span of trace `trace`, starting `start_ms` after [`BASE`] and lasting
/// `duration_ms` (`at` is `(start_ms, duration_ms)`), with status unset.
pub fn span(
    trace: u8,
    id: u8,
    parent: Option<u8>,
    service: &str,
    name: &str,
    kind: SpanKind,
    at: (u64, u64),
) -> Span {
    let (start_ms, duration_ms) = at;
    Span {
        trace_id: trace_id(trace),
        span_id: span_id(id),
        parent_span_id: parent.map(span_id),
        name: name.to_owned(),
        kind,
        start_unix_nano: BASE + start_ms * MS,
        end_unix_nano: BASE + (start_ms + duration_ms) * MS,
        status: StatusCode::Unset,
        status_message: String::new(),
        attributes: Vec::new(),
        events: Vec::new(),
        resource: resource(service),
        scope_name: "test".to_owned(),
    }
}

pub fn erroring(mut span: Span, message: &str) -> Span {
    span.status = StatusCode::Error;
    span.status_message = message.to_owned();
    span
}

/// An `exception` event on the span, with the three `exception.*` attributes.
pub fn with_exception(mut span: Span, kind: &str, message: &str, stacktrace: &str) -> Span {
    span.events.push(SpanEvent {
        time_unix_nano: span.start_unix_nano + MS,
        name: "exception".to_owned(),
        attributes: vec![
            ("exception.type".to_owned(), AttrValue::Str(kind.to_owned())),
            (
                "exception.message".to_owned(),
                AttrValue::Str(message.to_owned()),
            ),
            (
                "exception.stacktrace".to_owned(),
                AttrValue::Str(stacktrace.to_owned()),
            ),
        ],
    });
    span
}

/// A log of service `service`, `at_ms` after [`BASE`], optionally on a span.
pub fn log(
    service: &str,
    at_ms: u64,
    severity: Severity,
    body: &str,
    on: Option<(u8, u8)>,
) -> LogRecord {
    LogRecord {
        time_unix_nano: BASE + at_ms * MS,
        observed_unix_nano: BASE + at_ms * MS,
        severity,
        severity_text: String::new(),
        body: body.to_owned(),
        attributes: Vec::new(),
        trace_id: on.map(|(trace, _)| trace_id(trace)),
        span_id: on.map(|(_, span)| span_id(span)),
        resource: resource(service),
        scope_name: "test".to_owned(),
    }
}
