# Store (`loctel-store`)

The in-memory store behind loctel. It takes spans in any order from any
service, assembles them into traces, says when a trace has gone quiet, and
evicts the oldest trace whole when it holds too many. Design references: §4
(order and lateness, completeness), §5 (storage, eviction, clear all).

Everything outside the crate goes through `Store`. M5 replaces the internals
with a spill-to-disk segment store, so keep the public API below stable and
keep callers away from anything else.

Status: trace assembly, completeness, dedup, eviction, failure
classification, trace summaries and trace search are implemented. Logs and the
service map are added by later tasks and get their own sections here.

## API

```rust
let clock = Arc::new(ManualClock::new(start_unix_nano)); // or SystemClock
let store = Store::new(StoreConfig::default(), clock.clone());

store.ingest_spans(spans);               // Vec<loctel_model::Span>
let snapshot = store.trace(trace_id);    // Option<TraceSnapshot>
let summary = store.summary(trace_id);   // Option<TraceSummary>
let page = store.search_traces(&TraceFilter::default(), &PageRequest { limit: 20, cursor: None });
let stats = store.stats();               // StoreStats
store.clear();
```

- `Store` is `Clone + Send + Sync`. Clones share the same data (an `Arc`
  around an `RwLock`), so give each thread or task its own clone.
- `ingest_spans` takes the write lock once per batch. `trace`, `summary`,
  `search_traces` and `stats` take the read lock.
- `Clock` is `trait Clock: Send + Sync { fn now_unix_nano(&self) -> u64 }`.
  `SystemClock` is the wall clock. `ManualClock::new(start)` only moves when
  told: `advance(Duration)` (saturating) and `set(u64)` (either direction).
  Tests drive completeness with it instead of sleeping.
- `StoreConfig` and its defaults:

  | Field | Default | Meaning |
  |---|---|---|
  | `max_traces` | 200,000 | past this, the oldest trace is evicted whole |
  | `max_log_records` | 5,000,000 | log cap (enforced once logs are stored) |
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
the same way once logs are stored.

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
service map and are covered with it.

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

`TraceSummary` is `Serialize` (status as `snake_case`, IDs as hex), like the
types it holds.

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
membership, classification, the summary) is done newest first only until the
page is full. The read lock is held for the call. M5 replaces this with an
indexed segment store behind the same API.

## Tests

`crates/loctel-store/tests/`:

- `fixtures.rs`: `span(...)`, `error(span)`, `with_kind`, `with_attr`,
  `with_exception`, `tid(n)`, `sid(n)` and the `store_with_manual_clock()`
  builders every store test file shares.
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
