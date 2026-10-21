//! OTLP request messages to loctel model types.
//!
//! The conversion is lenient about what it can be and strict about identity.
//! A span needs a usable trace ID and span ID, otherwise nothing downstream
//! can place it, so it is rejected and counted (the receivers report the count
//! through `partial_success`). Everything else is kept: odd parent IDs are
//! repaired, unknown enum values fall back to "unspecified", and a resource
//! without `service.name` is still a resource (the model shows it as
//! `unknown_service`).

use std::sync::Arc;

use loctel_model::{
    AttrValue, Attributes, LogRecord, Resource, Severity, Span, SpanEvent, SpanId, SpanKind,
    StatusCode, TraceId,
};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::{AnyValue, InstrumentationScope, KeyValue};
use opentelemetry_proto::tonic::logs::v1::LogRecord as ProtoLog;
use opentelemetry_proto::tonic::resource::v1::Resource as ProtoResource;
use opentelemetry_proto::tonic::trace::v1::Span as ProtoSpan;
use opentelemetry_proto::tonic::trace::v1::span::SpanKind as ProtoKind;
use opentelemetry_proto::tonic::trace::v1::status::StatusCode as ProtoStatus;

/// The outcome of converting one export request.
#[derive(Clone, Debug, PartialEq)]
pub struct Converted<T> {
    /// The items that converted, in request order.
    pub items: Vec<T>,
    /// How many items were dropped as invalid. Reported to the exporter as
    /// `partial_success.rejected_*`.
    pub rejected: u64,
    /// A developer-facing explanation of the rejections (`None` when
    /// nothing was rejected). Reported as `partial_success.error_message`.
    pub error_message: Option<String>,
}

/// Converts a trace export request. Spans with an invalid trace ID or span
/// ID (missing, wrong length, all zeros) are rejected, as are spans with a
/// malformed parent ID (non-empty, wrong length). A parent ID that is empty,
/// all zeros or equal to the span's own ID means "no parent".
pub fn spans_from_request(request: ExportTraceServiceRequest) -> Converted<Span> {
    let mut items = Vec::new();
    let mut rejections = SpanRejections::default();
    let mut total = 0u64;

    for resource_spans in request.resource_spans {
        let resource = convert_resource(resource_spans.resource);
        for scope_spans in resource_spans.scope_spans {
            let scope_name = scope_name(scope_spans.scope);
            for proto in scope_spans.spans {
                total += 1;
                match convert_span(proto, &resource, &scope_name) {
                    Ok(span) => items.push(span),
                    Err(reason) => rejections.add(reason),
                }
            }
        }
    }

    Converted {
        items,
        rejected: rejections.total(),
        error_message: rejections.message(total),
    }
}

/// Converts a logs export request. Logs are never rejected: a log without
/// usable trace context is still a log, so an empty, all-zero or malformed
/// trace ID or span ID just means "no link" for that ID.
pub fn logs_from_request(request: ExportLogsServiceRequest) -> Converted<LogRecord> {
    let mut items = Vec::new();
    for resource_logs in request.resource_logs {
        let resource = convert_resource(resource_logs.resource);
        for scope_logs in resource_logs.scope_logs {
            let scope_name = scope_name(scope_logs.scope);
            for proto in scope_logs.log_records {
                items.push(convert_log(proto, &resource, &scope_name));
            }
        }
    }
    Converted {
        items,
        rejected: 0,
        error_message: None,
    }
}

// --- spans ------------------------------------------------------------------

/// Why a span was rejected.
#[derive(Clone, Copy)]
enum SpanRejection {
    Trace,
    Span,
    Parent,
}

#[derive(Default)]
struct SpanRejections {
    trace_id: u64,
    span_id: u64,
    parent_span_id: u64,
}

impl SpanRejections {
    fn add(&mut self, reason: SpanRejection) {
        match reason {
            SpanRejection::Trace => self.trace_id += 1,
            SpanRejection::Span => self.span_id += 1,
            SpanRejection::Parent => self.parent_span_id += 1,
        }
    }

    fn total(&self) -> u64 {
        self.trace_id + self.span_id + self.parent_span_id
    }

    /// For example: `rejected 2 of 3 spans: 1 invalid trace_id (needs 16
    /// bytes, not all zero), 1 invalid span_id (needs 8 bytes, not all zero)`.
    fn message(&self, total_spans: u64) -> Option<String> {
        let rejected = self.total();
        if rejected == 0 {
            return None;
        }
        let reasons: Vec<String> = [
            (
                self.trace_id,
                "invalid trace_id (needs 16 bytes, not all zero)",
            ),
            (
                self.span_id,
                "invalid span_id (needs 8 bytes, not all zero)",
            ),
            (
                self.parent_span_id,
                "invalid parent_span_id (needs 8 bytes, or none)",
            ),
        ]
        .into_iter()
        .filter(|(count, _)| *count > 0)
        .map(|(count, what)| format!("{count} {what}"))
        .collect();
        Some(format!(
            "rejected {rejected} of {total_spans} spans: {}",
            reasons.join(", ")
        ))
    }
}

fn convert_span(
    proto: ProtoSpan,
    resource: &Arc<Resource>,
    scope_name: &str,
) -> Result<Span, SpanRejection> {
    let trace_id = valid_trace_id(&proto.trace_id).ok_or(SpanRejection::Trace)?;
    let span_id = valid_span_id(&proto.span_id).ok_or(SpanRejection::Span)?;
    let parent_span_id = parent_id(&proto.parent_span_id, span_id)?;

    let kind = match ProtoKind::try_from(proto.kind) {
        Ok(ProtoKind::Internal) => SpanKind::Internal,
        Ok(ProtoKind::Server) => SpanKind::Server,
        Ok(ProtoKind::Client) => SpanKind::Client,
        Ok(ProtoKind::Producer) => SpanKind::Producer,
        Ok(ProtoKind::Consumer) => SpanKind::Consumer,
        // Unspecified, and any value a newer protocol revision adds.
        Ok(ProtoKind::Unspecified) | Err(_) => SpanKind::Unspecified,
    };
    let (status, status_message) = match proto.status {
        Some(status) => (
            match ProtoStatus::try_from(status.code) {
                Ok(ProtoStatus::Ok) => StatusCode::Ok,
                Ok(ProtoStatus::Error) => StatusCode::Error,
                Ok(ProtoStatus::Unset) | Err(_) => StatusCode::Unset,
            },
            status.message,
        ),
        None => (StatusCode::Unset, String::new()),
    };

    Ok(Span {
        trace_id,
        span_id,
        parent_span_id,
        name: proto.name,
        kind,
        start_unix_nano: proto.start_time_unix_nano,
        end_unix_nano: proto.end_time_unix_nano,
        status,
        status_message,
        attributes: convert_attributes(proto.attributes),
        events: proto
            .events
            .into_iter()
            .map(|event| SpanEvent {
                time_unix_nano: event.time_unix_nano,
                name: event.name,
                attributes: convert_attributes(event.attributes),
            })
            .collect(),
        resource: Arc::clone(resource),
        scope_name: scope_name.to_owned(),
    })
}

/// `Some` for a usable trace ID: 16 bytes, not all zero.
fn valid_trace_id(bytes: &[u8]) -> Option<TraceId> {
    TraceId::from_bytes(bytes).ok().filter(TraceId::is_valid)
}

/// `Some` for a usable span ID: 8 bytes, not all zero.
fn valid_span_id(bytes: &[u8]) -> Option<SpanId> {
    SpanId::from_bytes(bytes).ok().filter(SpanId::is_valid)
}

/// An empty, all-zero or self-referencing parent means "root". Anything else
/// that is not 8 bytes is malformed.
fn parent_id(bytes: &[u8], own: SpanId) -> Result<Option<SpanId>, SpanRejection> {
    if bytes.is_empty() {
        return Ok(None);
    }
    match SpanId::from_bytes(bytes) {
        Ok(parent) if parent.is_valid() && parent != own => Ok(Some(parent)),
        Ok(_) => Ok(None),
        Err(_) => Err(SpanRejection::Parent),
    }
}

// --- logs -------------------------------------------------------------------

fn convert_log(proto: ProtoLog, resource: &Arc<Resource>, scope_name: &str) -> LogRecord {
    LogRecord {
        time_unix_nano: proto.time_unix_nano,
        observed_unix_nano: proto.observed_time_unix_nano,
        severity: Severity::from_number(proto.severity_number),
        severity_text: proto.severity_text,
        body: proto
            .body
            .and_then(convert_value)
            .map(|body| body.to_display_string())
            .unwrap_or_default(),
        attributes: convert_attributes(proto.attributes),
        trace_id: valid_trace_id(&proto.trace_id),
        span_id: valid_span_id(&proto.span_id),
        resource: Arc::clone(resource),
        scope_name: scope_name.to_owned(),
    }
}

// --- shared pieces ------------------------------------------------------------

fn convert_resource(resource: Option<ProtoResource>) -> Arc<Resource> {
    Arc::new(Resource {
        attributes: convert_attributes(resource.map(|r| r.attributes).unwrap_or_default()),
    })
}

fn scope_name(scope: Option<InstrumentationScope>) -> String {
    scope.map(|scope| scope.name).unwrap_or_default()
}

/// Key/value pairs in order. A pair with no value (a missing `AnyValue`, or
/// one with nothing set) carries no information and is dropped.
fn convert_attributes(pairs: Vec<KeyValue>) -> Attributes {
    pairs
        .into_iter()
        .filter_map(|pair| Some((pair.key, convert_value(pair.value?)?)))
        .collect()
}

/// `None` for an `AnyValue` with nothing set. Empty array elements are
/// dropped for the same reason as empty attributes.
fn convert_value(any: AnyValue) -> Option<AttrValue> {
    Some(match any.value? {
        Value::StringValue(text) => AttrValue::Str(text),
        Value::BoolValue(flag) => AttrValue::Bool(flag),
        Value::IntValue(number) => AttrValue::Int(number),
        Value::DoubleValue(number) => AttrValue::Double(number),
        Value::BytesValue(bytes) => AttrValue::Bytes(bytes),
        Value::ArrayValue(array) => {
            AttrValue::Array(array.values.into_iter().filter_map(convert_value).collect())
        }
        Value::KvlistValue(list) => AttrValue::Map(convert_attributes(list.values)),
        // An index into a string table that only the profiles signal has; in
        // traces and logs there is nothing to look it up in.
        Value::StringValueStrindex(_) => return None,
    })
}
