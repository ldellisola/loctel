//! The rate meter: a 5-second sliding window over what was recorded.

use std::sync::Arc;
use std::thread;

use loctel_engine::RateMeter;

const SEC: u64 = 1_000_000_000;

#[test]
fn rate_meter_window() {
    let meter = RateMeter::new();
    meter.record(0, 100);
    meter.record(4 * SEC, 100);

    // 200 items over the 5 s window.
    assert_eq!(meter.per_sec(4 * SEC), 40.0);
    // The first batch has left the window, the second has not.
    assert_eq!(meter.per_sec(5 * SEC), 20.0);
    assert_eq!(meter.per_sec(8 * SEC), 20.0);
    // Both are gone.
    assert_eq!(meter.per_sec(10 * SEC), 0.0);
}

#[test]
fn rate_meter_empty_is_zero() {
    let meter = RateMeter::default();
    assert_eq!(meter.per_sec(0), 0.0);
    assert_eq!(meter.per_sec(123 * SEC), 0.0);
}

#[test]
fn rate_meter_adds_up_records_in_the_same_instant() {
    let meter = RateMeter::new();
    for _ in 0..10 {
        meter.record(SEC, 10);
    }
    assert_eq!(meter.per_sec(SEC), 20.0);
}

#[test]
fn rate_meter_accepts_records_out_of_order() {
    let meter = RateMeter::new();
    // Two threads read the clock and record in the other order.
    meter.record(3 * SEC, 50);
    meter.record(2 * SEC, 50);
    assert_eq!(meter.per_sec(3 * SEC), 20.0);
}

#[test]
fn rate_meter_does_not_count_what_has_not_happened_yet() {
    let meter = RateMeter::new();
    meter.record(10 * SEC, 100);
    assert_eq!(meter.per_sec(2 * SEC), 0.0);
    assert_eq!(meter.per_sec(10 * SEC), 20.0);
}

#[test]
fn rate_meter_old_records_do_not_leak_through_a_reused_slot() {
    let meter = RateMeter::new();
    meter.record(0, 1_000);
    // A little over the history the meter keeps: the slot of the first
    // record is reused for this one.
    let later = 6_400_000_000;
    meter.record(later, 5);

    assert_eq!(meter.per_sec(later), 1.0);
}

#[test]
fn rate_meter_survives_the_clock_going_backwards() {
    let meter = RateMeter::new();
    meter.record(106_400_000_000, 500);
    // The wall clock is set back by 6.4 s and counting goes on. The new
    // record lands in the slot of the old one and must replace it.
    meter.record(100 * SEC, 100);
    assert_eq!(meter.per_sec(100 * SEC), 20.0);
}

#[test]
fn rate_meter_handles_extreme_timestamps() {
    let meter = RateMeter::new();
    meter.record(u64::MAX, u64::MAX);
    meter.record(u64::MAX, 1);
    // Saturates instead of overflowing or panicking.
    assert!(meter.per_sec(u64::MAX) > 0.0);
    assert_eq!(meter.per_sec(0), 0.0);
}

#[test]
fn rate_meter_is_shared_between_threads() {
    let meter = Arc::new(RateMeter::new());
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let meter = Arc::clone(&meter);
            thread::spawn(move || {
                for _ in 0..1_000 {
                    meter.record(SEC, 1);
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    assert_eq!(meter.per_sec(SEC), 800.0);
}
