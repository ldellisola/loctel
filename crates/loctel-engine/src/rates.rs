//! Items per second over a short sliding window, for the status box and the
//! menu ("receiving 1,200 spans/s").

use std::sync::Mutex;

const NANOS_PER_SEC: u64 = 1_000_000_000;

/// The window the rate is averaged over.
const WINDOW_NANOS: u64 = 5 * NANOS_PER_SEC;

/// One slot covers 100 ms.
const BUCKET_NANOS: u64 = NANOS_PER_SEC / 10;

/// 64 slots hold 6.4 s, a little more than the window, so every bucket of the
/// window has a slot of its own.
const SLOTS: usize = 64;

/// How many items arrived in the last 5 seconds, divided by 5.
///
/// Items are counted in 100 ms buckets, so the edge of the window is blurred
/// by at most 100 ms. Time is passed in (nanoseconds since the Unix epoch),
/// never read, so tests do not sleep and the engine uses the same clock for
/// recording and reading. All methods take `&self`: share a meter between the
/// threads that record and the one that reads, in an `Arc`.
///
/// The first five seconds after start understate the true rate, as any
/// sliding window does. Records may arrive out of order (two threads read the
/// clock and record in the other order) and the clock may be set back; neither
/// panics or counts wrongly for more than the slot concerned.
#[derive(Debug)]
pub struct RateMeter {
    slots: Mutex<[Slot; SLOTS]>,
}

/// The count for one bucket. `bucket` is the time divided by the bucket
/// width, and tells a live slot from one left over from 6.4 s earlier.
#[derive(Clone, Copy, Debug, Default)]
struct Slot {
    bucket: u64,
    count: u64,
}

impl RateMeter {
    pub fn new() -> RateMeter {
        RateMeter {
            slots: Mutex::new([Slot::default(); SLOTS]),
        }
    }

    /// Notes that `count` items arrived at `at_unix_nano`.
    pub fn record(&self, at_unix_nano: u64, count: u64) {
        let bucket = at_unix_nano / BUCKET_NANOS;
        let mut slots = self.lock();
        let slot = &mut slots[slot_index(bucket)];
        if slot.bucket == bucket {
            slot.count = slot.count.saturating_add(count);
        } else {
            // The slot held another bucket, 6.4 s older (or newer, if the
            // clock went back): that one is out of the window now.
            *slot = Slot { bucket, count };
        }
    }

    /// Items per second over the 5 seconds up to `now_unix_nano`. Items
    /// recorded as happening after `now` are not counted yet.
    pub fn per_sec(&self, now_unix_nano: u64) -> f64 {
        let slots = self.lock();
        let total = slots
            .iter()
            .filter(|slot| slot.count > 0 && in_window(slot.bucket, now_unix_nano))
            .fold(0_u64, |total, slot| total.saturating_add(slot.count));
        // Rounding above 2^53 items is irrelevant to a display rate.
        total as f64 / (WINDOW_NANOS / NANOS_PER_SEC) as f64
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, [Slot; SLOTS]> {
        // The counts stay usable if a thread panicked while holding the lock.
        self.slots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Default for RateMeter {
    fn default() -> Self {
        RateMeter::new()
    }
}

fn slot_index(bucket: u64) -> usize {
    // The remainder is below SLOTS, so it fits in a usize.
    (bucket % SLOTS as u64) as usize
}

/// A bucket is in the window when it started no later than `now` and ends
/// within the last 5 seconds. Computed wide, so that times near `u64::MAX`
/// do not overflow.
fn in_window(bucket: u64, now: u64) -> bool {
    let start = u128::from(bucket) * u128::from(BUCKET_NANOS);
    start <= u128::from(now) && start + u128::from(WINDOW_NANOS) > u128::from(now)
}
