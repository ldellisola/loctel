//! One trace: the stored entry and the snapshot handed to readers.

use std::collections::HashMap;

use loctel_model::{Span, SpanId, TraceId};

use crate::failure::{FailedIn, TraceStatus};

/// A trace as of the moment it was read: a copy, so later ingest never
/// changes it.
#[derive(Clone, Debug)]
pub struct TraceSnapshot {
    pub trace_id: TraceId,
    /// Every span, sorted by `start_unix_nano`, then `span_id`. A child whose
    /// clock runs ahead of its parent's can sort before it.
    pub spans: Vec<Span>,
    /// The span with no parent. With several, the earliest start wins (then
    /// the lowest span ID). `None` until one arrives.
    pub root: Option<SpanId>,
    /// Spans whose parent ID is not in the trace (yet), in `spans` order.
    pub orphans: Vec<SpanId>,
    /// No new span has arrived for `completeness_quiet`.
    pub complete: bool,
    pub status: TraceStatus,
    pub failed_in: Option<FailedIn>,
}

impl TraceSnapshot {
    /// The direct children of span `id`, in `spans` order.
    ///
    /// This scans the spans, so walking a whole large trace through it is
    /// quadratic. Callers that do that should build their own
    /// parent-to-children map in one pass.
    pub fn children(&self, id: SpanId) -> impl Iterator<Item = &Span> {
        self.spans
            .iter()
            .filter(move |span| span.parent_span_id == Some(id))
    }
}

/// What the store keeps for one trace.
pub(crate) struct TraceEntry {
    /// In arrival order.
    spans: Vec<Span>,
    /// Span ID to position in `spans`: the dedup index.
    index: HashMap<SpanId, usize>,
    /// Clock time at which the last new span arrived.
    last_arrival: u64,
}

impl TraceEntry {
    pub(crate) fn new(now: u64) -> Self {
        Self {
            spans: Vec::new(),
            index: HashMap::new(),
            last_arrival: now,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.spans.len()
    }

    /// Adds `span` unless the trace already has a span with that ID (an
    /// exporter retry). Returns whether it was added. Only a new span moves
    /// `last_arrival`, so a retry never makes a complete trace incomplete.
    pub(crate) fn insert(&mut self, span: Span, now: u64) -> bool {
        if self.index.contains_key(&span.span_id) {
            return false;
        }
        self.index.insert(span.span_id, self.spans.len());
        self.spans.push(span);
        self.last_arrival = now;
        true
    }

    /// Nanoseconds since the last new span arrived (0 if the clock stepped
    /// backwards).
    pub(crate) fn quiet_for(&self, now: u64) -> u64 {
        now.saturating_sub(self.last_arrival)
    }

    pub(crate) fn snapshot(&self, trace_id: TraceId, complete: bool) -> TraceSnapshot {
        let mut spans = self.spans.clone();
        spans.sort_by_key(|span| (span.start_unix_nano, span.span_id));
        // Sorted, so the first parentless span is the earliest-starting one.
        let root = spans
            .iter()
            .find(|span| span.parent_span_id.is_none())
            .map(|span| span.span_id);
        let orphans = spans
            .iter()
            .filter(|span| {
                span.parent_span_id
                    .is_some_and(|parent| !self.index.contains_key(&parent))
            })
            .map(|span| span.span_id)
            .collect();
        TraceSnapshot {
            trace_id,
            spans,
            root,
            orphans,
            complete,
            // Failure classification fills these in on read later.
            status: TraceStatus::Ok,
            failed_in: None,
        }
    }
}
