//! The fake mesh scenarios (plan Task 8): `CheckoutFailure` is fixed content
//! that tests and screenshots rely on, so its table is spelled out here row by
//! row and compared field by field. The other scenarios are checked against
//! their description in the plan.
//!
//! The first five tests are the plan's own. The rest check what the plan
//! implies: the success tree, the log flood, that the generated data is valid
//! for the real converter, and that the store reaches the verdict the plan
//! states for the failing checkout.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use loctel_ingest::{logs_from_request, spans_from_request};
use loctel_meshgen::checkout::{EXCEPTION_MESSAGE, EXCEPTION_STACKTRACE, EXCEPTION_TYPE};
use loctel_meshgen::{APP_SERVICES, Generated, SERVICES, Scenario, generate};
use loctel_store::{ManualClock, Store, StoreConfig, TraceStatus, trace_exception};
use opentelemetry_proto::tonic::common::v1::KeyValue;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::logs::v1::LogRecord as ProtoLog;
use opentelemetry_proto::tonic::resource::v1::Resource as ProtoResource;
use opentelemetry_proto::tonic::trace::v1::Span as ProtoSpan;
use opentelemetry_proto::tonic::trace::v1::span::SpanKind;
use opentelemetry_proto::tonic::trace::v1::status::StatusCode;
use prost::Message;

const BASE: u64 = 1_800_000_000_000_000_000;
const MS: u64 = 1_000_000;
const SEED: u64 = 7;

// --- helpers ----------------------------------------------------------------

/// A span with the service that emitted it and the batch it travelled in.
struct Sent {
    service: String,
    batch: usize,
    span: ProtoSpan,
}

fn service_name(resource: &Option<ProtoResource>) -> String {
    let resource = resource.as_ref().expect("every export has a resource");
    string_attr(&resource.attributes, "service.name").expect("every resource has service.name")
}

fn spans_of(g: &Generated) -> Vec<Sent> {
    let mut out = Vec::new();
    for (batch, b) in g.batches.iter().enumerate() {
        let Some(request) = &b.traces else { continue };
        for resource_spans in &request.resource_spans {
            let service = service_name(&resource_spans.resource);
            for scope_spans in &resource_spans.scope_spans {
                for span in &scope_spans.spans {
                    out.push(Sent {
                        service: service.clone(),
                        batch,
                        span: span.clone(),
                    });
                }
            }
        }
    }
    out
}

fn logs_of(g: &Generated) -> Vec<(String, ProtoLog)> {
    let mut out = Vec::new();
    for b in &g.batches {
        let Some(request) = &b.logs else { continue };
        for resource_logs in &request.resource_logs {
            let service = service_name(&resource_logs.resource);
            for scope_logs in &resource_logs.scope_logs {
                for record in &scope_logs.log_records {
                    out.push((service.clone(), record.clone()));
                }
            }
        }
    }
    out
}

fn value_of<'a>(attrs: &'a [KeyValue], key: &str) -> Option<&'a Value> {
    attrs
        .iter()
        .find(|kv| kv.key == key)?
        .value
        .as_ref()?
        .value
        .as_ref()
}

fn string_attr(attrs: &[KeyValue], key: &str) -> Option<String> {
    match value_of(attrs, key)? {
        Value::StringValue(s) => Some(s.clone()),
        _ => None,
    }
}

fn int_attr(attrs: &[KeyValue], key: &str) -> Option<i64> {
    match value_of(attrs, key)? {
        Value::IntValue(i) => Some(*i),
        _ => None,
    }
}

/// A string or integer attribute, as text.
fn attr_text(attrs: &[KeyValue], key: &str) -> Option<String> {
    match value_of(attrs, key)? {
        Value::StringValue(s) => Some(s.clone()),
        Value::IntValue(i) => Some(i.to_string()),
        other => panic!("unexpected attribute type for {key}: {other:?}"),
    }
}

fn body_text(record: &ProtoLog) -> String {
    match record
        .body
        .as_ref()
        .and_then(|b| b.value.as_ref())
        .expect("every log has a body")
    {
        Value::StringValue(s) => s.clone(),
        other => panic!("log body is not a string: {other:?}"),
    }
}

fn status_code(span: &ProtoSpan) -> i32 {
    span.status.as_ref().map_or(0, |s| s.code)
}

fn is_error(span: &ProtoSpan) -> bool {
    status_code(span) == StatusCode::Error as i32
}

/// Every byte the scenario would put on the wire, batch by batch.
fn wire_bytes(g: &Generated) -> Vec<(Vec<u8>, Vec<u8>, Duration)> {
    g.batches
        .iter()
        .map(|b| {
            (
                b.traces
                    .as_ref()
                    .map(Message::encode_to_vec)
                    .unwrap_or_default(),
                b.logs
                    .as_ref()
                    .map(Message::encode_to_vec)
                    .unwrap_or_default(),
                b.delay_before,
            )
        })
        .collect()
}

/// The `CheckoutFailure` span table from the plan, row by row.
struct Row {
    n: usize,
    parent: Option<usize>,
    service: &'static str,
    kind: SpanKind,
    name: &'static str,
    start_ms: u64,
    dur_ms: u64,
    error: bool,
    attrs: &'static [(&'static str, &'static str)],
}

#[rustfmt::skip]
const TABLE: [Row; 15] = [
    Row { n: 1,  parent: None,     service: "web-bff",              kind: SpanKind::Server,   name: "POST /api/checkout",     start_ms: 0,    dur_ms: 1840, error: true,  attrs: &[("http.route", "/api/checkout"), ("http.response.status_code", "500")] },
    Row { n: 2,  parent: Some(1),  service: "web-bff",              kind: SpanKind::Internal, name: "validate_token",         start_ms: 4,    dur_ms: 18,   error: false, attrs: &[] },
    Row { n: 3,  parent: Some(1),  service: "web-bff",              kind: SpanKind::Client,   name: "POST",                   start_ms: 30,   dur_ms: 1780, error: true,  attrs: &[("server.address", "orders-api")] },
    Row { n: 4,  parent: Some(3),  service: "orders-api",           kind: SpanKind::Server,   name: "POST /orders",           start_ms: 34,   dur_ms: 1772, error: true,  attrs: &[("http.response.status_code", "500")] },
    Row { n: 5,  parent: Some(4),  service: "orders-api",           kind: SpanKind::Client,   name: "SELECT carts",           start_ms: 40,   dur_ms: 18,   error: false, attrs: &[("db.system.name", "postgresql")] },
    Row { n: 6,  parent: Some(4),  service: "orders-api",           kind: SpanKind::Client,   name: "GET",                    start_ms: 62,   dur_ms: 128,  error: false, attrs: &[("server.address", "inventory-api")] },
    Row { n: 7,  parent: Some(6),  service: "inventory-api",        kind: SpanKind::Server,   name: "GET /stock/{sku}",       start_ms: 66,   dur_ms: 120,  error: false, attrs: &[] },
    Row { n: 8,  parent: Some(7),  service: "inventory-api",        kind: SpanKind::Client,   name: "MGET",                   start_ms: 70,   dur_ms: 4,    error: false, attrs: &[("db.system.name", "redis")] },
    Row { n: 9,  parent: Some(4),  service: "orders-api",           kind: SpanKind::Client,   name: "POST",                   start_ms: 196,  dur_ms: 1594, error: true,  attrs: &[("server.address", "payments-api")] },
    Row { n: 10, parent: Some(9),  service: "payments-api",         kind: SpanKind::Server,   name: "POST /charges",          start_ms: 200,  dur_ms: 1586, error: true,  attrs: &[("order.id", "8812")] },
    Row { n: 11, parent: Some(10), service: "payments-api",         kind: SpanKind::Client,   name: "SELECT payment_methods", start_ms: 206,  dur_ms: 24,   error: false, attrs: &[("db.system.name", "postgresql")] },
    Row { n: 12, parent: Some(10), service: "payments-api",         kind: SpanKind::Client,   name: "POST",                   start_ms: 236,  dur_ms: 600,  error: true,  attrs: &[("server.address", "card-gateway"), ("error.type", "timeout")] },
    Row { n: 13, parent: Some(10), service: "payments-api",         kind: SpanKind::Client,   name: "POST",                   start_ms: 840,  dur_ms: 600,  error: true,  attrs: &[("server.address", "card-gateway"), ("http.request.resend_count", "1")] },
    Row { n: 14, parent: Some(4),  service: "orders-api",           kind: SpanKind::Producer, name: "order.failed publish",   start_ms: 1794, dur_ms: 6,    error: false, attrs: &[("messaging.system", "rabbitmq")] },
    Row { n: 15, parent: Some(14), service: "notifications-worker", kind: SpanKind::Consumer, name: "order.failed process",   start_ms: 1802, dur_ms: 14,   error: false, attrs: &[("messaging.system", "rabbitmq")] },
];

/// The plan's logs: (offset ms, severity number, span #, body).
#[rustfmt::skip]
const LOG_TABLE: [(u64, i32, usize, &str); 7] = [
    (10,   9,  4,  "Created order 8812 for cart c-5521"),
    (190,  9,  4,  "Requesting payment for order 8812"),
    (210,  9,  10, "Charging card for order 8812, amount 129.00 EUR"),
    (836,  13, 10, "card-gateway did not answer within 600 ms, retrying (attempt 2 of 2)"),
    (1440, 17, 10, "card-gateway did not answer within 600 ms"),
    (1782, 17, 10, "Charge failed for order 8812: CardGatewayTimeoutException: card-gateway did not respond after 2 attempts"),
    (1800, 17, 4,  "Checkout failed for order 8812: payments-api returned 500"),
];

/// The span for each table number, found by (service, name, start) so that
/// the order the generator emits them in does not matter.
fn spans_by_number(g: &Generated) -> HashMap<usize, ProtoSpan> {
    let sent = spans_of(g);
    TABLE
        .iter()
        .map(|row| {
            let found: Vec<&Sent> = sent
                .iter()
                .filter(|s| {
                    s.service == row.service
                        && s.span.name == row.name
                        && s.span.start_time_unix_nano == BASE + row.start_ms * MS
                })
                .collect();
            assert_eq!(
                found.len(),
                1,
                "span #{} ({} {} at +{} ms) must appear exactly once",
                row.n,
                row.service,
                row.name,
                row.start_ms
            );
            (row.n, found[0].span.clone())
        })
        .collect()
}

fn store_with(g: &Generated) -> (Store, Arc<ManualClock>) {
    let clock = Arc::new(ManualClock::new(BASE));
    let store = Store::new(StoreConfig::default(), clock.clone());
    for batch in &g.batches {
        if let Some(request) = &batch.traces {
            store.ingest_spans(spans_from_request(request.clone()).items);
        }
        if let Some(request) = &batch.logs {
            store.ingest_logs(logs_from_request(request.clone()).items);
        }
    }
    (store, clock)
}

// --- the plan's tests -------------------------------------------------------

#[test]
fn checkout_failure_is_deterministic() {
    let a = generate(&Scenario::CheckoutFailure, SEED, BASE);
    let b = generate(&Scenario::CheckoutFailure, SEED, BASE);
    assert_eq!(a, b);
    assert_eq!(wire_bytes(&a), wire_bytes(&b), "identical bytes");

    // The inputs matter: another seed gives other IDs, another base time
    // other timestamps, and both leave the shape alone.
    let other_seed = generate(&Scenario::CheckoutFailure, SEED + 1, BASE);
    assert_ne!(a.trace_ids, other_seed.trace_ids);
    assert_ne!(wire_bytes(&a), wire_bytes(&other_seed));
    let other_base = generate(&Scenario::CheckoutFailure, SEED, BASE + 5_000 * MS);
    assert_eq!(a.trace_ids, other_base.trace_ids);
    assert_ne!(wire_bytes(&a), wire_bytes(&other_base));
    let names =
        |g: &Generated| -> Vec<String> { spans_of(g).into_iter().map(|s| s.span.name).collect() };
    assert_eq!(names(&a), names(&other_seed));
    assert_eq!(names(&a), names(&other_base));
}

#[test]
fn checkout_failure_shape() {
    let g = generate(&Scenario::CheckoutFailure, SEED, BASE);
    assert_eq!(g.batches.len(), 1);
    assert_eq!(g.batches[0].delay_before, Duration::ZERO);
    assert_eq!(g.trace_ids.len(), 1);
    let trace = g.trace_ids[0];
    assert!(trace.is_valid());

    let sent = spans_of(&g);
    assert_eq!(sent.len(), 15);
    let by_number = spans_by_number(&g);
    let mut span_ids = HashSet::new();
    for row in &TABLE {
        let span = &by_number[&row.n];
        assert_eq!(span.trace_id, trace.0.to_vec(), "#{} trace id", row.n);
        assert_eq!(span.span_id.len(), 8, "#{} span id", row.n);
        assert!(span.span_id.iter().any(|&b| b != 0), "#{} span id", row.n);
        assert!(
            span_ids.insert(span.span_id.clone()),
            "#{} span id unique",
            row.n
        );
        match row.parent {
            None => assert!(span.parent_span_id.is_empty(), "#{} is the root", row.n),
            Some(parent) => assert_eq!(
                span.parent_span_id, by_number[&parent].span_id,
                "#{} parent is #{parent}",
                row.n
            ),
        }
        assert_eq!(span.kind, row.kind as i32, "#{} kind", row.n);
        assert_eq!(
            span.end_time_unix_nano,
            BASE + (row.start_ms + row.dur_ms) * MS,
            "#{} end",
            row.n
        );
        assert_eq!(is_error(span), row.error, "#{} status", row.n);
        assert_eq!(
            status_code(span),
            if row.error {
                StatusCode::Error as i32
            } else {
                StatusCode::Unset as i32
            },
            "#{} status is Error or Unset, never Ok",
            row.n
        );
        for (key, value) in row.attrs {
            assert_eq!(
                attr_text(&span.attributes, key).as_deref(),
                Some(*value),
                "#{} attribute {key}",
                row.n
            );
        }
        // Only the listed attributes are there.
        assert_eq!(
            span.attributes.len(),
            row.attrs.len(),
            "#{} attributes",
            row.n
        );
        assert_eq!(
            span.events.len(),
            usize::from(row.n == 10),
            "#{} events",
            row.n
        );
    }

    // Numbers stay numbers, so TraceQL comparisons on them work later.
    let status_500 = |n: usize| int_attr(&by_number[&n].attributes, "http.response.status_code");
    assert_eq!(status_500(1), Some(500));
    assert_eq!(status_500(4), Some(500));
    assert_eq!(
        int_attr(&by_number[&13].attributes, "http.request.resend_count"),
        Some(1)
    );

    // Every service has the same kind of resource.
    for s in &sent {
        let resource = g
            .batches
            .iter()
            .filter_map(|b| b.traces.as_ref())
            .flat_map(|r| &r.resource_spans)
            .find(|rs| service_name(&rs.resource) == s.service)
            .and_then(|rs| rs.resource.as_ref())
            .unwrap();
        assert_eq!(
            string_attr(&resource.attributes, "service.version").as_deref(),
            Some("1.4.0")
        );
        assert_eq!(
            string_attr(&resource.attributes, "deployment.environment").as_deref(),
            Some("local")
        );
    }
    let services: HashSet<&str> = sent.iter().map(|s| s.service.as_str()).collect();
    assert_eq!(services, HashSet::from_iter(SERVICES));

    // The exception event on span 10, at +1782 ms.
    let charges = &by_number[&10];
    let event = &charges.events[0];
    assert_eq!(event.name, "exception");
    assert_eq!(event.time_unix_nano, BASE + 1782 * MS);
    assert_eq!(
        string_attr(&event.attributes, "exception.type").as_deref(),
        Some("Payments.Api.Gateway.CardGatewayTimeoutException")
    );
    assert_eq!(
        string_attr(&event.attributes, "exception.message").as_deref(),
        Some("card-gateway did not respond after 2 attempts")
    );
    let stacktrace = string_attr(&event.attributes, "exception.stacktrace").unwrap();
    assert_eq!(
        stacktrace.lines().collect::<Vec<_>>(),
        [
            "at Payments.Api.Gateway.CardGatewayClient.ChargeAsync(ChargeRequest request) in CardGatewayClient.cs:line 64",
            "at Payments.Api.Charges.ChargeService.ChargeAsync(Guid orderId) in ChargeService.cs:line 41",
            "at Payments.Api.Charges.ChargeEndpoints.HandleCharge(ChargeRequest request) in ChargeEndpoints.cs:line 22",
        ]
    );
    // The public constants say the same thing.
    assert_eq!(
        EXCEPTION_TYPE,
        "Payments.Api.Gateway.CardGatewayTimeoutException"
    );
    assert_eq!(
        EXCEPTION_MESSAGE,
        "card-gateway did not respond after 2 attempts"
    );
    assert_eq!(EXCEPTION_STACKTRACE, stacktrace);

    // Logs: 7, four of them on span 10.
    let logs = logs_of(&g);
    assert_eq!(logs.len(), 7);
    let on_charges = logs
        .iter()
        .filter(|(_, l)| l.span_id == charges.span_id)
        .count();
    assert_eq!(on_charges, 4);
    let mut remaining: Vec<_> = logs.iter().collect();
    for (offset_ms, severity, span_n, body) in LOG_TABLE {
        let at = remaining
            .iter()
            .position(|(_, l)| body_text(l) == body)
            .unwrap_or_else(|| panic!("log {body:?} is missing"));
        let (service, log) = remaining.remove(at);
        assert_eq!(log.time_unix_nano, BASE + offset_ms * MS, "{body}");
        assert_eq!(log.severity_number, severity, "{body}");
        assert_eq!(log.trace_id, trace.0.to_vec(), "{body}");
        assert_eq!(log.span_id, by_number[&span_n].span_id, "{body}");
        assert_eq!(*service, TABLE[span_n - 1].service, "{body}");
    }
    assert!(remaining.is_empty());
    // severity text follows the number.
    for (_, log) in &logs {
        let expected = match log.severity_number {
            9 => "INFO",
            13 => "WARN",
            17 => "ERROR",
            n => panic!("unexpected severity {n}"),
        };
        assert_eq!(log.severity_text, expected);
    }
    // The last ERR on span 10 (+1782 ms) carries the exception attributes,
    // and no other log does.
    for (_, log) in &logs {
        let has_exception = string_attr(&log.attributes, "exception.type").is_some();
        assert_eq!(has_exception, log.time_unix_nano == BASE + 1782 * MS);
    }
    let (_, last_err) = logs
        .iter()
        .find(|(_, l)| l.time_unix_nano == BASE + 1782 * MS)
        .unwrap();
    assert_eq!(
        string_attr(&last_err.attributes, "exception.type").as_deref(),
        Some(EXCEPTION_TYPE)
    );
    assert_eq!(
        string_attr(&last_err.attributes, "exception.message").as_deref(),
        Some(EXCEPTION_MESSAGE)
    );
    assert_eq!(
        string_attr(&last_err.attributes, "exception.stacktrace").as_deref(),
        Some(EXCEPTION_STACKTRACE)
    );
}

#[test]
fn late_out_of_order_sends_root_last() {
    let g = generate(&Scenario::LateOutOfOrder, SEED, BASE);
    assert_eq!(g.batches.len(), 3);
    assert_eq!(g.trace_ids.len(), 1);
    assert_eq!(
        g.batches.iter().map(|b| b.delay_before).collect::<Vec<_>>(),
        [
            Duration::ZERO,
            Duration::from_millis(50),
            Duration::from_millis(50)
        ]
    );

    let sent = spans_of(&g);
    assert_eq!(sent.len(), 14, "the CheckoutSuccess tree");
    let in_batch = |i: usize| -> Vec<&Sent> { sent.iter().filter(|s| s.batch == i).collect() };

    // Last batch: the root and nothing else.
    let last = in_batch(2);
    assert_eq!(last.len(), 1);
    assert_eq!(last[0].span.name, "POST /api/checkout");
    assert!(last[0].span.parent_span_id.is_empty());
    // The root is in no earlier batch, and no earlier span lacks a parent.
    for early in sent.iter().filter(|s| s.batch < 2) {
        assert!(!early.span.parent_span_id.is_empty(), "{}", early.span.name);
    }

    // Middle batch: depth 1 and 2 (root = depth 0).
    let mut middle: Vec<&str> = in_batch(1).iter().map(|s| s.span.name.as_str()).collect();
    middle.sort_unstable();
    assert_eq!(middle, ["POST", "POST /orders", "validate_token"]);

    // First batch: depth 3 and deeper, 10 spans.
    assert_eq!(in_batch(0).len(), 10);
    assert!(
        in_batch(0).iter().any(|s| s.span.name == "SELECT carts"),
        "the deepest spans go first"
    );
    assert!(in_batch(0).iter().all(|s| s.span.name != "POST /orders"));

    // Every span is still in the same trace, and every parent exists in the
    // end.
    let ids: HashSet<Vec<u8>> = sent.iter().map(|s| s.span.span_id.clone()).collect();
    for s in &sent {
        assert_eq!(s.span.trace_id, g.trace_ids[0].0.to_vec());
        assert!(s.span.parent_span_id.is_empty() || ids.contains(&s.span.parent_span_id));
    }

    // A log travels with its span's batch: span 10 is in the first batch,
    // span 4 in the second, and the root batch has no logs.
    assert!(g.batches[0].logs.is_some());
    assert!(g.batches[1].logs.is_some());
    assert!(g.batches[2].logs.is_none());
    assert!(g.batches.iter().all(|b| b.traces.is_some()));
}

#[test]
fn huge_trace_has_requested_span_count() {
    let g = generate(&Scenario::HugeTrace { spans: 100_000 }, SEED, BASE);
    assert_eq!(g.trace_ids.len(), 1);
    let sent = spans_of(&g);
    assert_eq!(sent.len(), 100_000);

    let trace = g.trace_ids[0].0.to_vec();
    assert!(sent.iter().all(|s| s.span.trace_id == trace));

    let ids: HashSet<&[u8]> = sent.iter().map(|s| s.span.span_id.as_slice()).collect();
    assert_eq!(ids.len(), 100_000, "span ids are unique");
    let roots = sent
        .iter()
        .filter(|s| s.span.parent_span_id.is_empty())
        .count();
    assert_eq!(roots, 1, "one root");
    for s in &sent {
        assert!(
            s.span.parent_span_id.is_empty() || ids.contains(s.span.parent_span_id.as_slice()),
            "every parent ID is present"
        );
    }

    // Depth is at most 8 (the root is depth 1 here).
    let parent_of: HashMap<&[u8], &[u8]> = sent
        .iter()
        .map(|s| (s.span.span_id.as_slice(), s.span.parent_span_id.as_slice()))
        .collect();
    let mut deepest = 0;
    for s in &sent {
        let mut depth = 1;
        let mut at = s.span.span_id.as_slice();
        while let Some(parent) = parent_of.get(at).filter(|p| !p.is_empty()) {
            depth += 1;
            at = parent;
        }
        deepest = deepest.max(depth);
    }
    assert!(deepest <= 8, "depth {deepest}");
    assert!(deepest >= 3, "a real tree, not a flat list");

    // The four app services take turns.
    let mut per_service: HashMap<&str, usize> = HashMap::new();
    for s in &sent {
        *per_service.entry(s.service.as_str()).or_default() += 1;
    }
    assert_eq!(
        per_service.keys().copied().collect::<HashSet<_>>(),
        HashSet::from_iter(APP_SERVICES)
    );
    assert!(
        per_service.values().all(|&n| n == 25_000),
        "{per_service:?}"
    );

    // The sizes below a full tree level work too, down to a lone root.
    for spans in [1, 2, 9, 11, 101] {
        let g = generate(&Scenario::HugeTrace { spans }, SEED, BASE);
        assert_eq!(spans_of(&g).len(), spans, "{spans} spans");
    }
    let none = generate(&Scenario::HugeTrace { spans: 0 }, SEED, BASE);
    assert!(spans_of(&none).is_empty());
    assert!(none.trace_ids.is_empty());
}

#[test]
fn health_checks_are_get_health_server_spans() {
    let g = generate(&Scenario::HealthChecks { count: 3 }, SEED, BASE);
    let sent = spans_of(&g);
    assert_eq!(sent.len(), 3 * SERVICES.len());
    for s in &sent {
        assert_eq!(s.span.name, "GET /health");
        assert_eq!(s.span.kind, SpanKind::Server as i32);
        assert_eq!(
            string_attr(&s.span.attributes, "url.path").as_deref(),
            Some("/health")
        );
        assert!(s.span.parent_span_id.is_empty(), "each check is a root");
        assert!(!is_error(&s.span));
    }
    for service in SERVICES {
        assert_eq!(
            sent.iter().filter(|s| s.service == service).count(),
            3,
            "{service}"
        );
    }

    // Each check is a trace of its own.
    assert_eq!(g.trace_ids.len(), sent.len());
    let from_spans: HashSet<Vec<u8>> = sent.iter().map(|s| s.span.trace_id.clone()).collect();
    let listed: HashSet<Vec<u8>> = g.trace_ids.iter().map(|t| t.0.to_vec()).collect();
    assert_eq!(from_spans, listed);
    assert_eq!(listed.len(), sent.len());

    // Plus an INF log "GET /health 200" per span, linked to it.
    let logs = logs_of(&g);
    assert_eq!(logs.len(), sent.len());
    for (service, log) in &logs {
        assert_eq!(body_text(log), "GET /health 200");
        assert_eq!(log.severity_number, 9);
        let span = sent
            .iter()
            .find(|s| s.span.span_id == log.span_id)
            .expect("the log's span exists");
        assert_eq!(span.span.trace_id, log.trace_id);
        assert_eq!(&span.service, service);
    }
}

// --- the rest of the scenarios ----------------------------------------------

#[test]
fn checkout_success_is_the_same_tree_without_errors() {
    let g = generate(&Scenario::CheckoutSuccess, SEED, BASE);
    assert_eq!(g.batches.len(), 1);
    assert_eq!(g.trace_ids.len(), 1);
    let sent = spans_of(&g);
    assert_eq!(sent.len(), 14);

    for s in &sent {
        assert_eq!(
            status_code(&s.span),
            StatusCode::Unset as i32,
            "{}",
            s.span.name
        );
        assert!(s.span.events.is_empty(), "no exception events");
        assert!(string_attr(&s.span.attributes, "error.type").is_none());
        assert!(int_attr(&s.span.attributes, "http.request.resend_count").is_none());
    }
    // Exactly one call to the card gateway.
    let gateway_calls = sent
        .iter()
        .filter(|s| {
            string_attr(&s.span.attributes, "server.address").as_deref() == Some("card-gateway")
        })
        .count();
    assert_eq!(gateway_calls, 1);

    // The rest of the tree is the failing one's rows, with the messaging
    // spans renamed.
    let mut names: Vec<(String, String)> = sent
        .iter()
        .map(|s| (s.service.clone(), s.span.name.clone()))
        .collect();
    names.sort();
    let mut expected: Vec<(String, String)> = TABLE
        .iter()
        .filter(|row| row.n != 13)
        .map(|row| {
            let name = match row.n {
                14 => "order.created publish",
                15 => "order.created process",
                _ => row.name,
            };
            (row.service.to_string(), name.to_string())
        })
        .collect();
    expected.sort();
    assert_eq!(names, expected);

    // A successful request answers 200.
    let root = sent
        .iter()
        .find(|s| s.span.name == "POST /api/checkout")
        .unwrap();
    assert_eq!(
        int_attr(&root.span.attributes, "http.response.status_code"),
        Some(200)
    );

    // INF logs only, none with exception attributes.
    let logs = logs_of(&g);
    assert!(!logs.is_empty());
    for (_, log) in &logs {
        assert_eq!(log.severity_number, 9, "{}", body_text(log));
        assert_eq!(log.severity_text, "INFO");
        assert!(string_attr(&log.attributes, "exception.type").is_none());
        assert_eq!(log.trace_id, g.trace_ids[0].0.to_vec());
    }
}

#[test]
fn log_flood_has_requested_records_without_trace_context() {
    let g = generate(&Scenario::LogFlood { records: 25_000 }, SEED, BASE);
    assert!(g.trace_ids.is_empty());
    assert!(g.batches.iter().all(|b| b.traces.is_none()));
    let logs = logs_of(&g);
    assert_eq!(logs.len(), 25_000);

    let mut services = HashSet::new();
    let mut times = HashSet::new();
    let mut levels = HashSet::new();
    for (service, log) in &logs {
        assert!(log.trace_id.is_empty(), "no trace context");
        assert!(log.span_id.is_empty(), "no trace context");
        assert!(
            matches!(log.severity_number, 5 | 9),
            "DBG or INF only, got {}",
            log.severity_number
        );
        levels.insert(log.severity_number);
        services.insert(service.clone());
        times.insert(log.time_unix_nano);
        assert!(log.time_unix_nano >= BASE);
        assert!(!body_text(log).is_empty());
    }
    assert_eq!(levels, HashSet::from([5, 9]), "both levels appear");
    assert_eq!(services, HashSet::from_iter(SERVICES.map(String::from)));
    assert_eq!(
        times.len(),
        25_000,
        "distinct times, so dedup keeps them all"
    );

    // Batches stay a sensible size.
    assert!(g.batches.len() > 1);
    for b in &g.batches {
        let n: usize = b
            .logs
            .iter()
            .flat_map(|r| &r.resource_logs)
            .flat_map(|rl| &rl.scope_logs)
            .map(|sl| sl.log_records.len())
            .sum();
        assert!(n <= 10_000);
    }

    let empty = generate(&Scenario::LogFlood { records: 0 }, SEED, BASE);
    assert!(logs_of(&empty).is_empty());
}

#[test]
fn every_scenario_is_deterministic() {
    for scenario in all_scenarios() {
        let a = generate(&scenario, SEED, BASE);
        let b = generate(&scenario, SEED, BASE);
        assert_eq!(a, b, "{scenario:?}");
        assert_eq!(wire_bytes(&a), wire_bytes(&b), "{scenario:?}");
    }
}

fn all_scenarios() -> Vec<Scenario> {
    vec![
        Scenario::CheckoutFailure,
        Scenario::CheckoutSuccess,
        Scenario::HealthChecks { count: 4 },
        Scenario::LateOutOfOrder,
        Scenario::HugeTrace { spans: 2_500 },
        Scenario::LogFlood { records: 500 },
    ]
}

/// What the real converter and store make of the generated data: nothing
/// rejected, nothing missing, every trace whole once all batches are in.
#[test]
fn every_scenario_is_valid_otlp_and_assembles() {
    let expected = [
        (15, 7),
        (14, 6),
        (4 * SERVICES.len(), 4 * SERVICES.len()),
        (14, 6),
        (2_500, 0),
        (0, 500),
    ];
    for (scenario, (spans, logs)) in all_scenarios().into_iter().zip(expected) {
        let g = generate(&scenario, SEED, BASE);
        let mut converted_spans = 0;
        let mut converted_logs = 0;
        for batch in &g.batches {
            if let Some(request) = &batch.traces {
                let c = spans_from_request(request.clone());
                assert_eq!(c.rejected, 0, "{scenario:?}: {:?}", c.error_message);
                converted_spans += c.items.len();
            }
            if let Some(request) = &batch.logs {
                let c = logs_from_request(request.clone());
                assert_eq!(c.rejected, 0);
                converted_logs += c.items.len();
            }
        }
        assert_eq!(converted_spans, spans, "{scenario:?} spans");
        assert_eq!(converted_logs, logs, "{scenario:?} logs");

        let (store, _clock) = store_with(&g);
        assert_eq!(store.stats().spans, spans, "{scenario:?}");
        assert_eq!(store.stats().logs, logs, "{scenario:?}");
        for id in &g.trace_ids {
            let t = store.trace(*id).expect("the trace is in the store");
            assert!(t.root.is_some(), "{scenario:?}: root present");
            assert!(t.orphans.is_empty(), "{scenario:?}: no missing parents");
        }
    }
}

#[test]
fn checkout_failure_fails_in_the_card_gateway_call() {
    let g = generate(&Scenario::CheckoutFailure, SEED, BASE);
    let (store, _clock) = store_with(&g);
    let by_number = spans_by_number(&g);
    let trace = g.trace_ids[0];

    let summary = store.summary(trace).unwrap();
    assert_eq!(summary.status, TraceStatus::Failed);
    assert_eq!(summary.span_count, 15);
    assert_eq!(summary.root_name.as_deref(), Some("POST /api/checkout"));
    assert_eq!(summary.root_service.as_deref(), Some("web-bff"));
    assert_eq!(summary.duration_nanos, 1840 * MS);
    assert_eq!(
        summary.services,
        [
            "web-bff",
            "orders-api",
            "inventory-api",
            "payments-api",
            "notifications-worker"
        ],
        "five services, in first-seen order"
    );

    // Failed in span #12 (payments-api, POST, peer card-gateway) with +1
    // more (#13), not the leaf #11 or the exception-bearing #10.
    let failed_in = summary.failed_in.unwrap();
    assert_eq!(failed_in.span_id.0.to_vec(), by_number[&12].span_id);
    assert_eq!(failed_in.service, "payments-api");
    assert_eq!(failed_in.span_name, "POST");
    assert_eq!(failed_in.peer.as_deref(), Some("card-gateway"));
    assert_eq!(failed_in.more, 1);

    // The trace's exception comes from #10, the nearest ancestor with one.
    let snapshot = store.trace(trace).unwrap();
    let exception = trace_exception(&snapshot).unwrap();
    assert_eq!(exception.type_name, EXCEPTION_TYPE);
    assert_eq!(exception.message, EXCEPTION_MESSAGE);
    assert_eq!(exception.stacktrace.as_deref(), Some(EXCEPTION_STACKTRACE));
    assert_eq!(
        summary.first_exception.unwrap().type_name,
        EXCEPTION_TYPE,
        "the summary shows it too"
    );
}

#[test]
fn checkout_success_is_ok_and_late_out_of_order_ends_the_same() {
    for scenario in [Scenario::CheckoutSuccess, Scenario::LateOutOfOrder] {
        let g = generate(&scenario, SEED, BASE);
        let (store, _clock) = store_with(&g);
        let summary = store.summary(g.trace_ids[0]).unwrap();
        assert_eq!(summary.status, TraceStatus::Ok, "{scenario:?}");
        assert!(summary.failed_in.is_none());
        assert!(summary.first_exception.is_none());
        assert_eq!(summary.span_count, 14);
    }
}

/// The same seed in two scenarios must not give them the same IDs: a
/// developer who sends `checkout-failure` and then `checkout-success` to one
/// loctel with the default seed expects two traces, not the second one
/// swallowed by the store's duplicate check.
#[test]
fn scenarios_with_the_same_seed_never_share_ids() {
    let mut trace_ids = HashSet::new();
    let mut span_ids = HashSet::new();
    let mut traces = 0;
    let mut spans = 0;
    for scenario in all_scenarios() {
        let g = generate(&scenario, SEED, BASE);
        traces += g.trace_ids.len();
        trace_ids.extend(g.trace_ids.iter().copied());
        for sent in spans_of(&g) {
            spans += 1;
            span_ids.insert(sent.span.span_id);
        }
    }
    assert_eq!(
        trace_ids.len(),
        traces,
        "trace IDs are unique across scenarios"
    );
    assert_eq!(
        span_ids.len(),
        spans,
        "span IDs are unique across scenarios"
    );

    // Another seed is another trace, in the same scenario.
    let a = generate(&Scenario::HugeTrace { spans: 50 }, SEED, BASE);
    let b = generate(&Scenario::HugeTrace { spans: 50 }, SEED + 1, BASE);
    assert_ne!(a.trace_ids, b.trace_ids);
    let ids = |g: &Generated| -> HashSet<Vec<u8>> {
        spans_of(g).into_iter().map(|s| s.span.span_id).collect()
    };
    assert!(ids(&a).is_disjoint(&ids(&b)));
}
