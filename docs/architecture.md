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

Status says what is in the repository now. Rows marked "planned" are the
Milestone 1 target from `superpowers/plans/2026-10-21-loctel-m1-core.md`;
the task that creates a crate changes its row.

| Crate | Layer | Owns | Status |
|---|---|---|---|
| `loctel-model` | core | Domain types (spans, logs, resources, IDs, severity), independent of OTLP | implemented (see "Domain model" below) |
| `loctel-store` | core | In-memory store: trace assembly, completeness, dedup, eviction, failure semantics, search, logs, service map | implemented: assembly, completeness, dedup, eviction, failure semantics, trace summaries and search, logs, service map and services list (see "Store" below) |
| `loctel-ingest` | core | OTLP gRPC and HTTP receivers, protobuf and JSON to model conversion | implemented: gRPC and HTTP receivers, gzip, conversion, `partial_success` (see "Ingest" below) |
| `loctel-mcp` | core | MCP server (rmcp, Streamable HTTP), tools and their DTOs | planned |
| `loctel-engine` | core | Wires store, ingest and MCP; settings, pause, rates; owns threads and runtime | implemented: store and OTLP receivers, `settings.toml`, pause, rates (MCP is wired in by a later task; see "Engine" below) |
| `loctel-meshgen` | test tooling | Fake microservice mesh scenarios (library and `meshgen` binary) | planned |
| `loctel-e2e` | test tooling | End-to-end tests through real OTLP and a real MCP client (`publish = false`) | planned |
| `loctel-ui` | UI | GPUI views and the headless screenshot harness | planned |
| `loctel-macos` | macOS | Status item, activation policy, login item (`cfg(target_os = "macos")`) | planned |
| `loctel-app` | binary | The `loctel` binary: headless mode and GUI, bundle metadata | implemented: headless mode only (see "Headless mode" below); the GUI comes with the UI tasks |
| `xtask` | tooling | Screenshots, next-version, release notes, MCP smoke test | placeholder (empty `main`) |

## Data flow

Target shape for Milestone 1 (MCP and the UI are not wired in yet; the rest is
what runs today):

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

Readers (MCP, UI) get the same `EngineHandle` and ask the store or the status
directly; the engine is not in the read path.

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

`loctel-store` depends on `loctel-model`, `parking_lot` and `serde`. The
details (API, rules and the reasons) are in
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
  errors and duration, with an opaque keyset cursor (`Page`, `PageRequest`).
- **Logs.** `Store::ingest_logs` keeps log records in time order (observed
  time when the timestamp is unset), drops exporter retries by
  `LogRecord::dedup_key()` and evicts the oldest past `max_log_records`.
  `Store::search_logs` filters by time, service, minimum severity, body text
  (case-insensitive), trace and span, newest first with the same keyset
  paging. `Store::logs_for_span` returns a span's logs, oldest first, linked
  by ID only, so it works whether or not the span is held.
- **Service map and services.** `Store::service_map(TimeWindow)` and
  `Store::services(TimeWindow)` are computed on demand from the traces held:
  service nodes with entry-span counts, service-to-service edges, and
  database, queue and external nodes inferred from span attributes
  (design §9.2). External hosts appear only once their trace is complete.
  Node ids are unique within a kind, not across kinds.

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

## Engine

`loctel-engine` depends on `loctel-model`, `loctel-store`, `loctel-ingest`,
`tokio`, `toml`, `serde`, `dirs` and `anyhow`. It is the one object the menu
bar, the window and the headless binary hold, so none of them knows how the
store and the receivers are put together. Decisions and reasons:
[`decisions/0004-engine-lifecycle-and-settings.md`](decisions/0004-engine-lifecycle-and-settings.md).

- **Starting.** `Engine::start(EngineConfig { bind_ip, settings, store })`
  returns an `EngineHandle`. It builds the `Store` (with `SystemClock`), a
  multi-thread tokio runtime on threads named `loctel-engine`, and the OTLP
  receivers on `bind_ip` and the ports in `Settings` (loopback by default).
  The caller needs no async `main`. Calling it from inside a tokio runtime is
  an `Err`, not a panic.
- **Ports.** A port that cannot be bound is a `ListenerState::Failed` in
  `EngineStatus::listeners` and does not stop the other receivers (design §4).
  Port 0 picks a free one, and the listener status carries the real address.
- **The handle.** `EngineHandle` is `Clone`, and every clone is the same
  engine: `store()`, `status()`, `set_paused`, `is_paused`, `mcp_url()` (`None`
  until MCP is wired in) and `shutdown()`. `shutdown` is idempotent and safe
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
  headless binary).
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
- **Output.** Stdout carries one JSON line per listener, once, when the
  engine is up, and nothing else:
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
- **Without `--headless`** the binary does the same for now. The menu bar app
  and the window arrive with the UI tasks (Linux window, then the macOS status
  item), and those branches will live in `main`.
- **Tests.** `crates/loctel-app/tests/headless.rs` runs the built binary:
  the lines, SIGTERM and SIGINT, a taken port, settings file and flags.

## Platform

macOS 26 or later on Apple Silicon is the product. Linux x86_64 exists for
development and tests only. Mac-only features are unverified until a milestone
check on a real Mac (design §10.4).
