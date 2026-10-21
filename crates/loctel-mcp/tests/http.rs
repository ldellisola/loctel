//! The MCP server on a real socket (plan Task 9): Streamable HTTP on
//! loopback, an rmcp client the way Claude Code connects, and the `Origin`
//! check (design section 8).

mod common;

use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};
use std::time::{Duration, Instant};

use common::*;
use loctel_ingest::ListenerState;
use loctel_mcp::{McpShutdown, serve};
use loctel_meshgen::Scenario;
use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, ClientConfig};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use serde_json::{Value, json};

const LOOPBACK_ANY_PORT: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);

const EXPECTED_TOOLS: [&str; 9] = [
    "get_marker",
    "get_service_map",
    "get_span",
    "get_status",
    "get_trace",
    "list_services",
    "recent_failures",
    "search_logs",
    "search_traces",
];

struct Running {
    url: String,
    addr: SocketAddr,
    /// Dropping it stops the server.
    shutdown: McpShutdown,
}

/// The server over a store holding the failing checkout.
async fn start() -> Running {
    let (store, clock) = manual_store();
    load(&store, &Scenario::CheckoutFailure, 1, BASE);
    settle(&clock);
    let (status, shutdown) = serve(LOOPBACK_ANY_PORT, store, fake_status()).await;
    assert_eq!(status.name, "mcp");
    let addr = bound_address(&status.state);
    Running {
        url: format!("http://{addr}/mcp"),
        addr,
        shutdown,
    }
}

fn http() -> reqwest::Client {
    reqwest::Client::new()
}

const INITIALIZE_2025: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}"#;

/// A POST of `body`, the way an MCP client sends it, with an optional `Origin`.
async fn post(url: &str, origin: Option<&str>, body: &str) -> reqwest::Response {
    let mut request = http()
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(body.to_owned());
    if let Some(origin) = origin {
        request = request.header("Origin", origin);
    }
    request.send().await.expect("the request is sent")
}

/// The JSON-RPC message in a response, whether it came as JSON or as one
/// server-sent event.
async fn payload(response: reqwest::Response) -> Value {
    let is_stream = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/event-stream"));
    let body = response.text().await.unwrap();
    if !is_stream {
        return serde_json::from_str(&body).unwrap_or_else(|e| panic!("not JSON ({e}): {body}"));
    }
    let data = body
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim)
        .find(|data| !data.is_empty())
        .unwrap_or_else(|| panic!("no data event in {body:?}"));
    serde_json::from_str(data).unwrap_or_else(|e| panic!("not JSON ({e}): {data}"))
}

// --- the plan's tests -------------------------------------------------------------

#[tokio::test]
async fn serves_streamable_http_on_loopback() {
    let server = start().await;
    assert!(server.addr.ip().is_loopback());
    assert_ne!(
        server.addr.port(),
        0,
        "the real port, not the requested one"
    );

    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(server.url.clone()),
    );
    let client = ClientConfig::default().serve(transport).await.unwrap();

    let tools = client.list_all_tools().await.unwrap();
    let mut names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
    names.sort();
    assert_eq!(names, EXPECTED_TOOLS);
    for tool in &tools {
        assert!(tool.output_schema.is_some(), "{}", tool.name);
        assert!(
            tool.description.as_deref().is_some_and(|d| !d.is_empty()),
            "{}",
            tool.name
        );
    }

    // A call over the wire carries the same structured content and text.
    let result = client
        .call_tool(CallToolRequestParams::new("get_status"))
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    assert_eq!(structured["paused"], true);
    assert_eq!(structured["traces_held"], 4);
    let text: Value = serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(text, structured);

    let arguments = json!({ "limit": 1 }).as_object().unwrap().clone();
    let failures = client
        .call_tool(CallToolRequestParams::new("recent_failures").with_arguments(arguments))
        .await
        .unwrap();
    let failures = failures.structured_content.unwrap();
    assert_eq!(
        failures["failed"][0]["trace"]["failed_in"]["service"],
        "payments-api"
    );

    // A tool error arrives as a result, not as a protocol error.
    let arguments = json!({ "trace_id": "00000000000000000000000000000001" })
        .as_object()
        .unwrap()
        .clone();
    let missing = client
        .call_tool(CallToolRequestParams::new("get_trace").with_arguments(arguments))
        .await
        .unwrap();
    assert_eq!(missing.is_error, Some(true));
    assert_eq!(
        missing.content[0].as_text().unwrap().text,
        "trace not found"
    );

    let _ = client.cancel().await;
}

#[tokio::test]
async fn rejects_foreign_origin() {
    let server = start().await;

    for origin in [
        "https://evil.example",
        "http://evil.example:80",
        "http://localhost.evil.example",
        "http://127.0.0.1.evil.example:4319",
        "http://localhost@evil.example",
        "http://[::2]:8080",
        "http://0.0.0.0:4319",
        "chrome-extension://abcdefghijklmnop",
        "null",
        "not an origin",
        "",
    ] {
        let response = post(&server.url, Some(origin), INITIALIZE_2025).await;
        assert_eq!(response.status(), 403, "Origin: {origin:?}");
    }

    // Every method is checked, not only POST.
    let get = http()
        .get(&server.url)
        .header("Origin", "https://evil.example")
        .header("Accept", "text/event-stream")
        .send()
        .await
        .unwrap();
    assert_eq!(get.status(), 403);
    let delete = http()
        .delete(&server.url)
        .header("Origin", "https://evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(delete.status(), 403);

    // A refused request changes nothing: a good one still works.
    let response = post(&server.url, None, INITIALIZE_2025).await;
    assert_eq!(response.status(), 200);
}

#[tokio::test]
async fn allows_localhost_origin() {
    let server = start().await;

    for origin in [
        "http://localhost:3000",
        "http://localhost",
        "https://localhost:8443",
        "http://LocalHost:3000",
        "http://127.0.0.1:5173",
        "http://[::1]:5173",
        "http://[::1]",
    ] {
        let response = post(&server.url, Some(origin), INITIALIZE_2025).await;
        assert_eq!(response.status(), 200, "Origin: {origin:?}");
    }

    // No `Origin` at all (the usual case for a command line client) passes.
    let response = post(&server.url, None, INITIALIZE_2025).await;
    assert_eq!(response.status(), 200);
}

// --- protocol versions ------------------------------------------------------------------

#[tokio::test]
async fn serves_2025_11_25_clients_with_a_session() {
    let server = start().await;

    let response = post(&server.url, None, INITIALIZE_2025).await;

    assert_eq!(response.status(), 200);
    let session = response
        .headers()
        .get("mcp-session-id")
        .expect("an older client gets a session")
        .to_str()
        .unwrap()
        .to_owned();
    let reply = payload(response).await;
    assert_eq!(reply["result"]["protocolVersion"], "2025-11-25");
    assert!(reply["result"]["capabilities"]["tools"].is_object());
    assert!(
        reply["result"]["instructions"]
            .as_str()
            .is_some_and(|i| i.contains("read-only")),
        "{reply}"
    );

    let ack = http()
        .post(&server.url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("Mcp-Session-Id", &session)
        .header("MCP-Protocol-Version", "2025-11-25")
        .body(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
        .send()
        .await
        .unwrap();
    assert!(ack.status().is_success(), "{}", ack.status());
    let list = http()
        .post(&server.url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("Mcp-Session-Id", &session)
        .header("MCP-Protocol-Version", "2025-11-25")
        .body(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(list.status(), 200);
    let list = payload(list).await;
    assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 9);
}

#[tokio::test]
async fn serves_2026_07_28_clients_without_a_session() {
    let server = start().await;
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "get_marker",
            "arguments": {},
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientInfo": { "name": "test", "version": "1.0" },
                "io.modelcontextprotocol/clientCapabilities": {}
            }
        }
    });

    let response = http()
        .post(&server.url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tools/call")
        .header("Mcp-Name", "get_marker")
        .json(&body)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    assert!(
        response.headers().get("mcp-session-id").is_none(),
        "no sessions in 2026-07-28"
    );
    let reply = payload(response).await;
    let result = &reply["result"];
    assert_eq!(result["isError"], false, "{reply}");
    // The manual clock was moved 11 s on to settle the traces.
    assert_eq!(
        result["structuredContent"]["marker"],
        (BASE + 11 * SECOND).to_string()
    );
}

// --- the Host header, paths -------------------------------------------------------------------

#[tokio::test]
async fn rejects_a_foreign_host_header() {
    // DNS rebinding: a page on evil.example resolving to 127.0.0.1 sends
    // `Host: evil.example`.
    let server = start().await;

    let response = http()
        .post(&server.url)
        .header("Host", "evil.example")
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(INITIALIZE_2025)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 403);
}

#[tokio::test]
async fn only_mcp_is_served() {
    let server = start().await;
    let base = format!("http://{}", server.addr);

    for path in ["/", "/health", "/mcp2", "/v1/traces"] {
        let response = http().get(format!("{base}{path}")).send().await.unwrap();
        assert_eq!(response.status(), 404, "{path}");
    }
}

// --- the listener -------------------------------------------------------------------------------

#[tokio::test]
async fn shutdown_stops_the_server() {
    let server = start().await;
    assert!(std::net::TcpStream::connect(server.addr).is_ok());

    server.shutdown.shutdown();

    wait_until_refused(server.addr).await;
}

#[tokio::test]
async fn dropping_the_handle_stops_the_server() {
    let server = start().await;
    let addr = server.addr;

    drop(server);

    wait_until_refused(addr).await;
}

async fn wait_until_refused(addr: SocketAddr) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while std::net::TcpStream::connect(addr).is_ok() {
        assert!(
            Instant::now() < deadline,
            "{addr} still accepts connections"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn a_taken_port_is_reported_as_failed() {
    let taken = TcpListener::bind(LOOPBACK_ANY_PORT).unwrap();
    let requested = taken.local_addr().unwrap();
    let (store, _clock) = manual_store();

    let (status, _shutdown) = serve(requested, store, fake_status()).await;

    assert_eq!(status.name, "mcp");
    assert_eq!(status.requested, requested);
    match status.state {
        ListenerState::Failed(reason) => assert!(!reason.is_empty()),
        ListenerState::Listening(addr) => panic!("listening on {addr}"),
    }
}

#[tokio::test]
async fn mcp_never_listens_beyond_loopback() {
    let (store, _clock) = manual_store();
    let everywhere = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0);

    let (status, _shutdown) = serve(everywhere, store, fake_status()).await;

    assert_eq!(status.name, "mcp");
    assert_eq!(status.requested, everywhere);
    match status.state {
        ListenerState::Failed(reason) => assert!(reason.contains("loopback"), "{reason}"),
        ListenerState::Listening(addr) => panic!("listening on {addr}"),
    }
}

#[tokio::test]
async fn the_listener_status_names_the_real_address() {
    let (store, _clock) = manual_store();

    let (status, _shutdown) = serve(LOOPBACK_ANY_PORT, store, fake_status()).await;

    assert_eq!(status.requested, LOOPBACK_ANY_PORT);
    let ListenerState::Listening(addr) = status.state else {
        panic!("not listening: {status:?}");
    };
    assert_eq!(addr.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
    assert_ne!(addr.port(), 0);
}
