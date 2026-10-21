//! `send` against the real OTLP receivers from `loctel-ingest` (port 0 on
//! 127.0.0.1): every transport delivers every scenario, delays are honoured,
//! and failures are errors rather than silent.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use loctel_ingest::{IngestConfig, IngestServers, ListenerState, Sink, start};
use loctel_meshgen::{Scenario, SendReport, Transport, generate, send};
use loctel_model::{LogRecord, Span, StatusCode};

const BASE: u64 = 1_800_000_000_000_000_000;
const SEED: u64 = 11;

#[derive(Default)]
struct RecordingSink {
    spans: Mutex<Vec<Span>>,
    logs: Mutex<Vec<LogRecord>>,
}

impl Sink for RecordingSink {
    fn spans(&self, spans: Vec<Span>) {
        self.spans.lock().unwrap().extend(spans);
    }

    fn logs(&self, logs: Vec<LogRecord>) {
        self.logs.lock().unwrap().extend(logs);
    }
}

struct Running {
    sink: Arc<RecordingSink>,
    _servers: IngestServers,
    grpc: String,
    http: String,
}

impl Running {
    fn transports(&self) -> [Transport; 3] {
        [
            Transport::Grpc(self.grpc.clone()),
            Transport::HttpProtobuf(self.http.clone()),
            Transport::HttpJson(self.http.clone()),
        ]
    }

    fn spans(&self) -> Vec<Span> {
        self.sink.spans.lock().unwrap().clone()
    }

    fn logs(&self) -> Vec<LogRecord> {
        self.sink.logs.lock().unwrap().clone()
    }
}

async fn running() -> Running {
    let loopback = |port| SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let sink = Arc::new(RecordingSink::default());
    let servers = start(
        IngestConfig {
            grpc_addr: loopback(0),
            http_addr: loopback(0),
            ..IngestConfig::default()
        },
        sink.clone(),
    )
    .await;
    let ListenerState::Listening(grpc) = servers.grpc.state else {
        panic!("gRPC listener is not up: {:?}", servers.grpc);
    };
    let ListenerState::Listening(http) = servers.http.state else {
        panic!("HTTP listener is not up: {:?}", servers.http);
    };
    Running {
        sink,
        _servers: servers,
        grpc: format!("http://{grpc}"),
        http: format!("http://{http}"),
    }
}

#[tokio::test]
async fn checkout_failure_reaches_the_receiver_over_every_transport() {
    // A fresh receiver per transport, so what arrived is what was sent.
    for which in 0..3 {
        let receiver = running().await;
        let transport = receiver.transports()[which].clone();
        let g = generate(&Scenario::CheckoutFailure, SEED, BASE);
        let report = send(&g, &transport, 0.0).await.unwrap();
        assert_eq!(report, SendReport { spans: 15, logs: 7 }, "{transport:?}");

        let spans = receiver.spans();
        assert_eq!(spans.len(), 15, "{transport:?}");
        assert_eq!(receiver.logs().len(), 7, "{transport:?}");
        assert!(spans.iter().all(|s| s.trace_id == g.trace_ids[0]));
        assert_eq!(
            spans
                .iter()
                .filter(|s| s.status == StatusCode::Error)
                .count(),
            7,
            "{transport:?}"
        );
        // Exception events and attribute types survive JSON and protobuf.
        let charges = spans.iter().find(|s| s.name == "POST /charges").unwrap();
        assert_eq!(
            charges.exception().unwrap().type_name,
            "Payments.Api.Gateway.CardGatewayTimeoutException"
        );
        let logs = receiver.logs();
        assert_eq!(
            logs.iter().filter(|l| l.exception().is_some()).count(),
            1,
            "{transport:?}"
        );
    }
}

#[tokio::test]
async fn late_out_of_order_keeps_its_pauses_scaled() {
    let receiver = running().await;
    let transport = Transport::HttpProtobuf(receiver.http.clone());
    let g = generate(&Scenario::LateOutOfOrder, SEED, BASE);

    // Real pauses: 50 ms + 50 ms. Only the lower bound is asserted, because
    // a slow machine may take longer but never less.
    let started = Instant::now();
    let report = send(&g, &transport, 1.0).await.unwrap();
    assert!(started.elapsed() >= Duration::from_millis(100));
    assert_eq!(report.spans, 14);

    // Scaled to nothing, everything still arrives, in batch order.
    let receiver = running().await;
    let transport = Transport::HttpJson(receiver.http.clone());
    let report = send(&g, &transport, 0.0).await.unwrap();
    assert_eq!(report.spans, 14);
    let spans = receiver.spans();
    assert_eq!(
        spans.last().unwrap().name,
        "POST /api/checkout",
        "root last"
    );
    assert!(spans[0].parent_span_id.is_some());
}

#[tokio::test]
async fn big_scenarios_go_over_every_transport() {
    // Big enough that the HTTP bodies are gzipped (64 KiB and up) and that
    // there are several batches.
    for which in 0..3 {
        let receiver = running().await;
        let transport = receiver.transports()[which].clone();

        let huge = generate(&Scenario::HugeTrace { spans: 12_000 }, SEED, BASE);
        assert!(huge.batches.len() > 1);
        let report = send(&huge, &transport, 0.0).await.unwrap();
        assert_eq!(
            report,
            SendReport {
                spans: 12_000,
                logs: 0
            },
            "{transport:?}"
        );
        assert_eq!(receiver.spans().len(), 12_000, "{transport:?}");

        let flood = generate(&Scenario::LogFlood { records: 12_000 }, SEED, BASE);
        let report = send(&flood, &transport, 0.0).await.unwrap();
        assert_eq!(
            report,
            SendReport {
                spans: 0,
                logs: 12_000
            },
            "{transport:?}"
        );
        assert_eq!(receiver.logs().len(), 12_000, "{transport:?}");
    }
}

#[tokio::test]
async fn a_bare_host_and_port_and_a_trailing_slash_are_fine() {
    let receiver = running().await;
    let bare = receiver.http.trim_start_matches("http://").to_owned();
    let g = generate(&Scenario::CheckoutSuccess, SEED, BASE);
    for endpoint in [bare, format!("{}/", receiver.http)] {
        send(&g, &Transport::HttpJson(endpoint), 0.0).await.unwrap();
    }
    assert_eq!(receiver.spans().len(), 28, "two sends of 14");
}

#[tokio::test]
async fn rejected_items_are_an_error() {
    let receiver = running().await;
    let mut g = generate(&Scenario::CheckoutSuccess, SEED, BASE);
    // An all-zero trace ID is invalid: the receiver rejects that span and
    // says so in partial_success.
    let request = g.batches[0].traces.as_mut().unwrap();
    request.resource_spans[0].scope_spans[0].spans[0].trace_id = vec![0; 16];

    for transport in receiver.transports() {
        let error = send(&g, &transport, 0.0).await.unwrap_err();
        let text = format!("{error:#}");
        assert!(text.contains("rejected 1 spans"), "{transport:?}: {text}");
    }
}

#[tokio::test]
async fn an_unreachable_endpoint_is_an_error() {
    // A port that was just free and is closed again.
    let closed = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let url = format!("http://127.0.0.1:{closed}");
    let g = generate(&Scenario::CheckoutSuccess, SEED, BASE);
    for transport in [
        Transport::Grpc(url.clone()),
        Transport::HttpProtobuf(url.clone()),
        Transport::HttpJson(url.clone()),
    ] {
        assert!(send(&g, &transport, 0.0).await.is_err(), "{transport:?}");
    }
}

#[tokio::test]
async fn a_bad_time_scale_is_an_error() {
    let receiver = running().await;
    let transport = Transport::HttpProtobuf(receiver.http.clone());
    let g = generate(&Scenario::CheckoutSuccess, SEED, BASE);
    for scale in [-1.0, f64::NAN, f64::INFINITY] {
        assert!(send(&g, &transport, scale).await.is_err(), "{scale}");
    }
    assert!(receiver.spans().is_empty(), "nothing is sent");
}
