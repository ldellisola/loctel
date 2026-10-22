//! Moving the listeners to other ports while the engine runs
//! (`EngineHandle::rebind`), and the generation counter of the discard rules
//! that the rules editor uses to notice a change made behind its back.
//!
//! Every engine here starts on port 0, so tests never collide with each other
//! or with a running loctel; the ports they move to are free ones found by
//! binding port 0 and letting go of it.

use std::fs;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

use loctel_engine::{
    DropScope, Engine, EngineConfig, EngineHandle, ListenerState, ListenerStatus, Ports,
    RuleLanguage, RuleSpec, RulesFile, Settings,
};

const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// How long a test waits for something the engine does in the background (a
/// listener letting go of its port, the rules file being noticed: the poll is
/// every 2 s).
const WAIT: Duration = Duration::from_secs(5);

/// An engine with every port 0 and, with `rules_path`, the rules in a file.
fn start(rules_path: Option<std::path::PathBuf>) -> EngineHandle {
    start_on(0, 0, 0, rules_path)
}

fn start_on(
    otlp_grpc_port: u16,
    otlp_http_port: u16,
    mcp_port: u16,
    rules_path: Option<std::path::PathBuf>,
) -> EngineHandle {
    Engine::start(EngineConfig {
        settings: Settings {
            otlp_grpc_port,
            otlp_http_port,
            mcp_port,
            ..Settings::default()
        },
        rules_path,
        ..EngineConfig::default()
    })
    .expect("the engine starts")
}

/// A port nothing listens on right now.
fn free_port() -> u16 {
    free_ports::<1>()[0]
}

/// `N` different ports nothing listens on right now (all held at once, so the
/// system cannot hand out the same one twice).
fn free_ports<const N: usize>() -> [u16; N] {
    let held: Vec<TcpListener> = (0..N)
        .map(|_| TcpListener::bind((LOOPBACK, 0)).unwrap())
        .collect();
    let mut ports = [0; N];
    for (port, listener) in ports.iter_mut().zip(&held) {
        *port = listener.local_addr().unwrap().port();
    }
    ports
}

fn listener(statuses: &[ListenerStatus], name: &str) -> ListenerStatus {
    statuses
        .iter()
        .find(|l| l.name == name)
        .unwrap_or_else(|| panic!("no listener named {name}"))
        .clone()
}

fn address(status: &ListenerStatus) -> SocketAddr {
    match &status.state {
        ListenerState::Listening(address) => *address,
        ListenerState::Failed(why) => panic!("{} failed: {why}", status.name),
    }
}

fn names(statuses: &[ListenerStatus]) -> Vec<&'static str> {
    statuses.iter().map(|l| l.name).collect()
}

/// Whether something accepts a connection on `address`.
fn accepts(address: SocketAddr) -> bool {
    TcpStream::connect_timeout(&address, Duration::from_secs(1)).is_ok()
}

fn wait_for(what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !condition() {
        assert!(Instant::now() < deadline, "gave up waiting for {what}");
        thread::sleep(Duration::from_millis(25));
    }
}

/// POSTs JSON and returns the status code.
fn post_json(addr: SocketAddr, path: &str, body: &str) -> u16 {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    reply
        .strip_prefix("HTTP/1.1 ")
        .and_then(|rest| rest.get(..3))
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("not an HTTP reply: {reply:?}"))
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

const TRACE_A: &str = "5b8efff798038103d269b633813fc60c";
const TRACE_B: &str = "6c9f000808149214e37ac744924fd71d";

fn rule(name: &str) -> RuleSpec {
    RuleSpec {
        name: name.to_owned(),
        enabled: true,
        language: RuleLanguage::TraceQl,
        query: r#"{ name = "GET /health" }"#.to_owned(),
        scope: DropScope::Trace,
    }
}

#[test]
fn rebind_moves_http_listener() {
    let engine = start(None);
    let before = engine.status().listeners;
    let old = address(&listener(&before, "otlp-http"));
    let port = free_port();

    let after = engine.rebind(Ports {
        otlp_grpc: 0,
        otlp_http: port,
        mcp: 0,
    });

    // The usual order, and the status the engine reports says the same.
    assert_eq!(names(&after), ["otlp-grpc", "otlp-http", "mcp"]);
    assert_eq!(engine.status().listeners, after);
    let http = listener(&after, "otlp-http");
    assert_eq!(address(&http), SocketAddr::new(LOOPBACK, port));
    assert_eq!(http.requested, SocketAddr::new(LOOPBACK, port));
    // The other two did not move.
    assert_eq!(
        listener(&after, "otlp-grpc"),
        listener(&before, "otlp-grpc")
    );
    assert_eq!(listener(&after, "mcp"), listener(&before, "mcp"));

    // Data sent to the new port arrives; the old port is let go of.
    assert_eq!(
        post_json(
            address(&http),
            "/v1/traces",
            &trace_body(TRACE_A, "eee19b7ec3c1b174")
        ),
        200
    );
    assert_eq!(engine.store().stats().spans, 1);
    wait_for("the old HTTP port to close", || !accepts(old));
}

#[test]
fn rebind_moves_grpc_and_mcp_listeners() {
    let engine = start(None);
    let before = engine.status().listeners;
    let old_grpc = address(&listener(&before, "otlp-grpc"));
    let old_mcp = address(&listener(&before, "mcp"));
    let [grpc, mcp] = free_ports();

    let after = engine.rebind(Ports {
        otlp_grpc: grpc,
        otlp_http: 0,
        mcp,
    });

    assert_eq!(address(&listener(&after, "otlp-grpc")).port(), grpc);
    assert_eq!(address(&listener(&after, "mcp")).port(), mcp);
    assert_eq!(
        listener(&after, "otlp-http"),
        listener(&before, "otlp-http")
    );
    assert_eq!(engine.status().listeners, after);
    // Agents are told the new MCP address.
    assert_eq!(
        engine.mcp_url(),
        Some(format!("http://127.0.0.1:{mcp}/mcp"))
    );
    assert!(accepts(address(&listener(&after, "otlp-grpc"))));
    assert!(accepts(address(&listener(&after, "mcp"))));
    wait_for("the old gRPC port to close", || !accepts(old_grpc));
    wait_for("the old MCP port to close", || !accepts(old_mcp));
}

#[test]
fn rebind_to_busy_port_reports_failed_keeps_others() {
    let engine = start(None);
    let before = engine.status().listeners;
    let old = address(&listener(&before, "otlp-http"));
    // Something else holds this port for the whole test.
    let busy = TcpListener::bind((LOOPBACK, 0)).unwrap();
    let busy_port = busy.local_addr().unwrap().port();

    let after = engine.rebind(Ports {
        otlp_grpc: 0,
        otlp_http: busy_port,
        mcp: 0,
    });

    let http = listener(&after, "otlp-http");
    assert_eq!(http.requested, SocketAddr::new(LOOPBACK, busy_port));
    match &http.state {
        ListenerState::Failed(reason) => assert!(!reason.is_empty(), "{http:?}"),
        other => panic!("expected Failed, got {other:?}"),
    }
    // The others keep running, as they were.
    assert_eq!(
        listener(&after, "otlp-grpc"),
        listener(&before, "otlp-grpc")
    );
    assert_eq!(listener(&after, "mcp"), listener(&before, "mcp"));
    assert!(accepts(address(&listener(&after, "otlp-grpc"))));
    assert!(accepts(address(&listener(&after, "mcp"))));
    assert_eq!(engine.status().listeners, after);
    wait_for("the old HTTP port to close", || !accepts(old));

    // A later move to a free port recovers it.
    let free = free_port();
    let recovered = engine.rebind(Ports {
        otlp_grpc: 0,
        otlp_http: free,
        mcp: 0,
    });
    assert_eq!(address(&listener(&recovered, "otlp-http")).port(), free);
    assert_eq!(
        post_json(
            address(&listener(&recovered, "otlp-http")),
            "/v1/traces",
            &trace_body(TRACE_B, "fff2ac8fd4d2c285")
        ),
        200
    );
}

#[test]
fn rebind_unchanged_is_noop() {
    let engine = start(None);
    let before = engine.status().listeners;

    // The ports it was started with (0, which means "any") are no change.
    let after = engine.rebind(Ports {
        otlp_grpc: 0,
        otlp_http: 0,
        mcp: 0,
    });
    assert_eq!(after, before);
    assert_eq!(engine.status().listeners, before);
    let http = address(&listener(&after, "otlp-http"));
    assert_eq!(
        post_json(http, "/v1/traces", &trace_body(TRACE_A, "eee19b7ec3c1b174")),
        200
    );

    // And so are the ports it was last moved to.
    let [otlp_grpc, otlp_http, mcp] = free_ports();
    let ports = Ports {
        otlp_grpc,
        otlp_http,
        mcp,
    };
    let moved = engine.rebind(ports);
    assert_eq!(engine.rebind(ports), moved);
    assert_eq!(engine.status().listeners, moved);
    assert_eq!(
        address(&listener(&moved, "otlp-http")).port(),
        ports.otlp_http
    );
}

#[test]
fn rebind_can_take_the_port_another_listener_just_left() {
    let [a, b] = free_ports();
    let engine = start_on(a, b, 0, None);
    let before = engine.status().listeners;
    assert_eq!(address(&listener(&before, "otlp-grpc")).port(), a);
    assert_eq!(address(&listener(&before, "otlp-http")).port(), b);

    // Swapped in one go: each listener binds the port the other is leaving.
    let after = engine.rebind(Ports {
        otlp_grpc: b,
        otlp_http: a,
        mcp: 0,
    });

    assert_eq!(address(&listener(&after, "otlp-grpc")).port(), b);
    assert_eq!(address(&listener(&after, "otlp-http")).port(), a);
    assert_eq!(
        post_json(
            SocketAddr::new(LOOPBACK, a),
            "/v1/traces",
            &trace_body(TRACE_A, "eee19b7ec3c1b174")
        ),
        200
    );
}

#[test]
fn rebind_after_shutdown_changes_nothing() {
    let engine = start(None);
    let before = engine.status().listeners;
    engine.shutdown();

    let [otlp_grpc, otlp_http, mcp] = free_ports();
    let after = engine.rebind(Ports {
        otlp_grpc,
        otlp_http,
        mcp,
    });

    assert_eq!(after, before);
}

#[test]
fn rebind_works_from_inside_a_tokio_runtime() {
    // The window calls it on its own thread, but nothing stops a caller that
    // happens to be on a runtime: it must not panic.
    let engine = start(None);
    let port = free_port();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .build()
        .unwrap();

    let after = runtime.block_on(async {
        engine.rebind(Ports {
            otlp_grpc: 0,
            otlp_http: port,
            mcp: 0,
        })
    });

    assert_eq!(address(&listener(&after, "otlp-http")).port(), port);
}

#[test]
fn rules_generation_increments_on_reload_and_set() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.toml");
    let engine = start(Some(path.clone()));
    let first = engine.rules_generation();

    // A save through the handle.
    engine.set_rules(vec![rule("a")]).unwrap();
    let after_set = engine.rules_generation();
    assert!(after_set > first, "{after_set} after {first}");

    // A switch is a save too.
    let id = engine.rules()[0].id;
    engine.set_rule_enabled(id, false).unwrap();
    let after_switch = engine.rules_generation();
    assert!(after_switch > after_set, "{after_switch} after {after_set}");

    // Nothing changed, nothing counted.
    assert_eq!(engine.rules_generation(), after_switch);

    // A file that cannot be read replaces nothing.
    fs::write(&path, "not [ toml").unwrap();
    wait_for("the bad file to be noticed", || {
        engine.status().rules_file_error.is_some()
    });
    assert_eq!(engine.rules_generation(), after_switch);

    // An edit made behind the engine's back, once it is noticed.
    RulesFile::save(&path, &[rule("a"), rule("b")]).unwrap();
    wait_for("the edit to be loaded", || engine.rules().len() == 2);
    assert!(engine.rules_generation() > after_switch);
}

#[test]
fn rules_generation_counts_sets_without_a_file() {
    // In memory only: sets still count, and there is no file to reload.
    let engine = start(None);
    let first = engine.rules_generation();

    engine.set_rules(vec![rule("a")]).unwrap();

    assert!(engine.rules_generation() > first);
}
