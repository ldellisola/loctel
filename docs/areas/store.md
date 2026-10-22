# Store (`loctel-store`)

The in-memory store behind loctel. It takes spans in any order from any
service, assembles them into traces, says when a trace has gone quiet, and
evicts the oldest trace whole when it holds too many. It also holds log
records, linked to spans by ID, and works out the service map and the services
list from the traces, filters both by TraceQL and LogQL queries, and can
remove traces, spans and their logs again (for the discard rules). Design references: §4 (order and lateness, completeness,
logs), §5 (storage, eviction, clear all), §9.2 (the map) and §9.4 (what
"failed" means).

Everything outside the crate goes through `Store`. M5 replaces the internals
with a spill-to-disk segment store, so keep the public API below stable and
keep callers away from anything else.

Status: trace assembly, completeness, dedup, eviction, failure
classification, trace summaries, trace search, logs, the service map and the
services list are implemented. So are (Milestone 2) the TraceQL and LogQL query
filters, matched spans, removal and the scans (see "Query filters", "Removal"
and "Scans" below).

## API

```rust
let clock = Arc::new(ManualClock::new(start_unix_nano)); // or SystemClock
let store = Store::new(StoreConfig::default(), clock.clone());

store.ingest_spans(spans);               // Vec<loctel_model::Span>
let snapshot = store.trace(trace_id);    // Option<TraceSnapshot>
let summary = store.summary(trace_id);   // Option<TraceSummary>
let page = store.search_traces(&TraceFilter::default(), &PageRequest { limit: 20, cursor: None });
store.ingest_logs(logs);                 // Vec<loctel_model::LogRecord>
let page = store.search_logs(&LogFilter::default(), &PageRequest { limit: 50, cursor: None });
let lines = store.logs_for_span(trace_id, span_id); // Vec<LogRecord>, oldest first
let map = store.service_map(TimeWindow { since_unix_nano }); // ServiceMap
let services = store.services(TimeWindow { since_unix_nano }); // Vec<ServiceStats>
let stats = store.stats();               // StoreStats
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

- `StoreStats`: `traces`, `spans` and `logs` are what the store holds now.
  `spans_received` and `logs_received` are lifetime totals of what it
  accepted. See "Counting" below.

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

- It is computed on read from the store's clock. Nothing runs in the
  background, and the span timestamps play no part: it is about when the
  spans arrived, not when they happened.
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

Cost: the cheap conditions (`since`, `min_duration`, the cursor) use the trace
start and duration that each entry keeps up to date, so they need no pass over
the spans. The survivors are sorted, and the remaining work (service
membership, the query, classification, the summary) is done newest first only
until the page is full. The read lock is held for the call. M5 replaces this with an
indexed segment store behind the same API.

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
| `service` | logs from that service (exact name; `unknown_service` for logs without one) |
| `min_severity` | logs of that severity or higher. `Unspecified` is the lowest level, so any `Some(level)` above it drops unlabelled logs |
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

Cost: the scan runs from the newest log and stops when the page is full or
the logs are older than `since`, so a recent page costs about its size. A
filter that matches rarely (`contains`, `service`, `trace_id`) scans every log
that is newer than `since`. On the development container (one-off, release
build, 1,000,000 logs held), a search that matches nothing took about 0.12 s,
and ingest at the cap with eviction ran at roughly 600,000 logs/s. M5 replaces
this with an indexed segment store behind the same API.

### `logs_for_span`

`Store::logs_for_span(trace, span) -> Vec<LogRecord>` returns the logs whose
`trace_id` and `span_id` both equal the arguments, **oldest first**. A log
that has a trace ID but no span ID is not linked to any span (find it with
`search_logs` and `trace_id`). Logs and spans are stored independently, so the
call works before the span arrives and after its trace is evicted. It reads a
per-span index, so it costs the number of logs the span has.

### How logs are held

A `BTreeMap` keyed by `(time, arrival number)`, a `HashSet` of the dedup keys
of the held logs, and two indexes of those keys, each kept in time order: one
per span (`logs_for_span`) and one per trace for every log that carries a trace
ID, with or without a span ID (so a whole trace's logs can be removed without a
pass over every log). Eviction takes the oldest log, which is at the front of
both of its indexes; removal keeps them in step. The
plan suggested a time-ordered `Vec` with an index of positions; positions go
stale when a late log inserts in the middle, and evicting from the front of a
`Vec` moves every record each time. The stable key avoids both. See
[`decisions/0002-log-storage-structure.md`](../decisions/0002-log-storage-structure.md).

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

## Tests

`crates/loctel-store/tests/`:

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
- `service_map.rs`: the plan's Task 5 map tests (`unknown_service_appears_in_map`
  is the review-focus test for a missing `service.name`), plus each rule's
  corners: error counting, messaging skip, consumer-only queues, external
  naming and completeness, windows (including both-ends-in-window and exact
  boundaries), sorting, serialization, services equal to the map's service
  nodes, name collisions, and malformed parents (self-parent, cycle, a
  10,000-deep chain).
