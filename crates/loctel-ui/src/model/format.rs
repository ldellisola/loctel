//! The exact strings the main window shows for numbers and clock times, and
//! the two ways to cut text that is too long. They are plain functions of plain
//! values, so a table of cases pins every rule (`tests/model_format.rs`) and
//! the views never format anything themselves.
//!
//! | what | rule | examples |
//! |---|---|---|
//! | [`duration`] | 1 s or more: two decimals; 1 ms or more: whole ms; else whole µs | `1.84 s`, `212 ms`, `850 µs` |
//! | [`clock`] | `HH:MM:SS.mmm` in the given zone | `14:32:07.412` |
//! | [`rate`] | 10 or more: whole; else one decimal | `84 req/s`, `0.3 req/s` |
//! | [`error_pct`] | one decimal; none at all is `0%` | `2.1% err`, `0% err` |
//!
//! All the arithmetic is on integers, so a value on a rounding boundary rounds
//! the same way on every machine (`1.845 s` is `1.85 s`; a binary float would
//! have said `1.84`).

use chrono::Local;

const MICRO: u128 = 1_000;
const MILLI: u128 = 1_000_000;
const SECOND: u128 = 1_000_000_000;

/// The ellipsis that marks cut-off text.
const ELLIPSIS: char = '\u{2026}';

/// A length of time in the fewest digits that still say something.
///
/// The unit comes from the exact value, then the number is rounded to the
/// nearest whole (half up) in that unit:
///
/// - 1 s or more: seconds with two decimals, `1.84 s`.
/// - 1 ms or more: whole milliseconds, `212 ms`.
/// - otherwise: whole microseconds, `850 µs`, and `0 µs` for nothing.
///
/// So the unit never rolls over after rounding: 999,999,999 ns is `1000 ms`,
/// not `1.00 s`, and 999,500 ns is `1000 µs`, not `1 ms`.
pub fn duration(nanos: u64) -> String {
    let nanos = u128::from(nanos);
    if nanos >= SECOND {
        let hundredths = (nanos + SECOND / 200) / (SECOND / 100);
        format!("{}.{:02} s", hundredths / 100, hundredths % 100)
    } else if nanos >= MILLI {
        format!("{} ms", (nanos + MILLI / 2) / MILLI)
    } else {
        format!("{} \u{b5}s", (nanos + MICRO / 2) / MICRO)
    }
}

/// The time of day of a Unix timestamp as `HH:MM:SS.mmm`, in the zone that is
/// `utc_offset_secs` east of UTC (negative for west). The date is not shown.
/// Milliseconds are cut, not rounded, so the clock never runs ahead of the
/// instant: `14:32:07.412999` reads `14:32:07.412`.
///
/// The app passes [`local_utc_offset`]; tests pass 0 and get UTC.
pub fn clock(unix_nano: u64, utc_offset_secs: i32) -> String {
    // At most 1.8e10, which fits an i64 many times over.
    let secs = (unix_nano / 1_000_000_000) as i64;
    let millis = (unix_nano % 1_000_000_000) / 1_000_000;
    // `rem_euclid` keeps the time of day right on the other side of midnight,
    // for a negative offset near the epoch as much as for a positive one.
    let day_secs = (secs + i64::from(utc_offset_secs)).rem_euclid(86_400);
    format!(
        "{:02}:{:02}:{:02}.{millis:03}",
        day_secs / 3_600,
        day_secs % 3_600 / 60,
        day_secs % 60
    )
}

/// How many seconds east of UTC the system's zone is right now, for
/// [`clock`]. It is read at the time of the call, so a trace from before a
/// daylight saving change shows the current offset, which is what a developer
/// comparing it with their wall clock expects. Tests pass 0 to `clock` and
/// never depend on this.
pub fn local_utc_offset() -> i32 {
    Local::now().offset().local_minus_utc()
}

/// A rate and its unit: `84 req/s`, `0.3 req/s`.
///
/// From 10 up it is a whole number (rounded half up, no thousands separator),
/// below that one decimal, so zero is `0.0`. A value that rounds up to 10.0
/// reads `10`. Anything that is not a non-negative number reads as zero, as in
/// `menu_text::format_rate`.
pub fn rate(per_sec: f64, unit: &str) -> String {
    let value = if per_sec.is_finite() && per_sec > 0.0 {
        per_sec
    } else {
        0.0
    };
    let tenths = (value * 10.0).round();
    if tenths >= 100.0 {
        format!("{:.0} {unit}", value.round())
    } else {
        let tenths = tenths as u64;
        format!("{}.{} {unit}", tenths / 10, tenths % 10)
    }
}

/// The share of `total` that is `errors`, to one decimal: `2.1% err`.
///
/// - No errors, and no total at all, are both `0% err`, with no decimal: that
///   string means exactly nothing went wrong.
/// - An error count too small to show (1 in 100,000) reads `0.0% err`, so it
///   can be told from `0% err`.
/// - More errors than total (which cannot happen) reads `100.0% err`.
pub fn error_pct(errors: u64, total: u64) -> String {
    if errors == 0 || total == 0 {
        return "0% err".to_owned();
    }
    let errors = u128::from(errors.min(total));
    let total = u128::from(total);
    let tenths = (errors * 1_000 + total / 2) / total;
    format!("{}.{}% err", tenths / 10, tenths % 10)
}

/// `s` cut to at most `max_chars` characters, the last of them `…` when
/// something was cut: `abcdef` at 4 is `abc…`. Text that fits comes back
/// unchanged. Counts characters (Unicode scalar values), never bytes, so it
/// cannot cut inside one; it does not know about grapheme clusters, so a
/// combining mark at the cut can be left behind. With `max_chars` 0 the result
/// is empty, and with 1 it is just `…`.
pub fn truncate_end(s: &str, max_chars: usize) -> String {
    if fits(s, max_chars) {
        return s.to_owned();
    }
    let Some(keep) = max_chars.checked_sub(1) else {
        return String::new();
    };
    let mut out: String = s.chars().take(keep).collect();
    out.push(ELLIPSIS);
    out
}

/// Like [`truncate_end`], but the `…` goes in the middle and both ends stay:
/// `abcdefghij` at 5 is `ab…ij`. When the characters kept cannot be shared
/// equally the start gets the extra one (`abc…ij` at 6). Good for ids and
/// paths, where the end says as much as the start.
pub fn truncate_middle(s: &str, max_chars: usize) -> String {
    if fits(s, max_chars) {
        return s.to_owned();
    }
    let Some(keep) = max_chars.checked_sub(1) else {
        return String::new();
    };
    let head = keep.div_ceil(2);
    let tail = keep - head;
    let total = s.chars().count();
    let mut out: String = s.chars().take(head).collect();
    out.push(ELLIPSIS);
    out.extend(s.chars().skip(total - tail));
    out
}

/// Whether `s` has at most `max_chars` characters. It stops looking at the
/// first character past the limit, so a huge value is not walked in full just
/// to be cut to a few characters.
fn fits(s: &str, max_chars: usize) -> bool {
    s.chars().nth(max_chars).is_none()
}
