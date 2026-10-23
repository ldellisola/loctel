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
use loctel_query::logql::{self, LogQuery, StringMatcher};
use loctel_store::{LevelSet, LogFilter, LogId};

use super::colors::ServiceColors;
use super::format;
use super::trace_table::thousands;

/// How many leading hex digits of a trace ID the trace column shows.
pub const TRACE_SHORT_CHARS: usize = 8;

/// What stands in the message where the body had a line break.
pub const LINE_BREAK: &str = " \u{23ce} ";

/// How many lines one page of the list holds (M4 plan, Global Constraints:
/// pages of 500, newest first).
pub const PAGE_SIZE: usize = 500;

/// The most rows the list holds in memory. Past it the list stops loading and
/// [`CAPPED_FOOTER`] says so (the 10M-line scroll is M5).
pub const MAX_ROWS: usize = 20_000;

/// How close to the end of the loaded rows a row must be drawn for the next
/// page to be read: within this many rows.
pub const PREFETCH_ROWS: usize = 50;

/// The height of a row of the list, in pixels (design 9.2).
pub const ROW_PX: f32 = 30.0;

/// A list that matches more lines than this, with a search the store keeps counts
/// for ([`indexed_filter`]), is not loaded in pages (which stop at [`MAX_ROWS`])
/// but read by position through a [`RowWindow`](super::window::RowWindow): every
/// line can be reached, 10,000,000 of them if the store holds that many. A list
/// of this many lines or fewer, and any list of a search the store cannot count,
/// is the paged list of Milestone 4.
pub const WINDOWED_ABOVE: u64 = MAX_ROWS as u64;

/// The footer of a list that stopped at [`MAX_ROWS`].
pub const CAPPED_FOOTER: &str = "Showing the newest 20,000 matching lines";

/// The words of the link in the context banner that goes back to the search.
pub const BACK_TO_SEARCH: &str = "Back to search";

/// The pill that shows while the live tail is paused by scrolling away from
/// the top: a click on it goes back to the newest lines.
pub const NEW_LINES_PILL: &str = "New lines \u{b7} Jump to newest";

/// What the LogQL bar's empty input suggests (its placeholder). It is a real
/// query: a test pins that it parses, so the hint never teaches a syntax the
/// bar rejects.
pub const EXAMPLE_QUERY: &str = "{service_name=\"payments-api\"} |= \"error\"";

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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LevelBadge {
    Err,
    Wrn,
    Inf,
    Dbg,
}

impl LevelBadge {
    /// The four badges, in the order the toggles list them: `ERR`, `WRN`, `INF`,
    /// `DBG`.
    pub const ALL: [LevelBadge; 4] = [
        LevelBadge::Err,
        LevelBadge::Wrn,
        LevelBadge::Inf,
        LevelBadge::Dbg,
    ];

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

/// Whether the toggle of `badge` is on in `levels`: the set holds the severity
/// that names the group (error, warn, info or debug; the other severity of the
/// group, fatal, unspecified or trace, goes with it). A set the toggles built
/// always holds a group whole or not at all.
pub fn level_is_on(levels: LevelSet, badge: LevelBadge) -> bool {
    levels.contains(match badge {
        LevelBadge::Err => Severity::Error,
        LevelBadge::Wrn => Severity::Warn,
        LevelBadge::Inf => Severity::Info,
        LevelBadge::Dbg => Severity::Debug,
    })
}

/// `levels` with the toggle of `badge` set to `on`, the other three as they
/// were.
pub fn levels_with(levels: LevelSet, badge: LevelBadge, on: bool) -> LevelSet {
    let state = |b: LevelBadge| {
        if b == badge {
            on
        } else {
            level_is_on(levels, b)
        }
    };
    LevelSet::from_groups(
        state(LevelBadge::Err),
        state(LevelBadge::Wrn),
        state(LevelBadge::Inf),
        state(LevelBadge::Dbg),
    )
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

/// The line under the input while the text is not a query: `at column 12:
/// <message>` (the column counts from 1), like the TraceQL bar's. `None` for an
/// empty bar and for a query that parses.
pub fn error_line(state: &LogQueryState) -> Option<String> {
    match state {
        LogQueryState::Invalid { column, message } => {
            Some(format!("at column {column}: {message}"))
        }
        LogQueryState::Empty | LogQueryState::Valid(_) => None,
    }
}

/// The count on the right of the level toggles. `loaded` is the number of rows
/// the list holds, `more` whether another page is waiting, `capped` whether the
/// list stopped at [`MAX_ROWS`].
///
/// - everything is loaded: `1,204 lines · newest first` (`1 line` for one)
/// - more is waiting: `500+ lines · newest first`; the count is what is loaded
///   (the store has no cheap count of what matches), and the plus says there is
///   more
/// - capped: `20,000+ lines`
pub fn count_line(loaded: usize, more: bool, capped: bool) -> String {
    if capped {
        return format!("{}+ lines", thousands(loaded));
    }
    if more {
        return format!("{}+ lines \u{b7} newest first", thousands(loaded));
    }
    let noun = if loaded == 1 { "line" } else { "lines" };
    format!("{} {noun} \u{b7} newest first", thousands(loaded))
}

/// The count over the lines around a log: `51 lines · oldest first` (they are
/// read oldest first, like a file, and all of them are shown).
pub fn context_count_line(shown: usize) -> String {
    let noun = if shown == 1 { "line" } else { "lines" };
    format!("{} {noun} \u{b7} oldest first", thousands(shown))
}

/// The footer under a list that stopped at [`MAX_ROWS`] ([`CAPPED_FOOTER`]),
/// `None` for any other list.
pub fn footer(capped: bool) -> Option<&'static str> {
    capped.then_some(CAPPED_FOOTER)
}

/// The words of the banner over the lines around a log: `Lines around
/// 14:32:09.194` (the clock time of the log, which its ID carries, in the zone
/// `utc_offset` gives), or `This line is no longer held` when the log has been
/// evicted, removed or cleared (the lines shown are the ones that were nearest
/// to where it was). The view adds [`BACK_TO_SEARCH`] after it.
pub fn context_banner(target: LogId, held: bool, utc_offset: i32) -> String {
    if held {
        format!(
            "Lines around {}",
            format::clock(target.time_unix_nano, utc_offset)
        )
    } else {
        "This line is no longer held".to_owned()
    }
}

/// How the newest page, read again by the live tail, joins the rows the list
/// holds. `held` are the rows (`id` says which log each is), newest first, and
/// `fresh` the IDs of the page, newest first. The list after the join is the
/// fresh page followed by the held rows older than it: `fresh ++
/// held[keep_from..]`, and `Some(keep_from)` is that number of held rows, which
/// the page supersedes (it has them, and any line that arrived late among them).
///
/// Every row of the join is strictly older than the one before it, so no line
/// is in the list twice, whatever arrived since the last read.
///
/// `None` means the page cannot be joined and the list starts over from it:
///
/// - it is empty: nothing matches any more (the store was cleared), so nothing
///   held may stay
/// - it ends newer than the newest row held: more lines arrived than a page
///   holds (a log flood), so lines between the two were never read, and a join
///   would hide the gap
///
/// An empty list takes any non-empty page whole (`Some(0)`).
pub fn merge_head<T>(held: &[T], id: impl Fn(&T) -> LogId, fresh: &[LogId]) -> Option<usize> {
    let oldest = *fresh.last()?;
    let Some(top) = held.first().map(&id) else {
        return Some(0);
    };
    if oldest > top {
        return None;
    }
    Some(held.partition_point(|row| id(row) >= oldest))
}

// --- the part of a search the store keeps counts for -----------------------------------------------

/// The seven severities, lowest first.
const SEVERITIES: [Severity; 7] = [
    Severity::Unspecified,
    Severity::Trace,
    Severity::Debug,
    Severity::Info,
    Severity::Warn,
    Severity::Error,
    Severity::Fatal,
];

/// What the store can count and find by position without reading the lines
/// (`LogFilter::is_indexed`): one service and a set of levels. A search is
/// *indexed* when it comes to no more than that.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexedFilter {
    /// Only this service (exact name), or every service.
    pub service: Option<String>,
    /// The levels kept: the toggles, narrowed by the selector's `level`
    /// matchers. The empty set keeps nothing.
    pub levels: LevelSet,
}

impl IndexedFilter {
    /// The store's filter for lines in `[since, until)`: only the window, the
    /// service and the levels, so that `Store::log_count`, `Store::logs_at` and
    /// `Store::log_histogram` answer from what the store keeps about its chunks.
    pub fn filter(&self, since_unix_nano: Option<u64>, until_unix_nano: Option<u64>) -> LogFilter {
        LogFilter {
            since_unix_nano,
            until_unix_nano,
            service: self.service.clone(),
            levels: (self.levels != LevelSet::ALL).then_some(self.levels),
            ..LogFilter::default()
        }
    }
}

/// Whether a search is indexed ([`IndexedFilter`]), and as what: `query` is the
/// LogQL bar's query (`None` for an empty bar) and `toggles` the level toggles.
///
/// A search is indexed when it has no query, or only a stream selector whose
/// matchers are `service_name="..."` (one service) and `level` matchers of any
/// kind (`=`, `!=`, `=~`, `!~`, each judged against the seven level names, so
/// `level=~"warn|error"` is the set of warn and error). Any other label, a
/// regex or a negation on the service, a second service, or any stage after the
/// selector (a line filter, `| json`, a label filter) is not: `None`, and the
/// list is the paged one, which asks the query of every line.
pub fn indexed_filter(query: Option<&LogQuery>, toggles: LevelSet) -> Option<IndexedFilter> {
    let mut service: Option<String> = None;
    let mut levels = toggles;
    if let Some(query) = query {
        if !query.stages.is_empty() {
            return None;
        }
        for matcher in &query.matchers {
            match (matcher.label.as_str(), &matcher.matcher) {
                ("service_name", StringMatcher::Equal(name)) => {
                    if service.as_ref().is_some_and(|held| held != name) {
                        // Two services at once match nothing; the query path says so.
                        return None;
                    }
                    service = Some(name.clone());
                }
                ("level", level) => {
                    levels = SEVERITIES
                        .into_iter()
                        .filter(|severity| {
                            levels.contains(*severity) && level.matches(severity.level_label())
                        })
                        .fold(LevelSet::default(), LevelSet::with);
                }
                _ => return None,
            }
        }
    }
    Some(IndexedFilter { service, levels })
}

/// The count over a windowed list, which knows how many lines match:
/// `10,000,000 lines · newest first`. It is the right of the level toggles and
/// the footer of the list.
pub fn total_line(total: u64) -> String {
    let shown = usize::try_from(total).unwrap_or(usize::MAX);
    count_line(shown, false, false)
}
