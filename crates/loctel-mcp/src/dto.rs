//! What the tools return: small, JSON-shaped values with a schema.
//!
//! These are the shapes the MCP spec calls `structuredContent`; each tool
//! advertises its type's schema as its `outputSchema`. They are built from the
//! store's types and nothing else, so what an agent is told is what the UI
//! shows (design 9.4). The conventions:
//!
//! - IDs are lowercase hex strings, times are RFC 3339 in UTC to the
//!   millisecond (cut, not rounded, so a time read back as `since` still
//!   includes its own item), durations and offsets are milliseconds to the
//!   microsecond.
//! - A key that has no value is left out (`?` in the design table), except a
//!   `null` where the tool's table lists the key without `?`.
//! - Messages and stack traces longer than [`MAX_TEXT_CHARS`] characters are
//!   cut and end with [`TRUNCATED`] (design 8: answers stay small).

use chrono::{DateTime, SecondsFormat, Utc};
use loctel_model::{
    AttrValue, ExceptionInfo, LogRecord, Severity, Span, SpanEvent, SpanKind, StatusCode,
};
use loctel_store::{FailedIn, NodeKind, ServiceMap, ServiceStats, TraceStatus, TraceSummary};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Longest message or stack trace, in characters, before it is cut.
pub const MAX_TEXT_CHARS: usize = 2_000;

/// What a cut message ends with.
pub const TRUNCATED: &str = "…(truncated)";

// --- helpers ------------------------------------------------------------------

/// `text`, or its first [`MAX_TEXT_CHARS`] characters and [`TRUNCATED`] when
/// it is longer. Counts characters, never cutting one in half.
pub(crate) fn truncate(text: &str) -> String {
    match text.char_indices().nth(MAX_TEXT_CHARS) {
        None => text.to_owned(),
        Some((cut, _)) => format!("{}{TRUNCATED}", &text[..cut]),
    }
}

/// Cuts every string in a JSON value the way [`truncate`] does: attribute
/// values can hold the same long text (an `exception.stacktrace` event
/// attribute is the stack trace again).
pub(crate) fn truncate_json(value: &mut Value) {
    match value {
        Value::String(text) => {
            if text.chars().nth(MAX_TEXT_CHARS).is_some() {
                *text = truncate(text);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(truncate_json),
        Value::Object(map) => map.values_mut().for_each(truncate_json),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

/// RFC 3339 in UTC to the millisecond, `2027-01-15T08:00:00.123Z`.
pub(crate) fn rfc3339(unix_nano: u64) -> String {
    let nanos = i64::try_from(unix_nano).unwrap_or(i64::MAX);
    DateTime::<Utc>::from_timestamp_nanos(nanos).to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Nanoseconds as milliseconds, to the microsecond.
pub(crate) fn millis(nanos: u64) -> f64 {
    (nanos as f64 / 1_000.0).round() / 1_000.0
}

/// An error rate, to four decimals; 0 when there were no requests.
fn rate(errors: u64, requests: u64) -> f64 {
    if requests == 0 {
        return 0.0;
    }
    (errors as f64 / requests as f64 * 10_000.0).round() / 10_000.0
}

/// A JSON object of attributes, with long strings cut.
fn attributes_object(attributes: &[(String, AttrValue)]) -> Map<String, Value> {
    // `AttrValue::Map` serializes as a plain JSON object, keys in order.
    let value = serde_json::to_value(AttrValue::Map(attributes.to_vec())).unwrap_or(Value::Null);
    let mut object = match value {
        Value::Object(object) => object,
        _ => Map::new(),
    };
    object.values_mut().for_each(truncate_json);
    object
}

/// The schema of a key that is always there and is a string or `null`.
/// (`#[schemars(required)]` on an `Option<String>` alone would drop the
/// `null`.)
fn nullable_string(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({ "type": ["string", "null"] })
}

// --- enums ---------------------------------------------------------------------

/// Did the request work? See design 9.4.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TraceStatusDto {
    /// No span is erroring.
    Ok,
    /// The root span is fine (or has not arrived) but another span is erroring.
    ErrorsInside,
    /// The root span is erroring.
    Failed,
}

impl From<TraceStatus> for TraceStatusDto {
    fn from(status: TraceStatus) -> Self {
        match status {
            TraceStatus::Ok => TraceStatusDto::Ok,
            TraceStatus::ErrorsInside => TraceStatusDto::ErrorsInside,
            TraceStatus::Failed => TraceStatusDto::Failed,
        }
    }
}

/// OpenTelemetry span kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SpanKindDto {
    Unspecified,
    Internal,
    Server,
    Client,
    Producer,
    Consumer,
}

impl From<SpanKind> for SpanKindDto {
    fn from(kind: SpanKind) -> Self {
        match kind {
            SpanKind::Unspecified => SpanKindDto::Unspecified,
            SpanKind::Internal => SpanKindDto::Internal,
            SpanKind::Server => SpanKindDto::Server,
            SpanKind::Client => SpanKindDto::Client,
            SpanKind::Producer => SpanKindDto::Producer,
            SpanKind::Consumer => SpanKindDto::Consumer,
        }
    }
}

/// OpenTelemetry span status. Only `error` makes a span erroring.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SpanStatusDto {
    Unset,
    Ok,
    Error,
}

impl From<StatusCode> for SpanStatusDto {
    fn from(status: StatusCode) -> Self {
        match status {
            StatusCode::Unset => SpanStatusDto::Unset,
            StatusCode::Ok => SpanStatusDto::Ok,
            StatusCode::Error => SpanStatusDto::Error,
        }
    }
}

/// What a node of the service map stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NodeKindDto {
    /// A service that sends telemetry.
    Service,
    Database,
    /// A message broker.
    Queue,
    /// A remote host that never reported.
    External,
}

impl From<NodeKind> for NodeKindDto {
    fn from(kind: NodeKind) -> Self {
        match kind {
            NodeKind::Service => NodeKindDto::Service,
            NodeKind::Database => NodeKindDto::Database,
            NodeKind::Queue => NodeKindDto::Queue,
            NodeKind::External => NodeKindDto::External,
        }
    }
}

/// A log level, lowest first. `unknown` is a log that has no severity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Unknown,
    Trace,
    Debug,
    Info,
    #[serde(alias = "warning")]
    Warn,
    Error,
    Fatal,
}

impl From<Severity> for LogLevel {
    fn from(severity: Severity) -> Self {
        match severity {
            Severity::Unspecified => LogLevel::Unknown,
            Severity::Trace => LogLevel::Trace,
            Severity::Debug => LogLevel::Debug,
            Severity::Info => LogLevel::Info,
            Severity::Warn => LogLevel::Warn,
            Severity::Error => LogLevel::Error,
            Severity::Fatal => LogLevel::Fatal,
        }
    }
}

impl From<LogLevel> for Severity {
    fn from(level: LogLevel) -> Self {
        match level {
            LogLevel::Unknown => Severity::Unspecified,
            LogLevel::Trace => Severity::Trace,
            LogLevel::Debug => Severity::Debug,
            LogLevel::Info => Severity::Info,
            LogLevel::Warn => Severity::Warn,
            LogLevel::Error => Severity::Error,
            LogLevel::Fatal => Severity::Fatal,
        }
    }
}

// --- status and marker ----------------------------------------------------------

/// `get_status`: is loctel receiving, on which ports, how fast, how much it
/// holds.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct StatusDto {
    /// Receiving is paused: incoming data is dropped.
    pub paused: bool,
    pub listeners: Vec<ListenerDto>,
    /// Spans accepted per second over the last 5 seconds.
    pub spans_per_sec: f64,
    /// Log records accepted per second over the last 5 seconds.
    pub logs_per_sec: f64,
    pub traces_held: u64,
    pub spans_held: u64,
    pub logs_held: u64,
    /// The discard rules, in the order they are applied (file order), each
    /// with what it has dropped since launch. Empty when there are none: loctel
    /// never decides what to discard.
    pub rules: Vec<RuleDto>,
    /// Why `rules.toml` could not be used the last time it was read, or `null`.
    /// While it is set the rules listed are the last good ones.
    #[schemars(required, schema_with = "nullable_string")]
    pub rules_file_error: Option<String>,
    /// Spans and logs the rules dropped, divided by those dropped plus those
    /// accepted, since launch: 0 to 1, 0 when nothing has arrived.
    pub dropped_fraction: f64,
}

/// The query language of a discard rule, as written in `rules.toml`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum RuleLanguageDto {
    /// Judges spans (and, through them, traces).
    TraceQl,
    /// Judges logs.
    LogQl,
}

/// What a TraceQL discard rule drops when it matches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum RuleScopeDto {
    /// The whole trace: every span and every log of it, stored or still to
    /// arrive.
    Trace,
    /// Only the matched spans, their descendants and the logs emitted in them.
    Span,
}

/// One discard rule of `get_status`.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct RuleDto {
    /// What the user calls the rule.
    pub name: String,
    pub language: RuleLanguageDto,
    /// What a match drops. Only for `traceql` rules: a `logql` rule only ever
    /// drops the log that matched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<RuleScopeDto>,
    /// A rule that is off is kept and does nothing.
    pub enabled: bool,
    /// Why the query does not parse, with the column. A rule with an error does
    /// nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Spans this rule has dropped since launch: incoming ones and stored ones.
    pub dropped_spans: u64,
    /// Logs this rule has dropped since launch.
    pub dropped_logs: u64,
    /// Traces this rule has dropped whole since launch.
    pub dropped_traces: u64,
}

/// One endpoint (`otlp-grpc`, `otlp-http`, `mcp`).
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct ListenerDto {
    pub name: String,
    /// Where it listens; for a failed one, where it was asked to.
    pub address: String,
    /// `listening` or `failed`.
    pub state: String,
    /// Why it failed (the operating system's reason).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `get_marker`.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct MarkerDto {
    /// The server's clock in nanoseconds since the Unix epoch, as a decimal
    /// string. Pass it as `since`.
    pub marker: String,
    /// The same instant as an RFC 3339 time.
    pub time: String,
}

// --- services and the map ---------------------------------------------------------

/// `list_services`. An object holding the list, not a bare array: MCP
/// 2025-11-25 requires an output schema and `structuredContent` to be objects,
/// and a client that checks `tools/list` against that rule rejects the whole
/// list if one tool has an array root.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct ServiceListDto {
    /// The services that sent spans, by name.
    pub services: Vec<ServiceDto>,
}

/// One line of `list_services`.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct ServiceDto {
    pub name: String,
    /// Server and consumer spans (requests handled).
    pub requests: u64,
    /// How many of those are erroring.
    pub errors: u64,
    /// `errors / requests`, 0 to 1.
    pub error_rate: f64,
    /// When the service's latest span started.
    pub last_seen: String,
}

impl From<&ServiceStats> for ServiceDto {
    fn from(stats: &ServiceStats) -> Self {
        ServiceDto {
            name: stats.name.clone(),
            requests: stats.requests,
            errors: stats.errors,
            error_rate: rate(stats.errors, stats.requests),
            last_seen: rfc3339(stats.last_seen_unix_nano),
        }
    }
}

/// `get_service_map`: who calls whom, with call and error counts.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct ServiceMapDto {
    pub nodes: Vec<NodeDto>,
    pub edges: Vec<EdgeDto>,
}

/// A service, database, queue or external host. For a service, `requests`
/// counts its server and consumer spans; for the others, the calls it
/// received.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct NodeDto {
    pub id: String,
    pub kind: NodeKindDto,
    pub requests: u64,
    pub errors: u64,
}

/// `calls` calls from node `from` to node `to`, `errors` of them erroring.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct EdgeDto {
    pub from: String,
    pub to: String,
    pub calls: u64,
    pub errors: u64,
}

impl From<&ServiceMap> for ServiceMapDto {
    fn from(map: &ServiceMap) -> Self {
        ServiceMapDto {
            nodes: map
                .nodes
                .iter()
                .map(|node| NodeDto {
                    id: node.id.clone(),
                    kind: node.kind.into(),
                    requests: node.requests,
                    errors: node.errors,
                })
                .collect(),
            edges: map
                .edges
                .iter()
                .map(|edge| EdgeDto {
                    from: edge.from.clone(),
                    to: edge.to.clone(),
                    calls: edge.calls,
                    errors: edge.errors,
                })
                .collect(),
        }
    }
}

// --- traces and spans ---------------------------------------------------------------

/// A trace in one row.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct TraceSummaryDto {
    pub trace_id: String,
    /// The root span's name; `null` until the root arrives.
    #[schemars(required, schema_with = "nullable_string")]
    pub root_name: Option<String>,
    /// The root span's service; `null` until the root arrives.
    #[schemars(required, schema_with = "nullable_string")]
    pub root_service: Option<String>,
    /// When the earliest span started.
    pub start: String,
    /// From the earliest span start to the latest span end.
    pub duration_ms: f64,
    pub span_count: u64,
    /// Each service once, the entry point first.
    pub services: Vec<String>,
    pub status: TraceStatusDto,
    /// Where it broke: present unless `status` is `ok`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_in: Option<FailedInDto>,
    /// The exception that best explains the trace.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_exception: Option<ExceptionBriefDto>,
    /// No new span has arrived for a while (10 s by default). A trace that is
    /// not complete may still be gaining spans.
    pub complete: bool,
    /// The spans a TraceQL `query` matched in this trace (span IDs, at most
    /// 20, in the order the trace holds them). Only in `search_traces` with a
    /// `query`; left out otherwise.
    // `default` is what keeps the key out of the schema's `required` list:
    // schemars drops only an `Option`'s from it for `skip_serializing_if`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub matched_span_ids: Vec<String>,
}

/// The span that explains a failure (design 9.4): the earliest-starting
/// erroring span with no erroring descendant.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct FailedInDto {
    pub service: String,
    pub span_name: String,
    /// The remote party, for a client or producer span.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peer: Option<String>,
    pub span_id: String,
    /// How many other such spans there are ("+N more").
    pub more: u64,
}

impl From<&FailedIn> for FailedInDto {
    fn from(failed: &FailedIn) -> Self {
        FailedInDto {
            service: failed.service.clone(),
            span_name: failed.span_name.clone(),
            peer: failed.peer.clone(),
            span_id: failed.span_id.to_string(),
            more: failed.more as u64,
        }
    }
}

/// An exception without its stack trace.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct ExceptionBriefDto {
    /// `exception.type`.
    #[serde(rename = "type")]
    pub type_name: String,
    pub message: String,
}

impl From<&ExceptionInfo> for ExceptionBriefDto {
    fn from(exception: &ExceptionInfo) -> Self {
        ExceptionBriefDto {
            type_name: exception.type_name.clone(),
            message: truncate(&exception.message),
        }
    }
}

/// An exception with its stack trace.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct ExceptionDto {
    /// `exception.type`.
    #[serde(rename = "type")]
    pub type_name: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stacktrace: Option<String>,
}

impl From<&ExceptionInfo> for ExceptionDto {
    fn from(exception: &ExceptionInfo) -> Self {
        ExceptionDto {
            type_name: exception.type_name.clone(),
            message: truncate(&exception.message),
            stacktrace: exception.stacktrace.as_deref().map(truncate),
        }
    }
}

impl From<&TraceSummary> for TraceSummaryDto {
    fn from(summary: &TraceSummary) -> Self {
        TraceSummaryDto {
            trace_id: summary.trace_id.to_string(),
            root_name: summary.root_name.clone(),
            root_service: summary.root_service.clone(),
            start: rfc3339(summary.start_unix_nano),
            duration_ms: millis(summary.duration_nanos),
            span_count: summary.span_count as u64,
            services: summary.services.clone(),
            status: summary.status.into(),
            failed_in: summary.failed_in.as_ref().map(FailedInDto::from),
            first_exception: summary
                .first_exception
                .as_ref()
                .map(ExceptionBriefDto::from),
            complete: summary.complete,
            matched_span_ids: summary
                .matched_span_ids
                .iter()
                .map(ToString::to_string)
                .collect(),
        }
    }
}

/// A span in one row, for the tree of `get_trace`.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct SpanBriefDto {
    pub span_id: String,
    /// Absent for a root. A span whose parent has not arrived yet still names
    /// it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_span_id: Option<String>,
    pub name: String,
    pub service: String,
    pub kind: SpanKindDto,
    /// Milliseconds from the start of the trace to the start of this span.
    pub start_offset_ms: f64,
    pub duration_ms: f64,
    pub status: SpanStatusDto,
    /// `exception.type` of the span's exception event, if it has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exception_type: Option<String>,
}

impl SpanBriefDto {
    /// `trace_start` is the trace's earliest span start, which offsets count
    /// from.
    pub(crate) fn new(span: &Span, trace_start: u64) -> Self {
        SpanBriefDto {
            span_id: span.span_id.to_string(),
            parent_span_id: span.parent_span_id.map(|id| id.to_string()),
            name: span.name.clone(),
            service: span.service().to_owned(),
            kind: span.kind.into(),
            start_offset_ms: millis(span.start_unix_nano.saturating_sub(trace_start)),
            duration_ms: millis(span.duration_nanos()),
            status: span.status.into(),
            exception_type: span
                .exception()
                .map(|exception| exception.type_name)
                .filter(|type_name| !type_name.is_empty()),
        }
    }
}

/// An event on a span (an `exception`, a log-like note).
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct SpanEventDto {
    pub time: String,
    pub name: String,
    pub attributes: Map<String, Value>,
}

impl From<&SpanEvent> for SpanEventDto {
    fn from(event: &SpanEvent) -> Self {
        SpanEventDto {
            time: rfc3339(event.time_unix_nano),
            name: event.name.clone(),
            attributes: attributes_object(&event.attributes),
        }
    }
}

/// Everything about one span.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct SpanDetailDto {
    pub trace_id: String,
    pub span_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_span_id: Option<String>,
    pub name: String,
    pub service: String,
    pub kind: SpanKindDto,
    pub start: String,
    /// Milliseconds from the start of the trace to the start of this span.
    pub start_offset_ms: f64,
    pub duration_ms: f64,
    pub status: SpanStatusDto,
    pub status_message: String,
    pub attributes: Map<String, Value>,
    pub events: Vec<SpanEventDto>,
    /// The attributes of the emitting service (`service.name`, ...).
    pub resource: Map<String, Value>,
    /// The instrumentation scope that made the span.
    pub scope_name: String,
    /// The span's exception event, if it has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exception: Option<ExceptionDto>,
}

impl SpanDetailDto {
    pub(crate) fn new(span: &Span, trace_start: u64) -> Self {
        SpanDetailDto {
            trace_id: span.trace_id.to_string(),
            span_id: span.span_id.to_string(),
            parent_span_id: span.parent_span_id.map(|id| id.to_string()),
            name: span.name.clone(),
            service: span.service().to_owned(),
            kind: span.kind.into(),
            start: rfc3339(span.start_unix_nano),
            start_offset_ms: millis(span.start_unix_nano.saturating_sub(trace_start)),
            duration_ms: millis(span.duration_nanos()),
            status: span.status.into(),
            status_message: truncate(&span.status_message),
            attributes: attributes_object(&span.attributes),
            events: span.events.iter().map(SpanEventDto::from).collect(),
            resource: attributes_object(&span.resource.attributes),
            scope_name: span.scope_name.clone(),
            exception: span.exception().as_ref().map(ExceptionDto::from),
        }
    }
}

// --- logs ---------------------------------------------------------------------------------

/// One log line.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct LogDto {
    pub time: String,
    pub level: LogLevel,
    pub service: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span_id: Option<String>,
}

impl From<&LogRecord> for LogDto {
    fn from(log: &LogRecord) -> Self {
        // The log's own time, or when it was observed if it has none: the
        // time the store orders and filters by.
        let time = if log.time_unix_nano != 0 {
            log.time_unix_nano
        } else {
            log.observed_unix_nano
        };
        LogDto {
            time: rfc3339(time),
            level: log.severity.into(),
            service: log.service().to_owned(),
            message: truncate(&log.body),
            trace_id: log.trace_id.map(|id| id.to_string()),
            span_id: log.span_id.map(|id| id.to_string()),
        }
    }
}

// --- tool results ---------------------------------------------------------------------------

/// `search_traces`.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct SearchTracesResult {
    /// Newest first.
    pub traces: Vec<TraceSummaryDto>,
    /// Pass as `cursor` for the next page; `null` on the last page.
    #[schemars(required, schema_with = "nullable_string")]
    pub next_cursor: Option<String>,
}

/// One failed trace in `recent_failures`, with what an agent needs to say why.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct FailureDto {
    pub trace: TraceSummaryDto,
    /// The span the failure is in.
    pub failed_in_span: SpanBriefDto,
    /// The exception that explains it, with its stack trace.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exception: Option<ExceptionDto>,
    /// The trace's error-or-worse logs, oldest first, at most 10.
    pub error_logs: Vec<LogDto>,
}

/// `recent_failures`.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct RecentFailuresResult {
    /// Traces whose root span is erroring: the request failed. Newest first.
    pub failed: Vec<FailureDto>,
    /// Traces whose root is fine but another span is erroring (a call that
    /// failed and was retried, say): the request may have worked. Newest first.
    pub errors_inside: Vec<TraceSummaryDto>,
}

/// `get_trace`.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct GetTraceResult {
    pub summary: TraceSummaryDto,
    /// This page of spans, oldest start first.
    pub spans: Vec<SpanBriefDto>,
    /// How many spans come after this page.
    pub omitted: u64,
    /// Pass as `cursor` (with the same `detail`) for the next page; `null`
    /// when `omitted` is 0.
    #[schemars(required, schema_with = "nullable_string")]
    pub next_cursor: Option<String>,
}

/// `get_span`.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct GetSpanResult {
    pub span: SpanDetailDto,
    /// The logs emitted in the span, oldest first, at most 50.
    pub logs: Vec<LogDto>,
}

/// `search_logs`.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct SearchLogsResult {
    /// Newest first.
    pub logs: Vec<LogDto>,
    /// Pass as `cursor` for the next page; `null` on the last page.
    #[schemars(required, schema_with = "nullable_string")]
    pub next_cursor: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_cut_at_2000_characters_not_bytes() {
        assert_eq!(truncate(""), "");
        assert_eq!(truncate("short"), "short");
        let exactly = "é".repeat(2_000);
        assert_eq!(truncate(&exactly), exactly);
        let over = "é".repeat(2_001);
        assert_eq!(
            truncate(&over),
            format!("{}…(truncated)", "é".repeat(2_000))
        );
        // Emoji are one character each, however many bytes.
        let emoji = "😀".repeat(5_000);
        let cut = truncate(&emoji);
        assert_eq!(cut.chars().count(), 2_000 + TRUNCATED.chars().count());
        assert!(cut.ends_with("😀…(truncated)"));
    }

    #[test]
    fn long_strings_anywhere_in_json_are_cut() {
        let long = "x".repeat(2_500);
        let mut value = serde_json::json!({
            "a": long,
            "b": [long, 1, null, true],
            "c": { "d": long, "e": "short" },
        });
        truncate_json(&mut value);
        let cut = format!("{}…(truncated)", "x".repeat(2_000));
        assert_eq!(value["a"], cut.as_str());
        assert_eq!(value["b"][0], cut.as_str());
        assert_eq!(value["b"][1], 1);
        assert_eq!(value["c"]["d"], cut.as_str());
        assert_eq!(value["c"]["e"], "short");
    }

    #[test]
    fn times_are_utc_milliseconds_cut_not_rounded() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(
            rfc3339(1_800_000_000_999_999_999),
            "2027-01-15T08:00:00.999Z"
        );
        // Past what a signed 64-bit count of nanoseconds holds: no panic.
        assert_eq!(rfc3339(u64::MAX), "2262-04-11T23:47:16.854Z");
    }

    #[test]
    fn milliseconds_keep_the_microsecond() {
        assert_eq!(millis(0), 0.0);
        assert_eq!(millis(1_840_000_000), 1840.0);
        assert_eq!(millis(1_500), 0.002);
        assert_eq!(millis(1_499), 0.001);
        assert_eq!(millis(12_345_678), 12.346);
    }

    #[test]
    fn a_rate_is_zero_without_requests() {
        assert_eq!(rate(0, 0), 0.0);
        assert_eq!(rate(5, 0), 0.0);
        assert_eq!(rate(1, 3), 0.3333);
        assert_eq!(rate(3, 3), 1.0);
    }

    #[test]
    fn log_levels_round_trip_and_accept_warning() {
        for level in [
            LogLevel::Unknown,
            LogLevel::Trace,
            LogLevel::Debug,
            LogLevel::Info,
            LogLevel::Warn,
            LogLevel::Error,
            LogLevel::Fatal,
        ] {
            assert_eq!(LogLevel::from(Severity::from(level)), level);
            let json = serde_json::to_string(&level).unwrap();
            assert_eq!(serde_json::from_str::<LogLevel>(&json).unwrap(), level);
        }
        assert_eq!(
            serde_json::from_str::<LogLevel>("\"warning\"").unwrap(),
            LogLevel::Warn
        );
        assert!(serde_json::from_str::<LogLevel>("\"loud\"").is_err());
    }
}
