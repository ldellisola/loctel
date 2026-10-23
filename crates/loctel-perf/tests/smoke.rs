//! The scenarios, at a tiny scale (`Scale::SMOKE`), in a debug build: they run,
//! they find the data they built, and every budget they are asked for comes out
//! with a number. What the numbers are means nothing here (the budgets are for
//! release builds on full data, `cargo xtask perf`); that the scenarios do not
//! break is what the gate holds.

use loctel_perf::budget::{BUDGETS, Measurement, Outcome, Report, budget_ids};
use loctel_perf::run::{Options, measure};
use loctel_perf::scale::Scale;
use loctel_perf::wants::Wants;

fn options(only: &str) -> Options {
    Options {
        wants: Wants::only(only).expect("a budget id"),
        scale: Scale::SMOKE,
        loctel: None,
        perf: Some(env!("CARGO_BIN_EXE_loctel-perf").into()),
    }
}

fn value_of(measurements: &[Measurement], measure_text: &str) -> f64 {
    let found = measurements
        .iter()
        .find(|m| m.budget.measure.contains(measure_text))
        .unwrap_or_else(|| panic!("no measurement of `{measure_text}`"));
    match found.outcome {
        Outcome::Value(value) => value,
        Outcome::NotMeasured(ref why) => panic!("`{measure_text}` was not measured: {why}"),
    }
}

#[test]
fn the_log_budgets_run_over_one_store() {
    for id in ["logs_count", "logs_at", "histogram", "logql_first_page"] {
        let measurements = measure(&options(id)).unwrap_or_else(|e| panic!("{id}: {e:#}"));

        assert_eq!(measurements.len(), 1, "{id}");
        assert!(value_of(&measurements, "") >= 0.0);
    }
}

#[test]
fn the_resident_size_is_measured_on_linux_and_says_why_when_it_is_not() {
    let measurements = measure(&options("rss_10m")).unwrap();

    assert_eq!(measurements.len(), 1);
    match &measurements[0].outcome {
        Outcome::Value(bytes) => assert!(*bytes > 1_000_000.0, "{bytes}"),
        Outcome::NotMeasured(why) => assert!(why.contains("Linux"), "{why}"),
    }
    #[cfg(target_os = "linux")]
    assert!(matches!(measurements[0].outcome, Outcome::Value(_)));
}

#[test]
fn the_trace_budgets_open_a_hot_and_a_cold_trace() {
    for id in ["trace_open_hot", "trace_open_cold", "traceql_first_page"] {
        let measurements = measure(&options(id)).unwrap_or_else(|e| panic!("{id}: {e:#}"));

        assert_eq!(measurements.len(), 1, "{id}");
        assert!(value_of(&measurements, "") >= 0.0);
    }
}

#[test]
fn ingest_sends_and_drops_nothing() {
    let measurements = measure(&options("ingest")).unwrap();

    assert_eq!(measurements.len(), 3);
    assert!(
        value_of(&measurements, "accepted") > 0.0,
        "some items were accepted"
    );
    assert_eq!(
        value_of(&measurements, "not accepted"),
        0.0,
        "and none was dropped"
    );
    assert!(value_of(&measurements, "p99") > 0.0);
}

#[test]
fn the_first_frame_is_drawn_in_a_process_of_its_own() {
    let measurements = measure(&options("first_frame")).unwrap();

    assert_eq!(measurements.len(), 1);
    assert!(value_of(&measurements, "") > 0.0);
}

#[test]
fn startup_needs_the_loctel_binary_and_says_so() {
    let error = measure(&options("startup_headless")).unwrap_err();

    assert!(format!("{error:#}").contains("--loctel"), "{error:#}");
}

#[test]
fn every_budget_id_can_be_asked_for_and_is_in_the_table() {
    assert_eq!(budget_ids().len(), 11);
    for id in budget_ids() {
        assert!(Wants::only(id).is_ok());
    }
    // And a report of one measurement per budget has a row each.
    let all: Vec<Measurement> = BUDGETS
        .iter()
        .map(|budget| Measurement::value(budget, 0.0))
        .collect();
    assert_eq!(Report::new(all).rows().len(), BUDGETS.len());
}
