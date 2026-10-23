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
#[derive(Clone)]
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
    /// How many of the spans are erroring.
    errors: usize,
    /// The `(start, span ID)` of the root: the parentless span that started
    /// first, which is what [`root_span`] finds. `None` while there is none.
    root: Option<(u64, SpanId)>,
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
            errors: 0,
            root: None,
        }
    }

    /// A trace read back from a segment: its `spans` (which have distinct IDs,
    /// as they did when they were written), the trace's number in the order of
    /// first arrival and the clock time its last new span arrived, which the
    /// segment does not hold but the index does.
    pub(crate) fn from_spans(spans: Vec<Span>, last_arrival: u64, arrival: u64) -> Self {
        let mut entry = Self::new(last_arrival, arrival);
        entry.spans.reserve(spans.len());
        entry.index.reserve(spans.len());
        for span in spans {
            entry.insert(span, last_arrival);
        }
        entry
    }

    pub(crate) fn len(&self) -> usize {
        self.spans.len()
    }

    /// Clock time at which the last new span arrived.
    pub(crate) fn last_arrival(&self) -> u64 {
        self.last_arrival
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
        self.count(&span);
        self.spans.push(span);
        self.last_arrival = now;
        true
    }

    /// Takes `span`, which the trace holds or is about to, into the error count
    /// and the choice of the root.
    fn count(&mut self, span: &Span) {
        self.errors += usize::from(span.is_error());
        if span.parent_span_id.is_none() {
            let key = (span.start_unix_nano, span.span_id);
            if self.root.is_none_or(|root| key < root) {
                self.root = Some(key);
            }
        }
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
        self.errors = 0;
        self.root = None;
        let spans = std::mem::take(&mut self.spans);
        for (position, span) in spans.iter().enumerate() {
            self.index.insert(span.span_id, position);
            self.min_start = self.min_start.min(span.start_unix_nano);
            self.max_end = self.max_end.max(span_end(span));
            self.count(span);
        }
        self.spans = spans;
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

    /// Did the request work? What [`classify`] says of the spans, kept as they
    /// come and go, so that it costs nothing to ask: a search that wants the
    /// failed traces asks it of every trace it looks at.
    pub(crate) fn status(&self) -> TraceStatus {
        if self.errors == 0 {
            return TraceStatus::Ok;
        }
        let root_erroring = self
            .root
            .and_then(|(_, id)| self.span(id))
            .is_some_and(Span::is_error);
        if root_erroring {
            TraceStatus::Failed
        } else {
            TraceStatus::ErrorsInside
        }
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use loctel_model::{Resource, SpanKind, StatusCode};

    use super::*;

    fn span(id: u8, parent: Option<u8>, start: u64, erroring: bool) -> Span {
        Span {
            trace_id: TraceId([1; 16]),
            span_id: SpanId([id; 8]),
            parent_span_id: parent.map(|parent| SpanId([parent; 8])),
            name: "x".to_string(),
            kind: SpanKind::Internal,
            start_unix_nano: start,
            end_unix_nano: start + 10,
            status: if erroring {
                StatusCode::Error
            } else {
                StatusCode::Unset
            },
            status_message: String::new(),
            attributes: Vec::new(),
            events: Vec::new(),
            resource: Arc::new(Resource::default()),
            scope_name: String::new(),
        }
    }

    /// The status the failure rules give the spans the entry holds: what
    /// `status` must always say.
    fn by_the_rules(entry: &TraceEntry) -> TraceStatus {
        let root = root_span(entry.spans()).map(|span| span.span_id);
        classify(entry.spans(), root).0
    }

    #[test]
    fn status_follows_the_spans_as_they_arrive() {
        let mut entry = TraceEntry::new(0, 0);
        assert_eq!(entry.status(), TraceStatus::Ok, "no spans");

        // A child that errors before its root has arrived: errors inside.
        entry.insert(span(2, Some(1), 20, true), 1);
        assert_eq!(entry.status(), TraceStatus::ErrorsInside);
        // The root arrives and is fine.
        entry.insert(span(1, None, 10, false), 2);
        assert_eq!(entry.status(), TraceStatus::ErrorsInside);
        // A second parentless span that started earlier is the root now, and
        // erroring: the request failed.
        entry.insert(span(3, None, 5, true), 3);
        assert_eq!(entry.status(), TraceStatus::Failed);
        // A parentless span that started later is not the root.
        entry.insert(span(4, None, 50, false), 4);
        assert_eq!(entry.status(), TraceStatus::Failed);
        // A retry of a span the trace has changes nothing, even if it now says
        // that it is fine.
        assert!(!entry.insert(span(3, None, 5, false), 5));
        assert_eq!(entry.status(), TraceStatus::Failed);
        assert_eq!(entry.status(), by_the_rules(&entry));
    }

    #[test]
    fn status_follows_the_spans_as_they_are_removed() {
        let mut entry = TraceEntry::new(0, 0);
        entry.insert(span(1, None, 10, true), 1);
        entry.insert(span(2, Some(1), 20, true), 1);
        entry.insert(span(3, Some(1), 30, false), 1);
        assert_eq!(entry.status(), TraceStatus::Failed);

        // The erroring root goes: the next parentless span would be the root,
        // and there is none, so what errors is inside.
        entry.remove_spans(&[SpanId([1; 8])]);
        assert_eq!(entry.status(), TraceStatus::ErrorsInside);
        entry.remove_spans(&[SpanId([2; 8])]);
        assert_eq!(entry.status(), TraceStatus::Ok);
        assert_eq!(entry.status(), by_the_rules(&entry));
    }

    #[test]
    fn a_trace_read_back_has_the_status_it_had() {
        let mut entry = TraceEntry::new(0, 0);
        entry.insert(span(1, None, 10, true), 1);
        entry.insert(span(2, Some(1), 20, false), 1);
        let back = TraceEntry::from_spans(entry.spans().to_vec(), 1, 0);
        assert_eq!(back.status(), TraceStatus::Failed);
        assert_eq!(back.status(), entry.status());
    }

    #[test]
    fn status_equals_the_failure_rules_on_any_sequence() {
        // A fixed pseudo-random walk over spans arriving in any order with any
        // parents (parent cycles and self-parents included), then removals.
        let mut seed = 0x2545_F491_4F6C_DD1D_u64;
        let mut next = move |below: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % below
        };
        for round in 0..200 {
            let mut entry = TraceEntry::new(0, 0);
            for _ in 0..next(30) {
                let id = 1 + next(24) as u8;
                let parent = (next(3) != 0).then(|| 1 + next(24) as u8);
                let erroring = next(4) == 0;
                entry.insert(span(id, parent, next(100), erroring), 1);
                assert_eq!(
                    entry.status(),
                    by_the_rules(&entry),
                    "round {round}, insert"
                );
            }
            for _ in 0..next(6) {
                let doomed: Vec<SpanId> = (0..next(5))
                    .map(|_| SpanId([1 + next(24) as u8; 8]))
                    .collect();
                entry.remove_spans(&doomed);
                assert_eq!(
                    entry.status(),
                    by_the_rules(&entry),
                    "round {round}, remove"
                );
            }
        }
    }
}
