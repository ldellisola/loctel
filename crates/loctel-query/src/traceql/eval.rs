//! The TraceQL evaluator.
//!
//! A query is evaluated bottom-up over sets of span indices, each kept as a
//! `Vec<bool>` with one slot per span of the trace. A filter fills a set by
//! judging every span; the spanset operators combine sets; a pipeline stage
//! keeps or empties the set. The semantics follow Tempo's engine (checked
//! against `pkg/traceql/ast_execute.go` and the structural operators in
//! `tempodb/encoding/vparquet4/block_traceql.go`), as the plan fixes them;
//! `docs/areas/query.md` has the table and the places loctel is narrower.
//!
//! Everything here is linear in the number of spans (times the size of the
//! query). The structural operators do one pass over the spans in preorder
//! (see [`TraceView`]) instead of testing pairs, so a rule can run over a
//! trace of 100,000 spans on every arriving batch.
//!
//! Nothing here fails. A comparison that does not make sense (a missing
//! attribute, an int against a string) is false, as in Tempo, and a query is
//! never an error once it has parsed.

use std::cmp::Ordering;

use loctel_model::{AttrValue, Span, SpanKind, StatusCode, attr};

use super::ast::{
    AttributeScope, CompareOp, FieldExpr, Intrinsic, Operand, PipelineStage, SpansetExpr,
    SpansetOp, Static, TraceQuery,
};
use super::view::TraceView;

impl TraceQuery {
    /// The spans of `trace` that the query matches, as indices into
    /// [`TraceView::spans`], ascending and without duplicates. The trace
    /// matches the query when the result is not empty.
    ///
    /// This is the whole query: the spanset expression, then each pipeline
    /// stage over what is left.
    pub fn evaluate(&self, trace: &TraceView) -> Vec<usize> {
        let facts = TraceFacts::of(trace);
        let mut matched = eval_spanset(&self.spanset, trace, &facts);
        for stage in &self.stages {
            match stage {
                PipelineStage::Count { op, value } => {
                    let count = matched.iter().filter(|&&m| m).count();
                    if !holds(*op, (count as f64).partial_cmp(value)) {
                        matched.fill(false);
                    }
                }
            }
        }
        matched
            .iter()
            .enumerate()
            .filter_map(|(i, &m)| m.then_some(i))
            .collect()
    }

    /// Whether `span` matches, judged on its own. `Some` exactly when
    /// [`needs_whole_trace`](TraceQuery::needs_whole_trace) is false (one
    /// filter, no trace-level fields), and then it agrees with
    /// [`evaluate`](TraceQuery::evaluate) on a view of that one span. `None`
    /// when the query needs the rest of the trace.
    ///
    /// The rules engine calls this on every arriving span, so it builds no
    /// view and allocates nothing of its own.
    pub fn matches_span(&self, span: &Span) -> Option<bool> {
        if self.needs_whole_trace() {
            return None;
        }
        let SpansetExpr::Filter(filter) = &self.spanset else {
            return None;
        };
        Some(eval_field(filter, span, &TraceFacts::default()) == Tri::True)
    }
}

/// What is known about the trace as a whole, for `rootName`,
/// `rootServiceName` and `traceDuration`.
#[derive(Clone, Copy, Default)]
struct TraceFacts<'a> {
    /// The root span's name; `None` while there is no root.
    root_name: Option<&'a str>,
    /// The root span's service; `None` while there is no root.
    root_service: Option<&'a str>,
    /// Earliest start to latest end, in nanoseconds, clamped at 0; `None` for
    /// a trace with no spans.
    duration: Option<u64>,
}

impl<'a> TraceFacts<'a> {
    fn of(trace: &TraceView<'a>) -> TraceFacts<'a> {
        let spans = trace.spans();
        let root = trace.root().map(|i| &spans[i]);
        let earliest = spans.iter().map(|span| span.start_unix_nano).min();
        let latest = spans.iter().map(|span| span.end_unix_nano).max();
        TraceFacts {
            root_name: root.map(|span| span.name.as_str()),
            root_service: root.map(Span::service),
            duration: earliest
                .zip(latest)
                .map(|(start, end)| end.saturating_sub(start)),
        }
    }
}

// ---- spansets ---------------------------------------------------------------

fn eval_spanset(expr: &SpansetExpr, trace: &TraceView, facts: &TraceFacts) -> Vec<bool> {
    match expr {
        SpansetExpr::Filter(filter) => trace
            .spans()
            .iter()
            .map(|span| eval_field(filter, span, facts) == Tri::True)
            .collect(),
        SpansetExpr::Binary { op, lhs, rhs } => {
            // The right side is what a structural operator returns, so with
            // nothing there nothing needs computing, whichever relationship
            // is asked for.
            let structural = |relation: Relation, negated: bool| {
                let right = eval_spanset(rhs, trace, facts);
                if !any(&right) {
                    return right;
                }
                let left = eval_spanset(lhs, trace, facts);
                relate(relation, negated, &left, right, trace)
            };
            match op {
                // Both sides must find something; the answer is everything
                // they found.
                SpansetOp::And => {
                    let left = eval_spanset(lhs, trace, facts);
                    if !any(&left) {
                        return left;
                    }
                    let right = eval_spanset(rhs, trace, facts);
                    if !any(&right) {
                        return right;
                    }
                    union(left, &right)
                }
                SpansetOp::Or => {
                    let left = eval_spanset(lhs, trace, facts);
                    let right = eval_spanset(rhs, trace, facts);
                    union(left, &right)
                }
                SpansetOp::Child => structural(Relation::Child, false),
                SpansetOp::NotChild => structural(Relation::Child, true),
                SpansetOp::Parent => structural(Relation::Parent, false),
                SpansetOp::NotParent => structural(Relation::Parent, true),
                SpansetOp::Descendant => structural(Relation::Descendant, false),
                SpansetOp::NotDescendant => structural(Relation::Descendant, true),
                SpansetOp::Ancestor => structural(Relation::Ancestor, false),
                SpansetOp::NotAncestor => structural(Relation::Ancestor, true),
                SpansetOp::Sibling => structural(Relation::Sibling, false),
                SpansetOp::NotSibling => structural(Relation::Sibling, true),
            }
        }
    }
}

fn any(set: &[bool]) -> bool {
    set.iter().any(|&member| member)
}

fn union(mut set: Vec<bool>, other: &[bool]) -> Vec<bool> {
    for (member, &also) in set.iter_mut().zip(other) {
        *member |= also;
    }
    set
}

/// How a span can stand to a span of the left-hand set.
#[derive(Clone, Copy)]
enum Relation {
    /// The span's parent is in the set.
    Child,
    /// The span is the parent of one in the set.
    Parent,
    /// The span has an ancestor in the set.
    Descendant,
    /// The span is an ancestor of one in the set.
    Ancestor,
    /// The span shares its parent with one in the set, and is not that one.
    Sibling,
}

/// `left OP right` for a structural operator: the spans of `right` that stand
/// in `relation` to some span of `left`, or, for the `!` forms (`negated`),
/// that do not. With nothing in `left` no span has the relationship, so the
/// plain forms give nothing and the `!` forms give all of `right`.
fn relate(
    relation: Relation,
    negated: bool,
    left: &[bool],
    mut right: Vec<bool>,
    trace: &TraceView,
) -> Vec<bool> {
    let related = related_to(relation, left, trace);
    for (keep, related) in right.iter_mut().zip(related) {
        *keep = *keep && related != negated;
    }
    right
}

/// For every span, whether it stands in `relation` to some span of `set`.
/// Linear in the number of spans, using the preorder numbering of the view.
fn related_to(relation: Relation, set: &[bool], trace: &TraceView) -> Vec<bool> {
    let count = set.len();
    let mut related = vec![false; count];
    match relation {
        Relation::Child => {
            for (i, slot) in related.iter_mut().enumerate() {
                *slot = trace.parent(i).is_some_and(|p| set[p]);
            }
        }
        Relation::Parent => {
            for (i, _) in set.iter().enumerate().filter(|(_, in_set)| **in_set) {
                if let Some(p) = trace.parent(i) {
                    related[p] = true;
                }
            }
        }
        Relation::Descendant => {
            // Walking forward, `reach` is the furthest position that any set
            // span seen so far covers with its subtree. A span at a position
            // within it lies below one of them (subtrees nest or are apart).
            let mut reach: Option<usize> = None;
            for (position, &i) in trace.preorder().iter().enumerate() {
                related[i] = reach.is_some_and(|furthest| furthest >= position);
                if set[i] {
                    reach = Some(reach.map_or(trace.last(i), |r| r.max(trace.last(i))));
                }
            }
        }
        Relation::Ancestor => {
            // Walking backward, `next` is the nearest later position holding a
            // set span. A span has a set span below it when that position
            // falls inside its subtree.
            let mut next: Option<usize> = None;
            for (position, &i) in trace.preorder().iter().enumerate().rev() {
                related[i] = next.is_some_and(|p| p <= trace.last(i));
                if set[i] {
                    next = Some(position);
                }
            }
        }
        Relation::Sibling => {
            // How many set spans each span has as children; a span's siblings
            // in the set are its parent's, less itself. A span with no parent
            // has no siblings (Tempo: a root never matches `~`).
            let mut in_set_children = vec![0usize; count];
            for (i, _) in set.iter().enumerate().filter(|(_, in_set)| **in_set) {
                if let Some(p) = trace.parent(i) {
                    in_set_children[p] += 1;
                }
            }
            for (i, slot) in related.iter_mut().enumerate() {
                *slot = trace
                    .parent(i)
                    .is_some_and(|p| in_set_children[p] > usize::from(set[i]));
            }
        }
    }
    related
}

// ---- field expressions ------------------------------------------------------

/// A truth value that can also be "unknown": what a bare attribute is when it
/// is missing or not a boolean. Unknown is not true, so a span with an unknown
/// condition does not match, but `!` of it is still unknown, not true. That is
/// Tempo's reading of `{ !span.flag }` on a span without `flag`: no match.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tri {
    True,
    False,
    Unknown,
}

impl Tri {
    fn from_bool(value: bool) -> Tri {
        if value { Tri::True } else { Tri::False }
    }

    fn not(self) -> Tri {
        match self {
            Tri::True => Tri::False,
            Tri::False => Tri::True,
            Tri::Unknown => Tri::Unknown,
        }
    }
}

fn eval_field<'a>(expr: &'a FieldExpr, span: &'a Span, facts: &TraceFacts<'a>) -> Tri {
    match expr {
        FieldExpr::And(lhs, rhs) => match eval_field(lhs, span, facts) {
            Tri::False => Tri::False,
            left => match (left, eval_field(rhs, span, facts)) {
                (_, Tri::False) => Tri::False,
                (Tri::True, Tri::True) => Tri::True,
                _ => Tri::Unknown,
            },
        },
        FieldExpr::Or(lhs, rhs) => match eval_field(lhs, span, facts) {
            Tri::True => Tri::True,
            left => match (left, eval_field(rhs, span, facts)) {
                (_, Tri::True) => Tri::True,
                (Tri::False, Tri::False) => Tri::False,
                _ => Tri::Unknown,
            },
        },
        FieldExpr::Not(inner) => eval_field(inner, span, facts).not(),
        FieldExpr::Compare { op, lhs, rhs } => {
            let answer = match (
                operand_value(lhs, span, facts),
                operand_value(rhs, span, facts),
            ) {
                (Some(left), Some(right)) => compare(*op, left, right),
                _ => false,
            };
            Tri::from_bool(answer)
        }
        FieldExpr::Regex {
            negated,
            lhs,
            pattern,
        } => {
            let answer = match operand_value(lhs, span, facts) {
                Some(Value::Str(text)) => pattern.is_match(text) != *negated,
                _ => false,
            };
            Tri::from_bool(answer)
        }
        FieldExpr::Exists(operand) => Tri::from_bool(operand_value(operand, span, facts).is_some()),
        FieldExpr::NotExists(operand) => {
            Tri::from_bool(operand_value(operand, span, facts).is_none())
        }
        FieldExpr::Operand(operand) => match operand_value(operand, span, facts) {
            Some(Value::Bool(value)) => Tri::from_bool(value),
            _ => Tri::Unknown,
        },
    }
}

// ---- values -----------------------------------------------------------------

/// A value an operand can have for one span.
#[derive(Clone, Copy, Debug)]
enum Value<'a> {
    Str(&'a str),
    Int(i64),
    Float(f64),
    Bool(bool),
    /// Nanoseconds.
    Duration(u64),
    Status(StatusCode),
    Kind(SpanKind),
    /// An attribute that is there but is bytes, an array or a map: present
    /// for `!= nil`, and equal and ordered against nothing.
    Opaque,
}

/// The value of `operand` for `span`, or `None` when it has none: an
/// attribute the span lacks, or a trace-level field of a trace whose root has
/// not arrived.
fn operand_value<'a>(
    operand: &'a Operand,
    span: &'a Span,
    facts: &TraceFacts<'a>,
) -> Option<Value<'a>> {
    match operand {
        Operand::Static(value) => Some(match value {
            Static::Str(text) => Value::Str(text),
            Static::Int(n) => Value::Int(*n),
            Static::Float(f) => Value::Float(*f),
            Static::Duration(nanos) => Value::Duration(*nanos),
            Static::Bool(b) => Value::Bool(*b),
            Static::Status(status) => Value::Status(*status),
            Static::Kind(kind) => Value::Kind(*kind),
        }),
        Operand::Intrinsic(intrinsic) => match intrinsic {
            Intrinsic::Name => Some(Value::Str(&span.name)),
            Intrinsic::Status => Some(Value::Status(span.status)),
            Intrinsic::StatusMessage => Some(Value::Str(&span.status_message)),
            Intrinsic::Kind => Some(Value::Kind(span.kind)),
            Intrinsic::Duration => Some(Value::Duration(span.duration_nanos())),
            Intrinsic::RootName => facts.root_name.map(Value::Str),
            Intrinsic::RootServiceName => facts.root_service.map(Value::Str),
            Intrinsic::TraceDuration => facts.duration.map(Value::Duration),
        },
        Operand::Attribute(attribute) => {
            let name = attribute.name.as_str();
            let found = match attribute.scope {
                AttributeScope::Span => attr(&span.attributes, name),
                AttributeScope::Resource => attr(&span.resource.attributes, name),
                // The span's attribute if it has one, else the resource's.
                AttributeScope::Any => {
                    attr(&span.attributes, name).or_else(|| attr(&span.resource.attributes, name))
                }
            };
            found.map(attribute_value)
        }
    }
}

fn attribute_value(value: &AttrValue) -> Value<'_> {
    match value {
        AttrValue::Str(text) => Value::Str(text),
        AttrValue::Int(n) => Value::Int(*n),
        AttrValue::Double(f) => Value::Float(*f),
        AttrValue::Bool(b) => Value::Bool(*b),
        AttrValue::Bytes(_) | AttrValue::Array(_) | AttrValue::Map(_) => Value::Opaque,
    }
}

/// `left OP right` for two values that both exist. Only values of matching
/// types compare, as in Tempo; every other pair is false, whatever the
/// operator (`!=` too).
fn compare(op: CompareOp, left: Value, right: Value) -> bool {
    match (left, right) {
        (Value::Int(a), Value::Int(b)) => holds(op, Some(a.cmp(&b))),
        // An int and a float compare as numbers.
        (Value::Int(a), Value::Float(b)) => holds(op, (a as f64).partial_cmp(&b)),
        (Value::Float(a), Value::Int(b)) => holds(op, a.partial_cmp(&(b as f64))),
        (Value::Float(a), Value::Float(b)) => holds(op, a.partial_cmp(&b)),
        (Value::Duration(a), Value::Duration(b)) => holds(op, Some(a.cmp(&b))),
        // Strings compare byte by byte.
        (Value::Str(a), Value::Str(b)) => holds(op, Some(a.cmp(b))),
        (Value::Bool(a), Value::Bool(b)) => equality_only(op, a == b),
        (Value::Status(a), Value::Status(b)) => equality_only(op, a == b),
        (Value::Kind(a), Value::Kind(b)) => equality_only(op, a == b),
        _ => false,
    }
}

/// Booleans, statuses and kinds have no order: only `=` and `!=` mean
/// something.
fn equality_only(op: CompareOp, equal: bool) -> bool {
    match op {
        CompareOp::Eq => equal,
        CompareOp::Neq => !equal,
        _ => false,
    }
}

/// Whether `op` holds for values ordered as `ordering`. `None` (a NaN) is
/// unordered: it is equal to nothing, so `!=` holds and the rest do not, as
/// for floats in any language.
fn holds(op: CompareOp, ordering: Option<Ordering>) -> bool {
    match op {
        CompareOp::Eq => ordering == Some(Ordering::Equal),
        CompareOp::Neq => ordering != Some(Ordering::Equal),
        CompareOp::Gt => ordering == Some(Ordering::Greater),
        CompareOp::Gte => matches!(ordering, Some(Ordering::Greater | Ordering::Equal)),
        CompareOp::Lt => ordering == Some(Ordering::Less),
        CompareOp::Lte => matches!(ordering, Some(Ordering::Less | Ordering::Equal)),
    }
}
