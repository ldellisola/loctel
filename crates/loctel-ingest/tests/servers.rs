//! The real OTLP servers on real sockets (port 0 on 127.0.0.1): gRPC through
//! the generated tonic clients, HTTP through a tiny raw-socket client so that
//! headers, content types and bodies are exactly what the test says.

use std::io::Write as _;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use flate2::Compression;
use flate2::write::GzEncoder;
use loctel_ingest::{
    DEFAULT_MAX_MESSAGE_BYTES, IngestConfig, IngestServers, ListenerState, Sink, start, start_grpc,
    start_http,
};
use loctel_model::{AttrValue, LogRecord, Severity, Span, StatusCode, TraceId, attr};
use opentelemetry_proto::tonic::collector::logs::v1::logs_service_client::LogsServiceClient;
use opentelemetry_proto::tonic::collector::logs::v1::{
    ExportLogsServiceRequest, ExportLogsServiceResponse,
};
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::metrics_service_client::MetricsServiceClient;
use opentelemetry_proto::tonic::collector::trace::v1::trace_service_client::TraceServiceClient;
use opentelemetry_proto::tonic::collector::trace::v1::{
    ExportTracePartialSuccess, ExportTraceServiceRequest, ExportTraceServiceResponse,
};
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::{AnyValue, InstrumentationScope, KeyValue};
use opentelemetry_proto::tonic::logs::v1::{LogRecord as ProtoLog, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::metrics::v1::{ResourceMetrics, ScopeMetrics};
use opentelemetry_proto::tonic::resource::v1::Resource as ProtoResource;
use opentelemetry_proto::tonic::trace::v1::status::StatusCode as ProtoStatus;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span as ProtoSpan, Status};
use prost::Message;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tonic::codec::CompressionEncoding;

const PROTOBUF: &str = "application/x-protobuf";
const JSON: &str = "application/json";
const MIB: usize = 1024 * 1024;

// --- recording sink ---------------------------------------------------------

#[derive(Default)]
struct RecordingSink {
    spans: Mutex<Vec<Span>>,
    logs: Mutex<Vec<LogRecord>>,
    calls: AtomicUsize,
}

impl RecordingSink {
    fn spans(&self) -> Vec<Span> {
        self.spans.lock().unwrap().clone()
    }

    fn logs(&self) -> Vec<LogRecord> {
        self.logs.lock().unwrap().clone()
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Sink for RecordingSink {
    fn spans(&self, spans: Vec<Span>) {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.spans.lock().unwrap().extend(spans);
    }

    fn logs(&self, logs: Vec<LogRecord>) {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.logs.lock().unwrap().extend(logs);
    }
}

// --- server setup -----------------------------------------------------------

fn loopback(port: u16) -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
}

fn config() -> IngestConfig {
    IngestConfig {
        grpc_addr: loopback(0),
        http_addr: loopback(0),
        ..IngestConfig::default()
    }
}

struct Running {
    sink: Arc<RecordingSink>,
    servers: IngestServers,
    grpc: SocketAddr,
    http: SocketAddr,
}

async fn running() -> Running {
    let sink = Arc::new(RecordingSink::default());
    let servers = start(config(), sink.clone()).await;
    let ListenerState::Listening(grpc) = servers.grpc.state else {
        panic!("gRPC listener is not up: {:?}", servers.grpc);
    };
    let ListenerState::Listening(http) = servers.http.state else {
        panic!("HTTP listener is not up: {:?}", servers.http);
    };
    Running {
        sink,
        servers,
        grpc,
        http,
    }
}

// --- request builders -------------------------------------------------------

fn kv_str(key: &str, value: &str) -> KeyValue {
    KeyValue {
        key: key.to_owned(),
        value: Some(AnyValue {
            value: Some(Value::StringValue(value.to_owned())),
        }),
        ..Default::default()
    }
}

fn resource(service: &str) -> ProtoResource {
    ProtoResource {
        attributes: vec![kv_str("service.name", service)],
        ..Default::default()
    }
}

fn trace_id(n: u8) -> Vec<u8> {
    let mut id = vec![0u8; 16];
    id[15] = n;
    id
}

fn proto_span(trace: u8, span: u64, name: &str) -> ProtoSpan {
    ProtoSpan {
        trace_id: trace_id(trace),
        span_id: span.to_be_bytes().to_vec(),
        name: name.to_owned(),
        start_time_unix_nano: 1_000,
        end_time_unix_nano: 2_000,
        ..Default::default()
    }
}

fn trace_request(spans: Vec<ProtoSpan>) -> ExportTraceServiceRequest {
    ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: Some(resource("orders-api")),
            scope_spans: vec![ScopeSpans {
                scope: Some(InstrumentationScope {
                    name: "scope".to_owned(),
                    ..Default::default()
                }),
                spans,
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}

fn one_span_request() -> ExportTraceServiceRequest {
    let mut span = proto_span(1, 1, "POST /orders");
    span.status = Some(Status {
        code: ProtoStatus::Error as i32,
        message: "boom".to_owned(),
    });
    trace_request(vec![span])
}

/// A request with one valid span and one with an all-zero trace ID.
fn mixed_request() -> ExportTraceServiceRequest {
    let mut bad = proto_span(1, 2, "bad");
    bad.trace_id = vec![0u8; 16];
    trace_request(vec![proto_span(1, 1, "good"), bad])
}

/// At least 10 MiB of encoded spans (about 1 KiB of attribute text each), far
/// above the 4 MiB (tonic) and 2 MB (axum) library defaults.
fn big_request() -> (ExportTraceServiceRequest, usize) {
    let padding = "x".repeat(1000);
    let count = 10_500;
    let spans = (0..count)
        .map(|i| {
            let mut span = proto_span(7, i as u64 + 1, "bulk");
            span.attributes = vec![kv_str("padding", &padding)];
            span
        })
        .collect();
    let request = trace_request(spans);
    assert!(
        request.encoded_len() > 10 * MIB,
        "test input is too small: {} bytes",
        request.encoded_len()
    );
    (request, count)
}

fn log_request() -> ExportLogsServiceRequest {
    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(resource("payments-api")),
            scope_logs: vec![ScopeLogs {
                log_records: vec![ProtoLog {
                    time_unix_nano: 5_000,
                    severity_number: 17,
                    body: Some(AnyValue {
                        value: Some(Value::StringValue("card declined".to_owned())),
                    }),
                    trace_id: trace_id(1),
                    span_id: 9u64.to_be_bytes().to_vec(),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}

fn metrics_request() -> ExportMetricsServiceRequest {
    ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            resource: Some(resource("orders-api")),
            scope_metrics: vec![ScopeMetrics::default()],
            ..Default::default()
        }],
    }
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

// --- raw HTTP client --------------------------------------------------------

struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    fn content_type(&self) -> &str {
        self.header("content-type").unwrap_or_default()
    }

    fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("body is not JSON ({e}): {:?}", self.body))
    }
}

/// POSTs `body` with a `Content-Length` and reads the whole reply
/// (`Connection: close`).
async fn post(addr: SocketAddr, path: &str, headers: &[(&str, &str)], body: &[u8]) -> Reply {
    send(addr, path, headers, Framing::ContentLength, body).await
}

/// Like [`post`], but streams the body with `Transfer-Encoding: chunked`, as
/// exporters that compress on the fly do (the Go exporter, for one).
async fn post_chunked(
    addr: SocketAddr,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Reply {
    send(addr, path, headers, Framing::Chunked, body).await
}

#[derive(Clone, Copy)]
enum Framing {
    ContentLength,
    Chunked,
}

async fn send(
    addr: SocketAddr,
    path: &str,
    headers: &[(&str, &str)],
    framing: Framing,
    body: &[u8],
) -> Reply {
    let mut head = format!("POST {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
    match framing {
        Framing::ContentLength => head.push_str(&format!("Content-Length: {}\r\n", body.len())),
        Framing::Chunked => head.push_str("Transfer-Encoding: chunked\r\n"),
    }
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");

    let stream = TcpStream::connect(addr).await.unwrap();
    let (mut reader, mut writer) = stream.into_split();
    // Write and read at the same time: a server that rejects the body early
    // answers and hangs up while the client is still sending.
    let write = async {
        // Write errors are expected then, so they are ignored; a missing
        // reply fails the test below.
        let _ = writer.write_all(head.as_bytes()).await;
        match framing {
            Framing::ContentLength => {
                let _ = writer.write_all(body).await;
            }
            Framing::Chunked => {
                for chunk in body.chunks(1000) {
                    let framed =
                        [format!("{:x}\r\n", chunk.len()).as_bytes(), chunk, b"\r\n"].concat();
                    if writer.write_all(&framed).await.is_err() {
                        return writer;
                    }
                }
                let _ = writer.write_all(b"0\r\n\r\n").await;
            }
        }
        // Hold the write half open until the reply is in.
        writer
    };
    let read = async {
        let mut raw = Vec::new();
        // A reset after the reply is not a failure; what was read is kept.
        let _ = reader.read_to_end(&mut raw).await;
        raw
    };
    let (_writer, raw) =
        tokio::time::timeout(Duration::from_secs(60), async { tokio::join!(write, read) })
            .await
            .expect("HTTP request timed out");
    assert!(!raw.is_empty(), "the server sent no reply");
    parse_reply(&raw)
}

fn parse_reply(raw: &[u8]) -> Reply {
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("reply has a header block");
    let head = std::str::from_utf8(&raw[..split]).unwrap();
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("bad status line: {status_line}"));
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_owned(), value.trim().to_owned()))
        .collect();
    let body = &raw[split + 4..];
    let chunked = headers
        .iter()
        .any(|(k, v)| k.eq_ignore_ascii_case("transfer-encoding") && v.contains("chunked"));
    let body = if chunked {
        dechunk(body)
    } else {
        body.to_vec()
    };
    Reply {
        status,
        headers,
        body,
    }
}

fn dechunk(mut rest: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let line_end = rest
            .windows(2)
            .position(|window| window == b"\r\n")
            .expect("chunk size line");
        let size =
            usize::from_str_radix(std::str::from_utf8(&rest[..line_end]).unwrap().trim(), 16)
                .expect("chunk size");
        rest = &rest[line_end + 2..];
        if size == 0 {
            return out;
        }
        out.extend_from_slice(&rest[..size]);
        rest = &rest[size + 2..];
    }
}

fn grpc_url(addr: SocketAddr) -> String {
    format!("http://{addr}")
}

// --- gRPC -------------------------------------------------------------------

#[tokio::test]
async fn grpc_export_reaches_sink() {
    let up = running().await;
    let mut traces = TraceServiceClient::connect(grpc_url(up.grpc))
        .await
        .unwrap();
    let reply = traces
        .export(one_span_request())
        .await
        .unwrap()
        .into_inner();
    assert_eq!(reply.partial_success, None);

    let spans = up.sink.spans();
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].name, "POST /orders");
    assert_eq!(spans[0].service(), "orders-api");
    assert_eq!(spans[0].status, StatusCode::Error);
    assert_eq!(
        spans[0].trace_id,
        TraceId::from_bytes(&trace_id(1)).unwrap()
    );

    let mut logs = LogsServiceClient::connect(grpc_url(up.grpc)).await.unwrap();
    let reply = logs.export(log_request()).await.unwrap().into_inner();
    assert_eq!(reply.partial_success, None);
    let records = up.sink.logs();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].body, "card declined");
    assert_eq!(records[0].severity, Severity::Error);
    assert_eq!(records[0].service(), "payments-api");
}

#[tokio::test]
async fn grpc_large_request_accepted() {
    let up = running().await;
    let (request, count) = big_request();
    let mut traces = TraceServiceClient::connect(grpc_url(up.grpc))
        .await
        .unwrap();
    traces.export(request).await.unwrap();
    assert_eq!(up.sink.spans().len(), count);
}

#[tokio::test]
async fn grpc_gzip_large_request_accepted() {
    let up = running().await;
    let (request, count) = big_request();
    let mut traces = TraceServiceClient::connect(grpc_url(up.grpc))
        .await
        .unwrap()
        .send_compressed(CompressionEncoding::Gzip);
    traces.export(request).await.unwrap();
    assert_eq!(up.sink.spans().len(), count);
}

#[tokio::test]
async fn grpc_message_over_the_limit_is_refused() {
    let sink = Arc::new(RecordingSink::default());
    let servers = start(
        IngestConfig {
            max_message_bytes: 64 * 1024,
            ..config()
        },
        sink.clone(),
    )
    .await;
    let ListenerState::Listening(grpc) = servers.grpc.state else {
        panic!("gRPC is not up: {:?}", servers.grpc);
    };
    let mut traces = TraceServiceClient::connect(grpc_url(grpc)).await.unwrap();

    let (request, _) = big_request();
    let refused = traces.export(request).await.unwrap_err();
    assert_eq!(refused.code(), tonic::Code::OutOfRange, "{refused:?}");
    assert!(sink.spans().is_empty());

    // The limit is the configured one, not a lower default: a request under
    // it still goes through on the same channel.
    traces.export(one_span_request()).await.unwrap();
    assert_eq!(sink.spans().len(), 1);
    drop(servers);
}

#[tokio::test]
async fn grpc_partial_success_is_reported() {
    let up = running().await;
    let mut traces = TraceServiceClient::connect(grpc_url(up.grpc))
        .await
        .unwrap();
    let reply = traces.export(mixed_request()).await.unwrap().into_inner();
    let partial = reply.partial_success.expect("partial_success is set");
    assert_eq!(partial.rejected_spans, 1);
    assert!(!partial.error_message.is_empty());
    assert_eq!(up.sink.spans().len(), 1);
}

// --- HTTP -------------------------------------------------------------------

#[tokio::test]
async fn http_protobuf_export_reaches_sink() {
    let up = running().await;

    let reply = post(
        up.http,
        "/v1/traces",
        &[("Content-Type", PROTOBUF)],
        &one_span_request().encode_to_vec(),
    )
    .await;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.content_type(), PROTOBUF);
    let decoded = ExportTraceServiceResponse::decode(reply.body.as_slice()).unwrap();
    assert_eq!(decoded.partial_success, None);
    let spans = up.sink.spans();
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].name, "POST /orders");
    assert_eq!(spans[0].service(), "orders-api");

    let reply = post(
        up.http,
        "/v1/logs",
        &[("Content-Type", PROTOBUF)],
        &log_request().encode_to_vec(),
    )
    .await;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.content_type(), PROTOBUF);
    let decoded = ExportLogsServiceResponse::decode(reply.body.as_slice()).unwrap();
    assert_eq!(decoded.partial_success, None);
    assert_eq!(up.sink.logs().len(), 1);
}

const JSON_TRACE: &str = r#"{
  "resourceSpans": [{
    "resource": {"attributes": [{"key": "service.name", "value": {"stringValue": "json-svc"}}]},
    "scopeSpans": [{
      "scope": {"name": "json-scope"},
      "spans": [{
        "traceId": "4bf92f3577b34da6a3ce929d0e0e4736",
        "spanId": "00f067aa0ba902b7",
        "parentSpanId": "",
        "name": "GET /json",
        "kind": 2,
        "startTimeUnixNano": "1700000000000000000",
        "endTimeUnixNano": "1700000000100000000",
        "attributes": [{"key": "http.status_code", "value": {"intValue": "500"}}],
        "status": {"code": 2, "message": "bad"},
        "someFieldFromTheFuture": {"ignored": true}
      }]
    }]
  }]
}"#;

#[tokio::test]
async fn http_json_export_reaches_sink() {
    let up = running().await;

    let reply = post(
        up.http,
        "/v1/traces",
        &[("Content-Type", JSON)],
        JSON_TRACE.as_bytes(),
    )
    .await;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.content_type(), JSON);
    assert_eq!(reply.json(), serde_json::json!({}));

    let spans = up.sink.spans();
    assert_eq!(spans.len(), 1);
    let span = &spans[0];
    assert_eq!(
        span.trace_id,
        TraceId::from_hex("4bf92f3577b34da6a3ce929d0e0e4736").unwrap()
    );
    assert_eq!(span.span_id.to_string(), "00f067aa0ba902b7");
    assert_eq!(span.parent_span_id, None);
    assert_eq!(span.name, "GET /json");
    assert_eq!(span.service(), "json-svc");
    assert_eq!(span.scope_name, "json-scope");
    assert_eq!(span.start_unix_nano, 1_700_000_000_000_000_000);
    assert_eq!(span.status, StatusCode::Error);
    assert_eq!(
        attr(&span.attributes, "http.status_code"),
        Some(&AttrValue::Int(500))
    );

    let json_logs = r#"{"resourceLogs":[{"resource":{"attributes":[
        {"key":"service.name","value":{"stringValue":"json-svc"}}]},
        "scopeLogs":[{"logRecords":[{
            "timeUnixNano":"1700000000000000000","severityNumber":9,
            "body":{"stringValue":"hello"},
            "traceId":"4bf92f3577b34da6a3ce929d0e0e4736","spanId":"00f067aa0ba902b7"}]}]}]}"#;
    let reply = post(
        up.http,
        "/v1/logs",
        &[("Content-Type", JSON)],
        json_logs.as_bytes(),
    )
    .await;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.content_type(), JSON);
    let logs = up.sink.logs();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].body, "hello");
    assert_eq!(logs[0].severity, Severity::Info);
    assert_eq!(
        logs[0].span_id.map(|id| id.to_string()).as_deref(),
        Some("00f067aa0ba902b7")
    );
}

#[tokio::test]
async fn http_content_type_parameters_are_ignored() {
    let up = running().await;
    let reply = post(
        up.http,
        "/v1/traces",
        &[("Content-Type", "Application/JSON; charset=utf-8")],
        JSON_TRACE.as_bytes(),
    )
    .await;
    assert_eq!(reply.status, 200);
    assert!(
        reply.content_type().starts_with(JSON),
        "{}",
        reply.content_type()
    );
    assert_eq!(up.sink.spans().len(), 1);
}

#[tokio::test]
async fn http_gzip_large_request_accepted() {
    let up = running().await;
    let (request, count) = big_request();
    let compressed = gzip(&request.encode_to_vec());

    let reply = post(
        up.http,
        "/v1/traces",
        &[("Content-Type", PROTOBUF), ("Content-Encoding", "gzip")],
        &compressed,
    )
    .await;

    assert_eq!(reply.status, 200);
    assert_eq!(up.sink.spans().len(), count);
}

#[tokio::test]
async fn http_uncompressed_large_request_accepted() {
    let up = running().await;
    let (request, count) = big_request();
    let reply = post(
        up.http,
        "/v1/traces",
        &[("Content-Type", PROTOBUF)],
        &request.encode_to_vec(),
    )
    .await;
    assert_eq!(reply.status, 200);
    assert_eq!(up.sink.spans().len(), count);
}

#[tokio::test]
async fn http_gzip_json_request_accepted() {
    let up = running().await;
    let reply = post(
        up.http,
        "/v1/traces",
        &[("Content-Type", JSON), ("Content-Encoding", "gzip")],
        &gzip(JSON_TRACE.as_bytes()),
    )
    .await;
    assert_eq!(reply.status, 200);
    assert_eq!(up.sink.spans().len(), 1);
}

#[tokio::test]
async fn http_chunked_gzip_request_accepted() {
    let up = running().await;
    let (request, count) = big_request();
    let reply = post_chunked(
        up.http,
        "/v1/traces",
        &[("Content-Type", PROTOBUF), ("Content-Encoding", "gzip")],
        &gzip(&request.encode_to_vec()),
    )
    .await;
    assert_eq!(reply.status, 200);
    assert_eq!(up.sink.spans().len(), count);
}

#[tokio::test]
async fn http_chunked_request_over_the_limit_is_413() {
    let sink = Arc::new(RecordingSink::default());
    let servers = start(
        IngestConfig {
            max_message_bytes: 64 * 1024,
            ..config()
        },
        sink.clone(),
    )
    .await;
    let ListenerState::Listening(http) = servers.http.state else {
        panic!("HTTP is not up: {:?}", servers.http);
    };
    // No Content-Length to check up front, so the limit has to bite while the
    // body streams in.
    let padding = "x".repeat(200 * 1024);
    let too_big = trace_request(vec![{
        let mut span = proto_span(1, 1, "big");
        span.attributes = vec![kv_str("padding", &padding)];
        span
    }])
    .encode_to_vec();
    let reply = post_chunked(http, "/v1/traces", &[("Content-Type", PROTOBUF)], &too_big).await;
    assert_eq!(reply.status, 413);
    assert!(sink.spans().is_empty());
    drop(servers);
}

#[tokio::test]
async fn metrics_are_accepted_and_dropped() {
    let up = running().await;

    let reply = post(
        up.http,
        "/v1/metrics",
        &[("Content-Type", PROTOBUF)],
        &metrics_request().encode_to_vec(),
    )
    .await;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.content_type(), PROTOBUF);

    let reply = post(
        up.http,
        "/v1/metrics",
        &[("Content-Type", JSON)],
        br#"{"resourceMetrics":[{"scopeMetrics":[]}]}"#,
    )
    .await;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.content_type(), JSON);
    assert_eq!(reply.json(), serde_json::json!({}));

    let mut metrics = MetricsServiceClient::connect(grpc_url(up.grpc))
        .await
        .unwrap();
    let reply = metrics
        .export(metrics_request())
        .await
        .unwrap()
        .into_inner();
    assert_eq!(reply.partial_success, None);

    assert_eq!(up.sink.calls(), 0, "the sink sees nothing");
    assert!(up.sink.spans().is_empty());
    assert!(up.sink.logs().is_empty());
}

#[tokio::test]
async fn port_in_use_reports_failed_listener() {
    // Hold a listening socket so that the HTTP port is taken.
    let blocker = std::net::TcpListener::bind(loopback(0)).unwrap();
    let taken = blocker.local_addr().unwrap();
    let sink = Arc::new(RecordingSink::default());

    let servers = start(
        IngestConfig {
            http_addr: taken,
            ..config()
        },
        sink.clone(),
    )
    .await;

    assert_eq!(servers.http.name, "otlp-http");
    assert_eq!(servers.http.requested, taken);
    match &servers.http.state {
        ListenerState::Failed(message) => assert!(!message.is_empty()),
        other => panic!("HTTP should have failed, got {other:?}"),
    }
    assert_eq!(servers.grpc.name, "otlp-grpc");
    let ListenerState::Listening(grpc) = servers.grpc.state else {
        panic!("gRPC should be listening: {:?}", servers.grpc);
    };

    // The other listener keeps working.
    let mut traces = TraceServiceClient::connect(grpc_url(grpc)).await.unwrap();
    traces.export(one_span_request()).await.unwrap();
    assert_eq!(sink.spans().len(), 1);
    drop(blocker);
}

#[tokio::test]
async fn grpc_port_in_use_keeps_http_listening() {
    let blocker = std::net::TcpListener::bind(loopback(0)).unwrap();
    let taken = blocker.local_addr().unwrap();
    let sink = Arc::new(RecordingSink::default());

    let servers = start(
        IngestConfig {
            grpc_addr: taken,
            ..config()
        },
        sink.clone(),
    )
    .await;

    assert!(matches!(servers.grpc.state, ListenerState::Failed(_)));
    assert_eq!(servers.grpc.requested, taken);
    let ListenerState::Listening(http) = servers.http.state else {
        panic!("HTTP should be listening: {:?}", servers.http);
    };
    let reply = post(
        http,
        "/v1/traces",
        &[("Content-Type", PROTOBUF)],
        &one_span_request().encode_to_vec(),
    )
    .await;
    assert_eq!(reply.status, 200);
    assert_eq!(sink.spans().len(), 1);
    drop(blocker);
}

#[tokio::test]
async fn partial_success_reported_over_http() {
    let up = running().await;

    // Protobuf in, protobuf out.
    let reply = post(
        up.http,
        "/v1/traces",
        &[("Content-Type", PROTOBUF)],
        &mixed_request().encode_to_vec(),
    )
    .await;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.content_type(), PROTOBUF);
    let decoded = ExportTraceServiceResponse::decode(reply.body.as_slice()).unwrap();
    let ExportTracePartialSuccess {
        rejected_spans,
        error_message,
    } = decoded.partial_success.expect("partial_success is set");
    assert_eq!(rejected_spans, 1);
    assert!(error_message.contains("trace_id"), "{error_message}");
    assert_eq!(up.sink.spans().len(), 1, "the valid span still arrives");

    // JSON in, JSON out: int64 as a string.
    let json = serde_json::to_vec(&mixed_request()).unwrap();
    let reply = post(up.http, "/v1/traces", &[("Content-Type", JSON)], &json).await;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.content_type(), JSON);
    let body = reply.json();
    assert_eq!(body["partialSuccess"]["rejectedSpans"], "1");
    assert!(
        body["partialSuccess"]["errorMessage"]
            .as_str()
            .is_some_and(|message| message.contains("trace_id")),
        "{body}"
    );
    assert_eq!(up.sink.spans().len(), 2);

    // A request in which every span is invalid never reaches the sink.
    let calls = up.sink.calls();
    let mut only_bad = mixed_request();
    only_bad.resource_spans[0].scope_spans[0].spans.remove(0);
    let reply = post(
        up.http,
        "/v1/traces",
        &[("Content-Type", PROTOBUF)],
        &only_bad.encode_to_vec(),
    )
    .await;
    assert_eq!(reply.status, 200);
    let decoded = ExportTraceServiceResponse::decode(reply.body.as_slice()).unwrap();
    assert_eq!(decoded.partial_success.unwrap().rejected_spans, 1);
    assert_eq!(up.sink.calls(), calls);
}

#[tokio::test]
async fn http_unsupported_content_type_is_415() {
    let up = running().await;
    for content_type in ["text/plain", "application/xml"] {
        let reply = post(
            up.http,
            "/v1/traces",
            &[("Content-Type", content_type)],
            b"hello",
        )
        .await;
        assert_eq!(reply.status, 415, "{content_type}");
    }
    // A missing Content-Type is just as unusable.
    let reply = post(up.http, "/v1/traces", &[], b"hello").await;
    assert_eq!(reply.status, 415);
    assert!(up.sink.spans().is_empty());
}

#[tokio::test]
async fn http_unsupported_content_encoding_is_rejected() {
    let up = running().await;
    let reply = post(
        up.http,
        "/v1/traces",
        &[("Content-Type", PROTOBUF), ("Content-Encoding", "compress")],
        &one_span_request().encode_to_vec(),
    )
    .await;
    assert_eq!(reply.status, 415);
    assert!(up.sink.spans().is_empty());
}

#[tokio::test]
async fn http_malformed_body_is_400_with_a_status_message() {
    let up = running().await;

    let reply = post(
        up.http,
        "/v1/traces",
        &[("Content-Type", PROTOBUF)],
        &[0xff, 0xff, 0xff, 0xff],
    )
    .await;
    assert_eq!(reply.status, 400);
    assert_eq!(reply.content_type(), PROTOBUF);
    assert!(!reply.body.is_empty(), "a google.rpc.Status explains it");

    let reply = post(
        up.http,
        "/v1/logs",
        &[("Content-Type", JSON)],
        b"{ not json",
    )
    .await;
    assert_eq!(reply.status, 400);
    assert_eq!(reply.content_type(), JSON);
    let body = reply.json();
    assert_eq!(body["code"], 3);
    assert!(body["message"].as_str().is_some_and(|m| !m.is_empty()));

    // A truncated gzip stream is a bad request, not a crash.
    let mut broken = gzip(&one_span_request().encode_to_vec());
    broken.truncate(broken.len() / 2);
    let reply = post(
        up.http,
        "/v1/traces",
        &[("Content-Type", PROTOBUF), ("Content-Encoding", "gzip")],
        &broken,
    )
    .await;
    assert_eq!(reply.status, 400);
    assert!(up.sink.spans().is_empty());
}

#[tokio::test]
async fn http_body_over_the_limit_is_413() {
    const LIMIT: usize = 256 * 1024;
    let sink = Arc::new(RecordingSink::default());
    let servers = start(
        IngestConfig {
            max_message_bytes: LIMIT,
            ..config()
        },
        sink.clone(),
    )
    .await;
    let ListenerState::Listening(http) = servers.http.state else {
        panic!("HTTP is not up: {:?}", servers.http);
    };

    // Too big as sent.
    let padding = "x".repeat(LIMIT + 1024);
    let too_big = trace_request(vec![{
        let mut span = proto_span(1, 1, "big");
        span.attributes = vec![kv_str("padding", &padding)];
        span
    }])
    .encode_to_vec();
    assert!(too_big.len() > LIMIT);
    let reply = post(http, "/v1/traces", &[("Content-Type", PROTOBUF)], &too_big).await;
    assert_eq!(reply.status, 413);

    // Small on the wire, but far over the limit once inflated (a gzip bomb).
    let (request, _) = big_request();
    let bomb = gzip(&request.encode_to_vec());
    assert!(
        bomb.len() < LIMIT,
        "bomb is {} bytes on the wire",
        bomb.len()
    );
    let reply = post(
        http,
        "/v1/traces",
        &[("Content-Type", PROTOBUF), ("Content-Encoding", "gzip")],
        &bomb,
    )
    .await;
    assert_eq!(reply.status, 413);

    assert!(sink.spans().is_empty());
    drop(servers);
}

#[tokio::test]
async fn shutdown_stops_both_listeners() {
    let up = running().await;
    let (grpc, http) = (up.grpc, up.http);
    TcpStream::connect(grpc).await.unwrap();
    TcpStream::connect(http).await.unwrap();

    up.servers.shutdown();

    for addr in [grpc, http] {
        let mut closed = false;
        for _ in 0..100 {
            if TcpStream::connect(addr).await.is_err() {
                closed = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(closed, "{addr} still accepts connections after shutdown");
    }
}

#[tokio::test]
async fn shutdown_closes_idle_keep_alive_connections() {
    let up = running().await;

    // A keep-alive connection that has finished a request and sits idle.
    let mut stream = TcpStream::connect(up.http).await.unwrap();
    let body = one_span_request().encode_to_vec();
    let head = format!(
        "POST /v1/traces HTTP/1.1\r\nHost: loctel\r\nContent-Type: {PROTOBUF}\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await.unwrap();
    stream.write_all(&body).await.unwrap();
    let mut buf = vec![0u8; 4096];
    let n = stream.read(&mut buf).await.unwrap();
    assert!(
        buf[..n].starts_with(b"HTTP/1.1 200"),
        "{:?}",
        String::from_utf8_lossy(&buf[..n])
    );

    up.servers.shutdown();

    // The server hangs up on it instead of waiting for it forever.
    let closed = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf))
        .await
        .expect("the idle connection was not closed");
    assert!(matches!(closed, Ok(0) | Err(_)), "{closed:?}");
}

// --- one receiver at a time ---------------------------------------------------

/// Waits (up to 5 s) for `addr` to stop accepting connections.
async fn wait_until_closed(addr: SocketAddr) {
    for _ in 0..100 {
        if TcpStream::connect(addr).await.is_err() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{addr} still accepts connections");
}

#[tokio::test]
async fn one_receiver_stops_while_the_other_keeps_serving() {
    let sink = Arc::new(RecordingSink::default());
    let (grpc, grpc_stop) = start_grpc(loopback(0), sink.clone(), DEFAULT_MAX_MESSAGE_BYTES).await;
    let (http, http_stop) = start_http(loopback(0), sink.clone(), DEFAULT_MAX_MESSAGE_BYTES).await;
    assert_eq!((grpc.name, http.name), ("otlp-grpc", "otlp-http"));
    let (ListenerState::Listening(grpc), ListenerState::Listening(http)) = (grpc.state, http.state)
    else {
        panic!("both receivers should be listening");
    };

    // Each serves OTLP on its own.
    let mut traces = TraceServiceClient::connect(grpc_url(grpc)).await.unwrap();
    traces.export(one_span_request()).await.unwrap();
    let reply = post(
        http,
        "/v1/traces",
        &[("Content-Type", PROTOBUF)],
        &one_span_request().encode_to_vec(),
    )
    .await;
    assert_eq!(reply.status, 200);
    assert_eq!(sink.spans().len(), 2);

    // Stopping the gRPC receiver leaves HTTP up.
    grpc_stop.shutdown();
    wait_until_closed(grpc).await;
    let reply = post(
        http,
        "/v1/traces",
        &[("Content-Type", PROTOBUF)],
        &one_span_request().encode_to_vec(),
    )
    .await;
    assert_eq!(reply.status, 200);
    assert_eq!(sink.spans().len(), 3);

    // Dropping the handle stops a receiver too.
    drop(http_stop);
    wait_until_closed(http).await;
}

#[tokio::test]
async fn a_receiver_on_a_busy_port_reports_failed() {
    let blocker = std::net::TcpListener::bind(loopback(0)).unwrap();
    let taken = blocker.local_addr().unwrap();
    let sink = Arc::new(RecordingSink::default());

    let (grpc, grpc_stop) = start_grpc(taken, sink.clone(), DEFAULT_MAX_MESSAGE_BYTES).await;
    let (http, http_stop) = start_http(taken, sink, DEFAULT_MAX_MESSAGE_BYTES).await;

    for (status, name) in [(grpc, "otlp-grpc"), (http, "otlp-http")] {
        assert_eq!(status.name, name);
        assert_eq!(status.requested, taken);
        assert!(
            matches!(&status.state, ListenerState::Failed(why) if !why.is_empty()),
            "{status:?}"
        );
    }
    // The handles are inert: stopping them is harmless.
    grpc_stop.shutdown();
    http_stop.shutdown();
    drop(blocker);
}
