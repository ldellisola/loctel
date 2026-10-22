//! The in-memory store.

use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap, hash_map::Entry};
use std::sync::Arc;

use loctel_model::{LogRecord, Span, SpanId, TraceId};
use parking_lot::RwLock;

use crate::clock::{Clock, saturating_nanos};
use crate::config::StoreConfig;
use crate::failure::{TraceStatus, classify};
use crate::logs::{Around, HistogramBucket, LogFilter, LogId, LogStore};
use crate::page::{Page, PageRequest};
use crate::service_map::{Aggregate, ServiceMap, ServiceStats, TimeWindow};
use crate::summary::{TraceFilter, TraceSummary, decode_cursor, encode_cursor};
use crate::trace::{TraceEntry, TraceSnapshot, root_span};

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

/// What a removal took out of the store.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Removed {
    /// Spans removed.
    pub spans: usize,
    /// Log records removed.
    pub logs: usize,
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
    /// Trace IDs by first arrival, oldest first: the key is the trace's
    /// arrival number (`TraceEntry::arrival`). Holds exactly the traces of
    /// `traces`. A map rather than a queue so that removing a trace from the
    /// middle costs a lookup, not a pass over every trace.
    arrival_order: BTreeMap<u64, TraceId>,
    /// The arrival number the next new trace gets. Never reused.
    next_arrival: u64,
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
        let Page { items, next_cursor } = self.search_logs_keyed(filter, page);
        Page {
            items: items.into_iter().map(|(_, record)| record).collect(),
            next_cursor,
        }
    }

    /// [`search_logs`](Self::search_logs) with each log's [`LogId`]: the same
    /// logs in the same order, and the same cursor. The ID names the log for
    /// [`log`](Self::log) and [`logs_around`](Self::logs_around) for as long
    /// as the log is held.
    pub fn search_logs_keyed(
        &self,
        filter: &LogFilter,
        page: &PageRequest,
    ) -> Page<(LogId, LogRecord)> {
        self.shared.inner.read().logs.search(filter, page)
    }

    /// The log held under `id`, or `None` if it was evicted, removed or
    /// cleared (or never existed). An ID is never given to a second log.
    pub fn log(&self, id: LogId) -> Option<LogRecord> {
        self.shared.inner.read().logs.get(id)
    }

    /// A histogram of the logs in `[since, until)` (nanoseconds since the
    /// Unix epoch, by the log's time as for `since_unix_nano` of
    /// [`LogFilter`]) that match `f`: `buckets` buckets of equal width,
    /// oldest first, each with the number of logs and how many of them are
    /// errors (error or fatal). The result is always `buckets` long. `f`'s
    /// own `since_unix_nano` and `until_unix_nano` are ignored; the window is
    /// the arguments. An empty or backwards window, or no buckets, counts
    /// nothing.
    ///
    /// It is a pass over the logs inside the window, which the Logs screen
    /// asks for every 2 s: see `docs/areas/store.md` for the cost.
    pub fn log_histogram(
        &self,
        f: &LogFilter,
        since: u64,
        until: u64,
        buckets: usize,
    ) -> Vec<HistogramBucket> {
        self.shared
            .inner
            .read()
            .logs
            .histogram(f, since, until, buckets)
    }

    /// Up to `before` of the logs held just before `id`, the log itself, and
    /// up to `after` of the logs just after it, oldest first, with every log
    /// counted whatever its service or level (no filter). `target` is the
    /// index of `id` in the result when the log is still held. When it is
    /// not (evicted, removed or cleared), `target` is `None` and the lines
    /// are the ones nearest to where it was: the `before` just older than its
    /// time and the `after` just newer.
    pub fn logs_around(&self, id: LogId, before: usize, after: usize) -> Around {
        self.shared.inner.read().logs.around(id, before, after)
    }

    /// The logs emitted in span `span` of trace `trace`, oldest first. Logs
    /// and spans are stored independently, so this works whether or not the
    /// span itself is held.
    pub fn logs_for_span(&self, trace: TraceId, span: SpanId) -> Vec<LogRecord> {
        self.shared.inner.read().logs.for_span(trace, span)
    }

    /// Removes the trace and every held log that carries its trace ID (with or
    /// without a span ID), and says how many spans and logs that was. Costs the
    /// trace's spans plus its logs, not the size of the store. A trace the
    /// store does not hold removes only its logs (logs are stored
    /// independently of spans).
    ///
    /// What the store holds goes down; `spans_received` and `logs_received` do
    /// not change. The removed spans and logs are forgotten, so a retry of one
    /// of them is accepted again as new.
    pub fn remove_trace(&self, id: TraceId) -> Removed {
        self.shared.inner.write().remove_trace(id)
    }

    /// Removes the given spans of trace `trace` and the logs emitted in them
    /// (the logs whose trace ID and span ID both match, whether or not the span
    /// itself is held). A span ID the trace does not have is ignored for the
    /// spans, and a span listed twice counts once. Removing the last span of
    /// the trace removes the trace; logs that name the trace but no span stay.
    ///
    /// The trace's start, duration and orphans follow the spans that are left.
    /// Counters behave as for [`remove_trace`](Self::remove_trace).
    pub fn remove_spans(&self, trace: TraceId, ids: &[SpanId]) -> Removed {
        self.shared.inner.write().remove_spans(trace, ids)
    }

    /// Runs `f` on the spans the store holds for trace `id`, under the read
    /// lock and without copying them, and returns its result. `None` (and `f`
    /// does not run) if the trace is not held. The spans are in arrival order,
    /// not sorted.
    ///
    /// `f` runs while the store is locked for reading: keep it short, and do
    /// not call this store from inside it (a write would wait for the lock the
    /// caller holds).
    pub fn with_trace_spans<R>(&self, id: TraceId, f: impl FnOnce(&[Span]) -> R) -> Option<R> {
        let inner = self.shared.inner.read();
        inner.traces.get(&id).map(|entry| f(entry.spans()))
    }

    /// Calls `f` on every held trace that starts at or after `since_unix_nano`
    /// with its ID, its spans (as held, in arrival order) and its status as
    /// the failure rules read it now. Traces come in the order they first
    /// arrived, oldest first. For the rules preview and anything else that has
    /// to look at all of the data.
    ///
    /// The same locking rules as [`with_trace_spans`](Self::with_trace_spans):
    /// `f` runs under the read lock, once per trace, so keep it short.
    pub fn scan_traces(
        &self,
        since_unix_nano: u64,
        mut f: impl FnMut(TraceId, &[Span], TraceStatus),
    ) {
        let inner = self.shared.inner.read();
        for id in inner.arrival_order.values() {
            let Some(entry) = inner.traces.get(id) else {
                continue;
            };
            if entry.start_unix_nano() < since_unix_nano {
                continue;
            }
            let spans = entry.spans();
            let root = root_span(spans).map(|span| span.span_id);
            let (status, _) = classify(spans, root);
            f(*id, spans, status);
        }
    }

    /// Calls `f` on every held log whose time is at or after
    /// `since_unix_nano` (the log's own timestamp, or its observed time when it
    /// has none), oldest first. Runs under the read lock, like
    /// [`scan_traces`](Self::scan_traces).
    pub fn scan_logs(&self, since_unix_nano: u64, f: impl FnMut(&LogRecord)) {
        self.shared.inner.read().logs.scan(since_unix_nano, f);
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
    /// to the higher trace ID), one page at a time. With a `filter.query`, a
    /// trace matches when the query matches one of its spans (as the trace is
    /// held now), and its summary lists the first 20 matched spans.
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
            // The query before the summary: it is the condition that usually
            // rejects, and a rejected trace then costs no summary at all.
            let matched = match &filter.query {
                None => Vec::new(),
                Some(query) => match entry.matched_span_ids(query) {
                    Some(matched) => matched,
                    None => continue,
                },
            };
            let mut summary = entry.summary(id, self.shared.is_complete(entry, now));
            if filter.errors_only && summary.status == TraceStatus::Ok {
                continue;
            }
            summary.matched_span_ids = matched;
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

    /// The current time on the store's clock, in nanoseconds since the Unix
    /// epoch. Completeness is judged by this clock, and the MCP marker tool
    /// hands it to agents as the "since" mark, so both agree on "now" (and a
    /// test that drives the store with a `ManualClock` controls it).
    pub fn now_unix_nano(&self) -> u64 {
        self.shared.clock.now_unix_nano()
    }

    /// The earliest start of any trace held (the earliest span start of the
    /// trace that began first), or `None` when the store holds no trace. A
    /// start, not an arrival: a trace that arrived late can hold it.
    ///
    /// It is a pass over the traces that touches no span, cheap beside a
    /// search or the service map, and it takes the read lock like them. The
    /// window uses it to say how long the data it shows covers.
    pub fn oldest_start_unix_nano(&self) -> Option<u64> {
        let inner = self.shared.inner.read();
        inner.traces.values().map(TraceEntry::start_unix_nano).min()
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
                let arrival = self.next_arrival;
                self.next_arrival += 1;
                self.arrival_order.insert(arrival, trace_id);
                vacant.insert(TraceEntry::new(now, arrival))
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
            let Some((_, oldest)) = self.arrival_order.pop_first() else {
                break;
            };
            if let Some(entry) = self.traces.remove(&oldest) {
                self.span_count -= entry.len();
            }
        }
    }

    /// Drops trace `id`, if held, from every place that lists it.
    fn forget_trace(&mut self, id: TraceId) -> Option<TraceEntry> {
        let entry = self.traces.remove(&id)?;
        self.arrival_order.remove(&entry.arrival());
        self.span_count -= entry.len();
        Some(entry)
    }

    fn remove_trace(&mut self, id: TraceId) -> Removed {
        let spans = self.forget_trace(id).map_or(0, |entry| entry.len());
        let logs = self.logs.remove_trace(id);
        Removed { spans, logs }
    }

    fn remove_spans(&mut self, trace: TraceId, ids: &[SpanId]) -> Removed {
        let mut spans = 0;
        if let Some(entry) = self.traces.get_mut(&trace) {
            spans = entry.remove_spans(ids);
            self.span_count -= spans;
            if entry.len() == 0 {
                self.forget_trace(trace);
            }
        }
        let logs = self.logs.remove_spans(trace, ids);
        Removed { spans, logs }
    }
}
