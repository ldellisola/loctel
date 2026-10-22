//! The store: traces and logs, in memory and in segment files.
//!
//! A trace is **hot** while its spans are in memory and **cold** once they have
//! been written to a segment file (see [`trace_index`](crate::trace_index) and
//! `docs/areas/store.md`, "Hot and cold traces"). Nothing outside this module
//! can tell which it is, except through the counters of [`Store::spilled_traces`].
//! Logs are held in chunks, the newest in memory and the sealed ones in segment
//! files ([`log_chunks`](crate::log_chunks), "Log chunks" in the same document).

use std::collections::{HashMap, hash_map::Entry};
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use loctel_model::{LogRecord, Span, SpanId, TraceId};
use parking_lot::{Mutex, RwLock};

use crate::clock::{Clock, saturating_nanos};
use crate::config::StoreConfig;
use crate::failure::{TraceStatus, classify};
use crate::log_chunks::LogChunkInfo;
use crate::logs::{Around, HistogramBucket, LogFilter, LogId, LogStore};
use crate::page::{Page, PageRequest};
use crate::segment::{Kind, decode_trace, encode_trace};
use crate::segment_set::SegmentSet;
use crate::service_map::{Aggregate, ServiceMap, ServiceStats, TimeWindow};
use crate::summary::{TraceFilter, TraceSummary, decode_cursor, encode_cursor};
use crate::trace::{TraceEntry, TraceSnapshot, root_span};
use crate::trace_index::{Cold, ColdCache, Location, TraceIndex, TraceMeta};

/// Counts of what the store holds and has received.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StoreStats {
    /// Traces held now, in memory or in segment files.
    pub traces: usize,
    /// Spans held now, in memory or in segment files.
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

/// Holds traces and logs. Cloning is cheap and the clones share the same data,
/// so hand one to every thread that needs it. With a `spill_dir` in its
/// [`StoreConfig`] the traces that are complete move to files in that
/// directory, and the files are deleted when the last clone is dropped.
#[derive(Clone)]
pub struct Store {
    shared: Arc<Shared>,
}

/// `Shared::last_step` before the spill step has run.
const NEVER: u64 = u64::MAX;

struct Shared {
    config: StoreConfig,
    clock: Arc<dyn Clock>,
    inner: RwLock<Inner>,
    /// When the spill step last ran, on the store's clock: so that most calls
    /// can see, without a lock, that it is not their turn.
    last_step: AtomicU64,
}

#[derive(Default)]
struct Inner {
    /// Every trace held, hot or cold.
    index: TraceIndex,
    /// The spans of the hot traces.
    hot: HashMap<TraceId, TraceEntry>,
    /// The arrival number the next new trace gets. Never reused.
    next_arrival: u64,
    /// Spans across all held traces, hot and cold.
    span_count: usize,
    /// Spans of hot traces.
    hot_spans: usize,
    spans_received: u64,
    /// Every held log record, and the indexes over them.
    logs: LogStore,
    logs_received: u64,
    /// The segment files; `None` when the store keeps everything in memory.
    segments: Option<SegmentSet>,
    /// Decoded cold traces, filled by reads (hence the lock of its own).
    cache: Mutex<ColdCache>,
    /// How many traces have been decoded from a segment: a test hook.
    decodes: AtomicU64,
    /// Why the last write to a segment failed, until one works.
    spill_error: Option<String>,
}

impl Store {
    pub fn new(config: StoreConfig, clock: Arc<dyn Clock>) -> Store {
        let mut inner = Inner {
            logs: LogStore::new(config.log_chunk_max, config.log_chunk_age),
            ..Inner::default()
        };
        if let Some(dir) = &config.spill_dir {
            match SegmentSet::new(dir.clone(), config.segment_max_bytes) {
                Ok(segments) => inner.segments = Some(segments),
                Err(error) => {
                    eprintln!(
                        "loctel: cannot spill to {}: {error}; keeping everything in memory",
                        dir.display()
                    );
                    inner.spill_error = Some(error.to_string());
                }
            }
        }
        Store {
            shared: Arc::new(Shared {
                config,
                clock,
                inner: RwLock::new(inner),
                last_step: AtomicU64::new(NEVER),
            }),
        }
    }

    /// Adds spans to their traces, in any order and from any service. A span
    /// whose ID the trace already has is ignored. A span of a trace that has
    /// been spilled to a segment brings the whole trace back into memory first
    /// (and, being new, makes it incomplete again). When the batch is applied,
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
        self.shared.spill_step_if_due(&mut inner, now);
    }

    /// Adds log records, in any order. A record identical to one already
    /// held (an exporter retry: same time, body, trace, span and service) is
    /// ignored. When the batch is applied, the oldest records beyond
    /// `max_log_records` are evicted. The logs go to the open chunk, which is
    /// sealed when it has `log_chunk_max` of them or has been open for
    /// `log_chunk_age`, and written to a segment file if the store has them.
    pub fn ingest_logs(&self, logs: Vec<LogRecord>) {
        if logs.is_empty() {
            return;
        }
        let mut inner = self.shared.inner.write();
        let now = self.shared.clock.now_unix_nano();
        // A chunk that has been open for its time seals before this batch, so
        // that the batch starts a new one.
        inner.seal_logs_if_old(now);
        for log in logs {
            if inner.logs.insert(log, now) {
                inner.logs_received += 1;
                if inner.logs.open_is_full() {
                    inner.seal_logs();
                }
            }
        }
        inner.evict_logs_beyond(self.shared.config.max_log_records);
        self.shared.spill_step_if_due(&mut inner, now);
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
        self.shared.spill_if_due();
        let inner = self.shared.inner.read();
        inner.logs.search(filter, page, inner.segments.as_ref())
    }

    /// The log held under `id`, or `None` if it was evicted, removed or
    /// cleared (or never existed). An ID is never given to a second log.
    pub fn log(&self, id: LogId) -> Option<LogRecord> {
        let inner = self.shared.inner.read();
        inner.logs.get(id, inner.segments.as_ref())
    }

    /// The time of the oldest log held (its own timestamp, or its observed time
    /// when it has none), in nanoseconds since the Unix epoch, or `None` when
    /// the store holds no log. The Logs screen's histogram starts here when it
    /// shows everything stored. A lookup, not a pass over the logs.
    pub fn oldest_log_unix_nano(&self) -> Option<u64> {
        self.shared.inner.read().logs.oldest_time()
    }

    /// How many held logs `f` keeps, for an *indexed* filter
    /// ([`LogFilter::is_indexed`]: it sets only the window, `levels` and
    /// `service`), and `None` for any other, which cannot be counted without
    /// reading the logs (use [`search_logs`](Self::search_logs) for those). The
    /// answer is exact. It comes from what the store keeps about each chunk of
    /// logs, and reads only the chunks that the edges of the window cut through,
    /// so it costs about the number of chunks held, not the number of logs: the
    /// Logs screen's total over millions of lines.
    pub fn log_count(&self, f: &LogFilter) -> Option<u64> {
        self.shared.spill_if_due();
        let inner = self.shared.inner.read();
        inner.logs.count(f, inner.segments.as_ref())
    }

    /// Up to `limit` of the logs `f` keeps, from position `offset` of the list
    /// newest first (the order of [`search_logs_keyed`](Self::search_logs_keyed),
    /// which is the same list): `offset` 0 is the newest log that passes. Fewer
    /// come back at the end of the list, and none past it. A list that is
    /// scrolled through, by position, is read with this instead of with a cursor
    /// that has to be walked to.
    ///
    /// For an indexed filter ([`LogFilter::is_indexed`]) it finds the position
    /// from the counts of the chunks and reads the chunks the position and the
    /// rows are in, so any position of ten million logs costs about the same.
    /// Any other filter is walked from the newest log, which costs the logs
    /// before `offset`: slow past the first few thousand.
    pub fn logs_at(&self, f: &LogFilter, offset: u64, limit: usize) -> Vec<(LogId, LogRecord)> {
        self.shared.spill_if_due();
        let inner = self.shared.inner.read();
        inner.logs.at(f, offset, limit, inner.segments.as_ref())
    }

    /// What the log chunks hold now. A test hook.
    #[doc(hidden)]
    pub fn log_chunks(&self) -> LogChunkInfo {
        self.shared.inner.read().logs.info()
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
    /// For an indexed filter ([`LogFilter::is_indexed`]) it is answered from the
    /// counts the store keeps per chunk and per second, and reads only the
    /// chunks it has to for the answer to be exact (the ones a bucket edge falls
    /// inside a second of, and the ones at the edges of the window). For any
    /// other filter it is a pass over the logs inside the window. The Logs
    /// screen asks for it every 2 s: see `docs/areas/store.md` for the cost.
    pub fn log_histogram(
        &self,
        f: &LogFilter,
        since: u64,
        until: u64,
        buckets: usize,
    ) -> Vec<HistogramBucket> {
        self.shared.spill_if_due();
        let inner = self.shared.inner.read();
        inner
            .logs
            .histogram(f, since, until, buckets, inner.segments.as_ref())
    }

    /// Up to `before` of the logs held just before `id`, the log itself, and
    /// up to `after` of the logs just after it, oldest first, with every log
    /// counted whatever its service or level (no filter). `target` is the
    /// index of `id` in the result when the log is still held. When it is
    /// not (evicted, removed or cleared), `target` is `None` and the lines
    /// are the ones nearest to where it was: the `before` just older than its
    /// time and the `after` just newer.
    pub fn logs_around(&self, id: LogId, before: usize, after: usize) -> Around {
        let inner = self.shared.inner.read();
        inner
            .logs
            .around(id, before, after, inner.segments.as_ref())
    }

    /// The logs emitted in span `span` of trace `trace`, oldest first. Logs
    /// and spans are stored independently, so this works whether or not the
    /// span itself is held.
    pub fn logs_for_span(&self, trace: TraceId, span: SpanId) -> Vec<LogRecord> {
        let inner = self.shared.inner.read();
        inner.logs.for_span(trace, span, inner.segments.as_ref())
    }

    /// Removes the trace and every held log that carries its trace ID (with or
    /// without a span ID), and says how many spans and logs that was. Costs the
    /// trace's spans plus its logs, not the size of the store. A trace the
    /// store does not hold removes only its logs (logs are stored
    /// independently of spans). A trace in a segment file is removed from the
    /// index without being read: its record stays in the file, dead, until the
    /// file goes.
    ///
    /// What the store holds goes down; `spans_received` and `logs_received` do
    /// not change. The removed spans and logs are forgotten, so a retry of one
    /// of them is accepted again as new.
    pub fn remove_trace(&self, id: TraceId) -> Removed {
        self.shared.spill_if_due();
        self.shared.inner.write().remove_trace(id)
    }

    /// Removes the given spans of trace `trace` and the logs emitted in them
    /// (the logs whose trace ID and span ID both match, whether or not the span
    /// itself is held). A span ID the trace does not have is ignored for the
    /// spans, and a span listed twice counts once. Removing the last span of
    /// the trace removes the trace; logs that name the trace but no span stay.
    ///
    /// The trace's start, duration and orphans follow the spans that are left.
    /// Counters behave as for [`remove_trace`](Self::remove_trace). A trace in
    /// a segment file that has one of the spans is brought back into memory to
    /// lose it, and is not made incomplete by that.
    pub fn remove_spans(&self, trace: TraceId, ids: &[SpanId]) -> Removed {
        self.shared.spill_if_due();
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
        self.shared.spill_if_due();
        let inner = self.shared.inner.read();
        match inner.hot.get(&id) {
            Some(entry) => Some(f(entry.spans())),
            None => Some(f(inner.load_cold(id, true)?.spans())),
        }
    }

    /// Calls `f` on every held trace that starts at or after `since_unix_nano`
    /// with its ID, its spans (as held, in arrival order) and its status as
    /// the failure rules read it now. Traces come in the order they first
    /// arrived, oldest first. For the rules preview and anything else that has
    /// to look at all of the data. A trace in a segment file is read for the
    /// call and not kept; one that cannot be read is left out.
    ///
    /// The same locking rules as [`with_trace_spans`](Self::with_trace_spans):
    /// `f` runs under the read lock, once per trace, so keep it short.
    pub fn scan_traces(
        &self,
        since_unix_nano: u64,
        mut f: impl FnMut(TraceId, &[Span], TraceStatus),
    ) {
        self.shared.spill_if_due();
        let inner = self.shared.inner.read();
        for id in inner.index.by_arrival() {
            let Some(meta) = inner.index.get(id) else {
                continue;
            };
            if meta.start < since_unix_nano {
                continue;
            }
            match &meta.location {
                Location::Hot => {
                    let Some(entry) = inner.hot.get(id) else {
                        continue;
                    };
                    let spans = entry.spans();
                    let root = root_span(spans).map(|span| span.span_id);
                    let (status, _) = classify(spans, root);
                    f(*id, spans, status);
                }
                Location::Cold(cold) => {
                    let Some(entry) = inner.load_cold(*id, false) else {
                        continue;
                    };
                    f(*id, entry.spans(), cold.summary.status);
                }
            }
        }
    }

    /// Calls `f` on every held log whose time is at or after
    /// `since_unix_nano` (the log's own timestamp, or its observed time when it
    /// has none), oldest first. Runs under the read lock, like
    /// [`scan_traces`](Self::scan_traces).
    pub fn scan_logs(&self, since_unix_nano: u64, f: impl FnMut(&LogRecord)) {
        let inner = self.shared.inner.read();
        inner.logs.scan(since_unix_nano, inner.segments.as_ref(), f);
    }

    /// The service map over the spans that start inside `window`: services
    /// and their dependencies as nodes, calls between them as edges. Computed
    /// from the traces held, on every call: a trace in a segment file that
    /// reaches into the window is read for it. See `docs/areas/store.md`
    /// ("Service map") for the rules.
    pub fn service_map(&self, window: TimeWindow) -> ServiceMap {
        self.shared.spill_if_due();
        let inner = self.shared.inner.read();
        let now = self.shared.clock.now_unix_nano();
        let mut aggregate = Aggregate::default();
        inner.for_traces_in(window, |entry, meta| {
            aggregate.add_trace(entry, self.shared.is_complete(meta, now), window, true);
        });
        aggregate.into_map()
    }

    /// The services with spans inside `window`, sorted by name: the service
    /// nodes of [`service_map`](Self::service_map), with when each was last
    /// seen.
    pub fn services(&self, window: TimeWindow) -> Vec<ServiceStats> {
        self.shared.spill_if_due();
        let inner = self.shared.inner.read();
        let mut aggregate = Aggregate::default();
        inner.for_traces_in(window, |entry, _| {
            // Completeness only matters to dependencies.
            aggregate.add_trace(entry, false, window, false);
        });
        aggregate.into_services()
    }

    /// The trace as it stands now, or `None` if it is unknown or evicted.
    /// Its `status` and `failed_in` are worked out now, from the spans held.
    pub fn trace(&self, id: TraceId) -> Option<TraceSnapshot> {
        self.shared.spill_if_due();
        let inner = self.shared.inner.read();
        let meta = inner.index.get(&id)?;
        let now = self.shared.clock.now_unix_nano();
        let complete = self.shared.is_complete(meta, now);
        match &meta.location {
            Location::Hot => Some(inner.hot.get(&id)?.snapshot(id, complete)),
            Location::Cold(_) => Some(inner.load_cold(id, true)?.snapshot(id, complete)),
        }
    }

    /// The trace in one row, or `None` if it is unknown or evicted. A trace in
    /// a segment file is answered from the index, without reading it.
    pub fn summary(&self, id: TraceId) -> Option<TraceSummary> {
        self.shared.spill_if_due();
        let inner = self.shared.inner.read();
        let meta = inner.index.get(&id)?;
        let now = self.shared.clock.now_unix_nano();
        inner.summary_of(meta, self.shared.is_complete(meta, now))
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
    ///
    /// The search walks the index from the newest trace (or from the cursor)
    /// and stops when the page is full and one more match has shown that there
    /// is a next page: time, service, errors and duration are judged from what
    /// the index holds, and only a query reads the spans of a trace that is in
    /// a segment file, one trace at a time.
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

        self.shared.spill_if_due();
        let inner = self.shared.inner.read();
        let now = self.shared.clock.now_unix_nano();

        let mut items: Vec<TraceSummary> = Vec::new();
        let mut next_cursor = None;
        for (start, id) in inner.index.newest_first(after) {
            // Newest first: nothing after this one starts late enough.
            if filter.since_unix_nano.is_some_and(|since| start < since) {
                break;
            }
            let Some(meta) = inner.index.get(&id) else {
                continue;
            };
            if filter
                .min_duration_nanos
                .is_some_and(|min| meta.duration_nanos() < min)
            {
                continue;
            }
            let complete = self.shared.is_complete(meta, now);
            let Some((mut summary, matched)) = inner.candidate(meta, filter, complete) else {
                continue;
            };
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
    /// A lookup in the index: it touches no span, in memory or in a segment
    /// file. The window uses it to say how long the data it shows covers.
    pub fn oldest_start_unix_nano(&self) -> Option<u64> {
        self.shared.spill_if_due();
        self.shared.inner.read().index.oldest_start()
    }

    pub fn stats(&self) -> StoreStats {
        self.shared.spill_if_due();
        let inner = self.shared.inner.read();
        StoreStats {
            traces: inner.index.len(),
            spans: inner.span_count,
            logs: inner.logs.len(),
            spans_received: inner.spans_received,
            logs_received: inner.logs_received,
        }
    }

    /// How many of the traces held are in segment files now.
    pub fn spilled_traces(&self) -> usize {
        self.shared.inner.read().index.cold_len()
    }

    /// What the segment files take on disk, bytes of dead records included
    /// (records of traces that came back to memory or were removed stay in
    /// their file until it is deleted). A test hook.
    #[doc(hidden)]
    pub fn spill_disk_bytes(&self) -> u64 {
        let inner = self.shared.inner.read();
        inner.segments.as_ref().map_or(0, SegmentSet::disk_bytes)
    }

    /// The part of [`spill_disk_bytes`](Self::spill_disk_bytes) that belongs to
    /// dead records. A test hook.
    #[doc(hidden)]
    pub fn spill_dead_bytes(&self) -> u64 {
        let inner = self.shared.inner.read();
        inner.segments.as_ref().map_or(0, SegmentSet::dead_bytes)
    }

    /// How many traces have been decoded from a segment file so far (a read
    /// that finds the trace in the small cache of decoded traces is not one).
    /// A test hook.
    #[doc(hidden)]
    pub fn trace_decodes(&self) -> u64 {
        self.shared.inner.read().decodes.load(Ordering::Relaxed)
    }

    /// Why the store last failed to write a segment, until a write works
    /// again. A test hook for now.
    #[doc(hidden)]
    pub fn spill_error(&self) -> Option<String> {
        self.shared.inner.read().spill_error.clone()
    }

    /// Empties the store, and deletes its segment files. The `*_received`
    /// totals are kept.
    pub fn clear(&self) {
        let mut inner = self.shared.inner.write();
        inner.hot.clear();
        inner.index.clear();
        inner.span_count = 0;
        inner.hot_spans = 0;
        inner.cache.get_mut().clear();
        if let Some(segments) = &mut inner.segments {
            segments.clear();
        }
        inner.logs.clear();
    }
}

impl Shared {
    /// No new span for `completeness_quiet` as of `now`.
    fn is_complete(&self, meta: &TraceMeta, now: u64) -> bool {
        meta.quiet_for(now) >= saturating_nanos(self.config.completeness_quiet)
    }

    /// The least time between two spill steps: a second, or the time a trace
    /// waits after it is complete when that is less (the test config, which
    /// waits for nothing, runs a step at every call).
    fn step_interval(&self) -> u64 {
        saturating_nanos(self.config.spill_after.min(Duration::from_secs(1)))
    }

    fn step_due(&self, now: u64) -> bool {
        let last = self.last_step.load(Ordering::Relaxed);
        // The clock may have stepped back, and then it is long since the last
        // step, whichever way round.
        last == NEVER || now.abs_diff(last) >= self.step_interval()
    }

    /// Runs the spill step ahead of a call that reads or removes, if it is due.
    /// Ingest runs it itself (it holds the lock already), and that is the
    /// moment memory grows. Reads run it too, so that a store that nobody
    /// writes to still gives its memory back, which is the common case once an
    /// exporter has stopped.
    fn spill_if_due(&self) {
        let now = self.clock.now_unix_nano();
        if !self.step_due(now) {
            return;
        }
        let mut inner = self.inner.write();
        self.spill_step_if_due(&mut inner, now);
    }

    /// The spill step, if no other has run in the last [`step_interval`]
    /// (`Shared::step_interval`). It seals the open chunk of logs if it has been
    /// open for `log_chunk_age`, and moves hot traces to the open segment,
    /// oldest first (by when their last span arrived):
    ///
    /// - a trace that has been quiet for `completeness_quiet + spill_after`
    ///   goes, and
    /// - while the hot traces hold more than `hot_max_spans` spans, so does the
    ///   oldest of the traces that are complete (quiet for `completeness_quiet`),
    ///   without waiting for `spill_after`.
    ///
    /// The traces still being written, or recently finished, stay in memory. A
    /// write that fails ends the step with the trace still hot (and the error
    /// kept in `spill_error`, to be retried by the next step).
    fn spill_step_if_due(&self, inner: &mut Inner, now: u64) {
        if !self.step_due(now) {
            return;
        }
        self.last_step.store(now, Ordering::Relaxed);
        // A chunk of logs that has been open long enough seals, with or without
        // segment files to write it to.
        inner.seal_logs_if_old(now);
        if inner.segments.is_none() {
            return;
        }
        let quiet = saturating_nanos(self.config.completeness_quiet);
        let wait = quiet.saturating_add(saturating_nanos(self.config.spill_after));
        while let Some((last_arrival, id)) = inner.index.oldest_hot() {
            let idle = now.saturating_sub(last_arrival);
            let aged = idle >= wait;
            let early = idle >= quiet && inner.hot_spans > self.config.hot_max_spans;
            if !(aged || early) {
                // The hot traces are in order of age: none after this one is
                // old enough either.
                break;
            }
            match inner.spill_trace(id) {
                Ok(()) => inner.spill_error = None,
                Err(error) => {
                    if inner.spill_error.is_none() {
                        eprintln!(
                            "loctel: cannot write a segment: {error}; keeping traces in memory"
                        );
                    }
                    inner.spill_error = Some(error.to_string());
                    break;
                }
            }
        }
    }
}

impl Inner {
    /// Seals the open chunk of logs, if it holds any, and notes whether it
    /// could be written to a segment.
    fn seal_logs(&mut self) {
        self.logs.seal(self.segments.as_mut());
        self.note_log_writes();
    }

    /// [`seal_logs`](Self::seal_logs), if the open chunk has been open for the
    /// chunk age.
    fn seal_logs_if_old(&mut self, now: u64) {
        if self.logs.open_is_old(now) {
            self.seal_logs();
        }
    }

    fn evict_logs_beyond(&mut self, max: usize) {
        self.logs.evict_beyond(max, self.segments.as_mut());
        self.note_log_writes();
    }

    /// What the log chunks wrote since the last call: a write that failed is
    /// the store's spill error (the logs stay in memory), one that worked ends
    /// it, as for traces.
    fn note_log_writes(&mut self) {
        let error = self.logs.take_write_error();
        let wrote = self.logs.take_wrote();
        if let Some(error) = error {
            if self.spill_error.is_none() {
                eprintln!("loctel: cannot write a segment: {error}; keeping logs in memory");
            }
            self.spill_error = Some(error);
        } else if wrote {
            self.spill_error = None;
        }
    }

    fn insert_span(&mut self, span: Span, now: u64) {
        let trace_id = span.trace_id;
        // A span for a trace that is in a segment brings the trace back, unless
        // the trace already has the span (a retry: nothing to do).
        if self.index.is_cold(&trace_id) && !self.wake_for(trace_id, span.span_id) {
            return;
        }
        let is_new_trace = !self.hot.contains_key(&trace_id);
        let entry = match self.hot.entry(trace_id) {
            Entry::Occupied(occupied) => occupied.into_mut(),
            Entry::Vacant(vacant) => {
                let arrival = self.next_arrival;
                self.next_arrival += 1;
                vacant.insert(TraceEntry::new(now, arrival))
            }
        };
        if entry.insert(span, now) {
            self.span_count += 1;
            self.hot_spans += 1;
            self.spans_received += 1;
            if is_new_trace {
                self.index.add_hot(trace_id, entry);
            } else {
                self.index.sync_hot(trace_id, entry);
            }
        }
    }

    /// Gets the cold trace `id` ready for the span `span_id` to join it, and
    /// says whether it should: `false` when the trace already has that span.
    /// A trace that cannot be read (its record is damaged) is gone, and the
    /// span starts a new one.
    fn wake_for(&mut self, id: TraceId, span_id: SpanId) -> bool {
        match self.load_cold(id, true) {
            Some(entry) if entry.span(span_id).is_some() => false,
            Some(entry) => {
                self.rehydrate(id, entry);
                true
            }
            None => {
                self.forget_trace(id);
                true
            }
        }
    }

    /// Brings the cold trace `id`, which was decoded to `decoded`, back into
    /// memory. Its record is dead from now on. Its `last_arrival` and arrival
    /// number are what they were: it is the caller's business whether what
    /// follows is a new arrival.
    fn rehydrate(&mut self, id: TraceId, decoded: Arc<TraceEntry>) {
        let Some(cold) = self.index.make_hot(id) else {
            return;
        };
        self.kill(cold.blob);
        // The cache's reference went with `kill`, so this is the only one
        // unless a reader holds the trace, which cannot be under the write lock.
        let entry = Arc::try_unwrap(decoded).unwrap_or_else(|shared| (*shared).clone());
        self.hot_spans += entry.len();
        self.hot.insert(id, entry);
    }

    /// The record `blob` is not wanted any more: not read again, and its bytes
    /// are dead.
    fn kill(&mut self, blob: crate::segment::Location) {
        self.cache.get_mut().remove(blob);
        if let Some(segments) = &mut self.segments {
            segments.mark_dead(blob);
        }
    }

    /// Writes the hot trace `id` to the open segment and leaves it cold. On an
    /// error the trace stays hot.
    fn spill_trace(&mut self, id: TraceId) -> io::Result<()> {
        let Some(segments) = self.segments.as_mut() else {
            return Err(io::Error::other("the store has no segment files"));
        };
        let Some(entry) = self.hot.get(&id) else {
            // The index lists a hot trace that is not there. It cannot happen,
            // and if it did, the step would ask for this trace for ever.
            self.index.remove(id);
            return Ok(());
        };
        let blob = segments.append(Kind::Traces, &encode_trace(entry.spans()))?;
        let cold = Cold {
            blob,
            summary: entry.summary(id, true),
        };
        self.hot_spans -= entry.len();
        self.hot.remove(&id);
        self.index.make_cold(id, cold);
        Ok(())
    }

    /// The cold trace `id`, decoded: from the cache when it is there, else
    /// read from its segment (and, with `keep`, remembered for the next read).
    /// `None` for a trace that is not cold, or whose record cannot be read or
    /// decoded (the first is said on stderr, once per segment).
    fn load_cold(&self, id: TraceId, keep: bool) -> Option<Arc<TraceEntry>> {
        let meta = self.index.get(&id)?;
        let Location::Cold(cold) = &meta.location else {
            return None;
        };
        if let Some(hit) = self.cache.lock().get(cold.blob) {
            return Some(hit);
        }
        let bytes = self.segments.as_ref()?.read(cold.blob).ok()?;
        let spans = decode_trace(&bytes).ok()?;
        self.decodes.fetch_add(1, Ordering::Relaxed);
        let entry = Arc::new(TraceEntry::from_spans(
            spans,
            meta.last_arrival,
            meta.arrival,
        ));
        if keep {
            self.cache.lock().put(cold.blob, Arc::clone(&entry));
        }
        Some(entry)
    }

    /// The trace in one row: from the entry when hot, from the index when cold.
    fn summary_of(&self, meta: &TraceMeta, complete: bool) -> Option<TraceSummary> {
        match &meta.location {
            Location::Hot => Some(self.hot.get(&meta.id)?.summary(meta.id, complete)),
            Location::Cold(cold) => {
                let mut summary = cold.summary.clone();
                summary.complete = complete;
                Some(summary)
            }
        }
    }

    /// The summary of the trace `meta` is of and the spans a query matched in
    /// it, if the trace passes the service, errors-only and query conditions
    /// of `filter` (the time and duration ones are the caller's). The cheap
    /// conditions come first, so a trace in a segment is read only when it
    /// has to be, for the query.
    fn candidate(
        &self,
        meta: &TraceMeta,
        filter: &TraceFilter,
        complete: bool,
    ) -> Option<(TraceSummary, Vec<SpanId>)> {
        let id = meta.id;
        match &meta.location {
            Location::Hot => {
                let entry = self.hot.get(&id)?;
                if let Some(service) = &filter.service
                    && !entry.has_service(service)
                {
                    return None;
                }
                // The query before the summary: it is the condition that
                // usually rejects, and a rejected trace then costs no summary.
                let matched = match &filter.query {
                    None => Vec::new(),
                    Some(query) => entry.matched_span_ids(query)?,
                };
                let summary = entry.summary(id, complete);
                if filter.errors_only && summary.status == TraceStatus::Ok {
                    return None;
                }
                Some((summary, matched))
            }
            Location::Cold(cold) => {
                if let Some(service) = &filter.service
                    && !cold.summary.services.iter().any(|name| name == service)
                {
                    return None;
                }
                if filter.errors_only && cold.summary.status == TraceStatus::Ok {
                    return None;
                }
                let matched = match &filter.query {
                    None => Vec::new(),
                    // Read for the query only, and not kept: a search walks
                    // many traces once and should not push out the one the
                    // window has open.
                    Some(query) => self.load_cold(id, false)?.matched_span_ids(query)?,
                };
                let mut summary = cold.summary.clone();
                summary.complete = complete;
                Some((summary, matched))
            }
        }
    }

    /// Calls `f` on every trace that reaches into `window` (it ends at or after
    /// its start), hot or cold, with the index's entry for it. A cold trace is
    /// read for the call and dropped, unless it is in the cache already; one
    /// that cannot be read is skipped. A trace that ended before the window is
    /// not read at all.
    fn for_traces_in(&self, window: TimeWindow, mut f: impl FnMut(&TraceEntry, &TraceMeta)) {
        for meta in self.index.iter() {
            if meta.end < window.since_unix_nano {
                continue;
            }
            match &meta.location {
                Location::Hot => {
                    if let Some(entry) = self.hot.get(&meta.id) {
                        f(entry, meta);
                    }
                }
                Location::Cold(_) => {
                    if let Some(entry) = self.load_cold(meta.id, false) {
                        f(&entry, meta);
                    }
                }
            }
        }
    }

    /// Drops the oldest traces, whole, until at most `max_traces` remain.
    fn evict_beyond(&mut self, max_traces: usize) {
        while self.index.len() > max_traces {
            let Some(oldest) = self.index.oldest_arrival() else {
                break;
            };
            self.forget_trace(oldest);
        }
    }

    /// Drops trace `id`, if held, from every place that lists it, and says how
    /// many spans it had. A trace in a segment is not read, only forgotten.
    fn forget_trace(&mut self, id: TraceId) -> Option<usize> {
        let meta = self.index.remove(id)?;
        let spans = match meta.location {
            Location::Hot => {
                // The entry, not the index, has the count: spans may have gone
                // from it a moment ago.
                let spans = self.hot.remove(&id).map_or(0, |entry| entry.len());
                self.hot_spans -= spans;
                spans
            }
            Location::Cold(cold) => {
                self.kill(cold.blob);
                meta.span_count
            }
        };
        self.span_count -= spans;
        Some(spans)
    }

    fn remove_trace(&mut self, id: TraceId) -> Removed {
        let spans = self.forget_trace(id).unwrap_or(0);
        let logs = self.logs.remove_trace(id, self.segments.as_mut());
        self.note_log_writes();
        Removed { spans, logs }
    }

    fn remove_spans(&mut self, trace: TraceId, ids: &[SpanId]) -> Removed {
        // A trace in a segment is read to see whether it has any of the spans,
        // and brought back only if it has: otherwise it stays where it is.
        if self.index.is_cold(&trace) {
            match self.load_cold(trace, true) {
                Some(entry) if ids.iter().any(|id| entry.span(*id).is_some()) => {
                    self.rehydrate(trace, entry);
                }
                Some(_) => {}
                None => {
                    self.forget_trace(trace);
                }
            }
        }
        let mut spans = 0;
        if let Some(entry) = self.hot.get_mut(&trace) {
            spans = entry.remove_spans(ids);
            self.span_count -= spans;
            self.hot_spans -= spans;
            if entry.len() == 0 {
                self.forget_trace(trace);
            } else {
                self.index.sync_hot(trace, entry);
            }
        }
        let logs = self.logs.remove_spans(trace, ids, self.segments.as_mut());
        self.note_log_writes();
        Removed { spans, logs }
    }
}
