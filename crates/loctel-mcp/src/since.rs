//! The `since` argument of the MCP tools.
//!
//! An agent gets a marker from `get_marker` before it sends its request, and
//! passes it back as `since` to ask "what happened after that". A person (or an
//! agent that only knows the wall clock) can pass an RFC 3339 time instead.

use std::fmt;

use chrono::DateTime;

/// What a `since` value looks like when it is not accepted.
const EXPECTED: &str = "expected a marker from get_marker (decimal unix nanoseconds, as a string) \
     or an RFC 3339 time such as 2026-10-21T08:00:00Z";

/// A `since` value that is neither a marker nor an RFC 3339 time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SinceError {
    /// What was given, cut short so an error stays small.
    given: String,
    /// Why it was refused.
    reason: &'static str,
}

impl fmt::Display for SinceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid since {:?}: {}; {EXPECTED}",
            self.given, self.reason
        )
    }
}

impl std::error::Error for SinceError {}

/// Reads `since` as nanoseconds since the Unix epoch.
///
/// - A marker is decimal digits only (`"1800000000000000000"`). One that does
///   not fit in 64 bits is refused.
/// - Anything else must be an RFC 3339 time, with an offset
///   (`"2026-10-21T08:00:00Z"`, `"2026-10-21T10:00:00.5+02:00"`). A time before
///   1970 reads as 0, which is "everything"; one beyond the year 2262, which
///   64-bit nanoseconds cannot hold, is refused.
///
/// A sign, a space, a bare date or the empty string is an error.
pub fn parse_since(text: &str) -> Result<u64, SinceError> {
    let error = |reason| SinceError {
        given: text.chars().take(64).collect(),
        reason,
    };
    if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
        return text
            .parse::<u64>()
            .map_err(|_| error("the marker does not fit in 64 bits"));
    }
    let time = DateTime::parse_from_rfc3339(text)
        .map_err(|_| error("not a marker or an RFC 3339 time"))?;
    match time.timestamp_nanos_opt() {
        Some(nanos) => Ok(u64::try_from(nanos).unwrap_or(0)),
        None if time.timestamp() < 0 => Ok(0),
        None => Err(error("the time is beyond the year 2262")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_marker_is_decimal_nanoseconds() {
        assert_eq!(parse_since("0"), Ok(0));
        assert_eq!(
            parse_since("1800000000000000000"),
            Ok(1_800_000_000_000_000_000)
        );
        assert_eq!(parse_since("007"), Ok(7), "leading zeros are digits too");
        assert_eq!(parse_since("18446744073709551615"), Ok(u64::MAX));
    }

    #[test]
    fn a_marker_too_big_for_64_bits_is_refused() {
        let error = parse_since("18446744073709551616").unwrap_err();
        assert!(error.to_string().contains("64 bits"), "{error}");
    }

    #[test]
    fn rfc3339_times_are_converted_with_their_offset() {
        assert_eq!(
            parse_since("2027-01-15T08:00:00Z"),
            Ok(1_800_000_000_000_000_000)
        );
        assert_eq!(
            parse_since("2027-01-15T09:00:00+01:00"),
            Ok(1_800_000_000_000_000_000)
        );
        assert_eq!(
            parse_since("2027-01-15T08:00:00.123456789Z"),
            Ok(1_800_000_000_123_456_789)
        );
        assert_eq!(
            parse_since("2027-01-15t08:00:00z"),
            Ok(1_800_000_000_000_000_000),
            "RFC 3339 allows lowercase t and z"
        );
        assert_eq!(
            parse_since("2027-01-15 08:00:00Z"),
            Ok(1_800_000_000_000_000_000),
            "and a space for the T"
        );
    }

    #[test]
    fn a_time_before_1970_means_everything() {
        assert_eq!(parse_since("1969-12-31T23:59:59Z"), Ok(0));
        assert_eq!(parse_since("1000-01-01T00:00:00Z"), Ok(0));
        assert_eq!(parse_since("1970-01-01T00:00:00Z"), Ok(0));
    }

    #[test]
    fn a_time_beyond_what_nanoseconds_hold_is_refused() {
        let error = parse_since("2300-01-01T00:00:00Z").unwrap_err();
        assert!(error.to_string().contains("2262"), "{error}");
    }

    #[test]
    fn anything_else_is_refused_and_the_message_says_what_is_expected() {
        for bad in [
            "",
            " ",
            "yesterday",
            "-5",
            "+5",
            "12.5",
            "1e18",
            " 5",
            "5 ",
            "2027-01-15",
            "2027-01-15T08:00:00",
            "0x10",
            "١٢٣",
        ] {
            let error = parse_since(bad).unwrap_err();
            let message = error.to_string();
            assert!(message.starts_with("invalid since"), "{bad:?}: {message}");
            assert!(message.contains("get_marker"), "{bad:?}: {message}");
            assert!(message.contains("RFC 3339"), "{bad:?}: {message}");
        }
    }

    #[test]
    fn a_long_garbage_value_is_not_echoed_whole() {
        let error = parse_since(&"x".repeat(10_000)).unwrap_err();
        assert!(error.to_string().len() < 400, "{}", error.to_string().len());
    }
}
