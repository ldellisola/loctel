//! A run: which scenarios the budgets asked for, in the order that keeps them
//! honest, and the measurements they gave.

use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};

use crate::budget::{
    FIRST_FRAME, HISTOGRAM, INGEST_DROPPED, INGEST_P99, INGEST_RATE, LOGQL_FIRST_PAGE, LOGS_AT,
    LOGS_COUNT, Measurement, RSS_10M, STARTUP_HEADLESS, TRACE_OPEN_COLD, TRACE_OPEN_HOT,
    TRACEQL_FIRST_PAGE,
};
use crate::scale::Scale;
use crate::wants::Wants;
use crate::{frame, ingest, logs, progress, startup, traces};

/// What to run.
#[derive(Clone, Debug)]
pub struct Options {
    pub wants: Wants,
    pub scale: Scale,
    /// The release `loctel` that `startup_headless` starts. Needed only when
    /// that budget is measured.
    pub loctel: Option<PathBuf>,
    /// This program, which `first_frame` starts once per run. Needed only when
    /// that budget is measured.
    pub perf: Option<PathBuf>,
}

/// Runs the scenarios of the budgets `options.wants` names, and returns what
/// they measured. An error is a scenario that could not run (or found its data
/// wrong), never a breach.
///
/// The log store comes first: the resident size of `rss_10m` is read there, in a
/// process that has built nothing else, so it is the store that is in it and
/// not what an earlier scenario left behind.
pub fn measure(options: &Options) -> Result<Vec<Measurement>> {
    progress::begin();
    let wants = &options.wants;
    let scale = &options.scale;
    let mut out = Vec::new();

    if wants.any(&[
        &RSS_10M,
        &LOGS_COUNT,
        &LOGS_AT,
        &HISTOGRAM,
        &LOGQL_FIRST_PAGE,
    ]) {
        out.extend(logs::run(scale, wants).context("the log budgets")?);
    }
    if wants.has(&TRACEQL_FIRST_PAGE) {
        out.extend(traces::traceql(scale, wants).context("traceql_first_page")?);
    }
    if wants.any(&[&TRACE_OPEN_HOT, &TRACE_OPEN_COLD]) {
        out.extend(traces::open(scale, wants).context("the trace_open budgets")?);
    }
    if wants.any(&[&INGEST_RATE, &INGEST_DROPPED, &INGEST_P99]) {
        out.extend(ingest::run(scale).context("ingest")?);
    }
    if wants.has(&STARTUP_HEADLESS) {
        let Some(loctel) = &options.loctel else {
            bail!(
                "startup_headless needs the release `loctel` binary: pass `--loctel PATH` \
                 (`cargo xtask perf` builds it and does)"
            );
        };
        out.push(startup::run(loctel, scale.runs).context("startup_headless")?);
    }
    if wants.has(&FIRST_FRAME) {
        let Some(perf) = &options.perf else {
            bail!("first_frame needs the path of this program to start it again");
        };
        out.push(frame::run(perf, scale.runs).context("first_frame")?);
    }
    Ok(out)
}
