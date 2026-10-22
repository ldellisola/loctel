//! What the query bar knows about the text in it: nothing typed, a TraceQL
//! query that parses, or one that does not and where.
//!
//! The bar parses as the user types and never throws the last good results
//! away on an error: the table keeps its rows and the bar shows the message
//! under the input (design 9.2, review focus 4 of the M3 plan).

use std::sync::Arc;

use loctel_query::traceql::{self, TraceQuery};

/// The query the bar's empty input suggests (its placeholder). It is a real
/// query: a test pins that it parses, so the hint never teaches a syntax the
/// bar rejects.
pub const EXAMPLE_QUERY: &str = "{ status = error && duration > 1s }";

/// The result of reading the bar's text.
#[derive(Clone, Debug, PartialEq)]
pub enum QueryState {
    /// Nothing but blanks: no filter.
    Empty,
    /// A query, parsed once and shared, ready for `TraceFilter::query`.
    Valid(Arc<TraceQuery>),
    /// The text is not a query `loctel` can run.
    Invalid {
        /// Where the problem is, counted from 1 in characters of the text as
        /// typed (`{ status = ` is wrong at column 12, the end).
        column: usize,
        /// What is wrong, without the column: `select is not supported yet`.
        message: String,
    },
}

/// Reads the bar's text. All-whitespace text is [`QueryState::Empty`].
/// Anything else goes to the TraceQL parser as it is, so the column of an
/// error counts the blanks the user typed.
pub fn validate(text: &str) -> QueryState {
    if text.trim().is_empty() {
        return QueryState::Empty;
    }
    match traceql::parse(text) {
        Ok(query) => QueryState::Valid(Arc::new(query)),
        Err(error) => QueryState::Invalid {
            column: error.position + 1,
            message: error.message,
        },
    }
}

/// The query a click on a service writes into the bar:
/// `{ resource.service.name = "payments-api" }`. A `"` or `\` in the name is
/// escaped with a backslash, the only escapes a name needs inside a TraceQL
/// string, so whatever the name holds the text is a valid query for exactly
/// that service.
pub fn service_filter(service: &str) -> String {
    format!("{{ resource.service.name = \"{}\" }}", escaped(service))
}

/// The query "traces through this span name" of the trace view writes into
/// the bar: `{ name = "POST /charges" }`, escaped like [`service_filter`]'s.
/// It matches the traces that have a span with exactly that name.
pub fn span_name_filter(name: &str) -> String {
    format!("{{ name = \"{}\" }}", escaped(name))
}

/// `text` as it goes between the quotes of a TraceQL string: a backslash
/// before every `"` and `\`. Nothing else needs one (a raw newline is fine in
/// the lexer's strings).
fn escaped(text: &str) -> String {
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
/// <message>` (the column counts from 1). `None` for an empty bar and for a
/// query that parses: there is nothing to say.
pub fn error_line(state: &QueryState) -> Option<String> {
    match state {
        QueryState::Invalid { column, message } => Some(format!("at column {column}: {message}")),
        QueryState::Empty | QueryState::Valid(_) => None,
    }
}
