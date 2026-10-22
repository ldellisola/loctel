//! The `first_frame` budget: how long the main window takes to be built and
//! drawn the first time, through GPUI's headless renderer (software Vulkan,
//! lavapipe, on Linux), in a release build.
//!
//! What is timed is `Driver::new` (the headless platform and window, the main
//! window's construction and its first frame) and the capture of that frame (the
//! render the pixels come out of). The store it shows, with the checkout traces
//! and the health checks, is built before the clock starts.
//!
//! Each run is a process of its own: the first frame of a process pays for
//! what is set up once (the Vulkan driver, the glyph atlases, the pipelines),
//! which is what the first frame of the app pays, and a second window in the
//! same process would not. A run is `loctel-perf --first-frame-once`, which
//! prints `first_frame_ms=<milliseconds>`.

use std::path::Path;
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use gpui::AppContext as _;
use loctel_ingest::{logs_from_request, spans_from_request};
use loctel_meshgen::{Scenario, generate};
use loctel_store::{ManualClock, Store, StoreConfig};
use loctel_ui::Theme;
use loctel_ui::app::MainWindow;
use loctel_ui::testing::{Driver, FixedBackend};

use crate::budget::{FIRST_FRAME, Measurement};
use crate::progress::say;
use crate::stats::median;

/// 2027-01-15T08:00:00Z, where the screenshot tests' failing checkout starts.
const BASE: u64 = 1_800_000_000_000_000_000;
const SECOND: u64 = 1_000_000_000;

/// The window's default size, in logical pixels.
const WINDOW: (u32, u32) = (1440, 900);

/// The line a run prints, before its milliseconds.
pub const PREFIX: &str = "first_frame_ms=";

/// The store the window shows: the home screen's (a failing checkout, a
/// successful one and a health check of each service), complete.
fn home_store() -> Store {
    let clock = Arc::new(ManualClock::new(BASE));
    let store = Store::new(StoreConfig::default(), clock.clone());
    for (scenario, at) in [
        (Scenario::CheckoutFailure, BASE),
        (Scenario::CheckoutSuccess, BASE + 5 * SECOND),
        (Scenario::HealthChecks { count: 1 }, BASE + 10 * SECOND),
    ] {
        for batch in generate(&scenario, 7, at).batches {
            if let Some(request) = batch.traces {
                store.ingest_spans(spans_from_request(request).items);
            }
            if let Some(request) = batch.logs {
                store.ingest_logs(logs_from_request(request).items);
            }
        }
    }
    clock.advance(Duration::from_secs(70));
    store
}

/// Builds the main window and draws it once, in this process, and returns the
/// time that took in milliseconds. Fails if what was drawn is blank.
pub fn first_frame_once() -> Result<f64> {
    let backend = Rc::new(FixedBackend::new(home_store()));
    let started = Instant::now();
    let mut driver = Driver::new(WINDOW, &Theme::dark(), move |window, cx| {
        cx.new(|cx| MainWindow::new(backend, None, window, cx))
    });
    let frame = driver.capture();
    let millis = started.elapsed().as_secs_f64() * 1_000.0;

    // A frame that is one colour is a window that drew nothing, and a fast one.
    let first = *frame.get_pixel(0, 0);
    if frame.dimensions() != (WINDOW.0 * 2, WINDOW.1 * 2) || frame.pixels().all(|p| *p == first) {
        bail!(
            "the first frame is {:?} and blank: the window did not draw",
            frame.dimensions()
        );
    }
    Ok(millis)
}

/// The median of `runs` processes, each started as `perf --first-frame-once`.
pub fn run(perf: &Path, runs: usize) -> Result<Measurement> {
    let mut times = Vec::with_capacity(runs);
    for _ in 0..runs {
        let output = Command::new(perf)
            .arg("--first-frame-once")
            .stdin(Stdio::null())
            .stderr(Stdio::inherit())
            .output()
            .with_context(|| format!("cannot start {}", perf.display()))?;
        if !output.status.success() {
            bail!("a first-frame run failed ({})", output.status);
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let millis = text
            .lines()
            .find_map(|line| line.strip_prefix(PREFIX))
            .and_then(|value| value.trim().parse::<f64>().ok())
            .with_context(|| format!("a first-frame run said no `{PREFIX}` line: {text}"))?;
        times.push(millis);
    }
    let millis = median(&mut times);
    say(format!(
        "first_frame: {millis:.0} ms (runs: {})",
        times
            .iter()
            .map(|t| format!("{t:.0}"))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    Ok(Measurement::value(&FIRST_FRAME, millis))
}
