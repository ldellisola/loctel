//! The `startup_headless` budget: how long `loctel --headless` takes from the
//! moment it is started to the last of its listener lines on stdout (the point
//! at which a script that waits for them can connect).
//!
//! Each run is a fresh process on scratch settings and rules files and a
//! scratch `TMPDIR`, so it never touches the real ones and starts with no run
//! directories to clear. It is stopped with SIGTERM, the way it is asked to
//! quit, which lets it delete its own run directory.

use std::io::{BufRead as _, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};

use crate::budget::{Measurement, STARTUP_HEADLESS};
use crate::progress::say;
use crate::stats::median;

/// The listeners `loctel` prints a line for: OTLP gRPC, OTLP HTTP and MCP.
const LISTENERS: usize = 3;

/// How long a run waits for the lines before it gives up.
const PATIENCE: Duration = Duration::from_secs(30);

pub fn run(loctel: &Path, runs: usize) -> Result<Measurement> {
    let mut times = Vec::with_capacity(runs);
    for _ in 0..runs {
        times.push(start_once(loctel)?);
    }
    let millis = median(&mut times);
    say(format!("startup_headless: {millis:.1} ms"));
    Ok(Measurement::value(&STARTUP_HEADLESS, millis))
}

/// Starts `loctel --headless` once and returns, in milliseconds, the time to
/// its last listener line.
fn start_once(loctel: &Path) -> Result<f64> {
    let scratch = tempfile::tempdir()?;
    let temp = scratch.path().join("tmp");
    std::fs::create_dir_all(&temp)?;

    let started = Instant::now();
    let mut child = Command::new(loctel)
        .args([
            "--headless",
            "--grpc-port",
            "0",
            "--http-port",
            "0",
            "--mcp-port",
            "0",
        ])
        .arg("--settings")
        .arg(scratch.path().join("settings.toml"))
        .arg("--rules")
        .arg(scratch.path().join("rules.toml"))
        .env("TMPDIR", &temp)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("cannot start {}", loctel.display()))?;
    let stdout = child.stdout.take().context("no stdout to read")?;

    // The reader stamps each line when it arrives, so the time is not whenever
    // this thread got round to looking.
    let (lines, arrived) = mpsc::channel();
    let reader = thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let at = Instant::now();
            if lines.send((at, line)).is_err() {
                break;
            }
        }
    });

    let outcome = wait_for_listeners(&arrived, started);
    stop(&mut child);
    let _ = reader.join();
    outcome
}

fn wait_for_listeners(
    arrived: &mpsc::Receiver<(Instant, std::io::Result<String>)>,
    started: Instant,
) -> Result<f64> {
    let mut listening = 0;
    loop {
        match arrived.recv_timeout(PATIENCE) {
            Ok((at, Ok(line))) => {
                if !line.contains(r#""event":"listener""#) {
                    continue;
                }
                if !line.contains(r#""state":"listening""#) {
                    bail!("a listener did not start: {line}");
                }
                listening += 1;
                if listening == LISTENERS {
                    return Ok(at.duration_since(started).as_secs_f64() * 1_000.0);
                }
            }
            Ok((_, Err(error))) => bail!("cannot read loctel's output: {error}"),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                bail!("loctel printed {listening} of {LISTENERS} listener lines in {PATIENCE:?}")
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                bail!("loctel exited after {listening} of {LISTENERS} listener lines")
            }
        }
    }
}

/// Asks the process to quit (SIGTERM) and waits for it; kills it if it will not.
fn stop(child: &mut Child) {
    if let Ok(pid) = i32::try_from(child.id()) {
        // SAFETY: `kill` takes a pid and a signal number and touches no memory
        // of ours; a process that is already gone gives an error we ignore.
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    /// A stand-in for `loctel`: a script that prints what the real one prints
    /// at start and then waits to be stopped.
    fn fake_loctel(dir: &Path, body: &str) -> std::path::PathBuf {
        let path = dir.join("loctel");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    const LINES: &str = r#"
echo '{"event":"listener","name":"otlp-grpc","state":"listening","address":"127.0.0.1:1"}'
echo '{"event":"listener","name":"otlp-http","state":"listening","address":"127.0.0.1:2"}'
echo '{"event":"listener","name":"mcp","state":"listening","address":"127.0.0.1:3"}'
echo '{"event":"rules","path":null,"count":0,"error":null}'
exec sleep 30
"#;

    #[test]
    fn the_time_is_to_the_third_listener_line() {
        let dir = tempfile::tempdir().unwrap();
        let loctel = fake_loctel(dir.path(), LINES);

        let measurement = run(&loctel, 3).unwrap();

        let crate::budget::Outcome::Value(millis) = measurement.outcome else {
            panic!("a value");
        };
        assert!(std::ptr::eq(measurement.budget, &STARTUP_HEADLESS));
        assert!((0.0..5_000.0).contains(&millis), "{millis}");
    }

    #[test]
    fn the_run_waits_for_all_three_listeners() {
        let dir = tempfile::tempdir().unwrap();
        // The third line comes a quarter of a second late.
        let loctel = fake_loctel(
            dir.path(),
            r#"
echo '{"event":"listener","name":"otlp-grpc","state":"listening","address":"127.0.0.1:1"}'
echo '{"event":"listener","name":"otlp-http","state":"listening","address":"127.0.0.1:2"}'
sleep 0.25
echo '{"event":"listener","name":"mcp","state":"listening","address":"127.0.0.1:3"}'
exec sleep 30
"#,
        );

        let millis = start_once(&loctel).unwrap();

        assert!(millis >= 250.0, "{millis}");
    }

    #[test]
    fn a_listener_that_failed_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let loctel = fake_loctel(
            dir.path(),
            r#"echo '{"event":"listener","name":"mcp","state":"failed","error":"in use"}'; exec sleep 30"#,
        );

        let error = start_once(&loctel).unwrap_err();

        assert!(error.to_string().contains("did not start"), "{error:#}");
    }

    #[test]
    fn a_process_that_exits_early_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let loctel = fake_loctel(dir.path(), "exit 3");

        let error = start_once(&loctel).unwrap_err();

        assert!(error.to_string().contains("exited"), "{error:#}");
    }

    #[test]
    fn a_missing_binary_is_an_error_that_names_it() {
        let error = start_once(Path::new("/nonexistent/loctel")).unwrap_err();

        assert!(format!("{error:#}").contains("/nonexistent/loctel"));
    }
}
