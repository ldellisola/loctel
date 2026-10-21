//! The checkout scenarios: the fixed failing checkout from the mockups, its
//! successful twin, and that twin sent out of order.
//!
//! `CheckoutFailure` is frozen. Tests and screenshots rely on every row of
//! the table below, so changing one means updating the screenshots and the
//! e2e expectations too (`docs/areas/testing.md`).

use std::time::Duration;

use loctel_model::{SpanId, TraceId};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::KeyValue;
use opentelemetry_proto::tonic::logs::v1::LogRecord;
use opentelemetry_proto::tonic::trace::v1::span::SpanKind;
use opentelemetry_proto::tonic::trace::v1::{Span, span::Event};

use crate::scenario::{
    Batch, Generated, Level, Rand, at_ms, int_kv, log, log_request, set_error, span, str_kv,
    stream, trace_request,
};

/// `exception.type` of the failing charge.
pub const EXCEPTION_TYPE: &str = "Payments.Api.Gateway.CardGatewayTimeoutException";

/// `exception.message` of the failing charge.
pub const EXCEPTION_MESSAGE: &str = "card-gateway did not respond after 2 attempts";

/// `exception.stacktrace` of the failing charge: the three lines of the Trace
/// mockup.
pub const EXCEPTION_STACKTRACE: &str = "\
at Payments.Api.Gateway.CardGatewayClient.ChargeAsync(ChargeRequest request) in CardGatewayClient.cs:line 64
at Payments.Api.Charges.ChargeService.ChargeAsync(Guid orderId) in ChargeService.cs:line 41
at Payments.Api.Charges.ChargeEndpoints.HandleCharge(ChargeRequest request) in ChargeEndpoints.cs:line 22";

/// The order the whole scenario is about.
const ORDER_ID: i64 = 8812;

/// How many spans the failing tree has. Span IDs are drawn for every slot, so
/// the failing and the successful tree share IDs for the spans they share.
const SLOTS: usize = 15;

/// One row of the span table. `n` is the 1-based number in the plan.
struct Row {
    n: usize,
    parent: Option<usize>,
    service: &'static str,
    kind: SpanKind,
    name: &'static str,
    start_ms: u64,
    dur_ms: u64,
    /// The status message when the span is erroring (status `Error`); `None`
    /// leaves the status unset.
    error: Option<&'static str>,
    attributes: Vec<KeyValue>,
}

/// The span table. `failure` selects the failing tree (15 spans, errors) or
/// the successful one (14 spans, nothing erroring).
fn rows(failure: bool) -> Vec<Row> {
    // Error status only in the failing tree.
    let err = |message: &'static str| failure.then_some(message);
    let http_status = if failure { 500 } else { 200 };
    let (publish, process) = if failure {
        ("order.failed publish", "order.failed process")
    } else {
        ("order.created publish", "order.created process")
    };

    let mut gateway_call = vec![str_kv("server.address", "card-gateway")];
    if failure {
        gateway_call.push(str_kv("error.type", "timeout"));
    }

    let mut rows = vec![
        Row {
            n: 1,
            parent: None,
            service: "web-bff",
            kind: SpanKind::Server,
            name: "POST /api/checkout",
            start_ms: 0,
            dur_ms: 1840,
            error: err("HTTP 500"),
            attributes: vec![
                str_kv("http.route", "/api/checkout"),
                int_kv("http.response.status_code", http_status),
            ],
        },
        Row {
            n: 2,
            parent: Some(1),
            service: "web-bff",
            kind: SpanKind::Internal,
            name: "validate_token",
            start_ms: 4,
            dur_ms: 18,
            error: None,
            attributes: vec![],
        },
        Row {
            n: 3,
            parent: Some(1),
            service: "web-bff",
            kind: SpanKind::Client,
            name: "POST",
            start_ms: 30,
            dur_ms: 1780,
            error: err("HTTP 500"),
            attributes: vec![str_kv("server.address", "orders-api")],
        },
        Row {
            n: 4,
            parent: Some(3),
            service: "orders-api",
            kind: SpanKind::Server,
            name: "POST /orders",
            start_ms: 34,
            dur_ms: 1772,
            error: err("HTTP 500"),
            attributes: vec![int_kv("http.response.status_code", http_status)],
        },
        Row {
            n: 5,
            parent: Some(4),
            service: "orders-api",
            kind: SpanKind::Client,
            name: "SELECT carts",
            start_ms: 40,
            dur_ms: 18,
            error: None,
            attributes: vec![str_kv("db.system.name", "postgresql")],
        },
        Row {
            n: 6,
            parent: Some(4),
            service: "orders-api",
            kind: SpanKind::Client,
            name: "GET",
            start_ms: 62,
            dur_ms: 128,
            error: None,
            attributes: vec![str_kv("server.address", "inventory-api")],
        },
        Row {
            n: 7,
            parent: Some(6),
            service: "inventory-api",
            kind: SpanKind::Server,
            name: "GET /stock/{sku}",
            start_ms: 66,
            dur_ms: 120,
            error: None,
            attributes: vec![],
        },
        Row {
            n: 8,
            parent: Some(7),
            service: "inventory-api",
            kind: SpanKind::Client,
            name: "MGET",
            start_ms: 70,
            dur_ms: 4,
            error: None,
            attributes: vec![str_kv("db.system.name", "redis")],
        },
        Row {
            n: 9,
            parent: Some(4),
            service: "orders-api",
            kind: SpanKind::Client,
            name: "POST",
            start_ms: 196,
            dur_ms: 1594,
            error: err("HTTP 500"),
            attributes: vec![str_kv("server.address", "payments-api")],
        },
        Row {
            n: 10,
            parent: Some(9),
            service: "payments-api",
            kind: SpanKind::Server,
            name: "POST /charges",
            start_ms: 200,
            dur_ms: 1586,
            error: err(EXCEPTION_MESSAGE),
            attributes: vec![int_kv("order.id", ORDER_ID)],
        },
        Row {
            n: 11,
            parent: Some(10),
            service: "payments-api",
            kind: SpanKind::Client,
            name: "SELECT payment_methods",
            start_ms: 206,
            dur_ms: 24,
            error: None,
            attributes: vec![str_kv("db.system.name", "postgresql")],
        },
        Row {
            n: 12,
            parent: Some(10),
            service: "payments-api",
            kind: SpanKind::Client,
            name: "POST",
            start_ms: 236,
            dur_ms: 600,
            error: err("timeout"),
            attributes: gateway_call,
        },
    ];
    if failure {
        rows.push(Row {
            n: 13,
            parent: Some(10),
            service: "payments-api",
            kind: SpanKind::Client,
            name: "POST",
            start_ms: 840,
            dur_ms: 600,
            error: Some("timeout"),
            attributes: vec![
                str_kv("server.address", "card-gateway"),
                int_kv("http.request.resend_count", 1),
            ],
        });
    }
    rows.push(Row {
        n: 14,
        parent: Some(4),
        service: "orders-api",
        kind: SpanKind::Producer,
        name: publish,
        start_ms: 1794,
        dur_ms: 6,
        error: None,
        attributes: vec![str_kv("messaging.system", "rabbitmq")],
    });
    rows.push(Row {
        n: 15,
        parent: Some(14),
        service: "notifications-worker",
        kind: SpanKind::Consumer,
        name: process,
        start_ms: 1802,
        dur_ms: 14,
        error: None,
        attributes: vec![str_kv("messaging.system", "rabbitmq")],
    });
    rows
}

/// One log of the scenario: when, how bad, on which span, and what it says.
struct LogRow {
    offset_ms: u64,
    level: Level,
    span: usize,
    body: &'static str,
    /// Carries the `exception.*` attributes of the failing charge.
    exception: bool,
}

fn log_rows(failure: bool) -> Vec<LogRow> {
    let row = |offset_ms, level, span, body| LogRow {
        offset_ms,
        level,
        span,
        body,
        exception: false,
    };
    if failure {
        vec![
            row(10, Level::Info, 4, "Created order 8812 for cart c-5521"),
            row(190, Level::Info, 4, "Requesting payment for order 8812"),
            row(
                210,
                Level::Info,
                10,
                "Charging card for order 8812, amount 129.00 EUR",
            ),
            row(
                836,
                Level::Warn,
                10,
                "card-gateway did not answer within 600 ms, retrying (attempt 2 of 2)",
            ),
            row(
                1440,
                Level::Error,
                10,
                "card-gateway did not answer within 600 ms",
            ),
            LogRow {
                exception: true,
                ..row(
                    1782,
                    Level::Error,
                    10,
                    "Charge failed for order 8812: CardGatewayTimeoutException: card-gateway did not respond after 2 attempts",
                )
            },
            row(
                1800,
                Level::Error,
                4,
                "Checkout failed for order 8812: payments-api returned 500",
            ),
        ]
    } else {
        vec![
            row(10, Level::Info, 4, "Created order 8812 for cart c-5521"),
            row(190, Level::Info, 4, "Requesting payment for order 8812"),
            row(
                210,
                Level::Info,
                10,
                "Charging card for order 8812, amount 129.00 EUR",
            ),
            row(840, Level::Info, 10, "Charge accepted for order 8812"),
            row(1788, Level::Info, 4, "Payment completed for order 8812"),
            row(1800, Level::Info, 4, "Checkout completed for order 8812"),
        ]
    }
}

/// A built checkout trace, before it is cut into batches.
struct Checkout {
    trace_id: TraceId,
    /// `(row number, depth, service, span)`; the root has depth 0.
    spans: Vec<(usize, usize, &'static str, Span)>,
    /// `(span number, service, log)`.
    logs: Vec<(usize, &'static str, LogRecord)>,
}

fn build(failure: bool, stream: u64, seed: u64, base: u64) -> Checkout {
    let rows = rows(failure);
    let log_rows = log_rows(failure);

    // IDs are drawn in a fixed order: the trace, then one per table slot.
    let mut rand = Rand::new(seed, stream);
    let trace_id = rand.trace_id();
    let ids: Vec<SpanId> = (0..SLOTS).map(|_| rand.span_id()).collect();
    let id = |n: usize| ids[n - 1];

    let depth = |mut n: usize| {
        let mut depth = 0;
        while let Some(parent) = rows.iter().find(|r| r.n == n).and_then(|r| r.parent) {
            depth += 1;
            n = parent;
        }
        depth
    };

    let spans = rows
        .iter()
        .map(|row| {
            let start = at_ms(base, row.start_ms);
            let mut built = span(
                trace_id,
                id(row.n),
                row.parent.map(id),
                row.name,
                row.kind,
                start,
                at_ms(start, row.dur_ms),
            );
            built.attributes.clone_from(&row.attributes);
            if let Some(message) = row.error {
                set_error(&mut built, message);
            }
            if failure && row.n == 10 {
                built.events.push(exception_event(at_ms(base, 1782)));
            }
            (row.n, depth(row.n), row.service, built)
        })
        .collect();

    let logs = log_rows
        .iter()
        .map(|l| {
            let service = rows
                .iter()
                .find(|r| r.n == l.span)
                .map(|r| r.service)
                .expect("every log is on a span of the table");
            let mut record = log(
                at_ms(base, l.offset_ms),
                l.level,
                l.body,
                Some((trace_id, id(l.span))),
            );
            if l.exception {
                record.attributes = exception_attributes();
            }
            (l.span, service, record)
        })
        .collect();

    Checkout {
        trace_id,
        spans,
        logs,
    }
}

fn exception_attributes() -> Vec<KeyValue> {
    vec![
        str_kv("exception.type", EXCEPTION_TYPE),
        str_kv("exception.message", EXCEPTION_MESSAGE),
        str_kv("exception.stacktrace", EXCEPTION_STACKTRACE),
    ]
}

fn exception_event(time_unix_nano: u64) -> Event {
    Event {
        time_unix_nano,
        name: "exception".to_owned(),
        attributes: exception_attributes(),
        ..Default::default()
    }
}

fn traces_of<'a>(
    spans: impl Iterator<Item = &'a (usize, usize, &'static str, Span)>,
) -> ExportTraceServiceRequest {
    trace_request(
        spans
            .map(|(_, _, service, span)| (*service, span.clone()))
            .collect(),
    )
}

fn logs_of<'a>(
    logs: impl Iterator<Item = &'a (usize, &'static str, LogRecord)>,
) -> Option<ExportLogsServiceRequest> {
    let logs: Vec<_> = logs
        .map(|(_, service, record)| (*service, record.clone()))
        .collect();
    (!logs.is_empty()).then(|| log_request(logs))
}

/// `CheckoutFailure` (`failure`) or `CheckoutSuccess`: the whole trace and
/// its logs in one batch.
pub(crate) fn generate(failure: bool, seed: u64, base: u64) -> Generated {
    let stream = if failure {
        stream::CHECKOUT_FAILURE
    } else {
        stream::CHECKOUT_SUCCESS
    };
    let checkout = build(failure, stream, seed, base);
    Generated {
        batches: vec![Batch {
            traces: Some(traces_of(checkout.spans.iter())),
            logs: logs_of(checkout.logs.iter()),
            delay_before: Duration::ZERO,
        }],
        trace_ids: vec![checkout.trace_id],
    }
}

/// `LateOutOfOrder`: a `CheckoutSuccess` trace in three batches. The spans at
/// depth 3 and deeper go first (the root is depth 0), the middle spans (depth
/// 1 and 2) 50 ms later, and the root another 50 ms after that. A log goes
/// with the batch of its span.
pub(crate) fn late_out_of_order(seed: u64, base: u64) -> Generated {
    let checkout = build(false, stream::LATE_OUT_OF_ORDER, seed, base);
    // (pause before the batch, the depths in it)
    let groups = [
        (Duration::ZERO, 3..=usize::MAX),
        (Duration::from_millis(50), 1..=2),
        (Duration::from_millis(50), 0..=0),
    ];
    let batches = groups
        .into_iter()
        .map(|(delay_before, depths)| {
            let numbers: Vec<usize> = checkout
                .spans
                .iter()
                .filter(|(_, depth, _, _)| depths.contains(depth))
                .map(|(n, _, _, _)| *n)
                .collect();
            Batch {
                traces: Some(traces_of(
                    checkout.spans.iter().filter(|(n, ..)| numbers.contains(n)),
                )),
                logs: logs_of(
                    checkout
                        .logs
                        .iter()
                        .filter(|(span, _, _)| numbers.contains(span)),
                ),
                delay_before,
            }
        })
        .collect();
    Generated {
        batches,
        trace_ids: vec![checkout.trace_id],
    }
}
