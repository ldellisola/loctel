//! The TraceQL syntax tree.
//!
//! The types are public so that later tooling (the evaluator, the rules
//! screen) can walk them, but they promise nothing beyond what the plan for
//! this milestone says; they may change shape between milestones.

use loctel_model::{SpanKind, StatusCode};

use crate::Pattern;

/// A parsed query: a spanset expression and the pipeline stages after it.
///
/// `{ status = error } >> { name = "x" } | count() > 3` is the spanset
/// expression `{ status = error } >> { name = "x" }` with one stage.
#[derive(Clone, Debug, PartialEq)]
pub struct TraceQuery {
    pub spanset: SpansetExpr,
    pub stages: Vec<PipelineStage>,
}

impl TraceQuery {
    /// True when the query cannot be judged one span at a time: it needs the
    /// other spans of the trace (or, for a rule, the trace's root).
    ///
    /// That is the case when it uses a structural operator, `&&` or `||`
    /// between two spanset filters, a pipeline stage, or `rootName`,
    /// `rootServiceName` or `traceDuration` (or their scoped forms).
    /// Otherwise it is a single filter, and each span stands on its own.
    pub fn needs_whole_trace(&self) -> bool {
        !self.stages.is_empty()
            || match &self.spanset {
                SpansetExpr::Binary { .. } => true,
                SpansetExpr::Filter(filter) => filter.uses_trace_intrinsics(),
            }
    }
}

/// An expression over sets of spans.
#[derive(Clone, Debug, PartialEq)]
pub enum SpansetExpr {
    /// `{ ... }`: the spans for which the field expression is true. `{ }` is
    /// the always-true filter.
    Filter(FieldExpr),
    /// `lhs OP rhs`. Parenthesised groups leave no node of their own.
    Binary {
        op: SpansetOp,
        lhs: Box<SpansetExpr>,
        rhs: Box<SpansetExpr>,
    },
}

/// An operator between two spanset expressions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpansetOp {
    /// `&&`
    And,
    /// `||`
    Or,
    /// `>`: the right span is a child of a left span.
    Child,
    /// `<`: the right span is the parent of a left span.
    Parent,
    /// `>>`: the right span is a descendant of a left span.
    Descendant,
    /// `<<`: the right span is an ancestor of a left span.
    Ancestor,
    /// `~`: the right span has a sibling among the left spans.
    Sibling,
    /// `!>`
    NotChild,
    /// `!<`
    NotParent,
    /// `!>>`
    NotDescendant,
    /// `!<<`
    NotAncestor,
    /// `!~`
    NotSibling,
}

/// A condition inside `{ ... }`.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldExpr {
    /// `a && b`. Same precedence as `||`, left-associative, as in Tempo.
    And(Box<FieldExpr>, Box<FieldExpr>),
    /// `a || b`.
    Or(Box<FieldExpr>, Box<FieldExpr>),
    /// `!a`.
    Not(Box<FieldExpr>),
    /// `lhs OP rhs` for `= != > < >= <=`.
    Compare {
        op: CompareOp,
        lhs: Operand,
        rhs: Operand,
    },
    /// `lhs =~ "pattern"` (or `!~` when `negated`). The pattern is compiled at
    /// parse time and anchored, as in Tempo: it must match the whole value.
    Regex {
        negated: bool,
        lhs: Operand,
        pattern: Pattern,
    },
    /// `x != nil`: the attribute is present.
    Exists(Operand),
    /// `x = nil`: the attribute is missing.
    NotExists(Operand),
    /// A bare boolean: an attribute (`{ span.cache.hit }`) or `true` or
    /// `false`. `{ }` parses to `true`.
    Operand(Operand),
}

impl FieldExpr {
    /// Whether the expression reads `rootName`, `rootServiceName` or
    /// `traceDuration` anywhere.
    pub fn uses_trace_intrinsics(&self) -> bool {
        let trace_level = |operand: &Operand| matches!(operand, Operand::Intrinsic(intrinsic) if intrinsic.is_trace_level());
        match self {
            FieldExpr::And(a, b) | FieldExpr::Or(a, b) => {
                a.uses_trace_intrinsics() || b.uses_trace_intrinsics()
            }
            FieldExpr::Not(inner) => inner.uses_trace_intrinsics(),
            FieldExpr::Compare { lhs, rhs, .. } => trace_level(lhs) || trace_level(rhs),
            FieldExpr::Regex { lhs, .. } => trace_level(lhs),
            FieldExpr::Exists(operand)
            | FieldExpr::NotExists(operand)
            | FieldExpr::Operand(operand) => trace_level(operand),
        }
    }
}

/// A comparison operator other than the regex ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompareOp {
    Eq,
    Neq,
    Gt,
    Gte,
    Lt,
    Lte,
}

/// One side of a comparison.
#[derive(Clone, Debug, PartialEq)]
pub enum Operand {
    Attribute(Attribute),
    Intrinsic(Intrinsic),
    Static(Static),
}

/// An attribute reference such as `span.http.method` or `.service.name`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attribute {
    pub scope: AttributeScope,
    /// The name without the scope, with quotes and escapes resolved.
    pub name: String,
}

/// Where an attribute is looked up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttributeScope {
    /// `span.name`
    Span,
    /// `resource.name`
    Resource,
    /// `.name`: the span attribute if present, else the resource attribute.
    Any,
}

/// A built-in field of a span or of its trace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intrinsic {
    /// `name`, `span:name`
    Name,
    /// `status`, `span:status`
    Status,
    /// `statusMessage`, `span:statusMessage`
    StatusMessage,
    /// `kind`, `span:kind`
    Kind,
    /// `duration`, `span:duration`
    Duration,
    /// `rootName`, `trace:rootName`
    RootName,
    /// `rootServiceName`, `trace:rootService`
    RootServiceName,
    /// `traceDuration`, `trace:duration`
    TraceDuration,
}

impl Intrinsic {
    /// True for the fields that describe the trace, not the span: they need
    /// the whole trace (or at least its root) to be known.
    pub fn is_trace_level(self) -> bool {
        matches!(
            self,
            Intrinsic::RootName | Intrinsic::RootServiceName | Intrinsic::TraceDuration
        )
    }
}

/// A literal value.
#[derive(Clone, Debug, PartialEq)]
pub enum Static {
    Str(String),
    Int(i64),
    Float(f64),
    /// Nanoseconds.
    Duration(u64),
    Bool(bool),
    Status(StatusCode),
    Kind(SpanKind),
}

/// A stage after `|`.
#[derive(Clone, Debug, PartialEq)]
pub enum PipelineStage {
    /// `count() OP n`: keeps the whole current result when the number of
    /// spans in it satisfies the comparison, and nothing otherwise.
    Count { op: CompareOp, value: f64 },
}
