//! The loctel engine: everything between the sockets and the UI that is not
//! the store or the receivers themselves.
//!
//! [`Engine::start`] builds a [`Store`], starts the OTLP receivers and the MCP
//! server on a runtime of its own, and returns an [`EngineHandle`]: the one
//! object the menu bar, the window and the headless binary hold. Through it
//! they read the store, ask for a status snapshot, pause and resume receiving,
//! find the MCP address, move the listeners to other ports
//! ([`EngineHandle::rebind`]), read and change the discard rules and shut
//! everything down. [`Settings`] is the `settings.toml` the ports and the disk
//! cap come from; `rules.toml` holds the discard rules, which every batch the
//! receivers accept goes through on its way to the store (design §7).
//!
//! The engine has no UI or graphics dependency, and its `start` needs no async
//! `main`: it owns its threads. See `docs/architecture.md` ("Engine" and
//! "Headless mode") for the data flow.

mod rates;
mod rules_watch;
mod settings;
mod sink;

use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::Duration;

use anyhow::{Context as _, bail};
use loctel_ingest::{DEFAULT_MAX_MESSAGE_BYTES, ReceiverShutdown, Sink};
use loctel_mcp::{
    ListenerDto, McpShutdown, RuleDto, RuleLanguageDto, RuleScopeDto, StatusDto, StatusSource,
};
use loctel_store::{Clock, SystemClock};
use tokio::runtime::{Builder, Handle, Runtime};

pub use loctel_ingest::{ListenerState, ListenerStatus};
pub use loctel_query::QueryError;
pub use loctel_rules::{
    DropScope, PREVIEW_WINDOW, Preview, RuleId, RuleLanguage, RuleSpec, RuleStatus, RulesFile,
};
pub use loctel_store::{Store, StoreConfig, StoreStats};
pub use rates::RateMeter;
pub use rules_watch::RULES_POLL;
pub use settings::Settings;

use rules_watch::Rules;
use sink::EngineSink;

/// How long [`EngineHandle::shutdown`] waits for the engine's threads to
/// finish (from a thread that may block).
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

/// How long [`EngineHandle::rebind`] keeps trying to bind a port that another
/// listener of the engine has only just been told to leave. A server lets go of
/// its socket a moment after it is stopped, not at the instant.
const RELEASE_WAIT: Duration = Duration::from_secs(1);

/// How often [`EngineHandle::rebind`] tries again in that time.
const RELEASE_POLL: Duration = Duration::from_millis(20);

/// The three ports the engine listens on: what [`EngineHandle::rebind`] moves
/// the listeners to. Port 0 asks the system for a free one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ports {
    pub otlp_grpc: u16,
    pub otlp_http: u16,
    pub mcp: u16,
}

impl From<&Settings> for Ports {
    /// The ports `settings` names.
    fn from(settings: &Settings) -> Ports {
        Ports {
            otlp_grpc: settings.otlp_grpc_port,
            otlp_http: settings.otlp_http_port,
            mcp: settings.mcp_port,
        }
    }
}

/// What [`Engine::start`] needs.
#[derive(Clone, Debug)]
pub struct EngineConfig {
    /// The address the OTLP receivers bind to: loopback by default (design
    /// §4). The MCP server is always loopback, whatever this says.
    pub bind_ip: IpAddr,
    /// Ports (and the rest of `settings.toml`).
    pub settings: Settings,
    pub store: StoreConfig,
    /// Where `rules.toml` is. `Some(path)`: the engine writes a file with no
    /// rules there if there is none, loads it, saves it when the rules change
    /// through the handle and reloads it when it is edited (every
    /// [`RULES_POLL`]). `None`: the rules live in memory only and start empty,
    /// which is what tests want and what [`Default`] gives; the app passes
    /// `Some`.
    pub rules_path: Option<PathBuf>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig {
            bind_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            settings: Settings::default(),
            store: StoreConfig::default(),
            rules_path: None,
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
    /// Every discard rule, in file order, with its query error (if the query
    /// does not parse) and what it has dropped.
    pub rules: Vec<RuleStatus>,
    /// Why `rules.toml` could not be used the last time it was read, or
    /// `None`. While it is set the rules in force are the last good ones (none,
    /// if the file was bad from the start).
    pub rules_file_error: Option<String>,
    /// Spans the rules have dropped since launch, whichever rule dropped them
    /// (including rules since removed): the ones that never got into the store
    /// and the ones the store held and lost when their trace was dropped.
    pub dropped_spans: u64,
    /// Log records the rules have dropped since launch, counted the same way.
    pub dropped_logs: u64,
    /// Dropped divided by dropped plus accepted, spans and logs together, since
    /// launch: 0 when nothing has arrived. Items dropped while paused are in
    /// neither.
    pub dropped_fraction: f64,
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
            rules_path,
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
        let accepted = Arc::new(AtomicU64::new(0));
        // A rules file that cannot be used is reported in the status, never an
        // error here: the engine runs without rules.
        let rules = Arc::new(Rules::open(rules_path, Arc::clone(&clock)));

        let sink: Arc<dyn Sink> = Arc::new(EngineSink {
            store: store.clone(),
            paused: Arc::clone(&paused),
            clock: Arc::clone(&clock),
            span_rate: Arc::clone(&span_rate),
            log_rate: Arc::clone(&log_rate),
            rules: Arc::clone(&rules),
            accepted: Arc::clone(&accepted),
            apply: parking_lot::Mutex::new(()),
        });
        let bind = Arc::new(Bind {
            ip: bind_ip,
            sink,
            max_message_bytes: DEFAULT_MAX_MESSAGE_BYTES,
        });
        let (grpc, grpc_stop) = runtime.block_on(bind.grpc(settings.otlp_grpc_port));
        let (http, http_stop) = runtime.block_on(bind.http(settings.otlp_http_port));

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
            rules: Arc::clone(&rules),
            accepted,
            listeners: parking_lot::Mutex::new(Listeners {
                grpc,
                http,
                mcp: None,
            }),
        });
        let (mcp, mcp_stop) = runtime.block_on(bind.mcp(
            settings.mcp_port,
            store,
            state.clone() as Arc<dyn StatusSource>,
        ));
        state.listeners.lock().mcp = Some(mcp);

        // Edits to the rules file are picked up for as long as the engine runs.
        if rules.path().is_some() {
            runtime.spawn(rules_watch::watch(rules));
        }

        Ok(EngineHandle {
            shared: Arc::new(Shared {
                state,
                bind,
                running: Mutex::new(Some(Running {
                    runtime,
                    grpc: Some(grpc_stop),
                    http: Some(http_stop),
                    mcp: Some(mcp_stop),
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
    /// What starting a listener again needs.
    bind: Arc<Bind>,
    /// `None` once stopped. Held for the whole of a rebind, so that two rebinds
    /// (or a rebind and the shutdown) never interleave.
    running: Mutex<Option<Running>>,
}

/// What stopping the engine has to take down: the runtime, and a handle on
/// each listener's server. A handle is `None` only while a rebind is moving that
/// listener.
struct Running {
    runtime: Runtime,
    grpc: Option<ReceiverShutdown>,
    http: Option<ReceiverShutdown>,
    mcp: Option<McpShutdown>,
}

/// What it takes to start a listener, at launch and again when its port moves.
struct Bind {
    /// Where the OTLP receivers bind. MCP is loopback whatever this says.
    ip: IpAddr,
    sink: Arc<dyn Sink>,
    max_message_bytes: usize,
}

impl Bind {
    async fn grpc(&self, port: u16) -> (ListenerStatus, ReceiverShutdown) {
        let addr = SocketAddr::new(self.ip, port);
        loctel_ingest::start_grpc(addr, Arc::clone(&self.sink), self.max_message_bytes).await
    }

    async fn http(&self, port: u16) -> (ListenerStatus, ReceiverShutdown) {
        let addr = SocketAddr::new(self.ip, port);
        loctel_ingest::start_http(addr, Arc::clone(&self.sink), self.max_message_bytes).await
    }

    async fn mcp(
        &self,
        port: u16,
        store: Store,
        status: Arc<dyn StatusSource>,
    ) -> (ListenerStatus, McpShutdown) {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
        loctel_mcp::serve(addr, store, status).await
    }
}

/// Where each listener is, or why it is not. MCP is `None` only in the instant
/// between the engine's start and the MCP server's bind (the server reads the
/// status, so the status exists first).
#[derive(Clone)]
struct Listeners {
    grpc: ListenerStatus,
    http: ListenerStatus,
    mcp: Option<ListenerStatus>,
}

impl Listeners {
    /// The statuses in the usual order: `otlp-grpc`, `otlp-http`, `mcp`.
    fn all(&self) -> Vec<ListenerStatus> {
        [&self.grpc, &self.http]
            .into_iter()
            .chain(&self.mcp)
            .cloned()
            .collect()
    }
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
    /// The discard rules and their file.
    rules: Arc<Rules>,
    /// Spans and logs the sink accepted (after the rules) since launch.
    accepted: Arc<AtomicU64>,
    /// The listeners, which [`EngineHandle::rebind`] moves.
    listeners: parking_lot::Mutex<Listeners>,
}

impl State {
    fn snapshot(&self) -> EngineStatus {
        let now = self.clock.now_unix_nano();
        let dropped = self.rules.set().totals();
        EngineStatus {
            paused: self.paused.load(Ordering::Acquire),
            listeners: self.listeners.lock().all(),
            spans_per_sec: self.span_rate.per_sec(now),
            logs_per_sec: self.log_rate.per_sec(now),
            stats: self.store.stats(),
            rules: self.rules.set().statuses(),
            rules_file_error: self.rules.error(),
            dropped_spans: dropped.spans,
            dropped_logs: dropped.logs,
            dropped_fraction: dropped_fraction(
                dropped.spans.saturating_add(dropped.logs),
                self.accepted.load(Ordering::Relaxed),
            ),
        }
    }

    /// Where the MCP server listens, if it could bind.
    fn mcp_address(&self) -> Option<SocketAddr> {
        match &self.listeners.lock().mcp.as_ref()?.state {
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
            rules: status.rules.iter().map(rule_dto).collect(),
            rules_file_error: status.rules_file_error,
            dropped_fraction: status.dropped_fraction,
        }
    }
}

/// A rule as the MCP status tells it. A LogQL rule has no scope: it only ever
/// drops the log that matched.
fn rule_dto(rule: &RuleStatus) -> RuleDto {
    let spec = &rule.spec;
    let (language, scope) = match spec.language {
        RuleLanguage::TraceQl => (
            RuleLanguageDto::TraceQl,
            Some(match spec.scope {
                DropScope::Trace => RuleScopeDto::Trace,
                DropScope::Span => RuleScopeDto::Span,
            }),
        ),
        RuleLanguage::LogQl => (RuleLanguageDto::LogQl, None),
    };
    RuleDto {
        name: spec.name.clone(),
        language,
        scope,
        enabled: spec.enabled,
        error: rule.error.clone(),
        dropped_spans: rule.dropped_spans,
        dropped_logs: rule.dropped_logs,
        dropped_traces: rule.dropped_traces,
    }
}

/// `dropped / (dropped + accepted)`, or 0 when nothing has arrived.
fn dropped_fraction(dropped: u64, accepted: u64) -> f64 {
    if dropped == 0 {
        return 0.0;
    }
    // Rounding above 2^53 items is irrelevant to a percentage.
    dropped as f64 / (dropped as f64 + accepted as f64)
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

    /// The discard rules, in file order, with their query errors and what
    /// each has dropped. The same list as `status().rules`, without the rest of
    /// the status.
    pub fn rules(&self) -> Vec<RuleStatus> {
        self.shared.state.rules.set().statuses()
    }

    /// Where `rules.toml` is, or `None` when the rules live in memory only.
    pub fn rules_path(&self) -> Option<PathBuf> {
        self.shared.state.rules.path()
    }

    /// How many times the rules have been replaced since launch: every
    /// [`set_rules`](Self::set_rules), every
    /// [`set_rule_enabled`](Self::set_rule_enabled) and every reload of an edited
    /// `rules.toml` adds one (a file that cannot be used replaces nothing, so
    /// adds nothing). Starts at 0. An editor reads it when it opens a rule
    /// and compares it later: a different number means the rules changed behind
    /// its back.
    pub fn rules_generation(&self) -> u64 {
        self.shared.state.rules.generation()
    }

    /// Makes `specs` the rules, in that order, and saves `rules.toml` when
    /// there is a path. A rule that is unchanged (same name, language, query and
    /// scope) keeps its [`RuleId`] and its counters. A query that does not
    /// parse is allowed and shows up as that rule's error in the status, so an
    /// editor can save a draft. Turning a rule on affects data that arrives
    /// from now on: what is stored stays (design §7).
    ///
    /// If the file cannot be written the error comes back and the rules do not
    /// change.
    pub fn set_rules(&self, specs: Vec<RuleSpec>) -> anyhow::Result<()> {
        self.shared.state.rules.set_rules(specs)
    }

    /// Turns the rule `id` on or off and saves like [`set_rules`](Self::set_rules).
    /// An `id` that is not in the list is an error.
    pub fn set_rule_enabled(&self, id: RuleId, enabled: bool) -> anyhow::Result<()> {
        self.shared.state.rules.set_rule_enabled(id, enabled)
    }

    /// What `spec` would have dropped of the data stored in the last
    /// [`PREVIEW_WINDOW`] (5 minutes), ignoring every other rule and whether
    /// `spec` is enabled. Nothing is changed. The error is the query's, with
    /// its position.
    pub fn preview_rule(&self, spec: &RuleSpec) -> Result<Preview, QueryError> {
        let window = u64::try_from(PREVIEW_WINDOW.as_nanos()).unwrap_or(u64::MAX);
        let since = self
            .shared
            .state
            .clock
            .now_unix_nano()
            .saturating_sub(window);
        loctel_rules::preview(spec, &self.shared.state.store, since)
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

    /// Moves the listeners to `ports`. A listener whose port is the one it was
    /// asked for already (the port it was started or last moved with; 0 counts
    /// as a port) is left alone, and keeps serving. Each of the others is
    /// stopped and then bound again at its new port. A bind that fails (the port
    /// is in use, say) leaves that listener as [`ListenerState::Failed`], with
    /// the system's reason, and never touches the other two, which are
    /// not stopped to make room. A failed listener is bound again only when
    /// asked to move to a different port.
    ///
    /// Returns the statuses afterwards, in the usual order (`otlp-grpc`,
    /// `otlp-http`, `mcp`); [`status`](Self::status) says the same from then on.
    /// It blocks until the binds are done, which is quick (a port that another
    /// of these listeners has just left, such as in a swap, is retried for up to
    /// a second while its server lets go of it). Once the engine has been shut
    /// down it changes nothing and returns the statuses as they were.
    ///
    /// Callable from any thread, including one that is running a tokio
    /// runtime.
    pub fn rebind(&self, ports: Ports) -> Vec<ListenerStatus> {
        let shared = &self.shared;
        let mut guard = shared
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let before = shared.state.listeners.lock().clone();
        let Some(running) = guard.as_mut() else {
            return before.all();
        };

        let wanted = [ports.otlp_grpc, ports.otlp_http, ports.mcp];
        let moving = [
            !holds(&before.grpc, wanted[0]),
            !holds(&before.http, wanted[1]),
            before.mcp.as_ref().is_none_or(|mcp| !holds(mcp, wanted[2])),
        ];
        if !moving.contains(&true) {
            return before.all();
        }

        // The ports the moving listeners hold now: free again once their
        // servers notice they were stopped, which a bind of one of them has to
        // allow for.
        let released: Vec<u16> = [Some(&before.grpc), Some(&before.http), before.mcp.as_ref()]
            .into_iter()
            .zip(moving)
            .filter(|(_, moving)| *moving)
            .filter_map(|(status, _)| match &status?.state {
                ListenerState::Listening(address) => Some(address.port()),
                ListenerState::Failed(_) => None,
            })
            .collect();

        // Stop first, bind after: nothing is left listening on a port the
        // user has moved away from.
        if moving[0]
            && let Some(stop) = running.grpc.take()
        {
            stop.shutdown();
        }
        if moving[1]
            && let Some(stop) = running.http.take()
        {
            stop.shutdown();
        }
        if moving[2]
            && let Some(stop) = running.mcp.take()
        {
            stop.shutdown();
        }

        // The binds are async, and this may be called from inside a runtime
        // (where `block_on` would panic): they run on the engine's runtime and
        // the answer comes back over a channel.
        let (done, answer) = mpsc::channel();
        let task = Rebind {
            bind: Arc::clone(&shared.bind),
            store: shared.state.store.clone(),
            status: Arc::clone(&shared.state) as Arc<dyn StatusSource>,
            wanted,
            moving,
            released,
        };
        running.runtime.spawn(task.run(done));
        let Ok(bound) = answer.recv() else {
            // The runtime dropped the task: the engine is going down.
            return before.all();
        };

        let mut listeners = shared.state.listeners.lock();
        if let Some((status, stop)) = bound.grpc {
            listeners.grpc = status;
            running.grpc = Some(stop);
        }
        if let Some((status, stop)) = bound.http {
            listeners.http = status;
            running.http = Some(stop);
        }
        if let Some((status, stop)) = bound.mcp {
            listeners.mcp = Some(status);
            running.mcp = Some(stop);
        }
        listeners.all()
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
            grpc,
            http,
            mcp,
        }) = running
        else {
            return;
        };
        if let Some(stop) = grpc {
            stop.shutdown();
        }
        if let Some(stop) = http {
            stop.shutdown();
        }
        if let Some(stop) = mcp {
            stop.shutdown();
        }
        if wait {
            runtime.shutdown_timeout(SHUTDOWN_GRACE);
        } else {
            runtime.shutdown_background();
        }
    }
}

/// The binds of one [`EngineHandle::rebind`], run on the engine's runtime.
struct Rebind {
    bind: Arc<Bind>,
    store: Store,
    status: Arc<dyn StatusSource>,
    /// The ports asked for, in the order gRPC, HTTP, MCP.
    wanted: [u16; 3],
    /// Which of them move.
    moving: [bool; 3],
    /// The ports the moving listeners were holding.
    released: Vec<u16>,
}

/// What a rebind started: `Some` for each listener that moved.
struct Rebound {
    grpc: Option<(ListenerStatus, ReceiverShutdown)>,
    http: Option<(ListenerStatus, ReceiverShutdown)>,
    mcp: Option<(ListenerStatus, McpShutdown)>,
}

impl Rebind {
    async fn run(self, done: mpsc::Sender<Rebound>) {
        // The receiver is gone only if the caller was; nobody to tell then.
        let _ = done.send(self.start().await);
    }

    async fn start(&self) -> Rebound {
        let [grpc, http, mcp] = self.wanted;
        Rebound {
            grpc: if self.moving[0] {
                Some(self.patient(grpc, || self.bind.grpc(grpc)).await)
            } else {
                None
            },
            http: if self.moving[1] {
                Some(self.patient(http, || self.bind.http(http)).await)
            } else {
                None
            },
            mcp: if self.moving[2] {
                let attempt = || {
                    self.bind
                        .mcp(mcp, self.store.clone(), Arc::clone(&self.status))
                };
                Some(self.patient(mcp, attempt).await)
            } else {
                None
            },
        }
    }

    /// Binds `port` with `attempt`. If that port is one a listener has just
    /// been stopped on, a failure is tried again for up to [`RELEASE_WAIT`],
    /// since the old server may not have let go yet; any other failure stands.
    async fn patient<T, F>(&self, port: u16, attempt: impl Fn() -> F) -> (ListenerStatus, T)
    where
        F: Future<Output = (ListenerStatus, T)>,
    {
        let patient = port != 0 && self.released.contains(&port);
        let deadline = tokio::time::Instant::now() + RELEASE_WAIT;
        loop {
            let (status, stop) = attempt().await;
            let failed = matches!(status.state, ListenerState::Failed(_));
            if !(failed && patient) || tokio::time::Instant::now() >= deadline {
                return (status, stop);
            }
            tokio::time::sleep(RELEASE_POLL).await;
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

/// Whether `status` already answers on `port`: it was asked for that port, or
/// it was asked for port 0 and the system gave it that one. Such a listener
/// does not move on a rebind, so the Settings window can hand back the ports in
/// use without restarting the listeners it did not change.
fn holds(status: &ListenerStatus, port: u16) -> bool {
    port == status.requested.port()
        || matches!(&status.state, ListenerState::Listening(address) if address.port() == port)
}
