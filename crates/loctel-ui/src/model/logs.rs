//! What the Logs screen shows: the rows of the list, the details of an
//! expanded line, the state of the LogQL bar and the query `Open in Logs`
//! writes into it. Plain Rust over the store's types, with no GPUI in it: the
//! view lays out the strings it is given, and every rule here is pinned by plain
//! assertions in `tests/model_logs.rs`.
//!
//! The columns are time, level (a badge), service (a colour dot and the name),
//! message and trace (design 9.2). A [`LogRow`] holds each as the exact string
//! the view draws, with the IDs a click needs.

use std::sync::Arc;

use loctel_model::{LogRecord, Severity, SpanId, TraceId};
use loctel_query::logql::{self, LogQuery};
use loctel_store::LogId;

use super::colors::ServiceColors;
use super::format;

/// How many leading hex digits of a trace ID the trace column shows.
pub const TRACE_SHORT_CHARS: usize = 8;

/// What stands in the message where the body had a line break.
pub const LINE_BREAK: &str = " \u{23ce} ";

/// The four level badges and toggles of the Logs screen. Each is a group of
/// severities, the same four the level toggles have (and the same groups as
/// `loctel_store::LevelSet::from_groups`):
///
/// | badge | severities |
/// |---|---|
/// | `ERR` | error, fatal |
/// | `WRN` | warn |
/// | `INF` | info, unspecified |
/// | `DBG` | debug, trace |
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LevelBadge {
    Err,
    Wrn,
    Inf,
    Dbg,
}

impl LevelBadge {
    /// The badge of a severity.
    pub fn from_severity(severity: Severity) -> LevelBadge {
        match severity {
            Severity::Error | Severity::Fatal => LevelBadge::Err,
            Severity::Warn => LevelBadge::Wrn,
            Severity::Info | Severity::Unspecified => LevelBadge::Inf,
            Severity::Debug | Severity::Trace => LevelBadge::Dbg,
        }
    }

    /// The three letters of the badge: `ERR`, `WRN`, `INF` or `DBG`.
    pub fn label(self) -> &'static str {
        match self {
            LevelBadge::Err => "ERR",
            LevelBadge::Wrn => "WRN",
            LevelBadge::Inf => "INF",
            LevelBadge::Dbg => "DBG",
        }
    }
}

/// One line of the logs list.
#[derive(Clone, Debug, PartialEq)]
pub struct LogRow {
    /// The log's ID in the store: what `Store::log` and `Store::logs_around`
    /// take, and what the list keys its expanded line by.
    pub id: LogId,
    /// When the log happened as a clock time, `14:32:09.194` (its own time, or
    /// its observed time when the exporter left it unset).
    pub time: String,
    /// The level's badge; [`LevelBadge::label`] has its letters.
    pub level: LevelBadge,
    /// The emitting service.
    pub service: String,
    /// The service's colour index, from [`ServiceColors`].
    pub color: usize,
    /// The body on one line: every line break is [`LINE_BREAK`], a trailing one
    /// is dropped (nothing follows it), and the view cuts the rest to the width
    /// it has. The whole body is in [`details`].
    pub message: String,
    /// The first [`TRACE_SHORT_CHARS`] hex digits of the trace ID, the text of
    /// the trace column's link. `None` for a log outside any trace.
    pub trace_short: Option<String>,
    /// The trace the log was emitted in.
    pub trace_id: Option<TraceId>,
    /// The span the log was emitted in.
    pub span_id: Option<SpanId>,
}

/// The rows for `records`, in the order given (the store gives newest first).
/// `utc_offset` is the zone of the times ([`format::local_utc_offset`] in the
/// app, 0 in tests).
///
/// `colors` is asked for the service of every record, in order, so a service
/// first seen here gets its colour here and keeps it everywhere else.
pub fn rows(
    records: &[(LogId, LogRecord)],
    colors: &mut ServiceColors,
    utc_offset: i32,
) -> Vec<LogRow> {
    records
        .iter()
        .map(|(id, record)| {
            let time = if record.time_unix_nano != 0 {
                record.time_unix_nano
            } else {
                record.observed_unix_nano
            };
            let service = record.service();
            LogRow {
                id: *id,
                time: format::clock(time, utc_offset),
                level: LevelBadge::from_severity(record.severity),
                color: colors.index(service),
                service: service.to_owned(),
                message: one_line(&record.body),
                trace_short: record
                    .trace_id
                    .map(|trace| trace.to_string()[..TRACE_SHORT_CHARS].to_owned()),
                trace_id: record.trace_id,
                span_id: record.span_id,
            }
        })
        .collect()
}

/// `body` as one line: each line break (`\n`, `\r\n` or a lone `\r`) becomes
/// [`LINE_BREAK`], and breaks at the very end are dropped.
fn one_line(body: &str) -> String {
    let body = body.trim_end_matches(['\r', '\n']);
    let mut line = String::with_capacity(body.len());
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                // `\r\n` is one break.
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                line.push_str(LINE_BREAK);
            }
            '\n' => line.push_str(LINE_BREAK),
            c => line.push(c),
        }
    }
    line
}

/// The details grid of an expanded line: `body` first, with the whole body
/// (the view wraps it, line breaks and all), then the label set LogQL sees
/// ([`loctel_query::logql::labels`]: the resource and log attributes with
/// Loki's label names, `service_name`, `level`, `scope_name`, `trace_id` and
/// `span_id`), sorted by name.
pub fn details(record: &LogRecord) -> Vec<(String, String)> {
    let mut grid = vec![("body".to_owned(), record.body.clone())];
    grid.extend(logql::labels(record));
    grid
}

/// What the LogQL bar knows about the text in it, like the TraceQL bar's
/// [`QueryState`](super::query_bar::QueryState): nothing typed, a query that
/// parses, or one that does not and where. The list keeps its rows on an error.
#[derive(Clone, Debug, PartialEq)]
pub enum LogQueryState {
    /// Nothing but blanks: no filter.
    Empty,
    /// A query, parsed once and shared, ready for `LogFilter::query`.
    Valid(Arc<LogQuery>),
    /// The text is not a query loctel can run.
    Invalid {
        /// Where the problem is, counted from 1 in characters of the text as
        /// typed.
        column: usize,
        /// What is wrong, without the column.
        message: String,
    },
}

/// Reads the bar's text. All-whitespace text is [`LogQueryState::Empty`];
/// anything else goes to the LogQL parser as it is, so the column of an error
/// counts the blanks the user typed.
pub fn validate(text: &str) -> LogQueryState {
    if text.trim().is_empty() {
        return LogQueryState::Empty;
    }
    match logql::parse(text) {
        Ok(query) => LogQueryState::Valid(Arc::new(query)),
        Err(error) => LogQueryState::Invalid {
            column: error.position + 1,
            message: error.message,
        },
    }
}

/// The query the trace view's `Open in Logs` writes into the bar: the logs the
/// span emitted, `{service_name="payments-api"} | span_id = "00f067aa0ba902b7"`.
/// A `"` or `\` in the service name is escaped with a backslash (the escapes of
/// a LogQL string), so whatever the name holds the text is a valid query.
pub fn span_logs_query(service: &str, span: SpanId) -> String {
    format!(
        "{{service_name=\"{}\"}} | span_id = \"{span}\"",
        escaped(service)
    )
}

/// `text` as it goes between the quotes of a LogQL string: a backslash before
/// every `"` and `\`.
pub(crate) fn escaped(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '"' | '\\') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}
