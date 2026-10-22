# 0048: Bounded work on search and recent failures

Date: 2026-10-22
Status: accepted

Milestone 5, Task 5. The plan says `search_traces`, `recent_failures` and
`services` must not sort or clone every held trace per call, asks for
`recent_failures` to make one pass over the index, newest first, that stops at
its limits, and fixes two tests that count the summaries a call builds. This
file records what that took, and what the plan left open.

## Context

Task 2 had already put the ordered index under `search_traces` (it walks it newest
first and stops at the page), so nothing sorted the whole set any more, and
`services` never sorted or copied the traces it counted. What was still linear in
the traces held was the work done *per trace turned away*:

- A trace in memory was summarized (a walk of its spans: classify, the services,
  the exception) and the summary thrown away when `errors_only` found the trace
  fine. `errors_only` is what `recent_failures` searches with, so on 50,000
  traces of which 3 failed it built 50,000 summaries.
- The summary of the "one more match" that proves there is a next page was built
  and thrown away on every page.
- `recent_failures` paged `search_traces` and kept the first `limit` of each
  status. When one list was full and the other could not fill (a service that is
  down: thousands of failed traces and none with an error inside), every further
  failed trace was summarized and dropped, for the whole index.

## Decisions

- **The index holds the status of a trace in memory.** The plan lists "status" in
  `TraceMeta`; decision 0045 kept it only for a cold trace (inside its summary)
  and worked it out from the spans for a hot one. Now `TraceMeta::status` is set
  for both: a hot `TraceEntry` counts its erroring spans and tracks its root (the
  parentless span with the earliest `(start, span ID)`, what `root_span` finds)
  as spans are inserted, and recomputes both in the pass `remove_spans` already
  makes, so `TraceEntry::status()` is constant time and `sync_hot` copies it into
  the index. Keeping it as a cache that is recomputed on demand was the other
  option, and was refused: a lazily filled cell is interior mutability under the
  read lock, and the arithmetic here is a counter and a comparison. The rule is
  `classify`'s (a root that is erroring is `Failed`, else any error is
  `ErrorsInside`), and a unit test in `src/trace.rs` runs random span sequences,
  parent cycles and removals through both and demands they agree. A cold trace's
  entry takes the status of the summary it was spilled with.
- **`search_traces` judges, then summarizes.** `errors_only` and the duration
  are read from the entry, the service and the query are tested next
  (`Inner::matches`, which replaces `candidate`), and a summary is made only for
  a trace that goes into the page. The lookahead that sets `next_cursor` needs
  to know a match exists, not what it looks like, so it makes none. A page of
  `limit` makes exactly `limit` summaries. The plan's test says "at most
  `limit` + 1"; the stronger property costs nothing, and the tests assert the
  plan's bound for every kind of filter and the exact count where it is clean.
  The results do not change: the conditions are all conjunctions, so the order
  they are tried in only changes the cost.
- **`Store::recent_failures(since, limit) -> RecentFailures` is new, and
  additive.** The plan puts the one pass in `tools.rs`, but the index is the
  store's, and the public API reaches it only through `search_traces`, whose
  `TraceFilter` cannot say "failed but not errors-inside" (adding a field to it
  would break every struct literal of it outside the crate, `loctel-ui` among
  them, which the frozen API forbids). So the pass is a store method that returns
  both lists, and the tool calls it. It is one pass from the newest trace under
  one read lock, ends when both lists are full, and makes a summary only for the
  traces it puts in a list. A trace that qualifies for a list that is full costs
  an index lookup. The tool's answer is unchanged (the same traces, the same
  order, `failure` and `error_logs` as before); the previous loop in `tools.rs`
  is gone, and `SCAN_PAGE` with it.
- **`scan_traces` takes the status from the index** for a trace in memory too
  (it ran `classify` per trace). Same value, no walk.
- **The hook** is `Store::summaries_built()`, hidden, counting what `summary_of`
  hands out (the one function that makes a summary for a read, for a trace in
  memory and for one in a segment file). The spill step makes a summary to store
  in the index; that is not counted, so a test that crosses a step still reads
  the cost of its search alone.

## Not done

- **A service filter on a trace in memory still walks its spans** (`has_service`).
  The plan lists a service set in `TraceMeta`; keeping one up to date for hot
  traces costs memory in every hot trace and a pass in `remove_spans`, and the
  walk is over spans that are in memory already. A cold trace answers from its
  summary. Revisit if a service-filtered search over a very large hot set shows
  in `cargo xtask perf`.
- **`services` over a wide window still counts every span in it**, because the
  list is counts of spans: it cannot come from the index without a per-trace count
  of entry spans per service. A window that is narrow is cheap, since a trace that
  ended before it is skipped from its entry.
