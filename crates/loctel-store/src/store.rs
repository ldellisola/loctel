//! The in-memory store.

use std::cmp::Reverse;
use std::collections::{HashMap, VecDeque, hash_map::Entry};
use std::sync::Arc;

use loctel_model::{LogRecord, Span, SpanId, TraceId};
use parking_lot::RwLock;

use crate::clock::{Clock, saturating_nanos};
use crate::config::StoreConfig;
use crate::failure::TraceStatus;
use crate::logs::{LogFilter, LogStore};
use crate::page::{Page, PageRequest};
use crate::service_map::{Aggregate, ServiceMap, ServiceStats, TimeWindow};
use crate::summary::{TraceFilter, TraceSummary, decode_cursor, encode_cursor};
use crate::trace::{TraceEntry, TraceSnapshot};

/// Counts of what the store holds and has received.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StoreStats {
    /// Traces held now.
    pub traces: usize,
    /// Spans held now.
    pub spans: usize,
    /// Log records held now.
    pub logs: usize,
    /// Spans accepted since the store was created: duplicates are not
    /// counted, spans evicted later still are, and `clear` keeps the total.
    pub spans_received: u64,
    /// Log records accepted since the store was created, counted like
    /// `spans_received`.
    pub logs_received: u64,
}

/// Holds traces and logs in memory. Cloning is cheap and the clones share
/// the same data, so hand one to every thread that needs it.
#[derive(Clone)]
pub struct Store {
    shared: Arc<Shared>,
}

struct Shared {
    config: StoreConfig,
    clock: Arc<dyn Clock>,
    inner: RwLock<Inner>,
}

#[derive(Default)]
struct Inner {
    traces: HashMap<TraceId, TraceEntry>,
    /// Trace IDs by first arrival, oldest first. Holds exactly the keys of
    /// `traces`.
    arrival_order: VecDeque<TraceId>,
    /// Spans across all held traces.
    span_count: usize,
    spans_received: u64,
    /// Every held log record, and the indexes over them.
    logs: LogStore,
    logs_received: u64,
}

impl Store {
    pub fn new(config: StoreConfig, clock: Arc<dyn Clock>) -> Store {
        Store {
            shared: Arc::new(Shared {
                config,
                clock,
                inner: RwLock::new(Inner::default()),
            }),
        }
    }

    /// Adds spans to their traces, in any order and from any service. A span
    /// whose ID the trace already has is ignored. When the batch is applied,
    /// the oldest traces beyond `max_traces` are evicted whole.
    pub fn ingest_spans(&self, spans: Vec<Span>) {
        if spans.is_empty() {
            return;
        }
        let mut inner = self.shared.inner.write();
        let now = self.shared.clock.now_unix_nano();
        for span in spans {
            inner.insert_span(span, now);
        }
        inner.evict_beyond(self.shared.config.max_traces);
    }

    /// Adds log records, in any order. A record identical to one already
    /// held (an exporter retry: same time, body, trace, span and service) is
    /// ignored. When the batch is applied, the oldest records beyond
    /// `max_log_records` are evicted.
    pub fn ingest_logs(&self, logs: Vec<LogRecord>) {
        if logs.is_empty() {
            return;
        }
        let mut inner = self.shared.inner.write();
        for log in logs {
            if inner.logs.insert(log) {
                inner.logs_received += 1;
            }
        }
        inner.logs.evict_beyond(self.shared.config.max_log_records);
    }

    /// The logs matching `filter`, newest first, one page at a time.
    ///
    /// The cursor is a keyset on the log's time and arrival, so the next page
    /// continues right after the last item even if newer logs arrive or older
    /// ones are evicted meanwhile. `next_cursor` is `None` on the last page.
    /// A cursor that this method did not produce gives an empty page.
    pub fn search_logs(&self, filter: &LogFilter, page: &PageRequest) -> Page<LogRecord> {
        self.shared.inner.read().logs.search(filter, page)
    }

    /// The logs emitted in span `span` of trace `trace`, oldest first. Logs
    /// and spans are stored independently, so this works whether or not the
    /// span itself is held.
    pub fn logs_for_span(&self, trace: TraceId, span: SpanId) -> Vec<LogRecord> {
        self.shared.inner.read().logs.for_span(trace, span)
    }

    /// The service map over the spans that start inside `window`: services
    /// and their dependencies as nodes, calls between them as edges. Computed
    /// from the traces held, on every call. See `docs/areas/store.md`
    /// ("Service map") for the rules.
    pub fn service_map(&self, window: TimeWindow) -> ServiceMap {
        let inner = self.shared.inner.read();
        let now = self.shared.clock.now_unix_nano();
        let mut aggregate = Aggregate::default();
        for entry in inner.traces.values() {
            aggregate.add_trace(entry, self.shared.is_complete(entry, now), window, true);
        }
        aggregate.into_map()
    }

    /// The services with spans inside `window`, sorted by name: the service
    /// nodes of [`service_map`](Self::service_map), with when each was last
    /// seen.
    pub fn services(&self, window: TimeWindow) -> Vec<ServiceStats> {
        let inner = self.shared.inner.read();
        let mut aggregate = Aggregate::default();
        for entry in inner.traces.values() {
            // Completeness only matters to dependencies.
            aggregate.add_trace(entry, false, window, false);
        }
        aggregate.into_services()
    }

    /// The trace as it stands now, or `None` if it is unknown or evicted.
    /// Its `status` and `failed_in` are worked out now, from the spans held.
    pub fn trace(&self, id: TraceId) -> Option<TraceSnapshot> {
        let inner = self.shared.inner.read();
        let entry = inner.traces.get(&id)?;
        let now = self.shared.clock.now_unix_nano();
        Some(entry.snapshot(id, self.shared.is_complete(entry, now)))
    }

    /// The trace in one row, or `None` if it is unknown or evicted.
    pub fn summary(&self, id: TraceId) -> Option<TraceSummary> {
        let inner = self.shared.inner.read();
        let entry = inner.traces.get(&id)?;
        let now = self.shared.clock.now_unix_nano();
        Some(entry.summary(id, self.shared.is_complete(entry, now)))
    }

    /// The traces matching `filter`, newest first by trace start (a tie goes
    /// to the higher trace ID), one page at a time.
    ///
    /// The cursor is a keyset: `"{start_unix_nano}-{trace_id}"` of the last
    /// item, so the next page continues right after it even if newer traces
    /// arrive meanwhile. `next_cursor` is `None` on the last page. A cursor
    /// that this method did not produce gives an empty page, which ends a
    /// reader's loop instead of restarting it.
    pub fn search_traces(&self, filter: &TraceFilter, page: &PageRequest) -> Page<TraceSummary> {
        let after = match page.cursor.as_deref() {
            None => None,
            Some(text) => match decode_cursor(text) {
                Some(position) => Some(position),
                None => return Page::empty(),
            },
        };
        if page.limit == 0 {
            return Page::empty();
        }

        let inner = self.shared.inner.read();
        let now = self.shared.clock.now_unix_nano();

        // The cheap conditions first: they need only the trace's start and
        // duration, which the entry keeps, not a pass over its spans.
        let mut candidates: Vec<(u64, TraceId, &TraceEntry)> = inner
            .traces
            .iter()
            .filter_map(|(&id, entry)| {
                let start = entry.start_unix_nano();
                let wanted = filter.since_unix_nano.is_none_or(|since| start >= since)
                    && filter
                        .min_duration_nanos
                        .is_none_or(|min| entry.duration_nanos() >= min)
                    && after.is_none_or(|position| (start, id) < position);
                wanted.then_some((start, id, entry))
            })
            .collect();
        candidates.sort_unstable_by_key(|&(start, id, _)| Reverse((start, id)));

        let mut items: Vec<TraceSummary> = Vec::new();
        let mut next_cursor = None;
        for (_, id, entry) in candidates {
            if let Some(service) = &filter.service
                && !entry.has_service(service)
            {
                continue;
            }
            let summary = entry.summary(id, self.shared.is_complete(entry, now));
            if filter.errors_only && summary.status == TraceStatus::Ok {
                continue;
            }
            if items.len() == page.limit {
                // One more match exists, so there is a next page.
                next_cursor = items
                    .last()
                    .map(|last| encode_cursor(last.start_unix_nano, last.trace_id));
                break;
            }
            items.push(summary);
        }
        Page { items, next_cursor }
    }

    pub fn stats(&self) -> StoreStats {
        let inner = self.shared.inner.read();
        StoreStats {
            traces: inner.traces.len(),
            spans: inner.span_count,
            logs: inner.logs.len(),
            spans_received: inner.spans_received,
            logs_received: inner.logs_received,
        }
    }

    /// Empties the store. The `*_received` totals are kept.
    pub fn clear(&self) {
        let mut inner = self.shared.inner.write();
        inner.traces.clear();
        inner.arrival_order.clear();
        inner.span_count = 0;
        inner.logs.clear();
    }
}

impl Shared {
    /// No new span for `completeness_quiet` as of `now`.
    fn is_complete(&self, entry: &TraceEntry, now: u64) -> bool {
        entry.quiet_for(now) >= saturating_nanos(self.config.completeness_quiet)
    }
}

impl Inner {
    fn insert_span(&mut self, span: Span, now: u64) {
        let trace_id = span.trace_id;
        let entry = match self.traces.entry(trace_id) {
            Entry::Occupied(occupied) => occupied.into_mut(),
            Entry::Vacant(vacant) => {
                self.arrival_order.push_back(trace_id);
                vacant.insert(TraceEntry::new(now))
            }
        };
        if entry.insert(span, now) {
            self.span_count += 1;
            self.spans_received += 1;
        }
    }

    /// Drops the oldest traces, whole, until at most `max_traces` remain.
    fn evict_beyond(&mut self, max_traces: usize) {
        while self.traces.len() > max_traces {
            let Some(oldest) = self.arrival_order.pop_front() else {
                break;
            };
            if let Some(entry) = self.traces.remove(&oldest) {
                self.span_count -= entry.len();
            }
        }
    }
}
