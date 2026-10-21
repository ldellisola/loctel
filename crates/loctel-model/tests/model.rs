//! Behaviour of the domain model (plan Task 2).
//!
//! The review-focus items owned by this task are `duration_clamps_negative`
//! (clock skew: no panic, duration clamped to 0) and
//! `service_name_defaults_to_unknown_service` (a resource without
//! `service.name` is shown as `unknown_service`, never dropped).

use std::sync::Arc;

use loctel_model::{
    AttrValue, Attributes, LogRecord, Resource, Severity, Span, SpanEvent, SpanId, SpanKind,
    StatusCode, TraceId, loki_label_name,
};

fn str_attr(key: &str, value: &str) -> (String, AttrValue) {
    (key.to_string(), AttrValue::Str(value.to_string()))
}

fn span_fixture() -> Span {
    Span {
        trace_id: TraceId::from_hex("4bf92f3577b34da6a3ce929d0e0e4736").unwrap(),
        span_id: SpanId::from_hex("00f067aa0ba902b7").unwrap(),
        parent_span_id: None,
        name: "GET /checkout".to_string(),
        kind: SpanKind::Server,
        start_unix_nano: 1_000,
        end_unix_nano: 2_000,
        status: StatusCode::Unset,
        status_message: String::new(),
        attributes: vec![],
        events: vec![],
        resource: Arc::new(Resource {
            attributes: vec![str_attr("service.name", "checkout")],
        }),
        scope_name: "test".to_string(),
    }
}

fn log_fixture(attributes: Attributes) -> LogRecord {
    LogRecord {
        time_unix_nano: 1_000,
        observed_unix_nano: 1_001,
        severity: Severity::Error,
        severity_text: "ERROR".to_string(),
        body: "payment declined".to_string(),
        attributes,
        trace_id: None,
        span_id: None,
        resource: Arc::new(Resource {
            attributes: vec![str_attr("service.name", "payments")],
        }),
        scope_name: "test".to_string(),
    }
}

#[test]
fn trace_id_hex_roundtrip() {
    let id = TraceId::from_hex("4bf92f3577b34da6a3ce929d0e0e4736").unwrap();
    assert_eq!(id.to_string(), "4bf92f3577b34da6a3ce929d0e0e4736");
    assert!(TraceId::from_hex("abc").is_err());
    assert!(!TraceId::from_bytes(&[0u8; 16]).unwrap().is_valid());
}

#[test]
fn service_name_defaults_to_unknown_service() {
    assert_eq!(
        Resource { attributes: vec![] }.service_name(),
        "unknown_service"
    );
}

#[test]
fn duration_clamps_negative() {
    let mut s = span_fixture();
    s.start_unix_nano = 2_000;
    s.end_unix_nano = 1_000;
    assert_eq!(s.duration_nanos(), 0);
}

#[test]
fn span_exception_from_event() {
    let mut s = span_fixture();
    s.events = vec![SpanEvent {
        time_unix_nano: 1_500,
        name: "exception".to_string(),
        attributes: vec![
            str_attr("exception.type", "java.lang.IllegalStateException"),
            str_attr("exception.message", "card expired"),
            str_attr(
                "exception.stacktrace",
                "at Payments.charge(Payments.java:42)",
            ),
        ],
    }];
    let info = s.exception().expect("exception event yields ExceptionInfo");
    assert_eq!(info.type_name, "java.lang.IllegalStateException");
    assert_eq!(info.message, "card expired");
    assert_eq!(
        info.stacktrace.as_deref(),
        Some("at Payments.charge(Payments.java:42)")
    );
}

#[test]
fn exception_event_alone_is_not_error() {
    let mut s = span_fixture();
    s.status = StatusCode::Unset;
    s.events = vec![SpanEvent {
        time_unix_nano: 1_500,
        name: "exception".to_string(),
        attributes: vec![
            str_attr("exception.type", "TimeoutError"),
            str_attr("exception.message", "retrying"),
        ],
    }];
    assert!(!s.is_error());
    assert!(s.exception().is_some());
}

#[test]
fn log_exception_from_attributes() {
    let log = log_fixture(vec![
        str_attr("exception.type", "ValueError"),
        str_attr("exception.message", "bad amount"),
    ]);
    let info = log.exception().expect("exception.* attributes yield info");
    assert_eq!(info.type_name, "ValueError");
    assert_eq!(info.message, "bad amount");
    assert_eq!(info.stacktrace, None);
}

#[test]
fn severity_mapping() {
    assert_eq!(Severity::from_number(9), Severity::Info);
    assert_eq!(Severity::from_number(17).level_label(), "error");
    assert_eq!(Severity::from_number(0).level_label(), "unknown");
}

#[test]
fn loki_label_names() {
    assert_eq!(loki_label_name("service.name"), "service_name");
}

#[test]
fn attr_display_strings() {
    assert_eq!(AttrValue::Int(42).to_display_string(), "42");
    assert_eq!(
        AttrValue::Array(vec![AttrValue::Int(1), AttrValue::Str("a".to_string())])
            .to_display_string(),
        "[1,\"a\"]"
    );
}
