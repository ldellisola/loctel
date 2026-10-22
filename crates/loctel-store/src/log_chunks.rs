//! Log chunks: how the store holds its logs (Milestone 5, Task 3).
//!
//! The logs are a list of **chunks**. New logs go to the **open chunk**, in
//! memory, kept in time order (a `BTreeMap` by [`LogId`]) with the indexes of
//! M1 to M4 (the logs of a span, the logs of a trace). When it has
//! `log_chunk_max` logs, or has been open for `log_chunk_age` on the store's
//! clock, it is **sealed**: its logs become one sorted, immutable record, written
//! to a segment file when the store has one (and kept in memory as they are
//! when it has not, or when the write fails).
//!
//! A chunk is not a range of time. A late log goes to the open chunk whatever its
//! time, so chunks overlap when logs arrive late, and what the store lists is
//! always a **merge** of the chunks ([`Cursor`]): a heap of the chunks that can
//! hold the next log, which reads a sealed chunk only when it reaches the chunk's
//! newest (or oldest) log.
//!
//! Every chunk, the open one too, has a [`ChunkMeta`]: the first and last
//! [`LogId`] it holds, how many logs, how many at each level and from each
//! service, and a **count per second, service and level** (the cells). Sealed
//! chunks keep theirs in memory for good, so counting, finding the log at an
//! offset and the histogram can be answered for most chunks without reading
//! them, and a search can skip the chunks that cannot hold a match (no log of the
//! service, no log of the trace). Only a chunk that a window or a bucket edge
//! cuts through is read.
//!
//! Evicting the oldest logs trims a sealed chunk from the front: its first
//! `skip` logs are counted out in the metadata (no write), and the chunk is
//! dropped whole when none is left. Removing logs from the middle of one (a
//! discard rule) rewrites it.
//!
//! Everything here runs under the store's lock. Reads take `&self` and share
//! the [`ChunkCache`] through its own mutex; writes take `&mut self` and the
//! [`SegmentSet`] they write to.

use std::cmp::Ordering as CmpOrdering;
use std::collections::btree_map::Range as MapRange;
use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, BinaryHeap, HashMap, HashSet, VecDeque};
use std::hash::{BuildHasherDefault, Hasher};
use std::ops::Bound;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use loctel_model::{LogRecord, Severity, SpanId, TraceId};
use parking_lot::Mutex;

use crate::clock::saturating_nanos;
use crate::logs::{ALL_LEVELS, ERROR_LEVELS, LevelSet, LogId, level_index, log_time, span_link};
use crate::segment::{self, Kind, LogRecordWithId, decode_logs, encode_logs};
use crate::segment_set::SegmentSet;

/// The logs an open chunk takes before it is sealed.
pub(crate) const LOG_CHUNK_MAX: usize = 65_536;

/// How long an open chunk may stay open before it is sealed.
pub(crate) const LOG_CHUNK_AGE: Duration = Duration::from_secs(10);

/// The decoded chunks kept for the next read, least recently used out.
pub(crate) const CHUNK_CACHE: usize = 8;

/// Nanoseconds in a second: the width of the time cells.
const SECOND: u64 = 1_000_000_000;

/// The logs of a decoded chunk, sorted by [`LogId`].
pub(crate) type Rows = Vec<LogRecordWithId>;

// ---- services ---------------------------------------------------------------------

/// The number a service name is known by inside the metadata. Names are
/// numbered in the order they are first seen and never forgotten.
pub(crate) type ServiceIdx = u32;

#[derive(Default)]
pub(crate) struct Services {
    by_name: HashMap<Box<str>, ServiceIdx>,
}

impl Services {
    pub(crate) fn get(&self, name: &str) -> Option<ServiceIdx> {
        self.by_name.get(name).copied()
    }

    fn intern(&mut self, name: &str) -> ServiceIdx {
        if let Some(&index) = self.by_name.get(name) {
            return index;
        }
        let index = ServiceIdx::try_from(self.by_name.len()).unwrap_or(ServiceIdx::MAX);
        self.by_name.insert(name.into(), index);
        index
    }
}

/// What a filter asks of the level and the service of a log: the part of an
/// indexed filter that the metadata can count.
#[derive(Clone, Copy)]
pub(crate) struct Selector<'a> {
    /// The levels that pass, one bit each (see `LevelSet`).
    pub(crate) levels: u8,
    pub(crate) service: ServiceSel<'a>,
}

#[derive(Clone, Copy)]
pub(crate) enum ServiceSel<'a> {
    /// Any service.
    Any,
    /// A service nobody has logged from: nothing passes.
    Never,
    /// This one, by its number (for the metadata) and its name (for a log).
    One(ServiceIdx, &'a str),
}

impl<'a> Selector<'a> {
    pub(crate) fn new(
        levels: Option<LevelSet>,
        service: Option<&'a str>,
        services: &Services,
    ) -> Selector<'a> {
        Selector {
            levels: levels.map_or(ALL_LEVELS, LevelSet::mask),
            service: match service {
                None => ServiceSel::Any,
                Some(name) => match services.get(name) {
                    Some(index) => ServiceSel::One(index, name),
                    None => ServiceSel::Never,
                },
            },
        }
    }

    /// Nothing can pass.
    pub(crate) fn is_empty(&self) -> bool {
        self.levels == 0 || matches!(self.service, ServiceSel::Never)
    }

    pub(crate) fn matches(&self, record: &LogRecord) -> bool {
        self.levels & (1 << level_index(record.severity)) != 0
            && match self.service {
                ServiceSel::Any => true,
                ServiceSel::Never => false,
                ServiceSel::One(_, name) => record.service() == name,
            }
    }

    /// Whether the logs of a cell (a service and a level) pass.
    pub(crate) fn matches_cell(&self, service: ServiceIdx, level: u8) -> bool {
        self.passes_level(level)
            && match self.service {
                ServiceSel::Any => true,
                ServiceSel::Never => false,
                ServiceSel::One(index, _) => index == service,
            }
    }

    /// The same, for the logs that are errors (error or fatal) among them.
    pub(crate) fn errors(&self) -> Selector<'a> {
        Selector {
            levels: self.levels & ERROR_LEVELS,
            service: self.service,
        }
    }

    fn passes_level(&self, level: u8) -> bool {
        self.levels & (1 << level) != 0
    }
}

// ---- metadata -------------------------------------------------------------------------

/// The logs of one second, from one service, at one level.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct CellKey {
    pub(crate) second: u64,
    pub(crate) service: ServiceIdx,
    pub(crate) level: u8,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Cell {
    pub(crate) key: CellKey,
    pub(crate) count: u32,
}

/// What the store knows about a chunk without reading it. The numbers are
/// exact: they follow the chunk's logs through every insert, trim and removal.
#[derive(Clone, Debug)]
pub(crate) struct ChunkMeta {
    /// The oldest log held, and the newest: the chunk's range of ids. Not
    /// meaningful while `count` is 0.
    pub(crate) first: LogId,
    pub(crate) last: LogId,
    /// Logs held.
    pub(crate) count: u32,
    /// Logs held at each level, by `level_index`.
    pub(crate) level_counts: [u32; 7],
    /// Logs held from each service.
    pub(crate) service_counts: HashMap<ServiceIdx, u32>,
    /// Logs held per second, service and level, sorted by key, none of them 0.
    /// The per-second histogram of the plan, with the service and the level kept
    /// in it so that a filter on either can still be answered from it.
    pub(crate) cells: Vec<Cell>,
    /// The traces that have logs in a sealed chunk, sorted. A superset after
    /// logs are trimmed off the front (never missing a trace that has a log).
    /// Empty for the open chunk, which asks its own index.
    pub(crate) traces: Vec<TraceId>,
}

impl ChunkMeta {
    fn empty() -> ChunkMeta {
        let zero = LogId {
            time_unix_nano: 0,
            seq: 0,
        };
        ChunkMeta {
            first: zero,
            last: zero,
            count: 0,
            level_counts: [0; 7],
            service_counts: HashMap::new(),
            cells: Vec::new(),
            traces: Vec::new(),
        }
    }

    /// The metadata of `rows`, which must be sorted.
    fn from_rows(rows: &[LogRecordWithId], services: &mut Services) -> ChunkMeta {
        let mut meta = ChunkMeta::empty();
        let mut traces = Vec::new();
        for row in rows {
            let service = services.intern(row.record.service());
            meta.add(row.id, row.record.severity, service);
            if let Some(trace) = row.record.trace_id {
                traces.push(trace);
            }
        }
        traces.sort_unstable();
        traces.dedup();
        meta.traces = traces;
        meta
    }

    fn add(&mut self, id: LogId, severity: Severity, service: ServiceIdx) {
        if self.count == 0 {
            self.first = id;
            self.last = id;
        } else {
            self.first = self.first.min(id);
            self.last = self.last.max(id);
        }
        self.count += 1;
        let level = level_index(severity);
        self.level_counts[usize::from(level)] += 1;
        *self.service_counts.entry(service).or_insert(0) += 1;
        let key = CellKey {
            second: id.time_unix_nano / SECOND,
            service,
            level,
        };
        match self.cells.binary_search_by_key(&key, |cell| cell.key) {
            Ok(at) => self.cells[at].count += 1,
            Err(at) => self.cells.insert(at, Cell { key, count: 1 }),
        }
    }

    /// One log less. Leaves `first` and `last` to the caller, who knows the
    /// next ones.
    fn sub(&mut self, id: LogId, severity: Severity, service: ServiceIdx) {
        self.count = self.count.saturating_sub(1);
        let level = level_index(severity);
        let held = &mut self.level_counts[usize::from(level)];
        *held = held.saturating_sub(1);
        if let Entry::Occupied(mut entry) = self.service_counts.entry(service) {
            if *entry.get() <= 1 {
                entry.remove();
            } else {
                *entry.get_mut() -= 1;
            }
        }
        let key = CellKey {
            second: id.time_unix_nano / SECOND,
            service,
            level,
        };
        if let Ok(at) = self.cells.binary_search_by_key(&key, |cell| cell.key) {
            if self.cells[at].count <= 1 {
                self.cells.remove(at);
            } else {
                self.cells[at].count -= 1;
            }
        }
    }

    /// The levels that have a log, one bit each.
    pub(crate) fn present_levels(&self) -> u8 {
        self.level_counts
            .iter()
            .enumerate()
            .filter(|(_, count)| **count > 0)
            .fold(0, |mask, (level, _)| mask | (1 << level))
    }

    pub(crate) fn has_service(&self, service: ServiceIdx) -> bool {
        self.service_counts.contains_key(&service)
    }

    /// The logs held that `selector` passes (the whole chunk, whatever the
    /// window).
    pub(crate) fn count_matching(&self, selector: &Selector<'_>) -> u64 {
        if selector.is_empty() {
            return 0;
        }
        let every_level = selector.levels & ALL_LEVELS == ALL_LEVELS;
        match selector.service {
            ServiceSel::Never => 0,
            ServiceSel::Any if every_level => u64::from(self.count),
            ServiceSel::Any => (0..7u8)
                .filter(|level| selector.passes_level(*level))
                .map(|level| u64::from(self.level_counts[usize::from(level)]))
                .sum(),
            ServiceSel::One(service, _) if every_level => self
                .service_counts
                .get(&service)
                .map_or(0, |count| u64::from(*count)),
            ServiceSel::One(service, _) => self
                .cells
                .iter()
                .filter(|cell| cell.key.service == service && selector.passes_level(cell.key.level))
                .map(|cell| u64::from(cell.count))
                .sum(),
        }
    }
}

// ---- chunks ----------------------------------------------------------------------------

/// A log held in the open chunk.
pub(crate) struct Stored {
    pub(crate) record: LogRecord,
    /// `record.dedup_key()`, kept so that dropping the log can forget it
    /// without hashing again.
    dedup: u64,
}

/// The chunk that is taking logs.
pub(crate) struct Open {
    /// In time order (oldest first).
    pub(crate) records: BTreeMap<LogId, Stored>,
    /// The held logs of each span, oldest first.
    by_span: HashMap<(TraceId, SpanId), VecDeque<LogId>>,
    /// The held logs that carry a trace ID (with or without a span ID) by
    /// trace, oldest first.
    by_trace: HashMap<TraceId, VecDeque<LogId>>,
    pub(crate) meta: ChunkMeta,
    /// The store's clock when the first log of the chunk arrived.
    opened: u64,
}

impl Open {
    fn new() -> Open {
        Open {
            records: BTreeMap::new(),
            by_span: HashMap::new(),
            by_trace: HashMap::new(),
            meta: ChunkMeta::empty(),
            opened: 0,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.records.len()
    }

    /// `meta.first` and `meta.last` after logs were taken out.
    fn refresh_bounds(&mut self) {
        if let (Some((first, _)), Some((last, _))) = (
            self.records.first_key_value(),
            self.records.last_key_value(),
        ) {
            self.meta.first = *first;
            self.meta.last = *last;
        }
    }
}

/// Where a sealed chunk's logs are.
enum Data {
    /// In memory, as they were sealed (no segment files, or the write failed).
    Memory(Arc<Rows>),
    /// In a segment file.
    Disk(segment::Location),
}

/// A sealed chunk: immutable but for trimming (`skip`) and rewriting.
pub(crate) struct Chunk {
    pub(crate) meta: ChunkMeta,
    data: Data,
    /// The first `skip` logs of the stored record were evicted: the chunk is
    /// its logs from there on. The metadata already says so.
    skip: usize,
}

impl Chunk {
    pub(crate) fn location(&self) -> Option<segment::Location> {
        match &self.data {
            Data::Disk(location) => Some(*location),
            Data::Memory(_) => None,
        }
    }

    /// The part of `rows` (the stored record, sorted) that is held and lies in
    /// `[lo, hi)`, as a range of indexes.
    fn window(
        &self,
        rows: &[LogRecordWithId],
        lo: Option<LogId>,
        hi: Option<LogId>,
    ) -> (usize, usize) {
        let start = lo
            .map_or(0, |lo| rows.partition_point(|row| row.id < lo))
            .max(self.skip)
            .min(rows.len());
        let end = hi
            .map_or(rows.len(), |hi| rows.partition_point(|row| row.id < hi))
            .max(start);
        (start, end)
    }
}

/// Either kind of chunk, to be asked the same things.
#[derive(Clone, Copy)]
pub(crate) enum ChunkRef<'a> {
    Open(&'a Open),
    Sealed(&'a Chunk),
}

impl<'a> ChunkRef<'a> {
    pub(crate) fn meta(self) -> &'a ChunkMeta {
        match self {
            ChunkRef::Open(open) => &open.meta,
            ChunkRef::Sealed(chunk) => &chunk.meta,
        }
    }

    /// Whether the chunk may hold a log of `trace`: never "no" wrongly.
    pub(crate) fn may_have_trace(self, trace: &TraceId) -> bool {
        match self {
            ChunkRef::Open(open) => open.by_trace.contains_key(trace),
            ChunkRef::Sealed(chunk) => chunk.meta.traces.binary_search(trace).is_ok(),
        }
    }
}

/// The decoded chunks read last, most recently used first. Keyed by the
/// position of the chunk's record, which nothing else ever gets again, so an
/// entry cannot be mistaken for another version of its chunk.
#[derive(Default)]
struct ChunkCache {
    entries: VecDeque<(segment::Location, Arc<Rows>)>,
}

impl ChunkCache {
    fn get(&mut self, location: segment::Location) -> Option<Arc<Rows>> {
        let at = self.entries.iter().position(|(key, _)| *key == location)?;
        let hit = self.entries.remove(at)?;
        let rows = Arc::clone(&hit.1);
        self.entries.push_front(hit);
        Some(rows)
    }

    fn put(&mut self, location: segment::Location, rows: Arc<Rows>) {
        self.remove(location);
        self.entries.push_front((location, rows));
        self.entries.truncate(CHUNK_CACHE);
    }

    fn remove(&mut self, location: segment::Location) {
        if let Some(at) = self.entries.iter().position(|(key, _)| *key == location) {
            self.entries.remove(at);
        }
    }
}

/// The dedup keys are hashes already: the set uses them as they are.
#[derive(Default)]
pub(crate) struct KeyHasher(u64);

impl Hasher for KeyHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 = self.0.rotate_left(8) ^ u64::from(*byte);
        }
    }

    fn write_u64(&mut self, key: u64) {
        self.0 = key;
    }
}

/// What the hidden `Store::log_chunks` says, for tests.
#[doc(hidden)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LogChunkInfo {
    /// Logs in the open chunk.
    pub open: usize,
    /// The sealed chunks, oldest seal first, as the number of logs each holds
    /// now.
    pub sealed: Vec<usize>,
    /// How many of the sealed chunks are in segment files.
    pub on_disk: usize,
    /// Decoded chunks in the cache now.
    pub cached: usize,
    /// How many chunks have been decoded from a segment so far (a read that
    /// finds the chunk in the cache is not one).
    pub decodes: u64,
}

/// The logs the store holds: the open chunk, the sealed ones, and what they
/// share (the dedup keys, the arrival counter, the service numbers, the cache).
pub(crate) struct LogStore {
    pub(crate) open: Open,
    pub(crate) sealed: Vec<Chunk>,
    pub(crate) services: Services,
    /// The dedup keys of the held logs.
    held: HashSet<u64, BuildHasherDefault<KeyHasher>>,
    /// Arrival counter for `LogId`. Never reset, so a cursor taken before a
    /// `clear` cannot point at a later log.
    next_seq: u64,
    chunk_max: usize,
    chunk_age: u64,
    cache: Mutex<ChunkCache>,
    /// How many chunks have been decoded from a segment: a test hook.
    decodes: AtomicU64,
    /// The last write to a segment failed, and why (taken by the store).
    write_error: Option<String>,
    /// A write to a segment worked since the store last asked.
    wrote: bool,
}

impl Default for LogStore {
    fn default() -> Self {
        LogStore::new(LOG_CHUNK_MAX, LOG_CHUNK_AGE)
    }
}

impl LogStore {
    pub(crate) fn new(chunk_max: usize, chunk_age: Duration) -> LogStore {
        LogStore {
            open: Open::new(),
            sealed: Vec::new(),
            services: Services::default(),
            held: HashSet::default(),
            next_seq: 0,
            chunk_max: chunk_max.max(1),
            chunk_age: saturating_nanos(chunk_age),
            cache: Mutex::new(ChunkCache::default()),
            decodes: AtomicU64::new(0),
            write_error: None,
            wrote: false,
        }
    }

    /// Logs held, in every chunk.
    pub(crate) fn len(&self) -> usize {
        self.open.len()
            + self
                .sealed
                .iter()
                .map(|chunk| chunk.meta.count as usize)
                .sum::<usize>()
    }

    /// The open chunk, then the sealed ones (in the order they were sealed),
    /// leaving out the ones that hold nothing.
    pub(crate) fn chunk_refs(&self) -> impl Iterator<Item = ChunkRef<'_>> {
        std::iter::once(ChunkRef::Open(&self.open))
            .chain(self.sealed.iter().map(ChunkRef::Sealed))
            .filter(|chunk| chunk.meta().count > 0)
    }

    pub(crate) fn info(&self) -> LogChunkInfo {
        LogChunkInfo {
            open: self.open.len(),
            sealed: self
                .sealed
                .iter()
                .map(|chunk| chunk.meta.count as usize)
                .collect(),
            on_disk: self
                .sealed
                .iter()
                .filter(|chunk| chunk.location().is_some())
                .count(),
            cached: self.cache.lock().entries.len(),
            decodes: self.decodes.load(Ordering::Relaxed),
        }
    }

    /// Takes the reason the last segment write failed, if one did since.
    pub(crate) fn take_write_error(&mut self) -> Option<String> {
        self.write_error.take()
    }

    /// Whether a segment write worked since the last call.
    pub(crate) fn take_wrote(&mut self) -> bool {
        std::mem::take(&mut self.wrote)
    }

    // ---- writing -----------------------------------------------------------------------

    /// Stores `record` in the open chunk, in time order, unless an identical log
    /// is already held (an exporter retry). Returns whether it was stored. It
    /// does not seal: the caller asks [`open_is_full`](Self::open_is_full).
    pub(crate) fn insert(&mut self, record: LogRecord, now: u64) -> bool {
        let dedup = record.dedup_key();
        if !self.held.insert(dedup) {
            return false;
        }
        let id = LogId {
            time_unix_nano: log_time(&record),
            seq: self.next_seq,
        };
        self.next_seq += 1;
        let open = &mut self.open;
        if open.records.is_empty() {
            open.opened = now;
        }
        let service = self.services.intern(record.service());
        open.meta.add(id, record.severity, service);
        if let Some(link) = span_link(&record) {
            insert_in_order(open.by_span.entry(link).or_default(), id);
        }
        if let Some(trace) = record.trace_id {
            insert_in_order(open.by_trace.entry(trace).or_default(), id);
        }
        open.records.insert(id, Stored { record, dedup });
        true
    }

    pub(crate) fn open_is_full(&self) -> bool {
        self.open.len() >= self.chunk_max
    }

    /// The open chunk has logs and has been open for the chunk age.
    pub(crate) fn open_is_old(&self, now: u64) -> bool {
        !self.open.records.is_empty() && now.saturating_sub(self.open.opened) >= self.chunk_age
    }

    /// Seals the open chunk, if it holds anything: its logs become a record of
    /// the segment files when there are any (and the write works), else stay in
    /// memory as they are.
    pub(crate) fn seal(&mut self, segments: Option<&mut SegmentSet>) {
        if self.open.records.is_empty() {
            return;
        }
        let Open {
            records,
            by_trace,
            mut meta,
            ..
        } = std::mem::replace(&mut self.open, Open::new());
        let mut traces: Vec<TraceId> = by_trace.into_keys().collect();
        traces.sort_unstable();
        meta.traces = traces;
        let rows: Rows = records
            .into_iter()
            .map(|(id, stored)| LogRecordWithId {
                id,
                record: stored.record,
            })
            .collect();
        let data = self.store_rows(rows, segments);
        self.sealed.push(Chunk {
            meta,
            data,
            skip: 0,
        });
    }

    /// Writes `rows` to the segment files if there are any, and says where they
    /// are now. A write that fails leaves them in memory, and is remembered.
    fn store_rows(&mut self, rows: Rows, segments: Option<&mut SegmentSet>) -> Data {
        let Some(segments) = segments else {
            return Data::Memory(Arc::new(rows));
        };
        // A suspended segment set refuses before the chunk is encoded: that is
        // the work a full disk must not make the store do on every seal.
        let written = segments
            .check_writable()
            .and_then(|()| segments.append(Kind::Logs, &encode_logs(&rows)));
        match written {
            Ok(location) => {
                self.wrote = true;
                Data::Disk(location)
            }
            Err(error) => {
                self.write_error = Some(error.to_string());
                Data::Memory(Arc::new(rows))
            }
        }
    }

    /// The record `location` is not wanted any more: not read again, and its
    /// bytes are dead.
    fn kill(&mut self, location: Option<segment::Location>, segments: Option<&mut SegmentSet>) {
        let Some(location) = location else {
            return;
        };
        self.cache.get_mut().remove(location);
        if let Some(segments) = segments {
            segments.mark_dead(location);
        }
    }

    /// Forgets every log. The arrival counter keeps counting. The segment files
    /// are the caller's to delete.
    pub(crate) fn clear(&mut self) {
        self.open = Open::new();
        self.sealed.clear();
        self.held.clear();
        self.cache.get_mut().entries.clear();
        self.write_error = None;
        self.wrote = false;
    }

    /// Drops every sealed chunk whose record is in one of the segments `doomed`
    /// (sorted), which the caller is about to delete: the disk cap's eviction
    /// of the oldest files. Each chunk is read first, to forget the dedup keys of
    /// its logs as eviction by count does (a retry that arrives after its log
    /// went is taken as new); one that cannot be read is dropped all the same.
    /// Chunks in memory are in no segment and stay.
    pub(crate) fn drop_in_segments(
        &mut self,
        doomed: &[u64],
        mut segments: Option<&mut SegmentSet>,
    ) {
        let mut index = 0;
        while index < self.sealed.len() {
            let doomed_here = self.sealed[index]
                .location()
                .is_some_and(|location| doomed.binary_search(&location.seq).is_ok());
            if !doomed_here {
                index += 1;
                continue;
            }
            if let Some(rows) = self.decoded(&self.sealed[index], segments.as_deref(), false) {
                let skip = self.sealed[index].skip.min(rows.len());
                for row in &rows[skip..] {
                    self.held.remove(&row.record.dedup_key());
                }
            }
            self.drop_chunk(index, segments.as_deref_mut());
        }
    }

    /// Drops the oldest logs until at most `max` remain.
    pub(crate) fn evict_beyond(&mut self, max: usize, mut segments: Option<&mut SegmentSet>) {
        let mut excess = self.len().saturating_sub(max);
        while excess > 0 {
            let Some((which, bound)) = self.oldest() else {
                break;
            };
            let removed = match which {
                Which::Open => self.evict_open(excess, bound),
                Which::Sealed(index) => {
                    self.evict_sealed(index, excess, bound, segments.as_deref_mut())
                }
            };
            if removed == 0 {
                break;
            }
            excess = excess.saturating_sub(removed);
        }
    }

    /// The chunk that holds the oldest log, and the oldest log of the others (the
    /// first id the oldest chunk's run of oldest logs must stay below).
    fn oldest(&self) -> Option<(Which, Option<LogId>)> {
        let mut best: Option<(Which, LogId)> = None;
        let mut second: Option<LogId> = None;
        let mut consider = |which: Which, first: LogId| match best {
            Some((_, held)) if held <= first => {
                second = Some(second.map_or(first, |other| other.min(first)));
            }
            _ => {
                if let Some((_, held)) = best {
                    second = Some(second.map_or(held, |other| other.min(held)));
                }
                best = Some((which, first));
            }
        };
        if self.open.meta.count > 0 {
            consider(Which::Open, self.open.meta.first);
        }
        for (index, chunk) in self.sealed.iter().enumerate() {
            if chunk.meta.count > 0 {
                consider(Which::Sealed(index), chunk.meta.first);
            }
        }
        best.map(|(which, _)| (which, second))
    }

    /// Evicts up to `excess` of the oldest logs of the open chunk, as long as
    /// they are older than `bound`.
    fn evict_open(&mut self, excess: usize, bound: Option<LogId>) -> usize {
        let mut removed = 0;
        while removed < excess {
            let Some(entry) = self.open.records.first_entry() else {
                break;
            };
            let id = *entry.key();
            if bound.is_some_and(|bound| id >= bound) {
                break;
            }
            let stored = entry.remove();
            self.held.remove(&stored.dedup);
            if let Some(service) = self.services.get(stored.record.service()) {
                self.open.meta.sub(id, stored.record.severity, service);
            }
            // The oldest log overall is the oldest of its span and of its trace.
            if let Some(link) = span_link(&stored.record) {
                pop_oldest(&mut self.open.by_span, link, id);
            }
            if let Some(trace) = stored.record.trace_id {
                pop_oldest(&mut self.open.by_trace, trace, id);
            }
            removed += 1;
        }
        self.open.refresh_bounds();
        removed
    }

    /// Evicts up to `excess` of the oldest logs of sealed chunk `index`, as long
    /// as they are older than `bound`; the whole chunk if that is all it has
    /// (or all it can say, when its record cannot be read).
    fn evict_sealed(
        &mut self,
        index: usize,
        excess: usize,
        bound: Option<LogId>,
        segments: Option<&mut SegmentSet>,
    ) -> usize {
        let live = self.sealed[index].meta.count as usize;
        let Some(rows) = self.decoded(&self.sealed[index], segments.as_deref(), true) else {
            // Unreadable: nothing is lost that was not lost already.
            self.drop_chunk(index, segments);
            return live;
        };
        let chunk = &self.sealed[index];
        let held = &rows[chunk.skip.min(rows.len())..];
        let run = bound
            .map_or(held.len(), |bound| {
                held.partition_point(|row| row.id < bound)
            })
            .max(1);
        let take = run.min(excess);
        if take >= held.len() {
            for row in held {
                self.held.remove(&row.record.dedup_key());
            }
            self.drop_chunk(index, segments);
            return live;
        }
        let chunk = &mut self.sealed[index];
        for row in &held[..take] {
            self.held.remove(&row.record.dedup_key());
            if let Some(service) = self.services.get(row.record.service()) {
                chunk.meta.sub(row.id, row.record.severity, service);
            }
        }
        chunk.skip += take;
        chunk.meta.first = rows[chunk.skip].id;
        take
    }

    /// Takes sealed chunk `index` out, and its record with it.
    fn drop_chunk(&mut self, index: usize, segments: Option<&mut SegmentSet>) {
        let chunk = self.sealed.remove(index);
        self.kill(chunk.location(), segments);
    }

    /// Removes every held log that carries `trace` as its trace ID, with or
    /// without a span ID, and returns how many there were.
    pub(crate) fn remove_trace(
        &mut self,
        trace: TraceId,
        mut segments: Option<&mut SegmentSet>,
    ) -> usize {
        let mut removed = self.remove_open_trace(trace);
        for index in self.sealed_with_trace(&trace) {
            removed += self.remove_sealed_where(
                index,
                |record| record.trace_id == Some(trace),
                segments.as_deref_mut(),
            );
        }
        self.drop_empty_chunks();
        removed
    }

    /// Removes the held logs of the given spans of `trace` (whether or not the
    /// spans themselves are held) and returns how many there were. A log with
    /// the trace's ID but no span ID belongs to no span and stays.
    pub(crate) fn remove_spans(
        &mut self,
        trace: TraceId,
        spans: &[SpanId],
        mut segments: Option<&mut SegmentSet>,
    ) -> usize {
        let mut removed = self.remove_open_spans(trace, spans);
        let doomed: HashSet<SpanId> = spans.iter().copied().collect();
        for index in self.sealed_with_trace(&trace) {
            removed += self.remove_sealed_where(
                index,
                |record| {
                    record.trace_id == Some(trace)
                        && record.span_id.is_some_and(|span| doomed.contains(&span))
                },
                segments.as_deref_mut(),
            );
        }
        self.drop_empty_chunks();
        removed
    }

    fn sealed_with_trace(&self, trace: &TraceId) -> Vec<usize> {
        self.sealed
            .iter()
            .enumerate()
            .filter(|(_, chunk)| {
                chunk.meta.count > 0 && chunk.meta.traces.binary_search(trace).is_ok()
            })
            .map(|(index, _)| index)
            .collect()
    }

    /// Chunks that a removal emptied (they were taken out of the list by
    /// [`remove_sealed_where`](Self::remove_sealed_where) already when they
    /// could be; this only guards the list against a chunk of 0 logs).
    fn drop_empty_chunks(&mut self) {
        self.sealed.retain(|chunk| chunk.meta.count > 0);
    }

    fn remove_open_trace(&mut self, trace: TraceId) -> usize {
        let Some(keys) = self.open.by_trace.remove(&trace) else {
            return 0;
        };
        let mut removed = 0;
        for id in keys {
            let Some(stored) = self.open.records.remove(&id) else {
                continue;
            };
            self.held.remove(&stored.dedup);
            if let Some(service) = self.services.get(stored.record.service()) {
                self.open.meta.sub(id, stored.record.severity, service);
            }
            // Every log of the span is in this trace, so the span's whole index
            // entry goes (more than once, harmlessly).
            if let Some(link) = span_link(&stored.record) {
                self.open.by_span.remove(&link);
            }
            removed += 1;
        }
        self.open.refresh_bounds();
        removed
    }

    fn remove_open_spans(&mut self, trace: TraceId, spans: &[SpanId]) -> usize {
        let mut removed: HashSet<LogId> = HashSet::new();
        for &span in spans {
            // Taking the entry makes a span listed twice count once.
            let Some(keys) = self.open.by_span.remove(&(trace, span)) else {
                continue;
            };
            for id in keys {
                let Some(stored) = self.open.records.remove(&id) else {
                    continue;
                };
                self.held.remove(&stored.dedup);
                if let Some(service) = self.services.get(stored.record.service()) {
                    self.open.meta.sub(id, stored.record.severity, service);
                }
                removed.insert(id);
            }
        }
        if removed.is_empty() {
            return 0;
        }
        if let Entry::Occupied(mut keys) = self.open.by_trace.entry(trace) {
            keys.get_mut().retain(|id| !removed.contains(id));
            if keys.get().is_empty() {
                keys.remove();
            }
        }
        self.open.refresh_bounds();
        removed.len()
    }

    /// Removes the held logs of sealed chunk `index` that `doomed` says yes to,
    /// and returns how many. The chunk is read and written again without them
    /// (its first record is dead), or dropped when nothing is left.
    fn remove_sealed_where(
        &mut self,
        index: usize,
        doomed: impl Fn(&LogRecord) -> bool,
        mut segments: Option<&mut SegmentSet>,
    ) -> usize {
        let Some(rows) = self.decoded(&self.sealed[index], segments.as_deref(), false) else {
            return 0;
        };
        let chunk = &self.sealed[index];
        let held = &rows[chunk.skip.min(rows.len())..];
        let (gone, kept): (Vec<&LogRecordWithId>, Vec<&LogRecordWithId>) =
            held.iter().partition(|row| doomed(&row.record));
        if gone.is_empty() {
            return 0;
        }
        for row in &gone {
            self.held.remove(&row.record.dedup_key());
        }
        let removed = gone.len();
        let old = chunk.location();
        if kept.is_empty() {
            self.sealed[index].meta.count = 0;
            self.kill(old, segments);
            return removed;
        }
        let kept: Rows = kept.into_iter().cloned().collect();
        let meta = ChunkMeta::from_rows(&kept, &mut self.services);
        let data = self.store_rows(kept, segments.as_deref_mut());
        self.sealed[index] = Chunk {
            meta,
            data,
            skip: 0,
        };
        self.kill(old, segments);
        removed
    }

    // ---- reading -------------------------------------------------------------------------

    /// The logs of `chunk`, decoded: from memory, from the cache or read from
    /// its segment (and, with `keep`, remembered for the next read). `None` when
    /// the record cannot be read or decoded (the first is said on stderr, once
    /// per segment).
    pub(crate) fn decoded(
        &self,
        chunk: &Chunk,
        segments: Option<&SegmentSet>,
        keep: bool,
    ) -> Option<Arc<Rows>> {
        let location = match &chunk.data {
            Data::Memory(rows) => return Some(Arc::clone(rows)),
            Data::Disk(location) => *location,
        };
        if let Some(hit) = self.cache.lock().get(location) {
            return Some(hit);
        }
        let bytes = segments?.read(location).ok()?;
        let rows = Arc::new(decode_logs(&bytes).ok()?);
        self.decodes.fetch_add(1, Ordering::Relaxed);
        if keep {
            self.cache.lock().put(location, Arc::clone(&rows));
        }
        Some(rows)
    }

    /// Calls `f` on the logs of `chunk` in `[lo, hi)`, oldest first. `false`
    /// when the chunk could not be read (and `f` did not run).
    pub(crate) fn for_each_row(
        &self,
        chunk: ChunkRef<'_>,
        lo: Option<LogId>,
        hi: Option<LogId>,
        segments: Option<&SegmentSet>,
        keep: bool,
        mut f: impl FnMut(LogId, &LogRecord),
    ) -> bool {
        match chunk {
            ChunkRef::Open(open) => {
                if let Some(bounds) = range_bounds(lo, hi) {
                    for (&id, stored) in open.records.range(bounds) {
                        f(id, &stored.record);
                    }
                }
                true
            }
            ChunkRef::Sealed(chunk) => {
                let Some(rows) = self.decoded(chunk, segments, keep) else {
                    return false;
                };
                let (start, end) = chunk.window(&rows, lo, hi);
                for row in &rows[start..end] {
                    f(row.id, &row.record);
                }
                true
            }
        }
    }

    /// The log held under `id`.
    pub(crate) fn get(&self, id: LogId, segments: Option<&SegmentSet>) -> Option<LogRecord> {
        if let Some(stored) = self.open.records.get(&id) {
            return Some(stored.record.clone());
        }
        for chunk in &self.sealed {
            let meta = &chunk.meta;
            if meta.count == 0 || id < meta.first || id > meta.last {
                continue;
            }
            let Some(rows) = self.decoded(chunk, segments, true) else {
                continue;
            };
            let held = &rows[chunk.skip.min(rows.len())..];
            if let Ok(at) = held.binary_search_by_key(&id, |row| row.id) {
                return Some(held[at].record.clone());
            }
        }
        None
    }

    /// The logs of one span, oldest first.
    pub(crate) fn for_span(
        &self,
        trace: TraceId,
        span: SpanId,
        segments: Option<&SegmentSet>,
    ) -> Vec<LogRecord> {
        let mut found: Vec<(LogId, LogRecord)> = Vec::new();
        for id in self.open.by_span.get(&(trace, span)).into_iter().flatten() {
            if let Some(stored) = self.open.records.get(id) {
                found.push((*id, stored.record.clone()));
            }
        }
        for chunk in &self.sealed {
            if chunk.meta.count == 0 || chunk.meta.traces.binary_search(&trace).is_err() {
                continue;
            }
            self.for_each_row(
                ChunkRef::Sealed(chunk),
                None,
                None,
                segments,
                true,
                |id, record| {
                    if record.trace_id == Some(trace) && record.span_id == Some(span) {
                        found.push((id, record.clone()));
                    }
                },
            );
        }
        found.sort_unstable_by_key(|(id, _)| *id);
        found.into_iter().map(|(_, record)| record).collect()
    }
}

/// Which chunk.
#[derive(Clone, Copy)]
enum Which {
    Open,
    Sealed(usize),
}

/// Puts `key` into `keys`, which is in ascending order. Almost always at the
/// end; a late log lands further in.
fn insert_in_order(keys: &mut VecDeque<LogId>, key: LogId) {
    let at = keys.partition_point(|held| *held < key);
    keys.insert(at, key);
}

/// Drops `key`, the oldest of all held logs, from the front of the index entry
/// `id` (it is the oldest of that entry too), and the entry once it is empty.
fn pop_oldest<K: std::hash::Hash + Eq>(index: &mut HashMap<K, VecDeque<LogId>>, id: K, key: LogId) {
    if let Entry::Occupied(mut keys) = index.entry(id) {
        let oldest = keys.get_mut().pop_front();
        debug_assert_eq!(oldest, Some(key));
        if keys.get().is_empty() {
            keys.remove();
        }
    }
}

/// `[lo, hi)` as the bounds of a map range, or `None` when it is empty (a map
/// range panics when it starts after it ends).
fn range_bounds(lo: Option<LogId>, hi: Option<LogId>) -> Option<(Bound<LogId>, Bound<LogId>)> {
    if let (Some(lo), Some(hi)) = (lo, hi)
        && lo >= hi
    {
        return None;
    }
    Some((
        lo.map_or(Bound::Unbounded, Bound::Included),
        hi.map_or(Bound::Unbounded, Bound::Excluded),
    ))
}

// ---- the merge -------------------------------------------------------------------------------

/// Which way a [`Cursor`] walks.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    /// Newest log first: the order of every list loctel shows.
    Newest,
    /// Oldest log first.
    Oldest,
}

/// Where a chunk is in the merge.
enum State<'a> {
    /// Not read yet: the heap knows only the bound its logs are under.
    Pending,
    Open {
        range: MapRange<'a, LogId, Stored>,
        /// The next log, already taken from the range.
        head: Option<(LogId, &'a LogRecord)>,
    },
    Rows {
        rows: Arc<Rows>,
        /// What is left to give: `rows[start..end]`.
        start: usize,
        end: usize,
    },
    /// Nothing more (empty in the range, or unreadable).
    Done,
}

struct Source<'a> {
    chunk: ChunkRef<'a>,
    state: State<'a>,
}

/// A log the cursor has taken: in the open chunk (which outlives the cursor),
/// or in the rows of a source.
enum Taken<'a> {
    Open(&'a LogRecord),
    Row { source: usize, index: usize },
}

/// An entry of the heap: a chunk and the id its next log has (or, before it is
/// read, the id its logs are not beyond).
struct Item {
    rank: (u64, u64),
    source: usize,
}

impl PartialEq for Item {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == CmpOrdering::Equal
    }
}

impl Eq for Item {}

impl PartialOrd for Item {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}

impl Ord for Item {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        self.rank
            .cmp(&other.rank)
            .then(self.source.cmp(&other.source))
    }
}

/// The logs of every chunk in `[lo, hi)` as one list, in order, reading a sealed
/// chunk only when the walk comes to its first log. `next` lends the log until
/// the next call.
pub(crate) struct Cursor<'a> {
    store: &'a LogStore,
    segments: Option<&'a SegmentSet>,
    keep: bool,
    dir: Direction,
    lo: Option<LogId>,
    hi: Option<LogId>,
    sources: Vec<Source<'a>>,
    heap: BinaryHeap<Item>,
}

/// The max-heap's order for an id: biggest first for the newest-first walk,
/// smallest first (the bitwise complement) for the oldest-first one.
fn rank(dir: Direction, id: LogId) -> (u64, u64) {
    match dir {
        Direction::Newest => (id.time_unix_nano, id.seq),
        Direction::Oldest => (!id.time_unix_nano, !id.seq),
    }
}

fn pull<'a>(
    range: &mut MapRange<'a, LogId, Stored>,
    dir: Direction,
) -> Option<(LogId, &'a LogRecord)> {
    let next = match dir {
        Direction::Newest => range.next_back(),
        Direction::Oldest => range.next(),
    };
    next.map(|(id, stored)| (*id, &stored.record))
}

impl<'a> Cursor<'a> {
    /// A walk over the logs with `lo <= id < hi` (either side may be open) of
    /// the chunks that `wanted` says may hold what the caller is after. With
    /// `keep`, chunks it has to read are remembered in the cache.
    pub(crate) fn new(
        store: &'a LogStore,
        segments: Option<&'a SegmentSet>,
        keep: bool,
        dir: Direction,
        lo: Option<LogId>,
        hi: Option<LogId>,
        wanted: impl Fn(ChunkRef<'a>) -> bool,
    ) -> Cursor<'a> {
        let mut cursor = Cursor {
            store,
            segments,
            keep,
            dir,
            lo,
            hi,
            sources: Vec::new(),
            heap: BinaryHeap::new(),
        };
        for chunk in store.chunk_refs() {
            let meta = chunk.meta();
            if lo.is_some_and(|lo| meta.last < lo) || hi.is_some_and(|hi| meta.first >= hi) {
                continue;
            }
            if !wanted(chunk) {
                continue;
            }
            // Until it is read, a chunk's first log in the walk is no further
            // than the end of its range that the walk starts from.
            let bound = match dir {
                Direction::Newest => hi.map_or(meta.last, |hi| meta.last.min(hi)),
                Direction::Oldest => lo.map_or(meta.first, |lo| meta.first.max(lo)),
            };
            cursor.heap.push(Item {
                rank: rank(dir, bound),
                source: cursor.sources.len(),
            });
            cursor.sources.push(Source {
                chunk,
                state: State::Pending,
            });
        }
        cursor
    }

    /// The next log of the walk and its id, or `None` at its end.
    #[allow(clippy::should_implement_trait)]
    pub(crate) fn next(&mut self) -> Option<(LogId, &LogRecord)> {
        let (id, taken) = self.advance()?;
        match taken {
            Taken::Open(record) => Some((id, record)),
            Taken::Row { source, index } => match &self.sources[source].state {
                State::Rows { rows, .. } => Some((id, &rows[index].record)),
                _ => None,
            },
        }
    }

    /// Takes the next log off its chunk and puts that chunk's next one on the
    /// heap. Says where the log is, for [`next`](Self::next) to lend it.
    fn advance(&mut self) -> Option<(LogId, Taken<'a>)> {
        loop {
            let source = self.heap.pop()?.source;
            if matches!(self.sources[source].state, State::Pending) {
                self.open_source(source);
                continue;
            }
            let dir = self.dir;
            match &mut self.sources[source].state {
                State::Open { range, head } => {
                    let Some((id, record)) = head.take() else {
                        continue;
                    };
                    *head = pull(range, dir);
                    self.push_head(source);
                    return Some((id, Taken::Open(record)));
                }
                State::Rows { rows, start, end } => {
                    let index = match dir {
                        Direction::Newest => {
                            *end -= 1;
                            *end
                        }
                        Direction::Oldest => {
                            *start += 1;
                            *start - 1
                        }
                    };
                    let id = rows[index].id;
                    self.push_head(source);
                    return Some((id, Taken::Row { source, index }));
                }
                State::Pending | State::Done => {}
            }
        }
    }

    /// Reads chunk `source` (it came to the front of the heap) and puts its
    /// first log of the walk on the heap.
    fn open_source(&mut self, source: usize) {
        let state = match self.sources[source].chunk {
            ChunkRef::Open(open) => match range_bounds(self.lo, self.hi) {
                Some(bounds) => {
                    let mut range = open.records.range(bounds);
                    let head = pull(&mut range, self.dir);
                    State::Open { range, head }
                }
                None => State::Done,
            },
            ChunkRef::Sealed(chunk) => match self.store.decoded(chunk, self.segments, self.keep) {
                Some(rows) => {
                    let (start, end) = chunk.window(&rows, self.lo, self.hi);
                    State::Rows { rows, start, end }
                }
                None => State::Done,
            },
        };
        self.sources[source].state = state;
        self.push_head(source);
    }

    /// Puts the next log of `source`, if it has one, on the heap.
    fn push_head(&mut self, source: usize) {
        let id = match &self.sources[source].state {
            State::Open { head, .. } => head.map(|(id, _)| id),
            State::Rows { rows, start, end } if start < end => Some(match self.dir {
                Direction::Newest => rows[*end - 1].id,
                Direction::Oldest => rows[*start].id,
            }),
            _ => None,
        };
        if let Some(id) = id {
            self.heap.push(Item {
                rank: rank(self.dir, id),
                source,
            });
        }
    }
}
