# Architecture

loctel receives OTLP traces and logs, assembles them in memory, works out
which service failed, and serves the result to a menu bar UI and to AI agents
over MCP. The design is in
[`superpowers/specs/2026-10-21-loctel-design.md`](superpowers/specs/2026-10-21-loctel-design.md).
This page is the map: update it whenever a crate is added or a data path
changes.

The hard requirement (design §11): **the core has no UI or graphics
dependency and builds and tests on Linux alone.** The UI stays thin over it,
and macOS-only code sits behind `cfg(target_os = "macos")` in its own small
crate.

## Crate map

Status says what is in the repository now. The Milestone 1 set, from
`superpowers/plans/2026-10-21-loctel-m1-core.md`, all exists, and so does what
Milestone 2 (`superpowers/plans/2026-10-21-loctel-m2-query-rules.md`) added:
`loctel-query` and `loctel-rules`, and the changes they brought to the store,
MCP, engine, UI and macOS crates. Milestone 3
(`superpowers/plans/2026-10-22-loctel-m3-main-window.md`) added the main window
to `loctel-ui`, `loctel-macos` and `loctel-app` and no crate. Milestone 4
(`superpowers/plans/2026-10-22-loctel-m4-logs-rules-popover.md`) added the Logs and
Discard rules screens, the Settings window and the popover view to `loctel-ui`,
the popover to `loctel-macos` (it replaces the menu), live port rebinding to
`loctel-engine` and log IDs, level and window filters, the histogram and the
lines around a log to `loctel-store`, and no crate. Milestone 5
(`superpowers/plans/2026-10-22-loctel-m5-scale.md`) made the store spill to segment
files under a disk cap (`loctel-store`, `loctel-engine`, `loctel-mcp`) and added the
performance budgets as `loctel-perf`, run by `cargo xtask perf`. Everything
Mac-only in it is unverified until the milestone check
(`milestones/mac-checklist.md`). The change that adds a
crate updates this table and the diagram below it.

| Crate | Layer | Owns | Status |
|---|---|---|---|
| `loctel-model` | core | Domain types (spans, logs, resources, IDs, severity), independent of OTLP | implemented (see "Domain model" below) |
| `loctel-store` | core | Store, in memory and spilling to temporary segment files: trace assembly, completeness, dedup, eviction, failure semantics, search (with TraceQL and LogQL filters), logs, removal and scans, service map | implemented: assembly, completeness, dedup, eviction, failure semantics, trace summaries and search, logs, service map and services list, and (Milestone 2) query filters with matched spans, `remove_trace`, `remove_spans`, `with_trace_spans` and the scans, and (Milestone 4) log IDs, level and window filters on log search, the log histogram and the lines around a log, and (Milestone 5) segment files, a trace index with hot and cold traces, and logs in chunks with counts and positions from their metadata (see "Store" below) |
| `loctel-query` | core | TraceQL and LogQL lexers, parsers and evaluators, with errors that carry the position in the query. Pure: depends only on `loctel-model` (and `regex`) | implemented (Milestone 2): `QueryError`, `Pattern`, duration literals, the TraceQL lexer, parser and evaluator (`TraceView`, `evaluate`, `matches_span`) and LogQL (`logql::parse`, `LogQuery::matches`, `logql::labels`); the store, the rules and MCP (the `query` arguments of `search_traces` and `search_logs`) use them (see "Query languages" below) |
| `loctel-rules` | core | Discard rules: the rule model and `rules.toml`, `RuleSet` (the ingest filter for spans and logs, with a bounded memory of what was dropped), and the preview | implemented (Milestone 2): `RuleSpec`, `RulesFile`, `RuleSet`, `preview`; the engine runs it at ingest (see "Discard rules" and "Engine" below) |
| `loctel-ingest` | core | OTLP gRPC and HTTP receivers, protobuf and JSON to model conversion | implemented: gRPC and HTTP receivers, gzip, conversion, `partial_success` (see "Ingest" below) |
| `loctel-mcp` | core | MCP server (rmcp, Streamable HTTP), tools and their DTOs | implemented: nine read-only tools, the `Origin` check, both protocol generations and, from Milestone 2, the TraceQL and LogQL `query` arguments, the discard rules in `get_status` and the answer size budget (see "MCP" below) |
| `loctel-engine` | core | Wires store, ingest, MCP and the discard rules; settings, pause, rates; owns threads and runtime | implemented: store, OTLP receivers and MCP server, `settings.toml`, pause, rates, and (Milestone 2) the discard rules at ingest with live reload of `rules.toml` and the handle API, and (Milestone 4) rebinding the three ports live (see "Engine" below) |
| `loctel-meshgen` | test tooling | Fake microservice mesh scenarios (library and `meshgen` binary) | implemented: six scenarios and a sender over gRPC, HTTP protobuf and HTTP JSON, and (Milestone 5) the bulk generators `bulk_logs` and `bulk_traces` (see "Test tooling" below) |
| `loctel-perf` | test tooling | The performance budgets of design §10.3: a table of limits, the scenarios that measure them over ten million logs and spans, and the report (`publish = false`, binary `loctel-perf`) | implemented (Milestone 5): `ingest`, `trace_open_hot`, `trace_open_cold`, `logs_count`, `logs_at`, `histogram`, `traceql_first_page`, `logql_first_page`, `startup_headless`, `first_frame` and `rss_10m`; `cargo xtask perf` runs it in release and `ci.yml`'s `perf` job runs that (see "Test tooling" below) |
| `loctel-e2e` | test tooling | End-to-end tests through real OTLP and a real MCP client (`publish = false`) | implemented: a `Harness` over a real engine (with or without a rules file) and the tests of the transports, assembly, service map, agent flow, pause, release fixture, discard rules and queries (see "Test tooling" below) |
| `loctel-ui` | UI | GPUI views, the pure view models and the headless screenshot harness | implemented: the status view (with the discard rules line, from Milestone 2), the shared menu text, the theme, the pure models of the main window (`model`: formats, service colours, trace table rows, query bar state, map layout, waterfall rows, inspector text and the trace view's text), the main window's shell (`app`: sidebar, toolbar, `UiBackend`, the GPUI Kit theme bridge, `open_main_window`, and the live data entities `HomeModel`, `TraceModel` and, from Milestone 4, `LogsModel` and `RulesModel`), the views of its four screens (`views`: the service map, the trace table, and the trace view with its waterfall, inspector and span logs from Milestone 3; and, from Milestone 4, the Logs screen with its histogram and the Discard rules screen with its editor), the Settings window's and the menu bar popover's views (Milestone 4) and `testing::{render_png, render_window_png, Driver, headless_app, assert_snapshot}` behind the `screenshot` feature (see "UI" below) |
| `loctel-macos` | macOS | Status item, activation policy, login item (`cfg(target_os = "macos")`) | implemented, never run: the menu bar item (no menu since Milestone 4: a click opens the popover, `popover::toggle`, placed from the mouse by `popover::anchor`), the windows' lifecycle (`window::open_or_focus`, `show_screen`, `open_settings`: the Dock while the main or Settings window is open, Accessory after the last closes), Liquid Glass behind the main window's sidebar and toolbar and the popover's panel (`glass::attach`), the login item and the first-launch step; empty off macOS apart from plain-Rust parts that the Linux tests cover (see "macOS shell" below) |
| `loctel-app` | binary | The `loctel` binary: headless mode and GUI, bundle metadata | implemented: headless mode, on Linux the main window (it also quits cleanly on SIGINT and SIGTERM) and on macOS the menu bar app that opens the main window from its popover (see "Headless mode", "UI" and "macOS shell" below); `[package.metadata.bundle]` and `bundle/` hold what goes into `loctel.app` (see "Release pipeline" below) |
| `xtask` | tooling | Screenshots, next-version, release notes, MCP smoke test, performance budgets | implemented: `screenshots [--update]`, `run-shot`, `next-version`, `release-notes`, `mcp-smoke --url URL` and (Milestone 5) `perf [--only ID] [--json PATH]` (see "UI", "Test tooling" and "Release pipeline" below) |

Who depends on whom (an arrow reads "depends on"; edges that only tests use
are left out):

```
 loctel-app  (the `loctel` binary)
   |--> loctel-engine --+--> loctel-mcp ---+--> loctel-store ----> loctel-model
   |                    |                  |                 `---> loctel-query
   |                    |                  |--> loctel-ingest ---> loctel-model
   |                    |                  |--> loctel-query
   |                    |                  `--> loctel-model
   |                    |--> loctel-rules --> loctel-store, loctel-query, loctel-model
   |                    |--> loctel-ingest
   |                    |--> loctel-store
   |                    |--> loctel-query  (the `QueryError` of the preview)
   |                    `--> loctel-model
   |--> loctel-ui     (Linux window; owns `menu_text` and the `model`s) --> loctel-engine,
   |                                                    loctel-store, loctel-query, loctel-model
   `--> loctel-macos  (macOS only) --> loctel-engine, loctel-ui

 loctel-query   --> loctel-model                      (the store, the rules and MCP depend on it)
 loctel-rules   --> loctel-store, loctel-query, loctel-model
                                                      (the engine depends on it)

 loctel-meshgen --> loctel-model                      (test tooling)
 loctel-e2e     --> loctel-engine, loctel-meshgen     (+ rmcp's client)
 loctel-perf    --> loctel-engine, loctel-store, loctel-ingest, loctel-query, loctel-meshgen,
                    loctel-ui (its `screenshot` feature: the headless main window)
 xtask          --> rmcp's client                     (+ loctel-engine in its tests)
```

## Data flow

The shape since Milestone 2: the discard rules sit between the sink and the
store. Everything runs on Linux except the macOS status item, which is written
and type-checked but has never run:

```
 services (Docker or bare metal)
        |  OTLP gRPC :4317, OTLP HTTP :4318 (protobuf or JSON, gzip)
        v
 loctel-ingest ---- proto -> loctel-model types, invalid items -> partial_success
        |
        |  Sink::spans / Sink::logs
        v
 loctel-engine ---- EngineSink: paused? drop. Otherwise, under one lock:
     |    |          loctel-rules (RuleSet) judges the batch, what the store holds
     |    |          of dropped traces and spans is removed, then the survivors
     |    |          are metered and handed to the store. Also: settings, rules.toml
     |    |          and its reload, status, shutdown; owns the runtime.
     |    v
     |  loctel-store -- assembles traces, marks complete, finds failed-in span,
     |    |             keeps logs and the service map, evicts oldest whole trace
     |    |
     v    v
 loctel-mcp   loctel-ui  (main window, status view, menu text)  <-- loctel-macos (menu bar item, popover, windows, glass)
     |
     v
 AI agents  (MCP 2026-07-28, http://127.0.0.1:4319/mcp, read-only)
```

Readers (MCP, UI) read the store directly; the engine is not in the read path.
The UI gets the `EngineHandle` (wrapped in an `AppBackend`, as a `UiBackend`:
the store, the status, the discard rules, the settings, pause and the login
item) and the views read the store through it; the UI's own data flow is under
"UI" below. The MCP server gets
the store and a `StatusSource` that the engine implements, so `get_status` and
`EngineHandle::status()` read the same state.

A query takes two roads, and both end in `loctel-query`. An agent's `query`
argument is parsed by `loctel-mcp` and runs inside the store as one more filter
(on the way out). A discard rule's query is parsed by `loctel-rules` and runs on
the batch as it comes in (on the way in), over what the store already holds when
the query needs the whole trace. The two cannot disagree about what a query
means.

Telemetry is never persisted. Only `settings.toml` and `rules.toml` (the discard
rules) are written to disk.

## Domain model

`loctel-model` is the vocabulary every other crate speaks. It depends only on
`serde` and `serde_json`. The conventions below are tested in
`crates/loctel-model/tests/` and later crates rely on them:

- **IDs.** `TraceId` (16 bytes) and `SpanId` (8 bytes) parse from hex (either
  case) or raw bytes, and display as lowercase hex. A wrong length or a
  non-hex character is an `IdError`, never a panic. All zeros parses but is
  not `is_valid()`; the ingest layer decides what to do with it.
- **Service name.** `Resource::service_name()` is `service.name`, or
  `unknown_service` when it is absent, empty or not a string. Telemetry is
  never dropped for lacking a name.
- **Clock skew.** `Span::duration_nanos()` saturates, so an end before the
  start is a duration of 0.
- **Erroring.** `Span::is_error()` is status `Error` and nothing else. An
  exception event alone does not make a span erroring (design 9.4).
- **Exceptions.** `Span::exception()` reads the first event named
  `exception`, with missing `exception.*` attributes empty. A log record's
  `exception()` is `Some` only if it carries `exception.type`,
  `exception.message` or `exception.stacktrace`.
- **Peer.** `Span::peer()` is `server.address`, else `peer.service`
  (non-empty strings only).
- **Log dedup.** `LogRecord::dedup_key()` hashes the time, body, trace ID,
  span ID and service, so an exporter retry hashes the same. It is stable
  within a process only; never persist it.
- **Serialization.** Every type implements `serde::Serialize` as plain JSON:
  IDs and byte values as lowercase hex strings, attributes as JSON objects (in
  order), enums as `snake_case` strings. Every type also implements
  `serde::Deserialize` and reads back what it wrote. A binary format (postcard,
  which the store's segment files use) gets compact shapes (IDs as raw bytes,
  attribute values as tagged variants) and round-trips exactly; JSON round-trips
  too, except that a byte value comes back as its hex string (decision 0044).

## Store

`loctel-store` depends on `loctel-model`, `loctel-query`, `parking_lot`,
`serde` and (Milestone 5, for the segment files) `postcard`, `zstd`, `memmap2`,
`crc32fast` and `libc`. The details (API, rules and the reasons) are in
[`areas/store.md`](areas/store.md). What other crates rely on:

- **One entry point.** Everything goes through `Store` (`Clone + Send +
  Sync`, clones share data). The internals are replaced in Milestone 5, so
  nothing else may depend on them: the API is frozen, and every test of the
  crate runs against a store in memory and one that spills.
- **Hot and cold traces (Milestone 5).** With a `spill_dir`, a trace that has
  been complete for a while moves from memory to a temporary segment file, and
  comes back when a late span arrives. An index of every trace (start, end, count,
  and for a cold trace its summary) stays in memory, so lists, searches, counts and
  "oldest start" do not read the files. The spans of a cold trace are decoded on
  demand through a small cache. `Store::spilled_traces()` counts the cold ones;
  nothing else tells. The disk cap (`disk_cap_bytes`, 4 GiB) is enforced after every
  write by deleting the oldest segment file whole, with the traces and log chunks in
  it; a write that fails (a full volume) is waited out for a minute and reported in
  `StoreStats::spill_error` while the data stays in memory. Details and the reasons:
  [`areas/store.md`](areas/store.md), "Hot and cold traces", and
  [`decisions/0045-trace-index-and-spill-policy.md`](decisions/0045-trace-index-and-spill-policy.md).
- **Log chunks (Milestone 5).** Logs go to an open chunk in memory, which is
  sealed at 65,536 logs or after 10 s into one record of a logs segment file (or
  of memory, with no files). Chunks overlap in time when logs arrive late, so
  every list of logs is a merge of the chunks that reads a sealed one only when it
  has to. What is kept in memory about each sealed chunk (its range of ids, counts
  by level and service, counts per second, its traces) answers
  `Store::log_count` (a total) and `Store::logs_at` (rows from a position) and the
  histogram of an *indexed* filter (a window, levels, a service) without reading
  the logs, so the Logs screen can scroll ten million lines. A chunk on disk is a
  record of blocks of about a thousand logs, each with its own counts in memory,
  so a position is read from a block and not from a chunk of 65,536 logs (Task 6,
  [`decisions/0050-log-chunks-as-blocks.md`](decisions/0050-log-chunks-as-blocks.md)). Details, the cost and
  the reasons: [`areas/store.md`](areas/store.md), "Log chunks", and
  [`decisions/0046-log-chunks.md`](decisions/0046-log-chunks.md).
- **Time is injected.** `Store::new(config, Arc<dyn Clock>)`. `SystemClock`
  in the app, `ManualClock` in tests, so completeness is tested without
  sleeping.
- **Assembly.** Spans join a trace by trace ID in any order. The root is the
  parentless span (earliest start wins); a span whose parent is missing is an
  orphan until the parent arrives. `TraceSnapshot` is a copy, with spans
  sorted by start time, then span ID.
- **Dedup.** By span ID within a trace, first copy wins. Exporter retries
  are stored once and counted once.
- **Complete.** No new span for `completeness_quiet` (10 s by default),
  computed on read from arrival time, never from span timestamps.
- **Eviction.** Past `max_traces` the oldest trace by first arrival goes
  whole, after the whole batch has been applied.
- **Failure semantics.** Design §9.4 is implemented once, in the pure
  `classify`: `Failed` (root erroring), `ErrorsInside` (another span erroring,
  or errors while the root is missing) or `Ok`, plus the failed-in span (the
  earliest-starting erroring span with no erroring descendant, "+N more"). An
  exception event alone never makes a span erroring. Every trace view (the
  snapshot, summaries, MCP, the UI) takes its answer from it, computed on
  read.
- **Summaries and search.** `Store::summary` and `Store::search_traces` give
  one-row `TraceSummary` values, newest first, filtered by time, service,
  errors, duration and a TraceQL query (`TraceFilter::query`; the summary then
  lists the first 20 matched spans in `matched_span_ids`), with an opaque keyset
  cursor (`Page`, `PageRequest`).
- **Logs.** `Store::ingest_logs` keeps log records in time order (observed
  time when the timestamp is unset), drops exporter retries by
  `LogRecord::dedup_key()` and evicts the oldest past `max_log_records`.
  `Store::search_logs` filters by time window (`since` inclusive, `until`
  exclusive), service, minimum severity, a set of levels (`LogFilter::levels`),
  body text (case-insensitive), trace and span, newest first with the same keyset
  paging; `LogFilter::query` adds a LogQL query. `Store::search_logs_keyed` also
  returns each log's stable `LogId`, which `Store::log` and `Store::logs_around`
  read, `Store::log_histogram` counts a search into equal buckets (the Logs
  screen's volume bars), and (Milestone 5) `Store::log_count` and
  `Store::logs_at` give the total and the rows at a position of a list that is
  too long to page through. `Store::logs_for_span` returns
  a span's logs, oldest first, linked by ID only, so it works whether or not the
  span is held.
- **Removal and scans.** `Store::remove_trace` and `Store::remove_spans`
  delete stored data for the discard rules (a trace or spans, and their logs),
  reporting `Removed { spans, logs }`; what is held goes down and the
  `*_received` totals do not. `Store::with_trace_spans`, `scan_traces` and
  `scan_logs` read stored data in place, under the read lock, for the rules and
  their preview.
- **Service map and services.** `Store::service_map(TimeWindow)` and
  `Store::services(TimeWindow)` are computed on demand from the traces held:
  service nodes with entry-span counts, service-to-service edges, and
  database, queue and external nodes inferred from span attributes
  (design §9.2). External hosts appear only once their trace is complete.
  Node ids are unique within a kind, not across kinds.

## Query languages

`loctel-query` depends on `loctel-model` and `regex` only, so it builds and
tests on Linux with no UI and nothing else in the workspace is needed to use
it. The details (the supported subset, precedence, what is not supported and
why) are in [`areas/query.md`](areas/query.md); the choices and their reasons
are in [`decisions/0012-traceql-parser-choices.md`](decisions/0012-traceql-parser-choices.md),
[`decisions/0013-traceql-evaluator-choices.md`](decisions/0013-traceql-evaluator-choices.md)
and [`decisions/0014-logql-choices.md`](decisions/0014-logql-choices.md).
What other crates rely on:

- **Grafana's languages, not ours.** TraceQL follows Tempo (`expr.y`) and LogQL
  follows Loki, so queries pasted from Grafana work unchanged. Anything outside
  the supported subset is a `QueryError` that names the feature and its
  position, never a silent mismatch.
- **`QueryError { message, position }`.** The position is a 0-based character
  index; `Display` says `at column N: ...` with N counting from 1. Parsers
  return it for every failure and never panic, whatever the input (a depth and
  an operator limit keep a hand-edited rules file from overflowing the stack).
- **`Pattern`** is a compiled regex that is anchored (TraceQL, LogQL label
  matchers) or not (LogQL line filters).
- **`traceql::parse(text) -> TraceQuery`**, with `needs_whole_trace()`, which
  says whether the query can be judged span by span or needs the whole trace.
- **`TraceView::new(&[Span])`** indexes one trace's parent links and numbers
  the tree (O(n), no recursion), so ancestry is O(1). Spans whose parent is
  missing are roots of their own subtrees, and loops of parents are cut.
  **`TraceQuery::evaluate(&TraceView) -> Vec<usize>`** gives the matched spans'
  indices, ascending (the trace matches when it is not empty), and never fails.
  **`TraceQuery::matches_span(&Span) -> Option<bool>`** judges one span alone,
  without building a view, and is `Some` exactly when `needs_whole_trace()` is
  false.
  The semantics, and where they are narrower than Tempo's, are in
  `areas/query.md` and decision 0013.
- **`logql::parse(text) -> LogQuery`**, and **`LogQuery::matches(&LogRecord) ->
  bool`**, which cannot fail. **`LogQuery::service_hint()`** is the value of an
  exact `service_name="..."` selector matcher: every log the query matches is
  from that service, so the store can use it to skip the others. **`logql::labels(&LogRecord)`**
  (also `loctel_query::labels`) is the label set a query reads: resource and log
  attributes with dots turned into `_`, `service_name`, `level`, `scope_name`,
  `trace_id`, `span_id`; a label with an empty value does not exist. The
  selector reads it straight from the record and the full set is built only when
  `| json` or `| logfmt` runs. The semantics, the parsers, and where loctel
  differs from Loki (checked against Loki's own pipeline) are in `areas/query.md`
  and decision 0014.

## Discard rules

`loctel-rules` depends on `loctel-model`, `loctel-query`, `loctel-store`,
`serde`, `toml`, `dirs`, `anyhow` and `parking_lot`, and nothing else in the
workspace, so it builds and tests on Linux. The details (the file format, the
semantics, the memory, the counting, the preview) are in
[`areas/rules.md`](areas/rules.md); the choices and their reasons are in
[`decisions/0016-no-default-discard-rules.md`](decisions/0016-no-default-discard-rules.md)
and [`decisions/0017-rules-crate-choices.md`](decisions/0017-rules-crate-choices.md).
What other crates rely on:

- **No default rules.** A missing `rules.toml` is created holding no rules, only a
  comment header with the health-check example commented out
  (`RulesFile::write_empty`, `RulesFile::EXAMPLE_RULE`). loctel never decides what
  to discard.
- **`RuleSpec`** (`name`, `enabled`, `language`, `query`, `scope`) is a rule as
  saved, and **`RulesFile`** reads and writes `rules.toml` next to
  `settings.toml`: `load` gives `None` for a missing file and a one-line error for
  a bad one, `save` writes through a temporary file and a rename. A bad *query* is
  not a file error: the rule is kept and reported with its error, and the other
  rules apply.
- **`RuleSet::filter_spans(spans, &store) -> SpanVerdict`** and
  **`filter_logs(logs)`** judge a batch on its way into the store. The verdict
  lists the spans to keep and what the store holds that must go (`drop_traces`,
  `drop_spans`, and `drop_span_logs`, the dropped incoming spans whose earlier
  logs the store holds); the caller removes it with
  **`remove_dropped(&verdict, &store)`** (spans and their logs first, then
  traces, so each removal is credited to the first rule that dropped it) and
  then stores `keep`, as one step per batch. A trace-scope match drops the
  whole trace (stored spans, later spans and logs, for `DROPPED_TTL`); a
  span-scope match drops the matched spans and their descendants. Queries that
  need the whole trace are evaluated over the stored spans plus the batch.
- **`replace(specs)`** keeps a rule's `RuleId` and counters when its
  `(name, language, query, scope)` is unchanged, and keeps the memory of dropped
  data. **`statuses()`** gives `RuleStatus` (spec, query error, dropped spans,
  logs and traces) in file order, and **`totals()`** what the rules dropped since
  launch.
- **`preview(&spec, &store, since)`** says what one rule would have dropped of the
  stored data (`Preview`: traces, spans, logs, and how many of them had errors),
  or the parser's `QueryError`.
- **The engine runs it** (`loctel-engine`, "Engine" above): each batch the
  receivers accept goes receivers, then sink, then rules, then store, and
  `EngineHandle::{rules, set_rules, set_rule_enabled, preview_rule}` is the API
  the popover, the window and the Discard rules screen use. MCP cannot change
  rules.

## Ingest

`loctel-ingest` depends on `loctel-model`, the OTLP types from
`opentelemetry-proto`, `tonic` (gRPC) and `axum` (HTTP), and knows nothing
about the store. The details are in [`areas/ingest.md`](areas/ingest.md). What
other crates rely on:

- **A sink, not a store.** `start(IngestConfig, Arc<dyn Sink>)` converts what
  arrives and calls `Sink::spans` or `Sink::logs`. The engine's sink is where
  pause and the store live. The sink is called from async handlers, so it must
  not block, and it never sees an empty batch.
- **Two listeners, independent.** `IngestServers` carries a `ListenerStatus`
  for `otlp-grpc` and `otlp-http`. A port that cannot be bound is a
  `ListenerState::Failed` in the status, and the other listener keeps running.
  The same status types are shared with MCP and the engine.
- **Lenient in, strict about identity.** A span without a usable trace ID or
  span ID is rejected and counted in `partial_success`; a parent that is empty,
  all zeros or the span itself means "root"; a log is never rejected; a
  resource without `service.name` is kept (`unknown_service`).
- **Formats and limits.** Protobuf and OTLP/JSON over HTTP, gzip on both
  transports, requests up to 64 MiB (the libraries default to 4 MiB and 2 MB).
  Metrics are accepted and dropped.
- **Lifetime.** The servers run on the runtime that called `start`. Dropping
  `IngestServers`, or `shutdown(self)`, stops them gracefully. `start_grpc` and
  `start_http` start one receiver each, with a handle (`ReceiverShutdown`) that
  stops only it: how the engine moves one port at a time.

## MCP

`loctel-mcp` depends on `loctel-model`, `loctel-store`, `loctel-query` (to parse
the `query` arguments), `loctel-ingest` (only for `ListenerStatus`), `rmcp`,
`axum`, `tokio`, `schemars`, `chrono` and `serde`. The details (the tool table, the sizing rules, how to connect Claude
Code) are in [`areas/mcp.md`](areas/mcp.md); the choices and their reasons are
in [`decisions/0006-mcp-server-choices.md`](decisions/0006-mcp-server-choices.md)
(and [`0011`](decisions/0011-list-services-returns-an-object.md): every result is an
object, which 2025-11-25 clients require).
What other crates rely on:

- **Read-only and small.** Nine tools (`get_status`, `get_marker`,
  `list_services`, `get_service_map`, `search_traces`, `recent_failures`,
  `get_trace`, `get_span`, `search_logs`), each answering from the store with a
  compact summary by default, `limit` and `cursor` to page, and
  `structuredContent` with an output schema plus the same JSON as text. No tool
  changes anything.
- **Queries.** `search_traces` takes a TraceQL `query` and `search_logs` a LogQL
  one. The tool parses it with `loctel-query` (a parse error is a tool error
  with the column) and hands the parsed query to the store as a filter; a
  trace's summary then lists the spans it matched (`matched_span_ids`).
- **Answer size budget.** Every tool's default answer is at most
  `ANSWER_TEXT_BUDGET` (24,000 characters) on the fixtures of
  `crates/loctel-mcp/tests/budget.rs`.
- **`serve(addr, store, status)`** mounts rmcp's Streamable HTTP service at
  `/mcp` in an axum router and returns the `mcp` `ListenerStatus` and an
  `McpShutdown` (drop it to stop). It runs on the caller's runtime and refuses
  any address that is not loopback. A port that cannot be bound is a failed
  listener, not an error.
- **`Origin` and `Host`.** A request whose `Origin` names a host other than
  `localhost`, `127.0.0.1` or `[::1]` gets 403, as does a foreign `Host` (rmcp
  checks that one); no `Origin` is fine.
- **`StatusSource`.** The engine implements it. It is the only thing the MCP
  crate needs from the engine, so `loctel-mcp` does not depend on
  `loctel-engine`. The `StatusDto` it returns carries the discard rules
  (`RuleDto`, with what each has dropped), the rules file error and the dropped
  fraction, which the engine builds from the same `EngineStatus` the UI reads;
  MCP never changes rules.
- **Failure semantics are the store's.** The tools never re-derive "failed":
  they print `TraceSummary`, `TraceSnapshot` and `trace_exception`, so the UI
  and the agents agree (design 9.4).
- **`LoctelTools::call(name, args)`** runs a tool without a connection, which
  is how the tool tests drive them; `dispatch` routes by name and a test fails if
  it misses a registered tool.

## Engine

`loctel-engine` depends on `loctel-model`, `loctel-store`, `loctel-ingest`,
`loctel-mcp`, `loctel-rules`, `loctel-query`, `tokio`, `toml`, `serde`, `dirs`,
`parking_lot` and `anyhow`. It is the one object the menu
bar, the window and the headless binary hold, so none of them knows how the
store and the receivers are put together. Decisions and reasons:
[`decisions/0004-engine-lifecycle-and-settings.md`](decisions/0004-engine-lifecycle-and-settings.md)
and, for the rules,
[`decisions/0018-engine-rules-choices.md`](decisions/0018-engine-rules-choices.md).

- **Starting.** `Engine::start(EngineConfig { bind_ip, settings, store,
  rules_path })` returns an `EngineHandle`. It builds the `Store` (with `SystemClock`), a
  multi-thread tokio runtime on threads named `loctel-engine`, the OTLP
  receivers on `bind_ip` and the ports in `Settings` (loopback by default), and
  the MCP server on `127.0.0.1` at `mcp_port`, **always loopback whatever
  `bind_ip` says**. The caller needs no async `main`. Calling it from inside a
  tokio runtime is an `Err`, not a panic.
- **Ports.** A port that cannot be bound is a `ListenerState::Failed` in
  `EngineStatus::listeners` and does not stop the other endpoints (design §4).
  Port 0 picks a free one, and the listener status carries the real address.
- **Rebinding** (Milestone 4). `EngineHandle::rebind(Ports { otlp_grpc,
  otlp_http, mcp })` moves the listeners to new ports while the engine runs. A
  listener whose port is the one it was asked for already is left alone; each of
  the others is stopped and then bound at its new port, on the engine's runtime
  (so it works from any thread, even inside a runtime), and the call returns the
  three statuses in the usual order. A bind that fails leaves that listener
  `Failed` with the system's reason and never touches the other two. The engine
  holds one stop handle per listener (`loctel_ingest::start_grpc` and
  `start_http`, and the MCP server's own), and the listener statuses live behind
  a lock that `status()` and the MCP `get_status` both read. A port that another
  of the three has just been told to leave (a swap) is retried for up to a second,
  because a stopped server lets go of its socket a moment later. The reasons are
  in [`decisions/0035-live-port-rebinding.md`](decisions/0035-live-port-rebinding.md).
- **The handle.** `EngineHandle` is `Clone`, and every clone is the same
  engine: `store()`, `status()`, `set_paused`, `is_paused`, `mcp_url()`
  (`http://127.0.0.1:{port}/mcp` with the port really bound, `None` when the MCP
  port could not be bound), `rebind(Ports)`, the rules calls (`rules()`,
  `rules_path()`, `rules_generation()`, `set_rules`, `set_rule_enabled`,
  `preview_rule`; see "Discard rules" below) and `shutdown()`. `shutdown` is
  idempotent and safe from any thread, including inside a runtime (where it
  does not wait). The engine also stops when the last handle is dropped.
- **Pause.** `EngineSink` is the receivers' `Sink`. While paused it drops
  every batch before it reaches the rules, the store or the rate meters, and the
  receivers keep answering success, so exporters neither buffer nor retry
  (design §4). Dropped items are not counted anywhere (no rule counter moves),
  and they are not recoverable on resume.
- **Rules at ingest.** After the pause check, every batch goes through the
  discard rules (`loctel-rules`) before the store, under one `parking_lot` mutex
  so that batches from the gRPC and HTTP receivers apply in order: for spans
  `RuleSet::filter_spans`, then `remove_dropped` (the store loses what the
  verdict condemns, and the rules count it), then `ingest_spans(keep)`; for logs
  `filter_logs`, then `ingest_logs`. `EngineConfig::rules_path` is where
  `rules.toml` lives (`None`, the default, means in memory only); the engine
  writes a file holding no rules when there is none, loads it, and a background
  task polls it every `RULES_POLL` (2 s) and reloads it on a change, keeping the
  previous rules and setting `rules_file_error` when the file is bad. The
  details are in [`areas/rules.md`](areas/rules.md) ("In the engine").
- **Rates.** `EngineStatus::spans_per_sec` and `logs_per_sec` come from two
  `RateMeter`s: the count over the last 5 seconds divided by 5, in 100 ms
  buckets, of what the rules **accepted**. Time is passed in, never read, so
  tests need no sleeping. The first 5 seconds after start understate the rate.
- **Status.** `EngineStatus { paused, listeners, spans_per_sec, logs_per_sec,
  stats, rules, rules_file_error, dropped_spans, dropped_logs, dropped_fraction,
  disk_bytes, disk_cap_bytes, spill_error }` is a snapshot by value. Callers poll it (the UI) or print it (the
  headless binary). `listeners` is `otlp-grpc`, `otlp-http`, `mcp`, in that
  order. The same state, behind `StatusSource`, answers the MCP `get_status`
  tool, so the two cannot disagree.
- **Settings.** `Settings` is `settings.toml`: `otlp_grpc_port` (4317),
  `otlp_http_port` (4318), `mcp_port` (4319), `disk_cap_bytes` (4 GiB),
  `start_at_login` (true) and `first_launch_done` (false). Every key is
  optional, unknown keys are ignored, and `Settings::save` writes through a
  temporary file and a rename. `Settings::default_path()` is
  `dirs::config_dir()/loctel/settings.toml`, which on macOS is
  `~/Library/Application Support/loctel/settings.toml`. `load` is strict (a
  missing file is the defaults, an unreadable or invalid one is an `Err`
  naming the file); `load_or_default` never fails and never rewrites a bad file.
  `disk_cap_bytes` is given to the store at start (`Engine::start` sets
  `StoreConfig::disk_cap_bytes` from it, whatever the store config said), and
  `EngineHandle::set_disk_cap(bytes)` applies a new one at once (the app's
  `AppBackend::apply_settings` calls it after saving the file).
- **The run directory.** `EngineConfig::spill_base` is where the store's temporary
  segment files go: `Some(base)` makes `base/run-<pid>-<start>/` at start (deleting
  the directories of runs whose process is gone first), has the store spill into it
  and deletes it at `shutdown` (and when the last handle is dropped). The app passes
  `default_spill_base()`, `$TMPDIR/loctel`, unless it was started with `--no-spill`;
  the default (`None`) keeps everything in memory, which is what the tests want. A
  directory that cannot be made does not stop the engine: it runs in memory and says
  why in `EngineStatus::spill_error`. After `shutdown` the store still answers for
  what it holds in memory, and what was spilled is gone with the directory.

## Headless mode

`loctel --headless` runs the engine with no window and no menu bar item. It is
what the end-to-end tests, the release smoke test and agents on Linux use.

```
loctel [--headless] [--grpc-port N] [--http-port N] [--mcp-port N]
       [--settings PATH] [--rules PATH] [--completeness-ms N] [--no-spill]
```

- **Configuration.** Settings come from `--settings PATH`, else the per-user
  file; a missing file is fine, and a broken one is reported on stderr and
  replaced by the defaults for this run. The port flags win over the file.
  `--completeness-ms` sets how long a trace must be quiet to count as complete
  (tests use small values). `--no-spill` keeps everything in memory: without it
  the engine moves what it no longer needs in memory to temporary files in a run
  directory under the system's temporary directory, up to the disk cap of the
  settings, and deletes it on shutdown (see "The run directory" above). Port 0
  picks a free port. The file is only read:
  headless mode never writes it. The discard rules are in `--rules PATH`, else
  `rules.toml` in the directory of the settings file (so beside a scratch
  `--settings` file, and in the per-user folder otherwise). Unlike the settings,
  that file is created when it is missing (holding no rules), and rewritten
  when the rules change through the handle.
- **Output.** Stdout carries one JSON line per listener (`otlp-grpc`,
  `otlp-http`, `mcp`, in that order) and then one line for the rules, once, when
  the engine is up, and nothing else:
  `{"event":"listener","name":"otlp-grpc","state":"listening","address":"127.0.0.1:4317"}`
  or `{"event":"listener","name":"otlp-http","state":"failed","error":"..."}`,
  then `{"event":"rules","path":"/.../rules.toml","count":0,"error":null}` (how
  many rules the file holds, and why it could not be used, if so).
  A script that starts loctel with ports 0 reads its real ports from the
  listener lines. Logs and warnings go to stderr (quiet by default, `RUST_LOG` to
  change).
- **Lifetime.** The signal handlers are installed before the lines are
  printed, so a signal sent as soon as a line is read is never lost. SIGINT or
  SIGTERM shuts the engine down and exits 0. A taken port is not fatal: the
  process runs with what it could bind. An unknown flag exits 2, and an
  engine that cannot start exits 1.
- **Without `--headless`** on Linux the binary opens the main window (see
  "UI" below) and prints the same lines (listeners, then rules); closing the
  window, SIGINT and SIGTERM all quit with the engine shut down and exit 0.
  With no display it says so on stderr and runs headless. On macOS it is the menu bar app (see
  "macOS shell" below), and prints the same lines. On any other OS it does the same as
  headless.
- **Tests.** `crates/loctel-app/tests/headless.rs` runs the built binary:
  the lines (the rules line included), SIGTERM and SIGINT, a taken port,
  settings file and flags, and (Milestone 5) the run directory: it is made in
  `$TMPDIR/loctel/` (the test points `TMPDIR` beside its settings file, so the
  developer's own is never touched), a dead run's directory is deleted at start, and
  this run's is deleted at exit; `--no-spill` makes none.
  `crates/loctel-app/tests/window.rs` runs it without
  `--headless` under `xvfb-run` (Linux; skips with a printed notice without
  xvfb): the window stays open for 3 s, the startup lines are printed and
  SIGTERM ends it with status 0.

## UI

`loctel-ui` is the thin GPUI layer: it depends on the engine (for
`EngineStatus` and `EngineHandle`), on the store, the query crate and the model
(for the `model` module below) and on GPUI (`gpui-pre`, `gpui-pre-platform`,
`gpui-pre-wgpu`, all pinned `=0.3.9`, and GPUI Kit `=0.8.0` for the main
window's components), and the core depends on none of it. The
details, the theme tokens and the workflow are in
[`areas/ui.md`](areas/ui.md); the choices and their reasons are in
[`decisions/0008-ui-and-screenshot-choices.md`](decisions/0008-ui-and-screenshot-choices.md).

- **The status view** (Milestones 1 and 2; since Milestone 3 nothing opens it, and it keeps its tests as a standalone view). `StatusView` draws a `StatusModel`, a
  plain copy of what `EngineHandle::status()` returns
  (`StatusModel::from_status`, which now also counts the discard rules that
  are on and the ones among them that do not parse, and carries the rules
  file error as a flag and the share dropped), so it is tested by building the
  model by hand. The window refreshes it every second and shows the rules line
  under the rates. The text of the status line, the rules line and the rates is
  in `menu_text`, shared with the macOS popover so the two cannot drift.
- **The `model` module** (`crates/loctel-ui/src/model/`, from Milestone 3) is
  everything the main window shows that is not a pixel, as plain Rust over the
  store's `TraceSummary` and the TraceQL parser: `format` (durations, clock
  times, rates, error shares, truncation), `colors` (`ServiceColors`, the
  first-seen order of services), `trace_table` (`TraceRow`, `rows`,
  `failed_in_text`), `query_bar` (`QueryState`, `validate`,
  `service_filter`), `map_layout` (`layout`, `MapLayout`, `LayoutMemory`:
  the service map's nodes and edges placed in columns and rows, stable as
  services come and go; `route_edges`: where each edge is drawn, through the
  gaps between rows so that none passes under a node), `waterfall` (`Waterfall`, `WaterfallRow`,
  `initial_selection`: one trace as rows in tree order with their bars, folding,
  find and the errors path, kept by span ID as the trace fills in),
  `inspector` (`SpanDetails`, `span_details`: the text of the selected span's
  panel) and `trace_view` (the trace screen's header, the find box's count and
  the span logs' lines). Milestone 4 adds `logs` (`LogRow`, `rows`, `details`,
  `validate`, `span_logs_query`: the Logs list, an expanded line and the LogQL
  bar), `histogram` (`bars`: the log volume bars and their labels), `rules`
  (`RuleRow`, `rows`, `dropped_line`, `Draft`, `suggest_from_log`: the Discard
  rules cards, the editor's draft and the rule a log suggests), `settings`
  (`Form`: the Settings window's port and disk cap validation) and `popover` (`from_status`:
  the menu bar popover's pill, stats and endpoints). It has no GPUI types, so
  `tests/model_format.rs`, `tests/model_table.rs`, `tests/model_map.rs`,
  `tests/model_waterfall.rs`, `tests/model_trace_view.rs`, `tests/model_logs.rs`,
  `tests/model_rules.rs` and `tests/model_misc.rs` pin every string and every
  place with plain assertions. The formats, the layout and the waterfall rules are in
  [`areas/ui.md`](areas/ui.md) ("Models").
- **The `app` module** (`crates/loctel-ui/src/app/`, Milestone 3) is the main
  window's shell and its data: `MainWindow` (the floating sidebar with its
  status box, the toolbar (with `HomeControls`: Live, the time range, Pause and
  Clear all over the `HomeModel`) and the content panel, which show a `Screen`:
  `Home`, `Trace(id)`, `Logs { query }` or `Rules { draft }`), the `UiBackend` trait that gives views the `Store` and the
  `EngineStatus`, the discard rules, the settings, pause and the login item
  (`AppBackend`, over an `EngineHandle`, the settings path and an optional
  `LoginItem`, implements it; tests use `testing::FixedBackend`, which keeps
  all of it in memory), `open_main_window`, the `Backdrop` and
  `GlassRegion` types through which a platform learns where the glass parts
  are, and `kit_theme::apply`, the one place loctel's `Theme` is mapped onto
  GPUI Kit's. Three entities read the store over time (a fourth, `RulesModel`, below,
  reads it only for a preview), all on GPUI's background
  executor and never in a frame: `HomeModel` (`app::data`) every 500 ms, behind
  a `TimeRange`, the errors-only and slow-only toggles and the query bar (at
  most 1,000 rows), building the placed service map and the table's rows
  (`HomeData`) and owning the session's `ServiceColors`, which the views share;
  and `TraceModel` (`app::trace_data`) for the open trace, every 500 ms until
  it is complete and every 2 s after, rebuilding the waterfall by span ID so a
  trace that is still arriving does not jump, and reading the selected span's
  logs; and `LogsModel` (`app::logs_data`, Milestone 4) for the Logs screen:
  the list in pages of 500 (at most 20,000 rows), the live tail that joins the
  newest page every 500 ms without ever repeating a line (and stands still while
  the list is scrolled away from the top), the histogram every 2 s and the lines
  around a log, reading only while the Logs screen shows, and, from Milestone 5,
  for a search the store counts that matches more than 20,000 lines, a
  `model::window::RowWindow` over all of them read by position (`Store::log_count`
  and `logs_at`), so the list scrolls through ten million lines with a few hundred
  rows in memory ([decision 0051](decisions/0051-logs-list-at-ten-million-lines.md));
  and `RulesModel`
  (`app::rules_data`, Milestone 4) for the Discard rules screen, which reads the
  engine's rules and the share dropped once a second (in memory, not the store)
  while its screen shows and the store only for the editor's five-minute preview,
  and holds the editor's draft and the notice that the rules file changed behind
  it. See
  [`areas/ui.md`](areas/ui.md) ("The main window", "Live data",
  "The trace view", "The Logs screen"), [`decisions/0024-gpui-kit.md`](decisions/0024-gpui-kit.md),
  [`decisions/0025-live-home-data.md`](decisions/0025-live-home-data.md) and
  [`decisions/0028-trace-view.md`](decisions/0028-trace-view.md) and
  [`decisions/0037-logs-screen.md`](decisions/0037-logs-screen.md).
- **The `views` module** (`crates/loctel-ui/src/views/`, Milestone 3) holds the
  views of the main window's screens, thin GPUI views that lay out what a model gives
  them and write what the user does back into it: `MapView` (the live service
  map on one `canvas`, with pan, zoom and a click on a service that filters the
  table), `TraceTableView` (the TraceQL bar, the toggles and the table of
  traces; a click on a row opens the trace) and `TraceView` with its
  `WaterfallView`, `InspectorView` and `SpanLogsView` (Milestone 3), and the
  Logs screen's `LogsView` with its `HistogramView` (Milestone 4: the LogQL bar,
  the level toggles, the volume bars and a variable-height list whose rows expand
  where they stand), the Discard rules screen's `RulesView` with its
  `RuleEditorView` (Milestone 4: the cards with their switches and counts, and the
  editor with live validation and the five-minute preview), and the two windows of
  their own, `SettingsView` (the ports
  and the login switch, opened by `app::open_settings_window`) and `PopoverView`
  (Milestone 4 Task 8: the menu bar popover, 336 wide, over a `PopoverActions`
  trait that is everything it asks of the app, so the platform crate decides what
  a click does and where the panel opens, and the view is built and screenshotted
  on Linux; with an optional `Backdrop` it is glass and reports one `GlassRegion`).
  They use GPUI Kit's table, input, toggles, split panel,
  scroll areas and tooltips. See
  [`areas/ui.md`](areas/ui.md) ("Screens and views at a glance" is the map; "The
  popover") and [`decisions/0041-popover-view.md`](decisions/0041-popover-view.md).
- **The data flow of the main window** is one direction, with the user's
  actions coming back the same way:

  ```
  EngineHandle --(AppBackend: UiBackend)--> store() / status() / rules / settings / utc_offset()
        |
  HomeModel, TraceModel,  read the Store on the background executor, then on
  LogsModel               the foreground build the rows, the layout, the waterfall,
        |                 the log pages and the histogram (RulesModel reads the
        |                 engine's rules in memory, and the Store only for a preview)
        v
  model::*   (pure: strings, rows, positions, colour indices; no GPUI)
        |
        v
  views::*   (lay out with GPUI Kit and Theme tokens)  -->  pixels
        ^
        '-- what the user does: set_query, select_service, select, toggle, navigate
  ```

  The tests put a `FixedBackend` over a store loaded from meshgen on a
  `ManualClock` in place of the engine, so every screenshot is deterministic.
  Adding a screen: the `add-screen` skill (`.claude/skills/add-screen/SKILL.md`) has
  the steps, and [`areas/ui.md`](areas/ui.md) ("Adding a screen") the rules.
- **Screenshot tests** (`crates/loctel-ui/tests/screenshots.rs`, and `screens.rs`
  for the main window through `testing::render_window_png` and `Driver`) render views
  through GPUI's headless renderer (software wgpu, no GPU or display) with a
  bundled font, and compare pixel for pixel with references in
  `crates/loctel-ui/tests/snapshots/`. The `screenshot` feature is on in every
  test build of the crate, so `cargo test --workspace` runs them. On a failure
  the actual and diff images go to `target/screenshot-diffs/`, which `ci.yml`
  uploads.
- **`cargo xtask screenshots [--update]`** runs (or regenerates) the references;
  **`cargo xtask run-shot`** runs the real window under Xvfb and writes
  `target/run-shot.png`. The repo skills `run-and-screenshot` and
  `update-screenshots` are the procedures.

## macOS shell

`loctel-macos` is the only crate with `objc2` or `unsafe` code, and it is empty
off macOS (its dependencies are macOS-only) apart from plain-Rust parts that
hold decisions and are compiled for tests everywhere: the popover's anchoring
(`popover::anchor` and `popover::containing_screen`), the first-launch step, the
icon files and the y flip of the glass frames. It
depends on the engine, `loctel-ui` (for `menu_text`, the main window and the
popover's view), `gpui-pre`, `gpui-kit` (its `Root`, which the popover's window
sits in like every loctel window), `gpui-tray` (the `NSStatusItem`), `objc2`,
`objc2-app-kit`, `objc2-foundation`, `objc2-service-management`,
`raw-window-handle` and `dispatch2`. The details, the popover and the list of
what no one has seen work yet are in [`areas/macos.md`](areas/macos.md); the
choices and their reasons are in
[`decisions/0009-macos-check-from-linux.md`](decisions/0009-macos-check-from-linux.md)
and, for the popover, [`decisions/0042-popover.md`](decisions/0042-popover.md).

- **One entry point.** `loctel_macos::install(cx, engine, settings_path)`, called
  by `loctel-app` inside `Application::run` after it set
  `ActivationPolicy::Accessory` and `QuitMode::Explicit`. It opens no window.
- **The main window** is `loctel_macos::open_or_focus(cx, engine, settings_path)`: the
  popover's `Open loctel` (through `window::show_screen`) and GPUI's reopen event (`Application::on_reopen`, registered by
  `loctel-app`) call it. It switches to `ActivationPolicy::Regular` (Dock and
  ⌘-Tab), opens `loctel_ui::app::open_main_window` and activates the app; when
  the window closes the policy goes back to `Accessory` and the app keeps
  running. See "The main window's lifecycle" in `areas/macos.md`.
- **Liquid Glass** is `loctel_macos::glass::attach`, which `open_or_focus` passes
  to `open_main_window`: native `NSGlassEffectView`s under GPUI's view, behind
  the sidebar and the toolbar, in a transparent window. The shell (`loctel-ui`)
  tells the `Backdrop` where those two parts are; the frames are applied from
  the main queue, never inside a GPUI frame. Without glass the window is
  `Blurred` and the panels are opaque. See "Liquid Glass" in `areas/macos.md`
  and [`decisions/0030-liquid-glass.md`](decisions/0030-liquid-glass.md).
- **The popover** replaces the menu of Milestones 1 and 2 (decision
  [0042](decisions/0042-popover.md)). The status item has no menu; its activation
  callback is `popover::toggle`, which closes the popover or opens it as a GPUI
  `PopUp` window under the click: `NSEvent.mouseLocation` and the screen's
  `visibleFrame` into the pure `popover::anchor`, because `gpui-tray` reports no
  frame for its item. The window holds `loctel_ui::views::PopoverView` (the status
  and rules lines are `menu_text`'s, the same functions the Linux window uses) in
  glass; its actions (`MacActions`) open the main window or Settings, pause, copy
  the MCP address, clear, switch the login item and quit. It closes on
  deactivation, Esc and after an action that opens a window. See "The popover" in
  `areas/macos.md`.
- **Start at login** is `SMAppService.mainAppService`, and the system is the
  source of truth. The first launch turns it on once (`first_launch_done`).
- **Checked from Linux.** `script/check-macos.sh` type-checks and lints the
  macOS code for `aarch64-apple-darwin`, with a rustc wrapper that stands in for
  the one build step that only runs on a Mac and a stand-in C compiler
  (`script/stub-cc.sh`) for the build scripts that compile C (`zstd-sys`);
  `ci.yml` runs it in the Linux job. Nothing runs it: the macOS runner is only
  for releases.

## Test tooling

`loctel-meshgen` generates the data every test, screenshot and manual run
uses, and sends it to a running loctel. It depends on `loctel-model`, the OTLP
types, `tonic` and `reqwest` (clients), and a seeded ChaCha RNG; `loctel-ingest`
and `loctel-store` are only dev-dependencies, used to check that what it
generates is valid and reaches the right verdict. The scenarios, the frozen
`CheckoutFailure` table and the command line are in
[`areas/testing.md`](areas/testing.md); the choices and their reasons are in
[`decisions/0005-meshgen-scenario-choices.md`](decisions/0005-meshgen-scenario-choices.md).

- **Generate, then send.** `generate(&Scenario, seed, base_unix_nano)` is a pure
  function to `Generated { batches, trace_ids }`: the same inputs give the same
  bytes. `send(&Generated, &Transport, time_scale)` delivers the batches and
  sleeps their `delay_before * time_scale`. Tests use `generate` alone when
  they feed a store directly, and both when they go through real OTLP.
- **Frozen content.** `CheckoutFailure` is what the mockups show. Changing it
  means updating screenshots and e2e expectations.
- **Scenarios do not share IDs.** Each reads its own RNG stream, so one seed
  can feed several scenarios into the same loctel.

`loctel-e2e` is the proof that the pieces work together. It depends on the
engine, `loctel-meshgen` and rmcp's client (and `tempfile`, for its tests), and
has no code of its own beyond the harness: a `Harness` starts an `Engine` (every
port 0, a short completeness time, and with `start_with_rules` a `rules.toml` the
test wrote), sends meshgen scenarios to its real OTLP ports over gRPC, HTTP
protobuf and HTTP JSON, and reads the answer through an rmcp client over
Streamable HTTP, the way an agent does. Tests cover each transport, late-span
assembly, the service map, the marker flow, health checks stored without rules,
pause, and, from Milestone 2, the health-check rule from the rules file (and
its edit while running), the late spans of a dropped trace, TraceQL structural
queries, LogQL queries and the errors for queries that do not parse. See
"End-to-end tests" in [`areas/testing.md`](areas/testing.md) and
[`decisions/0007-e2e-harness-choices.md`](decisions/0007-e2e-harness-choices.md)
and [`decisions/0020-e2e-rules-and-query-tests.md`](decisions/0020-e2e-rules-and-query-tests.md).

`loctel-perf` is the performance check. A table in code (`budget.rs`) holds each budget
(an id, what is measured, the limit) and judges measurements against it; the scenarios
build their data in the process through the store, with meshgen's bulk generators (a
store on a manual clock that follows the data, so that traces complete and spill and
logs seal as in a long run) and measure: the log budgets over ten million logs, the span
search over ten million spans, the 100,000-span trace hot and cold, `ingest` through a
real engine over OTLP HTTP, the start of the release `loctel --headless` and the first
frame of the main window in a process each. `cargo xtask perf` builds it and `loctel` in
release and runs it (exit 1 on a breach); the `perf` job of `ci.yml` runs that beside
`gate`. The scenarios, the table, the limits that are raised for the harness and the
numbers are in [`areas/testing.md`](areas/testing.md) ("Performance budgets") and
[`decisions/0049-performance-harness-choices.md`](decisions/0049-performance-harness-choices.md);
the first run's one miss, and the blocks that fixed it, in
[`decisions/0050-log-chunks-as-blocks.md`](decisions/0050-log-chunks-as-blocks.md).

## Release pipeline

Every merge to main becomes a GitHub release with an unsigned `.dmg`. It is the
only place the `macos-26` runner is used. The steps, the version and notes
rules, the bundle contents, the Gatekeeper steps and the list of what has not
run on a Mac yet are in [`areas/release.md`](areas/release.md).

```
 push to main
     |
     v
 .github/workflows/release.yml  (macos-26; concurrency: one release at a time)
     |
     |-- cargo xtask next-version -----> vX.Y.Z   (highest tag, patch + 1)
     |-- cargo xtask release-notes ----> notes.md (commit subjects since the previous tag
     |                                             + docs/milestones/mac-checklist.md
     |                                             + install steps)
     |-- cargo build --release -p loctel-app
     |-- script/bundle-mac.sh vX.Y.Z
     |       cargo-bundle (crates/loctel-app/Cargo.toml metadata, bundle/AppIcon.svg,
     |       LSUIElement) --> loctel.app --> codesign --sign - (ad hoc)
     |       --> staged with an Applications symlink --> hdiutil --> loctel-vX.Y.Z.dmg
     |-- smoke: loctel.app --headless on ports 24317-24319
     |       <-- curl crates/loctel-e2e/fixtures/one-span.json (OTLP/JSON)
     |       <-- cargo xtask mcp-smoke (rmcp client: lists `recent_failures`)
     `-- gh release create vX.Y.Z loctel-vX.Y.Z.dmg --notes-file notes.md
```

`xtask` depends on rmcp's client, `reqwest` and `tokio` for `mcp-smoke`; `one-span.json` is kept valid by a test in `loctel-e2e`
(`tests/release_fixture.rs`).

## Platform

macOS 26 or later on Apple Silicon is the product. Linux x86_64 exists for
development and tests only. Mac-only features are unverified until a milestone
check on a real Mac (design §10.4).
