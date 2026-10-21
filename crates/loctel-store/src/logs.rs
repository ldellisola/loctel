//! Log storage: time order, dedup, eviction, search and the link to spans.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque, hash_map::Entry};
use std::ops::Bound;

use loctel_model::{LogRecord, Severity, SpanId, TraceId};

use crate::page::{Page, PageRequest};

/// What [`Store::search_logs`](crate::Store::search_logs) keeps. Every
/// condition that is set must hold; the default keeps everything.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LogFilter {
    /// Keep logs at or after this time (the log's own timestamp, or its
    /// observed time when it has none).
    pub since_unix_nano: Option<u64>,
    /// Keep logs from this service (exact name; `unknown_service` for logs
    /// without one).
    pub service: Option<String>,
    /// Keep logs of this severity or higher. `Unspecified` is the lowest
    /// level, so any `Some(level)` above it drops unlabelled logs.
    pub min_severity: Option<Severity>,
    /// Keep logs whose body contains this text, ignoring case. Only the body
    /// is searched. The empty text matches every log.
    pub contains: Option<String>,
    /// Keep logs emitted in this trace.
    pub trace_id: Option<TraceId>,
    /// Keep logs emitted in a span with this ID (in any trace, unless
    /// `trace_id` is set too).
    pub span_id: Option<SpanId>,
}

/// Where a held log sits: when it happened, then when it arrived. Unique
/// and stable for as long as the log is held, which a position in a vector
/// would not be (late logs insert in the middle, eviction removes from the
/// front).
type LogKey = (u64, u64);

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
    records: BTreeMap<LogKey, Stored>,
    /// The dedup keys of the held logs.
    held: HashSet<u64>,
    /// The held logs of each span, oldest first.
    by_span: HashMap<(TraceId, SpanId), VecDeque<LogKey>>,
    /// Arrival counter for `LogKey`. Never reset, so a cursor taken before a
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
        let key = (log_time(&record), self.next_seq);
        self.next_seq += 1;
        if let Some(link) = span_link(&record) {
            let keys = self.by_span.entry(link).or_default();
            // Almost always the end; a late log lands further in.
            let at = keys.partition_point(|held| *held < key);
            keys.insert(at, key);
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
            if let Some(link) = span_link(&stored.record)
                && let Entry::Occupied(mut keys) = self.by_span.entry(link)
            {
                // The oldest log overall is the oldest of its span.
                let oldest = keys.get_mut().pop_front();
                debug_assert_eq!(oldest, Some(key));
                if keys.get().is_empty() {
                    keys.remove();
                }
            }
        }
    }

    /// Forgets every log. The arrival counter keeps counting.
    pub(crate) fn clear(&mut self) {
        self.records.clear();
        self.held.clear();
        self.by_span.clear();
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

    /// The logs matching `filter`, newest first, one page at a time. The
    /// cursor is `"{time}-{arrival}"` of the last item: a keyset, so a page
    /// boundary holds still while newer logs arrive and older ones are
    /// evicted. A cursor that was not produced here gives an empty page.
    pub(crate) fn search(&self, filter: &LogFilter, page: &PageRequest) -> Page<LogRecord> {
        let before = match page.cursor.as_deref() {
            None => Bound::Unbounded,
            Some(text) => match decode_cursor(text) {
                Some(key) => Bound::Excluded(key),
                None => return Page::empty(),
            },
        };
        if page.limit == 0 {
            return Page::empty();
        }
        let needle = filter.contains.as_deref().map(str::to_lowercase);

        let mut items: Vec<LogRecord> = Vec::new();
        let mut last_key = (0, 0);
        let mut next_cursor = None;
        for (&key, stored) in self.records.range((Bound::Unbounded, before)).rev() {
            // Newest first, so once a log is too old, all the rest are.
            if filter.since_unix_nano.is_some_and(|since| key.0 < since) {
                break;
            }
            if !matches(filter, needle.as_deref(), &stored.record) {
                continue;
            }
            if items.len() == page.limit {
                // One more match exists, so there is a next page.
                next_cursor = Some(encode_cursor(last_key));
                break;
            }
            items.push(stored.record.clone());
            last_key = key;
        }
        Page { items, next_cursor }
    }
}

/// Whether `record` passes every condition of `filter` except `since`, which
/// the scan applies by time order. `needle` is `filter.contains`, lowercased.
fn matches(filter: &LogFilter, needle: Option<&str>, record: &LogRecord) -> bool {
    filter.min_severity.is_none_or(|min| record.severity >= min)
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
        && needle.is_none_or(|needle| contains_ignore_case(&record.body, needle))
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

fn encode_cursor((time, arrival): LogKey) -> String {
    format!("{time}-{arrival}")
}

/// Reads a cursor made by [`encode_cursor`]. `None` for anything else.
fn decode_cursor(cursor: &str) -> Option<LogKey> {
    let (time, arrival) = cursor.split_once('-')?;
    // `parse` alone would also take a leading `+`, and the empty string is
    // not a number either way.
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    if !digits(time) || !digits(arrival) {
        return None;
    }
    Some((time.parse().ok()?, arrival.parse().ok()?))
}
