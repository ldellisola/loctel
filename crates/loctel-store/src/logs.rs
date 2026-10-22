//! Log storage: time order, dedup, eviction, search and the link to spans.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque, hash_map::Entry};
use std::ops::Bound;
use std::sync::Arc;

use loctel_model::{LogRecord, Severity, SpanId, TraceId};
use loctel_query::logql::LogQuery;

use crate::page::{Page, PageRequest};

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

/// The bit of `severity` in a [`LevelSet`]. The match has no wildcard, so a
/// new severity cannot be added without a bit (and a place in
/// [`LevelSet::ALL`]).
fn bit(severity: Severity) -> u8 {
    1 << match severity {
        Severity::Unspecified => 0,
        Severity::Trace => 1,
        Severity::Debug => 2,
        Severity::Info => 3,
        Severity::Warn => 4,
        Severity::Error => 5,
        Severity::Fatal => 6,
    }
}

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

struct Stored {
    record: LogRecord,
    /// `record.dedup_key()`, kept so eviction can forget it without hashing
    /// again.
    dedup: u64,
}

/// The logs the store holds.
#[derive(Default)]
pub(crate) struct LogStore {
    /// In time order (oldest first): the order of eviction and the reverse of
    /// the order of search.
    records: BTreeMap<LogId, Stored>,
    /// The dedup keys of the held logs.
    held: HashSet<u64>,
    /// The held logs of each span, oldest first.
    by_span: HashMap<(TraceId, SpanId), VecDeque<LogId>>,
    /// The held logs that carry a trace ID (with or without a span ID) by
    /// trace, oldest first. Lets a whole trace's logs be found and removed
    /// without a pass over every log.
    by_trace: HashMap<TraceId, VecDeque<LogId>>,
    /// Arrival counter for `LogId`. Never reset, so a cursor taken before a
    /// `clear` cannot point at a later log.
    next_seq: u64,
}

/// When a log happened: its timestamp, or its observed time if the exporter
/// left the timestamp unset (OTLP's 0).
fn log_time(record: &LogRecord) -> u64 {
    if record.time_unix_nano != 0 {
        record.time_unix_nano
    } else {
        record.observed_unix_nano
    }
}

fn span_link(record: &LogRecord) -> Option<(TraceId, SpanId)> {
    Some((record.trace_id?, record.span_id?))
}

impl LogStore {
    pub(crate) fn len(&self) -> usize {
        self.records.len()
    }

    /// Stores `record` in time order unless an identical log is already held
    /// (an exporter retry). Returns whether it was stored.
    pub(crate) fn insert(&mut self, record: LogRecord) -> bool {
        let dedup = record.dedup_key();
        if !self.held.insert(dedup) {
            return false;
        }
        let key = LogId {
            time_unix_nano: log_time(&record),
            seq: self.next_seq,
        };
        self.next_seq += 1;
        if let Some(link) = span_link(&record) {
            insert_in_order(self.by_span.entry(link).or_default(), key);
        }
        if let Some(trace) = record.trace_id {
            insert_in_order(self.by_trace.entry(trace).or_default(), key);
        }
        self.records.insert(key, Stored { record, dedup });
        true
    }

    /// Drops the oldest logs until at most `max` remain.
    pub(crate) fn evict_beyond(&mut self, max: usize) {
        while self.records.len() > max {
            let Some((key, stored)) = self.records.pop_first() else {
                break;
            };
            self.held.remove(&stored.dedup);
            // The oldest log overall is the oldest of its span and of its trace.
            if let Some(link) = span_link(&stored.record) {
                pop_oldest(&mut self.by_span, link, key);
            }
            if let Some(trace) = stored.record.trace_id {
                pop_oldest(&mut self.by_trace, trace, key);
            }
        }
    }

    /// Forgets every log. The arrival counter keeps counting.
    pub(crate) fn clear(&mut self) {
        self.records.clear();
        self.held.clear();
        self.by_span.clear();
        self.by_trace.clear();
    }

    /// Removes every held log that carries `trace` as its trace ID, with or
    /// without a span ID, and returns how many there were. Costs the number of
    /// logs the trace has.
    pub(crate) fn remove_trace(&mut self, trace: TraceId) -> usize {
        let Some(keys) = self.by_trace.remove(&trace) else {
            return 0;
        };
        let mut removed = 0;
        for key in keys {
            if let Some(stored) = self.records.remove(&key) {
                self.held.remove(&stored.dedup);
                // Every log of the span is in this trace, so the span's whole
                // index entry goes (more than once, harmlessly).
                if let Some(link) = span_link(&stored.record) {
                    self.by_span.remove(&link);
                }
                removed += 1;
            }
        }
        removed
    }

    /// Removes the held logs of the given spans of `trace` (whether or not the
    /// spans themselves are held) and returns how many there were. A log with
    /// the trace's ID but no span ID belongs to no span and stays. Costs the
    /// number of logs the spans have, plus those of the trace as a whole.
    pub(crate) fn remove_spans(&mut self, trace: TraceId, spans: &[SpanId]) -> usize {
        let mut removed: HashSet<LogId> = HashSet::new();
        for &span in spans {
            // Taking the entry makes a span listed twice count once.
            let Some(keys) = self.by_span.remove(&(trace, span)) else {
                continue;
            };
            for key in keys {
                if let Some(stored) = self.records.remove(&key) {
                    self.held.remove(&stored.dedup);
                    removed.insert(key);
                }
            }
        }
        if removed.is_empty() {
            return 0;
        }
        if let Entry::Occupied(mut keys) = self.by_trace.entry(trace) {
            keys.get_mut().retain(|key| !removed.contains(key));
            if keys.get().is_empty() {
                keys.remove();
            }
        }
        removed.len()
    }

    /// Calls `f` on every held log whose time is at or after `since`, oldest
    /// first.
    pub(crate) fn scan(&self, since: u64, mut f: impl FnMut(&LogRecord)) {
        let from = LogId {
            time_unix_nano: since,
            seq: 0,
        };
        for (_, stored) in self.records.range(from..) {
            f(&stored.record);
        }
    }

    /// The logs of one span, oldest first.
    pub(crate) fn for_span(&self, trace: TraceId, span: SpanId) -> Vec<LogRecord> {
        self.by_span
            .get(&(trace, span))
            .into_iter()
            .flatten()
            .filter_map(|key| self.records.get(key))
            .map(|stored| stored.record.clone())
            .collect()
    }

    /// The log held under `id`.
    pub(crate) fn get(&self, id: LogId) -> Option<LogRecord> {
        self.records.get(&id).map(|stored| stored.record.clone())
    }

    /// The logs matching `filter`, newest first, one page at a time. The
    /// cursor is `"{time}-{arrival}"` of the last item: a keyset, so a page
    /// boundary holds still while newer logs arrive and older ones are
    /// evicted. A cursor that was not produced here gives an empty page.
    pub(crate) fn search(
        &self,
        filter: &LogFilter,
        page: &PageRequest,
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
        // starts below whichever is older. A log at `until` itself has
        // arrival number 0 or more, so it sits at or above `(until, 0)`.
        let until = filter.until_unix_nano.map(|until| LogId {
            time_unix_nano: until,
            seq: 0,
        });
        let before = match (cursor, until) {
            (None, None) => Bound::Unbounded,
            (Some(key), None) | (None, Some(key)) => Bound::Excluded(key),
            (Some(a), Some(b)) => Bound::Excluded(a.min(b)),
        };
        let matcher = Matcher::new(filter);

        let mut items: Vec<(LogId, LogRecord)> = Vec::new();
        let mut last_key = LogId {
            time_unix_nano: 0,
            seq: 0,
        };
        let mut next_cursor = None;
        for (&key, stored) in self.records.range((Bound::Unbounded, before)).rev() {
            // Newest first, so once a log is too old, all the rest are.
            if filter
                .since_unix_nano
                .is_some_and(|since| key.time_unix_nano < since)
            {
                break;
            }
            if !matcher.matches(&stored.record) {
                continue;
            }
            if items.len() == page.limit {
                // One more match exists, so there is a next page.
                next_cursor = Some(encode_cursor(last_key));
                break;
            }
            items.push((key, stored.record.clone()));
            last_key = key;
        }
        Page { items, next_cursor }
    }

    /// The logs in `[since, until)` that match `filter` (its own time fields
    /// ignored), counted into `buckets` buckets of equal width, oldest
    /// first. Always `buckets` long; an empty or backwards window counts
    /// nothing. Costs the logs inside the window.
    pub(crate) fn histogram(
        &self,
        filter: &LogFilter,
        since: u64,
        until: u64,
        buckets: usize,
    ) -> Vec<HistogramBucket> {
        let mut histogram = vec![HistogramBucket::default(); buckets];
        if buckets == 0 || until <= since {
            return histogram;
        }
        let matcher = Matcher::new(filter);
        let from = LogId {
            time_unix_nano: since,
            seq: 0,
        };
        let to = LogId {
            time_unix_nano: until,
            seq: 0,
        };
        // In 128 bits so that a window of years times the bucket count
        // cannot overflow.
        let width = u128::from(until - since);
        for (key, stored) in self.records.range(from..to) {
            if !matcher.matches(&stored.record) {
                continue;
            }
            let offset = u128::from(key.time_unix_nano - since);
            // `offset < width`, so the index is below `buckets`.
            let index = usize::try_from(offset * buckets as u128 / width)
                .map_or(buckets - 1, |index| index.min(buckets - 1));
            let bucket = &mut histogram[index];
            bucket.total += 1;
            if stored.record.severity >= Severity::Error {
                bucket.errors += 1;
            }
        }
        histogram
    }

    /// Up to `before` logs older than `id`, the log itself if it is held,
    /// and up to `after` logs newer than it, oldest first. If `id` is not
    /// held, the logs on either side of where it was.
    pub(crate) fn around(&self, id: LogId, before: usize, after: usize) -> Around {
        let entry = |(&key, stored): (&LogId, &Stored)| (key, stored.record.clone());
        let mut lines: Vec<(LogId, LogRecord)> = self
            .records
            .range(..id)
            .rev()
            .take(before)
            .map(entry)
            .collect();
        lines.reverse();
        let target = self.records.get(&id).map(|stored| {
            lines.push((id, stored.record.clone()));
            lines.len() - 1
        });
        lines.extend(
            self.records
                .range((Bound::Excluded(id), Bound::Unbounded))
                .take(after)
                .map(entry),
        );
        Around { lines, target }
    }
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
