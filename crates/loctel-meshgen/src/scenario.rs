//! The scenario vocabulary (what can be generated, and in what shape) and the
//! small builders every scenario shares: seeded IDs, resources, spans, logs
//! and the grouping of items into OTLP export requests.

use std::time::Duration;

use loctel_model::{SpanId, TraceId};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::{AnyValue, InstrumentationScope, KeyValue};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::span::SpanKind;
use opentelemetry_proto::tonic::trace::v1::status::StatusCode;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span, Status};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

use crate::{checkout, health, huge, log_flood};

/// Every service in the fake mesh, in the order they first appear in the
/// checkout trace. `notifications-worker` only consumes messages.
pub const SERVICES: [&str; 5] = [
    "web-bff",
    "orders-api",
    "inventory-api",
    "payments-api",
    "notifications-worker",
];

/// The four services that serve HTTP requests (every service except the
/// message worker). `HugeTrace` rotates among these.
pub const APP_SERVICES: [&str; 4] = ["web-bff", "orders-api", "inventory-api", "payments-api"];

/// `service.version` of every service in the mesh.
pub const SERVICE_VERSION: &str = "1.4.0";

/// `deployment.environment` of every service in the mesh.
pub const ENVIRONMENT: &str = "local";

/// What to generate. See `docs/areas/testing.md` for each one's content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scenario {
    /// The fixed 15-span, 7-log failing checkout the mockups show.
    CheckoutFailure,
    /// The same tree with every status unset: 14 spans, INF logs only.
    CheckoutSuccess,
    /// `count` `GET /health` spans (and a log each) for every service, each
    /// its own trace.
    HealthChecks { count: usize },
    /// A `CheckoutSuccess` trace sent leaves first, root last.
    LateOutOfOrder,
    /// One trace of exactly `spans` spans, at most 8 levels deep.
    HugeTrace { spans: usize },
    /// `records` INF and DBG logs with no trace context.
    LogFlood { records: usize },
}

/// One send: an optional trace export and an optional logs export, preceded
/// by a pause.
#[derive(Clone, Debug, PartialEq)]
pub struct Batch {
    pub traces: Option<ExportTraceServiceRequest>,
    pub logs: Option<ExportLogsServiceRequest>,
    /// How long a sender waits before this batch, before `time_scale`.
    pub delay_before: Duration,
}

/// The output of [`generate`].
#[derive(Clone, Debug, PartialEq)]
pub struct Generated {
    pub batches: Vec<Batch>,
    /// Every trace the scenario contains, in the order they were made.
    pub trace_ids: Vec<TraceId>,
}

/// Generates a scenario. Deterministic: the same inputs give identical bytes.
/// IDs come from a ChaCha RNG seeded with `seed`, and every timestamp is
/// `base_unix_nano` plus an offset the scenario fixes.
pub fn generate(s: &Scenario, seed: u64, base_unix_nano: u64) -> Generated {
    match s {
        Scenario::CheckoutFailure => checkout::generate(true, seed, base_unix_nano),
        Scenario::CheckoutSuccess => checkout::generate(false, seed, base_unix_nano),
        Scenario::LateOutOfOrder => checkout::late_out_of_order(seed, base_unix_nano),
        Scenario::HealthChecks { count } => health::generate(*count, seed, base_unix_nano),
        Scenario::HugeTrace { spans } => huge::generate(*spans, seed, base_unix_nano),
        Scenario::LogFlood { records } => log_flood::generate(*records, seed, base_unix_nano),
    }
}

// --- shared builders --------------------------------------------------------

pub(crate) const NANOS_PER_MS: u64 = 1_000_000;

/// `base` plus `offset_ms`, saturating.
pub(crate) fn at_ms(base: u64, offset_ms: u64) -> u64 {
    base.saturating_add(offset_ms.saturating_mul(NANOS_PER_MS))
}

/// Which ChaCha stream a scenario draws from. The seed picks the generator
/// and the stream picks a sequence within it, so two scenarios given the
/// same seed still get different IDs. Without this, `checkout-failure` and
/// `checkout-success` with the default seed would be one trace to a store
/// that ignores duplicate span IDs.
pub(crate) mod stream {
    pub(crate) const CHECKOUT_FAILURE: u64 = 1;
    pub(crate) const CHECKOUT_SUCCESS: u64 = 2;
    pub(crate) const LATE_OUT_OF_ORDER: u64 = 3;
    pub(crate) const HEALTH_CHECKS: u64 = 4;
    pub(crate) const HUGE_TRACE: u64 = 5;
    pub(crate) const LOG_FLOOD: u64 = 6;
    pub(crate) const BULK_LOGS: u64 = 7;
    pub(crate) const BULK_TRACES: u64 = 8;
}

/// The seeded source of every ID and of the small random choices.
#[derive(Clone, Debug)]
pub(crate) struct Rand(ChaCha8Rng);

impl Rand {
    /// A ChaCha RNG seeded with `seed`, reading the given [`stream`].
    pub(crate) fn new(seed: u64, stream: u64) -> Self {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        rng.set_stream(stream);
        Rand(rng)
    }

    /// A trace ID that is never all zero.
    pub(crate) fn trace_id(&mut self) -> TraceId {
        let mut bytes = [0u8; 16];
        self.0.fill_bytes(&mut bytes);
        if bytes == [0u8; 16] {
            bytes[15] = 1;
        }
        TraceId(bytes)
    }

    /// A span ID that is never all zero.
    pub(crate) fn span_id(&mut self) -> SpanId {
        let mut bytes = [0u8; 8];
        self.0.fill_bytes(&mut bytes);
        if bytes == [0u8; 8] {
            bytes[7] = 1;
        }
        SpanId(bytes)
    }

    /// A number in `0..n` (0 when `n` is 0). The slight modulo bias does not
    /// matter for test data.
    pub(crate) fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.0.next_u64() % n }
    }
}

pub(crate) fn str_kv(key: &str, value: &str) -> KeyValue {
    KeyValue {
        key: key.to_owned(),
        value: Some(AnyValue {
            value: Some(Value::StringValue(value.to_owned())),
        }),
        ..Default::default()
    }
}

pub(crate) fn int_kv(key: &str, value: i64) -> KeyValue {
    KeyValue {
        key: key.to_owned(),
        value: Some(AnyValue {
            value: Some(Value::IntValue(value)),
        }),
        ..Default::default()
    }
}

/// The resource every service in the mesh has: its name, version and
/// environment.
pub(crate) fn resource(service: &str) -> Resource {
    Resource {
        attributes: vec![
            str_kv("service.name", service),
            str_kv("service.version", SERVICE_VERSION),
            str_kv("deployment.environment", ENVIRONMENT),
        ],
        ..Default::default()
    }
}

fn scope() -> InstrumentationScope {
    InstrumentationScope {
        name: "loctel-meshgen".to_owned(),
        ..Default::default()
    }
}

/// A span with no attributes, events or status yet. The times are emitted as
/// given.
pub(crate) fn span(
    trace: TraceId,
    id: SpanId,
    parent: Option<SpanId>,
    name: &str,
    kind: SpanKind,
    start_unix_nano: u64,
    end_unix_nano: u64,
) -> Span {
    Span {
        trace_id: trace.0.to_vec(),
        span_id: id.0.to_vec(),
        parent_span_id: parent.map(|p| p.0.to_vec()).unwrap_or_default(),
        name: name.to_owned(),
        kind: kind as i32,
        start_time_unix_nano: start_unix_nano,
        end_time_unix_nano: end_unix_nano,
        ..Default::default()
    }
}

/// Marks a span as erroring (status `Error`) with a developer-facing message.
pub(crate) fn set_error(span: &mut Span, message: &str) {
    span.status = Some(Status {
        message: message.to_owned(),
        code: StatusCode::Error as i32,
    });
}

/// Log levels the scenarios use, with their OTLP severity number and text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    fn number(self) -> i32 {
        match self {
            Level::Debug => 5,
            Level::Info => 9,
            Level::Warn => 13,
            Level::Error => 17,
        }
    }

    fn text(self) -> &'static str {
        match self {
            Level::Debug => "DEBUG",
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
        }
    }
}

/// A log record at `time_unix_nano`, linked to a span when `context` is
/// given.
pub(crate) fn log(
    time_unix_nano: u64,
    level: Level,
    body: &str,
    context: Option<(TraceId, SpanId)>,
) -> LogRecord {
    let (trace_id, span_id) = context.map_or((Vec::new(), Vec::new()), |(t, s)| {
        (t.0.to_vec(), s.0.to_vec())
    });
    LogRecord {
        time_unix_nano,
        observed_time_unix_nano: time_unix_nano,
        severity_number: level.number(),
        severity_text: level.text().to_owned(),
        body: Some(AnyValue {
            value: Some(Value::StringValue(body.to_owned())),
        }),
        trace_id,
        span_id,
        ..Default::default()
    }
}

/// Groups `(service, item)` pairs by service, in the order the services first
/// appear and keeping each service's items in order.
fn group_by_service<T>(items: Vec<(&str, T)>) -> Vec<(String, Vec<T>)> {
    let mut groups: Vec<(String, Vec<T>)> = Vec::new();
    for (service, item) in items {
        match groups.iter_mut().find(|(name, _)| name == service) {
            Some((_, group)) => group.push(item),
            None => groups.push((service.to_owned(), vec![item])),
        }
    }
    groups
}

/// One trace export request: a `ResourceSpans` per service.
pub(crate) fn trace_request(spans: Vec<(&str, Span)>) -> ExportTraceServiceRequest {
    ExportTraceServiceRequest {
        resource_spans: group_by_service(spans)
            .into_iter()
            .map(|(service, spans)| ResourceSpans {
                resource: Some(resource(&service)),
                scope_spans: vec![ScopeSpans {
                    scope: Some(scope()),
                    spans,
                    ..Default::default()
                }],
                ..Default::default()
            })
            .collect(),
    }
}

/// One logs export request: a `ResourceLogs` per service.
pub(crate) fn log_request(logs: Vec<(&str, LogRecord)>) -> ExportLogsServiceRequest {
    ExportLogsServiceRequest {
        resource_logs: group_by_service(logs)
            .into_iter()
            .map(|(service, log_records)| ResourceLogs {
                resource: Some(resource(&service)),
                scope_logs: vec![ScopeLogs {
                    scope: Some(scope()),
                    log_records,
                    ..Default::default()
                }],
                ..Default::default()
            })
            .collect(),
    }
}
