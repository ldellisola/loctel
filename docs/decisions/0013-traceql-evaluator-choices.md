# 0013: TraceQL evaluator choices

Date: 2026-10-21
Status: accepted

## Context

The Milestone 2 plan (Task 2) fixes the evaluator's interface (`TraceView`,
`TraceQuery::evaluate`, `TraceQuery::matches_span`) and its semantics: filters,
missing attributes, typing, the root intrinsics, the structural operators,
spanset `&&` and `||`, and `count()`. It leaves open the cases a trace in
flight produces (orphans, loops), what a bare attribute and `!` do when the
attribute is missing, and a few typing pairs. Tempo's own engine
(`pkg/traceql/ast_execute.go` and the structural operators in
`tempodb/encoding/vparquet4/block_traceql.go`) was read to settle them; it is
readable from GitHub, grafana.com was not needed. These are the choices a
later agent might be tempted to undo.

## Decision

- **Structural operators are linear passes, not pair tests.** `TraceView`
  numbers the tree as a nested set in the interval form: each span's `left` is
  its position in a preorder walk, and `last` is the position of the last span
  of its subtree, so `a` is above `d` exactly when `left[a] < left[d] <=
  last[a]`. `is_ancestor` is those two comparisons, as the plan asks. The
  evaluator never calls it per pair, though: `>>` and `<<` sweep the preorder
  once, carrying the furthest subtree end seen (or the nearest later member),
  and `>`, `<` and `~` count over parent links. A health-check rule runs these
  on every arriving batch against whatever the trace has grown to, and testing
  every pair of two large sets would be quadratic (100,000 spans took about
  50 ms per query in a debug build). Walks use explicit stacks, so depth is
  never a stack-overflow risk, whatever arrives.
- **Orphans are roots of their own subtrees; a missing parent is not a
  parent.** A span whose parent has not arrived has `parent() == None`, keeps
  its own descendants, and is not *the trace's* root (`root()` is the store's
  rule: parentless by declaration, earliest start, ties to the lowest span ID).
  Spans without a parent have no siblings, so several roots, or an orphan and
  the root, never match `~`, as in Tempo (`nestedSetParent != 0` in its
  `SiblingOf`).
- **Bad data degrades, never loops.** A span that names itself as its parent is
  a root; a loop of parents is cut at its lowest-numbered span; with duplicate
  span IDs, children attach to the first. A rules file can run a query against
  any trace the network delivers, so the view must not be able to hang ingest.
- **Negated structural operators with nothing on the left give every right-hand
  span.** "The spans of B without that relationship" is literal, and it is
  what Tempo's `falseForAll` loops do. The plain forms give nothing.
- **A bare attribute is true only when it is the boolean `true`, and `!` of a
  missing or non-boolean attribute is not true.** Conditions are three-valued
  (true, false, unknown), and a span matches only on true. Plain two-valued
  logic would make `{ !span.cache.hit }` match every span that lacks the
  attribute, which is not what Tempo does (its `!` on a non-boolean is an error
  that fails the span) and is the kind of silent mismatch this milestone avoids.
  Comparisons are never unknown: on a missing attribute they are false, so
  `!(span.x = 1)` is true there, as in Tempo.
- **An int or float attribute does not compare with a duration.** Tempo
  treats durations as numbers of nanoseconds and compares the two; the plan
  says durations compare with durations only and any other type pair is false,
  and the plan wins. If Grafana queries like `{ span.latency_ns > 1s }` turn out
  to matter, add the `Int`/`Float` and `Duration` pairs to `compare` in
  `traceql/eval.rs` and the table in `docs/areas/query.md`.
- **Array, bytes and map attributes are present but compare as nothing.** The
  plan has no array semantics (Tempo matches when any element matches). They
  count for `!= nil` and for nothing else, so a query on them is a clean
  non-match, not a wrong answer.
- **Status and kind have no order, booleans have equality only.** `status >
  ok` and `kind >= server` are false. Statuses and kinds compare with their
  own keywords only: `status = "error"` and `status = 2` are false (Tempo
  rejects the first after parsing; the parser here accepts it, decision 0012).
- **`traceDuration` is the earliest start to the latest end, clamped at 0**,
  as the plan says, and needs no root. The store's trace duration counts every
  end as no earlier than its own start; the two differ only for a trace with a
  skewed span. `rootName` and `rootServiceName` are missing until a root
  arrives: a comparison is false and `= nil` is true.
- **NaN follows IEEE:** equal to nothing, so `!=` holds and the other
  comparisons do not. Ints compare exactly as `i64` among themselves and as
  `f64` against a float.
- **`matches_span` builds no view and allocates nothing of its own.** It is
  the single-span path the rules engine runs on every arriving span: it is
  `Some` exactly when `needs_whole_trace()` is false and then equals `evaluate`
  on a one-span view.

## Consequences

- `docs/areas/query.md` has the semantics table and the list of places where
  loctel is narrower than Tempo. Changing one of these means changing the
  table, the test that pins it (`typed_edge_cases_on_hand_made_spans`,
  `orphans_are_subtree_roots`, `structural_operators_with_an_empty_left_side`)
  and this record together.
- The rules engine (Task 5) can rely on: `evaluate` never fails and never
  panics, its result is ascending and duplicate-free, and an empty trace
  matches nothing.
- `TraceView` takes any slice and does not check that its spans share a trace
  ID: the caller gives it one trace's spans.
