//! The clocks the store reads "now" from (plan Task 3).

use std::time::Duration;

use loctel_store::{Clock, ManualClock, StoreConfig, SystemClock};

#[test]
fn manual_clock_starts_where_told() {
    assert_eq!(ManualClock::new(42).now_unix_nano(), 42);
}

#[test]
fn manual_clock_advances_by_a_duration() {
    let clock = ManualClock::new(1_000);
    clock.advance(Duration::from_micros(2));
    assert_eq!(clock.now_unix_nano(), 3_000);
    clock.advance(Duration::from_secs(1));
    assert_eq!(clock.now_unix_nano(), 1_000_003_000);
}

#[test]
fn manual_clock_set_jumps_in_either_direction() {
    let clock = ManualClock::new(5_000);
    clock.set(9_000);
    assert_eq!(clock.now_unix_nano(), 9_000);
    clock.set(100);
    assert_eq!(clock.now_unix_nano(), 100);
}

#[test]
fn manual_clock_advance_saturates_instead_of_wrapping() {
    let clock = ManualClock::new(u64::MAX - 5);
    clock.advance(Duration::from_secs(1));
    assert_eq!(clock.now_unix_nano(), u64::MAX);
    // A duration too large for u64 nanoseconds saturates as well.
    let clock = ManualClock::new(0);
    clock.advance(Duration::MAX);
    assert_eq!(clock.now_unix_nano(), u64::MAX);
}

#[test]
fn manual_clock_is_usable_as_a_trait_object_across_threads() {
    let clock: std::sync::Arc<dyn Clock> = std::sync::Arc::new(ManualClock::new(7));
    let seen = {
        let clock = clock.clone();
        std::thread::spawn(move || clock.now_unix_nano())
            .join()
            .unwrap()
    };
    assert_eq!(seen, 7);
}

#[test]
fn system_clock_reports_a_plausible_unix_time() {
    // After 2020-01-01 and before 2200-01-01, in nanoseconds.
    let now = SystemClock.now_unix_nano();
    assert!(now > 1_577_836_800_000_000_000, "too small: {now}");
    assert!(now < 7_258_118_400_000_000_000, "too large: {now}");
}

#[test]
fn system_clock_does_not_go_backwards_across_two_reads() {
    let first = SystemClock.now_unix_nano();
    let second = SystemClock.now_unix_nano();
    assert!(second >= first);
}

#[test]
fn store_config_defaults_match_the_design() {
    let config = StoreConfig::default();
    assert_eq!(config.max_traces, 200_000);
    assert_eq!(config.max_log_records, 5_000_000);
    assert_eq!(config.completeness_quiet, Duration::from_secs(10));
}

#[test]
fn store_reads_now_from_its_own_clock() {
    // The MCP marker tool hands out the store's time, so a test that drives
    // the store with a manual clock gets a marker it controls.
    let clock = std::sync::Arc::new(ManualClock::new(1_800_000_000_000_000_000));
    let store = loctel_store::Store::new(StoreConfig::default(), clock.clone());
    assert_eq!(store.now_unix_nano(), 1_800_000_000_000_000_000);
    clock.advance(Duration::from_secs(3));
    assert_eq!(store.now_unix_nano(), 1_800_000_003_000_000_000);
}
