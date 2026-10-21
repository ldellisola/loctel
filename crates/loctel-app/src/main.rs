//! The `loctel` binary.
//!
//! Headless mode (`--headless`) serves OTLP with no window, prints one JSON
//! line per listener and runs until SIGINT or SIGTERM.
//!
//! Without `--headless` on macOS it is the product: a menu bar item and no
//! window, no Dock icon (`loctel_macos::install`), running until the menu's
//! Quit. It prints the same listener lines as headless mode.
//!
//! Without `--headless` on Linux it opens one window with the status view
//! (`loctel_ui::StatusView`), refreshed every second, and prints the same
//! listener lines; closing the window quits. With no display to open it on, it
//! says so and runs headless instead. That window is a development aid.
//!
//! On any other OS, running without `--headless` is the same as running with
//! it.

mod cli;

use std::io::{self, Write};
use std::process::ExitCode;

use anyhow::Context as _;
use clap::Parser as _;
use cli::Cli;
use loctel_engine::{Engine, EngineConfig, ListenerState, ListenerStatus, Settings};
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
            return run_window(cli.engine_config(settings));
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
    print_listeners(&engine.status().listeners)?;

    signals.wait();
    engine.shutdown();
    Ok(())
}

/// The macOS app: the engine, and the menu bar item over it. It is an
/// accessory app (no Dock icon, no menu bar of its own) and quits only when
/// the menu says so, never because a window closed: it has none. Prints the
/// listener lines like headless mode does, then runs until Quit.
#[cfg(target_os = "macos")]
fn run_menu_bar(config: EngineConfig, settings_path: std::path::PathBuf) -> anyhow::Result<()> {
    use gpui::{ActivationPolicy, QuitMode};

    let engine = Engine::start(config)?;
    print_listeners(&engine.status().listeners)?;

    gpui_platform::application()
        .with_quit_mode(QuitMode::Explicit)
        .run({
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

/// The Linux development window: the engine, and the status view over it.
/// Prints the listener lines like headless mode does (so a script can find
/// the ports when they were asked for as 0), then runs the window until it is
/// closed.
#[cfg(target_os = "linux")]
fn run_window(config: EngineConfig) -> anyhow::Result<()> {
    use std::cell::RefCell;
    use std::rc::Rc;

    let engine = Engine::start(config)?;
    print_listeners(&engine.status().listeners)?;

    let failure: Rc<RefCell<Option<anyhow::Error>>> = Rc::new(RefCell::new(None));
    gpui_platform::application().run({
        let engine = engine.clone();
        let failure = Rc::clone(&failure);
        move |cx| match loctel_ui::open_status_window(engine, cx) {
            Ok(_window) => {
                // It is the only window, so closing it ends the app.
                cx.on_window_closed(|cx, _| {
                    if cx.windows().is_empty() {
                        cx.quit();
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

fn print_listeners(listeners: &[ListenerStatus]) -> anyhow::Result<()> {
    let mut stdout = io::stdout().lock();
    for listener in listeners {
        writeln!(stdout, "{}", listener_line(listener)).context("cannot write to stdout")?;
    }
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
