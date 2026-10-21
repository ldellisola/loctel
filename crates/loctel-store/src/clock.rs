//! Time sources. The store reads "now" from a [`Clock`] so that tests can
//! control trace completeness without sleeping.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Where the store gets the current time, in nanoseconds since the Unix
/// epoch.
pub trait Clock: Send + Sync {
    fn now_unix_nano(&self) -> u64;
}

/// The wall clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_unix_nano(&self) -> u64 {
        // A system clock set before 1970 reads as 0.
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, saturating_nanos)
    }
}

/// A clock that only moves when told to. Share it as `Arc<ManualClock>`: the
/// store gets a clone coerced to `Arc<dyn Clock>`, the test keeps the other.
#[derive(Debug)]
pub struct ManualClock {
    now: AtomicU64,
}

impl ManualClock {
    /// A clock reading `start` nanoseconds since the Unix epoch.
    pub fn new(start: u64) -> Self {
        Self {
            now: AtomicU64::new(start),
        }
    }

    /// Moves the clock forward, stopping at `u64::MAX` rather than wrapping.
    pub fn advance(&self, by: Duration) {
        let by = saturating_nanos(by);
        // The closure always returns `Some`, so the update cannot fail.
        let _ = self
            .now
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |now| {
                Some(now.saturating_add(by))
            });
    }

    /// Jumps to `now` nanoseconds since the Unix epoch, backwards included.
    pub fn set(&self, now: u64) {
        self.now.store(now, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now_unix_nano(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
}

/// A duration in nanoseconds, clamped to what a `u64` holds (about 584
/// years).
pub(crate) fn saturating_nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}
