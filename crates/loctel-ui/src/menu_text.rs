//! The words for the menu bar header and the status view's headline. They
//! live here, away from any window code, so the macOS menu and the Linux
//! window say the same thing and a plain test can pin the exact strings.

use crate::StatusModel;

/// How many characters of an unrecognised listener error the status line
/// shows.
const REASON_CHARS: usize = 40;

/// What the engine is doing, in one line:
///
/// - `Port problem: otlp-http (address in use)` when a listener failed (the
///   first one, in the engine's order). This wins over everything else:
///   nothing is being received on that port.
/// - `Paused` when receiving is paused.
/// - `Receiving · 412 spans/s · 1.2k logs/s` otherwise.
///
/// The reason is `address in use` when the operating system's error says
/// "in use", whatever the platform's wording around it, and otherwise the raw
/// error cut to 40 characters.
pub fn status_line(status: &StatusModel) -> String {
    if let Some(listener) = status.listeners.iter().find(|listener| !listener.ok) {
        return format!(
            "Port problem: {} ({})",
            listener.name,
            reason(listener.error.as_deref())
        );
    }
    if status.paused {
        return "Paused".to_owned();
    }
    format!(
        "Receiving \u{b7} {} spans/s \u{b7} {} logs/s",
        format_rate(status.spans_per_sec),
        format_rate(status.logs_per_sec)
    )
}

fn reason(error: Option<&str>) -> String {
    match error {
        None => "unknown error".to_owned(),
        Some(error) if error.to_lowercase().contains("in use") => "address in use".to_owned(),
        Some(error) => error.chars().take(REASON_CHARS).collect(),
    }
}

/// A per-second rate (or a count) in a few characters: `412`, `1.2k`, `10k`,
/// `1.3M`. Below a thousand it is the rounded whole number, so `0.4` is `0`.
/// From a thousand up it has one decimal under ten of a unit and none above,
/// and never a trailing `.0`. Anything that is not a non-negative number
/// reads as `0`.
pub fn format_rate(rate: f64) -> String {
    if !rate.is_finite() || rate < 0.0 {
        return "0".to_owned();
    }
    let rounded = rate.round();
    if rounded < 1_000.0 {
        return format!("{rounded:.0}");
    }
    let thousands = rate / 1_000.0;
    if thousands.round() < 1_000.0 {
        format!("{}k", scaled(thousands))
    } else {
        format!("{}M", scaled(rate / 1_000_000.0))
    }
}

/// A value of at least one unit, with one decimal below ten and none above.
fn scaled(value: f64) -> String {
    let tenths = (value * 10.0).round() as u64;
    if tenths >= 100 {
        format!("{:.0}", value.round())
    } else if tenths.is_multiple_of(10) {
        format!("{}", tenths / 10)
    } else {
        format!("{}.{}", tenths / 10, tenths % 10)
    }
}
