//! The preview: what a rule would have dropped of what the store holds.

mod fixtures;

use fixtures::*;
use loctel_meshgen::Scenario;
use loctel_rules::{DropScope, PREVIEW_WINDOW, Preview, preview};
use loctel_store::Store;

/// Time zero of the scenarios below, in nanoseconds. The manual clock of the
/// store is irrelevant to the preview: it works on the data's own times.
const BASE: u64 = 2_000_000_000_000;

/// The failing checkout (15 spans, 7 logs, one trace) and `count` rounds of
/// health checks (5 traces each, one span and one log per trace), stored.
fn stored(health_rounds: usize) -> (World, usize) {
    let w = World::new(vec![]);
    w.store
        .ingest_spans(scenario_spans(&Scenario::CheckoutFailure, 1, BASE));
    w.store
        .ingest_logs(scenario_logs(&Scenario::CheckoutFailure, 1, BASE));
    let health = Scenario::HealthChecks {
        count: health_rounds,
    };
    w.store.ingest_spans(scenario_spans(&health, 2, BASE));
    w.store.ingest_logs(scenario_logs(&health, 2, BASE));
    (w, health_rounds * 5)
}

const HEALTH: &str = r#"{ span.url.path = "/health" }"#;

fn run(store: &Store, spec: &loctel_rules::RuleSpec, since: u64) -> Preview {
    preview(spec, store, since).unwrap_or_else(|err| panic!("{err}"))
}

#[test]
fn preview_counts_traces_spans_logs_and_errors() {
    let (w, health_traces) = stored(2);
    let health_traces = health_traces as u64;
    assert_eq!(health_traces, 10);

    // The health rule: every health trace, its span and its log; none failed.
    let health = run(&w.store, &trace_rule("Health", HEALTH), 0);
    assert_eq!(
        health,
        Preview {
            traces: health_traces,
            spans: health_traces,
            logs: health_traces,
            error_traces: 0,
            error_logs: 0,
        }
    );

    // `{ status = error }` matches the failing checkout: the whole trace, with
    // its 15 spans and 7 logs, 3 of them at error level. The trace failed.
    let failing = run(&w.store, &trace_rule("Errors", "{ status = error }"), 0);
    assert_eq!(
        failing,
        Preview {
            traces: 1,
            spans: 15,
            logs: 7,
            error_traces: 1,
            error_logs: 3,
        }
    );
    assert_eq!(failing.error_traces, 1);

    // A rule is previewed whether or not it is switched on.
    let mut off = trace_rule("Health", HEALTH);
    off.enabled = false;
    assert_eq!(run(&w.store, &off, 0), health);
}

#[test]
fn preview_of_a_log_rule_counts_logs_only() {
    let (w, _) = stored(2);

    let errors = run(&w.store, &log_rule("Errors", r#"{level="error"}"#), 0);
    assert_eq!(
        errors,
        Preview {
            traces: 0,
            spans: 0,
            logs: 3,
            error_traces: 0,
            error_logs: 3,
        }
    );

    // The health access logs are info: dropped, but no errors among them.
    let access = run(
        &w.store,
        &log_rule(
            "Access logs",
            r#"{service_name=~".+"} |= "GET /health 200""#,
        ),
        0,
    );
    assert_eq!(
        access,
        Preview {
            logs: 10,
            ..Preview::default()
        }
    );
}

#[test]
fn preview_of_a_span_scope_rule_counts_the_subtree() {
    let (w, _) = stored(1);

    // `POST /charges` (span 10) and what is below it: spans 10 to 13. Its four
    // logs are in span 10 (two of them at error level), so they go with it; the
    // trace as a whole failed, so it counts as an error trace (the preview says
    // whether the trace had errors, not whether the dropped spans did).
    let charges = run(
        &w.store,
        &span_rule("Charges", r#"{ name = "POST /charges" }"#),
        0,
    );
    assert_eq!(
        charges,
        Preview {
            traces: 1,
            spans: 4,
            logs: 4,
            error_traces: 1,
            error_logs: 2,
        }
    );

    // The same query with the whole trace as the scope.
    let whole = run(
        &w.store,
        &trace_rule("Charges", r#"{ name = "POST /charges" }"#),
        0,
    );
    assert_eq!((whole.traces, whole.spans, whole.logs), (1, 15, 7));
}

#[test]
fn preview_of_a_structural_rule() {
    let (w, _) = stored(1);
    let spec = trace_rule("Cache", r#"{ kind = server } >> { name = "MGET" }"#);

    let result = run(&w.store, &spec, 0);

    assert_eq!((result.traces, result.spans), (1, 15));
    assert_eq!(spec.scope, DropScope::Trace);
}

#[test]
fn preview_respects_since() {
    // Three rounds of health checks, a second apart: round i starts at
    // BASE + i seconds, and its five traces over the next second.
    let w = World::new(vec![]);
    let health = Scenario::HealthChecks { count: 3 };
    w.store.ingest_spans(scenario_spans(&health, 2, BASE));
    w.store.ingest_logs(scenario_logs(&health, 2, BASE));
    let spec = trace_rule("Health", HEALTH);

    let everything = run(&w.store, &spec, 0);
    assert_eq!((everything.traces, everything.logs), (15, 15));

    // From the third round on: five traces.
    let last_round = run(&w.store, &spec, BASE + 2_000_000_000);
    assert_eq!(
        (last_round.traces, last_round.spans, last_round.logs),
        (5, 5, 5)
    );

    // Past all of it: nothing.
    assert_eq!(
        run(&w.store, &spec, BASE + 60_000_000_000),
        Preview::default()
    );
    // The same for logs.
    let logs = log_rule("Access", r#"{service_name=~".+"}"#);
    assert_eq!(run(&w.store, &logs, BASE + 2_000_000_000).logs, 5);
    assert_eq!(run(&w.store, &logs, BASE + 60_000_000_000).logs, 0);
}

#[test]
fn preview_window_is_five_minutes() {
    assert_eq!(PREVIEW_WINDOW, std::time::Duration::from_secs(5 * 60));
}

#[test]
fn preview_invalid_query_is_error() {
    let (w, _) = stored(1);

    let error = preview(&trace_rule("Typo", r#"{ nme = "x" }"#), &w.store, 0).unwrap_err();
    assert!(error.message.contains("unknown intrinsic"), "{error}");
    assert_eq!(error.position, 2);

    let error = preview(&log_rule("Empty", "{}"), &w.store, 0).unwrap_err();
    assert!(error.message.contains("matcher"), "{error}");

    // A query that is not valid in its language: LogQL text as TraceQL.
    assert!(preview(&trace_rule("Wrong", r#"{service_name="x"}"#), &w.store, 0).is_err());
}

#[test]
fn preview_changes_nothing() {
    let (w, _) = stored(1);
    let before = w.store.stats();

    run(&w.store, &trace_rule("Health", HEALTH), 0);
    run(&w.store, &log_rule("All", r#"{service_name=~".+"}"#), 0);

    assert_eq!(w.store.stats(), before);
}

#[test]
fn preview_of_an_empty_store_is_zero() {
    let w = World::new(vec![]);

    assert_eq!(
        run(&w.store, &trace_rule("Health", HEALTH), 0),
        Preview::default()
    );
    assert_eq!(
        run(&w.store, &log_rule("All", r#"{service_name=~".+"}"#), 0),
        Preview::default()
    );
}
