//! Logs: the types of the log API, and what the store answers about its logs.
//!
//! The logs themselves are held in chunks ([`log_chunks`](crate::log_chunks)):
//! this module is the questions put to them. Search, the histogram, the lines
//! around a log and the scans walk the chunks as one list in time order (a
//! merge); the count, the log at an offset and the histogram of an *indexed*
//! filter are answered from the chunks' metadata where they can be, and read the
//! chunks that a window edge or a bucket edge cuts through.

use std::sync::Arc;

use loctel_model::{LogRecord, Severity, SpanId, TraceId};
use loctel_query::logql::LogQuery;

pub(crate) use crate::log_chunks::LogStore;
use crate::log_chunks::{ChunkRef, Cursor, Direction, Selector, ServiceIdx, Services};
use crate::page::{Page, PageRequest};
use crate::segment_set::SegmentSet;

/// What [`Store::search_logs`](crate::Store::search_logs) keeps. Every
/// condition that is set must hold; the default keeps everything.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LogFilter {
    /// Keep logs at or after this time (the log's own timestamp, or its
    /// observed time when it has none).
    pub since_unix_nano: Option<u64>,
    /// Keep logs strictly before this time (exclusive, by the same time as
    /// `since_unix_nano`), so `[since, until)` is a window that tiles with
    /// the next one.
    pub until_unix_nano: Option<u64>,
    /// Keep logs from this service (exact name; `unknown_service` for logs
    /// without one).
    pub service: Option<String>,
    /// Keep logs of this severity or higher. `Unspecified` is the lowest
    /// level, so any `Some(level)` above it drops unlabelled logs.
    pub min_severity: Option<Severity>,
    /// Keep logs whose severity is in this set (the Logs screen's level
    /// toggles). `None` keeps every level; `Some` of an empty set keeps
    /// nothing. Combines with `min_severity`: both must hold.
    pub levels: Option<LevelSet>,
    /// Keep logs whose body contains this text, ignoring case. Only the body
    /// is searched. The empty text matches every log.
    pub contains: Option<String>,
    /// Keep logs emitted in this trace.
    pub trace_id: Option<TraceId>,
    /// Keep logs emitted in a span with this ID (in any trace, unless
    /// `trace_id` is set too).
    pub span_id: Option<SpanId>,
    /// Keep logs that this LogQL query matches (see
    /// `loctel_query::logql::LogQuery::matches`). Parsed once by the caller
    /// and shared, so a search never re-parses it.
    pub query: Option<Arc<LogQuery>>,
}

impl LogFilter {
    /// Whether the filter is *indexed*: it sets only the window (`since`,
    /// `until`), `levels` and `service`, the conditions the store keeps counts
    /// for. [`Store::log_count`](crate::Store::log_count) answers only for
    /// such a filter, and [`Store::logs_at`](crate::Store::logs_at) and
    /// [`Store::log_histogram`](crate::Store::log_histogram) are fast only for
    /// one.
    pub fn is_indexed(&self) -> bool {
        self.min_severity.is_none()
            && self.contains.is_none()
            && self.trace_id.is_none()
            && self.span_id.is_none()
            && self.query.is_none()
    }

    /// The window as the ids it spans: `lo` is the first id in it, `hi` the
    /// first one out (a log at `until` has an arrival number of 0 or more, so
    /// it sits at or above `(until, 0)`).
    fn window(&self) -> (Option<LogId>, Option<LogId>) {
        let at = |time_unix_nano| LogId {
            time_unix_nano,
            seq: 0,
        };
        (self.since_unix_nano.map(at), self.until_unix_nano.map(at))
    }
}

/// Where a held log sits: when it happened, then when it arrived. Unique
/// and stable for as long as the log is held, which a position in a vector
/// would not be (late logs insert in the middle, eviction removes from the
/// front). It is never given to another log, even after the log is gone or
/// the store is cleared (the arrival counter only counts up).
///
/// IDs order logs the way the store does: by time, a tie going to the later
/// arrival. [`Store::log`](crate::Store::log) reads a log by its ID, and
/// [`Store::logs_around`](crate::Store::logs_around) its neighbours.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LogId {
    /// The log's time: its timestamp, or its observed time when it has none.
    pub time_unix_nano: u64,
    /// The log's arrival number: the store's own counter, never reused.
    pub seq: u64,
}

/// Which severities a [`LogFilter`] keeps: one bit per [`Severity`].
///
/// The Logs screen shows four toggles, each a group of severities; see
/// [`from_groups`](Self::from_groups). The default is the empty set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LevelSet(u8);

impl LevelSet {
    /// Every severity.
    pub const ALL: LevelSet = LevelSet(0b0111_1111);

    /// The set as its bits, by [`level_index`].
    pub(crate) const fn mask(self) -> u8 {
        self.0
    }

    /// This set plus `severity`.
    pub fn with(self, severity: Severity) -> LevelSet {
        LevelSet(self.0 | bit(severity))
    }

    /// Whether `severity` is in the set.
    pub fn contains(self, severity: Severity) -> bool {
        self.0 & bit(severity) != 0
    }

    /// The union of the groups that are on:
    ///
    /// | group | severities |
    /// |---|---|
    /// | `err` | error, fatal |
    /// | `wrn` | warn |
    /// | `inf` | info, unspecified |
    /// | `dbg` | debug, trace |
    pub fn from_groups(err: bool, wrn: bool, inf: bool, dbg: bool) -> LevelSet {
        let mut set = LevelSet::default();
        let groups: [(bool, &[Severity]); 4] = [
            (err, &[Severity::Error, Severity::Fatal]),
            (wrn, &[Severity::Warn]),
            (inf, &[Severity::Info, Severity::Unspecified]),
            (dbg, &[Severity::Debug, Severity::Trace]),
        ];
        for (on, severities) in groups {
            if on {
                for &severity in severities {
                    set = set.with(severity);
                }
            }
        }
        set
    }
}

/// The place of `severity` among the seven levels, lowest first: the order of
/// `Severity`, and the number of its bit in a [`LevelSet`]. The match has no
/// wildcard, so a new severity cannot be added without a bit (and a place in
/// [`LevelSet::ALL`]).
pub(crate) const fn level_index(severity: Severity) -> u8 {
    match severity {
        Severity::Unspecified => 0,
        Severity::Trace => 1,
        Severity::Debug => 2,
        Severity::Info => 3,
        Severity::Warn => 4,
        Severity::Error => 5,
        Severity::Fatal => 6,
    }
}

const fn bit(severity: Severity) -> u8 {
    1 << level_index(severity)
}

/// Every level, as a mask.
pub(crate) const ALL_LEVELS: u8 = LevelSet::ALL.0;

/// The levels that count as errors in the histogram (error and fatal), as a
/// mask.
pub(crate) const ERROR_LEVELS: u8 = bit(Severity::Error) | bit(Severity::Fatal);

/// One bar of the log volume histogram.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HistogramBucket {
    /// Logs in the bucket.
    pub total: u64,
    /// Of those, the logs of severity error or fatal.
    pub errors: u64,
}

/// A log and what the store holds around it, from
/// [`Store::logs_around`](crate::Store::logs_around).
#[derive(Clone, Debug, PartialEq)]
pub struct Around {
    /// The lines, oldest first.
    pub lines: Vec<(LogId, LogRecord)>,
    /// The index in `lines` of the log asked about, or `None` if it is no
    /// longer held (then `lines` are the ones nearest to where it was).
    pub target: Option<usize>,
}

/// When a log happened: its timestamp, or its observed time if the exporter
/// left the timestamp unset (OTLP's 0).
pub(crate) fn log_time(record: &LogRecord) -> u64 {
    if record.time_unix_nano != 0 {
        record.time_unix_nano
    } else {
        record.observed_unix_nano
    }
}

pub(crate) fn span_link(record: &LogRecord) -> Option<(TraceId, SpanId)> {
    Some((record.trace_id?, record.span_id?))
}

/// The id right after `id` in the order of the store (it saturates at the last
/// id there is).
fn successor(id: LogId) -> LogId {
    if id.seq < u64::MAX {
        LogId {
            time_unix_nano: id.time_unix_nano,
            seq: id.seq + 1,
        }
    } else if id.time_unix_nano < u64::MAX {
        LogId {
            time_unix_nano: id.time_unix_nano + 1,
            seq: 0,
        }
    } else {
        id
    }
}

/// What the metadata of a chunk can rule out for a filter: no log at a level
/// the filter keeps, none from the service it names (or that its query
/// selects), none of the trace it asks for. A chunk it keeps may still have no
/// match.
struct Prune {
    /// The levels the filter keeps (`levels` and `min_severity`), as a mask.
    levels: u8,
    /// The services every log must be from, by number.
    services: Vec<ServiceIdx>,
    /// The filter names a service nobody has logged from.
    ghost: bool,
    trace: Option<TraceId>,
}

impl Prune {
    fn new(filter: &LogFilter, services: &Services) -> Prune {
        let mut levels = filter.levels.map_or(ALL_LEVELS, LevelSet::mask);
        if let Some(min) = filter.min_severity {
            levels &= (ALL_LEVELS >> level_index(min)) << level_index(min);
        }
        let hint = filter.query.as_deref().and_then(LogQuery::service_hint);
        let mut wanted = Vec::new();
        let mut ghost = false;
        for name in filter.service.as_deref().into_iter().chain(hint) {
            match services.get(name) {
                Some(index) => wanted.push(index),
                None => ghost = true,
            }
        }
        Prune {
            levels,
            services: wanted,
            ghost,
            trace: filter.trace_id,
        }
    }

    fn may_match(&self, chunk: ChunkRef<'_>) -> bool {
        if self.ghost {
            return false;
        }
        let meta = chunk.meta();
        meta.present_levels() & self.levels != 0
            && self
                .services
                .iter()
                .all(|service| meta.has_service(*service))
            && self
                .trace
                .as_ref()
                .is_none_or(|trace| chunk.may_have_trace(trace))
    }
}

impl LogStore {
    /// The time of the oldest held log, `None` when there is none.
    pub(crate) fn oldest_time(&self) -> Option<u64> {
        self.chunk_refs()
            .map(|chunk| chunk.meta().first.time_unix_nano)
            .min()
    }

    /// Calls `f` on every held log whose time is at or after `since`, oldest
    /// first. The chunks it has to read are not remembered: a scan reads them
    /// all, once.
    pub(crate) fn scan(
        &self,
        since: u64,
        segments: Option<&SegmentSet>,
        mut f: impl FnMut(&LogRecord),
    ) {
        let from = LogId {
            time_unix_nano: since,
            seq: 0,
        };
        let mut cursor = Cursor::new(
            self,
            segments,
            false,
            Direction::Oldest,
            Some(from),
            None,
            |_| true,
        );
        while let Some((_, record)) = cursor.next() {
            f(record);
        }
    }

    /// The logs matching `filter`, newest first, one page at a time. The
    /// cursor is `"{time}-{arrival}"` of the last item: a keyset, so a page
    /// boundary holds still while newer logs arrive and older ones are
    /// evicted. A cursor that was not produced here gives an empty page.
    pub(crate) fn search(
        &self,
        filter: &LogFilter,
        page: &PageRequest,
        segments: Option<&SegmentSet>,
    ) -> Page<(LogId, LogRecord)> {
        let cursor = match page.cursor.as_deref() {
            None => None,
            Some(text) => match decode_cursor(text) {
                Some(key) => Some(key),
                None => return Page::empty(),
            },
        };
        if page.limit == 0 {
            return Page::empty();
        }
        // Both `until` and the cursor say "strictly older than": the page
        // starts below whichever is older.
        let (lo, until) = filter.window();
        let hi = match (cursor, until) {
            (None, None) => None,
            (Some(key), None) | (None, Some(key)) => Some(key),
            (Some(a), Some(b)) => Some(a.min(b)),
        };
        let matcher = Matcher::new(filter);
        let prune = Prune::new(filter, &self.services);
        let mut walk = Cursor::new(self, segments, true, Direction::Newest, lo, hi, |chunk| {
            prune.may_match(chunk)
        });

        let mut items: Vec<(LogId, LogRecord)> = Vec::new();
        let mut last_key = LogId {
            time_unix_nano: 0,
            seq: 0,
        };
        let mut next_cursor = None;
        while let Some((key, record)) = walk.next() {
            if !matcher.matches(record) {
                continue;
            }
            if items.len() == page.limit {
                // One more match exists, so there is a next page.
                next_cursor = Some(encode_cursor(last_key));
                break;
            }
            items.push((key, record.clone()));
            last_key = key;
        }
        Page { items, next_cursor }
    }

    /// How many held logs `filter` keeps, for an indexed filter (`None`
    /// otherwise). A chunk that lies inside the window is counted from its
    /// metadata; one that a window edge cuts through is read.
    pub(crate) fn count(&self, filter: &LogFilter, segments: Option<&SegmentSet>) -> Option<u64> {
        if !filter.is_indexed() {
            return None;
        }
        let (lo, hi) = filter.window();
        let selector = Selector::new(filter.levels, filter.service.as_deref(), &self.services);
        Some(self.count_in(&selector, lo, hi, segments))
    }

    /// How many held logs with `lo <= id < hi` the selector passes.
    fn count_in(
        &self,
        selector: &Selector<'_>,
        lo: Option<LogId>,
        hi: Option<LogId>,
        segments: Option<&SegmentSet>,
    ) -> u64 {
        if selector.is_empty() || matches!((lo, hi), (Some(lo), Some(hi)) if lo >= hi) {
            return 0;
        }
        self.chunk_refs()
            .map(|chunk| self.count_chunk(chunk, selector, lo, hi, segments))
            .sum()
    }

    fn count_chunk(
        &self,
        chunk: ChunkRef<'_>,
        selector: &Selector<'_>,
        lo: Option<LogId>,
        hi: Option<LogId>,
        segments: Option<&SegmentSet>,
    ) -> u64 {
        let meta = chunk.meta();
        if lo.is_some_and(|lo| meta.last < lo) || hi.is_some_and(|hi| meta.first >= hi) {
            return 0;
        }
        let inside = lo.is_none_or(|lo| meta.first >= lo) && hi.is_none_or(|hi| meta.last < hi);
        if inside {
            return meta.count_matching(selector);
        }
        // The window cuts through the chunk: count what is in it.
        let mut count = 0;
        self.for_each_row(chunk, lo, hi, segments, true, |_, record| {
            if selector.matches(record) {
                count += 1;
            }
        });
        count
    }

    /// Up to `limit` of the logs matching `filter` from position `offset` in
    /// the list newest first (position 0 is the newest log that matches).
    ///
    /// For an indexed filter the position is found without reading logs the way
    /// a walk would: the counts of the chunks say how many matching logs are
    /// newer than a given id, and a search on that finds the chunk the position
    /// is in (the ones that overlap it, when chunks overlap in time), and only
    /// those are read. Any other filter walks the list from the newest log and
    /// counts, which costs the logs before `offset`.
    pub(crate) fn at(
        &self,
        filter: &LogFilter,
        offset: u64,
        limit: usize,
        segments: Option<&SegmentSet>,
    ) -> Vec<(LogId, LogRecord)> {
        let (lo, hi) = filter.window();
        if limit == 0 || matches!((lo, hi), (Some(lo), Some(hi)) if lo >= hi) {
            return Vec::new();
        }
        let prune = Prune::new(filter, &self.services);
        let mut rows = Vec::new();

        if !filter.is_indexed() {
            let matcher = Matcher::new(filter);
            let mut walk = Cursor::new(self, segments, true, Direction::Newest, lo, hi, |chunk| {
                prune.may_match(chunk)
            });
            let mut skip = offset;
            while let Some((id, record)) = walk.next() {
                if !matcher.matches(record) {
                    continue;
                }
                if skip > 0 {
                    skip -= 1;
                    continue;
                }
                rows.push((id, record.clone()));
                if rows.len() == limit {
                    break;
                }
            }
            return rows;
        }

        let selector = Selector::new(filter.levels, filter.service.as_deref(), &self.services);
        if selector.is_empty() {
            return rows;
        }
        // Where to start walking, and how many matching logs to step over from
        // there: `upper` is an id with at most `offset` matching logs at or above
        // it, so the walk starts just below it.
        let (upper, skip) = if offset == 0 {
            (None, 0)
        } else {
            // The ids at which the answer to "how many matching logs are at or
            // above this id" can change: the first id of each chunk, and the one
            // after its last. Between two of them, no chunk starts or ends.
            let mut bounds: Vec<LogId> = Vec::new();
            for chunk in self.chunk_refs() {
                let meta = chunk.meta();
                bounds.push(meta.first);
                bounds.push(successor(meta.last));
            }
            bounds.sort_unstable_by(|a, b| b.cmp(a));
            bounds.dedup();
            let at_or_above = |bound: LogId| {
                let from = lo.map_or(bound, |lo| lo.max(bound));
                self.count_in(&selector, Some(from), hi, segments)
            };
            // The first bound with more than `offset` matching logs at or above
            // it, going from the newest down: the position is below the one
            // before it. (At the oldest bound, all of them are at or above.)
            let found = bounds.partition_point(|bound| at_or_above(*bound) <= offset);
            match found {
                0 => (None, offset),
                found if found >= bounds.len() => return rows,
                found => {
                    let upper = bounds[found - 1];
                    (Some(upper), offset - at_or_above(upper))
                }
            }
        };
        let hi = match (upper, hi) {
            (Some(upper), Some(hi)) => Some(upper.min(hi)),
            (upper, hi) => upper.or(hi),
        };
        let mut walk = Cursor::new(self, segments, true, Direction::Newest, lo, hi, |chunk| {
            prune.may_match(chunk)
        });
        let mut skip = skip;
        while let Some((id, record)) = walk.next() {
            if !selector.matches(record) {
                continue;
            }
            if skip > 0 {
                skip -= 1;
                continue;
            }
            rows.push((id, record.clone()));
            if rows.len() == limit {
                break;
            }
        }
        rows
    }

    /// The logs in `[since, until)` that match `filter` (its own time fields
    /// ignored), counted into `buckets` buckets of equal width, oldest first.
    /// Always `buckets` long; an empty or backwards window counts nothing.
    ///
    /// For an indexed filter, a chunk that lies inside the window and inside one
    /// bucket is counted from its metadata, and so is one that spans buckets if
    /// every second in it that has logs lies in one bucket (the counts are per
    /// second). Any other chunk is read and its logs counted one by one, so the
    /// answer is always exactly what a scan gives. The chunks that are read are
    /// the ones a bucket edge falls inside a second of, and the ones at the edges
    /// of the window.
    pub(crate) fn histogram(
        &self,
        filter: &LogFilter,
        since: u64,
        until: u64,
        buckets: usize,
        segments: Option<&SegmentSet>,
    ) -> Vec<HistogramBucket> {
        let mut histogram = vec![HistogramBucket::default(); buckets];
        if buckets == 0 || until <= since {
            return histogram;
        }
        // In 128 bits so that a window of years times the bucket count
        // cannot overflow.
        let width = u128::from(until - since);
        let bucket_of = |time: u64| {
            let offset = u128::from(time - since);
            // `offset < width` for a time in the window, so the index is below
            // `buckets`.
            usize::try_from(offset * buckets as u128 / width)
                .map_or(buckets - 1, |index| index.min(buckets - 1))
        };
        let lo = LogId {
            time_unix_nano: since,
            seq: 0,
        };
        let hi = LogId {
            time_unix_nano: until,
            seq: 0,
        };
        let matcher = Matcher::new(filter);
        let prune = Prune::new(filter, &self.services);
        let selector = Selector::new(filter.levels, filter.service.as_deref(), &self.services);
        let indexed = filter.is_indexed();

        for chunk in self.chunk_refs() {
            let meta = chunk.meta();
            if meta.last < lo || meta.first >= hi || !prune.may_match(chunk) {
                continue;
            }
            if indexed && meta.first >= lo && meta.last < hi {
                // The whole chunk is in the window.
                let (first, last) = (
                    bucket_of(meta.first.time_unix_nano),
                    bucket_of(meta.last.time_unix_nano),
                );
                if first == last {
                    histogram[first].total += meta.count_matching(&selector);
                    histogram[first].errors += meta.count_matching(&selector.errors());
                    continue;
                }
                if add_cells(meta, &selector, &bucket_of, &mut histogram) {
                    continue;
                }
            }
            self.for_each_row(chunk, Some(lo), Some(hi), segments, true, |id, record| {
                if !matcher.matches(record) {
                    return;
                }
                let bucket = &mut histogram[bucket_of(id.time_unix_nano)];
                bucket.total += 1;
                if record.severity >= Severity::Error {
                    bucket.errors += 1;
                }
            });
        }
        histogram
    }

    /// Up to `before` logs older than `id`, the log itself if it is held,
    /// and up to `after` logs newer than it, oldest first. If `id` is not
    /// held, the logs on either side of where it was.
    pub(crate) fn around(
        &self,
        id: LogId,
        before: usize,
        after: usize,
        segments: Option<&SegmentSet>,
    ) -> Around {
        let mut lines: Vec<(LogId, LogRecord)> = Vec::new();
        let mut older = Cursor::new(
            self,
            segments,
            true,
            Direction::Newest,
            None,
            Some(id),
            |_| true,
        );
        while lines.len() < before {
            let Some((key, record)) = older.next() else {
                break;
            };
            lines.push((key, record.clone()));
        }
        lines.reverse();
        let target = self.get(id, segments).map(|record| {
            lines.push((id, record));
            lines.len() - 1
        });
        let mut newer = Cursor::new(
            self,
            segments,
            true,
            Direction::Oldest,
            Some(successor(id)),
            None,
            |_| true,
        );
        for _ in 0..after {
            let Some((key, record)) = newer.next() else {
                break;
            };
            lines.push((key, record.clone()));
        }
        Around { lines, target }
    }
}

/// Adds the cells of the chunk to the buckets, if each of them (the logs of one
/// second, from one service, at one level) can only fall in one bucket, which
/// is when the second's logs, which the chunk bounds to its first and last, all
/// land in the same one. Says whether it did; if not, it has added nothing.
fn add_cells(
    meta: &crate::log_chunks::ChunkMeta,
    selector: &Selector<'_>,
    bucket_of: &impl Fn(u64) -> usize,
    histogram: &mut [HistogramBucket],
) -> bool {
    let (first, last) = (meta.first.time_unix_nano, meta.last.time_unix_nano);
    let bucket_of_cell = |second: u64| {
        let start = second.saturating_mul(1_000_000_000).max(first);
        let end = second
            .saturating_mul(1_000_000_000)
            .saturating_add(999_999_999)
            .min(last);
        let (a, b) = (bucket_of(start), bucket_of(end));
        (a == b).then_some(a)
    };
    let wanted =
        |cell: &&crate::log_chunks::Cell| selector.matches_cell(cell.key.service, cell.key.level);
    if meta
        .cells
        .iter()
        .filter(wanted)
        .any(|cell| bucket_of_cell(cell.key.second).is_none())
    {
        return false;
    }
    for cell in meta.cells.iter().filter(wanted) {
        let Some(index) = bucket_of_cell(cell.key.second) else {
            continue;
        };
        histogram[index].total += u64::from(cell.count);
        if cell.key.level >= level_index(Severity::Error) {
            histogram[index].errors += u64::from(cell.count);
        }
    }
    true
}

/// The conditions of a [`LogFilter`] that look at one log, with the work
/// that can be done once (lowercasing the text, finding the query's service)
/// done up front.
struct Matcher<'a> {
    filter: &'a LogFilter,
    /// `filter.contains`, lowercased.
    needle: Option<String>,
    /// The service the query is known to match (its exact `service_name`
    /// selector, if any), a cheap test before the query itself.
    hint: Option<&'a str>,
}

impl<'a> Matcher<'a> {
    fn new(filter: &'a LogFilter) -> Matcher<'a> {
        Matcher {
            filter,
            needle: filter.contains.as_deref().map(str::to_lowercase),
            hint: filter.query.as_deref().and_then(LogQuery::service_hint),
        }
    }

    /// Whether `record` passes every condition of the filter except the
    /// time window (`since` and `until`), which callers apply by time order.
    fn matches(&self, record: &LogRecord) -> bool {
        let filter = self.filter;
        filter
            .levels
            .is_none_or(|levels| levels.contains(record.severity))
            && filter.min_severity.is_none_or(|min| record.severity >= min)
            && filter
                .trace_id
                .is_none_or(|trace| record.trace_id == Some(trace))
            && filter
                .span_id
                .is_none_or(|span| record.span_id == Some(span))
            && filter
                .service
                .as_deref()
                .is_none_or(|service| record.service() == service)
            && self
                .needle
                .as_deref()
                .is_none_or(|needle| contains_ignore_case(&record.body, needle))
            && self.hint.is_none_or(|service| record.service() == service)
            && filter
                .query
                .as_deref()
                .is_none_or(|query| query.matches(record))
    }
}

/// Whether `text` contains `needle_lower` (already lowercased), ignoring
/// case. Plain ASCII, the usual log line, is compared in place; anything else
/// is lowercased first.
fn contains_ignore_case(text: &str, needle_lower: &str) -> bool {
    if needle_lower.is_empty() {
        return true;
    }
    if text.is_ascii() && needle_lower.is_ascii() {
        let needle = needle_lower.as_bytes();
        text.as_bytes()
            .windows(needle.len())
            .any(|window| window.eq_ignore_ascii_case(needle))
    } else {
        text.to_lowercase().contains(needle_lower)
    }
}

fn encode_cursor(key: LogId) -> String {
    format!("{}-{}", key.time_unix_nano, key.seq)
}

/// Reads a cursor made by [`encode_cursor`]. `None` for anything else.
fn decode_cursor(cursor: &str) -> Option<LogId> {
    let (time, arrival) = cursor.split_once('-')?;
    // `parse` alone would also take a leading `+`, and the empty string is
    // not a number either way.
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    if !digits(time) || !digits(arrival) {
        return None;
    }
    Some(LogId {
        time_unix_nano: time.parse().ok()?,
        seq: arrival.parse().ok()?,
    })
}
