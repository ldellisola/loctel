//! LogQL: the query language for logs.
//!
//! The syntax and semantics follow Grafana Loki (design §6.2); the
//! authoritative grammar is Loki's `pkg/logql/syntax/syntax.y`. [`parse`] turns
//! text into a [`LogQuery`], or a [`QueryError`] with the
//! position of the problem. The subset that is supported, and what happens to
//! the rest, is in `docs/areas/query.md`.
//!
//! A query is a stream selector followed by a pipeline:
//!
//! ```text
//! {service_name="payments-api", level=~"warn|error"} |= "card" | json | status >= 500
//! ```
//!
//! [`LogQuery::matches`] judges one [`LogRecord`](loctel_model::LogRecord).
//! The labels it reads are [`labels()`]: the resource and log attributes (dots
//! become `_`), `service_name`, `level`, `scope_name`, `trace_id` and
//! `span_id`.

mod ast;
mod eval;
mod labels;
mod lexer;
mod parser;

pub use ast::{LabelFilter, LabelMatcher, LineFilter, LogQuery, NumberOp, Stage, StringMatcher};
pub use labels::labels;

use crate::QueryError;

/// Parses a LogQL query.
///
/// Anything outside the supported subset is a [`QueryError`]: a feature that
/// exists in Loki but not here says `"<feature> is not supported yet"` at the
/// feature's position, and an empty selector `{}` is an error.
pub fn parse(text: &str) -> Result<LogQuery, QueryError> {
    parser::parse(text)
}
