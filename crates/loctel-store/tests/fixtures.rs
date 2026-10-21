//! Shared builders for the store's integration tests.
//!
//! Every test file does `mod fixtures;` and uses the subset it needs, so
//! unused items are expected here. (Cargo also compiles this file as a test
//! binary of its own, with no tests in it.)
#![allow(dead_code)]

use std::sync::Arc;

use loctel_model::{AttrValue, Resource, Span, SpanEvent, SpanId, SpanKind, StatusCode, TraceId};
use loctel_store::{ManualClock, Store, StoreConfig};

/// Where the manual clock starts: well away from 0 so that "now minus
/// something" never underflows.
pub const CLOCK_START: u64 = 1_000_000_000_000;

/// A trace ID whose 16 bytes all equal `n`.
pub fn tid(n: u8) -> TraceId {
    TraceId([n; 16])
}

/// A span ID whose 8 bytes all equal `n`.
pub fn sid(n: u8) -> SpanId {
    SpanId([n; 8])
}

/// A span of trace `trace` with ID `id`, under `parent` (`None` for a
/// root). Times are in milliseconds and stored as nanoseconds. The span is
/// `Internal`, status `Unset`, with no attributes or events.
pub fn span(
    trace: u8,
    id: u8,
    parent: Option<u8>,
    service: &str,
    name: &str,
    start_ms: u64,
    dur_ms: u64,
) -> Span {
    Span {
        trace_id: tid(trace),
        span_id: sid(id),
        parent_span_id: parent.map(sid),
        name: name.to_string(),
        kind: SpanKind::Internal,
        start_unix_nano: start_ms * 1_000_000,
        end_unix_nano: (start_ms + dur_ms) * 1_000_000,
        status: StatusCode::Unset,
        status_message: String::new(),
        attributes: vec![],
        events: vec![],
        resource: Arc::new(Resource {
            attributes: vec![(
                "service.name".to_string(),
                AttrValue::Str(service.to_string()),
            )],
        }),
        scope_name: "test".to_string(),
    }
}

/// The same span with status `Error`.
pub fn error(mut span: Span) -> Span {
    span.status = StatusCode::Error;
    span
}

/// The same span with kind `kind`.
pub fn with_kind(mut span: Span, kind: SpanKind) -> Span {
    span.kind = kind;
    span
}

/// The same span with one more string attribute.
pub fn with_attr(mut span: Span, key: &str, value: &str) -> Span {
    span.attributes
        .push((key.to_string(), AttrValue::Str(value.to_string())));
    span
}

/// The same span with an `exception` event carrying the three
/// `exception.*` attributes. The span's status is left alone: an exception
/// event alone does not make a span erroring.
pub fn with_exception(mut span: Span, type_name: &str, message: &str) -> Span {
    span.events.push(SpanEvent {
        time_unix_nano: span.start_unix_nano,
        name: "exception".to_string(),
        attributes: vec![
            (
                "exception.type".to_string(),
                AttrValue::Str(type_name.to_string()),
            ),
            (
                "exception.message".to_string(),
                AttrValue::Str(message.to_string()),
            ),
            (
                "exception.stacktrace".to_string(),
                AttrValue::Str(format!("at {}", span.name)),
            ),
        ],
    });
    span
}

/// A store with the default configuration (10 s quiet period) on a manual
/// clock that starts at [`CLOCK_START`]. The clock is returned so tests can
/// advance it.
pub fn store_with_manual_clock() -> (Store, Arc<ManualClock>) {
    store_with_config(StoreConfig::default())
}

/// A store with `config` on a manual clock that starts at [`CLOCK_START`].
pub fn store_with_config(config: StoreConfig) -> (Store, Arc<ManualClock>) {
    let clock = Arc::new(ManualClock::new(CLOCK_START));
    (Store::new(config, clock.clone()), clock)
}
