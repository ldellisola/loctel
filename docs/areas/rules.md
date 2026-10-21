# Discard rules (`loctel-rules`)

Rules drop data as it arrives. A rule is a name, a language (TraceQL for spans,
LogQL for logs), a query, an on/off switch and, for TraceQL, what to drop on a
match. Whatever an enabled rule matches is **dropped for good**: it is never
stored or shown. Design references: §7 (discard rules), §6 (the query
languages) and §8 (MCP can see rules, not change them).

Status (Milestone 2): this crate is implemented: the model, `rules.toml`, the
ingest filter (`RuleSet`) and the preview. The engine runs every batch through
it, reloads the file and reports the counts in Task 6, MCP shows the rules in
`get_status` in Task 7, and the menu and status window show the rules line in
Task 8 (see the M2 plan, `superpowers/plans/2026-10-21-loctel-m2-query-rules.md`).
The rules screen is Milestone 4. This page grows with them.

The crate depends on `loctel-model`, `loctel-query` and `loctel-store` (to read
and remove stored data), plus `serde`, `toml`, `dirs`, `anyhow` and
`parking_lot`. It has no engine, UI or network code, and builds and tests on
Linux. The choices and their reasons are in
[`decisions/0016-no-default-discard-rules.md`](../decisions/0016-no-default-discard-rules.md)
and [`decisions/0017-rules-crate-choices.md`](../decisions/0017-rules-crate-choices.md).

## There are no default rules

loctel never decides what to discard. A missing `rules.toml` is created holding
**no rules**: only a comment header that explains the format and shows a
health-check rule commented out. Loading that file gives an empty list. To use
the example, remove the `# ` from each of its lines. See decision 0016.

## The file

`rules.toml` sits next to `settings.toml`: `dirs::config_dir()/loctel/rules.toml`
(`RulesFile::default_path()`), which on macOS is
`~/Library/Application Support/loctel/rules.toml`. It is TOML, an array of
`[[rule]]` tables, written to be edited by hand:

```toml
[[rule]]
name = "Health checks"
enabled = true
language = "traceql"
scope = "trace"
query = '{ span.url.path =~ "/(health|healthz|alive|livez|ready|readyz)(/.*)?" || span.http.route =~ "/(health|healthz|alive|livez|ready|readyz)(/.*)?" }'

[[rule]]
name = "Framework SQL logs"
enabled = true
language = "logql"
query = '{service_name="orders-api"} |= "Executed DbCommand"'

[[rule]]
name = "Cache calls"
enabled = false
language = "traceql"
scope = "span"
query = '{ name = "MGET" }'
```

| key | |
|---|---|
| `name` | what the user calls the rule (required) |
| `enabled` | `true` or `false`; a rule that is off stays in the file and does nothing (required) |
| `language` | `"traceql"` (judges spans) or `"logql"` (judges logs) (required) |
| `query` | the query, as written in Grafana (required); single quotes around it save escaping the double quotes inside |
| `scope` | TraceQL only: `"trace"` (the default) or `"span"`. Ignored for LogQL |

- **Rules apply in file order,** and only the enabled ones with a valid query.
- **`RulesFile::load(path)`** gives `Ok(None)` when the file does not exist and
  `Ok(Some(rules))` otherwise. Bad TOML, an unknown `language` or `scope`, a wrong
  type, or a rule without `name`, `enabled`, `language` or `query` is an `Err`
  whose one-line text names the file and, for TOML syntax, the line. The caller
  keeps the rules it had (the engine, Task 6).
- **A bad query is not a file error.** The file loads, and the rule with the typo
  is reported by `RuleSet` (`RuleStatus::error`, the parser's text with its
  column) and does nothing, while the other rules apply. One typo in a hand-edited
  file never switches everything off.
- **`RulesFile::save(path, rules)`** writes the header and one table per rule,
  creating the directory, through a temporary file and a rename (a crash leaves
  the old file or the new one, never half of one). Comments a person added by hand
  are not kept; the header is.
- **`RulesFile::write_empty(path)`** is `save` with no rules. **`EXAMPLE_RULE`**
  is the example as one `[[rule]]` table of TOML text; the header has each of its
  lines prefixed by `# `, and a test checks that.
- **`RulesFile::default_path()`** repeats `Settings::default_path()`'s rule
  (this crate cannot depend on the engine); with no config directory, the
  system's temporary directory stands in.

## What a rule does

`RuleSet::new(specs, clock)` holds the rules; `replace(specs)` swaps them;
`statuses()` lists them in file order with their counters and errors.
`filter_spans(spans, &store) -> SpanVerdict` and `filter_logs(logs)` judge a
batch on its way into the store.

### Spans (TraceQL)

For each batch, in this order:

1. **A remembered trace.** If a rule dropped the trace earlier (and the
   memory has not expired), every span of it is dropped and counted on the rule
   that dropped the trace.
2. **A remembered parent.** If a span's `(trace, parent)` is in the memory of
   dropped spans, it is a late descendant, and it is dropped the way a span-scope
   match is, counted on the same rule: with everything below it, and every ID
   remembered. That includes the spans of the same batch below it, whatever order
   the batch is in, and the spans the store already holds below it. Those are
   descendants that arrived before it (a service's exporter flushed early, so a
   grandchild came before the child): they went in the store because their
   parent was unknown, and they go in `SpanVerdict::drop_spans` now.
3. **The rules, in file order,** each over what is still live in the batch.
   - A query that can be judged span by span (`TraceQuery::needs_whole_trace()`
     is false: one filter, no structural operator, no pipeline, no `rootName`,
     `rootServiceName` or `traceDuration`) judges each incoming span with
     `matches_span`.
   - Any other query is evaluated **once per trace in the batch,** over the trace
     as it would stand once the batch is stored: the spans the store holds, then
     the batch's. So `{ kind = server } >> { name = "MGET" }` matches when the
     server span was stored earlier and the `MGET` span is in this batch.
4. Otherwise the span is kept, and appears in `SpanVerdict::keep`, in order.

A match drops according to the rule's **scope**:

- **`trace` (the default): the whole trace.**
  - Every incoming span of the trace is dropped.
  - The trace ID goes in `SpanVerdict::drop_traces`, so the caller removes the
    spans and logs the store holds of it (including those stored before the
    match).
  - The trace is remembered for `DROPPED_TTL`, so spans and logs that arrive
    later are dropped too.
  - It counts as one dropped trace.
- **`span`: only the matched spans and their descendants,** among the incoming
  spans and the stored ones.
  - Incoming ones are dropped.
  - Stored ones go in `SpanVerdict::drop_spans` for the caller to remove, with
    the logs emitted in them.
  - Every ID is remembered, so descendants that arrive later are dropped
    on arrival. A descendant that arrived *earlier* than the span that links it
    to the match (its parent unknown at the time, so it was kept) is dropped when
    that span arrives (step 2).
  - The rest of the trace is kept.

**The caller's side.** `filter_spans` changes nothing in the store. For a batch
the caller runs, as one step and before judging the next batch:
`filter_spans`, then `remove_dropped(&verdict, &store)` (which removes what the
verdict lists and credits each removal to the rule that dropped it), then
`store.ingest_spans(verdict.keep)`. Judging the next batch in between could store
a span after its trace was removed. `record_removed(rule, Removed)` is the same
crediting for a caller that already knows the rule.

`remove_dropped` removes the `drop_spans` first and the `drop_traces` after
them, and the order is load-bearing for the counters: when a span-scope rule
condemned stored spans of a trace that a later trace-scope rule of the same
batch dropped whole, the stored spans (and their logs) belong to the first
rule. Removing the trace first would hand them to the trace rule.

### Logs (LogQL, and the memory)

`filter_logs` drops a log when any of these holds, and counts it on the rule
responsible:

- its trace ID is remembered as dropped (a whole-trace drop);
- its `(trace, span)` is remembered as dropped (a span-scope drop);
- an enabled, valid LogQL rule matches it; the first in file order is credited.

The logs the store already holds for a dropped trace or span are removed by
`remove_dropped`, together with the spans. So a health check's logs vanish with
its trace, whichever arrived first.

### Memory and its limits

What was dropped is remembered in two sets, one of trace IDs and one of
`(trace, span)` pairs.

- **`DROPPED_TTL` is 10 minutes,** from when the entry was made. It is not
  renewed when it drops something, and it ends at exactly the TTL. After that, a
  span of the trace that does not match a rule itself is kept.
- **`DROPPED_MAX` is 100,000 entries** in each set, the oldest forgotten first.
- Time is read from the `Clock` given to `RuleSet::new` (the store's, so tests
  move both with one `ManualClock`).
- `replace` leaves the memory alone: the rest of a trace dropped by a rule that
  has since been edited or removed is still dropped for its time. It is credited
  to the totals only, since the rule is gone.

### Counting

- A rule's `dropped_spans` and `dropped_logs` count the incoming items it
  dropped **plus** the stored ones removed for it (`remove_dropped` or
  `record_removed`). `dropped_traces` counts traces dropped whole, once each.
- A span, log or trace is counted once, on the first rule that dropped it (file
  order) or, when it was dropped through the memory, on the rule that dropped the
  trace or span first.
- **`RuleId`** is assigned when a rule first appears and kept, across `replace`,
  for a rule with the same `(name, language, query, scope)`, wherever it moves in
  the list. Toggling `enabled` keeps the ID and the counters; changing the query
  makes a new rule, with new counters. IDs are never reused.
- **`totals()`** is what the rules dropped since launch, including rules that
  have since gone, so the dropped percentage does not fall when a rule is deleted.
  It includes stored items removed after they were accepted, so the engine's
  `dropped / (dropped + accepted)` counts those on both sides.

### What turning a rule on does

Turning a rule on does not purge what is stored. But a span that arrives for a
stored trace makes the trace be judged again as a whole (design §7: structural
rules are "evaluated against the spans present so far, on each arrival"), so a
match that was already there can now drop the trace, stored spans included.

## Preview

`preview(&spec, &store, since_unix_nano) -> Result<Preview, QueryError>` runs one
rule over the stored data and says what it would have dropped. The editor calls it
with `now - PREVIEW_WINDOW` (5 minutes) as `since`. It looks only at this rule:
other rules, the `enabled` flag and the memory are ignored, and nothing is
changed.

| field | meaning |
|---|---|
| `traces` | traces the rule matches (TraceQL; each trace is judged as the store holds it) |
| `spans` | spans it would drop: all spans of the matched traces for the trace scope, the matched spans and their descendants for the span scope |
| `logs` | LogQL: logs it matches. TraceQL: the logs of the dropped traces or spans, as `filter_logs` would drop them |
| `error_traces` | matched traces whose status is not `Ok` (for both scopes) |
| `error_logs` | dropped logs at level `error` or above |

A query that does not parse is the parser's `QueryError`, with the position, so
the editor can show it inline. Traces are those that start at or after `since`;
logs are those at or after it.

## Cost

- No enabled TraceQL rules and nothing remembered: `filter_spans` hands the batch
  back without looking at it. The same for logs and LogQL rules.
- A single-span rule is one evaluation per span.
- A rule that needs the whole trace copies the trace's stored spans for each trace
  in the batch, builds a `TraceView` and evaluates it. That is cheap for traces of
  tens of spans, and the cost to watch for a rule like this on the 100,000-span
  trace.
- All of it runs under one lock, so the gRPC and HTTP receivers see each other's
  drops; `statuses()` waits for a batch in progress.

## Tests

`crates/loctel-rules/tests/`: `file.rs` (the file: missing, round trip, defaults,
the empty file and its example, errors), `filter.rs` (the filter with a
`ManualClock`, a `Store` and the fake mesh: whole-trace drops that cover stored
and late spans, structural rules over stored spans, span scope and its
descendants (including stored ones that arrived before their ancestor), LogQL
rules, disabled and invalid rules, file-order counting, memory expiry, `replace`,
crediting removals), `preview.rs` (the counts, `since`,
errors). `memory.rs` has unit tests for the bounded memory, and
`fixtures.rs` holds the shared builders. The whole-trace drop with a root that
arrives last is `whole_trace_drop_covers_stored_and_late_spans`, and
`late_out_of_order_trace_is_dropped_when_its_root_arrives` runs it on the real
`LateOutOfOrder` scenario.

## Not here

The engine's side (rules at ingest, reloading the file when it changes, the
status and the handle API) is Task 6 of the M2 plan, the MCP view of the rules is
Task 7, and the menu and window rules line is Task 8. Prefilling a rule from a log
line and the rules screen are M4.
