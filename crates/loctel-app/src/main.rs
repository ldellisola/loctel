//! The `loctel` binary.
//!
//! Headless mode (`--headless`) serves OTLP with no window, prints one JSON
//! line per listener and one for the discard rules, and runs until SIGINT or
//! SIGTERM.
//!
//! In every mode the engine moves traces and logs it no longer needs in memory to
//! temporary files in a directory of its own under the system's temporary
//! directory (`$TMPDIR/loctel/run-<pid>-<time>/`), keeps them under the disk cap
//! of the settings and deletes the directory when it shuts down, which every way
//! of quitting does (a run that was killed leaves its directory, and the next one
//! deletes it). `--no-spill` keeps everything in memory instead.
//!
//! Without `--headless` on macOS it is the product: a menu bar item and no
//! window at launch, no Dock icon (`loctel_macos::install`), running until the
//! popover's `Quit loctel`. A click on the icon opens the popover (status, rates,
//! endpoints, the rules line and the actions); its `Open loctel` opens the main
//! window, and while that or the Settings window is open loctel shows in the Dock
//! and in Cmd-Tab (`loctel_macos::open_or_focus` also runs when the Dock icon is
//! clicked with no window open). It prints the same startup lines as headless
//! mode.
//!
//! Without `--headless` on Linux it opens the main window
//! (`loctel_ui::app::open_main_window`: the service map, the trace table and
//! the trace view, over a `loctel_ui::app::AppBackend` with no login item) and
//! prints the same startup lines; closing the window
//! quits, and so does SIGINT or SIGTERM, with the engine shut down and exit
//! status 0. With no display to open it on, it says so and runs headless
//! instead. That window is a development aid, and how an agent sees the app.
//! (`loctel_ui::StatusView`, the old Linux window, is still there as a
//! standalone view.)
//!
//! On any other OS, running without `--headless` is the same as running with
//! it.

mod cli;

use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;

use anyhow::Context as _;
use clap::Parser as _;
use cli::Cli;
use loctel_engine::{Engine, EngineConfig, EngineHandle, ListenerState, ListenerStatus, Settings};
use serde::Serialize;
use tokio::runtime::Runtime;
use tokio::signal::unix::{Signal, SignalKind, signal};
use tracing_subscriber::EnvFilter;

fn main() -> ExitCode {
    // `parse` prints usage errors and `--help` itself, and exits (2 for a
    // usage error).
    let cli = Cli::parse();
    init_logging();
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("loctel: {error:#}");
            ExitCode::FAILURE
        }
    }
}

/// Library diagnostics (tonic, hyper) go to stderr, quiet unless `RUST_LOG`
/// says otherwise. Stdout is only for the listener lines.
fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    // A second subscriber cannot be installed; that is not worth failing for.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(io::stderr)
        .try_init();
}

fn run(cli: &Cli) -> anyhow::Result<()> {
    let settings_path = cli.settings_path();
    let settings = Settings::load(&settings_path).unwrap_or_else(|error| {
        eprintln!("loctel: {error:#}; using the default settings");
        Settings::default()
    });

    #[cfg(target_os = "linux")]
    if !cli.headless {
        if display_available() {
            return run_window(cli.engine_config(settings), settings_path);
        }
        eprintln!(
            "loctel: no display (DISPLAY and WAYLAND_DISPLAY are unset), so there is no window; \
             running headless"
        );
    }

    #[cfg(target_os = "macos")]
    if !cli.headless {
        return run_menu_bar(cli.engine_config(settings), settings_path);
    }

    // Headless, or an OS with no app shell: serve and print.
    // Listen for the signals before announcing anything: whoever reads the
    // listener lines may signal right away, and a signal that arrives before
    // the handlers exist would kill the process instead of stopping it.
    let signals = ShutdownSignals::install()?;
    let engine = Engine::start(cli.engine_config(settings))?;
    print_startup(&engine)?;

    signals.wait();
    engine.shutdown();
    Ok(())
}

/// The macOS app: the engine, and the menu bar item over it. It starts as an
/// accessory app (no Dock icon, no menu bar of its own, no window) and quits
/// only when the popover says so (`Quit loctel`, or command-Q in it), never
/// because a window closed. The popover's `Open loctel` opens the main window and
/// shows loctel in the Dock for as long as it is open
/// (`loctel_macos::open_or_focus`); a click on the Dock icon with no window
/// open does the same. Prints the listener lines like headless mode does, then
/// runs until Quit.
#[cfg(target_os = "macos")]
fn run_menu_bar(config: EngineConfig, settings_path: std::path::PathBuf) -> anyhow::Result<()> {
    use gpui::{ActivationPolicy, QuitMode};

    let engine = Engine::start(config)?;
    print_startup(&engine)?;

    // The icons of GPUI Kit's components (the main window's) need the assets
    // registered here; the quit mode is why a closing window never ends the
    // app.
    let app = gpui_platform::application()
        .with_assets(loctel_ui::KitAssets)
        .with_quit_mode(QuitMode::Explicit);
    // GPUI calls this when the Dock icon is clicked while no window is open.
    app.on_reopen({
        let engine = engine.clone();
        let settings_path = settings_path.clone();
        move |cx| loctel_macos::open_or_focus(cx, engine.clone(), settings_path.clone())
    });
    app.run({
        let engine = engine.clone();
        move |cx| {
            cx.set_activation_policy(ActivationPolicy::Accessory);
            loctel_macos::install(cx, engine, settings_path);
        }
    });
    engine.shutdown();
    Ok(())
}

/// Whether there is an X11 or Wayland display for a window. Without one, GPUI
/// would hand out windows that draw nothing and the app would sit there
/// invisible.
#[cfg(target_os = "linux")]
fn display_available() -> bool {
    ["DISPLAY", "WAYLAND_DISPLAY"]
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
}

/// The Linux development window: the engine, and the main window over it.
/// Prints the listener lines like headless mode does (so a script can find
/// the ports when they were asked for as 0), then runs the window until it is
/// closed, or until SIGINT or SIGTERM: either way the engine is shut down and
/// the exit status is 0. `settings_path` is the file the window reads and
/// writes the settings in (the `AppBackend`'s).
#[cfg(target_os = "linux")]
fn run_window(config: EngineConfig, settings_path: std::path::PathBuf) -> anyhow::Result<()> {
    use std::cell::RefCell;
    use std::rc::Rc;

    // The handlers come before anything is announced, as in headless mode.
    let signals = ShutdownSignals::install()?;
    let engine = Engine::start(config)?;
    print_startup(&engine)?;

    // A signal is waited for on a thread of its own and told to the window as
    // a message, because only GPUI's own thread can ask the app to quit.
    let (signalled, signal) = tokio::sync::oneshot::channel::<()>();
    std::thread::spawn(move || {
        signals.wait();
        // The receiver is gone once the app has quit; nothing to tell then.
        let _ = signalled.send(());
    });

    let failure: Rc<RefCell<Option<anyhow::Error>>> = Rc::new(RefCell::new(None));
    gpui_platform::application()
        // GPUI Kit's components draw their icons from its assets.
        .with_assets(loctel_ui::KitAssets)
        .run({
            let engine = engine.clone();
            let failure = Rc::clone(&failure);
            move |cx| {
                // Linux has no login item.
                let backend = Rc::new(loctel_ui::app::AppBackend::new(engine, settings_path, None));
                match loctel_ui::app::open_main_window(backend, cx, |_| None) {
                    Ok(_window) => {
                        // It is the only window, so closing it ends the app.
                        cx.on_window_closed(|cx, _| {
                            if cx.windows().is_empty() {
                                cx.quit();
                            }
                        })
                        .detach();
                        cx.spawn(async move |cx| {
                            if signal.await.is_ok() {
                                cx.update(|cx| cx.quit());
                            }
                        })
                        .detach();
                        cx.activate(true);
                    }
                    Err(error) => {
                        *failure.borrow_mut() = Some(error);
                        cx.quit();
                    }
                }
            }
        });
    engine.shutdown();

    match failure.take() {
        None => Ok(()),
        Some(error) => Err(error.context("cannot open the window (`--headless` runs without one)")),
    }
}

/// A listener as the one JSON line printed for it. Field order is the order
/// of the line.
#[derive(Serialize)]
struct ListenerLine<'a> {
    event: &'static str,
    name: &'a str,
    state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
}

fn listener_line(listener: &ListenerStatus) -> String {
    let line = match &listener.state {
        ListenerState::Listening(address) => ListenerLine {
            event: "listener",
            name: listener.name,
            state: "listening",
            address: Some(address.to_string()),
            error: None,
        },
        ListenerState::Failed(error) => ListenerLine {
            event: "listener",
            name: listener.name,
            state: "failed",
            address: None,
            error: Some(error),
        },
    };
    serde_json::to_string(&line).expect("a listener line is always valid JSON")
}

/// The discard rules as the one JSON line printed for them, after the listener
/// lines. Field order is the order of the line.
#[derive(Serialize)]
struct RulesLine<'a> {
    event: &'static str,
    /// Where the rules file is, `null` when the rules live in memory only
    /// (the app always has a path).
    path: Option<String>,
    /// How many rules there are, enabled or not.
    count: usize,
    /// Why the rules file could not be used, `null` when it could.
    error: Option<&'a str>,
}

fn rules_line(path: Option<&Path>, count: usize, error: Option<&str>) -> String {
    let line = RulesLine {
        event: "rules",
        path: path.map(|path| path.to_string_lossy().into_owned()),
        count,
        error,
    };
    serde_json::to_string(&line).expect("a rules line is always valid JSON")
}

/// What stdout carries once the engine is up: a line per listener, then the
/// rules line.
fn print_startup(engine: &EngineHandle) -> anyhow::Result<()> {
    let status = engine.status();
    let mut stdout = io::stdout().lock();
    for listener in &status.listeners {
        writeln!(stdout, "{}", listener_line(listener)).context("cannot write to stdout")?;
    }
    let rules = rules_line(
        engine.rules_path().as_deref(),
        status.rules.len(),
        status.rules_file_error.as_deref(),
    );
    writeln!(stdout, "{rules}").context("cannot write to stdout")?;
    stdout.flush().context("cannot write to stdout")
}

/// SIGINT and SIGTERM, on a small runtime of their own (the engine has its
/// own and must not be started inside this one).
struct ShutdownSignals {
    runtime: Runtime,
    interrupt: Signal,
    terminate: Signal,
}

impl ShutdownSignals {
    fn install() -> anyhow::Result<ShutdownSignals> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("cannot start the signal runtime")?;
        let (interrupt, terminate) = runtime.block_on(async {
            Ok::<_, io::Error>((
                signal(SignalKind::interrupt())?,
                signal(SignalKind::terminate())?,
            ))
        })?;
        Ok(ShutdownSignals {
            runtime,
            interrupt,
            terminate,
        })
    }

    /// Blocks until one of them arrives.
    fn wait(mut self) {
        self.runtime.block_on(async {
            tokio::select! {
                _ = self.interrupt.recv() => {}
                _ = self.terminate.recv() => {}
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::path::Path;

    use super::*;

    fn status(state: ListenerState) -> ListenerStatus {
        ListenerStatus {
            name: "otlp-grpc",
            requested: SocketAddr::from(([127, 0, 0, 1], 4317)),
            state,
        }
    }

    #[test]
    fn listening_line_has_the_documented_shape() {
        let line = listener_line(&status(ListenerState::Listening(SocketAddr::from((
            [127, 0, 0, 1],
            4317,
        )))));
        assert_eq!(
            line,
            r#"{"event":"listener","name":"otlp-grpc","state":"listening","address":"127.0.0.1:4317"}"#
        );
    }

    #[test]
    fn rules_line_has_the_documented_shape() {
        let line = rules_line(Some(Path::new("/tmp/loctel/rules.toml")), 2, None);
        assert_eq!(
            line,
            r#"{"event":"rules","path":"/tmp/loctel/rules.toml","count":2,"error":null}"#
        );
    }

    #[test]
    fn rules_line_carries_the_file_error_on_one_line() {
        let line = rules_line(
            Some(Path::new("/tmp/r.toml")),
            0,
            Some("invalid rules in \"r.toml\" (line 3)"),
        );
        assert_eq!(
            line,
            r#"{"event":"rules","path":"/tmp/r.toml","count":0,"error":"invalid rules in \"r.toml\" (line 3)"}"#
        );
        assert!(!line.contains('\n'));
        // Rules that live in memory only have no path.
        assert_eq!(
            rules_line(None, 0, None),
            r#"{"event":"rules","path":null,"count":0,"error":null}"#
        );
    }

    #[test]
    fn failed_line_has_the_documented_shape_and_escapes_the_error() {
        let line = listener_line(&status(ListenerState::Failed(
            "Address \"already\" in use\n(os error 98)".to_owned(),
        )));
        assert_eq!(
            line,
            r#"{"event":"listener","name":"otlp-grpc","state":"failed","error":"Address \"already\" in use\n(os error 98)"}"#
        );
        assert!(!line.contains('\n'), "one line per listener");
    }
}
