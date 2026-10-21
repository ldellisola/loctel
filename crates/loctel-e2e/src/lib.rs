//! The end-to-end harness: a real loctel engine on real sockets, fed by the
//! fake mesh over real OTLP and read back through a real MCP client over
//! Streamable HTTP, the way an agent reads it.
//!
//! The tests are in `tests/`; this library is what they share. A [`Harness`]
//! starts the engine (the same one `loctel --headless` runs) with every port
//! at 0, so tests never collide with each other or with a running loctel, and
//! with a short trace-completeness time so that a test waits for 200 ms and
//! not for 10 s.
//!
//! ```text
//! let harness = Harness::start(Duration::from_millis(200));
//! let sent = harness.send(&Scenario::CheckoutFailure, TransportKind::Grpc, 1, 0.0).await;
//! harness.wait_complete(&sent.trace_ids[0]).await;
//! let failures = harness.tool("recent_failures", json!({})).await;
//! ```
//!
//! `Harness::start_with_rules` is the same on a `rules.toml` the test wrote,
//! for the discard rules (which a plain `start` has none of, as in the engine's
//! tests: loctel never decides what to discard).
//!
//! Nothing here asserts on what loctel says: that is the tests' job. The
//! harness only fails (with a panic that says why) when it cannot do its own,
//! such as an engine that will not start, a tool that errors where success
//! was expected, or a trace that never completes. See
//! `docs/areas/testing.md`.

use std::fmt::{self, Display};
use std::net::SocketAddr;
use std::panic;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use loctel_engine::{Engine, EngineConfig, EngineHandle, ListenerState, Settings, StoreConfig};
use loctel_meshgen::{Generated, Scenario, Transport, generate};
use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, ClientConfig};
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use serde_json::{Value, json};
use tokio::sync::OnceCell;

/// How long [`Harness::wait_complete`] waits for a trace to complete.
const COMPLETE_TIMEOUT: Duration = Duration::from_secs(10);

/// How often [`Harness::wait_complete`] asks.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// How long one MCP request may take before the harness gives up, so a hung
/// server fails a test instead of hanging the suite.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// The three ways OTLP reaches loctel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportKind {
    /// OTLP over gRPC (port 4317 by default).
    Grpc,
    /// OTLP over HTTP, protobuf bodies (port 4318).
    HttpProtobuf,
    /// OTLP over HTTP, JSON bodies (port 4318).
    HttpJson,
}

impl TransportKind {
    /// Every transport, in a fixed order, for tests that go through each.
    pub const ALL: [TransportKind; 3] = [
        TransportKind::Grpc,
        TransportKind::HttpProtobuf,
        TransportKind::HttpJson,
    ];
}

/// A running loctel engine with a way to send it OTLP and an MCP client to
/// read it. Dropping the harness stops the engine.
pub struct Harness {
    engine: EngineHandle,
    grpc: SocketAddr,
    http: SocketAddr,
    mcp: McpClient,
}

impl Harness {
    /// Starts an engine on loopback with every port 0 and the store's
    /// `completeness_quiet` set to `completeness` (a trace is complete once no
    /// span has arrived for that long). It has no discard rules: nothing is
    /// dropped, and the rules cannot be edited from a file.
    ///
    /// Callable from sync code and from inside a tokio runtime (every test is
    /// `#[tokio::test]`): the engine owns its runtime and refuses to start
    /// inside another one, so it is started on a thread of its own. That
    /// blocks the caller for as long as it takes to bind three sockets.
    ///
    /// # Panics
    ///
    /// If the engine does not start or a listener cannot bind.
    pub fn start(completeness: Duration) -> Harness {
        Harness::start_engine(completeness, None)
    }

    /// [`Harness::start`], with the discard rules in the file at `rules_path`,
    /// the way `loctel --rules PATH` runs them: the engine loads the file if
    /// it is there (a test writes it first), creates one holding no rules if
    /// it is not, and reloads it within a few seconds of an edit.
    ///
    /// # Panics
    ///
    /// As [`Harness::start`].
    pub fn start_with_rules(completeness: Duration, rules_path: impl Into<PathBuf>) -> Harness {
        Harness::start_engine(completeness, Some(rules_path.into()))
    }

    fn start_engine(completeness: Duration, rules_path: Option<PathBuf>) -> Harness {
        let config = EngineConfig {
            settings: Settings {
                otlp_grpc_port: 0,
                otlp_http_port: 0,
                mcp_port: 0,
                ..Settings::default()
            },
            store: StoreConfig {
                completeness_quiet: completeness,
                ..StoreConfig::default()
            },
            rules_path,
            ..EngineConfig::default()
        };
        let engine = thread::scope(|scope| scope.spawn(|| Engine::start(config)).join())
            .unwrap_or_else(|payload| panic::resume_unwind(payload))
            .expect("the engine starts");

        let grpc = bound(&engine, "otlp-grpc");
        let http = bound(&engine, "otlp-http");
        let mcp_url = engine.mcp_url().expect("the MCP server is listening");
        Harness {
            engine,
            grpc,
            http,
            mcp: McpClient::new(mcp_url),
        }
    }

    /// The engine under test: to pause it, read its status or look in its
    /// store directly.
    pub fn engine(&self) -> &EngineHandle {
        &self.engine
    }

    /// Where `kind` of OTLP is received, in the form `loctel_meshgen::send`
    /// takes.
    pub fn otlp(&self, kind: TransportKind) -> Transport {
        match kind {
            TransportKind::Grpc => Transport::Grpc(format!("http://{}", self.grpc)),
            TransportKind::HttpProtobuf => Transport::HttpProtobuf(format!("http://{}", self.http)),
            TransportKind::HttpJson => Transport::HttpJson(format!("http://{}", self.http)),
        }
    }

    /// The MCP client of this harness, connected to loctel's MCP server over
    /// Streamable HTTP when it is first used. Clones share the connection.
    pub fn mcp(&self) -> McpClient {
        self.mcp.clone()
    }

    /// Calls an MCP tool and returns its structured content. `args` is a JSON
    /// object (`json!({})` for none). Goes through the real transport, and
    /// checks on the way that the text copy of the result is the same JSON.
    ///
    /// # Panics
    ///
    /// If the call fails or the tool answers with an error. Use
    /// `harness.mcp().call(..)` to look at a tool error.
    pub async fn tool(&self, name: &str, args: Value) -> Value {
        self.mcp.tool(name, args).await
    }

    /// Generates `scenario` with `seed`, dated now, and sends it over `kind`,
    /// sleeping the scenario's pauses times `time_scale` (0 sends at once).
    /// Returns what was generated (its `trace_ids` identify the traces).
    ///
    /// The same scenario with the same seed is the same trace, which the store
    /// keeps once; give each send in a test its own seed.
    ///
    /// # Panics
    ///
    /// If delivery fails, which includes loctel rejecting an item: generated
    /// data is always valid.
    pub async fn send(
        &self,
        scenario: &Scenario,
        kind: TransportKind,
        seed: u64,
        time_scale: f64,
    ) -> Generated {
        let generated = generate(scenario, seed, now_unix_nano());
        loctel_meshgen::send(&generated, &self.otlp(kind), time_scale)
            .await
            .unwrap_or_else(|e| panic!("sending {scenario:?} over {kind:?} failed: {e:#}"));
        generated
    }

    /// The marker an agent takes before it makes a request: `get_marker`'s
    /// `marker`, to pass back as `since`.
    pub async fn marker(&self) -> String {
        let marker = self.tool("get_marker", json!({})).await;
        marker["marker"]
            .as_str()
            .unwrap_or_else(|| panic!("get_marker has no marker: {marker}"))
            .to_owned()
    }

    /// Polls `get_trace` until the trace is `complete`, for at most 10 s. A
    /// trace that has not arrived yet is waited for too.
    ///
    /// # Panics
    ///
    /// If the trace is not complete in time, with the last answer.
    pub async fn wait_complete(&self, trace_id: impl Display) {
        let trace_id = trace_id.to_string();
        let deadline = Instant::now() + COMPLETE_TIMEOUT;
        loop {
            let outcome = self
                .mcp
                .call("get_trace", json!({ "trace_id": trace_id, "limit": 1 }))
                .await;
            let complete = outcome
                .structured
                .as_ref()
                .is_some_and(|result| result["summary"]["complete"] == true);
            if !outcome.is_error && complete {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "trace {trace_id} is not complete after {COMPLETE_TIMEOUT:?}; last answer: {outcome:?}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        // Safe inside a tokio runtime: it does not wait for the engine's
        // threads there.
        self.engine.shutdown();
    }
}

/// The address `name` really bound.
fn bound(engine: &EngineHandle, name: &str) -> SocketAddr {
    let listener = engine
        .status()
        .listeners
        .into_iter()
        .find(|listener| listener.name == name)
        .unwrap_or_else(|| panic!("the engine has no {name} listener"));
    match listener.state {
        ListenerState::Listening(address) => address,
        ListenerState::Failed(reason) => panic!("{name} could not bind: {reason}"),
    }
}

/// Now, in nanoseconds since the Unix epoch: the base time of generated
/// scenarios, so that their traces start after a marker taken just before.
pub fn now_unix_nano() -> u64 {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after 1970");
    u64::try_from(since_epoch.as_nanos()).expect("the date fits in 64 bits of nanoseconds")
}

/// What a tool call answered, without rmcp's types.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolOutcome {
    /// `isError` of the result: the tool could not answer, and `text` says why.
    pub is_error: bool,
    /// `structuredContent`, when the tool sent any (it does not on an error).
    pub structured: Option<Value>,
    /// The text content: the same JSON as `structured` on success, the reason
    /// on an error.
    pub text: String,
}

type Connection = RunningService<RoleClient, ClientConfig>;

/// An MCP client for loctel's Streamable HTTP endpoint (rmcp's client, the
/// protocol an agent speaks). It connects on first use and is cheap to clone;
/// clones share one connection.
#[derive(Clone)]
pub struct McpClient {
    inner: Arc<Inner>,
}

struct Inner {
    url: String,
    connection: OnceCell<Connection>,
}

impl McpClient {
    /// A client for the MCP server at `url` (`http://127.0.0.1:PORT/mcp`).
    pub fn new(url: impl Into<String>) -> McpClient {
        McpClient {
            inner: Arc::new(Inner {
                url: url.into(),
                connection: OnceCell::new(),
            }),
        }
    }

    async fn connection(&self) -> &Connection {
        self.inner
            .connection
            .get_or_init(|| async {
                // No proxy: loctel is on this machine, and a developer's
                // HTTP(S)_PROXY must not carry the request away.
                let http = reqwest::Client::builder()
                    .no_proxy()
                    .build()
                    .expect("the HTTP client builds");
                let transport = StreamableHttpClientTransport::with_client(
                    http,
                    StreamableHttpClientTransportConfig::with_uri(self.inner.url.clone()),
                );
                ClientConfig::default()
                    .serve(transport)
                    .await
                    .unwrap_or_else(|e| panic!("cannot connect to MCP at {}: {e}", self.inner.url))
            })
            .await
    }

    /// Calls a tool and returns what it answered, error or not. `args` is a
    /// JSON object, or `null` for none.
    ///
    /// # Panics
    ///
    /// On a protocol failure (no answer, a malformed one, 30 s of silence). A
    /// tool that answers with an error is not one: see
    /// [`ToolOutcome::is_error`].
    pub async fn call(&self, name: &str, args: Value) -> ToolOutcome {
        let mut request = CallToolRequestParams::new(name.to_owned());
        match args {
            Value::Object(arguments) => request = request.with_arguments(arguments),
            Value::Null => {}
            other => panic!("the arguments of {name} must be a JSON object, not {other}"),
        }
        let connection = self.connection().await;
        let result = tokio::time::timeout(CALL_TIMEOUT, connection.call_tool(request))
            .await
            .unwrap_or_else(|_| panic!("{name} did not answer within {CALL_TIMEOUT:?}"))
            .unwrap_or_else(|e| panic!("{name} failed at the protocol level: {e}"));
        let text = result
            .content
            .iter()
            .filter_map(|content| content.as_text())
            .map(|text| text.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        ToolOutcome {
            is_error: result.is_error == Some(true),
            structured: result.structured_content,
            text,
        }
    }

    /// Calls a tool that must succeed and returns its structured content,
    /// after checking that the text content is the same JSON (design section 8:
    /// both are always sent).
    ///
    /// # Panics
    ///
    /// If the tool answers with an error, sends no structured content, or
    /// sends text that differs from it.
    pub async fn tool(&self, name: &str, args: Value) -> Value {
        let outcome = self.call(name, args).await;
        assert!(
            !outcome.is_error,
            "{name} answered an error: {}",
            outcome.text
        );
        let structured = outcome
            .structured
            .unwrap_or_else(|| panic!("{name} sent no structured content: {}", outcome.text));
        let text: Value = serde_json::from_str(&outcome.text)
            .unwrap_or_else(|e| panic!("the text of {name} is not JSON ({e}): {}", outcome.text));
        assert_eq!(
            text, structured,
            "{name}: the text and the structured content differ"
        );
        structured
    }
}

impl fmt::Debug for McpClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpClient")
            .field("url", &self.inner.url)
            .finish_non_exhaustive()
    }
}
