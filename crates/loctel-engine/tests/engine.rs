//! The engine on real sockets (port 0 on 127.0.0.1): OTLP/JSON over HTTP in,
//! status and store out. The engine owns its runtime, so these are plain
//! synchronous tests, as a non-async `main` would call it.

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

use loctel_engine::{
    Engine, EngineConfig, EngineHandle, ListenerState, ListenerStatus, Settings, StoreConfig,
};

const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// Every port 0, so tests never collide with each other or with a running
/// loctel.
fn config() -> EngineConfig {
    EngineConfig {
        settings: Settings {
            otlp_grpc_port: 0,
            otlp_http_port: 0,
            mcp_port: 0,
            ..Settings::default()
        },
        ..EngineConfig::default()
    }
}

fn listener(handle: &EngineHandle, name: &str) -> ListenerStatus {
    handle
        .status()
        .listeners
        .into_iter()
        .find(|l| l.name == name)
        .unwrap_or_else(|| panic!("no listener named {name}"))
}

fn http_addr(handle: &EngineHandle) -> SocketAddr {
    match listener(handle, "otlp-http").state {
        ListenerState::Listening(addr) => addr,
        ListenerState::Failed(why) => panic!("otlp-http failed: {why}"),
    }
}

/// POSTs JSON and returns the status code and the body.
fn post_json(addr: SocketAddr, path: &str, body: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    let status = reply
        .strip_prefix("HTTP/1.1 ")
        .and_then(|rest| rest.get(..3))
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("not an HTTP reply: {reply:?}"));
    let body = reply.split_once("\r\n\r\n").map_or("", |(_, b)| b);
    (status, body.to_owned())
}

fn trace_body(trace_id: &str, span_id: &str) -> String {
    format!(
        r#"{{"resourceSpans":[{{
            "resource":{{"attributes":[
                {{"key":"service.name","value":{{"stringValue":"checkout"}}}}]}},
            "scopeSpans":[{{"scope":{{"name":"test"}},"spans":[{{
                "traceId":"{trace_id}","spanId":"{span_id}","name":"GET /cart","kind":2,
                "startTimeUnixNano":"1700000000000000000",
                "endTimeUnixNano":"1700000000500000000",
                "status":{{"code":1}}}}]}}]}}]}}"#
    )
}

fn logs_body() -> &'static str {
    r#"{"resourceLogs":[{
        "resource":{"attributes":[{"key":"service.name","value":{"stringValue":"checkout"}}]},
        "scopeLogs":[{"logRecords":[{
            "timeUnixNano":"1700000000100000000","severityNumber":9,
            "body":{"stringValue":"cart loaded"}}]}]}]}"#
}

const TRACE_A: &str = "5b8efff798038103d269b633813fc60c";
const TRACE_B: &str = "6c9f000808149214e37ac744924fd71d";

#[test]
fn engine_ingests_and_reports_status() {
    let engine = Engine::start(config()).unwrap();

    // Both OTLP listeners are up, on real ports, and the status says so.
    let status = engine.status();
    let names: Vec<_> = status.listeners.iter().map(|l| l.name).collect();
    assert_eq!(&names[..2], ["otlp-grpc", "otlp-http"]);
    for l in &status.listeners[..2] {
        assert!(
            matches!(l.state, ListenerState::Listening(addr) if addr.port() != 0 && addr.ip() == LOOPBACK),
            "{l:?}"
        );
    }
    assert!(!status.paused);
    assert_eq!(status.stats.spans, 0);
    assert_eq!(status.spans_per_sec, 0.0);

    let (code, _) = post_json(
        http_addr(&engine),
        "/v1/traces",
        &trace_body(TRACE_A, "eee19b7ec3c1b174"),
    );
    assert_eq!(code, 200);

    let status = engine.status();
    assert_eq!(status.stats.spans, 1);
    assert_eq!(status.stats.traces, 1);
    assert_eq!(status.stats.spans_received, 1);
    assert!(status.spans_per_sec > 0.0, "{status:?}");
    assert_eq!(status.logs_per_sec, 0.0);
    // The same data is reachable through the store handle.
    assert_eq!(engine.store().stats(), status.stats);

    let (code, _) = post_json(http_addr(&engine), "/v1/logs", logs_body());
    assert_eq!(code, 200);
    let status = engine.status();
    assert_eq!(status.stats.logs, 1);
    assert!(status.logs_per_sec > 0.0, "{status:?}");

    engine.shutdown();
}

#[test]
fn paused_engine_drops_but_accepts() {
    let engine = Engine::start(config()).unwrap();
    let addr = http_addr(&engine);
    assert!(!engine.is_paused());

    engine.set_paused(true);
    assert!(engine.is_paused());
    assert!(engine.status().paused);

    // The receiver still answers success, so exporters do not retry.
    let (code, body) = post_json(addr, "/v1/traces", &trace_body(TRACE_A, "eee19b7ec3c1b174"));
    assert_eq!(code, 200);
    assert_eq!(body.trim(), "{}", "a plain success, nothing rejected");
    let (code, _) = post_json(addr, "/v1/logs", logs_body());
    assert_eq!(code, 200);

    // But nothing was kept, or counted.
    let status = engine.status();
    assert_eq!(status.stats.spans, 0);
    assert_eq!(status.stats.logs, 0);
    assert_eq!(status.stats.spans_received, 0);
    assert_eq!(status.stats.logs_received, 0);
    assert_eq!(status.spans_per_sec, 0.0);
    assert_eq!(status.logs_per_sec, 0.0);

    // Resuming keeps what arrives next; what was dropped stays dropped.
    engine.set_paused(false);
    assert!(!engine.status().paused);
    let (code, _) = post_json(addr, "/v1/traces", &trace_body(TRACE_B, "fff2ac8fd4d2c285"));
    assert_eq!(code, 200);
    let status = engine.status();
    assert_eq!(status.stats.spans, 1);
    assert_eq!(status.stats.traces, 1);

    engine.shutdown();
}

#[test]
fn handles_are_clones_of_one_engine() {
    let engine = Engine::start(config()).unwrap();
    let other = engine.clone();

    other.set_paused(true);
    assert!(engine.is_paused());

    let (code, _) = post_json(
        http_addr(&engine),
        "/v1/traces",
        &trace_body(TRACE_A, "eee19b7ec3c1b174"),
    );
    assert_eq!(code, 200);
    other.set_paused(false);
    let (code, _) = post_json(
        http_addr(&other),
        "/v1/traces",
        &trace_body(TRACE_B, "fff2ac8fd4d2c285"),
    );
    assert_eq!(code, 200);
    assert_eq!(engine.store().stats().spans, 1);
    assert_eq!(other.store().stats().spans, 1);

    other.shutdown();
}

#[test]
fn store_config_reaches_the_store() {
    // A store that holds one trace, so the second evicts the first.
    let engine = Engine::start(EngineConfig {
        store: StoreConfig {
            max_traces: 1,
            ..StoreConfig::default()
        },
        ..config()
    })
    .unwrap();
    let addr = http_addr(&engine);

    post_json(addr, "/v1/traces", &trace_body(TRACE_A, "eee19b7ec3c1b174"));
    post_json(addr, "/v1/traces", &trace_body(TRACE_B, "fff2ac8fd4d2c285"));

    let stats = engine.status().stats;
    assert_eq!((stats.traces, stats.spans_received), (1, 2));
    engine.shutdown();
}

#[test]
fn a_taken_port_is_a_failed_listener_and_the_other_keeps_running() {
    let taken = TcpListener::bind((LOOPBACK, 0)).unwrap();
    let taken_port = taken.local_addr().unwrap().port();
    let engine = Engine::start(EngineConfig {
        settings: Settings {
            otlp_http_port: taken_port,
            ..config().settings
        },
        ..config()
    })
    .unwrap();

    let http = listener(&engine, "otlp-http");
    assert_eq!(http.requested, SocketAddr::new(LOOPBACK, taken_port));
    assert!(matches!(http.state, ListenerState::Failed(_)), "{http:?}");
    let grpc = listener(&engine, "otlp-grpc");
    assert!(
        matches!(grpc.state, ListenerState::Listening(_)),
        "{grpc:?}"
    );
    engine.shutdown();
}

#[test]
fn bind_ip_decides_the_listening_address() {
    // All interfaces, which every platform allows (127.0.0.2 is not bindable
    // on a Mac by default).
    let engine = Engine::start(EngineConfig {
        bind_ip: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        ..config()
    })
    .unwrap();
    for name in ["otlp-grpc", "otlp-http"] {
        match listener(&engine, name).state {
            ListenerState::Listening(addr) => assert!(addr.ip().is_unspecified(), "{name}"),
            ListenerState::Failed(why) => panic!("{name}: {why}"),
        }
    }
    engine.shutdown();
}

#[test]
fn shutdown_stops_the_listeners_and_can_be_repeated() {
    let engine = Engine::start(config()).unwrap();
    let addr = http_addr(&engine);
    assert!(TcpStream::connect(addr).is_ok());

    engine.shutdown();
    engine.clone().shutdown();

    // Connections are refused soon after. Poll rather than assume how fast
    // the sockets close.
    let deadline = Instant::now() + Duration::from_secs(10);
    while TcpStream::connect(addr).is_ok() {
        assert!(Instant::now() < deadline, "still accepting after shutdown");
        thread::sleep(Duration::from_millis(20));
    }
    // The store outlives the receivers.
    assert_eq!(engine.store().stats().spans, 0);
    let _ = engine.status();
}

#[test]
fn dropping_the_last_handle_stops_the_engine() {
    let engine = Engine::start(config()).unwrap();
    let addr = http_addr(&engine);
    drop(engine);

    let deadline = Instant::now() + Duration::from_secs(10);
    while TcpStream::connect(addr).is_ok() {
        assert!(Instant::now() < deadline, "still accepting after drop");
        thread::sleep(Duration::from_millis(20));
    }
}

#[tokio::test]
async fn start_inside_a_runtime_is_an_error_not_a_panic() {
    let error = Engine::start(config()).err().expect("must refuse");
    assert!(
        format!("{error:#}").contains("runtime"),
        "the message says why: {error:#}"
    );
}

#[tokio::test]
async fn shutdown_and_drop_are_safe_inside_a_runtime() {
    // A handle made outside the runtime, used and dropped inside one, as a
    // test harness or a GUI callback might.
    let engine = thread::spawn(|| Engine::start(config()).unwrap())
        .join()
        .unwrap();
    engine.set_paused(true);
    engine.shutdown();
    drop(engine);
}
