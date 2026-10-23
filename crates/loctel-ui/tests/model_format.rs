//! The exact strings the main window shows for durations, clock times, rates,
//! error shares and cut-off text (M3 plan, Global Constraints "Formats"). Each
//! table row is a rule stated as a case, so a change to a rule shows up as a
//! failing row with the input in the message.

use loctel_ui::model::format::{
    bytes, clock, duration, error_pct, local_utc_offset, rate, truncate_end, truncate_middle,
};

const MS: u64 = 1_000_000;
const SECOND: u64 = 1_000 * MS;

const KIB: u64 = 1_024;
const MIB: u64 = 1_024 * KIB;
const GIB: u64 = 1_024 * MIB;
const TIB: u64 = 1_024 * GIB;

/// 2027-01-15T08:00:00Z, the base time the fake mesh is generated at.
const BASE: u64 = 1_800_000_000 * SECOND;

#[test]
fn durations() {
    let cases: [(u64, &str); 24] = [
        // Under a millisecond: whole microseconds, rounded to the nearest.
        (0, "0 \u{b5}s"),
        (1, "0 \u{b5}s"),
        (499, "0 \u{b5}s"),
        (500, "1 \u{b5}s"),
        (850_000, "850 \u{b5}s"),
        (999_499, "999 \u{b5}s"),
        // The unit is chosen from the exact value and then the number is
        // rounded, so a hair under 1 ms is `1000 \u{b5}s`, never `1 ms`.
        (999_500, "1000 \u{b5}s"),
        // From a millisecond: whole milliseconds, rounded to the nearest.
        (1_000_000, "1 ms"),
        (1_499_999, "1 ms"),
        (1_500_000, "2 ms"),
        (212 * MS, "212 ms"),
        (999 * MS, "999 ms"),
        // Rounding stays in ms below one second: the unit does not roll over.
        (999_999_999, "1000 ms"),
        // From a second: two decimals, rounded half up.
        (SECOND, "1.00 s"),
        (1_004_999_999, "1.00 s"),
        (1_005_000_000, "1.01 s"),
        (1_840 * MS, "1.84 s"),
        (1_845 * MS, "1.85 s"),
        (1_994_999_999, "1.99 s"),
        (1_995_000_000, "2.00 s"),
        (59_990 * MS, "59.99 s"),
        (125 * SECOND, "125.00 s"),
        (3_600 * SECOND, "3600.00 s"),
        (u64::MAX, "18446744073.71 s"),
    ];
    for (nanos, expected) in cases {
        assert_eq!(duration(nanos), expected, "duration({nanos})");
    }
}

#[test]
fn clock_utc() {
    // 08:00:00 UTC plus 6 h 32 min 7 s 412 ms.
    let at = BASE + (6 * 3_600 + 32 * 60 + 7) * SECOND + 412 * MS;
    assert_eq!(clock(at, 0), "14:32:07.412");

    // Milliseconds are cut, not rounded: the clock never runs ahead.
    assert_eq!(clock(at + 999_999, 0), "14:32:07.412");
    assert_eq!(clock(at - 412 * MS, 0), "14:32:07.000");

    // The date is not shown, only the time of day.
    assert_eq!(clock(0, 0), "00:00:00.000");
    assert_eq!(clock(BASE, 0), "08:00:00.000");
    assert_eq!(
        clock(BASE + 24 * 3_600 * SECOND + 5 * MS, 0),
        "08:00:00.005"
    );

    // A zone offset moves the clock, across midnight in both directions.
    assert_eq!(clock(at, 2 * 3_600), "16:32:07.412");
    assert_eq!(clock(at, -(5 * 3_600 + 30 * 60)), "09:02:07.412");
    assert_eq!(clock(0, -3_600), "23:00:00.000");
    assert_eq!(
        clock(23 * 3_600 * SECOND + 30 * 60 * SECOND, 3_600),
        "00:30:00.000"
    );

    // The largest values do not panic.
    assert_eq!(clock(u64::MAX, 0).len(), "HH:MM:SS.mmm".len());
    assert_eq!(clock(u64::MAX, i32::MIN).len(), "HH:MM:SS.mmm".len());
    assert_eq!(clock(u64::MAX, i32::MAX).len(), "HH:MM:SS.mmm".len());
}

#[test]
fn local_offset_is_a_real_zone_offset() {
    // What the zone is depends on the machine; that it is a possible one does
    // not. (Tests pass 0 to `clock` so nothing else depends on this.)
    let offset = local_utc_offset();
    assert!((-24 * 3_600..24 * 3_600).contains(&offset), "{offset}");
}

#[test]
fn rates() {
    let cases: [(f64, &str, &str); 17] = [
        // From 10 whole numbers, below that one decimal.
        (84.0, "req/s", "84 req/s"),
        (0.3, "req/s", "0.3 req/s"),
        (10.0, "req/s", "10 req/s"),
        (10.4, "req/s", "10 req/s"),
        (9.94, "req/s", "9.9 req/s"),
        (1.0, "req/s", "1.0 req/s"),
        (0.0, "req/s", "0.0 req/s"),
        (0.04, "req/s", "0.0 req/s"),
        // A value that rounds up to 10.0 reads `10`, not `10.0`.
        (9.96, "req/s", "10 req/s"),
        (84.5, "req/s", "85 req/s"),
        // No thousands separator and no `k`: that belongs to the status line.
        (1_234.5, "req/s", "1235 req/s"),
        // The unit is whatever the caller says.
        (3.0, "spans/s", "3.0 spans/s"),
        (412.0, "spans/s", "412 spans/s"),
        // Anything that is not a non-negative number reads as zero.
        (f64::NAN, "req/s", "0.0 req/s"),
        (f64::INFINITY, "req/s", "0.0 req/s"),
        (f64::NEG_INFINITY, "req/s", "0.0 req/s"),
        (-3.0, "req/s", "0.0 req/s"),
    ];
    for (per_sec, unit, expected) in cases {
        assert_eq!(rate(per_sec, unit), expected, "rate({per_sec}, {unit:?})");
    }
}

#[test]
fn error_pct_cases() {
    let cases: [(u64, u64, &str); 13] = [
        (2, 95, "2.1% err"),
        (1, 3, "33.3% err"),
        (2, 3, "66.7% err"),
        (1, 8, "12.5% err"),
        (1, 1, "100.0% err"),
        (50, 100, "50.0% err"),
        // No errors, and nothing at all, are both `0%`, with no decimal.
        (0, 100, "0% err"),
        (0, 0, "0% err"),
        // Nothing to divide by.
        (5, 0, "0% err"),
        // An error count that rounds to 0.0 still reads differently from
        // having no errors at all.
        (1, 100_000, "0.0% err"),
        // More errors than total cannot happen; it reads as everything.
        (7, 4, "100.0% err"),
        // Large counts do not overflow.
        (u64::MAX, u64::MAX, "100.0% err"),
        (u64::MAX / 2, u64::MAX, "50.0% err"),
    ];
    for (errors, total, expected) in cases {
        assert_eq!(
            error_pct(errors, total),
            expected,
            "error_pct({errors}, {total})"
        );
    }
}

#[test]
fn truncate_middle_and_end() {
    // The plan's cases.
    assert_eq!(truncate_end("abcdef", 4), "abc\u{2026}");
    assert_eq!(truncate_middle("abcdefghij", 5), "ab\u{2026}ij");

    // Text that fits is returned as it is, whether `max_chars` is the exact
    // length or more.
    assert_eq!(truncate_end("abcd", 4), "abcd");
    assert_eq!(truncate_end("abcd", 40), "abcd");
    assert_eq!(truncate_end("", 5), "");
    assert_eq!(truncate_middle("abcdefghij", 10), "abcdefghij");
    assert_eq!(truncate_middle("abcdefghij", 99), "abcdefghij");
    assert_eq!(truncate_middle("", 5), "");

    // The result is `max_chars` long, the ellipsis included.
    assert_eq!(truncate_end("abcdefghij", 9), "abcdefgh\u{2026}");
    assert_eq!(truncate_middle("abcdefghij", 9), "abcd\u{2026}ghij");
    // With an odd number left to share, the start gets the extra character.
    assert_eq!(truncate_middle("abcdefghij", 6), "abc\u{2026}ij");
    assert_eq!(truncate_middle("abcdefghij", 4), "ab\u{2026}j");

    // Tiny limits: the ellipsis alone, or nothing at all.
    assert_eq!(truncate_end("abcdef", 1), "\u{2026}");
    assert_eq!(truncate_middle("abcdef", 1), "\u{2026}");
    assert_eq!(truncate_end("abcdef", 0), "");
    assert_eq!(truncate_middle("abcdef", 0), "");
    assert_eq!(truncate_end("a", 1), "a");

    // Characters, not bytes: multi-byte text is never cut inside a character.
    assert_eq!(truncate_end("日本語テキスト", 4), "日本語\u{2026}");
    assert_eq!(truncate_middle("日本語テキスト", 5), "日本\u{2026}スト");
    assert_eq!(truncate_end("héllo wörld", 6), "héllo\u{2026}");
    assert_eq!(truncate_end("日本語", 3), "日本語");

    // Review focus 5: a 300-character span name and a 20 KB attribute value.
    let name = format!("GET /{}", "x".repeat(295));
    assert_eq!(name.chars().count(), 300);
    let end = truncate_end(&name, 48);
    assert_eq!(end.chars().count(), 48);
    assert!(end.starts_with("GET /xxx") && end.ends_with('\u{2026}'));
    let middle = truncate_middle(&name, 48);
    assert_eq!(middle.chars().count(), 48);
    assert!(middle.starts_with("GET /xxx") && middle.ends_with("xxx"));
    assert!(middle.contains('\u{2026}'));

    let value: String = (0..20_000)
        .map(|i| char::from(b'a' + (i % 26) as u8))
        .collect();
    let end = truncate_end(&value, 80);
    assert_eq!(end.chars().count(), 80);
    assert!(end.starts_with(&value[..79]));
    let middle = truncate_middle(&value, 80);
    assert_eq!(middle.chars().count(), 80);
    assert!(middle.starts_with(&value[..40]));
    assert!(middle.ends_with(&value[value.len() - 39..]));
}

#[test]
fn bytes_format() {
    let cases: [(u64, &str); 30] = [
        // Under a kilobyte: whole bytes.
        (0, "0 B"),
        (1, "1 B"),
        (512, "512 B"),
        (1_023, "1023 B"),
        // From a kilobyte the units step by 1024. One decimal under 10, and a
        // value that is whole at that precision has no `.0`: the cap of 4 GiB
        // reads `4 GB`, which is how the popover and Settings say it.
        (KIB, "1 KB"),
        (KIB + KIB / 2, "1.5 KB"),
        (9 * KIB + KIB / 2, "9.5 KB"),
        // From 10 whole numbers.
        (10 * KIB, "10 KB"),
        (512 * KIB, "512 KB"),
        (MIB, "1 MB"),
        (MIB + MIB / 4, "1.3 MB"),
        (250 * MIB, "250 MB"),
        (GIB, "1 GB"),
        (GIB * 13 / 10, "1.3 GB"),
        (GIB * 27 / 10, "2.7 GB"),
        (4 * GIB, "4 GB"),
        (12 * GIB, "12 GB"),
        (100 * GIB, "100 GB"),
        (TIB, "1 TB"),
        (TIB + TIB / 2, "1.5 TB"),
        // A value that rounds to 10 reads `10`, not `10.0`, and one that
        // rounds to a whole reads without its decimal.
        (GIB * 996 / 100, "10 GB"),
        (GIB * 994 / 100, "9.9 GB"),
        (GIB * 104 / 100, "1 GB"),
        // The unit comes from the exact value and the number is rounded in it,
        // so it never rolls over after rounding.
        (10 * KIB - 1, "10 KB"),
        (MIB - 1, "1024 KB"),
        (GIB - 1, "1024 MB"),
        // The largest unit is the terabyte, with as many digits as it takes,
        // and no thousands separator.
        (TIB * 1_000, "1000 TB"),
        (u64::MAX, "16777216 TB"),
        // The sizes are binary: 1.3 GiB is `1.3 GB`, and 1.3 billion bytes is less.
        (1_395_864_371, "1.3 GB"),
        (1_300_000_000, "1.2 GB"),
    ];
    for (value, expected) in cases {
        assert_eq!(bytes(value), expected, "bytes({value})");
    }
}
