//! What the trace view says outside the waterfall and the inspector: the header
//! (the root's name, the failure badge, the line of numbers, the trace ID and
//! the completeness indicator), the find box's count and the rows of the
//! selected span's logs. Plain Rust over the store's types, with no GPUI in it:
//! the view lays out the strings it is given, and every rule of them is tested
//! with plain assertions in `tests/model_trace_view.rs`.

use std::collections::HashSet;

use loctel_model::{LogRecord, Severity};
use loctel_store::{TraceSnapshot, TraceStatus};

use super::format;
use super::trace_table::{self, ROOT_NOT_ARRIVED};
use super::waterfall::extent;

/// What the header's indicator says while the trace may still get spans.
pub const RECEIVING: &str = "Receiving spans\u{2026}";
/// And once it has been quiet for the store's quiet period.
pub const COMPLETE: &str = "Complete";

/// What the header says about a trace, ready to lay out.
#[derive(Clone, Debug, PartialEq)]
pub struct TraceHeader {
    /// The root span's name, or [`ROOT_NOT_ARRIVED`] while the root has not
    /// arrived.
    pub root: String,
    /// The root span has arrived (`root` is its name).
    pub root_arrived: bool,
    /// Whether the trace failed, and where (design 9.4).
    pub failure: Failure,
    /// The line of numbers: `1.84 s · 15 spans · 5 services · 14:32:07.412`
    /// (the length, the span count, the number of services and when the trace
    /// started, in the clock of the table).
    pub summary: String,
    /// The trace ID, whole, in lower-case hex. The view cuts it in the middle.
    pub trace_id: String,
    /// No new span has arrived for the store's quiet period.
    pub complete: bool,
}

/// The trace's failure (design 9.4), as the badge shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// Nothing failed: no badge.
    None,
    /// The root span is erroring: the red badge, with the `service · span name`
    /// text of the span that explains it
    /// ([`trace_table::failed_in_text`]).
    FailedIn(String),
    /// The root is fine (or has not arrived) but another span is erroring: the
    /// amber badge.
    ErrorsInside,
}

impl Failure {
    /// The words of the badge: `Failed in payments-api · POST card-gateway +1`,
    /// `Errors inside`, and `None` for a trace that did not fail.
    pub fn badge(&self) -> Option<String> {
        match self {
            Failure::None => None,
            Failure::FailedIn(text) => Some(format!("Failed in {text}")),
            Failure::ErrorsInside => Some("Errors inside".to_owned()),
        }
    }
}

impl TraceHeader {
    /// What the completeness indicator says: [`RECEIVING`] or [`COMPLETE`].
    pub fn completeness(&self) -> &'static str {
        if self.complete { COMPLETE } else { RECEIVING }
    }
}

/// The header of `snapshot`. `utc_offset` is the zone of the start time
/// ([`format::local_utc_offset`] in the app, 0 in tests).
///
/// The length is the one of the trace table's row (from the earliest start to
/// the latest end), so the two never disagree about the same trace.
pub fn header(snapshot: &TraceSnapshot, utc_offset: i32) -> TraceHeader {
    let spans = &snapshot.spans;
    let (start, end) = extent(spans);
    let root = snapshot
        .root
        .and_then(|id| spans.iter().find(|span| span.span_id == id))
        .map(|span| span.name.clone());
    let services = spans
        .iter()
        .map(|span| span.service())
        .collect::<HashSet<_>>()
        .len();
    let failure = match snapshot.status {
        TraceStatus::Failed => snapshot.failed_in.as_ref().map_or(Failure::None, |failed| {
            Failure::FailedIn(trace_table::failed_in_text(failed))
        }),
        TraceStatus::ErrorsInside => Failure::ErrorsInside,
        TraceStatus::Ok => Failure::None,
    };
    TraceHeader {
        root_arrived: root.is_some(),
        root: root.unwrap_or_else(|| ROOT_NOT_ARRIVED.to_owned()),
        failure,
        summary: format!(
            "{} \u{b7} {} \u{b7} {} \u{b7} {}",
            format::duration(end - start),
            counted(spans.len(), "span"),
            counted(services, "service"),
            format::clock(start, utc_offset)
        ),
        trace_id: snapshot.trace_id.to_string(),
        complete: snapshot.complete,
    }
}

/// `3 spans`, `1 span`, `100,000 spans` (a thousands separator, as in the
/// table's count line).
fn counted(n: usize, noun: &str) -> String {
    let plural = if n == 1 { "" } else { "s" };
    format!("{} {noun}{plural}", trace_table::thousands(n))
}

/// What the find box says about its matches: nothing for an empty box, `No
/// matches`, `1 match` or `5 matches` while none of them is the selected span,
/// and `2 of 5` while the selected span is the second match (`current`, the
/// match's position from 0).
pub fn find_status(text_is_empty: bool, matches: usize, current: Option<usize>) -> String {
    if text_is_empty {
        return String::new();
    }
    match (matches, current) {
        (0, _) => "No matches".to_owned(),
        (total, Some(at)) => format!(
            "{} of {}",
            trace_table::thousands(at + 1),
            trace_table::thousands(total)
        ),
        (1, None) => "1 match".to_owned(),
        (total, None) => format!("{} matches", trace_table::thousands(total)),
    }
}

/// One line of the selected span's logs.
#[derive(Clone, Debug, PartialEq)]
pub struct LogLine {
    /// When the log happened, `14:32:07.412` (its own time, or the observed
    /// time when the exporter left it unset).
    pub time: String,
    /// The level's badge: [`level_badge`].
    pub level: &'static str,
    /// The level the badge is coloured by.
    pub severity: Severity,
    /// The log's body on one line: a line break becomes a space (a stack trace
    /// in a log message is one long line here), and the view cuts it to the
    /// width it has.
    pub message: String,
}

/// The three letters of a level's badge: `TRC`, `DBG`, `INF`, `WRN`, `ERR`,
/// `FTL`, and `LOG` for a log with no level.
pub fn level_badge(severity: Severity) -> &'static str {
    match severity {
        Severity::Unspecified => "LOG",
        Severity::Trace => "TRC",
        Severity::Debug => "DBG",
        Severity::Info => "INF",
        Severity::Warn => "WRN",
        Severity::Error => "ERR",
        Severity::Fatal => "FTL",
    }
}

/// The lines for `logs`, in the order given (the store gives a span's logs
/// oldest first). `utc_offset` is the zone of the times.
pub fn log_lines(logs: &[LogRecord], utc_offset: i32) -> Vec<LogLine> {
    logs.iter()
        .map(|log| {
            let time = if log.time_unix_nano != 0 {
                log.time_unix_nano
            } else {
                log.observed_unix_nano
            };
            LogLine {
                time: format::clock(time, utc_offset),
                level: level_badge(log.severity),
                severity: log.severity,
                message: log.body.replace(['\r', '\n'], " "),
            }
        })
        .collect()
}

/// The title of the logs panel: `Logs from POST /charges`.
pub fn logs_title(span_name: &str) -> String {
    format!("Logs from {span_name}")
}

/// The count next to the title: `4 logs`, `1 log`, `0 logs`.
pub fn logs_count(n: usize) -> String {
    counted(n, "log")
}
