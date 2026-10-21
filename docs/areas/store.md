# Store (`loctel-store`)

The in-memory store behind loctel. It takes spans in any order from any
service, assembles them into traces, says when a trace has gone quiet, and
evicts the oldest trace whole when it holds too many. Design references: §4
(order and lateness, completeness), §5 (storage, eviction, clear all).

Everything outside the crate goes through `Store`. M5 replaces the internals
with a spill-to-disk segment store, so keep the public API below stable and
keep callers away from anything else.

Status: trace assembly, completeness, dedup and eviction are implemented.
Failure classification, trace search and summaries, logs and the service map
are added by later tasks and get their own sections here.

## API

```rust
let clock = Arc::new(ManualClock::new(start_unix_nano)); // or SystemClock
let store = Store::new(StoreConfig::default(), clock.clone());

store.ingest_spans(spans);               // Vec<loctel_model::Span>
let snapshot = store.trace(trace_id);    // Option<TraceSnapshot>
let stats = store.stats();               // StoreStats
store.clear();
```

- `Store` is `Clone + Send + Sync`. Clones share the same data (an `Arc`
  around an `RwLock`), so give each thread or task its own clone.
- `ingest_spans` takes the write lock once per batch. `trace` and `stats`
  take the read lock.
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
`snake_case`) and `failed_in` an `Option<FailedIn>`. Both types are declared
in `src/failure.rs`. The classification is not implemented yet: until it is,
every snapshot reports `Ok` and `None`.

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

## Tests

`crates/loctel-store/tests/`:

- `fixtures.rs`: `span(...)`, `error(span)`, `tid(n)`, `sid(n)` and the
  `store_with_manual_clock()` builders every store test file shares.
- `assembly.rs`: the plan's Task 3 tests. `duplicate_span_is_ignored`
  (exporter retries) and `skewed_child_still_assembles` (clock skew) are the
  review-focus tests this crate owns.
- `edge_cases.rs`: ordering, root choice, orphans, dedup and completeness
  corner cases, eviction order and batching, clear, sharing across threads.
- `clock.rs`: the clocks and the config defaults.
