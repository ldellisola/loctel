//! The budgets over traces: `trace_open_hot` and `trace_open_cold` (opening a trace of
//! 100,000 spans: the read and the waterfall's rows) and `traceql_first_page`
//! (the first page of a TraceQL search over ten million spans).

use std::sync::Arc;
use std::time::Instant;

use anyhow::{Result, bail};
use loctel_meshgen::{BULK_START_UNIX_NANO, Scenario, bulk_traces, generate};
use loctel_model::TraceId;
use loctel_store::{ManualClock, PageRequest, Store, StoreConfig, TraceFilter};
use loctel_ui::model::colors::ServiceColors;
use loctel_ui::model::waterfall::Waterfall;

use crate::budget::{Measurement, TRACE_OPEN_COLD, TRACE_OPEN_HOT, TRACEQL_FIRST_PAGE};
use crate::load::{SEED, expect_held, feed_generated, feed_spans, new_store, settle};
use crate::progress::say;
use crate::scale::Scale;
use crate::stats::median;
use crate::wants::Wants;

/// What opening a trace costs: its snapshot from the store, and the rows of its
/// waterfall (what the trace screen does before it draws anything), in
/// milliseconds.
fn open_millis(store: &Store, id: TraceId, spans: usize) -> Result<f64> {
    let started = Instant::now();
    let Some(snapshot) = store.trace(id) else {
        bail!("the trace {id} is not in the store");
    };
    let waterfall = Waterfall::new(&snapshot, &mut ServiceColors::new());
    let millis = started.elapsed().as_secs_f64() * 1_000.0;
    if snapshot.spans.len() != spans || waterfall.rows().is_empty() {
        bail!(
            "the trace {id} has {} spans and {} rows, not {spans} spans",
            snapshot.spans.len(),
            waterfall.rows().len()
        );
    }
    Ok(millis)
}

pub fn open(scale: &Scale, wants: &Wants) -> Result<Vec<Measurement>> {
    let spans = scale.big_trace_spans;
    let mut out = Vec::new();

    if wants.has(&TRACE_OPEN_HOT) {
        say(format!("trace_open_hot: loading a trace of {spans} spans"));
        // In memory: no segment files, so it stays where it was put.
        let clock = Arc::new(ManualClock::new(BULK_START_UNIX_NANO));
        let store = Store::new(StoreConfig::default(), clock.clone());
        let generated = generate(&Scenario::HugeTrace { spans }, SEED, BULK_START_UNIX_NANO);
        feed_generated(&store, &generated);
        settle(&store, &clock);
        let id = generated.trace_ids[0];
        if store.spilled_traces() != 0 {
            bail!("a store with no segment files spilled a trace");
        }
        let mut times = Vec::with_capacity(scale.runs);
        for _ in 0..scale.runs {
            times.push(open_millis(&store, id, spans)?);
        }
        let millis = median(&mut times);
        say(format!("trace_open_hot: {millis:.1} ms"));
        out.push(Measurement::value(&TRACE_OPEN_HOT, millis));
    }

    if wants.has(&TRACE_OPEN_COLD) {
        say(format!(
            "trace_open_cold: loading {} traces of {spans} spans",
            scale.runs
        ));
        let dir = tempfile::tempdir()?;
        let (store, clock) = new_store(dir.path());
        // One trace per run, each read once: a trace read twice comes from the
        // store's small cache of decoded traces, and that is not a cold read.
        let mut ids = Vec::new();
        for run in 0..scale.runs as u64 {
            let generated = generate(
                &Scenario::HugeTrace { spans },
                SEED + run,
                BULK_START_UNIX_NANO,
            );
            feed_generated(&store, &generated);
            ids.push(generated.trace_ids[0]);
        }
        settle(&store, &clock);
        if store.spilled_traces() != ids.len() {
            bail!(
                "{} of {} traces are in segment files after the spill step",
                store.spilled_traces(),
                ids.len()
            );
        }
        let mut times = Vec::with_capacity(ids.len());
        for id in &ids {
            times.push(open_millis(&store, *id, spans)?);
        }
        let millis = median(&mut times);
        say(format!("trace_open_cold: {millis:.1} ms"));
        out.push(Measurement::value(&TRACE_OPEN_COLD, millis));
    }
    Ok(out)
}

pub fn traceql(scale: &Scale, wants: &Wants) -> Result<Vec<Measurement>> {
    if !wants.has(&TRACEQL_FIRST_PAGE) {
        return Ok(Vec::new());
    }
    let dir = tempfile::tempdir()?;
    let (store, clock) = new_store(dir.path());
    say(format!(
        "traceql_first_page: building {} spans",
        scale.searched_spans
    ));
    feed_spans(&store, &clock, bulk_traces(scale.searched_spans, SEED));
    settle(&store, &clock);
    expect_held(&store, scale.searched_spans, 0)?;
    let stats = store.stats();
    say(format!(
        "traceql_first_page: built; {} of {} traces on disk, {} MB of segment files",
        stats.spilled_traces,
        stats.traces,
        stats.disk_bytes / 1_000_000
    ));

    let query = loctel_query::traceql::parse("{ status = error }").map_err(|error| {
        anyhow::anyhow!("the TraceQL query of the budget does not parse: {error}")
    })?;
    let filter = TraceFilter {
        query: Some(Arc::new(query)),
        ..TraceFilter::default()
    };
    let page = PageRequest {
        limit: 20,
        cursor: None,
    };
    let first = store.search_traces(&filter, &page);
    if first.items.len() != 20 || first.items.iter().any(|t| t.matched_span_ids.is_empty()) {
        bail!(
            "the first page of the TraceQL budget has {} traces, not 20 that matched",
            first.items.len()
        );
    }
    let mut times = Vec::with_capacity(scale.runs);
    for _ in 0..scale.runs {
        let started = Instant::now();
        let _ = store.search_traces(&filter, &page);
        times.push(started.elapsed().as_secs_f64() * 1_000.0);
    }
    let millis = median(&mut times);
    say(format!("traceql_first_page: {millis:.1} ms"));
    Ok(vec![Measurement::value(&TRACEQL_FIRST_PAGE, millis)])
}
