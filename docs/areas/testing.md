# Testing (`loctel-meshgen` and how tests use it)

How loctel is tested, and the fake microservice mesh every test, screenshot
and manual run draws its data from. Design references: §10.1 (the fake mesh),
§10.3 (the checks, which are the only quality gate) and §9.4 (what "failed"
means).

Status: the per-crate unit and integration tests and `loctel-meshgen` exist.
The end-to-end crate (`loctel-e2e`), the screenshot harness and the
performance tests are planned (see `architecture.md`); they will add their own
sections here and use the scenarios below.

The gate, which is the only quality gate:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

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
cargo run -p loctel-app -- --headless --completeness-ms 500 --settings /tmp/loctel-settings.toml &
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
field. It is one trace and one batch, with no delay.

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

## How tests use it

- **`crates/loctel-meshgen/tests/scenarios.rs`** pins the content: the
  `CheckoutFailure` table row by row, the other scenarios against their
  description, determinism, valid IDs through the real converter, the store's
  verdict on the checkout traces, and that scenarios never share IDs.
- **`crates/loctel-meshgen/tests/send.rs`** sends every scenario over every
  transport to the real receivers from `loctel-ingest`: counts, delays,
  gzip, rejections and unreachable endpoints.
- **End-to-end tests** (Task 10) start `loctel --headless`, drive these
  scenarios through real OTLP and assert through an MCP client.
- **Screenshot tests** (Task 11 and later) render views from fixed meshgen
  data (a fixed seed and base time), which is why `CheckoutFailure` is frozen.
- **Benchmarks** (Milestone 5) use `HugeTrace` and `LogFlood`.

Add a scenario by adding a variant to `Scenario`, a module for it, a stream
constant in `scenario.rs` (so it never shares IDs), a CLI value in `main.rs`,
tests, and a section above.
