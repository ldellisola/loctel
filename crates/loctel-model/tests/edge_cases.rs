//! Edge cases around the plan's Task 2 interfaces. The plan's own nine tests
//! live in `model.rs`; these pin the behaviour the later crates rely on.

use std::collections::HashSet;
use std::sync::Arc;

use loctel_model::{
    AttrValue, Attributes, IdError, LogRecord, Resource, Severity, Span, SpanEvent, SpanId,
    SpanKind, StatusCode, TraceId, attr, loki_label_name,
};
use serde_json::json;

fn s(key: &str, value: &str) -> (String, AttrValue) {
    (key.to_string(), AttrValue::Str(value.to_string()))
}

fn resource(service: &str) -> Arc<Resource> {
    Arc::new(Resource {
        attributes: vec![s("service.name", service)],
    })
}

fn span(attributes: Attributes, events: Vec<SpanEvent>) -> Span {
    Span {
        trace_id: TraceId([1; 16]),
        span_id: SpanId([2; 8]),
        parent_span_id: Some(SpanId([3; 8])),
        name: "op".to_string(),
        kind: SpanKind::Client,
        start_unix_nano: 10,
        end_unix_nano: 20,
        status: StatusCode::Ok,
        status_message: String::new(),
        attributes,
        events,
        resource: resource("svc"),
        scope_name: "scope".to_string(),
    }
}

fn log(body: &str) -> LogRecord {
    LogRecord {
        time_unix_nano: 100,
        observed_unix_nano: 101,
        severity: Severity::Info,
        severity_text: "INFO".to_string(),
        body: body.to_string(),
        attributes: vec![],
        trace_id: Some(TraceId([1; 16])),
        span_id: Some(SpanId([2; 8])),
        resource: resource("svc"),
        scope_name: "scope".to_string(),
    }
}

fn event(name: &str, attributes: Attributes) -> SpanEvent {
    SpanEvent {
        time_unix_nano: 15,
        name: name.to_string(),
        attributes,
    }
}

#[test]
fn span_id_hex_roundtrip_and_validity() {
    let id = SpanId::from_hex("00f067aa0ba902b7").unwrap();
    assert_eq!(id.to_string(), "00f067aa0ba902b7");
    assert!(id.is_valid());
    assert!(!SpanId::from_hex("0000000000000000").unwrap().is_valid());
}

#[test]
fn malformed_ids_are_errors_not_panics() {
    // Review focus 2: a 7-byte span ID, and friends.
    assert_eq!(
        SpanId::from_bytes(&[1; 7]),
        Err(IdError::WrongByteLength {
            expected: 8,
            actual: 7
        })
    );
    assert_eq!(
        TraceId::from_bytes(&[]),
        Err(IdError::WrongByteLength {
            expected: 16,
            actual: 0
        })
    );
    assert_eq!(
        SpanId::from_hex("00f067aa0ba902b"),
        Err(IdError::WrongHexLength {
            expected: 16,
            actual: 15
        })
    );
    assert_eq!(
        SpanId::from_hex("00f067aa0ba902bz"),
        Err(IdError::InvalidHexDigit)
    );
    // Same byte length as a valid id, but multi-byte characters.
    assert_eq!(
        SpanId::from_hex("00f067aa0ba902é"),
        Err(IdError::InvalidHexDigit)
    );
    assert!(!IdError::InvalidHexDigit.to_string().is_empty());
}

#[test]
fn hex_parsing_accepts_either_case_and_displays_lowercase() {
    let id = TraceId::from_hex("4BF92F3577B34DA6A3CE929D0E0E4736").unwrap();
    assert_eq!(id.to_string(), "4bf92f3577b34da6a3ce929d0e0e4736");
    assert_eq!(id, TraceId::from_hex(&id.to_string()).unwrap());
}

#[test]
fn ids_are_hashable_ordered_and_copy() {
    let low = SpanId([0, 0, 0, 0, 0, 0, 0, 1]);
    let high = SpanId([0, 0, 0, 0, 0, 0, 0, 2]);
    assert!(low < high);
    let copy = low;
    let set: HashSet<SpanId> = [low, copy, high].into_iter().collect();
    assert_eq!(set.len(), 2);
}

#[test]
fn ids_serialize_as_hex_strings() {
    assert_eq!(
        serde_json::to_value(TraceId([0xab; 16])).unwrap(),
        json!("abababababababababababababababab")
    );
    assert_eq!(
        serde_json::to_value(SpanId([0x01; 8])).unwrap(),
        json!("0101010101010101")
    );
}

#[test]
fn attr_finds_the_first_match() {
    let attrs: Attributes = vec![s("k", "first"), s("k", "second"), s("other", "x")];
    assert_eq!(
        attr(&attrs, "k"),
        Some(&AttrValue::Str("first".to_string()))
    );
    assert_eq!(attr(&attrs, "missing"), None);
}

#[test]
fn attr_display_covers_every_variant() {
    assert_eq!(AttrValue::Str("hi".to_string()).to_display_string(), "hi");
    assert_eq!(AttrValue::Bool(true).to_display_string(), "true");
    assert_eq!(AttrValue::Int(-7).to_display_string(), "-7");
    assert_eq!(AttrValue::Double(1.5).to_display_string(), "1.5");
    assert_eq!(AttrValue::Double(2.0).to_display_string(), "2");
    assert_eq!(
        AttrValue::Bytes(vec![0x00, 0xff, 0x10]).to_display_string(),
        "00ff10"
    );
    assert_eq!(
        AttrValue::Map(vec![
            ("a".to_string(), AttrValue::Int(1)),
            (
                "b".to_string(),
                AttrValue::Array(vec![AttrValue::Bool(false)])
            ),
        ])
        .to_display_string(),
        r#"{"a":1,"b":[false]}"#
    );
    assert_eq!(AttrValue::Array(vec![]).to_display_string(), "[]");
}

#[test]
fn service_name_ignores_empty_and_non_string_values() {
    let named = |value: AttrValue| Resource {
        attributes: vec![("service.name".to_string(), value)],
    };
    assert_eq!(
        named(AttrValue::Str("cart".to_string())).service_name(),
        "cart"
    );
    assert_eq!(
        named(AttrValue::Str(String::new())).service_name(),
        "unknown_service"
    );
    assert_eq!(named(AttrValue::Int(5)).service_name(), "unknown_service");
}

#[test]
fn span_and_log_service_come_from_the_resource() {
    assert_eq!(span(vec![], vec![]).service(), "svc");
    assert_eq!(log("x").service(), "svc");
    let mut unnamed = span(vec![], vec![]);
    unnamed.resource = Arc::new(Resource::default());
    assert_eq!(unnamed.service(), "unknown_service");
}

#[test]
fn error_status_is_the_only_error() {
    let mut sp = span(vec![], vec![]);
    for (status, erroring) in [
        (StatusCode::Unset, false),
        (StatusCode::Ok, false),
        (StatusCode::Error, true),
    ] {
        sp.status = status;
        assert_eq!(sp.is_error(), erroring, "{status:?}");
    }
}

#[test]
fn duration_is_end_minus_start() {
    let mut sp = span(vec![], vec![]);
    sp.start_unix_nano = 1_000;
    sp.end_unix_nano = 4_500;
    assert_eq!(sp.duration_nanos(), 3_500);
    sp.end_unix_nano = sp.start_unix_nano;
    assert_eq!(sp.duration_nanos(), 0);
    sp.start_unix_nano = u64::MAX;
    sp.end_unix_nano = 0;
    assert_eq!(sp.duration_nanos(), 0);
}

#[test]
fn span_exception_uses_the_first_exception_event_only() {
    let sp = span(
        vec![s("exception.type", "ignored: span attributes do not count")],
        vec![
            event("retry", vec![s("exception.type", "ignored: wrong event")]),
            event("exception", vec![s("exception.type", "First")]),
            event("exception", vec![s("exception.type", "Second")]),
        ],
    );
    let info = sp.exception().unwrap();
    assert_eq!(info.type_name, "First");
    assert_eq!(info.message, "");
    assert_eq!(info.stacktrace, None);
}

#[test]
fn span_exception_is_none_without_an_exception_event() {
    let sp = span(vec![], vec![event("retry", vec![])]);
    assert_eq!(sp.exception(), None);
}

#[test]
fn bare_exception_event_still_counts_as_an_exception() {
    // The marker is worth showing even when the instrumentation sent no
    // attributes.
    let sp = span(vec![], vec![event("exception", vec![])]);
    let info = sp.exception().unwrap();
    assert_eq!(info.type_name, "");
    assert_eq!(info.message, "");
}

#[test]
fn log_exception_needs_an_exception_attribute() {
    let mut record = log("boom");
    assert_eq!(record.exception(), None);
    record.attributes = vec![s("exception.escaped", "true"), s("other", "x")];
    assert_eq!(record.exception(), None);
    record.attributes = vec![s("exception.stacktrace", "at main")];
    let info = record.exception().unwrap();
    assert_eq!(info.stacktrace.as_deref(), Some("at main"));
    assert_eq!(info.type_name, "");
}

#[test]
fn exception_attribute_values_of_other_types_are_rendered() {
    let mut record = log("boom");
    record.attributes = vec![
        s("exception.type", "HttpError"),
        ("exception.message".to_string(), AttrValue::Int(503)),
    ];
    assert_eq!(record.exception().unwrap().message, "503");
}

#[test]
fn peer_prefers_server_address_then_peer_service() {
    let both = span(
        vec![
            s("peer.service", "legacy"),
            s("server.address", "db.internal"),
        ],
        vec![],
    );
    assert_eq!(both.peer(), Some("db.internal"));
    let only_peer = span(vec![s("peer.service", "legacy")], vec![]);
    assert_eq!(only_peer.peer(), Some("legacy"));
    assert_eq!(span(vec![], vec![]).peer(), None);
    let empty_address = span(
        vec![s("server.address", ""), s("peer.service", "legacy")],
        vec![],
    );
    assert_eq!(empty_address.peer(), Some("legacy"));
}

#[test]
fn severity_number_boundaries() {
    let expected = [
        (-1, Severity::Unspecified),
        (0, Severity::Unspecified),
        (1, Severity::Trace),
        (4, Severity::Trace),
        (5, Severity::Debug),
        (8, Severity::Debug),
        (9, Severity::Info),
        (12, Severity::Info),
        (13, Severity::Warn),
        (16, Severity::Warn),
        (17, Severity::Error),
        (20, Severity::Error),
        (21, Severity::Fatal),
        (24, Severity::Fatal),
        (25, Severity::Unspecified),
        (i32::MAX, Severity::Unspecified),
        (i32::MIN, Severity::Unspecified),
    ];
    for (number, severity) in expected {
        assert_eq!(Severity::from_number(number), severity, "number {number}");
    }
}

#[test]
fn severity_labels_and_order() {
    let ascending = [
        (Severity::Unspecified, "unknown"),
        (Severity::Trace, "trace"),
        (Severity::Debug, "debug"),
        (Severity::Info, "info"),
        (Severity::Warn, "warn"),
        (Severity::Error, "error"),
        (Severity::Fatal, "fatal"),
    ];
    for pair in ascending.windows(2) {
        assert!(pair[0].0 < pair[1].0);
    }
    for (severity, label) in ascending {
        assert_eq!(severity.level_label(), label);
    }
}

#[test]
fn loki_label_names_replace_every_odd_character() {
    assert_eq!(
        loki_label_name("http.request-method"),
        "http_request_method"
    );
    assert_eq!(loki_label_name("already_ok_9"), "already_ok_9");
    assert_eq!(loki_label_name("a b/c:d"), "a_b_c_d");
    assert_eq!(loki_label_name("naïve"), "na_ve");
    assert_eq!(loki_label_name(""), "");
}

#[test]
fn dedup_key_ignores_what_a_retry_may_change() {
    // Review focus 1: an exporter retry resends the same record.
    let original = log("payment declined");
    let mut retry = original.clone();
    retry.observed_unix_nano = 999;
    retry.severity_text = "ERR".to_string();
    retry.attributes = vec![s("attempt", "2")];
    assert_eq!(original.dedup_key(), retry.dedup_key());
}

#[test]
fn dedup_key_changes_with_each_identifying_field() {
    let base = log("payment declined");
    let mut keys = HashSet::new();
    keys.insert(base.dedup_key());

    let mut other_time = base.clone();
    other_time.time_unix_nano += 1;
    let mut other_body = base.clone();
    other_body.body.push('!');
    let mut other_trace = base.clone();
    other_trace.trace_id = Some(TraceId([9; 16]));
    let mut no_trace = base.clone();
    no_trace.trace_id = None;
    let mut other_span = base.clone();
    other_span.span_id = Some(SpanId([9; 8]));
    let mut other_service = base.clone();
    other_service.resource = resource("other");

    for variant in [
        other_time,
        other_body,
        other_trace,
        no_trace,
        other_span,
        other_service,
    ] {
        assert!(keys.insert(variant.dedup_key()), "collision: {variant:?}");
    }
}

#[test]
fn span_serializes_as_plain_json() {
    let mut sp = span(
        vec![
            s("http.method", "GET"),
            ("retries".to_string(), AttrValue::Int(2)),
            ("raw".to_string(), AttrValue::Bytes(vec![0xde, 0xad])),
        ],
        vec![event("exception", vec![s("exception.type", "Boom")])],
    );
    sp.status = StatusCode::Error;
    let value = serde_json::to_value(&sp).unwrap();
    assert_eq!(value["trace_id"], json!("01010101010101010101010101010101"));
    assert_eq!(value["span_id"], json!("0202020202020202"));
    assert_eq!(value["parent_span_id"], json!("0303030303030303"));
    assert_eq!(value["kind"], json!("client"));
    assert_eq!(value["status"], json!("error"));
    assert_eq!(
        value["attributes"],
        json!({"http.method": "GET", "retries": 2, "raw": "dead"})
    );
    assert_eq!(
        value["events"][0]["attributes"],
        json!({"exception.type": "Boom"})
    );
    assert_eq!(
        value["resource"]["attributes"],
        json!({"service.name": "svc"})
    );
}

#[test]
fn log_record_serializes_as_plain_json() {
    let mut record = log("hello");
    record.trace_id = None;
    record.severity = Severity::Warn;
    let value = serde_json::to_value(&record).unwrap();
    assert_eq!(value["severity"], json!("warn"));
    assert_eq!(value["trace_id"], json!(null));
    assert_eq!(value["span_id"], json!("0202020202020202"));
    assert_eq!(value["body"], json!("hello"));
}
