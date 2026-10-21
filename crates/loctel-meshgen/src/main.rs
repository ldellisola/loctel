//! `meshgen`: send a fake mesh scenario to a running OTLP receiver (loctel).
//!
//! ```text
//! meshgen --scenario checkout-failure --transport http-json --endpoint http://127.0.0.1:4318
//! ```

use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use loctel_meshgen::{Scenario, Transport, generate, send};

/// Health checks per service, spans in the huge trace and records in the log
/// flood, unless `--size` says otherwise.
const DEFAULT_HEALTH_CHECKS: usize = 20;
const DEFAULT_HUGE_TRACE_SPANS: usize = 100_000;
const DEFAULT_LOG_FLOOD_RECORDS: usize = 10_000;

/// Send a deterministic fake microservice mesh to an OTLP receiver.
#[derive(Debug, Parser)]
#[command(name = "meshgen", version)]
struct Cli {
    /// What to send.
    #[arg(long, value_enum)]
    scenario: ScenarioArg,

    /// How to send it.
    #[arg(long, value_enum)]
    transport: TransportArg,

    /// The receiver's base URL: the gRPC address, or the HTTP base that
    /// `/v1/traces` and `/v1/logs` hang off (for loctel, http://127.0.0.1:4317
    /// or http://127.0.0.1:4318).
    #[arg(long, value_name = "URL")]
    endpoint: String,

    /// How many times to send the scenario. Every repetition is new data:
    /// fresh trace IDs (the seed plus the repetition number) and timestamps
    /// from the moment it is sent.
    #[arg(long, default_value_t = 1, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    repeat: u32,

    /// Pause between repetitions, in milliseconds.
    #[arg(long, default_value_t = 1000, value_name = "N")]
    interval_ms: u64,

    /// Seed of the first repetition. The same seed gives the same IDs.
    #[arg(long, default_value_t = 1, value_name = "N")]
    seed: u64,

    /// Scale of the scenarios that have one: health checks per service
    /// (default 20), spans in the huge trace (default 100000) or records in
    /// the log flood (default 10000). The checkout scenarios ignore it.
    #[arg(long, value_name = "N")]
    size: Option<usize>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ScenarioArg {
    CheckoutFailure,
    CheckoutSuccess,
    HealthChecks,
    LateOutOfOrder,
    HugeTrace,
    LogFlood,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum TransportArg {
    Grpc,
    HttpProtobuf,
    HttpJson,
}

impl Cli {
    fn scenario(&self) -> Scenario {
        match self.scenario {
            ScenarioArg::CheckoutFailure => Scenario::CheckoutFailure,
            ScenarioArg::CheckoutSuccess => Scenario::CheckoutSuccess,
            ScenarioArg::LateOutOfOrder => Scenario::LateOutOfOrder,
            ScenarioArg::HealthChecks => Scenario::HealthChecks {
                count: self.size.unwrap_or(DEFAULT_HEALTH_CHECKS),
            },
            ScenarioArg::HugeTrace => Scenario::HugeTrace {
                spans: self.size.unwrap_or(DEFAULT_HUGE_TRACE_SPANS),
            },
            ScenarioArg::LogFlood => Scenario::LogFlood {
                records: self.size.unwrap_or(DEFAULT_LOG_FLOOD_RECORDS),
            },
        }
    }

    fn transport(&self) -> Transport {
        let endpoint = self.endpoint.clone();
        match self.transport {
            TransportArg::Grpc => Transport::Grpc(endpoint),
            TransportArg::HttpProtobuf => Transport::HttpProtobuf(endpoint),
            TransportArg::HttpJson => Transport::HttpJson(endpoint),
        }
    }
}

fn now_unix_nano() -> Result<u64> {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("the system clock is before 1970")?;
    u64::try_from(since_epoch.as_nanos()).context("the system clock is too far in the future")
}

#[tokio::main]
async fn main() -> ExitCode {
    // Print the error chain on one line each, not anyhow's debug format,
    // which adds a backtrace whenever RUST_BACKTRACE is set.
    match run(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("meshgen: {error:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<()> {
    let scenario = cli.scenario();
    let transport = cli.transport();

    for repetition in 0..cli.repeat {
        if repetition > 0 {
            tokio::time::sleep(Duration::from_millis(cli.interval_ms)).await;
        }
        let generated = generate(
            &scenario,
            cli.seed.wrapping_add(u64::from(repetition)),
            now_unix_nano()?,
        );
        let report = send(&generated, &transport, 1.0)
            .await
            .with_context(|| format!("sending {scenario:?} to {}", cli.endpoint))?;
        let traces = match generated.trace_ids.as_slice() {
            [] => String::new(),
            [only] => format!(" (trace {only})"),
            many => format!(" ({} traces)", many.len()),
        };
        println!(
            "sent {} spans and {} logs{traces}",
            report.spans, report.logs
        );
    }
    Ok(())
}
