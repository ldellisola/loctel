# Testing (`loctel-meshgen` and how tests use it)

How loctel is tested, and the fake microservice mesh every test, screenshot
and manual run draws its data from. Design references: §10.1 (the fake mesh),
§10.3 (the checks, which are the only quality gate) and §9.4 (what "failed"
means).

Status: the per-crate unit and integration tests, `loctel-meshgen`, the
end-to-end crate (`loctel-e2e`), the screenshot harness (`loctel-ui`, see
"Screenshot tests" below) and, since Milestone 5, the performance budgets
(`loctel-perf`, see "Performance budgets" below) exist.

The gate, which is the only quality gate:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The performance budgets are a second check, `cargo xtask perf` (the `perf` job of
`ci.yml`, beside `gate`): a release build and minutes of work, so not part of
`cargo test`. A pull request merges on both.

## The fake mesh: `loctel-meshgen`

A library and a binary (`meshgen`) that generate realistic OTLP traces and
logs for a small, made-up shop, and send them to any OTLP receiver. It depends
on `loctel-model` and the OTLP types, not on the engine, the store or the
ingest crate (those are dev-dependencies, used to check that the generated data
is valid).

The shop has five services, every one with the resource
`service.name`, `service.version = "1.4.0"` and
`deployment.environment = "local"`:

| Service | Role |
|---|---|
| `web-bff` | entry point (the browser calls it) |
| `orders-api` | orders; talks to postgresql, `inventory-api`, `payments-api`, rabbitmq |
| `inventory-api` | stock; talks to redis |
| `payments-api` | charges; talks to postgresql and the external `card-gateway` |
| `notifications-worker` | consumes rabbitmq messages |

`SERVICES` lists all five, and `APP_SERVICES` the first four (the ones that
serve HTTP requests).

### Library

```rust
let g = loctel_meshgen::generate(&Scenario::CheckoutFailure, seed, base_unix_nano);
// g.batches:   Vec<Batch { traces: Option<ExportTraceServiceRequest>,
//                          logs: Option<ExportLogsServiceRequest>,
//                          delay_before: Duration }>
// g.trace_ids: every trace in the scenario, in the order they were made

let report = loctel_meshgen::send(&g, &Transport::HttpJson("http://127.0.0.1:4318".into()), 1.0).await?;
// SendReport { spans, logs }
```

- **Deterministic.** The same scenario, seed and base time give identical
  bytes. IDs come from a ChaCha8 RNG seeded with `seed`; every timestamp is
  `base_unix_nano` plus a fixed offset. Tests pass a fixed base time, the
  command line passes "now".
- **Scenarios never share IDs.** Each scenario reads its own ChaCha stream, so
  `CheckoutFailure` and `CheckoutSuccess` given the same seed still make
  different traces. (Without this, the store's duplicate check would swallow
  the second one: see `docs/decisions/0005-meshgen-scenario-choices.md`.) The
  same scenario with the same seed is the same trace, so to put two of them in
  one loctel, change the seed.
- **`send`** takes a `Transport` (`Grpc`, `HttpProtobuf` or `HttpJson`, each
  with the endpoint's base URL; `http://` is added when missing) and a
  `time_scale`. Before each batch it sleeps `delay_before * time_scale`
  (`0.0` sends everything at once, `1.0` keeps the scenario's pauses). HTTP
  bodies of 64 KiB or more are gzipped. It fails when a request fails **and
  when the receiver rejects items** (`partial_success`), because generated data
  is always valid, so a rejection means a bug in the receiver or the
  generator. `SendReport` counts what was sent.
- **Batches.** The big scenarios are cut into batches (5,000 spans for the
  huge trace, 10,000 records for the flood, 1,000 probes for health checks) the
  way real exporters flush, so a request stays far below the receivers'
  64 MiB limit.

### Command line

```
meshgen --scenario <checkout-failure|checkout-success|health-checks|late-out-of-order|huge-trace|log-flood>
        --transport <grpc|http-protobuf|http-json> --endpoint URL
        [--repeat N] [--interval-ms N] [--seed N] [--size N]
```

- `--endpoint` is the base URL: `http://127.0.0.1:4317` for gRPC,
  `http://127.0.0.1:4318` for HTTP (loctel's defaults).
- `--repeat N` sends the scenario N times, `--interval-ms` apart (default
  1000). Every repetition is new data: seed plus the repetition number, and
  timestamps from the moment it is sent.
- `--seed` defaults to 1. `--size` scales the scenarios that have a size:
  health checks per service (default 20), spans in the huge trace (default
  100000), records in the log flood (default 10000). The checkout scenarios
  ignore it.
- It prints one line per repetition, for example
  `sent 15 spans and 7 logs (trace 5b7b6135be198533f7c7ec46651216b7)`, and
  exits 1 with `meshgen: <reason>` when it cannot deliver.

Try it by hand (Linux, no window):

```bash
cargo run -p loctel-app -- --headless --completeness-ms 500 --settings /tmp/loctel-settings.toml --rules /tmp/loctel-rules.toml &
cargo run -p loctel-meshgen -- --scenario checkout-failure --transport http-json --endpoint http://127.0.0.1:4318
# sent 15 spans and 7 logs (trace ...)
kill %1
```

## Scenarios

### `CheckoutFailure` (frozen)

**This scenario is fixed content.** The mockups, the screenshot tests and the
end-to-end expectations all rely on every row below. Any change to it (a name,
an offset, a duration, an attribute, a log line) requires updating the
reference screenshots and the e2e expectations in the same change, and the
table in `crates/loctel-meshgen/tests/scenarios.rs`, which pins it field by
field. The TraceQL evaluator's tests
(`crates/loctel-query/tests/traceql_eval.rs`) name its spans by row number and
work out each expected answer from this table, and the LogQL tests
(`crates/loctel-query/tests/logql.rs`) count its 7 logs by service, level and
body, so they change with it. So do the discard rules' tests
(`crates/loctel-rules/tests/filter.rs` and `preview.rs`), which name spans of it
(`MGET`, `POST /charges`, `GET /stock/{sku}`), count its 15 spans and 7 logs
(3 at error level) and use `HealthChecks` and `LateOutOfOrder` beside it. It is
one trace and one batch, with no delay.

Offsets are milliseconds from `base_unix_nano`.

| # | parent | service | kind | name | start | dur | status | key attributes |
|---|---|---|---|---|---|---|---|---|
| 1 | n/a | web-bff | Server | POST /api/checkout | 0 | 1840 | Error | http.route=/api/checkout, http.response.status_code=500 |
| 2 | 1 | web-bff | Internal | validate_token | 4 | 18 | Unset | |
| 3 | 1 | web-bff | Client | POST | 30 | 1780 | Error | server.address=orders-api |
| 4 | 3 | orders-api | Server | POST /orders | 34 | 1772 | Error | http.response.status_code=500 |
| 5 | 4 | orders-api | Client | SELECT carts | 40 | 18 | Unset | db.system.name=postgresql |
| 6 | 4 | orders-api | Client | GET | 62 | 128 | Unset | server.address=inventory-api |
| 7 | 6 | inventory-api | Server | GET /stock/{sku} | 66 | 120 | Unset | |
| 8 | 7 | inventory-api | Client | MGET | 70 | 4 | Unset | db.system.name=redis |
| 9 | 4 | orders-api | Client | POST | 196 | 1594 | Error | server.address=payments-api |
| 10 | 9 | payments-api | Server | POST /charges | 200 | 1586 | Error | order.id=8812, exception event at 1782 |
| 11 | 10 | payments-api | Client | SELECT payment_methods | 206 | 24 | Unset | db.system.name=postgresql |
| 12 | 10 | payments-api | Client | POST | 236 | 600 | Error | server.address=card-gateway, error.type=timeout |
| 13 | 10 | payments-api | Client | POST | 840 | 600 | Error | server.address=card-gateway, http.request.resend_count=1 |
| 14 | 4 | orders-api | Producer | order.failed publish | 1794 | 6 | Unset | messaging.system=rabbitmq |
| 15 | 14 | notifications-worker | Consumer | order.failed process | 1802 | 14 | Unset | messaging.system=rabbitmq |

Only those attributes exist. Numbers are integers (`http.response.status_code`,
`http.request.resend_count`, `order.id`), everything else is a string. Erroring
spans carry a status message: `HTTP 500` on 1, 3, 4 and 9; the exception message
on 10; `timeout` on 12 and 13.

The exception event on span 10, at +1782 ms, has `exception.type =
Payments.Api.Gateway.CardGatewayTimeoutException`, `exception.message =
card-gateway did not respond after 2 attempts` and an `exception.stacktrace` of
three `at ...` lines, taken from the Trace mockup (the constants are public in
`loctel_meshgen::checkout`).

Logs, each linked to its span (span, level, time offset):

| offset | level | span | body |
|---|---|---|---|
| 10 | INF | 4 | Created order 8812 for cart c-5521 |
| 190 | INF | 4 | Requesting payment for order 8812 |
| 210 | INF | 10 | Charging card for order 8812, amount 129.00 EUR |
| 836 | WRN | 10 | card-gateway did not answer within 600 ms, retrying (attempt 2 of 2) |
| 1440 | ERR | 10 | card-gateway did not answer within 600 ms |
| 1782 | ERR | 10 | Charge failed for order 8812: CardGatewayTimeoutException: card-gateway did not respond after 2 attempts |
| 1800 | ERR | 4 | Checkout failed for order 8812: payments-api returned 500 |

The ERR at 1782 also carries the three `exception.*` attributes. Severity
numbers are 9 (INF), 13 (WRN) and 17 (ERR).

What loctel must say about it (design §9.4), checked by
`checkout_failure_fails_in_the_card_gateway_call` through the real converter
and store:

- the trace is **Failed** (the root is erroring), with 15 spans and 5 services
- failed in span #12 (`payments-api` · `POST`, peer `card-gateway`), with
  `more == 1` (span #13 is the other lowest error; #10 has erroring
  descendants, so it is not a candidate)
- the trace's exception is #10's, found as the nearest ancestor of the
  failed-in span that has one

### `CheckoutSuccess`

The same tree with every status unset: 14 spans (no span 13), no
`error.type`, no exception events, and the messaging spans named
`order.created publish` and `order.created process`. The timings are the
failing trace's, so it is a slow success (1.84 s). A successful request
answers `http.response.status_code=200` on spans 1 and 4. Six INF logs, no
exception attributes: the first three of the table above, then
`Charge accepted for order 8812` (840, span 10),
`Payment completed for order 8812` (1788, span 4) and
`Checkout completed for order 8812` (1800, span 4).

### `LateOutOfOrder`

A `CheckoutSuccess` trace sent in three batches, to exercise assembly with
children before parents (design §4). Depth counts the root as 0:

1. spans at depth 3 and deeper, no delay
2. the middle spans (depth 1 and 2: `validate_token`, `POST`, `POST /orders`),
   after 50 ms
3. the root (#1), after another 50 ms

A log travels in the batch of its span, so the last batch has no logs.

### `HealthChecks { count }`

`count` probes for each of the five services: a `GET /health` Server span
(`http.request.method=GET`, `url.path=/health`,
`http.response.status_code=200`, 1 to 4 ms) and an INF log
`GET /health 200` linked to it. Every probe is a root and a trace of its own,
so `5 * count` traces, one per second per service. This is the noise the
discard rules of Milestone 2 silence.

### `HugeTrace { spans }`

One trace of exactly `spans` spans for the waterfall and the store's limits.
A complete k-ary tree in breadth-first order (span `i`'s parent is
`(i - 1) / k`), where k starts at 10 and grows only as far as needed to stay
within 8 levels, so 100,000 spans take 6 levels. The services rotate among the
four app services by span index, the root is a `POST /api/bulk` server span, the
rest are internal `work item N` spans, and a parent always encloses its
children in time. Nothing errors. Zero spans give no batches and no traces.

### `LogFlood { records }`

`records` logs with no trace context, 40% DBG and 60% INF, spread at random
over the five services, 20 microseconds apart (the design's 50,000 per second
target). Times are distinct, so the store's duplicate check keeps them all.

### Bulk data: `bulk_logs` and `bulk_traces`

Not scenarios (nothing is fixed content, and the command line does not send
them): iterators that make ten million logs or spans a batch at a time, without
holding them, for the performance budgets and the Logs list at ten million lines
(Milestone 5, `crates/loctel-meshgen/src/bulk.rs`, decision 0049).

```rust
for batch in bulk_logs(10_000_000, seed) { /* Batch { logs: Some(request), .. } */ }
let spans = bulk_traces(10_000_000, seed).starting_at(now).in_batches_of(5_000);
```

- Batches of 1,000 (`BULK_BATCH`); `in_batches_of(n)` and `starting_at(unix_nano)` change
  that and the time of the first item (`BULK_START_UNIX_NANO`, 2027-01-15T08:00:00Z,
  by default). Deterministic for a seed, on streams of their own (so they never share
  IDs with a scenario given the same seed).
- **Logs** are 20 microseconds apart (50,000 a second), over the five services: 35%
  DBG, 50% INF, 10% WRN, 5% ERR. A quarter carry a trace and a span ID, sixteen
  consecutive logs sharing a trace. About 1 in 100 of `payments-api`'s says
  `upstream timeout calling card-gateway after N ms (attempt K)` (a WRN or an ERR);
  nothing else says `timeout`. Bodies differ, so the store's duplicate check keeps all of
  them. A request groups its logs by service, so the order inside a batch is the
  services', not the clock's.
- **Traces** are ten spans over four services (`GET /api/items` through `web-bff`,
  `orders-api`, `inventory-api` and `payments-api`, with a database or cache call
  beside), a trace starting every 200 microseconds (50,000 spans a second), the whole
  trace stretched by 1 to 2 so that durations differ, every span inside its parent. About 1
  in 100 traces fails: the spans from the failed leaf (`SELECT prices`) up to the root are
  in error (and answer 500), so the trace is `Failed`. A trace is never split across
  batches while the batch size is a multiple of ten; the last trace is cut short when the
  span count is not.

## How tests use it

- **`crates/loctel-meshgen/tests/scenarios.rs`** pins the content: the
  `CheckoutFailure` table row by row, the other scenarios against their
  description, determinism, valid IDs through the real converter, the store's
  verdict on the checkout traces, and that scenarios never share IDs.
- **`crates/loctel-store/tests/`** runs every test that builds a store twice,
  through `each_mode` (`tests/modes.rs`): once on a store in memory, once on one
  that spills to segment files (`StoreConfig::spilling_for_tests`), because the
  store's API is frozen across Milestone 5 and must answer the same either way.
  `tests/spill.rs` looks at the spill itself with a store of each kind fed the same
  data (Milestone 5), and `tests/logs_scale.rs` at the log chunks (the spilling config
  seals one every 7 logs, so every log test meets sealed chunks). See `store.md`,
  "Tests". A new store test file builds its stores
  with `each_mode`, unless it is about spilling and needs to see the files.
- **`crates/loctel-meshgen/tests/send.rs`** sends every scenario over every
  transport to the real receivers from `loctel-ingest`: counts, delays,
  gzip, rejections and unreachable endpoints.
- **`crates/loctel-mcp/tests/tools.rs`** loads `CheckoutFailure` (and the
  success, health-check and huge scenarios) into a store through the real
  converter, with a `ManualClock` moved past the quiet period, and calls the
  MCP tools directly. Because the checkout scenario is frozen, it asserts the
  failed-in span, the exception, the log counts and the span counts it implies.
  Its `query` tests (a structural TraceQL query, LogQL, the error texts) run on
  the same data. `tests/budget.rs` there checks that every tool's default answer
  fits `ANSWER_TEXT_BUDGET` (24,000 characters) on `CheckoutFailure`,
  `HealthChecks{50}` and `HugeTrace{10000}`. `tests/http.rs` there and the MCP
  tests in `loctel-engine` and `loctel-app` talk to the real server with an rmcp
  client or raw HTTP; see [`mcp.md`](mcp.md).
- **End-to-end tests** (`crates/loctel-e2e`) start the engine, drive these
  scenarios through real OTLP and assert through a real MCP client; see
  "End-to-end tests" below.
- **Screenshot tests** (Task 11 and later) render views from fixed meshgen
  data (a fixed seed and base time), which is why `CheckoutFailure` is frozen.
- **`crates/loctel-meshgen/tests/bulk.rs`** pins the bulk generators: the batches, the
  spacing, determinism per seed, the mix the budgets' queries rely on (levels, the thin
  slice of `payments-api` timeouts, a quarter of logs with trace context, 1 in 100 traces
  failed, each trace one tree with parents holding their children) and that the real
  converter and the store take them.
- **The performance budgets** (Milestone 5) use `HugeTrace` for the trace they open and
  the bulk generators for everything else: see "Performance budgets" below.
- **`crates/loctel-ui/tests/logs_scale.rs`** (Milestone 5 Task 7) puts the bulk logs in a
  store that spills and drives the Logs screen over it: 25,000 lines in chunks of 4,096
  for the tests of the windowed list (more than the 20,000 a paged list holds), and
  **ten million** for `ten_million_scroll_to_middle_dark`, which builds them with a
  thread that makes and converts the batches while the store takes the last one. The
  big one takes about 30 s in a debug build (about 22 s of it making the data); it fits
  because the dev profile builds `loctel-store` and `zstd-sys` with `opt-level = 3`
  (workspace `Cargo.toml`, decision 0051): without it the store alone needs 47 s. The
  plan allows 60 s; if the test gets near that, fix the build of the data, not the
  size of the test.

Add a scenario by adding a variant to `Scenario`, a module for it, a stream
constant in `scenario.rs` (so it never shares IDs), a CLI value in `main.rs`,
tests, and a section above.

## Screenshot tests: `loctel-ui`

The views are tested by pixels: each test renders a view with a fixed model
through GPUI's headless renderer (software wgpu: no GPU, no display) and
compares it, pixel for pixel, with a reference PNG in
`crates/loctel-ui/tests/snapshots/`. They run with `cargo test --workspace`
(the `screenshot` feature is on in every test build of that crate), and alone:

```bash
cargo test -p loctel-ui           # menu text, theme, model and screenshots (about a second for the pixels)
cargo xtask screenshots --update  # rewrite the references; then view every changed PNG
cargo xtask run-shot              # the real window under Xvfb, to target/run-shot.png
```

A failure writes `target/screenshot-diffs/{name}.actual.png` and `.diff.png`
(CI uploads them). Rules: a reference changes only with an intended change to
the view, in the same commit, and the agent looks at the new PNG before
committing it. Everything else (the harness API, the 2x scale, the bundled
font that makes it machine-independent, the CI diff workflow) is in
[`ui.md`](ui.md) and the repo skill `update-screenshots`; the choices are in
[`decisions/0008-ui-and-screenshot-choices.md`](../decisions/0008-ui-and-screenshot-choices.md).
Screenshot tests can also be fed meshgen scenarios: `generate` is
deterministic, so the same seed draws the same pixels.

## End-to-end tests: `loctel-e2e`

`crates/loctel-e2e` (`publish = false`, tests only) proves the whole path an
agent depends on: **real OTLP in, real MCP out**, with nothing mocked between.
Design reference: §10.3 (the end-to-end bullet of the checks). The crate is a
small harness (`src/lib.rs`) and one test file per concern (`tests/`).

```bash
cargo test -p loctel-e2e   # a few seconds of test time; the suite must stay under 60 s
```

### The harness

```rust
let harness = Harness::start(Duration::from_millis(200));
let marker = harness.marker().await;
let sent = harness.send(&Scenario::CheckoutFailure, TransportKind::Grpc, 1, 0.0).await;
harness.wait_complete(&sent.trace_ids[0]).await;
let failures = harness.tool("recent_failures", json!({ "since": marker })).await;
```

| | |
|---|---|
| `Harness::start(completeness)` | an engine with every port 0 (so tests never collide with each other or with a running loctel) and the store's `completeness_quiet` set to `completeness`. It has no discard rules (`rules_path` is `None`). Callable from `#[tokio::test]`: the engine refuses to start inside a runtime, so it is started on a thread of its own. Dropping the harness shuts the engine down |
| `Harness::start_with_rules(completeness, rules_path)` | the same, with the discard rules in the file at `rules_path`, as `loctel --rules PATH` runs them: the engine loads the file if the test wrote one first (`RulesFile::EXAMPLE_RULE` is a ready-made file holding the health-check rule), writes one holding no rules if there is none, and reloads it within about 2 s of an edit. Give each test a `tempfile::tempdir()` of its own |
| `otlp(kind) -> Transport` | where `TransportKind::{Grpc, HttpProtobuf, HttpJson}` is received, for `loctel_meshgen::send` (`TransportKind::ALL` lists the three) |
| `send(&Scenario, kind, seed, time_scale) -> Generated` | generates the scenario dated now and sends it; panics if delivery fails or loctel rejects an item. `Generated::trace_ids` identify what was sent |
| `mcp() -> McpClient` | an rmcp client over Streamable HTTP (connects on first use, clones share the connection); `McpClient::call` returns a `ToolOutcome { is_error, structured, text }` for tests of tool errors |
| `tool(name, args) -> Value` | calls a tool through the real transport and returns its `structuredContent`; panics on a tool error, and checks that the text copy is the same JSON |
| `marker() -> String` | `get_marker`'s `marker`, the `since` of the agent flow |
| `wait_complete(trace_id)` | polls `get_trace` until `complete`, 10 s at most, and panics with the last answer otherwise |
| `engine() -> &EngineHandle` | the engine itself: pause it, read its status or its store |

The harness asserts nothing about what loctel says. It runs the same
`Engine::start` the `loctel` binary runs, in the test process, which is faster
and lets a test pause it; the binary's own start, stop and output lines are
tested in `crates/loctel-app/tests/headless.rs`, and the Linux window's (it opens
under `xvfb-run`, prints the startup lines and stops with status 0 on SIGTERM; it
prints `skipped: no xvfb-run` and passes where that is missing) in
`crates/loctel-app/tests/window.rs`. See
[`decisions/0007-e2e-harness-choices.md`](../decisions/0007-e2e-harness-choices.md)
(and [`0020`](../decisions/0020-e2e-rules-and-query-tests.md) for the tests of the
rules and the queries).

### The tests

| File | Test | What it proves |
|---|---|---|
| `tests/transports.rs` | `checkout_failure_over_each_transport` | over gRPC, HTTP protobuf and HTTP JSON in turn, `recent_failures(since)` names failed-in `payments-api`, peer `card-gateway`, the exception type, and the trace's logs arrived with it |
| `tests/assembly.rs` | `late_spans_assemble_into_one_trace` | `LateOutOfOrder` with its real pauses is one complete trace of 14 spans, the root (sent last) present, no orphans |
| `tests/map.rs` | `service_map_matches_mockup_topology` | the nine nodes with their kinds, `orders-api` to `payments-api` red, and the message goes through `rabbitmq`, never straight to the worker |
| `tests/agent_flow.rs` | `agent_flow_since_marker` | marker, then a success and a failure: `recent_failures(since)` is exactly 1 failed and 0 errors-inside, `search_traces(since)` is 2, and the earlier failure is left out |
| `tests/agent_flow.rs` | `health_checks_are_visible` | with no discard rules (the default, decision 0016), health checks are stored and listed among the real traces |
| `tests/pause.rs` | `paused_engine_stores_nothing` | paused over every transport: success to the exporter, nothing held, nothing found; after resuming only the new trace is there |
| `tests/rules.rs` | `health_rule_silences_health_checks_over_otlp` | `rules.toml` holding `EXAMPLE_RULE`, `HealthChecks{5}` over gRPC and a failing checkout over HTTP JSON: `search_traces` has the checkout and no `GET /health`, `search_logs` its 7 logs and no `GET /health 200`, and `get_status` has one rule with `dropped_traces` 25 (5 probes of each of 5 services), 25 spans and 25 logs dropped, and `dropped_fraction` 50/72 |
| `tests/rules.rs` | `late_spans_of_dropped_trace_never_appear` | a rule on the root's name and `LateOutOfOrder`, whose root arrives last: the 13 spans stored before it go with it (1 trace, 14 spans, 6 logs dropped). Replayed with the root first and the rest straggling in one request at a time, nothing is stored after any batch, and the trace is still counted once |
| `tests/rules.rs` | `disabling_rule_in_file_restores_flow` | with the example rule on, health checks are dropped; the test edits `enabled = false` in the file, and within a few seconds `get_status` shows the rule off with no restart, new health checks are stored, and the rule keeps its count |
| `tests/query.rs` | `traceql_structural_query_over_mcp` | the Grafana-style `{ resource.service.name = "web-bff" } >> { name = "POST /charges" }` finds both checkouts and `matched_span_ids` is the `POST /charges` span (checked with `get_span`); the reversed query finds nothing, without an error; `>> { status = error }` finds only the failing one with 6 matched spans; `count()` pipelines and the other filters combine with `query` |
| `tests/query.rs` | `logql_query_over_mcp` | selectors, `level` labels, regex label matchers (anchored), a substring line filter, a regex line filter (unanchored) and `service` beside `query` give the counts and messages the frozen `CheckoutFailure` table implies |
| `tests/query.rs` | `invalid_query_is_tool_error` | a query that does not parse is `isError: true` with no structured content and the text `invalid TraceQL: at column N: ...` (or `invalid LogQL`), including `select()`, `line_format` and metric queries "not supported yet"; the session answers the next call |
| `tests/release_fixture.rs` | `one_span_fixture_is_accepted_and_stored` | `fixtures/one-span.json`, the request the release job posts to the built app, gets a success over HTTP JSON with no rejected span, and the trace (one span of `release-smoke`) is complete and visible through `get_trace`. The job's `curl -f` cannot see a `partial_success` rejection, so this test guards the file (see [`release.md`](release.md)) |

### Writing one

- **One harness per test**, started with `Duration::from_millis(200)` (or
  less): a trace is complete 200 ms after its last span, so a test waits for
  that, not for 10 s. Tests in a file run in parallel on their own engines.
- **Give every send its own seed.** The same scenario with the same seed is
  the same trace, which the store keeps once (see "Scenarios never share
  IDs" above); different scenarios can share a seed. (The one exception is
  deliberate: a test of the discard rules sends a dropped trace again to see
  that its late spans are still dropped. Say so in the test.)
- **A test of the rules writes its own `rules.toml`** in a `tempfile::tempdir()`
  and starts with `Harness::start_with_rules`. A plain `Harness::start` has no
  rules, because loctel has no default ones: a test that expects a fresh engine
  to drop health checks is a bug in the test. To send one batch of a scenario
  at a time (late spans), build a `Generated` of that one batch and give it to
  `loctel_meshgen::send` with `harness.otlp(kind)`; `tests/rules.rs` does.
- **To ask "what happened since", take `marker()` first**, then send, then
  pass the marker as `since`. `send` dates the scenario at the real time, so
  its traces start after the marker. (`since` filters on when a trace
  started, so a marker taken after a trace's `send` does not include it.)
- **Wait for completeness before asserting on a whole trace** (`wait_complete`):
  the service map shows an external host only once its trace is complete, and
  `complete` is part of the trace summary. Failed-in and the counts do not
  need it.
- **Assert on the frozen `CheckoutFailure` literally** (service names, the
  exception type), not through meshgen's constants, so a change to the
  scenario fails here as it should.
- **If a test fails because of a bug in another crate**, fix it there and add
  the regression test in that crate; the end-to-end test then stays as the
  proof it works through the whole path.


## Performance budgets: `loctel-perf`

Design §10.3 turns the throughput and latency targets into a check that fails CI. It
is `crates/loctel-perf` (`publish = false`, binary `loctel-perf`), run by `cargo xtask
perf [--only ID] [--json PATH]`, which builds `loctel-perf` and `loctel` in release (one
`cargo build --release -p loctel-perf -p loctel-app`), runs it from the workspace root and
exits non-zero when a budget is missed. The `perf` job of `ci.yml` runs `cargo xtask perf
--json perf.json` on `ubuntu-24.04` beside `gate` and uploads `perf.json` as the `perf`
artifact even when it fails. The way to run one budget, read the table, profile a breach
and the rule about changing a budget is the `perf-budgets` skill
(`.claude/skills/perf-budgets/SKILL.md`); the reasons for the scenarios are decision
[`0049`](../decisions/0049-performance-harness-choices.md).

**A budget is never raised to get green.** Raising one takes a decision file with the
measurements, and is acceptable only for `first_frame`, which software rendering (lavapipe)
cannot hold to the Mac's 500 ms.

| id | measure | budget | measured here |
|---|---|---|---|
| `ingest` | items accepted a second: OTLP HTTP protobuf, batches of 1,000 spans and 1,000 logs, one discard rule on, spill on, 20 s | >= 50,000 items/s | 389,000 items/s |
| `ingest` | items sent and not accepted (rejected, failed, cut by a rule) | 0 | 0 |
| `ingest` | p99 latency of a batch | < 250 ms | 95 ms |
| `trace_open_hot` | `store.trace(id)` and `Waterfall::new`, a 100,000-span trace in memory, median of 5 | < 250 ms | 137 ms |
| `trace_open_cold` | the same, the trace in a segment file (five traces, each read once), median of 5 | < 250 ms | 201 ms |
| `logs_count` | `log_count` over 10M logs, levels and service, median of 5 | < 50 ms | 0.005 ms |
| `logs_at` | `logs_at(offset, 50)` at 100 random offsets over 10M logs, p95 | < 20 ms | 4.9 ms |
| `histogram` | 48 buckets over 10M logs, levels and service, a window of whole seconds, median of 5 | < 100 ms | 0.015 ms |
| `traceql_first_page` | `{ status = error }` over 10M spans (1% of traces fail), limit 20, median of 5 | < 1 s | 110 ms |
| `logql_first_page` | `{service_name="payments-api"} \|= "timeout"` over 10M logs, limit 50, median of 5 | < 1 s | 3.1 ms |
| `startup_headless` | process start to the last listener line, release `loctel --headless`, median of 5 | < 300 ms | 5.4 ms |
| `first_frame` | main window construction and the first headless render (lavapipe), release, a process each, median of 5 | < 1.5 s (Mac target 500 ms, by hand) | 504 ms |
| `rss_10m` | resident memory after 10M logs and 1M spans, spill on | < 1.5 GB | 920 MB |

"Measured here" is one run on the development container (4 vCPU, release), for
orientation, **not a gate**. Timings are the median of 5 runs except `logs_at`'s p95,
`ingest`'s p99 and the rate; `rss_10m` is read once and is "not measured" off Linux.
A whole run takes about two minutes after the release build (eight minutes the first time).

How each is measured:

- **The data** is built in the process through the store, with `loctel-meshgen`'s bulk
  generators and the real OTLP converter, on a `ManualClock` that follows the data (so
  traces complete and spill, and logs seal, as in a long run); the limits of the store
  are raised past what a budget loads (`max_traces` 20M, `max_log_records` 100M, 64 GiB
  of disk cap) and the run fails if the store does not hold everything it was given.
  The log budgets share one store (1M spans, then 10M logs) and `rss_10m` is read first,
  before a query has filled the caches; `traceql_first_page` has a store of 10M spans of
  its own, settled so that they are in segment files. `ingest` is the exception: a real
  engine with its run directory and one rule, fed over HTTP by two senders in a closed
  loop for 20 s.
- **`startup_headless`** starts the release `loctel` five times on scratch files and a
  scratch `TMPDIR`, and times to the third listener line. **`first_frame`** runs
  `loctel-perf --first-frame-once` five times, a process each.
- **Tiny scale in the gate.** `cargo test -p loctel-perf` (part of `cargo test
  --workspace`) runs every scenario on `Scale::SMOKE` in a debug build in seven seconds,
  so that a scenario that stops working fails the gate, and unit-tests the table
  (`budget_table_flags_breaches`: a measurement over its budget is a `BREACH` row and exit
  code 1) and the startup reader against a fake `loctel`.

```bash
cargo xtask perf                       # the table, exit 1 on a breach
cargo xtask perf --only logs_at        # one id (`ingest` is three rows)
cargo run --release -p loctel-perf -- --smoke   # tiny scale, numbers mean nothing
```

Exit codes of `loctel-perf`: 0 every budget held, 1 a budget was missed, 2 a scenario
could not run.

## Release tooling: `xtask`

```bash
cargo test -p xtask   # about a second once built
```

The unit tests of the pieces the release job runs, all on Linux (the macOS
parts of the pipeline cannot be tested here; see [`release.md`](release.md)):

- `version.rs`: no tags gives `v0.1.0`; the highest `vX.Y.Z` has its patch
  bumped, compared as numbers (`[v0.1.4, v0.1.10, v0.0.9]` gives `v0.1.11`);
  tags that are not versions are ignored.
- `notes.rs`: the three sections in order (Changes, Check on your Mac,
  Installing), the checklist carried over unchanged, the Open Anyway and
  `xattr -dr com.apple.quarantine` steps present.
- `main.rs`: the command line (`perf` takes `--only ID` and `--json PATH`, each once, in
  either order), and the git glue against scratch repositories (the commits since the
  highest version tag, the tags of a repository).
- `smoke.rs`: `mcp-smoke` passes against a real engine (the same one
  `loctel --headless` runs) and fails when nothing listens or the tool is
  missing.
