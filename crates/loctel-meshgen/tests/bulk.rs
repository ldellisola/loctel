//! The bulk generators (`bulk_logs`, `bulk_traces`): millions of items,
//! streamed a batch at a time, for the performance budgets of Milestone 5 and
//! the Logs list at ten million lines. They are checked here at a small scale:
//! the shape, determinism, validity for the real converter and the mix of
//! content the budgets' queries rely on.

use std::collections::{HashMap, HashSet};

use loctel_ingest::{logs_from_request, spans_from_request};
use loctel_meshgen::{BULK_START_UNIX_NANO, SERVICES, Scenario, bulk_logs, bulk_traces, generate};
use loctel_model::{LogRecord, Severity, Span, StatusCode, TraceId};
use loctel_store::{ManualClock, Store, StoreConfig, TraceStatus};

const SEED: u64 = 7;

fn all_logs(n: usize, seed: u64) -> Vec<LogRecord> {
    bulk_logs(n, seed)
        .flat_map(|batch| {
            let converted = logs_from_request(batch.logs.expect("a logs batch"));
            assert_eq!(converted.rejected, 0, "generated logs are valid");
            converted.items
        })
        .collect()
}

fn all_spans(n: usize, seed: u64) -> Vec<Span> {
    bulk_traces(n, seed)
        .flat_map(|batch| {
            let converted = spans_from_request(batch.traces.expect("a traces batch"));
            assert_eq!(converted.rejected, 0, "generated spans are valid");
            converted.items
        })
        .collect()
}

#[test]
fn bulk_logs_stream_the_requested_count_in_batches_of_a_thousand() {
    let sizes: Vec<usize> = bulk_logs(2_500, SEED)
        .map(|batch| {
            assert!(batch.traces.is_none(), "logs only");
            assert!(batch.delay_before.is_zero(), "no pauses");
            let request = batch.logs.expect("a logs batch");
            request
                .resource_logs
                .iter()
                .flat_map(|resource| &resource.scope_logs)
                .map(|scope| scope.log_records.len())
                .sum()
        })
        .collect();

    assert_eq!(sizes, [1_000, 1_000, 500]);
    assert_eq!(bulk_logs(0, SEED).count(), 0);
}

#[test]
fn bulk_logs_batch_size_can_change() {
    let batches = bulk_logs(25, SEED).in_batches_of(10).count();

    assert_eq!(batches, 3);
}

#[test]
fn bulk_logs_are_50_000_a_second_from_the_start_time() {
    let logs = all_logs(3_000, SEED);

    // A request groups its logs by service, so the order within a batch is the
    // services', not the clock's: the times are what is fixed.
    let mut times: Vec<u64> = logs.iter().map(|log| log.time_unix_nano).collect();
    times.sort_unstable();
    assert_eq!(times.len(), 3_000);
    assert_eq!(times[0], BULK_START_UNIX_NANO);
    for pair in times.windows(2) {
        assert_eq!(pair[1] - pair[0], 20_000, "50,000 logs a second");
    }

    let later: Vec<u64> = bulk_logs(5, SEED)
        .starting_at(5_000)
        .flat_map(|batch| logs_from_request(batch.logs.unwrap()).items)
        .map(|log| log.time_unix_nano)
        .collect();
    assert_eq!(later.iter().min(), Some(&5_000));
}

#[test]
fn bulk_logs_are_deterministic_and_differ_by_seed() {
    let a = bulk_logs(1_500, SEED).collect::<Vec<_>>();
    let b = bulk_logs(1_500, SEED).collect::<Vec<_>>();
    let c = bulk_logs(1_500, SEED + 1).collect::<Vec<_>>();

    assert_eq!(a, b);
    assert_ne!(a, c);
}

#[test]
fn bulk_logs_have_the_mix_the_budgets_query() {
    let logs = all_logs(20_000, SEED);

    // Every service and every level the Logs screen has a toggle for.
    let services: HashSet<&str> = logs.iter().map(LogRecord::service).collect();
    assert_eq!(services, SERVICES.into_iter().collect::<HashSet<_>>());
    let count_of = |severity| logs.iter().filter(|log| log.severity == severity).count();
    for severity in [
        Severity::Debug,
        Severity::Info,
        Severity::Warn,
        Severity::Error,
    ] {
        assert!(count_of(severity) > 400, "{severity:?} is not rare");
    }
    assert!(count_of(Severity::Info) > count_of(Severity::Error) * 4);

    // `{service_name="payments-api"} |= "timeout"`: a thin slice of one
    // service's logs, so the first page is found among the newest few
    // thousand lines and not by chance in the first one.
    let payments = logs
        .iter()
        .filter(|log| log.service() == "payments-api")
        .count();
    let timeouts = logs
        .iter()
        .filter(|log| log.service() == "payments-api" && log.body.contains("timeout"))
        .count();
    assert!(timeouts > 0, "some payments-api log says timeout");
    assert!(
        timeouts * 20 < payments,
        "{timeouts} of {payments} is not a thin slice"
    );
    assert!(
        logs.iter()
            .filter(|log| log.service() != "payments-api")
            .all(|log| !log.body.contains("timeout")),
        "only payments-api times out"
    );
}

#[test]
fn bulk_logs_some_carry_trace_context_and_none_repeat() {
    let logs = all_logs(20_000, SEED);

    let with_context = logs.iter().filter(|log| log.trace_id.is_some()).count();
    assert!(
        (3_000..7_000).contains(&with_context),
        "about a quarter carry trace context, got {with_context}"
    );
    assert!(
        logs.iter()
            .all(|log| log.trace_id.is_some() == log.span_id.is_some()),
        "a trace ID comes with a span ID"
    );
    let keys: HashSet<u64> = logs.iter().map(LogRecord::dedup_key).collect();
    assert_eq!(keys.len(), logs.len(), "the store would drop none of them");
}

#[test]
fn bulk_traces_are_whole_traces_of_ten_spans_and_batches_of_a_thousand_spans() {
    let sizes: Vec<usize> = bulk_traces(2_500, SEED)
        .map(|batch| {
            assert!(batch.logs.is_none(), "spans only");
            let converted = spans_from_request(batch.traces.expect("a traces batch"));
            assert_eq!(converted.rejected, 0);
            let mut per_trace: HashMap<TraceId, usize> = HashMap::new();
            for span in &converted.items {
                *per_trace.entry(span.trace_id).or_default() += 1;
            }
            let whole = per_trace.values().filter(|&&n| n == 10).count();
            assert_eq!(whole, per_trace.len(), "a batch holds whole traces");
            converted.items.len()
        })
        .collect();

    assert_eq!(sizes, [1_000, 1_000, 500]);
}

#[test]
fn bulk_traces_end_with_a_shorter_trace_when_the_count_is_not_a_multiple() {
    let spans = all_spans(1_003, SEED);

    assert_eq!(spans.len(), 1_003);
    let mut per_trace: HashMap<TraceId, usize> = HashMap::new();
    for span in &spans {
        *per_trace.entry(span.trace_id).or_default() += 1;
    }
    assert_eq!(per_trace.len(), 101);
    assert_eq!(per_trace.values().filter(|&&n| n == 3).count(), 1);
}

#[test]
fn bulk_traces_form_one_tree_each_with_parents_enclosing_children() {
    let spans = all_spans(2_000, SEED);

    let mut by_trace: HashMap<TraceId, Vec<&Span>> = HashMap::new();
    for span in &spans {
        by_trace.entry(span.trace_id).or_default().push(span);
    }
    assert_eq!(by_trace.len(), 200);
    for (trace, members) in by_trace {
        let ids: HashMap<_, _> = members.iter().map(|span| (span.span_id, *span)).collect();
        assert_eq!(ids.len(), members.len(), "unique span IDs in {trace}");
        let roots: Vec<_> = members
            .iter()
            .filter(|span| span.parent_span_id.is_none())
            .collect();
        assert_eq!(roots.len(), 1, "one root in {trace}");
        for span in members {
            let Some(parent) = span.parent_span_id else {
                continue;
            };
            let parent = ids.get(&parent).expect("no orphans");
            assert!(parent.start_unix_nano <= span.start_unix_nano);
            assert!(parent.end_unix_nano >= span.end_unix_nano);
        }
    }
}

#[test]
fn bulk_traces_start_50_000_spans_a_second_apart() {
    let spans = all_spans(1_000, SEED);

    let mut starts: HashMap<TraceId, u64> = HashMap::new();
    for span in &spans {
        let start = starts.entry(span.trace_id).or_insert(u64::MAX);
        *start = (*start).min(span.start_unix_nano);
    }
    let mut starts: Vec<u64> = starts.into_values().collect();
    starts.sort_unstable();
    assert_eq!(starts[0], BULK_START_UNIX_NANO);
    for pair in starts.windows(2) {
        assert_eq!(
            pair[1] - pair[0],
            200_000,
            "a trace of ten spans every 200 us is 50,000 spans a second"
        );
    }
}

#[test]
fn bulk_traces_about_one_in_a_hundred_fails() {
    let spans = all_spans(100_000, SEED);

    let mut failed: HashSet<TraceId> = HashSet::new();
    let mut traces: HashSet<TraceId> = HashSet::new();
    for span in &spans {
        traces.insert(span.trace_id);
        if span.status == StatusCode::Error {
            failed.insert(span.trace_id);
        }
    }
    assert_eq!(traces.len(), 10_000);
    assert!(
        (50..=200).contains(&failed.len()),
        "about 1% of 10,000 traces, got {}",
        failed.len()
    );
}

#[test]
fn bulk_traces_are_deterministic_and_differ_by_seed() {
    let a = bulk_traces(1_500, SEED).collect::<Vec<_>>();
    let b = bulk_traces(1_500, SEED).collect::<Vec<_>>();
    let c = bulk_traces(1_500, SEED + 1).collect::<Vec<_>>();

    assert_eq!(a, b);
    assert_ne!(a, c);
}

#[test]
fn bulk_traces_assemble_and_classify_in_the_store() {
    let store = Store::new(
        StoreConfig::default(),
        std::sync::Arc::new(ManualClock::new(BULK_START_UNIX_NANO)),
    );
    for batch in bulk_traces(20_000, SEED) {
        store.ingest_spans(spans_from_request(batch.traces.unwrap()).items);
    }

    let stats = store.stats();
    assert_eq!(stats.traces, 2_000);
    assert_eq!(stats.spans, 20_000);
    let page = store.search_traces(
        &loctel_store::TraceFilter {
            errors_only: true,
            ..Default::default()
        },
        &loctel_store::PageRequest {
            limit: 500,
            cursor: None,
        },
    );
    assert!(!page.items.is_empty(), "some trace failed");
    assert!(
        page.items
            .iter()
            .all(|summary| summary.status == TraceStatus::Failed),
        "a failing trace fails in its root"
    );
}

#[test]
fn bulk_data_never_shares_ids_with_the_other_scenarios() {
    // The same seed, as the perf harness uses it beside a huge trace.
    let huge = generate(
        &Scenario::HugeTrace { spans: 50 },
        SEED,
        BULK_START_UNIX_NANO,
    );
    let bulk: HashSet<TraceId> = all_spans(2_000, SEED)
        .iter()
        .map(|span| span.trace_id)
        .collect();

    assert!(huge.trace_ids.iter().all(|id| !bulk.contains(id)));
}
