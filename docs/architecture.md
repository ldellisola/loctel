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
`superpowers/plans/2026-10-21-loctel-m1-core.md`, all exists. Milestone 2
(`superpowers/plans/2026-10-21-loctel-m2-query-rules.md`) adds `loctel-query`
and `loctel-rules`. The change that adds a crate updates this table and the
diagram below it.

| Crate | Layer | Owns | Status |
|---|---|---|---|
| `loctel-model` | core | Domain types (spans, logs, resources, IDs, severity), independent of OTLP | implemented (see "Domain model" below) |
| `loctel-store` | core | In-memory store: trace assembly, completeness, dedup, eviction, failure semantics, search (with TraceQL and LogQL filters), logs, removal and scans, service map | implemented: assembly, completeness, dedup, eviction, failure semantics, trace summaries and search, logs, service map and services list, and (Milestone 2) query filters with matched spans, `remove_trace`, `remove_spans`, `with_trace_spans` and the scans (see "Store" below) |
| `loctel-query` | core | TraceQL and LogQL lexers, parsers and evaluators, with errors that carry the position in the query. Pure: depends only on `loctel-model` (and `regex`) | in progress (Milestone 2): `QueryError`, `Pattern`, duration literals, the TraceQL lexer, parser and evaluator (`TraceView`, `evaluate`, `matches_span`) and LogQL (`logql::parse`, `LogQuery::matches`, `logql::labels`); the store and the rules use them, and MCP will in a later task (see "Query languages" below) |
| `loctel-rules` | core | Discard rules: the rule model and `rules.toml`, `RuleSet` (the ingest filter for spans and logs, with a bounded memory of what was dropped), and the preview | implemented (Milestone 2, Task 5): `RuleSpec`, `RulesFile`, `RuleSet`, `preview`; the engine does not run it yet (Task 6) (see "Discard rules" below) |
| `loctel-ingest` | core | OTLP gRPC and HTTP receivers, protobuf and JSON to model conversion | implemented: gRPC and HTTP receivers, gzip, conversion, `partial_success` (see "Ingest" below) |
| `loctel-mcp` | core | MCP server (rmcp, Streamable HTTP), tools and their DTOs | implemented: nine read-only tools, the `Origin` check, both protocol generations (see "MCP" below) |
| `loctel-engine` | core | Wires store, ingest and MCP; settings, pause, rates; owns threads and runtime | implemented: store, OTLP receivers and MCP server, `settings.toml`, pause, rates (see "Engine" below) |
| `loctel-meshgen` | test tooling | Fake microservice mesh scenarios (library and `meshgen` binary) | implemented: six scenarios and a sender over gRPC, HTTP protobuf and HTTP JSON (see "Test tooling" below) |
| `loctel-e2e` | test tooling | End-to-end tests through real OTLP and a real MCP client (`publish = false`) | implemented: a `Harness` over a real engine and six tests (see "Test tooling" below) |
| `loctel-ui` | UI | GPUI views and the headless screenshot harness | implemented: the status view, the shared menu text, the theme, and `testing::{render_png, assert_snapshot}` behind the `screenshot` feature (see "UI" below) |
| `loctel-macos` | macOS | Status item, activation policy, login item (`cfg(target_os = "macos")`) | implemented, never run: the menu bar item and its menu, the login item and the first-launch step; empty off macOS apart from three plain-Rust modules that the Linux tests cover (see "macOS shell" below) |
| `loctel-app` | binary | The `loctel` binary: headless mode and GUI, bundle metadata | implemented: headless mode, on Linux the status window and on macOS the menu bar app (see "Headless mode", "UI" and "macOS shell" below); `[package.metadata.bundle]` and `bundle/` hold what goes into `loctel.app` (see "Release pipeline" below) |
| `xtask` | tooling | Screenshots, next-version, release notes, MCP smoke test | implemented: `screenshots [--update]`, `run-shot`, `next-version`, `release-notes` and `mcp-smoke --url URL` (see "UI" and "Release pipeline" below) |

Who depends on whom (an arrow reads "depends on"; edges that only tests use
are left out):

```
 loctel-app  (the `loctel` binary)
   |--> loctel-engine --+--> loctel-mcp ---+--> loctel-store ----> loctel-model
   |                    |                  |                 `---> loctel-query
   |                    |                  |--> loctel-ingest ---> loctel-model
   |                    |                  `--> loctel-model
   |                    |--> loctel-ingest
   |                    |--> loctel-store
   |                    `--> loctel-model
   |--> loctel-ui     (Linux window; owns `menu_text`) --> loctel-engine
   `--> loctel-macos  (macOS only) --> loctel-engine, loctel-ui

 loctel-query   --> loctel-model                      (the store and the rules depend on it; MCP
                                                       will, in Milestone 2)
 loctel-rules   --> loctel-store, loctel-query, loctel-model
                                                      (the engine will depend on it, in Milestone 2)

 loctel-meshgen --> loctel-model                      (test tooling)
 loctel-e2e     --> loctel-engine, loctel-meshgen     (+ rmcp's client)
 xtask          --> rmcp's client                     (+ loctel-engine in its tests)
```

## Data flow

The Milestone 1 shape. Everything runs on Linux except the macOS status item,
which is written and type-checked but has never run:

```
 services (Docker or bare metal)
        |  OTLP gRPC :4317, OTLP HTTP :4318 (protobuf or JSON, gzip)
        v
 loctel-ingest ---- proto -> loctel-model types, invalid items -> partial_success
        |
        |  Sink::spans / Sink::logs
        v
 loctel-engine ---- EngineSink: paused? drop. Otherwise meter the rate and
     |    |          hand the batch to the store. Also: settings, status,
     |    |          shutdown; owns the runtime.
     |    v
     |  loctel-store -- assembles traces, marks complete, finds failed-in span,
     |    |             keeps logs and the service map, evicts oldest whole trace
     |    |
     v    v
 loctel-mcp   loctel-ui  (status view, menu text)  <-- loctel-macos (status item)
     |
     v
 AI agents  (MCP 2026-07-28, http://127.0.0.1:4319/mcp, read-only)
```

Readers (MCP, UI) read the store directly; the engine is not in the read path.
The UI gets the `EngineHandle` and asks it for the status. The MCP server gets
the store and a `StatusSource` that the engine implements, so `get_status` and
`EngineHandle::status()` read the same state.

Telemetry is never persisted. Only `settings.toml` (and discard rules from
Milestone 2) is written to disk.

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
  order), enums as `snake_case` strings.

## Store

`loctel-store` depends on `loctel-model`, `loctel-query`, `parking_lot` and
`serde`. The details (API, rules and the reasons) are in
[`areas/store.md`](areas/store.md). What other crates rely on:

- **One entry point.** Everything goes through `Store` (`Clone + Send +
  Sync`, clones share data). The internals are replaced in Milestone 5, so
  nothing else may depend on them.
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
  `Store::search_logs` filters by time, service, minimum severity, body text
  (case-insensitive), trace and span, newest first with the same keyset
  paging; `LogFilter::query` adds a LogQL query. `Store::logs_for_span` returns
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
  `drop_spans`); the caller removes it with **`remove_dropped(&verdict, &store)`**
  and then stores `keep`, as one step per batch. A trace-scope match drops the
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
  `IngestServers`, or `shutdown(self)`, stops them gracefully.

## MCP

`loctel-mcp` depends on `loctel-model`, `loctel-store`, `loctel-ingest` (only
for `ListenerStatus`), `rmcp`, `axum`, `tokio`, `schemars`, `chrono` and
`serde`. The details (the tool table, the sizing rules, how to connect Claude
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
  `loctel-engine`.
- **Failure semantics are the store's.** The tools never re-derive "failed":
  they print `TraceSummary`, `TraceSnapshot` and `trace_exception`, so the UI
  and the agents agree (design 9.4).
- **`LoctelTools::call(name, args)`** runs a tool without a connection, which
  is how the tool tests drive them; `dispatch` routes by name and a test fails if
  it misses a registered tool.

## Engine

`loctel-engine` depends on `loctel-model`, `loctel-store`, `loctel-ingest`,
`loctel-mcp`, `tokio`, `toml`, `serde`, `dirs` and `anyhow`. It is the one object the menu
bar, the window and the headless binary hold, so none of them knows how the
store and the receivers are put together. Decisions and reasons:
[`decisions/0004-engine-lifecycle-and-settings.md`](decisions/0004-engine-lifecycle-and-settings.md).

- **Starting.** `Engine::start(EngineConfig { bind_ip, settings, store })`
  returns an `EngineHandle`. It builds the `Store` (with `SystemClock`), a
  multi-thread tokio runtime on threads named `loctel-engine`, the OTLP
  receivers on `bind_ip` and the ports in `Settings` (loopback by default), and
  the MCP server on `127.0.0.1` at `mcp_port`, **always loopback whatever
  `bind_ip` says**. The caller needs no async `main`. Calling it from inside a
  tokio runtime is an `Err`, not a panic.
- **Ports.** A port that cannot be bound is a `ListenerState::Failed` in
  `EngineStatus::listeners` and does not stop the other endpoints (design §4).
  Port 0 picks a free one, and the listener status carries the real address.
- **The handle.** `EngineHandle` is `Clone`, and every clone is the same
  engine: `store()`, `status()`, `set_paused`, `is_paused`, `mcp_url()`
  (`http://127.0.0.1:{port}/mcp` with the port really bound, `None` when the MCP
  port could not be bound) and `shutdown()`. `shutdown` is idempotent and safe
  from any thread, including inside a runtime (where it does not wait). The
  engine also stops when the last handle is dropped.
- **Pause.** `EngineSink` is the receivers' `Sink`. While paused it drops
  every batch before it reaches the store or the rate meters, and the receivers
  keep answering success, so exporters neither buffer nor retry (design §4).
  Dropped items are not counted anywhere, and they are not recoverable on
  resume.
- **Rates.** `EngineStatus::spans_per_sec` and `logs_per_sec` come from two
  `RateMeter`s: the count over the last 5 seconds divided by 5, in 100 ms
  buckets. Time is passed in, never read, so tests need no sleeping. The
  first 5 seconds after start understate the rate.
- **Status.** `EngineStatus { paused, listeners, spans_per_sec, logs_per_sec,
  stats }` is a snapshot by value. Callers poll it (the UI) or print it (the
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
  `disk_cap_bytes` is stored but not enforced until the spill-to-disk store
  exists (Milestone 5).

## Headless mode

`loctel --headless` runs the engine with no window and no menu bar item. It is
what the end-to-end tests, the release smoke test and agents on Linux use.

```
loctel [--headless] [--grpc-port N] [--http-port N] [--mcp-port N]
       [--settings PATH] [--completeness-ms N]
```

- **Configuration.** Settings come from `--settings PATH`, else the per-user
  file; a missing file is fine, and a broken one is reported on stderr and
  replaced by the defaults for this run. The port flags win over the file.
  `--completeness-ms` sets how long a trace must be quiet to count as complete
  (tests use small values). Port 0 picks a free port. The file is only read:
  headless mode never writes it.
- **Output.** Stdout carries one JSON line per listener (`otlp-grpc`,
  `otlp-http`, `mcp`, in that order), once, when the engine is up, and nothing
  else:
  `{"event":"listener","name":"otlp-grpc","state":"listening","address":"127.0.0.1:4317"}`
  or `{"event":"listener","name":"otlp-http","state":"failed","error":"..."}`.
  A script that starts loctel with ports 0 reads its real ports from these
  lines. Logs and warnings go to stderr (quiet by default, `RUST_LOG` to
  change).
- **Lifetime.** The signal handlers are installed before the lines are
  printed, so a signal sent as soon as a line is read is never lost. SIGINT or
  SIGTERM shuts the engine down and exits 0. A taken port is not fatal: the
  process runs with what it could bind. An unknown flag exits 2, and an
  engine that cannot start exits 1.
- **Without `--headless`** on Linux the binary opens the status window (see
  "UI" below) and prints the same listener lines; with no display it says so
  on stderr and runs headless. On macOS it is the menu bar app (see "macOS
  shell" below), and prints the same lines. On any other OS it does the same as
  headless.
- **Tests.** `crates/loctel-app/tests/headless.rs` runs the built binary:
  the lines, SIGTERM and SIGINT, a taken port, settings file and flags.

## UI

`loctel-ui` is the thin GPUI layer: it depends on the engine (for
`EngineStatus` and `EngineHandle`) and on GPUI (`gpui-pre`, `gpui-pre-platform`,
`gpui-pre-wgpu`, all pinned `=0.3.9`), and the core depends on none of it. The
details, the theme tokens and the workflow are in
[`areas/ui.md`](areas/ui.md); the choices and their reasons are in
[`decisions/0008-ui-and-screenshot-choices.md`](decisions/0008-ui-and-screenshot-choices.md).

- **Milestone 1 has one view.** `StatusView` draws a `StatusModel`, a plain
  copy of what `EngineHandle::status()` returns (`StatusModel::from_status`),
  so it is tested by building the model by hand. The window refreshes it every
  second. The text of the status line and the rates is in `menu_text`, shared
  with the macOS menu so the two cannot drift.
- **Screenshot tests** (`crates/loctel-ui/tests/screenshots.rs`) render views
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
off macOS (its dependencies are macOS-only) apart from three plain-Rust modules
that hold decisions and are compiled for tests everywhere: the menu's rows, the
first-launch step and the icon files. It depends on the engine, `loctel-ui`
(for `menu_text`), `gpui-pre`, `gpui-tray` (the `NSStatusItem`), `objc2` and
`objc2-service-management`. The details, the menu and the list of what no one
has seen work yet are in [`areas/macos.md`](areas/macos.md); the choices and
their reasons are in
[`decisions/0009-macos-check-from-linux.md`](decisions/0009-macos-check-from-linux.md).

- **One entry point.** `loctel_macos::install(cx, engine, settings_path)`, called
  by `loctel-app` inside `Application::run` after it set
  `ActivationPolicy::Accessory` and `QuitMode::Explicit`. It opens no window.
  The main window is Milestone 3.
- **The menu** is the status line (the `menu_text` the Linux window uses),
  Copy MCP address, Pause or Resume, Start at login (checkmark from the
  system) and Quit, rebuilt on every click and when its state changes (checked
  every 2 s).
- **Start at login** is `SMAppService.mainAppService`, and the system is the
  source of truth. The first launch turns it on once (`first_launch_done`).
- **Checked from Linux.** `script/check-macos.sh` type-checks and lints the
  macOS code for `aarch64-apple-darwin`, with a rustc wrapper that stands in for
  the one build step that only runs on a Mac; `ci.yml` runs it in the Linux
  job. Nothing runs it: the macOS runner is only for releases.

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
engine, `loctel-meshgen` and rmcp's client, and has no code of its own beyond
the harness: a `Harness` starts an `Engine` (every port 0, a short completeness
time), sends meshgen scenarios to its real OTLP ports over gRPC, HTTP protobuf
and HTTP JSON, and reads the answer through an rmcp client over Streamable HTTP,
the way an agent does. Tests cover each transport, late-span assembly, the
service map, the marker flow, health checks and pause. See "End-to-end tests"
in [`areas/testing.md`](areas/testing.md) and
[`decisions/0007-e2e-harness-choices.md`](decisions/0007-e2e-harness-choices.md).

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
