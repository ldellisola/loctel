//! The loctel engine: everything between the sockets and the UI that is not
//! the store or the receivers themselves.
//!
//! [`Engine::start`] builds a [`Store`], starts the OTLP receivers on a
//! runtime of its own, and returns an [`EngineHandle`]: the one object the
//! menu bar, the window and the headless binary hold. Through it they read the
//! store, ask for a status snapshot, pause and resume receiving, and shut
//! everything down. [`Settings`] is the `settings.toml` the ports and the
//! disk cap come from.
//!
//! The engine has no UI or graphics dependency, and its `start` needs no async
//! `main`: it owns its threads. See `docs/architecture.md` ("Engine" and
//! "Headless mode") for the data flow.

mod rates;
mod settings;
mod sink;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use anyhow::{Context as _, bail};
use loctel_ingest::{DEFAULT_MAX_MESSAGE_BYTES, IngestConfig, IngestServers};
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
    /// `otlp-http` and, once the MCP server is wired in, `mcp`.
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
    /// the store, and the OTLP receivers.
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
        let listeners = vec![ingest.grpc.clone(), ingest.http.clone()];

        Ok(EngineHandle {
            shared: Arc::new(Shared {
                store,
                paused,
                clock,
                span_rate,
                log_rate,
                listeners,
                running: Mutex::new(Some(Running { runtime, ingest })),
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
    store: Store,
    paused: Arc<AtomicBool>,
    clock: Arc<dyn Clock>,
    span_rate: Arc<RateMeter>,
    log_rate: Arc<RateMeter>,
    listeners: Vec<ListenerStatus>,
    /// `None` once stopped.
    running: Mutex<Option<Running>>,
}

/// What stopping the engine has to take down.
struct Running {
    runtime: Runtime,
    ingest: IngestServers,
}

impl EngineHandle {
    /// The store the receivers fill. It stays usable after shutdown, with
    /// whatever it held.
    pub fn store(&self) -> &Store {
        &self.shared.store
    }

    /// A snapshot: pause, endpoints, rates and what the store holds.
    pub fn status(&self) -> EngineStatus {
        let now = self.shared.clock.now_unix_nano();
        EngineStatus {
            paused: self.is_paused(),
            listeners: self.shared.listeners.clone(),
            spans_per_sec: self.shared.span_rate.per_sec(now),
            logs_per_sec: self.shared.log_rate.per_sec(now),
            stats: self.shared.store.stats(),
        }
    }

    /// While paused, the receivers keep answering success and everything
    /// that arrives is dropped (design §4). Resuming keeps what arrives next;
    /// what was dropped stays dropped.
    pub fn set_paused(&self, paused: bool) {
        self.shared.paused.store(paused, Ordering::Release);
    }

    pub fn is_paused(&self) -> bool {
        self.shared.paused.load(Ordering::Acquire)
    }

    /// Where agents reach the MCP server: `None` until the MCP server is
    /// wired into the engine.
    pub fn mcp_url(&self) -> Option<String> {
        None
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
        let Some(Running { runtime, ingest }) = running else {
            return;
        };
        ingest.shutdown();
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
