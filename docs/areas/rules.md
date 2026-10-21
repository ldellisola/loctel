# Discard rules (`loctel-rules`)

Rules drop data as it arrives. A rule is a name, a language (TraceQL for spans,
LogQL for logs), a query, an on/off switch and, for TraceQL, what to drop on a
match. Whatever an enabled rule matches is **dropped for good**: it is never
stored or shown. Design references: §7 (discard rules), §6 (the query
languages) and §8 (MCP can see rules, not change them).

Status (Milestone 2): this crate is implemented: the model, `rules.toml`, the
ingest filter (`RuleSet`) and the preview. The engine runs every batch through
it, reloads the file and reports the counts (Task 6, "In the engine" below). MCP
shows the rules in `get_status` (Task 7, "In MCP" below), and the menu and
status window show the rules line and the macOS menu opens the file for editing
(Task 8, "In the menu and the window" below; see the M2 plan,
`superpowers/plans/2026-10-21-loctel-m2-query-rules.md`). The rules screen is
Milestone 4. This page grows with them.

The crate depends on `loctel-model`, `loctel-query` and `loctel-store` (to read
and remove stored data), plus `serde`, `toml`, `dirs`, `anyhow` and
`parking_lot`. It has no engine, UI or network code, and builds and tests on
Linux. The choices and their reasons are in
[`decisions/0016-no-default-discard-rules.md`](../decisions/0016-no-default-discard-rules.md),
[`decisions/0017-rules-crate-choices.md`](../decisions/0017-rules-crate-choices.md)
and, for the engine's side,
[`decisions/0018-engine-rules-choices.md`](../decisions/0018-engine-rules-choices.md).

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
  keeps the rules it had (the engine does, see "In the engine").
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

## In the engine

`loctel-engine` runs the rules (`crates/loctel-engine/src/{sink.rs,
rules_watch.rs}`). The reasons are in decision 0018.

- **Where the file is.** `EngineConfig::rules_path`: `Some(path)` for the app,
  `None` (the default) for tests, where the rules live in memory and start empty.
  `loctel` takes it from `--rules PATH`; without the flag it is `rules.toml` in
  the directory of the settings file, so the per-user
  `RulesFile::default_path()` normally, and `rules.toml` beside a scratch
  `--settings` file when one is given (a run on scratch settings never touches the
  real rules). `EngineHandle::rules_path()` says which.
- **At start.** With a path and no file, the engine writes the empty file
  (`RulesFile::write_empty`) and has no rules. With a file, it loads it. With a
  file that cannot be read or parsed it starts with **no rules and
  `EngineStatus::rules_file_error` set**, and leaves the file as it is: it never
  rewrites a file it could not read. Where even the empty file cannot be written
  (a read-only folder) the error is set too. None of these stops the engine.
- **Every batch goes through the rules** in the receivers' sink, after the pause
  check and before the store, under one lock so batches apply in order: for spans
  `filter_spans`, then `remove_dropped` (what the store holds of the condemned
  traces and spans goes, and is credited to the rule that dropped it), then
  `ingest_spans(keep)`; for logs `filter_logs`, then `ingest_logs`. A paused
  engine drops the batch before the rules see it, so no counter moves.
- **Rates and the dropped fraction count what was accepted.** `spans_per_sec` and
  `logs_per_sec` count what the rules let through. `dropped_spans` and
  `dropped_logs` are `RuleSet::totals()`, since launch. `dropped_fraction` is
  dropped divided by dropped plus accepted, spans and logs together, since launch
  (0 when nothing has arrived). It counts a stored item that a later drop removed
  on both sides, which for a ratio shown as a whole percent is not worth a
  correction.
- **Reload.** A task on the engine's runtime looks at the file every
  `RULES_POLL` (2 s). It reads the file when its modification time or its length
  differs from what the engine last saw. On success the rules are replaced (an
  unchanged rule keeps its ID and counters) and the error clears. On a failure
  (bad TOML, a wrong key or value) **the current rules stay** and
  `rules_file_error` holds the one-line text, until a good file arrives. A bad
  *query* is no file error: that one rule shows its error in `status().rules` and
  the others apply. The engine's own writes update what it last saw, so they do
  not reload. A file that disappears changes nothing: the rules in memory stay
  and nothing is written over the gap (an editor that saves by renaming leaves
  none for a moment); the next change through the handle writes the file again.
- **The handle.** `rules()` (the same list as `status().rules`),
  `rules_path()`, `set_rules(specs)`, `set_rule_enabled(id, enabled)` and
  `preview_rule(&spec)`.
  - `set_rules` and `set_rule_enabled` save the file first (when there is a path)
    and then apply, so that a failed write is an `Err` that changes nothing. They
    clear a file error, since the file has just been written over. Invalid
    queries are allowed (a draft); they show as that rule's error. Hand-written
    comments in the file do not survive a save, the header does.
  - `set_rule_enabled` with an ID that is not in the list is an `Err`.
  - `preview_rule` runs `preview` over the last `PREVIEW_WINDOW` (5 minutes) of
    stored data, ignoring `enabled` and the other rules.
- **Status.** `EngineStatus` carries `rules` (`Vec<RuleStatus>`),
  `rules_file_error`, `dropped_spans`, `dropped_logs` and `dropped_fraction`.
- **Headless output.** `loctel` prints one JSON line about the rules after the
  listener lines: `{"event":"rules","path":"...","count":N,"error":null}`. `count`
  is how many rules the file holds, enabled or not, and `error` is the file's
  error text or `null`. `path` is `null` when the rules live in memory only (the
  app always has one).

## Tests

`crates/loctel-engine/tests/rules.rs` runs the engine on real sockets with the
fake mesh (the file at start, health checks dropped, a trace whose root arrives
last, live reload, a broken file, the handle API, the preview, the dropped
fraction, rates and pause), `rules_watch.rs` has unit tests for the file polling,
and `crates/loctel-app/tests/headless.rs` checks the `rules` line of the binary.

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

## In MCP

`get_status` lists the rules, read-only (design §7: MCP cannot change rules; the
server has no tool that does). The engine builds the list from the same
`EngineStatus` the menu and the window read, so they cannot disagree: `rules`
(name, `language`, `scope` for TraceQL only, `enabled`, `error` if the query does
not parse, `dropped_spans`, `dropped_logs`, `dropped_traces`), `rules_file_error`
and `dropped_fraction`. Names and errors are cut at 2,000 characters. The shapes
and the reasons are in [`mcp.md`](mcp.md), "Rules in `get_status`"; the engine
test `mcp_get_status_lists_the_rules_the_engine_has` reads them through a real
MCP client.

## In the menu and the window

The menu bar menu (macOS) and the status window (Linux) show the rules in one
line, `loctel_ui::menu_text::rules_line`, built from the same `EngineStatus` as
everything else (never from the file). In priority order:

| Line | When |
|---|---|
| `Discard rules file has an error` | `rules_file_error` is set: `rules.toml` could not be used, and the rules in force are the last good ones |
| `1 discard rule on · 1 has an error` (`3 discard rules on · 2 have errors`) | a rule that is on has a query that does not parse, so it drops nothing |
| `No discard rules on` | no rule is on (none in the file, or all switched off) |
| `1 discard rule on · 31% dropped`, `3 discard rules on · 0% dropped` | rules are on and working; the share of what arrived that they dropped since launch, as a whole percent |

"On" means `enabled`, whether or not the query parses. A rule that is switched
off is not counted as on, and is not counted as having an error either, even if
its saved draft query does not parse (the status still lists that error for it):
the line warns about what you expect to be working. Because of that, the number
of rules with errors never exceeds the number on, which keeps "3 discard rules on
· 2 have errors" from ever reading "0 on · 1 has an error".

On macOS the menu also has **Edit discard rules…**, which opens
`engine.rules_path()` in the default text editor (`NSWorkspace` `openURL`, and
`open -t` if no app handles `.toml`). Editing the file by hand is the way to edit
rules until the rules screen (M4); the engine reloads the file within about 2
seconds (`RULES_POLL`, see "In the engine"). The item is greyed out when the
engine has no rules file. Saving through the handle (the future rules screen)
rewrites the file and drops hand-written comments, but keeps the header, so the
menu item and the screen must not be offered as if both were lossless (decision
0018). See [`macos.md`](macos.md) for what is unverified until the Mac check, and
[`ui.md`](ui.md) for the strings and the view.

## Not here

Prefilling a rule from a log line and the rules screen are M4.
