//! The both-modes harness (Milestone 5, Task 2).
//!
//! The store's public API is frozen in Milestone 5: whatever it answered when
//! everything lived in memory it must answer when completed traces have moved
//! to segment files. So every test that builds a [`Store`] runs its body twice,
//! through [`each_mode`]:
//!
//! 1. **in memory**: `StoreConfig::default()` (no `spill_dir`), exactly as in
//!    Milestones 1 to 4;
//! 2. **spilling**: `StoreConfig::spilling_for_tests`, which spills a trace the
//!    moment it is complete, rolls a segment every 4 KiB and spills early past
//!    16 hot spans, so the cold paths (decode on read, rehydrate on a late
//!    span, whole-segment bookkeeping) run in every test that lets the clock
//!    pass the quiet period.
//!
//! A test file does `mod fixtures; mod modes;` and wraps what follows its
//! store set-up:
//!
//! ```ignore
//! each_mode(|store, clock| {
//!     store.ingest_spans(...);
//!     assert!(...);
//! });
//! ```
//!
//! A body that needs a non-default config calls [`each_mode_with`]: the limits
//! (`max_traces`, `max_log_records`, `completeness_quiet`) are taken from it in
//! both modes, and the spill settings are the test ones in the second.
//!
//! The body must not depend on which mode it is in, and so must not look at
//! the segment files or at what is spilled: tests that do that
//! (`spill.rs`) build their stores themselves.
//!
//! (Cargo also compiles this file as a test binary of its own, with no tests
//! in it.)
#![allow(dead_code)]

use std::sync::Arc;

use loctel_store::{ManualClock, Store, StoreConfig};

/// Where the manual clock starts in both modes: the same time as
/// `fixtures::CLOCK_START` (the two files stand alone, so that either can be
/// declared by a test without the other, and `spill.rs` checks that the numbers
/// agree).
pub const CLOCK_START: u64 = 1_000_000_000_000;

/// Runs `f` on a default store in memory, then on a default store that spills.
/// Each run gets a fresh store and a fresh manual clock at [`CLOCK_START`].
pub fn each_mode(f: impl Fn(Store, Arc<ManualClock>)) {
    each_mode_with(StoreConfig::default(), f);
}

/// [`each_mode`] with the limits of `config`: `max_traces`, `max_log_records`
/// and `completeness_quiet` apply in both modes.
pub fn each_mode_with(config: StoreConfig, f: impl Fn(Store, Arc<ManualClock>)) {
    println!("each_mode: in memory");
    let clock = Arc::new(ManualClock::new(CLOCK_START));
    f(Store::new(config.clone(), clock.clone()), clock);

    println!("each_mode: spilling");
    let dir = tempfile::tempdir().expect("a temporary directory for the segments");
    let spilling = StoreConfig {
        max_traces: config.max_traces,
        max_log_records: config.max_log_records,
        completeness_quiet: config.completeness_quiet,
        ..StoreConfig::spilling_for_tests(dir.path().to_path_buf())
    };
    let clock = Arc::new(ManualClock::new(CLOCK_START));
    f(Store::new(spilling, clock.clone()), clock);
}
