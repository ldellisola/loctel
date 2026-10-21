# 0017: Rules crate choices

Date: 2026-10-21
Status: accepted

## Context

The Milestone 2 plan (Task 5) fixes `loctel-rules`: the rule model, the file,
`RuleSet` with `filter_spans` and `filter_logs`, the memory of dropped data, and
the preview. It leaves open how a batch is judged when whole-trace queries and
span memory interact, who is credited for what, and a few edges. It also has one
gap: `record_removed(rule, Removed)` needs the rule that dropped something, and
the verdict (`drop_traces`, `drop_spans`) names none. These are the choices a
later agent might be tempted to undo, and the one place the plan was adapted.

## Decision

- **Rules are applied rule by rule over the batch, not span by span over the
  rules.** The plan reads "for each incoming span, in order: ... TraceQL rules in
  file order", and also says whole-trace queries are evaluated once per trace.
  Doing the first literally would let a later rule claim a trace because its
  span came first in the batch. Walking the rules in file order, each over what
  is still live, gives "a span is counted once, on the first rule that dropped
  it" for spans, traces and logs alike, whatever order the batch is in.
- **A late descendant is dropped with its whole subtree, stored spans
  included.** The plan drops a span whose parent is in the dropped-span memory.
  A batch is in no particular order, so a child can precede its parent, and
  services flush their exporters on their own schedules, so a grandchild can
  arrive in an earlier batch than the child, be kept (its parent is unknown) and
  be stored. When the child then arrives as a late descendant of a dropped span,
  that grandchild is a descendant of the matched span (design §7: "only the
  matching span and its descendants"), so it must go. A late descendant is
  therefore handled exactly like a span-scope match: the same sweep
  (`Pass::drop_subtrees`) over the stored spans of the trace and the batch's
  condemns everything below it, lists the stored spans in `drop_spans` and
  remembers every ID. The late spans of a batch are gathered first and swept once
  per trace and rule, so a batch of them costs one copy of the trace's stored
  spans. A stored span counts on the rule that dropped its ancestor. (An earlier
  version swept only the batch, which left the stored grandchild, and its logs,
  for good.)
- **`remove_dropped(&SpanVerdict, &Store) -> Removed` is added, to close the gap
  in `record_removed`.** The plan has the engine call `remove_trace` or
  `remove_spans` for the verdict's lists "feeding `record_removed(rule,
  Removed)`", but the verdict does not say which rule dropped what, and its
  shape (`drop_traces: Vec<TraceId>`, `drop_spans: Vec<(TraceId, SpanId)>`) is
  fixed. The memory of dropped data already holds the rule for every trace and
  span, so `remove_dropped` reads it, calls the store, and credits each removal
  to its rule through the same code `record_removed` uses. `record_removed`
  stays public and as specified, for a caller that knows the rule. The engine
  calls `remove_dropped`. It removes the verdict's `drop_spans` before its
  `drop_traces`: an earlier span-scope rule and a later trace-scope rule can
  both hit one trace in a batch, and `remove_trace` would otherwise take the
  stored spans (and logs) the span rule had already claimed and credit them to
  the trace rule, breaking "a span is counted once, on the first rule that
  dropped it". Tests cover both rule orders.
- **Counting.** A rule's `dropped_spans` and `dropped_logs` are the incoming
  items it dropped plus the stored ones removed for it; `dropped_traces` is the
  traces it dropped whole, counted once each. The stored logs of a dropped trace
  count as dropped logs, the same as the ones that arrive afterwards. A span is
  counted once: a retry of a span that is stored and now dropped counts again
  (the retry is a second arrival), which is rare and not worth a lookup.
- **`totals()` counts since launch, over rules that are gone too.** The engine's
  dropped fraction is "since launch", and a rule the user deletes (or edits,
  which makes it a new rule) must not make the number fall. So the totals are
  kept apart from the per-rule counters, and are added to in the same places.
  The totals include stored items removed after they were accepted, so the
  engine's `dropped / (dropped + accepted)` counts such an item on both sides; at
  most a trace's worth at a time, and a ratio shown as a whole percent.
- **A rule's identity is `(name, language, query, scope)`,** as the plan says,
  and `enabled` is not part of it: turning a rule off and on keeps its ID and its
  counters. Changing the scope of a LogQL rule, where scope means nothing, still
  makes a new rule; that is what the plan's tuple says, and it is not worth a
  special case. Two identical rules in the list stay two rules, each with its own
  ID and counters.
- **The query is parsed for every rule, enabled or not,** so that a draft saved
  with `enabled = false` still reports its error. A rule with an error never
  applies, whether or not it is enabled.
- **Memory.** Two memories, one for trace IDs and one for `(trace, span)` pairs,
  each bounded by `DROPPED_MAX` (100,000) with the oldest forgotten first (the
  plan's single bound, applied to each kind, so that a span-scope rule on a huge
  trace cannot push every remembered trace out). An entry lives `DROPPED_TTL`
  (10 minutes) from the moment it was made and is **not** renewed when it drops
  something: a trace that keeps sending spans is remembered for ten minutes
  from its drop, not for ever. It ends at exactly the TTL (`now >= made + TTL`).
  Replacing the rules leaves the memory alone, so the rest of a trace that a
  removed rule dropped is still dropped for its time.
- **A whole-trace query sees the trace as it would stand after the batch:** the
  stored spans (minus any already condemned in this pass), then the batch's live
  spans that the store does not hold yet. The stored spans are cloned out of the
  store for that, once per trace per whole-trace rule per batch, because
  `TraceView` borrows a slice. Single-span queries and the common case (no
  whole-trace rule) cost nothing like that.
- **A rule newly turned on can drop part of a stored trace.** "Turning a rule on
  affects new data only" holds for what is already stored and never touched
  again. But a span that arrives for a stored trace makes the trace be judged
  again as a whole, as design §7 says for structural rules ("evaluated against
  the spans present so far, on each arrival"), so a match that was always there
  can now drop the trace, stored spans included.
- **The preview's `error_traces` is the trace's status for both scopes.** The
  plan defines it as "matched traces whose status isn't `Ok`". For a span-scope
  rule that flags a failed trace even when the dropped spans themselves were
  fine. It errs towards warning, which is the right way for a "this would have
  dropped errors" light to be wrong.
- **The preview's logs follow the rule's scope:** a TraceQL rule's logs are those
  of the traces (trace scope) or spans (span scope) it would drop, the same logs
  `filter_logs` drops through its memory.
- **The file's errors are one line.** `RulesFile::load` names the file and, for
  bad TOML, the line, in one line of text (the engine puts it in the menu and
  the status). A missing key or a wrong `language` or `scope` is a file error
  too, so a typo there keeps the previous rules; only a bad *query* disables a
  single rule.
- **An unknown key is a file error too** (`#[serde(deny_unknown_fields)]` on
  `RuleSpec` and on the file's top-level table). `scope` is the only optional
  key, so without this a misspelled `scpoe = "span"` (or `Scope`) loaded as a
  rule without a scope and got the default, the whole trace: it dropped traces
  and their logs for good when the person asked for only the matched spans to go.
  A typo in any other key already failed as a missing field, and `[[rules]]` for
  `[[rule]]` loaded as a file with no rules, so every rule in it silently did
  nothing. Both are now an error that names the key and the line, which keeps the
  previous rules and sets `rules_file_error` (the same handling as bad TOML).
  The plan's derives are unchanged; the attribute is the only addition. Do not
  remove it to make a future key "forward compatible": a new key is added to
  `RuleSpec` and the header together, and an older loctel reading a newer file
  should refuse it, not guess.
- **The save rewrites the whole file** (header plus tables, through a temporary
  file and a rename, like settings). Hand-written comments do not survive a save
  by loctel; the header does.

## Consequences

- The engine's sink must run `filter_spans`, `remove_dropped` and
  `ingest_spans(keep)` for a batch as one step, under its lock, before the next
  batch (the plan already puts the sink under one mutex).
- A rule that needs the whole trace costs a copy of the stored trace per batch
  that touches it. With very large traces (the 100,000-span scenario) and such a
  rule this is the cost to watch; M5's store can hand out something cheaper than
  a `&[Span]`.
