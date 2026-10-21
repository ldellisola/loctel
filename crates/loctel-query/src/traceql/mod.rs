//! TraceQL: the query language for traces.
//!
//! The syntax and semantics follow Grafana Tempo (design §6.1); the
//! authoritative grammar is Tempo's `pkg/traceql/expr.y`. [`parse`] turns text
//! into a [`TraceQuery`], or a [`QueryError`](crate::QueryError) with the
//! position of the problem. The subset that is supported, and what happens to
//! the rest, is in `docs/areas/query.md`.

mod ast;
mod lexer;
mod parser;

pub use ast::{
    Attribute, AttributeScope, CompareOp, FieldExpr, Intrinsic, Operand, PipelineStage,
    SpansetExpr, SpansetOp, Static, TraceQuery,
};

use crate::QueryError;

/// Parses a TraceQL query.
///
/// Anything outside the supported subset is a [`QueryError`]: a feature that
/// exists in Tempo but not here says `"<feature> is not supported yet"` at
/// the feature's position.
pub fn parse(text: &str) -> Result<TraceQuery, QueryError> {
    parser::parse(text)
}
