//! The loctel engine: everything between the sockets and the UI that is not
//! the store or the receivers themselves.
//!
//! [`Engine::start`] builds a [`Store`], starts the OTLP receivers and the MCP
//! server on a runtime of its own, and returns an [`EngineHandle`]: the one
//! object the menu bar, the window and the headless binary hold. Through it
//! they read the store, ask for a status snapshot, pause and resume receiving,
//! find the MCP address and shut everything down. [`Settings`] is the
//! `settings.toml` the ports and the disk cap come from.
//!
//! The engine has no UI or graphics dependency, and its `start` needs no async
//! `main`: it owns its threads. See `docs/architecture.md` ("Engine" and
//! "Headless mode") for the data flow.

mod rates;
mod settings;
mod sink;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use anyhow::{Context as _, bail};
use loctel_ingest::{DEFAULT_MAX_MESSAGE_BYTES, IngestConfig, IngestServers};
use loctel_mcp::{ListenerDto, McpShutdown, StatusDto, StatusSource};
use loctel_store::{Clock, SystemClock};
use tokio::runtime::{Builder, Handle, Runtime};

pub use loctel_ingest::{ListenerState, ListenerStatus};
pub use loctel_store::{Store, StoreConfig, StoreStats};
pub use rates::RateMeter;
pub use settings::Settings;

use sink::EngineSink;

/// How long [`EngineHandle::shutdown`] waits for the engine's threads to
/// finish (from a thread that may block).
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

/// What [`Engine::start`] needs.
#[derive(Clone, Debug)]
pub struct EngineConfig {
    /// The address the OTLP receivers bind to: loopback by default (design
    /// §4). The MCP server is always loopback, whatever this says.
    pub bind_ip: IpAddr,
    /// Ports (and the rest of `settings.toml`).
    pub settings: Settings,
    pub store: StoreConfig,
}

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig {
            bind_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            settings: Settings::default(),
            store: StoreConfig::default(),
        }
    }
}

/// A snapshot of how the engine is doing, for the status box and the menu.
#[derive(Clone, Debug, PartialEq)]
pub struct EngineStatus {
    pub paused: bool,
    /// Every endpoint, as bound (or why it could not be): `otlp-grpc`,
    /// `otlp-http` and `mcp`, in that order.
    pub listeners: Vec<ListenerStatus>,
    /// Spans accepted per second over the last 5 seconds. Spans dropped
    /// while paused are not counted.
    pub spans_per_sec: f64,
    /// Log records accepted per second over the last 5 seconds.
    pub logs_per_sec: f64,
    pub stats: StoreStats,
}

/// Namespace for [`Engine::start`].
pub struct Engine;

impl Engine {
    /// Starts the engine: a multi-thread tokio runtime on threads of its own,
    /// the store, the OTLP receivers and the MCP server (on loopback, at the
    /// port in the settings, whatever `bind_ip` says).
    ///
    /// Call it from plain code, such as a non-async `main`. It blocks only
    /// while the sockets are bound, which is quick; a port that cannot be
    /// bound is not an error but a [`ListenerState::Failed`] in the status,
    /// and the other receivers still run (design §4). It is an error to call
    /// it from inside a tokio runtime (that would panic in `block_on`), and
    /// an error if the operating system will not give it threads.
    pub fn start(config: EngineConfig) -> anyhow::Result<EngineHandle> {
        if Handle::try_current().is_ok() {
            bail!(
                "the engine owns its own runtime, so it cannot be started from inside a tokio runtime"
            );
        }
        let EngineConfig {
            bind_ip,
            settings,
            store,
        } = config;

        let runtime = Builder::new_multi_thread()
            .enable_all()
            .thread_name("loctel-engine")
            .build()
            .context("cannot start the engine's runtime")?;

        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let store = Store::new(store, Arc::clone(&clock));
        let paused = Arc::new(AtomicBool::new(false));
        let span_rate = Arc::new(RateMeter::new());
        let log_rate = Arc::new(RateMeter::new());

        let sink = Arc::new(EngineSink {
            store: store.clone(),
            paused: Arc::clone(&paused),
            clock: Arc::clone(&clock),
            span_rate: Arc::clone(&span_rate),
            log_rate: Arc::clone(&log_rate),
        });
        let ingest_config = IngestConfig {
            grpc_addr: SocketAddr::new(bind_ip, settings.otlp_grpc_port),
            http_addr: SocketAddr::new(bind_ip, settings.otlp_http_port),
            max_message_bytes: DEFAULT_MAX_MESSAGE_BYTES,
        };
        let ingest = runtime.block_on(loctel_ingest::start(ingest_config, sink));

        // The MCP server reads the engine's status, so it is started with the
        // state it reads. The state learns the MCP listener's own status once
        // the server has bound (a request in the instant before that sees the
        // other two listeners).
        let state = Arc::new(State {
            store: store.clone(),
            paused,
            clock,
            span_rate,
            log_rate,
            otlp: [ingest.grpc.clone(), ingest.http.clone()],
            mcp: OnceLock::new(),
        });
        let mcp_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), settings.mcp_port);
        let status_source: Arc<dyn StatusSource> = state.clone();
        let (mcp, mcp_shutdown) =
            runtime.block_on(loctel_mcp::serve(mcp_addr, store, status_source));
        state
            .mcp
            .set(mcp)
            .expect("the MCP status is set once, here");

        Ok(EngineHandle {
            shared: Arc::new(Shared {
                state,
                running: Mutex::new(Some(Running {
                    runtime,
                    ingest,
                    mcp: mcp_shutdown,
                })),
            }),
        })
    }
}

/// The running engine. Cloning is cheap and the clones are the same engine;
/// hand one to every part of the app that needs it. The engine stops on
/// [`EngineHandle::shutdown`] or when the last handle is dropped.
#[derive(Clone)]
pub struct EngineHandle {
    shared: Arc<Shared>,
}

struct Shared {
    state: Arc<State>,
    /// `None` once stopped.
    running: Mutex<Option<Running>>,
}

/// What stopping the engine has to take down.
struct Running {
    runtime: Runtime,
    ingest: IngestServers,
    mcp: McpShutdown,
}

/// What the engine knows about itself: the one place a status is read from,
/// for [`EngineHandle::status`] and for the MCP `get_status` tool, so the two
/// cannot disagree.
struct State {
    store: Store,
    paused: Arc<AtomicBool>,
    clock: Arc<dyn Clock>,
    span_rate: Arc<RateMeter>,
    log_rate: Arc<RateMeter>,
    /// `otlp-grpc` and `otlp-http`.
    otlp: [ListenerStatus; 2],
    /// Set once, right after the MCP server has bound.
    mcp: OnceLock<ListenerStatus>,
}

impl State {
    fn snapshot(&self) -> EngineStatus {
        let now = self.clock.now_unix_nano();
        EngineStatus {
            paused: self.paused.load(Ordering::Acquire),
            listeners: self.otlp.iter().chain(self.mcp.get()).cloned().collect(),
            spans_per_sec: self.span_rate.per_sec(now),
            logs_per_sec: self.log_rate.per_sec(now),
            stats: self.store.stats(),
        }
    }

    /// Where the MCP server listens, if it could bind.
    fn mcp_address(&self) -> Option<SocketAddr> {
        match &self.mcp.get()?.state {
            ListenerState::Listening(address) => Some(*address),
            ListenerState::Failed(_) => None,
        }
    }
}

impl StatusSource for State {
    fn status(&self) -> StatusDto {
        let status = self.snapshot();
        StatusDto {
            paused: status.paused,
            listeners: status.listeners.iter().map(listener_dto).collect(),
            spans_per_sec: status.spans_per_sec,
            logs_per_sec: status.logs_per_sec,
            traces_held: status.stats.traces as u64,
            spans_held: status.stats.spans as u64,
            logs_held: status.stats.logs as u64,
        }
    }
}

/// A listener as the MCP status tells it: where it listens, or where it was
/// asked to and why it could not.
fn listener_dto(listener: &ListenerStatus) -> ListenerDto {
    match &listener.state {
        ListenerState::Listening(address) => ListenerDto {
            name: listener.name.to_owned(),
            address: address.to_string(),
            state: "listening".to_owned(),
            error: None,
        },
        ListenerState::Failed(reason) => ListenerDto {
            name: listener.name.to_owned(),
            address: listener.requested.to_string(),
            state: "failed".to_owned(),
            error: Some(reason.clone()),
        },
    }
}

impl EngineHandle {
    /// The store the receivers fill. It stays usable after shutdown, with
    /// whatever it held.
    pub fn store(&self) -> &Store {
        &self.shared.state.store
    }

    /// A snapshot: pause, endpoints, rates and what the store holds.
    pub fn status(&self) -> EngineStatus {
        self.shared.state.snapshot()
    }

    /// While paused, the receivers keep answering success and everything
    /// that arrives is dropped (design §4). Resuming keeps what arrives next;
    /// what was dropped stays dropped.
    pub fn set_paused(&self, paused: bool) {
        self.shared.state.paused.store(paused, Ordering::Release);
    }

    pub fn is_paused(&self) -> bool {
        self.shared.state.paused.load(Ordering::Acquire)
    }

    /// Where agents reach the MCP server, `http://127.0.0.1:{port}/mcp` with
    /// the port it really bound (the one to give `claude mcp add`). `None`
    /// when the MCP port could not be bound.
    pub fn mcp_url(&self) -> Option<String> {
        self.shared
            .state
            .mcp_address()
            .map(|address| format!("http://127.0.0.1:{}/mcp", address.port()))
    }

    /// Stops the receivers and the engine's threads. Safe to call again, from
    /// any clone, and from inside a tokio runtime (where it does not wait for
    /// the threads, since blocking there would panic). Requests in flight
    /// when it is called may be cut off: exporters retry.
    pub fn shutdown(&self) {
        self.shared.stop(Handle::try_current().is_err());
    }
}

impl Shared {
    /// Stops the engine once. With `wait`, gives the runtime's threads
    /// [`SHUTDOWN_GRACE`] to finish; without, abandons them.
    fn stop(&self, wait: bool) {
        let running = self
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let Some(Running {
            runtime,
            ingest,
            mcp,
        }) = running
        else {
            return;
        };
        ingest.shutdown();
        mcp.shutdown();
        if wait {
            runtime.shutdown_timeout(SHUTDOWN_GRACE);
        } else {
            runtime.shutdown_background();
        }
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        // The last handle went away without a shutdown. Dropping a runtime
        // inside another one panics, and this may run anywhere, so never wait.
        self.stop(false);
    }
}
