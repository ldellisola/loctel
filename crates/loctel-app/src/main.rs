//! The `loctel` binary.
//!
//! Headless mode (`--headless`) serves OTLP with no window, prints one JSON
//! line per listener and runs until SIGINT or SIGTERM. The menu bar app and
//! the window come with the UI tasks; until then, running without `--headless`
//! does the same.

mod cli;

use std::io::{self, Write};
use std::process::ExitCode;

use anyhow::Context as _;
use clap::Parser as _;
use cli::Cli;
use loctel_engine::{Engine, ListenerState, ListenerStatus, Settings};
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
    // Placeholder until the UI tasks: with or without `--headless`, serve
    // and print.
    let settings_path = cli.settings_path();
    let settings = Settings::load(&settings_path).unwrap_or_else(|error| {
        eprintln!("loctel: {error:#}; using the default settings");
        Settings::default()
    });

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
