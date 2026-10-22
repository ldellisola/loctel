# 0045: The trace index, the spill policy and rehydration

Date: 2026-10-22
Status: accepted

Milestone 5, Task 2. The plan fixes the shape (an ordered index, hot and cold
traces, a spill step that runs on ingest, rehydration by a late span, reads that
decode through a small LRU of 64) and the names (`TraceMeta`, `spilled_traces`,
`StoreConfig::spilling_for_tests`, `each_mode`). This file records what it left
open, and the places where it had to be read.

## Context

Task 1 built the segment files; the store did not use them. Task 2 makes the
store use them for **traces**, behind a public API that must not change: every
test of the crate has to pass unchanged, and has to pass again against a store
that spills. Logs stay in memory until Task 3, and the disk cap is Task 4's.

## Decisions

### The index

- **`TraceMeta`** (internal, `src/trace_index.rs`) has the fields the plan
  lists: id, first-arrival number, start, end, span count, `last_arrival`, and
  `location: Hot | Cold`. The plan's "service set, status, failed-in, first
  exception" are one value for a cold trace: **the `TraceSummary` it had when it
  left memory** (which also holds the services in the order of their first
  span, the root's name and service, and the span count). Keeping the whole
  summary, not four fields of it, is what lets `summary`, `search_traces` and
  the MCP tools answer for a cold trace without reading a byte of it, and costs
  a few hundred bytes a trace (a failing trace's first exception holds its
  stack, which is the one large part). For a hot trace the entry keeps only what
  the index keys need (start, end, count, `last_arrival`); status and the rest
  are worked out from the spans when asked, as before M5. (Task 5 moved one of
  them: the status of a hot trace is kept in the index too, decision 0048.)
- **Three orders**, as the plan says for the first: `BTreeSet<(start, id)>`
  walked backwards for the search, `BTreeMap<arrival, id>` for eviction and
  `scan_traces` (the existing `arrival_order`), and one more the plan does not
  name, `BTreeSet<(last_arrival, id)>` over the hot traces, which is what the
  spill step takes from. The three are kept in step by `TraceIndex`, whose
  methods are the only way to change them (`sync_hot` after a hot trace gains
  or loses a span).
- **The hot traces' spans** stay in their own `HashMap<TraceId, TraceEntry>`
  (the M1 type, plus `from_spans` and `last_arrival()`), as the plan has it:
  hot traces in memory, the index beside them.
- **`search_traces` no longer sorts.** It walks the index newest first from the
  cursor, and breaks at the first trace that starts before `since`. The old
  version collected and sorted every trace that passed the cheap conditions;
  Task 5 asks that nothing do that, and the index makes it free here.
  `oldest_start_unix_nano` is the index's first key. `service_map` and
  `services` skip, without reading, every trace whose end is before the window.

### The spill step

- **Where it runs.** The plan says "on each ingest call and at most once a
  second". It runs there (at the end of `ingest_spans` and `ingest_logs`) and
  **also ahead of every read of traces and `stats`**, with the same throttle.
  Reason: an exporter that stops leaves a store that is never ingested into
  again, and a step that only runs on ingest would hold the last 30 seconds of
  traces in memory for ever. The window reads every half second, so in practice
  the traces go. A read takes the write lock only when a step is due (it
  looks at the store clock and the time of the last step first, without a lock), so
  at most one read a second pays for it. The cost of this reading is that a caller must not call the store from
  inside `with_trace_spans`, `scan_traces` or `scan_logs`: the docs already said
  so (a nested read could deadlock behind a waiting writer), and now a nested read
  can deadlock without a writer. Nothing in the workspace does.
- **"At most once a second" is judged by the store's clock**, so tests with a
  `ManualClock` control it. The interval is the smaller of a second and
  `spill_after`, so the test config (`spill_after` 0) runs a step at every call
  and a test needs no sleeping or clock moving to see a trace spill. The throttle
  measures the distance between two times either way round, so a clock that steps
  back does not stall the step.
- **The policy**, oldest `last_arrival` first, stopping at the first trace that
  does not qualify (the order is by age, so no later one does): spill a trace that
  has been quiet for `completeness_quiet + spill_after`, and, while the hot
  traces hold more than `hot_max_spans`, spill the oldest that is complete
  (quiet for `completeness_quiet`) without waiting. An **incomplete** trace is
  never spilled, even over the budget: that is the plan's wording, and what to do
  when everything in memory is still being written is "memory pressure", Task 4.
- **The I/O is under the write lock**, one trace at a time: encode, compress and
  one `pwrite`. Moving it out of the lock is a performance matter for Task 6's
  budgets (`ingest` p99 under 250 ms), which will measure whether it is needed.
- **A write that fails** leaves the trace hot, ends the step, and is kept in
  `spill_error` (a hidden `Store::spill_error()` for now; Task 4 puts it in the
  status). The next step retries; Task 4 changes that to once a minute.

### Rehydration

- A span for a cold trace is first checked against the trace (decoded through
  the cache): **if the trace has the span already, nothing happens**, and the
  trace stays cold. An exporter retrying a batch of finished traces must not make
  every one of them hot again, only to be written out a second later.
- Otherwise the trace becomes hot with its old first-arrival number and
  `last_arrival`, its record becomes **dead**, and the span is inserted like any
  new span, which sets `last_arrival` to now: **completeness is reset**, as the
  plan's Review Focus 1 asks, and the trace is spilled again when it has gone quiet
  again. `spans_received` counts the new span only, and `stats().spans` is not
  doubled (the spans of the dead record were never subtracted, and are not added
  again).
- **`remove_spans`** on a cold trace is the one write that is not an arrival: it
  keeps `last_arrival` (the trace stays complete, as for a hot trace) and does not
  wake a trace that has none of the spans.
- **A cold trace that cannot be read** (checksum, truncated, undecodable) is
  forgotten when a write meets it (a late span, `remove_spans`): its spans leave
  the counts and a late span starts a new partial trace, the same as for an
  evicted trace. Reads (`trace`, `with_trace_spans`, a query, `scan_traces`, the
  map) skip it, and the index still lists it from its summary; eviction will take
  it. The reader says so on stderr, once per segment (Task 1).

### Dead bytes, segments and the cache

- **`SegmentSet`** (`src/segment_set.rs`) is the new piece the plan does not
  name: the open writer per kind, the files and their sizes, the dead bytes of
  each, the maps. It is separate from the index because Task 3's log chunks need
  the same bookkeeping. Segment numbers are one counter over every kind, never
  reused (not even after `clear`, and an existing file name is skipped), so the
  numbers are the files' age and "the oldest segment" (Task 4) is the lowest.
- **Dead bytes** count toward the disk total until their segment is deleted, as
  the plan says; a segment whose records are all dead is not deleted early. A
  record is killed when its trace is rehydrated, removed or evicted.
- **A segment is rolled after an append** that makes it reach `segment_max_bytes`,
  so a file is a little over the size, and a trace is never split.
- **Maps are made on first read** and again when the file has grown past the part
  mapped, which is how a record of the segment still being written is read (a map
  covers the file as it was). Task 1's `Reader` is unchanged.
- **The store deletes its own files** on `clear` and when the last clone is
  dropped, besides the run directory's deletion of the directory. Tests then
  leave nothing behind, and a store given a directory that is not a run
  directory cleans up after itself.
- **The cache** of 64 decoded traces is keyed by the record's position
  (`segment::Location`), which is never given to another record, so a stale entry
  cannot be returned; it is also capped at a million spans in all, because 64
  traces of 100,000 spans would otherwise be a "small" cache of a gigabyte. Reads
  that look at many traces once (a TraceQL search, `scan_traces`, the service
  map) read through it and do not fill it, so they cannot push out the trace the
  window has open.
- **`Aggregate`** (service map) owns its names now, a copy at the first sight of
  each, where it used to borrow them from the spans: a trace decoded for the map
  is dropped as soon as it is counted. Lookups still hash a `&str`, with no
  allocation, so the map over hot traces costs what it did.

### The both-modes harness

- `tests/modes.rs` has `each_mode(|store, clock| ...)` as the plan says, and
  `each_mode_with(config, ...)` for the tests that set limits (`max_traces`,
  `max_log_records`, `completeness_quiet`), which the plan's one function cannot
  express. The limits are taken from the config in both modes; the spill settings
  are `spilling_for_tests`'s in the second.
- **Conversion.** Every test that built a store now runs its body inside the
  closure; the assertions are unchanged (rustfmt moved them one level in). Where a
  test built values before its store (`failure.rs`: spans and a failed-in that the
  body then consumes), the closure starts at the top of the test so it can run
  twice; a few helpers that returned a store (`with_cap`, `two_trace_store`, the
  query tests' `mesh`) take the closure instead. `clock.rs`'s clock test sets the
  clock to its own start inside the body instead of making its own.
- **`modes.rs` stands alone**, with its own `CLOCK_START`, instead of including
  `fixtures.rs`: Clippy rejects a file loaded as a module twice
  (`duplicate_mod`), and a `#[path]` include of the fixtures is one. `spill.rs`
  checks that the two numbers agree.
- **How much of the existing suite reaches a cold trace.** The policy only spills
  complete traces, so a test body that never moves the clock past the quiet period
  never has one, in either mode; about 23 of the converted tests do (the
  completeness and service-map ones). That is the plan's design, and `spill.rs` is
  where the cold paths are pinned one by one. A way to make the whole suite cold
  would be to spill incomplete traces in the test config; it was not done,
  because it would make the test config a different policy from the real one.

## Where the plan met what was already there

- `StoreConfig` is `Eq`; `PathBuf` is, so the new field changes nothing. Existing
  struct literals use `..StoreConfig::default()` and keep compiling.
- The test hooks the plan wants ("count the decodes with a test hook") are
  `#[doc(hidden)]` methods on `Store`: `trace_decodes`, `spill_disk_bytes`,
  `spill_dead_bytes`, `spill_error`. Task 4 adds the public `disk_bytes()` and the
  status fields; these hidden ones are not what the status will use.

## Consequences

- The store's memory is bounded by the hot traces plus the index (about a few
  hundred bytes a trace) plus the logs, which Task 3 moves out.
- `service_map` and `services` read every cold trace that reaches into the window,
  on every call. For the window the UI shows by default that is little; for
  "everything" over a large cold store it is the slowest call the store has. If it
  matters, a cold trace's contribution to the map can be kept in its index entry
  (it depends on the window only for a trace that starts before it).
- `scan_traces` (the rules preview) decodes every trace from `since` on.
- Ingest and reads that find a step due wait for the spill under the write lock.
  Task 6's `ingest` budget says whether that has to move out.
