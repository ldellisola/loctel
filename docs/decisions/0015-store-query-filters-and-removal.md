# 0015: Store query filters, removal and scans

Date: 2026-10-21
Status: accepted

## Context

The Milestone 2 plan (Task 4) fixes the store's new interface: `query` on
`TraceFilter` and `LogFilter`, `matched_span_ids` on `TraceSummary`, `Removed`,
`remove_trace`, `remove_spans`, `with_trace_spans`, `scan_traces` and
`scan_logs`. It leaves open which order the spans are in when a query runs,
how removal fits the structures the store already had, and what a removal does
at the edges. The rules layer (Task 5) and the preview build on these answers,
so they are recorded here for later agents.

## Decision

- **A query runs over the spans as held, and `matched_span_ids` follow that
  order.** `TraceView::new` takes the entry's spans in arrival order, with no
  copy and no sort, and `matched_span_ids` are the first 20 positions of
  `evaluate`'s answer, mapped to span IDs. So the 20 are "the first 20 matched
  in arrival order", not "the 20 that started first". Sorting would copy every
  span of every trace a search looks at (a `Span` owns strings and attribute
  vectors) to give an order nobody asked for, and `with_trace_spans`, which the
  rules use to evaluate the same trace, has to be the held order anyway, so the
  rules and the search agree. A caller that wants start order sorts the 20 it
  gets, or reads the snapshot.
- **The order of first arrival is a map, not a queue.** `arrival_order` was a
  `VecDeque` of trace IDs; removing a trace from the middle of one is a pass
  over every trace, and the plan asks for `remove_trace` to cost the trace's own
  spans and logs. It is now a `BTreeMap<u64, TraceId>` keyed by an arrival
  number that the trace entry also carries, so removal is a lookup, eviction is
  `pop_first`, and `scan_traces` can walk the traces in a stable order (oldest
  arrival first) instead of hash order. The alternative, leaving stale IDs in
  the queue and skipping them at eviction, needs compaction to stay bounded
  under a rule that drops hundreds of traces a second.
- **Logs get a per-trace index next to the per-span one.** `by_trace` holds,
  for every log that carries a trace ID (with or without a span ID), its key in
  time order, exactly like `by_span`. Eviction takes the oldest log, which is
  at the front of both; removal drops whole entries. The cost is one 16-byte
  key per log that has a trace ID. Merging the two indexes into one two-level
  map would save that, but it rewrites `logs_for_span` and eviction for a
  saving that M5's segment store makes moot.
- **`remove_spans` removes the logs of the IDs it is given, held span or not.**
  Logs and spans are stored independently (M1's rule), so a span can have
  been evicted, or never have arrived, while its logs are held. "Remove this
  span" then means its logs too, and `Removed::spans` still counts only spans
  that were held. **Removing the last span removes the trace entry only:** a log
  that names the trace but no span is not any span's log, so it stays, and
  `remove_trace` is the call that takes every log of a trace.
- **Removal forgets, like eviction.** The dedup memory covers only what is
  held, so a retry of a removed span or log is accepted as new. Keeping retries
  of dropped data out is the rules layer's memory, with its TTL, not the
  store's. `spans_received` and `logs_received` do not go down: they count what
  the store accepted.
- **`remove_spans` rebuilds the trace's index in one pass** (linear in the
  trace's spans, whatever the number of IDs) and recomputes its start and end,
  so duration, search by `since` and orphans follow the spans that are left. It
  does not touch `last_arrival`: removing a span is not a new arrival and must
  not make a quiet trace incomplete again.
- **The query runs before the summary is built.** `search_traces` tests the
  cheap conditions and the service, then the query, then builds the summary and
  tests `errors_only`: a query is the condition that usually rejects, and a
  rejected trace then costs no summary (no classification, no exception pick).
- **The filters are `PartialEq` but no longer `Eq`.** A query holds floats
  (`Static::Float`), so neither `TraceFilter` nor `LogFilter` can keep `Eq`. The
  plan allows the first; the second follows for the same reason. Nothing used
  `Eq` on a filter. `TraceSummary::matched_span_ids` is left out of the JSON when
  it is empty, as the MCP DTO will do (Task 7), so a summary with no query
  serializes as before.
- **The scans run the callback under the read lock.** `scan_traces`,
  `scan_logs` and `with_trace_spans` copy nothing, which means the callback
  runs while the store is locked for reading. The doc says to keep it short and
  never to call the store from it; callers collect IDs and remove after the scan
  returns.
- **One existing test needed a mechanical change.** `search_traces.rs` built a
  `TraceFilter` with every field spelled out, so it gained `query: None`. Its
  assertions are untouched.
- **The store's tests use the fake mesh,** so `loctel-store` has
  `loctel-meshgen` and `loctel-ingest` as dev-dependencies, though `loctel-meshgen`
  has the store as a dev-dependency too. Cargo allows a cycle through
  dev-dependencies (a dependency's dev-dependencies are never built), and the
  two crates share only `loctel-model`'s types.

## Consequences

- A search with a query costs one `TraceView` (linear in the trace) per
  candidate trace that gets past the cheap conditions. With 200,000 traces held
  and a selective query, that is the cost to watch when M5 replaces the
  internals.
- Removing a trace never leaves a stale ID anywhere in the store, so the
  consistency checks (`debug_assert_eq!` in log eviction) hold after any mix of
  removal and eviction; `tests/removal.rs` pins that.
