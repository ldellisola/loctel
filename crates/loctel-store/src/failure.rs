//! Failure semantics (design 9.4): did the request work, and if not, which
//! span explains it?
//!
//! [`classify`] is the one place that decides. It is pure, so everything that
//! shows or filters by failure (the trace snapshot and summary here, and
//! through them the MCP tools and the UI) agrees by construction.

use std::collections::HashMap;

use loctel_model::{ExceptionInfo, Span, SpanId, SpanKind};
use serde::Serialize;

use crate::trace::TraceSnapshot;

/// Did the request work? Used everywhere a trace is shown or filtered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceStatus {
    /// No span is erroring.
    Ok,
    /// The root span is fine (or has not arrived) but another span is
    /// erroring.
    ErrorsInside,
    /// The root span is erroring.
    Failed,
}

/// The span that explains a failure: the earliest-starting span among the
/// erroring spans that have no erroring descendant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FailedIn {
    pub span_id: SpanId,
    pub service: String,
    pub span_name: String,
    /// The remote party, for client and producer spans.
    pub peer: Option<String>,
    /// How many other such spans there are ("+N more").
    pub more: usize,
}

/// Classifies a trace by its spans (in any order) and its root, if it has one.
///
/// 1. A span is *erroring* when its status is `Error`. An exception event
///    alone does not count.
/// 2. The trace is `Failed` when the root is erroring, else `ErrorsInside`
///    when any span is erroring, else `Ok`. A trace with errors and no root
///    (yet) is therefore `ErrorsInside`.
/// 3. The candidates are the erroring spans with no erroring descendant: the
///    lowest error on each branch.
/// 4. `failed_in` is the candidate that started first (a tie goes to the
///    lowest span ID) and `more` counts the other candidates.
///
/// Descendants follow parent IDs, never timestamps, so a child whose clock
/// runs ahead of its parent's is still a descendant. The input may be
/// malformed (a span that is its own parent, a parent cycle) and this still
/// terminates. It is iterative, so a very deep trace cannot overflow the
/// stack.
pub fn classify(spans: &[Span], root: Option<SpanId>) -> (TraceStatus, Option<FailedIn>) {
    if !spans.iter().any(Span::is_error) {
        return (TraceStatus::Ok, None);
    }
    let root_erroring = root.is_some_and(|id| {
        spans
            .iter()
            .any(|span| span.span_id == id && span.is_error())
    });
    let status = if root_erroring {
        TraceStatus::Failed
    } else {
        TraceStatus::ErrorsInside
    };

    // Position of each span ID (the first, if a caller passes duplicates) and
    // of each span's parent. A span is never its own parent here: a
    // self-parenting span is malformed, and must not hide itself.
    let mut position: HashMap<SpanId, usize> = HashMap::with_capacity(spans.len());
    for (i, span) in spans.iter().enumerate() {
        position.entry(span.span_id).or_insert(i);
    }
    let parent: Vec<Option<usize>> = spans
        .iter()
        .enumerate()
        .map(|(i, span)| {
            span.parent_span_id
                .and_then(|id| position.get(&id).copied())
                .filter(|&p| p != i)
        })
        .collect();

    // Mark every ancestor of an erroring span. A walk stops at the first
    // ancestor already marked, because everything above it is marked too, so
    // the whole pass is linear, and a cycle ends when it comes round.
    let mut has_erroring_descendant = vec![false; spans.len()];
    for (i, span) in spans.iter().enumerate() {
        if !span.is_error() {
            continue;
        }
        let mut up = parent[i];
        while let Some(p) = up {
            if has_erroring_descendant[p] {
                break;
            }
            has_erroring_descendant[p] = true;
            up = parent[p];
        }
    }

    let erroring = || (0..spans.len()).filter(|&i| spans[i].is_error());
    let mut candidates: Vec<usize> = erroring()
        .filter(|&i| !has_erroring_descendant[i])
        .collect();
    if candidates.is_empty() {
        // Only a parent cycle (malformed input) leaves no lowest error. Name
        // something rather than nothing: every erroring span is a candidate.
        candidates = erroring().collect();
    }
    let best = candidates
        .iter()
        .copied()
        .min_by_key(|&i| (spans[i].start_unix_nano, spans[i].span_id))
        .expect("an erroring span exists, so there is a candidate");
    let span = &spans[best];
    let peer = if matches!(span.kind, SpanKind::Client | SpanKind::Producer) {
        span.peer().map(str::to_owned)
    } else {
        None
    };
    let failed_in = FailedIn {
        span_id: span.span_id,
        service: span.service().to_owned(),
        span_name: span.name.clone(),
        peer,
        more: candidates.len() - 1,
    };
    (status, Some(failed_in))
}

/// The exception that best explains the trace: the failed-in span's own, else
/// the nearest ancestor's on the path from it to the root, else the first
/// exception in start order (so a handled exception in a trace without errors
/// is still found).
pub fn trace_exception(snapshot: &TraceSnapshot) -> Option<ExceptionInfo> {
    pick_exception(
        &snapshot.spans,
        snapshot.failed_in.as_ref().map(|failed| failed.span_id),
    )
}

/// [`trace_exception`] on a bare span slice (any order) and the failed-in
/// span's ID.
pub(crate) fn pick_exception(spans: &[Span], failed_in: Option<SpanId>) -> Option<ExceptionInfo> {
    if let Some(start) = failed_in {
        // Reversed, so that the first of any duplicate IDs wins.
        let by_id: HashMap<SpanId, &Span> = spans
            .iter()
            .rev()
            .map(|span| (span.span_id, span))
            .collect();
        let mut current = by_id.get(&start).copied();
        // At most one step per span: a parent cycle cannot loop forever.
        for _ in 0..spans.len() {
            let Some(span) = current else { break };
            if let Some(exception) = span.exception() {
                return Some(exception);
            }
            current = span
                .parent_span_id
                .and_then(|parent| by_id.get(&parent).copied());
        }
    }
    spans
        .iter()
        .filter_map(|span| {
            span.exception()
                .map(|exception| ((span.start_unix_nano, span.span_id), exception))
        })
        .min_by_key(|(key, _)| *key)
        .map(|(_, exception)| exception)
}
