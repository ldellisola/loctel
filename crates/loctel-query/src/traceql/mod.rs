//! TraceQL: the query language for traces.
//!
//! The syntax and semantics follow Grafana Tempo (design §6.1); the
//! authoritative grammar is Tempo's `pkg/traceql/expr.y`. [`parse`] turns text
//! into a [`TraceQuery`], or a [`QueryError`] with the
//! position of the problem. The subset that is supported, and what happens to
//! the rest, is in `docs/areas/query.md`.
//!
//! A parsed query is run against a trace with [`TraceQuery::evaluate`], which
//! takes the trace as a [`TraceView`] (the spans with their parent links
//! indexed), or against one span on its own with
//! [`TraceQuery::matches_span`].

mod ast;
mod eval;
mod lexer;
mod parser;
mod view;

pub use ast::{
    Attribute, AttributeScope, CompareOp, FieldExpr, Intrinsic, Operand, PipelineStage,
    SpansetExpr, SpansetOp, Static, TraceQuery,
};
pub use view::TraceView;

use crate::QueryError;

/// Parses a TraceQL query.
///
/// Anything outside the supported subset is a [`QueryError`]: a feature that
/// exists in Tempo but not here says `"<feature> is not supported yet"` at
/// the feature's position.
pub fn parse(text: &str) -> Result<TraceQuery, QueryError> {
    parser::parse(text)
}
