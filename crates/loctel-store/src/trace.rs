//! One trace: the stored entry and the snapshot handed to readers.

use std::collections::{HashMap, HashSet};

use loctel_model::{Span, SpanId, TraceId};

use crate::failure::{FailedIn, TraceStatus, classify};

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
    /// Did the request work (design 9.4)? Computed when the snapshot is
    /// taken, so a late span can change it.
    pub status: TraceStatus,
    /// The span that explains the failure, when any span is erroring.
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
    /// Where the trace sits in the store's order of first arrival (the key of
    /// the store's `arrival_order`). Never changes while the trace is held.
    arrival: u64,
    /// In arrival order.
    spans: Vec<Span>,
    /// Span ID to position in `spans`: the dedup index.
    index: HashMap<SpanId, usize>,
    /// Clock time at which the last new span arrived.
    last_arrival: u64,
    /// The earliest span start: the trace's start. `u64::MAX` while empty.
    min_start: u64,
    /// The latest span end, each end counted no earlier than its own span's
    /// start (so clock skew never makes a trace end before it begins).
    max_end: u64,
}

/// The root of `spans` (in any order): the parentless span that started
/// first, a tie going to the lowest span ID.
pub(crate) fn root_span(spans: &[Span]) -> Option<&Span> {
    spans
        .iter()
        .filter(|span| span.parent_span_id.is_none())
        .min_by_key(|span| (span.start_unix_nano, span.span_id))
}

/// When a span ends, as far as the trace's extent is concerned: an end before
/// the span's own start (clock skew) counts as that start.
fn span_end(span: &Span) -> u64 {
    span.end_unix_nano.max(span.start_unix_nano)
}

impl TraceEntry {
    pub(crate) fn new(now: u64, arrival: u64) -> Self {
        Self {
            arrival,
            spans: Vec::new(),
            index: HashMap::new(),
            last_arrival: now,
            min_start: u64::MAX,
            max_end: 0,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.spans.len()
    }

    /// The trace's number in the store's order of first arrival.
    pub(crate) fn arrival(&self) -> u64 {
        self.arrival
    }

    /// Adds `span` unless the trace already has a span with that ID (an
    /// exporter retry). Returns whether it was added. Only a new span moves
    /// `last_arrival`, so a retry never makes a complete trace incomplete.
    pub(crate) fn insert(&mut self, span: Span, now: u64) -> bool {
        if self.index.contains_key(&span.span_id) {
            return false;
        }
        self.index.insert(span.span_id, self.spans.len());
        self.min_start = self.min_start.min(span.start_unix_nano);
        self.max_end = self.max_end.max(span_end(&span));
        self.spans.push(span);
        self.last_arrival = now;
        true
    }

    /// Removes the spans whose IDs are in `ids` (IDs the trace does not have
    /// are ignored) and returns how many went. The dedup index and the
    /// trace's start and end are brought up to date, so they describe the spans
    /// that are left. `last_arrival` is left alone: removing a span is not a
    /// new arrival, and it must not make a quiet trace incomplete again.
    ///
    /// Takes time linear in the trace's spans, whatever `ids` holds.
    pub(crate) fn remove_spans(&mut self, ids: &[SpanId]) -> usize {
        let doomed: HashSet<SpanId> = ids
            .iter()
            .copied()
            .filter(|id| self.index.contains_key(id))
            .collect();
        if doomed.is_empty() {
            return 0;
        }
        self.spans.retain(|span| !doomed.contains(&span.span_id));
        self.index.clear();
        self.min_start = u64::MAX;
        self.max_end = 0;
        for (position, span) in self.spans.iter().enumerate() {
            self.index.insert(span.span_id, position);
            self.min_start = self.min_start.min(span.start_unix_nano);
            self.max_end = self.max_end.max(span_end(span));
        }
        doomed.len()
    }

    /// The spans, in arrival order.
    pub(crate) fn spans(&self) -> &[Span] {
        &self.spans
    }

    /// The span with ID `id`, if the trace has it.
    pub(crate) fn span(&self, id: SpanId) -> Option<&Span> {
        self.index.get(&id).map(|&position| &self.spans[position])
    }

    /// The latest span end, each counted no earlier than its own span's
    /// start, so no span starts after it. 0 while empty.
    pub(crate) fn end_unix_nano(&self) -> u64 {
        self.max_end
    }

    /// When the trace started: its earliest span start.
    pub(crate) fn start_unix_nano(&self) -> u64 {
        self.min_start
    }

    /// From the earliest span start to the latest span end (0 if they are
    /// out of order).
    pub(crate) fn duration_nanos(&self) -> u64 {
        self.max_end.saturating_sub(self.min_start)
    }

    /// Nanoseconds since the last new span arrived (0 if the clock stepped
    /// backwards).
    pub(crate) fn quiet_for(&self, now: u64) -> u64 {
        now.saturating_sub(self.last_arrival)
    }

    pub(crate) fn snapshot(&self, trace_id: TraceId, complete: bool) -> TraceSnapshot {
        let mut spans = self.spans.clone();
        spans.sort_by_key(|span| (span.start_unix_nano, span.span_id));
        let root = root_span(&spans).map(|span| span.span_id);
        let orphans = spans
            .iter()
            .filter(|span| {
                span.parent_span_id
                    .is_some_and(|parent| !self.index.contains_key(&parent))
            })
            .map(|span| span.span_id)
            .collect();
        let (status, failed_in) = classify(&spans, root);
        TraceSnapshot {
            trace_id,
            spans,
            root,
            orphans,
            complete,
            status,
            failed_in,
        }
    }
}
