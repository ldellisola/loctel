//! The rows of the trace table: what each of the store's trace summaries
//! looks like as a line of text, with no GPUI in sight.
//!
//! The columns are started, root span, `Failed in`, services (colour dots),
//! span count and duration with a bar (design 9.2). A row holds every one of
//! them as the exact string or number the view draws, so the failure rules of
//! design 9.4 and the formats of [`format`] are tested here and the view only
//! lays them out.

use std::fmt::Write;

use loctel_model::TraceId;
use loctel_store::{FailedIn, TraceStatus, TraceSummary};

use super::colors::ServiceColors;
use super::format;

/// What the root column says while the root span has not arrived: spans come
/// in any order, and a trace is shown from its first span on.
pub const ROOT_NOT_ARRIVED: &str = "(root not arrived)";

/// One line of the trace table.
#[derive(Clone, Debug, PartialEq)]
pub struct TraceRow {
    pub trace_id: TraceId,
    /// The earliest span start as a clock time, `14:32:07.412`.
    pub started: String,
    /// The root span's name, or [`ROOT_NOT_ARRIVED`].
    pub root: String,
    /// The red `Failed in` text ([`failed_in_text`]). Only a failed trace has
    /// it: the root span is erroring. A trace with errors inside has
    /// [`errors_inside`](Self::errors_inside) instead.
    pub failed_in: Option<String>,
    /// The root is fine but another span is erroring: the amber `errors
    /// inside`. Never set together with `failed_in`.
    pub errors_inside: bool,
    /// Each service of the trace, in the order of its first span (the entry
    /// point comes first), with its colour index from [`ServiceColors`].
    pub services: Vec<(String, usize)>,
    pub span_count: usize,
    /// The trace's length as [`format::duration`] writes it.
    pub duration: String,
    /// The length of the duration bar, from 0 to 1: this trace's duration
    /// over the longest of the rows built together. 0 when none has any
    /// length.
    pub duration_fraction: f32,
    /// No new span has arrived for the store's quiet period.
    pub complete: bool,
}

/// The table's rows for `summaries`, in the same order (the store gives them
/// newest first).
///
/// `colors` is asked for every service of every row, in order, so the services
/// of the first rows get the first colours; keep one `ServiceColors` for the
/// whole run and a service never changes colour. `utc_offset` is the zone for
/// the `started` column ([`format::local_utc_offset`] in the app, 0 in tests).
/// The duration bars are relative to the longest of `summaries`, which are the
/// rows shown, not everything the store holds.
pub fn rows(
    summaries: &[TraceSummary],
    colors: &mut ServiceColors,
    utc_offset: i32,
) -> Vec<TraceRow> {
    let longest = summaries
        .iter()
        .map(|summary| summary.duration_nanos)
        .max()
        .unwrap_or(0);
    summaries
        .iter()
        .map(|summary| row(summary, colors, utc_offset, longest))
        .collect()
}

fn row(
    summary: &TraceSummary,
    colors: &mut ServiceColors,
    utc_offset: i32,
    longest: u64,
) -> TraceRow {
    let failed_in = match summary.status {
        TraceStatus::Failed => summary.failed_in.as_ref().map(failed_in_text),
        TraceStatus::ErrorsInside | TraceStatus::Ok => None,
    };
    let duration_fraction = if longest == 0 {
        0.0
    } else {
        (summary.duration_nanos as f64 / longest as f64) as f32
    };
    TraceRow {
        trace_id: summary.trace_id,
        started: format::clock(summary.start_unix_nano, utc_offset),
        root: summary
            .root_name
            .clone()
            .unwrap_or_else(|| ROOT_NOT_ARRIVED.to_owned()),
        failed_in,
        errors_inside: summary.status == TraceStatus::ErrorsInside,
        services: summary
            .services
            .iter()
            .map(|service| (service.clone(), colors.index(service)))
            .collect(),
        span_count: summary.span_count,
        duration: format::duration(summary.duration_nanos),
        duration_fraction,
        complete: summary.complete,
    }
}

/// The text of `Failed in` (design 9.4): `service · span name`, and for a
/// span with a peer (a client or producer span) `service · span name peer`,
/// then ` +N` when `more > 0` other spans failed on their own branches:
/// `payments-api · POST card-gateway +1`. The text is whole; the view cuts it
/// to the width it has.
pub fn failed_in_text(f: &FailedIn) -> String {
    let mut text = format!("{} \u{b7} {}", f.service, f.span_name);
    if let Some(peer) = f.peer.as_deref().filter(|peer| !peer.is_empty()) {
        text.push(' ');
        text.push_str(peer);
    }
    if f.more > 0 {
        // Writing to a String cannot fail.
        let _ = write!(text, " +{}", f.more);
    }
    text
}
