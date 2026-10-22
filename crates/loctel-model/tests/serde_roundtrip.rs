//! Every model type that serializes also deserializes, and comes back equal
//! (Milestone 5, Task 1: the store writes them into segment files).
//!
//! Two formats matter. JSON keeps its plain shapes (see `edge_cases.rs` and
//! the MCP tests for what they look like), so it reads back what it wrote
//! except where JSON itself cannot say the difference: a byte value is a hex
//! string there and reads back as a string. Postcard, which the segment files
//! use, is not self-describing and has no such gaps: every type must come back
//! equal from it, bit for bit.

use std::sync::Arc;

use loctel_model::{
    AttrValue, Attributes, ExceptionInfo, LogRecord, Resource, Severity, Span, SpanEvent, SpanId,
    SpanKind, StatusCode, TraceId,
};

fn s(key: &str, value: &str) -> (String, AttrValue) {
    (key.to_string(), AttrValue::Str(value.to_string()))
}

fn postcard_roundtrip<T>(value: &T) -> T
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let bytes = postcard::to_stdvec(value).expect("postcard serializes");
    postcard::from_bytes(&bytes).expect("postcard deserializes")
}

fn json_roundtrip<T>(value: &T) -> T
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let text = serde_json::to_string(value).expect("json serializes");
    serde_json::from_str(&text).expect("json deserializes")
}

/// One of every attribute value, nested, with a key that repeats (OTLP allows
/// it and the model keeps the order and every pair).
fn every_attribute(with_bytes: bool) -> Attributes {
    let mut nested = vec![
        s("inner", "x"),
        ("flag".to_string(), AttrValue::Bool(true)),
        ("again".to_string(), AttrValue::Int(-7)),
        s("inner", "y"),
    ];
    let mut array = vec![
        AttrValue::Str("a".to_string()),
        AttrValue::Int(i64::MIN),
        AttrValue::Int(i64::MAX),
        AttrValue::Double(2.5),
        AttrValue::Array(vec![AttrValue::Bool(false), AttrValue::Map(vec![])]),
    ];
    if with_bytes {
        nested.push(("blob".to_string(), AttrValue::Bytes(vec![0, 1, 254, 255])));
        array.push(AttrValue::Bytes(vec![]));
    }
    let mut attributes = vec![
        s("http.method", "GET"),
        s("empty", ""),
        s("unicode", "caf\u{e9} \u{1f980}"),
        ("http.status_code".to_string(), AttrValue::Int(503)),
        ("ratio".to_string(), AttrValue::Double(0.25)),
        ("whole".to_string(), AttrValue::Double(3.0)),
        ("negative".to_string(), AttrValue::Double(-1.5e300)),
        ("ok".to_string(), AttrValue::Bool(false)),
        ("list".to_string(), AttrValue::Array(array)),
        ("map".to_string(), AttrValue::Map(nested)),
        s("http.method", "POST"),
    ];
    if with_bytes {
        attributes.push((
            "trace_state".to_string(),
            AttrValue::Bytes(vec![0xde, 0xad, 0xbe, 0xef]),
        ));
    }
    attributes
}

fn resource() -> Arc<Resource> {
    Arc::new(Resource {
        attributes: vec![
            s("service.name", "checkout"),
            s("service.version", "1.4.2"),
            s("deployment.environment", "dev"),
        ],
    })
}

fn span(with_bytes: bool) -> Span {
    Span {
        trace_id: TraceId::from_hex("4bf92f3577b34da6a3ce929d0e0e4736").unwrap(),
        span_id: SpanId::from_hex("00f067aa0ba902b7").unwrap(),
        parent_span_id: Some(SpanId::from_hex("0102030405060708").unwrap()),
        name: "POST /checkout".to_string(),
        kind: SpanKind::Server,
        start_unix_nano: 1_700_000_000_123_456_789,
        end_unix_nano: u64::MAX,
        status: StatusCode::Error,
        status_message: "card declined".to_string(),
        attributes: every_attribute(with_bytes),
        events: vec![
            SpanEvent {
                time_unix_nano: 1_700_000_000_200_000_000,
                name: "exception".to_string(),
                attributes: vec![
                    s("exception.type", "PaymentDeclined"),
                    s("exception.message", "insufficient funds"),
                    s("exception.stacktrace", "at pay()\nat checkout()"),
                ],
            },
            SpanEvent {
                time_unix_nano: 0,
                name: "bare".to_string(),
                attributes: vec![],
            },
        ],
        resource: resource(),
        scope_name: "io.opentelemetry.test".to_string(),
    }
}

fn log(with_bytes: bool, context: bool) -> LogRecord {
    LogRecord {
        time_unix_nano: 1_700_000_000_300_000_000,
        observed_unix_nano: 1_700_000_000_300_000_001,
        severity: Severity::Error,
        severity_text: "ERROR".to_string(),
        body: "payment declined: \"card\"\n\tretry later".to_string(),
        attributes: every_attribute(with_bytes),
        trace_id: context.then_some(TraceId([9; 16])),
        span_id: context.then_some(SpanId([7; 8])),
        resource: resource(),
        scope_name: "logger".to_string(),
    }
}

const SPAN_KINDS: [SpanKind; 6] = [
    SpanKind::Unspecified,
    SpanKind::Internal,
    SpanKind::Server,
    SpanKind::Client,
    SpanKind::Producer,
    SpanKind::Consumer,
];
const STATUSES: [StatusCode; 3] = [StatusCode::Unset, StatusCode::Ok, StatusCode::Error];
const SEVERITIES: [Severity; 7] = [
    Severity::Unspecified,
    Severity::Trace,
    Severity::Debug,
    Severity::Info,
    Severity::Warn,
    Severity::Error,
    Severity::Fatal,
];

#[test]
fn model_serde_roundtrip_all_types() {
    // Ids.
    let trace = TraceId::from_hex("4bf92f3577b34da6a3ce929d0e0e4736").unwrap();
    let span_id = SpanId::from_hex("00f067aa0ba902b7").unwrap();
    for id in [trace, TraceId([0; 16]), TraceId([255; 16])] {
        assert_eq!(postcard_roundtrip(&id), id);
        assert_eq!(json_roundtrip(&id), id);
    }
    for id in [span_id, SpanId([0; 8]), SpanId([255; 8])] {
        assert_eq!(postcard_roundtrip(&id), id);
        assert_eq!(json_roundtrip(&id), id);
    }

    // The enums.
    for kind in SPAN_KINDS {
        assert_eq!(postcard_roundtrip(&kind), kind);
        assert_eq!(json_roundtrip(&kind), kind);
    }
    for status in STATUSES {
        assert_eq!(postcard_roundtrip(&status), status);
        assert_eq!(json_roundtrip(&status), status);
    }
    for severity in SEVERITIES {
        assert_eq!(postcard_roundtrip(&severity), severity);
        assert_eq!(json_roundtrip(&severity), severity);
    }

    // Attribute values, bytes included, in postcard.
    for (_, value) in every_attribute(true) {
        assert_eq!(postcard_roundtrip(&value), value);
    }

    // Exceptions.
    for exception in [
        ExceptionInfo {
            type_name: "T".to_string(),
            message: "m".to_string(),
            stacktrace: Some("trace".to_string()),
        },
        ExceptionInfo {
            type_name: String::new(),
            message: String::new(),
            stacktrace: None,
        },
    ] {
        assert_eq!(postcard_roundtrip(&exception), exception);
        assert_eq!(json_roundtrip(&exception), exception);
    }

    // Resources, spans, events and logs: exactly in postcard.
    let resource = Resource {
        attributes: every_attribute(true),
    };
    assert_eq!(postcard_roundtrip(&resource), resource);
    let span = span(true);
    assert_eq!(postcard_roundtrip(&span), span);
    assert_eq!(postcard_roundtrip(&span.events[0]), span.events[0]);
    for context in [true, false] {
        let log = log(true, context);
        assert_eq!(postcard_roundtrip(&log), log);
    }
    // A span of every kind and status, a log of every severity.
    for kind in SPAN_KINDS {
        for status in STATUSES {
            let mut span = span.clone();
            span.kind = kind;
            span.status = status;
            span.parent_span_id = None;
            assert_eq!(postcard_roundtrip(&span), span);
        }
    }
    for severity in SEVERITIES {
        let mut log = log(false, true);
        log.severity = severity;
        assert_eq!(postcard_roundtrip(&log), log);
    }

    // The same types in JSON, with no byte values (JSON cannot tell them from
    // strings, see `json_reads_bytes_back_as_the_hex_string`).
    let resource = Resource {
        attributes: every_attribute(false),
    };
    assert_eq!(json_roundtrip(&resource), resource);
    let span = self::span(false);
    assert_eq!(json_roundtrip(&span), span);
    for context in [true, false] {
        let log = self::log(false, context);
        assert_eq!(json_roundtrip(&log), log);
    }
}

#[test]
fn postcard_keeps_every_double_bit_for_bit() {
    // NaN is not equal to itself, and JSON writes the non-finite values as
    // null, so these are compared by bits and only in postcard.
    for value in [
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        -0.0,
        0.0,
        f64::MIN_POSITIVE / 2.0,
        f64::MAX,
    ] {
        let back = postcard_roundtrip(&AttrValue::Double(value));
        let AttrValue::Double(back) = back else {
            panic!("a double came back as {back:?}");
        };
        assert_eq!(back.to_bits(), value.to_bits());
    }
}

#[test]
fn json_reads_bytes_back_as_the_hex_string() {
    // The JSON shape of a byte value is its lowercase hex string, which is
    // also what a string looks like: that one gap is by design.
    let bytes = AttrValue::Bytes(vec![0xde, 0xad, 0xbe, 0xef]);
    assert_eq!(
        json_roundtrip(&bytes),
        AttrValue::Str("deadbeef".to_string())
    );
}

#[test]
fn json_attribute_values_read_by_their_shape() {
    let read = |text: &str| -> AttrValue { serde_json::from_str(text).unwrap() };
    assert_eq!(read("\"x\""), AttrValue::Str("x".to_string()));
    assert_eq!(read("true"), AttrValue::Bool(true));
    assert_eq!(read("-12"), AttrValue::Int(-12));
    assert_eq!(read("9223372036854775807"), AttrValue::Int(i64::MAX));
    // The whole double keeps its point in JSON, so it stays a double.
    assert_eq!(read("3.0"), AttrValue::Double(3.0));
    assert_eq!(read("1e3"), AttrValue::Double(1000.0));
    assert_eq!(
        read("[1,[],{}]"),
        AttrValue::Array(vec![
            AttrValue::Int(1),
            AttrValue::Array(vec![]),
            AttrValue::Map(vec![]),
        ])
    );
    // An object keeps its order and a repeated key.
    assert_eq!(
        read(r#"{"b":1,"a":2,"b":3}"#),
        AttrValue::Map(vec![
            ("b".to_string(), AttrValue::Int(1)),
            ("a".to_string(), AttrValue::Int(2)),
            ("b".to_string(), AttrValue::Int(3)),
        ])
    );
}

#[test]
fn postcard_ids_are_their_raw_bytes() {
    assert_eq!(postcard::to_stdvec(&TraceId([5; 16])).unwrap().len(), 16);
    assert_eq!(postcard::to_stdvec(&SpanId([5; 8])).unwrap().len(), 8);
}

#[test]
fn malformed_input_is_an_error_not_a_panic() {
    // A bad id in JSON.
    assert!(serde_json::from_str::<TraceId>("\"abc\"").is_err());
    assert!(serde_json::from_str::<SpanId>("\"zzzzzzzzzzzzzzzz\"").is_err());
    assert!(serde_json::from_str::<TraceId>("7").is_err());
    // An unknown variant name in JSON.
    assert!(serde_json::from_str::<Severity>("\"loud\"").is_err());
    // Truncated and garbage postcard.
    let bytes = postcard::to_stdvec(&span(true)).unwrap();
    for len in [0, 1, bytes.len() / 2, bytes.len() - 1] {
        assert!(postcard::from_bytes::<Span>(&bytes[..len]).is_err());
    }
    assert!(postcard::from_bytes::<AttrValue>(&[0x7f, 0x01, 0x02]).is_err());
}
