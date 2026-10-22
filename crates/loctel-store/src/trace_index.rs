//! The trace index: what the store knows about every trace it holds, in memory,
//! whether the trace's spans are in memory (hot) or in a segment file (cold).
//!
//! Every held trace has a [`TraceMeta`]. The index orders them three ways:
//!
//! - **by start**, newest first, which is how traces are listed and searched:
//!   a `BTreeSet<(start, TraceId)>` walked backwards from a cursor, so a page
//!   of results costs the traces looked at, not the traces held;
//! - **by first arrival**, oldest first, which is the order of eviction (and of
//!   `scan_traces`);
//! - **hot traces by when their last new span arrived**, oldest first, which is
//!   the order in which the spill step takes them.
//!
//! A cold trace's meta also carries the [`TraceSummary`] it had when it left
//! memory (its services, status, failed-in span, first exception, root), so
//! listing and filtering cold traces needs no decoding. Nothing about a cold
//! trace can change, because a new span brings it back to memory first.
//!
//! The hot traces' spans are in the store's own map. For a hot trace the meta
//! keeps only what the keys above need (start, end, count, last arrival), kept
//! in step by [`TraceIndex::sync_hot`].
//!
//! The decoded traces of recent cold reads are in the [`ColdCache`].

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::ops::Bound;
use std::sync::Arc;

use loctel_model::TraceId;

use crate::segment;
use crate::summary::TraceSummary;
use crate::trace::TraceEntry;

/// Decoded cold traces kept for the next read: the trace the window has open is
/// read again every half second.
const CACHE_TRACES: usize = 64;

/// ... but never more than this many spans in all, so that a few huge traces
/// cannot fill memory behind a "small" cache.
const CACHE_MAX_SPANS: usize = 1_000_000;

/// Where a trace's spans are.
pub(crate) enum Location {
    /// In the store's map of hot traces.
    Hot,
    /// In a segment file.
    Cold(Box<Cold>),
}

/// What the store keeps of a trace that is in a segment file.
pub(crate) struct Cold {
    /// Where its record is.
    pub(crate) blob: segment::Location,
    /// The trace in one row as it was when it left memory: the service set (in
    /// the order of their first span), status, failed-in span and first
    /// exception, and the root. `complete` is a placeholder: it depends on the
    /// clock, so it is worked out from [`TraceMeta::last_arrival`] when read.
    pub(crate) summary: TraceSummary,
}

pub(crate) struct TraceMeta {
    pub(crate) id: TraceId,
    /// Where the trace sits in the order of first arrival. Never changes.
    pub(crate) arrival: u64,
    /// The earliest span start.
    pub(crate) start: u64,
    /// The latest span end (no earlier than its own span's start).
    pub(crate) end: u64,
    pub(crate) span_count: usize,
    /// Clock time at which the last new span arrived.
    pub(crate) last_arrival: u64,
    pub(crate) location: Location,
}

impl TraceMeta {
    /// From the earliest start to the latest end.
    pub(crate) fn duration_nanos(&self) -> u64 {
        self.end.saturating_sub(self.start)
    }

    /// Nanoseconds since the last new span arrived (0 if the clock stepped
    /// backwards).
    pub(crate) fn quiet_for(&self, now: u64) -> u64 {
        now.saturating_sub(self.last_arrival)
    }
}

#[derive(Default)]
pub(crate) struct TraceIndex {
    metas: HashMap<TraceId, TraceMeta>,
    by_start: BTreeSet<(u64, TraceId)>,
    by_arrival: BTreeMap<u64, TraceId>,
    hot_by_quiet: BTreeSet<(u64, TraceId)>,
    cold: usize,
}

impl TraceIndex {
    pub(crate) fn len(&self) -> usize {
        self.metas.len()
    }

    /// How many traces are in segment files.
    pub(crate) fn cold_len(&self) -> usize {
        self.cold
    }

    pub(crate) fn get(&self, id: &TraceId) -> Option<&TraceMeta> {
        self.metas.get(id)
    }

    pub(crate) fn is_cold(&self, id: &TraceId) -> bool {
        self.metas
            .get(id)
            .is_some_and(|meta| matches!(meta.location, Location::Cold(_)))
    }

    /// Every trace, in no order.
    pub(crate) fn iter(&self) -> impl Iterator<Item = &TraceMeta> {
        self.metas.values()
    }

    /// The traces by first arrival, oldest first.
    pub(crate) fn by_arrival(&self) -> impl Iterator<Item = &TraceId> {
        self.by_arrival.values()
    }

    /// The trace that arrived first.
    pub(crate) fn oldest_arrival(&self) -> Option<TraceId> {
        self.by_arrival.values().next().copied()
    }

    /// The earliest start of any trace held.
    pub(crate) fn oldest_start(&self) -> Option<u64> {
        self.by_start.first().map(|&(start, _)| start)
    }

    /// `(start, id)` of every trace, newest first: by start, a tie going to the
    /// higher trace ID. With `after`, only those strictly after that position
    /// (a search's cursor).
    pub(crate) fn newest_first(
        &self,
        after: Option<(u64, TraceId)>,
    ) -> impl Iterator<Item = (u64, TraceId)> + '_ {
        let upper = after.map_or(Bound::Unbounded, Bound::Excluded);
        self.by_start
            .range((Bound::Unbounded, upper))
            .rev()
            .copied()
    }

    /// The hot trace whose last new span arrived longest ago, with the time it
    /// did.
    pub(crate) fn oldest_hot(&self) -> Option<(u64, TraceId)> {
        self.hot_by_quiet.first().copied()
    }

    /// A trace that has just got its first span, and is hot.
    pub(crate) fn add_hot(&mut self, id: TraceId, entry: &TraceEntry) {
        let meta = TraceMeta {
            id,
            arrival: entry.arrival(),
            start: entry.start_unix_nano(),
            end: entry.end_unix_nano(),
            span_count: entry.len(),
            last_arrival: entry.last_arrival(),
            location: Location::Hot,
        };
        self.by_start.insert((meta.start, id));
        self.by_arrival.insert(meta.arrival, id);
        self.hot_by_quiet.insert((meta.last_arrival, id));
        self.metas.insert(id, meta);
    }

    /// The hot trace `id` changed (a span came or went): bring what the index
    /// keeps of it up to date. Does nothing for a trace that is not hot.
    pub(crate) fn sync_hot(&mut self, id: TraceId, entry: &TraceEntry) {
        let Some(meta) = self.metas.get_mut(&id) else {
            return;
        };
        if !matches!(meta.location, Location::Hot) {
            return;
        }
        if meta.start != entry.start_unix_nano() {
            self.by_start.remove(&(meta.start, id));
            meta.start = entry.start_unix_nano();
            self.by_start.insert((meta.start, id));
        }
        if meta.last_arrival != entry.last_arrival() {
            self.hot_by_quiet.remove(&(meta.last_arrival, id));
            meta.last_arrival = entry.last_arrival();
            self.hot_by_quiet.insert((meta.last_arrival, id));
        }
        meta.end = entry.end_unix_nano();
        meta.span_count = entry.len();
    }

    /// The hot trace `id` was written to a segment.
    pub(crate) fn make_cold(&mut self, id: TraceId, cold: Cold) {
        let Some(meta) = self.metas.get_mut(&id) else {
            return;
        };
        if matches!(meta.location, Location::Hot) {
            self.hot_by_quiet.remove(&(meta.last_arrival, id));
            meta.location = Location::Cold(Box::new(cold));
            self.cold += 1;
        }
    }

    /// The cold trace `id` is in memory again. Returns where it was.
    pub(crate) fn make_hot(&mut self, id: TraceId) -> Option<Cold> {
        let meta = self.metas.get_mut(&id)?;
        let Location::Cold(cold) = std::mem::replace(&mut meta.location, Location::Hot) else {
            return None;
        };
        self.hot_by_quiet.insert((meta.last_arrival, id));
        self.cold -= 1;
        Some(*cold)
    }

    /// Takes `id` out of the index.
    pub(crate) fn remove(&mut self, id: TraceId) -> Option<TraceMeta> {
        let meta = self.metas.remove(&id)?;
        self.by_start.remove(&(meta.start, id));
        self.by_arrival.remove(&meta.arrival);
        match meta.location {
            Location::Hot => {
                self.hot_by_quiet.remove(&(meta.last_arrival, id));
            }
            Location::Cold(_) => self.cold -= 1,
        }
        Some(meta)
    }

    pub(crate) fn clear(&mut self) {
        *self = TraceIndex::default();
    }
}

/// The decoded form of the cold traces read last, least recently used first
/// out. Keyed by the trace's record (a position nothing else ever gets again),
/// so an entry can never be mistaken for another version of its trace.
#[derive(Default)]
pub(crate) struct ColdCache {
    /// Most recently used first.
    entries: VecDeque<(segment::Location, Arc<TraceEntry>)>,
    spans: usize,
}

impl ColdCache {
    /// The decoded trace of `blob`, which becomes the most recently used.
    pub(crate) fn get(&mut self, blob: segment::Location) -> Option<Arc<TraceEntry>> {
        let position = self.entries.iter().position(|(key, _)| *key == blob)?;
        let hit = self.entries.remove(position)?;
        let entry = Arc::clone(&hit.1);
        self.entries.push_front(hit);
        Some(entry)
    }

    /// Remembers `entry` as the decoded trace of `blob`.
    pub(crate) fn put(&mut self, blob: segment::Location, entry: Arc<TraceEntry>) {
        self.remove(blob);
        if entry.len() > CACHE_MAX_SPANS {
            return;
        }
        self.spans += entry.len();
        self.entries.push_front((blob, entry));
        while self.entries.len() > CACHE_TRACES || self.spans > CACHE_MAX_SPANS {
            let Some((_, evicted)) = self.entries.pop_back() else {
                break;
            };
            self.spans -= evicted.len();
        }
    }

    /// Forgets the decoded trace of `blob`, and gives it back if it was there.
    pub(crate) fn remove(&mut self, blob: segment::Location) -> Option<Arc<TraceEntry>> {
        let position = self.entries.iter().position(|(key, _)| *key == blob)?;
        let (_, entry) = self.entries.remove(position)?;
        self.spans -= entry.len();
        Some(entry)
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.spans = 0;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use loctel_model::{Resource, Span, SpanId, SpanKind, StatusCode};

    use super::*;

    fn id(n: u8) -> TraceId {
        TraceId([n; 16])
    }

    fn span(trace: u8, span: u8, start: u64) -> Span {
        Span {
            trace_id: id(trace),
            span_id: SpanId([span; 8]),
            parent_span_id: None,
            name: "x".to_string(),
            kind: SpanKind::Internal,
            start_unix_nano: start,
            end_unix_nano: start + 10,
            status: StatusCode::Unset,
            status_message: String::new(),
            attributes: Vec::new(),
            events: Vec::new(),
            resource: Arc::new(Resource::default()),
            scope_name: String::new(),
        }
    }

    /// A hot trace of `spans` spans from `start`, arrived at `now`.
    fn entry(trace: u8, spans: u8, start: u64, now: u64, arrival: u64) -> TraceEntry {
        let mut entry = TraceEntry::new(now, arrival);
        for n in 0..spans {
            entry.insert(span(trace, n, start + u64::from(n)), now);
        }
        entry
    }

    fn blob(n: u64) -> segment::Location {
        segment::Location {
            seq: 0,
            offset: n * 100,
            len: 50,
        }
    }

    fn newest(index: &TraceIndex) -> Vec<TraceId> {
        index.newest_first(None).map(|(_, id)| id).collect()
    }

    #[test]
    fn the_orders_follow_every_change() {
        let mut index = TraceIndex::default();
        let (a, b, c) = (
            entry(1, 2, 100, 5, 0),
            entry(2, 1, 300, 6, 1),
            entry(3, 1, 200, 7, 2),
        );
        index.add_hot(id(1), &a);
        index.add_hot(id(2), &b);
        index.add_hot(id(3), &c);
        assert_eq!(newest(&index), vec![id(2), id(3), id(1)]);
        assert_eq!(index.oldest_start(), Some(100));
        assert_eq!(index.oldest_arrival(), Some(id(1)));
        assert_eq!(index.oldest_hot(), Some((5, id(1))));

        // A cursor is "strictly after" in the newest-first order.
        let after: Vec<TraceId> = index
            .newest_first(Some((300, id(2))))
            .map(|(_, id)| id)
            .collect();
        assert_eq!(after, vec![id(3), id(1)]);

        // A span that starts earlier, and arrives later, moves the trace in two
        // of the orders and not in the third.
        let mut a = a;
        a.insert(span(1, 9, 50), 9);
        index.sync_hot(id(1), &a);
        assert_eq!(index.oldest_start(), Some(50));
        assert_eq!(index.oldest_hot(), Some((6, id(2))));
        assert_eq!(index.oldest_arrival(), Some(id(1)));
        let meta = index.get(&id(1)).unwrap();
        assert_eq!((meta.start, meta.end, meta.span_count), (50, 111, 3));

        // Cold: out of the order of the spill step, still in the others.
        let summary = a.summary(id(1), true);
        index.make_cold(
            id(1),
            Cold {
                blob: blob(1),
                summary,
            },
        );
        assert!(index.is_cold(&id(1)));
        assert_eq!(index.cold_len(), 1);
        assert_eq!(index.oldest_hot(), Some((6, id(2))));
        assert_eq!(newest(&index), vec![id(2), id(3), id(1)]);

        // And back.
        assert_eq!(index.make_hot(id(1)).map(|cold| cold.blob), Some(blob(1)));
        assert_eq!(index.cold_len(), 0);
        assert_eq!(index.oldest_hot(), Some((6, id(2))));
        assert!(index.make_hot(id(1)).is_none(), "it is hot already");

        // Gone from every order, cold or hot.
        index.make_cold(
            id(3),
            Cold {
                blob: blob(3),
                summary: c.summary(id(3), true),
            },
        );
        assert!(index.remove(id(3)).is_some());
        assert!(index.remove(id(2)).is_some());
        assert_eq!((index.len(), index.cold_len()), (1, 0));
        assert_eq!(newest(&index), vec![id(1)]);
        assert_eq!(index.oldest_hot(), Some((9, id(1))));
        assert!(index.remove(id(2)).is_none());
        index.clear();
        assert_eq!(
            (index.len(), index.oldest_start(), index.oldest_arrival()),
            (0, None, None)
        );
    }

    #[test]
    fn cache_keeps_the_last_64_traces_and_the_one_just_read() {
        let mut cache = ColdCache::default();
        for n in 0..100u64 {
            cache.put(blob(n), Arc::new(entry(1, 1, 0, 0, 0)));
        }
        assert!(cache.get(blob(99)).is_some(), "the newest is there");
        assert!(cache.get(blob(36)).is_some(), "the 64th newest is there");
        assert!(cache.get(blob(35)).is_none(), "older ones are out");

        // A read makes a trace the newest: it outlives ones put after it.
        let mut cache = ColdCache::default();
        for n in 0..64u64 {
            cache.put(blob(n), Arc::new(entry(1, 1, 0, 0, 0)));
        }
        assert!(cache.get(blob(0)).is_some());
        for n in 64..100u64 {
            cache.put(blob(n), Arc::new(entry(1, 1, 0, 0, 0)));
        }
        assert!(cache.get(blob(0)).is_some(), "read after it was put");
        assert!(cache.get(blob(1)).is_none());
    }

    #[test]
    fn cache_forgets_what_is_removed_and_counts_spans() {
        let mut cache = ColdCache::default();
        cache.put(blob(1), Arc::new(entry(1, 3, 0, 0, 0)));
        cache.put(blob(1), Arc::new(entry(1, 5, 0, 0, 0)));
        assert_eq!(
            cache.get(blob(1)).map(|entry| entry.len()),
            Some(5),
            "replaced"
        );
        assert_eq!(cache.spans, 5, "not counted twice");
        assert_eq!(cache.remove(blob(1)).map(|entry| entry.len()), Some(5));
        assert_eq!(cache.spans, 0);
        assert!(cache.remove(blob(1)).is_none());
        assert!(cache.get(blob(1)).is_none());
    }
}
