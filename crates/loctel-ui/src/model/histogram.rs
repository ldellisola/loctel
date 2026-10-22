//! The Logs screen's volume histogram: the bars and the four time labels under
//! them. The store counts (`Store::log_histogram`: equal buckets over the
//! range, each with its total and its errors); this turns the counts into
//! fractions of the tallest bar, so the view only multiplies by a height.

use loctel_store::HistogramBucket;

use super::format;

/// How many labels sit under the histogram: at the start, a third, two thirds
/// and the end of the range.
pub const LABELS: usize = 4;

/// What the histogram draws.
#[derive(Clone, Debug, PartialEq)]
pub struct Bars {
    /// One entry per bucket, oldest first: `(total, errors)`, each as a
    /// fraction of the tallest bar's total (0 to 1). The errors are stacked at
    /// the bottom of the bar, so they are on the same scale as the total and
    /// never exceed it. All 0 when no bucket holds a log.
    pub bars: Vec<(f32, f32)>,
    /// The clock times (`format::clock`) at the start of the range, a third of
    /// the way, two thirds and the end.
    pub labels: [String; LABELS],
}

/// The bars of `buckets`, which cover `[since, until)` in unix nanoseconds.
/// `utc_offset` is the zone of the labels ([`format::local_utc_offset`] in the
/// app, 0 in tests).
///
/// The labels are cut to a millisecond, not rounded, like every clock time. An
/// empty or reversed range puts all four at `since`.
pub fn bars(buckets: &[HistogramBucket], since: u64, until: u64, utc_offset: i32) -> Bars {
    let tallest = buckets.iter().map(|bucket| bucket.total).max().unwrap_or(0);
    let bars = buckets
        .iter()
        .map(|bucket| {
            if tallest == 0 {
                return (0.0, 0.0);
            }
            // A bar cannot hold more errors than logs; the store never says
            // so, and the stack stays inside the bar if it ever did.
            let errors = bucket.errors.min(bucket.total);
            (fraction(bucket.total, tallest), fraction(errors, tallest))
        })
        .collect();

    let width = u128::from(until.saturating_sub(since));
    let label = |third: u128| {
        // At most `until`, which fits a u64.
        let at = u128::from(since) + width * third / 3;
        format::clock(at as u64, utc_offset)
    };
    Bars {
        bars,
        labels: [label(0), label(1), label(2), label(3)],
    }
}

fn fraction(part: u64, whole: u64) -> f32 {
    (part as f64 / whole as f64) as f32
}
