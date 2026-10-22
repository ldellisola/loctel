# Store (`loctel-store`)

The store behind loctel. It takes spans in any order from any
service, assembles them into traces, says when a trace has gone quiet, and
evicts the oldest trace whole when it holds too many. Traces that are complete
move out of memory into temporary segment files and come back when a late span
arrives (Milestone 5, see "Hot and cold traces" below). It also holds log
records, linked to spans by ID, in chunks of which only the newest is in memory
(Milestone 5, see "Log chunks" below), and works out the service map and the
services list from the traces, filters both by TraceQL and LogQL queries, and can
remove traces, spans and their logs again (for the discard rules). Design references: §4 (order and lateness, completeness,
logs), §5 (storage, eviction, clear all), §9.2 (the map) and §9.4 (what
"failed" means).

Everything outside the crate goes through `Store`. M5 replaces the internals
with a spill-to-disk segment store, so the public API below is frozen (M5 adds
to it, never changes it) and callers stay away from anything else: every test of
the crate runs against a store in memory and against one that spills.

Status: trace assembly, completeness, dedup, eviction, failure
classification, trace summaries, trace search, logs, the service map and the
services list are implemented. So are (Milestone 2) the TraceQL and LogQL query
filters, matched spans, removal and the scans (see "Query filters", "Removal"
and "Scans" below), and (Milestone 4) the log IDs, the level and window filters,
the log histogram and the lines around a log, which the Logs screen is built on
(see "Log IDs, levels, histogram and lines around" below). Milestone 5 is under
way: the segment files and the run directory it spills into exist (see "Segment
files" below), **traces** spill through them (see "Hot and cold traces" below)
and **logs** are held in chunks, the sealed ones in segment files, with the
count of a filter, the log at a position and the histogram answered from what is
kept about each chunk (see "Log chunks" below). The **disk cap** is enforced: the oldest
segment files are deleted whole when the files take more than it, and a write that
fails is waited out and reported (see "The disk cap" and "A failing disk" below).
The searches over traces do bounded work: a summary is made only for a trace that
is returned, and "recent failures" is one pass over the index (see "Bounded work"
below).

## API

```rust
let clock = Arc::new(ManualClock::new(start_unix_nano)); // or SystemClock
let store = Store::new(StoreConfig::default(), clock.clone());

store.ingest_spans(spans);               // Vec<loctel_model::Span>
let snapshot = store.trace(trace_id);    // Option<TraceSnapshot>
let summary = store.summary(trace_id);   // Option<TraceSummary>
let page = store.search_traces(&TraceFilter::default(), &PageRequest { limit: 20, cursor: None });
let found = store.recent_failures(since, 5); // RecentFailures { failed, errors_inside }, one pass
store.ingest_logs(logs);                 // Vec<loctel_model::LogRecord>
let page = store.search_logs(&LogFilter::default(), &PageRequest { limit: 50, cursor: None });
let lines = store.logs_for_span(trace_id, span_id); // Vec<LogRecord>, oldest first
let page = store.search_logs_keyed(&LogFilter::default(), &PageRequest { limit: 50, cursor: None }); // Page<(LogId, LogRecord)>
let record = store.log(log_id);          // Option<LogRecord>
let bars = store.log_histogram(&LogFilter::default(), since, until, 48); // Vec<HistogramBucket>
let total = store.log_count(&LogFilter::default()); // Option<u64>: Some for an indexed filter
let rows = store.logs_at(&LogFilter::default(), offset, 50); // Vec<(LogId, LogRecord)>, newest first from a position
let around = store.logs_around(log_id, 3, 3); // Around { lines, target }
let oldest = store.oldest_log_unix_nano(); // Option<u64>
let map = store.service_map(TimeWindow { since_unix_nano }); // ServiceMap
let services = store.services(TimeWindow { since_unix_nano }); // Vec<ServiceStats>
let stats = store.stats();               // StoreStats
store.set_disk_cap(bytes);               // applies at once: the oldest segment files go whole
let on_disk = store.disk_bytes();        // what the segment files take, dead records included
let cap = store.disk_cap_bytes();        // the cap now
let oldest = store.oldest_start_unix_nano(); // Option<u64>, the earliest trace start held
let now = store.now_unix_nano();         // the store's clock, for markers
let removed = store.remove_trace(trace_id);               // Removed { spans, logs }
let removed = store.remove_spans(trace_id, &[span_id]);   // Removed { spans, logs }
let n = store.with_trace_spans(trace_id, |spans| spans.len()); // Option<R>, no copy
store.scan_traces(since_unix_nano, |id, spans, status| { /* ... */ });
store.scan_logs(since_unix_nano, |log| { /* ... */ });
store.clear();
```

- `Store` is `Clone + Send + Sync`. Clones share the same data (an `Arc`
  around an `RwLock`), so give each thread or task its own clone.
- `ingest_spans` and `ingest_logs` take the write lock once per batch. Every
  other method takes the read lock.
- `Clock` is `trait Clock: Send + Sync { fn now_unix_nano(&self) -> u64 }`.
  `SystemClock` is the wall clock. `ManualClock::new(start)` only moves when
  told: `advance(Duration)` (saturating) and `set(u64)` (either direction).
  Tests drive completeness with it instead of sleeping.
- `Store::oldest_start_unix_nano()` is the earliest start of any trace held
  (`None` for an empty store): a start, not an arrival, so a trace that arrived
  late can hold it. A pass over the traces that touches no span, under the read
  lock. The main window uses it for the length of the window "Everything" covers
  (the rates of the service map).
- `Store::now_unix_nano()` is the current time on that same clock. The MCP
  `get_marker` tool hands it to agents as the `since` mark, so the marker and
  completeness agree on "now", and a test that drives the store with a
  `ManualClock` controls the marker.
- `StoreConfig` and its defaults:

  | Field | Default | Meaning |
  |---|---|---|
  | `max_traces` | 200,000 | past this, the oldest trace is evicted whole |
  | `max_log_records` | 5,000,000 | past this, the oldest log records are evicted |
  | `completeness_quiet` | 10 s | quiet time after which a trace is complete |
  | `spill_dir` | `None` | where the segment files go; `None` keeps everything in memory |
  | `spill_after` | 30 s | how long a complete trace stays in memory before it is spilled |
  | `hot_max_spans` | 2,000,000 | spans of traces in memory before the oldest complete ones go early |
  | `segment_max_bytes` | 64 MiB | a segment file is finished at this size and the next trace starts a new one |
  | `disk_cap_bytes` | 4 GiB | the most the segment files may take: past it the oldest segment goes whole (see "The disk cap") |
  | `spill_retry` | 60 s | how long the store waits after a failed write before it tries to write again (see "A failing disk") |
  | `log_chunk_max` | 65,536 | logs the open chunk takes before it is sealed |
  | `log_chunk_age` | 10 s | how long the open chunk may stay open, from its first log, before it is sealed |

  `StoreConfig::spilling_for_tests(dir)` (hidden from the docs) is the config
  the tests spill with: `spill_after` 0, `segment_max_bytes` 4 KiB,
  `hot_max_spans` 16, `log_chunk_max` 7 (so that every log test of the crate
  runs over sealed chunks) and `spill_retry` 0 (a failed write is tried again at
  the next step), the rest the defaults.

- `StoreStats`: `traces`, `spans` and `logs` are what the store holds now.
  `spans_received` and `logs_received` are lifetime totals of what it
  accepted. See "Counting" below. Since Milestone 5 it also says where the data
  is: `spilled_traces` (the traces that are in segment files), `disk_bytes` (what
  the files take, dead records included) and `spill_error` (why the last write to
  a segment failed, until one works). It is no longer `Copy` (`spill_error` is a
  `String`), and **`==` compares the five counters only**: the store's tests
  compare a store in memory with one that spills, fed the same data, and they
  must keep passing now that the second has files and the first has none. Compare
  `spilled_traces`, `disk_bytes` and `spill_error` yourself when you mean them.

### `TraceSnapshot`

What `Store::trace` returns: a copy of the trace as of the call. Later ingest
never changes a snapshot you hold.

| Field | Meaning |
|---|---|
| `trace_id` | the ID asked for |
| `spans` | every span, sorted by `start_unix_nano`, then `span_id` |
| `root` | the root span's ID, or `None` while no root has arrived |
| `orphans` | span IDs whose parent is not in the trace, in `spans` order |
| `complete` | no new span for `completeness_quiet` |
| `status`, `failed_in` | failure classification (see below) |

`children(id)` yields the direct children of a span, in `spans` order. It
scans the spans, so walking a whole large trace through it is quadratic: a
caller that does that should build its own parent-to-children map in one
pass.

`status` is a `TraceStatus` (`Ok`, `ErrorsInside`, `Failed`, serialized as
`snake_case`) and `failed_in` an `Option<FailedIn>`. Both types and the
classification live in `src/failure.rs`. They are computed when the snapshot
is taken, from the spans held at that moment, so a late span can change them
(see "Failure semantics" below).

## Assembly rules

- **Join by trace ID.** A span joins its trace whenever it arrives, whatever
  the order and whichever service sent it. A batch may mix traces.
- **Root.** The span with `parent_span_id == None`. If there are several,
  the earliest start wins, and a tie goes to the lowest span ID. A trace with
  no such span has `root == None`.
- **Orphan.** A span whose parent ID is not in the trace. It stays listed as
  an orphan until the parent arrives, which can be in a later batch or later
  in the same batch (design §4, "missing parent"). A span whose parent is
  itself is not an orphan and not a root; it is stored and otherwise ignored
  by assembly. Rejecting such spans is the ingest layer's job; the store only
  has to survive them.
- **Ordering.** `spans` is sorted by start time, then span ID. Clock skew
  between containers can put a child before its parent in that order, or make
  a span's end earlier than its start. Neither breaks anything: `children()`
  follows parent IDs, not positions, and `Span::duration_nanos()` clamps to 0.

## Dedup

Dedup is by `span_id` within a trace: the first copy stored wins and later
copies are dropped. Exporters resend batches on retry, so this is routine,
not an error. The same span ID in two different traces is two spans.

A duplicate is not a new span, so it does not count for completeness. A retry
that arrives after the quiet period leaves the trace complete.

## Completeness

A trace is complete when `now - last_arrival >= completeness_quiet`, where
`last_arrival` is the clock time at which its last new span was stored (design
§4: "complete once no new span has arrived for it for 10 seconds").

- It is computed on read from the store's clock. The span timestamps play no
  part: it is about when the spans arrived, not when they happened. (A trace
  that is spilled keeps the time its last span arrived in the index, so the
  answer is the same for a trace in a segment file as for one in memory.)
- A new span arriving for a complete trace makes it incomplete again.
- If the clock steps backwards, the quiet time reads as 0 (not complete); it
  never underflows.
- An open trace keeps updating either way. `complete` is information, not a
  lock.

## Eviction

Once the number of traces exceeds `max_traces`, the oldest trace by **first
arrival** is removed, whole, never a few of its spans (design §5).

- Order is by when the trace first appeared, not by recent activity or span
  timestamps. A busy old trace is still the first to go.
- A trace in a segment file is evicted like any other: it leaves the index
  without being read, and its record in the file is dead (see "Dead bytes"
  below).
- A batch is applied in full before eviction runs. Evicting in the middle of
  a batch would drop a trace and then resurrect a fragment of it from the
  batch's remaining spans.
- A span that arrives for an already evicted trace cannot be matched with the
  dropped spans. It starts a new, partial trace (its parent shows as
  missing), which can in turn evict the oldest trace.
- `clear()` drops everything at once ("Clear all", design §5) and keeps the
  `*_received` totals.

## Counting

`spans_received` counts spans the store accepted: a duplicate is counted
once (the retry is ignored), a span evicted later still counts, and
`clear()` keeps the total. It is a lifetime counter for rate displays, not a
size: use `StoreStats::spans` for what is held now. `logs_received` is counted
the same way: a duplicate log is counted once, an evicted log still counts, and
`clear()` keeps the total.

## Failure semantics

"Failed" is defined once, in design §9.4, and used everywhere: the trace
table, the trace header, map colours, the "errors only" toggle,
auto-selection and MCP. The store implements it in `classify` (`src/failure.rs`),
a pure function, so nothing else may re-derive it. The rules, quoted from
§9.4:

> - **Erroring span:** a span whose status is `Error`.
>   - An exception event alone does not make a span erroring. That covers a
>     handled exception, or a retry that then succeeded.
>   - Such exceptions are still shown: as markers on the waterfall, and in
>     the inspector.
> - **Failed trace:** a trace whose **root span** (the request's entry point)
>   is erroring. This answers "did my request work?".
> - **Trace with errors inside:** the root span is fine, but some other span
>   is erroring, for example a call that failed and then recovered on retry.
>   - It is shown differently from a failed trace: amber "errors inside", not
>     red "Failed in".
>   - The "errors only" toggle includes both kinds.
>   - MCP reports the two separately, so an agent can tell "the request
>     failed" from "the request succeeded after errors".
> - **Root not arrived yet:** while the root span hasn't arrived, a trace
>   with erroring spans counts as "errors inside". It becomes "failed" if the
>   root arrives erroring.
> - **Failed-in span:** first take the erroring spans with no erroring
>   descendants (the lowest error on each branch). The one that **started
>   first** wins. If others exist, show "+N more".
>   - Example: checkout calls payments, which fails. Later, checkout calls
>     inventory, which calls the database, which also fails. Failed in =
>     payments, "+1 more".
>   - Shown as "service · span name".

The last two bullets of §9.4 (service and edge error rates) belong to the
service map and are covered in "Service map" below.

How the store reads those rules:

- **`classify(spans: &[Span], root: Option<SpanId>) -> (TraceStatus, Option<FailedIn>)`.**
  `spans` may be in any order. `status` is `Failed` if the root is erroring,
  else `ErrorsInside` if any span is erroring, else `Ok`. `failed_in` is
  `Some` exactly when `status` is not `Ok`. A missing root, or a `root` ID that
  matches no span, can never make a trace `Failed`.
- **Descendants follow parent IDs, never time.** A child that starts before
  its parent (clock skew between containers) is still its parent's
  descendant, so the parent is not a lowest error. Spans whose parent has not
  arrived are not descendants of anything yet; they count as erroring spans
  like any other.
- **Failed-in choice.** Among the candidates (erroring spans with no
  erroring descendant) the minimum `(start_unix_nano, span_id)` wins: the
  earliest start, and a tie goes to the lowest span ID, so the answer does not
  depend on arrival order. `more` is the number of other candidates.
- **`peer`** is `Span::peer()` for `Client` and `Producer` spans and `None`
  for every other kind, even if a server span happens to carry
  `server.address`.
- **Malformed input.** Ingest rejects a span that is its own parent, but the
  store only has to survive it. A self-parenting span is not its own
  descendant. A longer parent cycle (A under B under A) leaves no lowest
  error; then every erroring span counts as a candidate, so `failed_in` is
  still `Some`. `classify` is iterative and linear, so a very deep trace does
  not overflow the stack.
- **Computed on read.** `TraceSnapshot::status`, `failed_in` and the
  summary's copies are worked out from the spans held when the snapshot or
  summary is taken. Nothing is cached, so a late span (the root arriving, a
  retry) takes effect on the next read.

### `trace_exception`

`trace_exception(&TraceSnapshot) -> Option<ExceptionInfo>` picks the exception
that best explains the trace:

1. the failed-in span's own exception, if it has one;
2. else the nearest ancestor's, walking from the failed-in span up the parent
   chain towards the root;
3. else the first exception in start order (`(start_unix_nano, span_id)`)
   anywhere in the trace. This is what finds the handled exception of a trace
   whose status is `Ok`, which §9.4 says is still shown.

A span's exception is its first event named `exception` (`Span::exception()`).

## Trace summaries

`Store::summary(id) -> Option<TraceSummary>` is a trace in one row, the item
of every trace list. `search_traces` returns the same values.

| Field | Meaning |
|---|---|
| `trace_id` | the trace |
| `root_name`, `root_service` | the root span's name and service; `None` until the root arrives |
| `start_unix_nano` | the earliest span start |
| `duration_nanos` | from the earliest span start to the latest span end. An end before its own span's start counts as that start, so skew never gives a negative or wrapped duration |
| `span_count` | spans held |
| `services` | each service once, in the order of its earliest span start, so the entry point comes first (arrival order would put the leaf services first, since they finish and report first) |
| `status`, `failed_in` | as in the snapshot |
| `first_exception` | `trace_exception` of the trace |
| `complete` | no new span for `completeness_quiet` |
| `matched_span_ids` | the spans a TraceQL query matched (at most `MATCHED_SPANS_MAX`, 20); empty without a query. See "Query filters" |

`TraceSummary` is `Serialize` (status as `snake_case`, IDs as hex), like the
types it holds. `matched_span_ids` is left out of the JSON when it is empty, so
a summary that came with no query serializes exactly as it did before queries.

## Trace search

`Store::search_traces(&TraceFilter, &PageRequest) -> Page<TraceSummary>`.

`TraceFilter` (the default keeps everything; every condition that is set must
hold):

| Field | Keeps |
|---|---|
| `since_unix_nano` | traces whose start is at or after it (inclusive) |
| `service` | traces in which any span has that service (exact name) |
| `errors_only` | `Failed` and `ErrorsInside` traces, per §9.4 |
| `min_duration_nanos` | traces whose duration is at least it (inclusive) |
| `query` | traces in which the TraceQL query matches at least one span (`Option<Arc<TraceQuery>>`; see "Query filters") |

`TraceFilter` is `Clone + Debug + Default + PartialEq`, but not `Eq`: a query
holds floats.

Results are **newest first by trace start**; traces that start together go by
trace ID, descending, so the order is total.

Paging is keyset-based:

- `PageRequest { limit, cursor }`. `limit` 0 gives an empty page with no
  cursor. A huge limit is fine (`usize::MAX` returns everything).
- `Page { items, next_cursor }`. `next_cursor` is `"{start_unix_nano}-{trace_id}"`
  of the last item, with the trace ID as 32 lowercase hex characters. Callers
  must treat it as opaque and pass it back unchanged.
- `next_cursor` is `Some` only if at least one more matching trace exists, so
  the last page, full or not, ends with `None`.
- The next page is "strictly after the cursor position", so traces arriving
  between two requests never shift the page boundary: newer traces simply are
  not on later pages, and no trace appears twice. (A late span with an earlier
  start can change a trace's start and so its position; that is rare and
  harmless.)
- A cursor the store did not produce yields an **empty page** with no
  cursor. Restarting from the top would send a paging agent round in
  circles; the MCP layer is free to reject such a cursor with a clear error
  before it gets here.

Cost: the search walks the trace index newest first, from the top or from the
cursor, and stops at the first trace that starts before `since` and once the
page is full and one more match has shown that there is a next page. It never
sorts or copies the traces held. The cheap conditions (`since`, `min_duration`,
`errors_only`, the cursor) are read from the index, for a trace in memory and one
in a segment file alike; service membership of a trace in a segment file and its
summary are too (see "Hot and cold traces"), and only a TraceQL query has to read
the spans of a trace in a segment file, one trace at a time and only for traces
the other conditions kept. The read lock is held for the call. See "Bounded work"
for what a call makes.

### Bounded work

A search looks at many traces and returns few, so what it does for a trace it
turns away is what it costs. Since Milestone 5, Task 5:

- **A summary is made only for a trace that goes into the page.** A trace is
  judged by the conditions above first, the cheap ones from the index, then
  the service and the query, and only then summarized (for a trace in memory
  that is a walk of its spans; for one in a segment file, a copy of the summary in
  the index). The next page's "one more match" is found without making its
  summary. So a page of `limit` makes `limit` summaries, however many traces were
  turned away, and a search with `errors_only` over 50,000 traces of which 3
  failed makes 3.
- **The index knows whether a trace failed.** `errors_only` has to be answered
  for every trace the walk meets, so `TraceMeta` holds the trace's status
  (`Ok`, `ErrorsInside` or `Failed`), for a trace in memory as well as for one in
  a segment file. A trace in memory keeps it up to date as spans come and go
  (`TraceEntry` counts its erroring spans and tracks which parentless span is the
  root, so asking costs nothing; a test holds it equal to `classify` over random
  spans, removals included), and the index copies it after every change. Before,
  a trace in memory was summarized to find out, and thrown away if it was fine.
- **`Store::recent_failures(since_unix_nano, limit) -> RecentFailures`** is the
  traces behind the MCP `recent_failures`: up to `limit` that failed and up to
  `limit` with an error inside, each newest first, which is what two searches with
  `errors_only` give once split by status. It is one pass over the index from the
  newest trace, under one read of the store, that stops when both lists are full
  and makes a summary only for the traces it puts in them. A trace that failed
  after the `failed` list is full costs a lookup, and nothing else: with a
  hundred thousand failed traces and no trace with an error inside (a service
  that is down), it still makes `limit` summaries, not a hundred thousand.
  `limit` 0 is two empty lists. It reads nothing from a segment file.
- **`services` and `service_map`** do not sort or copy the traces held either:
  they count the spans of the traces that reach into the window, one trace at a
  time, and a trace that ended before the window is skipped from its index entry
  (so a window of the last minute decodes the traces of the last minute, not all
  of those in segment files).

The hidden `Store::summaries_built()` counts the summaries reads have made (a
search result or `summary`; the spill step writing a trace out is not a read), and
is how the tests hold the above (`spill.rs` and the MCP `tools.rs`).

## Logs

```rust
store.ingest_logs(logs);
let page = store.search_logs(&LogFilter::default(), &PageRequest { limit: 50, cursor: None });
let lines = store.logs_for_span(trace_id, span_id);
```

Logs arrive from the same exporters as spans, in batches, in any order, and
are linked to spans by `trace_id` and `span_id` when the log carries them
(design §4). A log without trace context is still stored and searchable.

- **Time order.** A log's time is its `time_unix_nano`, or its
  `observed_unix_nano` when the timestamp is unset (OTLP sends 0 for
  "unknown"; such a log would otherwise sit in 1970, be evicted first and
  never match `since`). Logs are kept in time order, a tie going to arrival
  order, and a late arrival takes its place in that order, so batches from
  several services interleave correctly.
- **Dedup.** By `LogRecord::dedup_key()`: a hash of the time, body, trace ID,
  span ID and service. Observed time and attributes are left out on purpose,
  since a retry may change them. A log identical to one already held is
  ignored, and not counted in `logs_received`. Dedup remembers only the logs
  the store still holds, which keeps memory bounded: a retry that arrives
  after its original was evicted is accepted like a new log (and, being the
  oldest, is the first to go again).
- **Eviction.** After a whole batch is applied, the oldest logs by time are
  removed until at most `max_log_records` remain. Arrival order does not
  matter: a late log older than everything held is the one dropped. As with
  spans, evicting inside a batch would forget a log and then accept its retry
  from the same batch as new. Logs are evicted independently of traces: a
  trace's spans can go while its logs stay, and the other way round.
- **`clear()`** drops every log and its dedup memory, so the same line is
  accepted again afterwards. `logs_received` is kept.

### `LogFilter` and `search_logs`

`Store::search_logs(&LogFilter, &PageRequest) -> Page<LogRecord>`. Every
condition that is set must hold; the default keeps everything.

| Field | Keeps |
|---|---|
| `since_unix_nano` | logs at or after it (inclusive), by the time defined above |
| `until_unix_nano` | logs strictly before it (exclusive), by the same time. `[since, until)` is a window that tiles with the next one |
| `service` | logs from that service (exact name; `unknown_service` for logs without one) |
| `min_severity` | logs of that severity or higher. `Unspecified` is the lowest level, so any `Some(level)` above it drops unlabelled logs |
| `levels` | logs whose severity is in the `LevelSet` (see "Log IDs, levels, histogram and lines around"). `None` keeps every level, `Some` of an empty set keeps nothing. Combines with `min_severity` (both must hold) |
| `contains` | logs whose **body** contains the text, ignoring case (Unicode lowercase folding). Attributes and service name are not searched. The empty text matches every log |
| `trace_id` | logs emitted in that trace (with or without a span ID) |
| `span_id` | logs emitted in a span with that ID, in any trace unless `trace_id` is set too |
| `query` | logs the LogQL query matches (`Option<Arc<LogQuery>>`; see "Query filters") |

`LogFilter` is `Clone + Debug + Default + PartialEq`, not `Eq`, like
`TraceFilter`.

Results are **newest first**: by log time, then by arrival (the later arrival
first). Paging is a keyset, as for traces:

- `limit` 0 gives an empty page with no cursor.
- `next_cursor` is `"{time}-{arrival}"` of the last item (the arrival number
  is the store's own counter), and is `Some` only if at least one more match
  exists. Callers must treat it as opaque.
- The next page is "strictly older than the cursor", so logs arriving between
  two requests never shift the boundary, and eviction of older logs just ends
  the listing sooner.
- A cursor the store did not produce yields an empty page with no cursor.

Cost: the walk starts at the newest log older than `until` (and than the
cursor) and stops when the page is full or the logs are older than `since`, so a
recent page costs about its size: the open chunk, or one sealed chunk read from
its segment, two when the page runs over a chunk's edge. A filter that matches
rarely scans every log that is newer than `since`, but it passes over the chunks
that cannot hold a match without reading them: one with no log of the `service`
(or of the exact `service_name` of a LogQL selector), none at the `levels` or
above the `min_severity`, or none of the `trace_id`. `contains`, `span_id` and the
line filters of a LogQL query have no such shortcut, and read every chunk from
`since` on: about 33 ms for each sealed chunk of 65,536 logs, the time to decode
it (see "What it costs" under "Log chunks"). On the development container
(one-off, release build, 10,000,000 logs held, 152 sealed chunks), the first
page of `{service_name="payments-api"} |= "timeout=7"` took 0.8 ms, the
service's logs being found in the newest chunk.

### `logs_for_span`

`Store::logs_for_span(trace, span) -> Vec<LogRecord>` returns the logs whose
`trace_id` and `span_id` both equal the arguments, **oldest first**. A log
that has a trace ID but no span ID is not linked to any span (find it with
`search_logs` and `trace_id`). Logs and spans are stored independently, so the
call works before the span arrives and after its trace is evicted. It reads a
per-span index, so it costs the number of logs the span has.

### Log IDs, levels, histogram and lines around

Added in Milestone 4 for the Logs screen (plan Task 1). Nothing about
`search_logs` changed: it is now `search_logs_keyed` with the IDs dropped.

**`LogId { time_unix_nano, seq }`** is the key the store already used inside
(`time` as defined above, then the arrival number), made public. It is `Copy`,
`Ord` (the order of the store: by time, a tie going to the later arrival) and
`Hash`. It is stable for as long as the log is held (a late log inserting in
the middle or the eviction of older logs does not change it), and it is never
given to a second log: the arrival counter only counts up, `clear` included.
Because of that, an ID that outlives its log (evicted, removed by a discard rule,
cleared) reads as "gone", never as some other log.

- `Store::search_logs_keyed(&LogFilter, &PageRequest) -> Page<(LogId, LogRecord)>`
  is `search_logs` with each log's ID: the same filter, order and cursor
  (`"{time}-{arrival}"`, still opaque to callers). Both bound the page from
  above by the older of `until` and the cursor.
- `Store::log(LogId) -> Option<LogRecord>` is a map lookup: `None` once the log
  is gone, or for an ID never issued.
- `Store::oldest_log_unix_nano() -> Option<u64>` is the time of the oldest log
  held (a lookup at the front of the time order, not a pass), `None` for no logs.
  Added in Milestone 4 Task 4: the Logs screen's histogram has no start to count
  from under `Everything` without it.

**`LevelSet`** is a `u8` with a bit per `Severity` (a `match` without a wildcard
maps severities to bits, so a new severity is a compile error until it has one).
`LevelSet::default()` is the empty set, `LevelSet::ALL` every level, `with(sev)`
adds one and `contains(sev)` asks. `LevelSet::from_groups(err, wrn, inf, dbg)`
is the Logs screen's four toggles, as the union of the groups that are on:

| group | severities |
|---|---|
| `err` | error, fatal |
| `wrn` | warn |
| `inf` | info, **unspecified** |
| `dbg` | debug, trace |

Unlabelled logs (OTLP severity 0) count as `INF`, so switching `INF` off hides
them and no toggle combination makes a log unreachable.

**`Store::log_histogram(&LogFilter, since, until, buckets) -> Vec<HistogramBucket>`**
counts the logs in `[since, until)` that match the filter into `buckets` buckets
of equal width, oldest first. `HistogramBucket { total, errors }`: `errors`
counts error and fatal. Rules:

- The result is always exactly `buckets` long. No buckets gives an empty vector;
  an empty or backwards window (`until <= since`) gives all-zero buckets.
- The filter's own `since_unix_nano` and `until_unix_nano` are ignored: the
  window is the arguments (the Logs screen keeps its search window, the
  histogram its own range). Every other condition holds, `levels` and `query`
  included.
- A log at `t` goes into bucket `(t - since) * buckets / (until - since)`, rounded
  down, worked out in 128 bits (a window of years times 48 cannot overflow). So
  a log at exactly `since` is in bucket 0, one at exactly `until` is out, and
  a window that does not divide evenly still tiles: every log in it is counted
  once. The 48 buckets over 15 minutes of the mockup are 18.75 s wide.
- Cost: for an **indexed** filter (see "`log_count` and `logs_at`") the bars
  come from the counts kept per chunk and per second, and nothing is read when
  each chunk lies inside the window and each of its seconds inside one bucket
  (whole-second windows and buckets always do): see "Counts, positions and the
  histogram from the metadata" under "Log chunks". For any other filter it is a
  pass over the logs inside the window, each tested against the filter, reading
  every chunk in it. The Logs screen asks every 2 s.

**`Store::logs_around(LogId, before, after) -> Around`** is the "Show lines
around" of the Logs screen. It ignores every filter (the lines in between are
the point) and returns the lines oldest first, as `Around { lines:
Vec<(LogId, LogRecord)>, target: Option<usize> }`:

- The target is held: up to `before` logs older than it, the log itself and up to
  `after` newer ones. `target` is its index in `lines`, which is `before` unless
  fewer older logs remain (the oldest log held gives `Some(0)`).
- The target is not held (evicted, removed or cleared): `target` is `None` and
  the lines are the ones on either side of where it was, the `before` just
  older than its time and the `after` just newer. With nothing held the result is
  empty. The Logs screen shows `This line is no longer held` when `target` is
  `None` (see Review Focus 5 of the M4 plan).

### `log_count` and `logs_at`

Added in Milestone 5 for the Logs screen at ten million lines (a scrollbar needs a
total, and a row at a position, without walking to it).

A filter is **indexed** when it sets only `since_unix_nano`, `until_unix_nano`,
`levels` and `service` (`LogFilter::is_indexed()`): the conditions that the store
keeps counts for. A `min_severity`, `contains`, `trace_id`, `span_id` or `query`
makes a filter not indexed.

- **`Store::log_count(&LogFilter) -> Option<u64>`**: how many held logs the
  filter keeps, `Some` for an indexed filter and `None` for any other (a count
  of those needs the logs; page through `search_logs` for them). The count is
  exact, the same as the number of logs `search_logs` would list. It is worked out
  from the metadata of the chunks and reads only the chunks that a window edge
  cuts through, so with no window it reads nothing, and with one at most the two
  chunks of its edges (more only if chunks overlap in time there).
- **`Store::logs_at(&LogFilter, offset, limit) -> Vec<(LogId, LogRecord)>`**: up
  to `limit` of the logs the filter keeps, from position `offset` of the list
  newest first (the list `search_logs_keyed` pages through: position 0 is the
  newest match). Fewer come at the end of the list and none past it; `limit` 0
  gives none. For an indexed filter the position is found from the counts, and
  only the chunk or chunks that hold the position and the rows are read (see
  below). Any other filter is walked from the newest log with the filter applied
  to every log and the first `offset` matches skipped: it costs the logs before
  `offset` (and is slow past the first few thousand), and the Logs screen pages
  those the old way.

Both answer for the store as of the call, like every read: logs that arrive
meanwhile are not in a result, and the position of a log moves down by one with
each newer log that arrives.

### How logs are held

Since Milestone 5, in chunks: see "Log chunks" below. (Before that, one
`BTreeMap` keyed by `LogId`, with a `HashSet` of dedup keys and an index of the
held logs of each span and of each trace; the open chunk is that still. The plan
for Milestone 1 suggested a time-ordered `Vec` with an index of positions;
positions go stale when a late log inserts in the middle, and evicting from the
front of a `Vec` moves every record each time. The stable key avoids both, see
[`decisions/0002-log-storage-structure.md`](../decisions/0002-log-storage-structure.md).)

## Query filters

`TraceFilter::query` and `LogFilter::query` hold a parsed query behind an
`Arc` (the caller parses once and shares it; the store never parses). Both are
ordinary conditions: every other condition that is set must hold too, and
paging, ordering and cursors are exactly as without a query. The query
languages are in [`query.md`](query.md).

- **TraceQL.** A trace matches when the query matches at least one of its
  spans. The store builds a `TraceView` over the spans **as held** (arrival
  order, no copy) and calls `TraceQuery::evaluate`, so structural operators,
  `&&` and `||` between spansets, pipeline stages and the trace-level fields
  (`rootName`, `traceDuration`, ...) all work, judged on the spans the trace
  has now. The query runs after the cheap conditions and the service test and
  before the summary is built, so a trace the query rejects costs no summary.
- **`matched_span_ids`.** The summary of a matching trace lists the IDs of the
  first `MATCHED_SPANS_MAX` (20) matched spans, **in `evaluate` order**: the
  matched spans' positions in the held spans, ascending (arrival order, not
  start order). A trace with 500 matches lists 20. It is empty when the filter
  has no query, and `summary(id)` never fills it.
- **LogQL.** A log matches when `LogQuery::matches` is true for it. The exact
  `service_name="..."` of the selector (`LogQuery::service_hint`) is compared
  first, so a log of another service is skipped without evaluating the query.
  The scan order and the `since` cut-off are unchanged, so a query that matches
  rarely scans every log newer than `since`, like `contains` or `service`.

## Removal

```rust
store.remove_trace(trace_id)           // -> Removed { spans, logs }
store.remove_spans(trace_id, &[a, b])  // -> Removed { spans, logs }
```

`Removed` is `Clone + Copy + Debug + Default + PartialEq + Eq` with `spans` and
`logs`, the counts actually removed. The discard rules use these to drop stored
data when a rule matches (the data is gone for good, design §7).

- **`remove_trace`** removes the trace and **every held log that carries that
  trace ID**, whether or not the log has a span ID. It costs the trace's spans
  plus its logs, not the size of the store (a per-trace log index and a keyed
  order of arrival make that so). A trace the store does not hold removes just
  its logs (logs are stored independently of spans), and a trace with nothing
  removes nothing (`Removed::default()`).
- **`remove_spans`** removes those spans of one trace and **the logs emitted
  in them** (the logs whose trace ID and span ID both match). A span ID the
  trace does not hold is ignored for the span count, a span listed twice counts
  once, and the logs of an ID are removed even when the span itself is not held
  (logs and spans are stored independently, so the span may have been evicted
  or may never have arrived). A log that names the trace but no span is not any
  span's log and stays. The trace's start, duration, root and orphans follow
  the spans that are left: the children of a removed span become orphans until
  a span with that ID arrives again. Removing the **last span removes the
  trace** (the trace entry only: its trace-level logs stay). It costs the
  trace's spans, whatever `ids` holds.
- **Counters.** `traces`, `spans` and `logs` held go down. `spans_received`
  and `logs_received` do not change: they count what the store accepted, as
  with eviction.
- **Dedup forgets what was removed**, like eviction does: a retry of a removed
  span or log is accepted as new. Keeping such a retry out is the rules layer's
  job (its memory of dropped traces and spans).
- **Order of arrival.** A removed trace leaves the order of first arrival
  (eviction takes the oldest trace that is *held*), and one that comes back
  later is the newest.

## Scanning and reading in place

For code that has to look at stored data without copying it (the rules, and the
preview of a rule):

- **`with_trace_spans(id, f) -> Option<R>`** runs `f` on the spans the store
  holds for the trace, **in arrival order, not sorted, without copying**, and
  returns what `f` returns. `None`, and `f` does not run, if the trace is not
  held. Rules evaluate a whole-trace query over these spans followed by the
  spans of the batch being filtered.
- **`scan_traces(since, f)`** calls `f(trace_id, spans, status)` for every held
  trace whose **start is at or after `since`** (inclusive), in the order the
  traces first arrived, oldest first. `spans` are as for `with_trace_spans` and
  `status` is `classify`'s answer for them now (so a failing trace is `Failed`
  whatever the order the spans came in).
- **`scan_logs(since, f)`** calls `f(&log)` for every held log whose time (the
  log's timestamp, or its observed time when that is unset) is at or after
  `since`, oldest first.

All three run `f` **under the store's read lock**, so keep `f` short, and never
call the store from inside it: a writer waiting behind the read lock the caller
holds would deadlock the call. Copy what you need out and act after it returns
(this is also how a rule removes what it found: collect the IDs in the scan,
then `remove_trace`).

## Service map

```rust
let map = store.service_map(TimeWindow { since_unix_nano });
let services = store.services(TimeWindow { since_unix_nano });
```

The map is computed on demand from the traces held, on every call, over the
spans that **start at or after `since_unix_nano`** (there is no upper bound:
the window runs up to now). A trace that ended before the window is skipped
without looking at its spans.

```rust
enum NodeKind { Service, Database, Queue, External }       // serialized snake_case
struct ServiceNode { id: String, kind: NodeKind, requests: u64, errors: u64 }
struct ServiceEdge { from: String, to: String, calls: u64, errors: u64 }
struct ServiceMap { nodes: Vec<ServiceNode>, edges: Vec<ServiceEdge> }
```

Nodes are sorted by id, then kind (declaration order above); edges by
`(from, to)`. `from` and `to` are node ids, and every edge's ends are nodes
in the map (a map never has a dangling edge).

The rules (design §9.2, as the plan states them):

- **Service nodes.** One per `service()` that has a span in the window
  (`unknown_service` for a resource without `service.name`; nothing is
  dropped for lacking a name). `requests` counts the service's **server and
  consumer** spans, `errors` the erroring ones among them. Client, producer
  and internal spans are not entry spans, so they never count here.
- **Service-to-service edges.** A span whose parent is in another service
  gives an edge `parent.service -> span.service`, one call per child span,
  whatever the kinds of the two spans. It counts as an error if the parent
  or the child is erroring (once, if both are). It is **skipped when both
  spans carry `messaging.system`**: the queue sits between them (see below).
  A child whose parent has not arrived gives no edge until it does.
- **Databases.** A client span with `db.system.name` (or the legacy
  `db.system`; the new name wins) gives a Database node named by that value
  and an edge from its service. The edge's errors come from the client span.
- **Queues.** A producer span with `messaging.system` gives a Queue node
  named by it and an edge `service -> queue`. A consumer span with
  `messaging.system` gives an edge `queue -> service`, with the consumer span
  as the source of its errors. So a message that goes producer to consumer
  shows as `orders-api -> rabbitmq -> notifications-worker`, and never as a
  direct service-to-service edge.
- **External hosts.** A client span with no database or messaging attribute
  and **no direct child in another service**, in a **complete trace**, gives
  an External node named by `peer()` (`server.address`, else `peer.service`),
  or `unknown`, and an edge from its service. The completeness condition is
  there because the remote service's own span may still be on its way: until
  the quiet period passes, the call is not yet known to be unmatched. A new
  span makes the trace incomplete again and its external node disappears
  until it settles.

Choices the plan leaves open, and what the store does:

- **Both ends in the window.** A service-to-service edge is counted only if
  the parent and the child both start in the window. A call that began before
  the window and finished inside it is left out of the edges, though the
  child's span still counts in its own service's `requests`. This keeps every
  edge's end a node of the map.
- **Non-service nodes count calls received.** For Database, Queue and
  External nodes, `requests` is the number of calls that gave the node an
  incoming edge (database calls, producer sends, unmatched client calls) and
  `errors` how many of those were erroring. That matches what `requests`
  means on a service (work received), so the UI can show a rate on every node.
  A queue that only has consumers in the window exists, with 0 requests.
- **Names are non-empty strings.** An attribute whose value is empty or not a
  string is treated as absent (so a client span with an empty `db.system.name`
  is an unmatched client call, named `unknown`).
- **Ids are unique within a kind, not across kinds.** A broker instrumented as
  service `kafka` and a queue called `kafka` are two nodes with the same id
  (the service sorts first), and an edge to `kafka` does not say which. The
  store does not rename or merge them; callers that key nodes by id should key
  by `(id, kind)`. A test pins this.
- **Same-service parents.** A parent in the same service gives no edge.
  Parents and children are found by parent ID, never by time, so a child whose
  clock runs ahead of or behind its parent's still gets its edge. A span that
  is its own parent, or a parent cycle, cannot loop (only the direct parent is
  ever looked at).

Rates (§9.4: "Service error rate: the share of that service's entry spans
(server and consumer spans) in the time range that are erroring", and "Edge
error rate: the share of the calls along that edge whose client or producer
span is erroring") are `errors / requests` and `errors / calls`, computed by
the caller; a count of 0 means there is no rate to show. The plan's edge rule
also counts an erroring child, so a failing server whose client span did not
record the error still turns the edge red.

### `services`

`Store::services(window) -> Vec<ServiceStats>` is the Service nodes of the
map as a list, sorted by name: `name`, `requests` and `errors` as above, and
`last_seen_unix_nano`, the **latest start** of any of the service's spans in
the window. A service that only calls out (no server or consumer spans) is
listed with 0 requests. Dependencies are not services and are not listed. A
service with no span in the window is not listed. It is computed from the same
counts as the map (a test holds the two equal) but skips the dependency work.

## Segment files

Milestone 5 moves completed traces and sealed log chunks out of memory into
temporary files, and keeps the disk use under a cap (design §5). The building
blocks are two modules of this crate, `segment` and `run_dir`. Both are `pub`
and hidden from the generated docs (`#[doc(hidden)]`) only so that tests can
reach them; nothing outside the crate uses them, and `Store` is still the only
entry point. **Status:** the modules and their tests exist, and `Store` spills
traces and sealed chunks of logs through them (see "Hot and cold traces" and "Log
chunks"), and the disk cap deletes the oldest of the files (see "The disk cap").
The decision and its reasons are
[`decisions/0044-segment-file-format.md`](../decisions/0044-segment-file-format.md).

### The record

A segment is an append-only file of records, with no header or footer:

```text
[u32 len][u32 crc32 of the compressed bytes][zstd(postcard(payload))]
```

Both numbers are little-endian, `len` is the length of the compressed bytes,
and zstd runs at level 1. Record 0 starts at byte 0 and the next record starts
where the last one ended, so the size of the file is the sum of its records
(`Writer::bytes`, and what the disk cap counts). A `Location { seq, offset,
len }` finds a record again: the segment's number, the byte its header starts
at, and the length from its header (`Location::disk_bytes` adds the 8 header
bytes). Files are `traces-{seq:06}.seg` and `logs-{seq:06}.seg`; a number is
used once (`Writer::create` refuses an existing file).

- `Writer::create(dir, kind, seq)`, `append(&payload) -> Location`, `bytes()`,
  `seal()`. Each append is one write of a whole record, at the end of the file
  (`pwrite`), so a `Reader` opened at any moment sees whole records. A write that
  fails is taken back by cutting the file to the last good record, so the next
  append lands where its `Location` says; if even that fails the writer refuses
  more appends. Nothing is synced: segments do not outlive the run.
- `Reader::open(path)` maps the file (`memmap2`) and learns what it holds from
  its name. `read(location)` returns one payload; `iter()` walks the file in
  order and gives `(Location, payload)`. Open a segment that is sealed: the map
  covers the file as it was at `open`. An empty file opens and has no records.

### Damage

Every record is checked, and a record that fails is an error **value**, never a
panic (Review Focus 2 of the M5 plan):

| `SegmentError` | meaning | what `iter` does next |
|---|---|---|
| `Truncated` | the file ends inside the record's header or body (a crash mid-write), or a header promises more bytes than the file has | yields the error and stops: nothing can follow |
| `Checksum` | the body does not match its CRC32 (or the stored CRC itself is wrong) | yields the error and goes on with the next record |
| `Decode` | the checksum holds but the bytes are not zstd, not a blob, or `read` was given a `Location` whose length is not the record's | yields the error and goes on |

The first problem a `Reader` meets is printed once on stderr
(`loctel: <file>: the record at byte N: ...`), then only counted
(`Reader::problems`). Callers skip the record: loading and search treat it as
not there, and eviction still deletes the file whole.

### Blobs

`encode_trace(&[Span]) -> Vec<u8>` and `decode_trace(&[u8])`, and `encode_logs(&[LogRecordWithId])` and
`decode_logs`, make the payloads: a postcard struct of a **table of resources**
followed by the spans (or log records), each pointing at its resource by index.
So a service's resource is written once per blob, not once per span; a 15-span
checkout trace holds each of its services' resources once. Resources are the same
when they are equal by value, not only when they are the same `Arc`, because the
spans of a service arrive in many batches with a fresh `Arc` each. Decoding gives
the spans of one resource a shared `Arc` again. Everything else round-trips
equal, in the order given. A `LogRecordWithId` is a `LogRecord` with the `LogId`
it is held under (time and arrival number, written out as they are, not worked
out again). A blob that is cut short, or has bytes after it, is
`SegmentError::Decode`.

The shapes of the model types in postcard are the binary ones described in
[`architecture.md`](../architecture.md) ("Domain model"): IDs are their raw
bytes, and an attribute value is a tagged variant, so bytes and strings stay
different things.

### The run directory

`RunDir::create(base)` makes `<base>/run-<pid>-<start_unix_secs>/`; the app will
use `std::env::temp_dir()/loctel` as the base. It makes `base` when it is
missing, with mode `0700` (the directories hold copies of traces and logs).
Dropping the `RunDir` deletes the directory and everything in it.

A run that is killed leaves its directory behind, so `create` first deletes
every sibling directory named `run-<pid>-<anything>` whose pid is not a live
process (`kill(pid, 0)`: success or `EPERM` means alive). It is careful:

- only real directories with such a name go; a file, a symbolic link, another
  name, a pid that is not a number, 0 or too big for a pid stays;
- a directory that cannot be deleted is said on stderr and `create` goes on;
- two `RunDir`s of one process in one second (tests) get different names, the
  start time moving on by a second, and one run never deletes another's, since
  they share a live pid.

The known gaps are a pid that another process has taken over since (its
directory stays until that process ends) and a program that is always the same
pid, such as pid 1 in a container, which never finds its own old directory
stale. Neither loses data; both leave a directory in the temp directory.

Both modules are Unix only (`pwrite`, `kill`), like the rest of loctel (macOS and
Linux). The macOS build compiles `zstd`'s C sources; `script/check-macos.sh` has
no Apple SDK to do that with and compiles them with a stand-in
(`script/stub-cc.sh`), see decision 0044.

## Hot and cold traces

With a `spill_dir` in the config (the app passes its run directory; tests and
`--no-spill` runs leave it `None` and nothing below happens), a trace is in one
of two places:

- **hot**: its spans are in memory, in the store's map of hot traces. A trace is
  hot while spans are still arriving, for `completeness_quiet` after the last
  one, and for `spill_after` more.
- **cold**: its spans are one record in a traces segment file (an encoded span
  list, see "Blobs"), and only the index entry stays in memory.

Nothing outside the crate can tell which a trace is, except that
`Store::spilled_traces()` counts the cold ones. Every method gives the same
answer for a cold trace as for the same trace hot (the test files run each body
in both modes to hold that, see "Tests"); the rest of this section is how.

### The trace index

Every held trace, hot or cold, has an entry in the index
(`src/trace_index.rs`, `TraceMeta`): its ID, the order it first arrived in, the
earliest start, the latest end, the span count, `last_arrival` (the clock time
its last new span arrived) and where it is (`Hot`, or `Cold` with the position of
its record). Its **status** (did the request work) is there too, kept up to date for
a trace in memory and fixed for a cold one. A cold trace's entry also holds the
**summary it had when it left memory**: its services in the order of their first span
(the service set), status, failed-in span, first exception and root. Nothing in a cold
trace can change, since a new span brings it back to memory first, so that summary
stays true.

The index orders its entries three ways: by start (a `BTreeSet<(start, id)>`,
walked backwards for "newest first" and for cursors), by first arrival (what
eviction and `scan_traces` use) and, for the hot traces, by `last_arrival` (what
the spill step uses). So these cost the traces they look at, not the traces held:

| call | what it reads for a cold trace |
|---|---|
| `summary` | the index entry |
| `search_traces` (time, service, errors only, duration, paging) | the index entry |
| `recent_failures` | the index entries, newest first, and the summaries of the traces it returns |
| `search_traces` with a TraceQL `query` | the index entry for every candidate, and the record for the ones the other conditions kept, newest first, until the page and one more match are found |
| `oldest_start_unix_nano` | the index (the first key) |
| `stats` | counters |
| `trace`, `with_trace_spans` | the record (through the cache) |
| `scan_traces` | the record, for every trace from `since` on (not cached) |
| `service_map`, `services` | the record, for every trace that reaches into the window (not cached) |
| `remove_trace`, eviction | the index entry only; the record is marked dead |
| `remove_spans` | the record (through the cache), to see whether the trace has any of the spans |

The two last "not cached" passes read every cold trace they touch, so the service
map over a window that covers a lot of cold data decodes a lot of it, on every call
(the window's refresh is every 2 s). That is the cost of keeping the cold
traces' spans out of memory; a trace that ended before the window is never read.

### The spill step

A trace goes from hot to cold in the **spill step**. It runs at the end of every
`ingest_spans` and `ingest_logs`, and ahead of every call about traces (`trace`,
`summary`, `search_traces`, `with_trace_spans`, `scan_traces`, `service_map`,
`services`, `oldest_start_unix_nano`, `remove_trace`, `remove_spans`) and of `stats`,
so that a store that nobody writes to gives its memory back too. It runs at most once a second by the
store's clock (every call in the test config, whose `spill_after` is 0: the interval
is the smaller of a second and `spill_after`). The step takes the hot traces
oldest first, by when their last span arrived, and spills one when

- it has been quiet for `completeness_quiet + spill_after`, or
- the hot traces hold more than `hot_max_spans` spans and it is complete (quiet for
  `completeness_quiet`), without waiting for `spill_after`.

It stops at the first trace that is neither, because the traces after it arrived
later. A trace that is still being written is never spilled. Spilling encodes the
trace, appends it to the open traces segment, and swaps its index entry to `Cold`
with the summary computed from the spans. The work is done under the store's write
lock, a trace at a time.

If a write fails (the disk is full, the directory is gone) the trace stays hot, the
step ends, and the store waits before it writes again and says why
(`StoreStats::spill_error`, see "A failing disk"). The store never panics over it.
Memory then grows with the data, up to `max_traces`, as it did before M5.

### Rehydration

A span for a cold trace, a slow service or a retried exporter, brings the trace back:

1. The trace is read (through the cache) and, if it already has a span with that ID
   (a retry), **nothing changes** and the trace stays cold.
2. Otherwise it becomes hot again, with the first arrival and `last_arrival` it
   had, and its old record is **dead**. Then the span is added as to any hot
   trace, which moves `last_arrival` to now: the trace is **incomplete again**, and
   will spill again when it goes quiet. `stats().spans` does not count the trace
   twice (the spans were counted when they first came), and `spans_received` goes
   up by the one new span.
3. A cold trace that cannot be read (its record is damaged, see "Damage") is
   forgotten: its spans leave the counts, and the new span starts a new, partial
   trace, as for a trace that was evicted.

`remove_spans` does the same, with one difference: it is not a new arrival, so the
trace keeps its `last_arrival` (it stays complete and is spilled again by the
next step) and a cold trace that has none of the spans is not brought back at all.

### Dead bytes and segments

A record cannot be taken out of its file. When its trace comes back to memory or is
removed or evicted (or, for a chunk of logs, evicted whole or written again
without logs a rule removed), the record is **dead**: still on disk, counted in the disk
total (`Store::spill_disk_bytes`, hidden for now) and in `spill_dead_bytes`, until
its whole segment is deleted. The disk cap deletes the oldest segment whole, which
takes its traces and its log chunks with it (see "The disk cap"); a trace never
straddles two segments.

`src/segment_set.rs` keeps the books. One segment is open for writing for each kind of
record; when an append makes it reach `segment_max_bytes` it is finished and the next
append opens a new one. Segment numbers count up over all kinds and are never reused
(not even after `clear`), so the order of the numbers is the order of the files'
age. A segment is memory-mapped the first time one of its records is read, and
mapped again when the file has grown past what the map covers (reading a record of
the segment still being written). `clear` and dropping the last clone of the store
delete the store's files.

### The cache of decoded traces

`trace`, `with_trace_spans` and the checks of `remove_spans` and of a late span
decode a cold trace and keep the result in a cache of the last 64 decoded traces
(and at most a million spans in all, so that a few huge traces cannot fill memory
behind a small cache). It is keyed by the position of the record, so an entry can
never be mistaken for a later version of its trace, and it forgets a trace's entry
when its record dies. A search, `scan_traces` and the service map read through the
cache without adding to it: they look at many traces once, and must not push out
the trace the window has open.

A hidden `Store::trace_decodes()` counts the traces decoded from a segment (a cache
hit is not one), which is how the tests show a search stops early.

## Log chunks

The logs are a list of **chunks** (`src/log_chunks.rs`, the questions put to them
in `src/logs.rs`), with or without a `spill_dir`:

- The **open chunk** takes every new log, whatever its time. It is in memory, in
  time order, as the logs were before Milestone 5 (a `BTreeMap` keyed by `LogId`,
  the dedup keys, and an index of the held logs of each span and of each trace so
  that removing a trace's logs is a lookup).
- A **sealed chunk** is a closed open chunk: its logs sorted by `LogId` in one
  immutable record, of a logs segment file when the store has them (the format of
  "Blobs": one resource table, then the logs) and in memory as they were when it
  has not, or when the write failed. A sealed chunk keeps no index of spans or
  traces: it has the metadata below, and its record is read when it is needed.

Nothing outside the crate can tell which a log is in (the hidden
`Store::log_chunks()` says how many chunks there are and how many are on disk, for
tests), and every method of the store gives the same answer in both modes: the
test config seals a chunk every 7 logs (`log_chunk_max`), so every log test of the
crate runs over sealed chunks.

### When a chunk seals

- **At `log_chunk_max` logs** (65,536): the log that makes it full seals it, in the
  same `ingest_logs` call (so a batch can seal several).
- **At `log_chunk_age`** (10 s) on the store's clock since the chunk's **first**
  log, so that a store nobody writes to still gives its last logs to the disk. It
  is checked at the start of every `ingest_logs` (the batch then starts a chunk of
  its own) and by the spill step, which runs ahead of reads at most once a second,
  and runs with or without segment files; a chunk with no log in it never seals.

Sealing happens under the write lock: build the metadata (already kept up to
date), sort (already in order), encode, compress and append one record. A write
that fails keeps the chunk in memory and sets the spill error (see "Hot and cold
traces"); the chunk is not moved to disk later by the store yet.

### Late logs: chunks overlap

A late log (a slow exporter) goes to the open chunk, in its place in the open
chunk's time order. So a chunk is not a range of time: the open chunk, and a
sealed one that took late logs, can overlap the chunks next to them, and with a
batch exporter that delays its logs by a few seconds every pair of neighbouring
chunks overlaps a little. Everything that lists logs is therefore a **merge**:

- `Cursor` (in `log_chunks.rs`) walks the chunks as one list, newest first or
  oldest first, between two ids. It is a heap that has an entry per chunk that can
  have a log in range, ordered by the next log the chunk would give; a sealed
  chunk is **read only when its entry comes to the top**, so a page of the newest
  logs reads one chunk (two at an edge) and the other chunks cost one entry each.
  It lends each log until the next call (`next` returns a reference into the chunk,
  not a copy), and the search clones only the logs it returns.
- Search, `logs_around`, `log_histogram` for a filter that is not indexed,
  `logs_at` and `scan_logs` use it. `Store::log` and `logs_for_span` look at the
  chunks whose range or traces can hold the answer.
- Paging is unchanged: the cursor is a `LogId`, and a page starts strictly below
  it, wherever the logs below it are.

### What is kept about a chunk

Every chunk, the open one too, has a `ChunkMeta`, exact at every moment (inserts,
eviction and removal update it, and a test compares it with a plain model after
every operation of a random run):

| Field | Meaning |
|---|---|
| `first`, `last` | the oldest and the newest `LogId` held: the chunk's range |
| `count` | logs held |
| `level_counts[7]` | logs at each level (`Severity` order, the bit order of `LevelSet`) |
| `service_counts` | logs from each service (services are numbered in the order they are first seen: `ServiceIdx`) |
| `cells` | logs per **second, service and level**, sorted, none of them 0 |
| `traces` | the traces that have a log in a sealed chunk, sorted (a superset after trimming, never missing one) |

The plan's per-second histogram has a count and an error count per second. The
cells are that, with the service and the level kept in them: an indexed filter may
name both, and counting a chunk exactly for "these levels from this service"
needs the two together (`level_counts` and `service_counts` answer a filter that
names only one of them, and let a chunk be passed over). A cell's second is the
log's time in whole seconds, as a `u64`, so there is no range to overflow. A chunk
holds about as many cells as it has seconds times services times levels in use
(hundreds for a chunk of ten seconds; a chunk of logs spread over many hours of
history has more).

### Counts, positions and the histogram from the metadata

For an **indexed** filter (see "`log_count` and `logs_at`") the three questions are
answered from the metadata where the metadata is exact, and a chunk is read only
where it is not. The answer is always what a pass over the logs gives.

- **`log_count`.** A chunk that lies wholly inside the window (`first` at or above
  `since`, `last` below `until`) counts as `count_matching`: `count` with no
  condition, the sum of `level_counts` for the levels, `service_counts` for the
  service, and the cells for both. A chunk the window does not touch counts 0, and
  one that a window edge cuts through is read and its logs in the window counted.
- **`logs_at`.** Let `at_or_above(b)` be the number of matching logs with id `b` or
  above, from the same counts. The ids where it can change are the first id of each
  chunk and the id after its last, so a binary search over those, `at_or_above`
  being non-decreasing as the id goes down, finds the last of them that has no more
  than `offset` matching logs at or above it. The position is below it: the walk
  starts there with `offset - at_or_above` matches left to step over, and then gives
  `limit` rows. When chunks do not overlap, no chunk is cut through by any of those
  ids and **the search reads nothing**; the walk then reads the chunk the position
  is in, and the next one if the rows run over its edge: at most two. When chunks
  overlap, the chunks an id falls inside are read, which are the ones that overlap
  there. Offset 0 skips the search. The rows are read newest first, each tested
  against the levels and service, so the step over matches costs the matching logs
  between the start of that chunk and the position: up to a chunk's worth.
- **`log_histogram`.** A chunk inside the window and inside **one bucket** (its
  first and last log fall in the same) adds its whole counts to that bucket. A chunk
  inside the window that spans buckets adds its **cells** if each cell (a second of a
  service at a level) can only fall in one bucket, which is true when no bucket edge
  falls inside a second that has logs. A chunk at the edge of the window, or with a
  bucket edge inside one of its seconds, is read and its logs put in their buckets
  one by one. So the bars are exactly a scan's, and nothing is read when the window,
  the bucket width and the chunks' seconds line up: **a window and a bucket count
  that make every bucket a whole number of seconds, starting on a whole second, are
  answered from the metadata alone**, and any other window reads each chunk that has
  a bucket edge inside it (up to one per bucket, 47 for the usual 48, though the
  cache only holds eight). The Logs screen should snap its window to whole seconds.

A filter that is not indexed gets none of this: search and `logs_at` walk and test
every log (skipping the chunks the metadata rules out), `log_count` is `None` and
the histogram reads every chunk in the window.

### Eviction

`max_log_records` still means exactly that: past it, the oldest logs by `LogId`
go, after the whole batch is in. The oldest log is the first of one chunk, found by
looking at the `first` of each; a run of the oldest chunk's logs older than the
next chunk's first goes together.

- In the **open chunk** the logs are popped as before.
- A **sealed chunk** that loses only some of its logs is **trimmed from the front**:
  its record is not written again; its first `skip` logs are counted out of the
  metadata (the chunk is read, once, to know their levels and services; it is in the
  cache from then on) and `first` moves. It is a few hundred nanoseconds a log
  evicted, not a rewrite of 65,536.
- A chunk that loses all its logs is dropped, its record dead (counted in the disk
  total until its segment goes, see "Dead bytes and segments"). One that cannot be
  read is dropped whole as soon as it is the oldest, whatever it counted.

The dedup keys of evicted logs are forgotten (recomputed from the logs, which is
why eviction reads the chunk), so a retry that arrives after its original was
evicted is taken as new, as before.

### Removal

`remove_trace` and `remove_spans` take the logs out of the open chunk by its
indexes as before, and out of each sealed chunk whose `traces` list has the trace:
the chunk is read, its logs that match are removed (their dedup keys forgotten),
the metadata is built again from the rest, and the rest is **written as a new
record** (the old one dead), or the chunk dropped if nothing is left. That costs
the size of the chunk, per chunk that holds a log of the trace; the discard rules
remove what has just arrived, which is in the open chunk. `logs_for_span` and the
removals find a trace's logs in a sealed chunk only through that chunk's `traces`
list, which is why a sealed chunk keeps it.

### The cache of decoded chunks

A sealed chunk's logs are decoded once and kept in a cache of the last **8** chunks
(`CHUNK_CACHE`, the least recently used goes first), keyed by the position of the
record. Counts, `logs_at`, search, `Store::log`, `logs_around`, `logs_for_span`,
the histogram and the eviction read through it and add to it; `scan_logs` (the
rules' preview, which reads every chunk once) and the removals read through it and
do not add. A chunk that is in memory (no
segment files, or a failed write) is never decoded and never in the cache. The
hidden `Store::log_chunks()` has the number cached and the number of decodes, which
is how the tests show how few chunks a call reads.

### Damage and failed writes

A record that cannot be read (checksum, truncated, undecodable) leaves its chunk
**in the list**: reads skip it (the first problem of a segment is said on stderr,
once, as for traces), and its logs are still in `stats().logs` and in the counts
until the chunk is evicted, which drops it whole as the oldest. Nothing panics. A
write that fails (the disk is full, the directory is gone) leaves the chunk in
memory and the error in `spill_error`, and ingest goes on; a later seal that
works clears it. Such a chunk **stays in memory** for good, even when the disk
works again (the logs are bounded by `max_log_records`, and the chunks sealed from
then on go to disk): see "A failing disk" for what waits and for how long.

### What it costs

One-off figures from the development container (4 vCPU, release build, **not a
gate**; the performance budgets are Task 6 of the milestone): 10,000,000 logs of 8
services, 70-byte bodies, a trace and a span ID on each, in 152 sealed chunks and
38,528 open, ingested in batches of 1,000 with segment files on:

| | |
|---|---|
| ingest | 530,000 logs/s on one thread; resident memory 230 MB afterwards |
| sealed chunks | 186 MB on disk (the bodies are repetitive); one chunk is read in about 33 ms: 6 ms of zstd, 20 ms of postcard and 12 ms making the logs |
| `log_count`, levels and a service, no window | 0.17 ms |
| `log_count` with a window cutting through two chunks | 73 ms (two reads) |
| `logs_at`, 50 rows at a random position | p50 32 ms, p95 about 40 ms: one chunk read; 2 ms when it is in the cache |
| `log_histogram`, 48 buckets of whole seconds | 0.24 ms |
| `log_histogram`, 48 buckets with edges inside seconds | 1.7 s (every chunk with an edge in it is read) |
| the first page of a LogQL query for one service | 0.8 ms |

What a chunk costs to read is most of every slow row: Task 6 of the milestone
(budgets) and Task 7 (the Logs list) work from it. Reading less of a chunk (a
record of independent blocks with an index of their first ids) or faster, are the
two ways to make `logs_at` fast; the window of the Logs screen can snap to whole
seconds for the histogram.

## The disk cap

The files of a store that spills take at most `disk_cap_bytes` (4 GiB by default,
the `Settings::disk_cap_bytes` of the app, changed while loctel runs with
`Store::set_disk_cap`). **Disk bytes** are the sizes of the segment files, so they
include the dead records (see "Dead bytes and segments"): a record is not
taken out of its file, the file goes whole.

After every call that can write (`ingest_spans`, `ingest_logs`, `remove_trace`,
`remove_spans`, and the spill step that runs ahead of the reads) and at once when
`set_disk_cap` is called, the store compares the disk bytes with the cap. Over it,
it deletes **the oldest segment file** (the lowest number: numbers count up over
traces and logs, so the order is the order of the files' birth), then the next
oldest, until the files take no more than the cap. Deleting a segment forgets what
was in it, whole:

- every **cold trace** whose record is in the file leaves the index, as if it had
  been evicted (`traces` and `spans` go down by what it held, a late span for it
  starts a new trace, `spans_received` does not change). A trace is one record in
  one file, so a trace is there whole or not at all;
- every **sealed chunk of logs** whose record is in the file is dropped. Each is
  read first to forget the dedup keys of its logs, as eviction by count does, so a
  retry of a log that went is taken as new. That costs a read and decode of every
  chunk in the file (about 30 ms a chunk, in the development container: a 64 MiB
  segment is tens of chunks), under the store's write lock, once per file
  deleted: the cap only costs this when the disk is full of data, once per 64 MiB
  written. Chunks that are in memory (a failed write) are in no file and stay;
- the file is deleted, with its map, and its writer if it was the one being
  written (the next write starts a file).

Nothing is deleted that the cap does not require, nothing out of order (a file with
nothing but dead records is not deleted early), and nothing is left over: the
disk bytes, the files and `StoreStats` agree after the call (`disk_cap.rs`
checks all three after every cap it sets). A cap too small to hold one record
keeps nothing on disk: whatever is written goes at the end of the call. A store
with no `spill_dir` has no files and `set_disk_cap` only remembers the number.

Deleting needs no new space, so the cap works on a full volume, which is how space
comes back.

## A failing disk

A write to a segment can fail: the volume is full (`ENOSPC`), the directory is gone
(someone cleaned the temporary directory), the disk fails. Review Focus 4 of the
plan: the store stops spilling, keeps ingesting in memory, says so, and tries
again later; it never panics. Concretely:

- The failed write leaves a trace **hot** (it is tried again, not lost) and a
  sealed chunk of logs **in memory**.
- The reason becomes `StoreStats::spill_error` (`"No space left on device (os error
  28)"` for a full volume), and is printed on stderr once, not once per failed write.
- **Writes are suspended** for `spill_retry` (60 s): every append fails at once,
  without trying the disk, with the same reason, so a store with a full disk does not
  spend its time on writes that fail (a seal of 65,536 logs would be encoded and
  refused every time). The first call after the minute resumes writes: the step
  tries the oldest hot trace that is ready, a seal tries its chunk. A write that
  fails again suspends them for another minute.
- What cannot be written stays in memory, **up to the limits that always applied**:
  `max_traces` evicts the oldest trace whole, `max_log_records` the oldest logs. It
  does not grow without bound, and ingest, search and every read go on working.
- A write that works clears the error (a trace spilled, or a chunk written), and
  from then on every complete trace spills again. The chunks of logs whose
  write failed stay in memory until they are evicted: they are not written
  later, which is a bounded cost (`max_log_records`) that keeps the store simple.
- **The error is cleared by a write**, not by a probe: a store with nothing to write
  keeps the error until it has. `clear` forgets it (everything is gone, and the files
  with it).
- Deleting still works, so the disk cap goes on, and lowering it is a way to make room
  (but the files of the run are all the store can free: the operator frees the rest).

The hidden hooks `Store::inject_write_failure(bool)` (every write fails as a full
volume's does) and `Store::failed_write_attempts()` (how many writes tried the disk
and failed) are how `disk_cap.rs` tests this without a full volume.

## Tests

`crates/loctel-store/tests/`:

- `modes.rs`: the both-modes harness. `each_mode(|store, clock| { ... })` runs a
  body on a default store in memory and then on one that spills
  (`StoreConfig::spilling_for_tests`), each on its own manual clock;
  `each_mode_with(config, ...)` does it with the limits of a config. Every test
  file of this crate that builds a store runs its bodies through one of them
  (`fixtures::store_with_manual_clock` and `store_with_config` stay for the tests that
  want one store of their own), so the public API is held equal in both modes.
  The spilling store only spills what the plan says it may, a *complete* trace, so a
  body that never moves the clock past the quiet period never leaves memory in
  either mode: the cold paths are pinned by `spill.rs`, and by the bodies that
  advance the clock (the completeness tests, the service-map tests). **Logs are
  different**: `spilling_for_tests` seals a chunk of logs every 7 logs, so the log
  tests meet sealed chunks, chunks that overlap and trimmed chunks in every run.
  A file declares `mod fixtures; mod modes;` (`modes.rs` has its own clock start,
  which `spill.rs` checks against `fixtures::CLOCK_START`).
- `spill.rs`: Milestone 5, Task 2. The settings and `spilling_for_tests`; what
  spills and when (`complete_trace_spills_and_reads_back_equal`, the hot-span
  budget, segments rolling, `clear` and drop deleting the files, reads of a
  segment that is still growing); `late_span_rehydrates_cold_trace` and its
  neighbours (a retry changes nothing, a late span that starts earlier moves the
  trace in the order); `search_order_and_cursor_identical_across_modes` over 60
  traces, hot and cold; `traceql_search_decodes_newest_first_and_stops_at_page`
  (the decode hook); `service_map_identical_across_modes`;
  `remove_trace_on_cold_trace`, `remove_spans_on_cold_trace_keeps_it_complete`
  and eviction of a cold trace; a damaged record (skipped, never a panic); a write
  that fails (the trace stays hot, and the next step retries); and readers and a
  writer on one spilling store. Most compare a store in memory
  with one that spills, fed the same data on the same clock. Task 5 adds the
  bounded-work tests, with the `summaries_built` hook:
  `search_first_page_builds_at_most_limit_plus_one` (every kind of filter, in both
  modes), `a_search_builds_a_summary_only_for_what_it_returns`,
  `recent_failures_one_pass_equals_a_full_search` (the same lists as a full search
  split by status, for many limits and `since`s, hot and cold),
  `recent_failures_builds_nothing_for_a_list_that_is_full` and
  `services_reads_only_the_traces_that_reach_the_window` (the decode hook). The
  status a trace keeps in the index is held equal to `classify` by unit tests in
  `src/trace.rs`.
- `logs_scale.rs`: Milestone 5, Task 3. When a chunk seals (`chunk_seals_at_count_and_time`:
  65,536 logs or ten seconds, not a nanosecond early); late logs and chunks that overlap
  (`late_log_lands_in_time_order_across_chunks`, and 20,000 logs three seconds out of
  order); `log_count_indexed_matches_scan` (200 random filters over 200,000 logs, against
  a count made from the logs themselves) and how few chunks it reads;
  `log_count_none_for_body_filter`; `logs_at_matches_paged_search` (offsets at the edges of
  chunks, with and without a filter, against a keyed walk), at most two chunks read when
  chunks do not overlap, and filters that are not indexed; `histogram_from_meta_equals_scan`
  (the same bars as a scan and as an oracle, nothing decoded for whole-second buckets, odd
  windows exact too); `chunk_cache_bounded`; eviction trimming chunks, removal rewriting
  them, retries of sealed logs; `clear`; chunks skipped by service and trace; `scan_logs`
  order; a damaged chunk and a failed write; readers and a writer; and
  `random_operations_agree_with_a_plain_model`, which runs a hundred random ingests (late
  logs, retries, trace context), removals and clock steps against a plain list and compares
  every read of the store with it after each one, for chunk sizes from 3 to 500, with and
  without segment files.
- `disk_cap.rs`: Milestone 5, Task 4. `cap_evicts_oldest_segments_whole` (the files that
  go are the oldest, the newest traces stay whole, it stops as soon as it is under the cap),
  `the_cap_holds_while_data_arrives`, `log_segments_go_with_their_chunks` (and the
  dedup keys with them), `lowering_cap_evicts_now_and_keeps_stats_consistent` (traces and
  logs mixed, caps down to one byte, every counter and the files agree, then it works
  again), a cap smaller than a record, a store in memory;
  `spill_write_failure_keeps_ingesting_and_reports` (a volume that fills up: nothing
  panics, memory is held to its limits, one attempt a minute, the error clears when a write
  works) and the cap on a failing disk.
- `fixtures.rs`: `span(...)`, `error(span)`, `with_kind`, `with_attr`,
  `with_exception`, `log(...)`, `with_severity`, `in_span`, `in_trace`,
  `tid(n)`, `sid(n)` and the `store_with_manual_clock()` builders every store
  test file shares, plus `scenario_spans` and `scenario_logs` (the fake mesh's
  data through the real OTLP conversion, which is why `loctel-meshgen` and
  `loctel-ingest` are dev-dependencies; give each trace of a test its own seed,
  because one seed gives every scenario the same IDs).
- `assembly.rs`: the plan's Task 3 tests. `duplicate_span_is_ignored`
  (exporter retries) and `skewed_child_still_assembles` (clock skew) are the
  review-focus tests this crate owns.
- `edge_cases.rs`: ordering, root choice, orphans, dedup and completeness
  corner cases, eviction order and batching, clear, sharing across threads.
- `clock.rs`: the clocks and the config defaults.
- `failure.rs`: the plan's Task 4 failure tests plus the corners above: the
  tie-break, skewed children, peers, the exception choice, and malformed
  parents (self-parent, cycle, a 20,000-deep chain).
- `search_traces.rs`: the plan's search tests plus paging corners (cursor
  format, equal starts, new traces between pages, garbage cursors) and the
  summary fields.
- `query_filters.rs`: the plan's Task 4 query tests on the fake mesh
  (CheckoutFailure and CheckoutSuccess, HugeTrace) plus structural queries,
  paging past traces a query rejects, the exact cap of 20, combining a query
  with each other condition (traces and logs), LogQL selectors with and without
  a service hint, the JSON of `matched_span_ids` and filter equality.
- `removal.rs`: `remove_trace`, `remove_spans`, the order-of-arrival and log
  indexes after removal (eviction still works), dedup after removal,
  `with_trace_spans`, `scan_traces` and `scan_logs`.
- `logs.rs`: the plan's Task 5 log tests, plus paging (cursor, equal
  timestamps, filters while paging, garbage cursors), late arrivals, the
  observed-time fallback, each filter, the span link, eviction corners
  (whole batches, late old logs, a zero cap, retries after eviction) and
  counting and clearing. `duplicate_log_is_ignored` (exporter retries) is the
  review-focus test this file owns for logs.
- `segment.rs`: the segment files and the run directory (Milestone 5, Task 1):
  appending and reading back, the file names, the trace and log blobs (each
  service's resource written once, by value), a flipped bit
  (`checksum_mismatch_is_reported`), a record that is not zstd, a file cut off
  at every kind of place (`truncated_record_is_skipped` is the review-focus
  test for a crash mid-write), an empty file, and the run directory deleting its
  stale siblings (and only those) and itself.
- `service_map.rs`: the plan's Task 5 map tests (`unknown_service_appears_in_map`
  is the review-focus test for a missing `service.name`), plus each rule's
  corners: error counting, messaging skip, consumer-only queues, external
  naming and completeness, windows (including both-ends-in-window and exact
  boundaries), sorting, serialization, services equal to the map's service
  nodes, name collisions, and malformed parents (self-parent, cycle, a
  10,000-deep chain).
