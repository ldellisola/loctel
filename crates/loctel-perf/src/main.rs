//! `loctel-perf`: measures the performance budgets and prints the table.
//! `cargo xtask perf` builds it in release and runs it; this is its command
//! line.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use loctel_perf::budget::Report;
use loctel_perf::frame::{self, PREFIX};
use loctel_perf::run::{Options, measure};
use loctel_perf::scale::Scale;
use loctel_perf::wants::Wants;

/// Measures loctel against its performance budgets (design 10.3) and exits 1 when
/// one is missed. Build it in release: `cargo xtask perf` does.
#[derive(Debug, Parser)]
#[command(name = "loctel-perf", version)]
struct Cli {
    /// Measure only the budgets with this id (`ingest`, `trace_open_hot`,
    /// `trace_open_cold`, `logs_count`, `logs_at`, `histogram`,
    /// `traceql_first_page`, `logql_first_page`, `startup_headless`,
    /// `first_frame` or `rss_10m`).
    #[arg(long, value_name = "ID")]
    only: Option<String>,

    /// Also write the report as JSON to this file.
    #[arg(long, value_name = "PATH")]
    json: Option<PathBuf>,

    /// The release `loctel` binary that `startup_headless` starts. Defaults to
    /// the one next to this program.
    #[arg(long, value_name = "PATH")]
    loctel: Option<PathBuf>,

    /// A tiny run that only shows the scenarios work. Its numbers mean nothing,
    /// and the budgets do not apply to a debug build either.
    #[arg(long)]
    smoke: bool,

    /// Builds the main window, draws it once and prints
    /// `first_frame_ms=<milliseconds>`: one run of `first_frame`, which starts
    /// this program again for each.
    #[arg(long, hide = true)]
    first_frame_once: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if cli.first_frame_once {
        return match frame::first_frame_once() {
            Ok(millis) => {
                println!("{PREFIX}{millis}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("loctel-perf: {error:#}");
                ExitCode::from(2)
            }
        };
    }
    match run(&cli) {
        Ok(code) => ExitCode::from(code),
        Err(message) => {
            eprintln!("loctel-perf: {message}");
            ExitCode::from(2)
        }
    }
}

/// Runs and reports. The exit code is 0 when every budget held, 1 when one was
/// missed; an error (a scenario that could not run) is the caller's 2.
fn run(cli: &Cli) -> Result<u8, String> {
    let wants = match &cli.only {
        Some(id) => Wants::only(id)?,
        None => Wants::all(),
    };
    let perf = std::env::current_exe().map_err(|error| format!("cannot find myself: {error}"))?;
    let loctel = cli
        .loctel
        .clone()
        .or_else(|| perf.parent().map(|dir| dir.join("loctel")));
    let options = Options {
        wants,
        scale: if cli.smoke { Scale::SMOKE } else { Scale::FULL },
        loctel,
        perf: Some(perf),
    };

    let measurements = measure(&options).map_err(|error| format!("{error:#}"))?;
    let report = Report::new(measurements);
    println!("{}", report.table());
    if let Some(path) = &cli.json {
        let json = serde_json::to_string_pretty(&report.to_json())
            .map_err(|error| format!("cannot write the report: {error}"))?;
        std::fs::write(path, json + "\n")
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    }
    Ok(u8::try_from(report.exit_code()).unwrap_or(1))
}
