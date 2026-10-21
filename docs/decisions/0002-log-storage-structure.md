# 0002: How the store holds logs

Date: 2026-10-21
Status: accepted

## Context

The Milestone 1 plan (Task 5) says logs live in a time-ordered `Vec`, with
binary-searched insertion for late arrivals, plus a
`HashMap<(TraceId, SpanId), Vec<idx>>` index rebuilt on eviction. Written out,
that has two problems at the sizes the design asks for (5,000,000 records, 50,000
spans plus logs per second, §4 and §5):

- Late arrivals are normal: batches from several services interleave. An
  insertion in the middle of the `Vec` shifts every later position, so the
  index of positions goes stale on every late log, not only on eviction.
- Once the store is at its cap, every batch evicts from the front of the
  `Vec`. A `Vec` moves all the remaining records each time, and rebuilding the
  index is a pass over every record. Both are work proportional to the whole
  store, done while holding the write lock, once per batch.

## Decision

Give each held log a stable key `(time, arrival number)`, and hold:

- the records in a `BTreeMap` keyed by it (oldest first),
- a `HashSet` of the dedup keys of the held records,
- a per-span index of keys, in time order (a `VecDeque` per `(TraceId, SpanId)`).

Insertion in time order, eviction of the oldest and a keyset cursor over
`(time, arrival)` are then all logarithmic or constant, and no index is ever
rebuilt. The public API is exactly the plan's. Time is the log's timestamp, or
its observed time when the timestamp is unset (0), so logs from exporters that
leave the timestamp out are not filed under 1970.

## Consequences

- A measured one-off (release build, cap of 1,000,000, batches of 500 slightly
  out of order): about 600,000 logs/s with eviction on every batch, against a
  target of 50,000 spans plus logs per second.
- A search that matches nothing still scans every log newer than `since`
  (about 0.12 s per 1,000,000 logs). That is accepted for Milestone 1; M5
  replaces the store with indexed segments behind the same API (design §5).
- Dedup remembers only held logs. A retry that arrives after its original was
  evicted is accepted as new. That bounds memory, and the retry is the oldest
  log, so it goes first again.
- The structure is an internal detail of `loctel-store`. Nothing outside the
  crate may depend on it. A later agent that finds the plan's `Vec` description
  should not change this back without redoing the arithmetic above.
