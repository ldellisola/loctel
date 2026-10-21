//! Conversion of OTLP requests into model types: field mapping, the rules for
//! invalid and odd IDs, log bodies and severities.

use std::sync::Arc;

use loctel_ingest::{Converted, logs_from_request, spans_from_request};
use loctel_model::{AttrValue, Severity, SpanId, SpanKind, StatusCode, TraceId, attr};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::{
    AnyValue, ArrayValue, InstrumentationScope, KeyValue, KeyValueList,
};
use opentelemetry_proto::tonic::logs::v1::{LogRecord as ProtoLog, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::resource::v1::Resource as ProtoResource;
use opentelemetry_proto::tonic::trace::v1::span::{Event, SpanKind as ProtoKind};
use opentelemetry_proto::tonic::trace::v1::status::StatusCode as ProtoStatus;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span as ProtoSpan, Status};

// --- builders ---------------------------------------------------------------

fn any(value: Value) -> AnyValue {
    AnyValue { value: Some(value) }
}

fn kv(key: &str, value: Value) -> KeyValue {
    KeyValue {
        key: key.to_owned(),
        value: Some(any(value)),
        ..Default::default()
    }
}

fn kv_str(key: &str, value: &str) -> KeyValue {
    kv(key, Value::StringValue(value.to_owned()))
}

fn resource(service: Option<&str>) -> ProtoResource {
    ProtoResource {
        attributes: service
            .map(|name| vec![kv_str("service.name", name)])
            .unwrap_or_default(),
        ..Default::default()
    }
}

fn trace_bytes(n: u8) -> Vec<u8> {
    let mut id = vec![0u8; 16];
    id[15] = n;
    id
}

fn span_bytes(n: u8) -> Vec<u8> {
    let mut id = vec![0u8; 8];
    id[7] = n;
    id
}

fn proto_span(trace: Vec<u8>, span: Vec<u8>, parent: Vec<u8>, name: &str) -> ProtoSpan {
    ProtoSpan {
        trace_id: trace,
        span_id: span,
        parent_span_id: parent,
        name: name.to_owned(),
        start_time_unix_nano: 1_000,
        end_time_unix_nano: 2_000,
        ..Default::default()
    }
}

fn trace_request(service: Option<&str>, spans: Vec<ProtoSpan>) -> ExportTraceServiceRequest {
    ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: Some(resource(service)),
            scope_spans: vec![ScopeSpans {
                scope: Some(InstrumentationScope {
                    name: "test-scope".to_owned(),
                    ..Default::default()
                }),
                spans,
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}

fn proto_log(body: Option<AnyValue>) -> ProtoLog {
    ProtoLog {
        time_unix_nano: 5_000,
        observed_time_unix_nano: 6_000,
        body,
        ..Default::default()
    }
}

fn logs_request(service: Option<&str>, records: Vec<ProtoLog>) -> ExportLogsServiceRequest {
    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(resource(service)),
            scope_logs: vec![ScopeLogs {
                scope: Some(InstrumentationScope {
                    name: "log-scope".to_owned(),
                    ..Default::default()
                }),
                log_records: records,
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}

fn tid(n: u8) -> TraceId {
    TraceId::from_bytes(&trace_bytes(n)).unwrap()
}

fn sid(n: u8) -> SpanId {
    SpanId::from_bytes(&span_bytes(n)).unwrap()
}

// --- spans ------------------------------------------------------------------

#[test]
fn converts_resource_scope_and_span_fields() {
    let mut full = proto_span(trace_bytes(1), span_bytes(2), span_bytes(1), "POST /orders");
    full.kind = ProtoKind::Server as i32;
    full.start_time_unix_nano = 1_700_000_000_000_000_000;
    full.end_time_unix_nano = 1_700_000_000_250_000_000;
    full.status = Some(Status {
        code: ProtoStatus::Error as i32,
        message: "card declined".to_owned(),
    });
    full.attributes = vec![
        kv_str("http.route", "/orders"),
        kv("http.status_code", Value::IntValue(502)),
        kv("retry", Value::BoolValue(true)),
        kv("ratio", Value::DoubleValue(0.5)),
        kv("blob", Value::BytesValue(vec![0xde, 0xad])),
        kv(
            "tags",
            Value::ArrayValue(ArrayValue {
                values: vec![any(Value::IntValue(1)), any(Value::StringValue("a".into()))],
            }),
        ),
        kv(
            "nested",
            Value::KvlistValue(KeyValueList {
                values: vec![kv_str("k", "v")],
            }),
        ),
    ];
    full.events = vec![Event {
        time_unix_nano: 1_700_000_000_100_000_000,
        name: "exception".to_owned(),
        attributes: vec![
            kv_str("exception.type", "CardDeclined"),
            kv_str("exception.message", "insufficient funds"),
        ],
        ..Default::default()
    }];
    let other = proto_span(trace_bytes(1), span_bytes(3), vec![], "GET /health");

    let converted: Converted<_> =
        spans_from_request(trace_request(Some("orders-api"), vec![full, other]));

    assert_eq!(converted.rejected, 0);
    assert_eq!(converted.error_message, None);
    assert_eq!(converted.items.len(), 2);

    let span = &converted.items[0];
    assert_eq!(span.trace_id, tid(1));
    assert_eq!(span.span_id, sid(2));
    assert_eq!(span.parent_span_id, Some(sid(1)));
    assert_eq!(span.name, "POST /orders");
    assert_eq!(span.kind, SpanKind::Server);
    assert_eq!(span.start_unix_nano, 1_700_000_000_000_000_000);
    assert_eq!(span.end_unix_nano, 1_700_000_000_250_000_000);
    assert_eq!(span.status, StatusCode::Error);
    assert_eq!(span.status_message, "card declined");
    assert!(span.is_error());
    assert_eq!(span.service(), "orders-api");
    assert_eq!(span.scope_name, "test-scope");

    assert_eq!(
        attr(&span.attributes, "http.route"),
        Some(&AttrValue::Str("/orders".into()))
    );
    assert_eq!(
        attr(&span.attributes, "http.status_code"),
        Some(&AttrValue::Int(502))
    );
    assert_eq!(
        attr(&span.attributes, "retry"),
        Some(&AttrValue::Bool(true))
    );
    assert_eq!(
        attr(&span.attributes, "ratio"),
        Some(&AttrValue::Double(0.5))
    );
    assert_eq!(
        attr(&span.attributes, "blob"),
        Some(&AttrValue::Bytes(vec![0xde, 0xad]))
    );
    assert_eq!(
        attr(&span.attributes, "tags"),
        Some(&AttrValue::Array(vec![
            AttrValue::Int(1),
            AttrValue::Str("a".into())
        ]))
    );
    assert_eq!(
        attr(&span.attributes, "nested"),
        Some(&AttrValue::Map(vec![(
            "k".into(),
            AttrValue::Str("v".into())
        )]))
    );

    assert_eq!(span.events.len(), 1);
    assert_eq!(span.events[0].name, "exception");
    assert_eq!(span.events[0].time_unix_nano, 1_700_000_000_100_000_000);
    let exception = span.exception().expect("exception event is converted");
    assert_eq!(exception.type_name, "CardDeclined");
    assert_eq!(exception.message, "insufficient funds");

    // The second span has no parent, no status and no kind.
    let second = &converted.items[1];
    assert_eq!(second.parent_span_id, None);
    assert_eq!(second.kind, SpanKind::Unspecified);
    assert_eq!(second.status, StatusCode::Unset);
    // Every span of one ResourceSpans shares one Resource allocation.
    assert!(Arc::ptr_eq(&span.resource, &second.resource));
}

#[test]
fn maps_every_span_kind_and_status_code() {
    let kinds = [
        (0, SpanKind::Unspecified),
        (1, SpanKind::Internal),
        (2, SpanKind::Server),
        (3, SpanKind::Client),
        (4, SpanKind::Producer),
        (5, SpanKind::Consumer),
        // A newer protocol revision may add values; they must not fail.
        (99, SpanKind::Unspecified),
    ];
    let statuses = [
        (0, StatusCode::Unset),
        (1, StatusCode::Ok),
        (2, StatusCode::Error),
        (99, StatusCode::Unset),
    ];
    for (n, (kind_number, expected_kind)) in kinds.into_iter().enumerate() {
        let mut span = proto_span(trace_bytes(1), span_bytes(n as u8 + 1), vec![], "kinds");
        span.kind = kind_number;
        let converted = spans_from_request(trace_request(Some("svc"), vec![span]));
        assert_eq!(converted.items[0].kind, expected_kind, "kind {kind_number}");
    }
    for (n, (code, expected)) in statuses.into_iter().enumerate() {
        let mut span = proto_span(trace_bytes(1), span_bytes(n as u8 + 1), vec![], "statuses");
        span.status = Some(Status {
            code,
            message: String::new(),
        });
        let converted = spans_from_request(trace_request(Some("svc"), vec![span]));
        assert_eq!(converted.items[0].status, expected, "status {code}");
    }
}

#[test]
fn missing_service_name_becomes_unknown_service() {
    let span = proto_span(trace_bytes(1), span_bytes(1), vec![], "no name");
    let converted = spans_from_request(trace_request(None, vec![span]));
    assert_eq!(converted.items[0].service(), "unknown_service");

    // A request with no resource at all behaves the same.
    let mut request = trace_request(
        Some("x"),
        vec![proto_span(
            trace_bytes(1),
            span_bytes(2),
            vec![],
            "no resource",
        )],
    );
    request.resource_spans[0].resource = None;
    let converted = spans_from_request(request);
    assert_eq!(converted.items[0].service(), "unknown_service");
}

#[test]
fn rejects_invalid_ids_with_partial_success() {
    let all_zero_trace = proto_span(vec![0u8; 16], span_bytes(1), vec![], "zero trace");
    let seven_byte_span = proto_span(trace_bytes(1), vec![1u8; 7], vec![], "short span id");
    let valid = proto_span(trace_bytes(1), span_bytes(3), vec![], "valid");

    let converted = spans_from_request(trace_request(
        Some("orders-api"),
        vec![all_zero_trace, seven_byte_span, valid],
    ));

    assert_eq!(converted.rejected, 2);
    assert_eq!(converted.items.len(), 1, "the valid span is kept");
    assert_eq!(converted.items[0].name, "valid");
    let message = converted.error_message.expect("a message explains it");
    assert!(message.contains("trace_id"), "{message}");
    assert!(message.contains("span_id"), "{message}");
}

#[test]
fn rejects_missing_and_malformed_ids() {
    let missing_trace = proto_span(vec![], span_bytes(1), vec![], "missing trace id");
    let missing_span = proto_span(trace_bytes(1), vec![], vec![], "missing span id");
    let zero_span = proto_span(trace_bytes(1), vec![0u8; 8], vec![], "zero span id");
    let long_trace = proto_span(vec![1u8; 17], span_bytes(1), vec![], "17-byte trace id");
    let bad_parent = proto_span(
        trace_bytes(1),
        span_bytes(2),
        vec![1u8; 7],
        "7-byte parent id",
    );

    let converted = spans_from_request(trace_request(
        Some("svc"),
        vec![
            missing_trace,
            missing_span,
            zero_span,
            long_trace,
            bad_parent,
        ],
    ));

    assert_eq!(converted.rejected, 5);
    assert!(converted.items.is_empty());
    let message = converted.error_message.unwrap();
    assert!(message.contains("parent_span_id"), "{message}");
}

#[test]
fn self_parent_becomes_root() {
    let self_parent = proto_span(trace_bytes(1), span_bytes(5), span_bytes(5), "self parent");
    let zero_parent = proto_span(trace_bytes(1), span_bytes(6), vec![0u8; 8], "zero parent");
    let empty_parent = proto_span(trace_bytes(1), span_bytes(7), vec![], "empty parent");
    let real_parent = proto_span(trace_bytes(1), span_bytes(8), span_bytes(5), "real parent");

    let converted = spans_from_request(trace_request(
        Some("svc"),
        vec![self_parent, zero_parent, empty_parent, real_parent],
    ));

    assert_eq!(converted.rejected, 0);
    assert_eq!(converted.error_message, None);
    let parents: Vec<_> = converted
        .items
        .iter()
        .map(|span| span.parent_span_id)
        .collect();
    assert_eq!(parents, vec![None, None, None, Some(sid(5))]);
}

#[test]
fn empty_attribute_values_are_dropped() {
    let mut span = proto_span(trace_bytes(1), span_bytes(1), vec![], "empty values");
    span.attributes = vec![
        kv_str("kept", "yes"),
        // An AnyValue with nothing set, and a key with no AnyValue at all.
        KeyValue {
            key: "empty".to_owned(),
            value: Some(AnyValue { value: None }),
            ..Default::default()
        },
        KeyValue {
            key: "absent".to_owned(),
            value: None,
            ..Default::default()
        },
    ];
    let converted = spans_from_request(trace_request(Some("svc"), vec![span]));
    assert_eq!(
        converted.items[0].attributes,
        vec![("kept".to_owned(), AttrValue::Str("yes".to_owned()))]
    );
}

#[test]
fn an_empty_request_converts_to_nothing() {
    let converted = spans_from_request(ExportTraceServiceRequest::default());
    assert!(converted.items.is_empty());
    assert_eq!(converted.rejected, 0);
    assert_eq!(converted.error_message, None);
    let converted = logs_from_request(ExportLogsServiceRequest::default());
    assert!(converted.items.is_empty());
    assert_eq!(converted.rejected, 0);
}

// --- logs -------------------------------------------------------------------

#[test]
fn converts_log_fields() {
    let mut record = proto_log(Some(any(Value::StringValue("payment failed".into()))));
    record.severity_number = 17;
    record.severity_text = "ERROR".to_owned();
    record.trace_id = trace_bytes(1);
    record.span_id = span_bytes(2);
    record.attributes = vec![
        kv_str("exception.type", "CardDeclined"),
        kv_str("exception.message", "insufficient funds"),
    ];

    let converted = logs_from_request(logs_request(Some("payments-api"), vec![record]));

    assert_eq!(converted.rejected, 0);
    assert_eq!(converted.error_message, None);
    let log = &converted.items[0];
    assert_eq!(log.time_unix_nano, 5_000);
    assert_eq!(log.observed_unix_nano, 6_000);
    assert_eq!(log.severity, Severity::Error);
    assert_eq!(log.severity_text, "ERROR");
    assert_eq!(log.body, "payment failed");
    assert_eq!(log.trace_id, Some(tid(1)));
    assert_eq!(log.span_id, Some(sid(2)));
    assert_eq!(log.service(), "payments-api");
    assert_eq!(log.scope_name, "log-scope");
    assert_eq!(log.exception().unwrap().type_name, "CardDeclined");
}

#[test]
fn log_body_map_becomes_json() {
    let body = any(Value::KvlistValue(KeyValueList {
        values: vec![
            kv_str("user", "alice"),
            kv("attempt", Value::IntValue(3)),
            kv(
                "steps",
                Value::ArrayValue(ArrayValue {
                    values: vec![
                        any(Value::StringValue("auth".into())),
                        any(Value::IntValue(2)),
                    ],
                }),
            ),
        ],
    }));
    let converted = logs_from_request(logs_request(Some("svc"), vec![proto_log(Some(body))]));
    assert_eq!(
        converted.items[0].body,
        r#"{"user":"alice","attempt":3,"steps":["auth",2]}"#
    );
}

#[test]
fn log_body_scalars_and_missing_body() {
    let bodies = [
        (
            Some(any(Value::StringValue("plain text".into()))),
            "plain text",
        ),
        (Some(any(Value::IntValue(42))), "42"),
        (Some(any(Value::BoolValue(true))), "true"),
        (Some(any(Value::DoubleValue(1.5))), "1.5"),
        (
            Some(any(Value::ArrayValue(ArrayValue {
                values: vec![any(Value::IntValue(1))],
            }))),
            "[1]",
        ),
        (Some(AnyValue { value: None }), ""),
        (None, ""),
    ];
    let records = bodies
        .iter()
        .map(|(body, _)| proto_log(body.clone()))
        .collect();
    let converted = logs_from_request(logs_request(Some("svc"), records));
    let got: Vec<_> = converted
        .items
        .iter()
        .map(|log| log.body.as_str())
        .collect();
    let want: Vec<_> = bodies.iter().map(|(_, expected)| *expected).collect();
    assert_eq!(got, want);
}

#[test]
fn severity_number_maps() {
    let numbers = [
        (0, Severity::Unspecified),
        (1, Severity::Trace),
        (5, Severity::Debug),
        (9, Severity::Info),
        (13, Severity::Warn),
        (17, Severity::Error),
        (21, Severity::Fatal),
        (24, Severity::Fatal),
        (25, Severity::Unspecified),
        (-3, Severity::Unspecified),
    ];
    let records = numbers
        .iter()
        .map(|(number, _)| {
            let mut record = proto_log(Some(any(Value::StringValue("x".into()))));
            record.severity_number = *number;
            record
        })
        .collect();
    let converted = logs_from_request(logs_request(Some("svc"), records));
    let got: Vec<_> = converted.items.iter().map(|log| log.severity).collect();
    let want: Vec<_> = numbers.iter().map(|(_, severity)| *severity).collect();
    assert_eq!(got, want);
}

#[test]
fn log_context_ids_are_optional_and_never_reject() {
    let mut zero = proto_log(Some(any(Value::StringValue("zero".into()))));
    zero.trace_id = vec![0u8; 16];
    zero.span_id = vec![0u8; 8];
    let mut malformed = proto_log(Some(any(Value::StringValue("malformed".into()))));
    malformed.trace_id = vec![1u8; 5];
    malformed.span_id = vec![1u8; 7];
    let mut trace_only = proto_log(Some(any(Value::StringValue("trace only".into()))));
    trace_only.trace_id = trace_bytes(9);
    let none = proto_log(Some(any(Value::StringValue("none".into()))));

    let converted = logs_from_request(logs_request(
        Some("svc"),
        vec![zero, malformed, trace_only, none],
    ));

    assert_eq!(converted.rejected, 0);
    assert_eq!(converted.error_message, None);
    assert_eq!(converted.items.len(), 4, "logs are always kept");
    let context: Vec<_> = converted
        .items
        .iter()
        .map(|log| (log.trace_id, log.span_id))
        .collect();
    assert_eq!(
        context,
        vec![
            (None, None),
            (None, None),
            (Some(tid(9)), None),
            (None, None)
        ]
    );
}

#[test]
fn log_without_service_name_is_unknown_service() {
    let converted = logs_from_request(logs_request(
        None,
        vec![proto_log(Some(any(Value::StringValue("x".into()))))],
    ));
    assert_eq!(converted.items[0].service(), "unknown_service");
}
