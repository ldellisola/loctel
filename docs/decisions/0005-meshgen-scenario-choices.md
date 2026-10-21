# 0005: Fake mesh scenario choices

Date: 2026-10-21
Status: accepted

## Context

The Milestone 1 plan (Task 8) fixes the `CheckoutFailure` table, the scenario
list and the interfaces of `loctel-meshgen` (`Scenario`, `Batch`, `Generated`,
`generate`, `Transport`, `send`, `SendReport`, and the command line), and says
IDs come from a ChaCha RNG seeded with `seed`. It leaves some content and
behaviour to the builder. The choices below are the ones later agents might be
tempted to "simplify", or that screenshots and e2e tests will depend on. The
scenarios themselves are described in `docs/areas/testing.md`.

## Decision

- **Every scenario reads its own ChaCha stream.** The RNG is still a ChaCha8
  seeded with `seed`, but each scenario calls `set_stream` with its own
  constant. Reason: with one shared stream, the same seed gave
  `CheckoutFailure` and `CheckoutSuccess` the same trace ID and the same span
  IDs. The command line's default seed is 1, so `meshgen --scenario
  checkout-failure` followed by `--scenario checkout-success` against one
  loctel would have put the second trace's spans into the first trace, where
  the store's duplicate check (by span ID within a trace) drops them without a
  word. The same hazard applies to e2e tests that send two scenarios with one
  seed. The same scenario with the same seed is still the same trace, on
  purpose: it is what makes the output reproducible.
- **`CheckoutSuccess` is the failing tree with the changes the plan lists, and
  nothing else moves.** The timings stay (a slow success of 1.84 s). Two
  details the plan does not state: spans 1 and 4 carry
  `http.response.status_code=200` (a 500 on a successful request would
  mislead anyone, or any test, that looks at attributes), and the success logs
  are six INF lines: the failing trace's first three, then `Charge accepted for
  order 8812`, `Payment completed for order 8812` and `Checkout completed for
  order 8812`.
- **`LateOutOfOrder` counts depth from 0 at the root.** "Depth 3 and deeper"
  is then #5 to #12 and #14, #15 (the database calls, the downstream HTTP
  calls and the messaging spans), the middle batch is #2, #3 and #4, and the
  root is last. A log is sent with the batch of its span.
- **Health checks and the log flood use all five services, the huge trace the
  four app services.** The plan says "per service" for health checks,
  "across the services" for the flood and "the 4 app services" for the huge
  trace. `APP_SERVICES` is the four that serve HTTP (`notifications-worker`
  only consumes messages).
- **Health-check spans also carry `http.request.method=GET` and
  `http.response.status_code=200`**, and the access log is linked to its span
  (the plan names only `url.path=/health` and the log text). `CheckoutFailure`
  gets no attribute beyond the plan's table.
- **Attribute types.** `http.response.status_code`, `http.request.resend_count`
  and `order.id` are integers, everything else strings, so TraceQL comparisons
  in Milestone 2 can use numbers. Erroring spans carry a status message
  (`HTTP 500`, the exception message, `timeout`).
- **`send` fails on `partial_success` rejections**, not only on transport
  errors. Generated data is always valid, so a rejection is a bug, and a test
  that ignored it would count rejected spans as delivered. `SendReport` keeps
  exactly the plan's two fields.
- **HTTP bodies of 64 KiB or more are gzipped** (with `flate2`), smaller ones
  go out plain. reqwest's `gzip` feature, which the plan names, only
  decompresses responses, so without this the big scenarios would never
  exercise the receivers' gzip path.
- **reqwest is built without default features** (`rustls` and `gzip` only) and
  the client calls `no_proxy()`. The default `system-proxy` feature routes
  requests for 127.0.0.1 through `HTTP_PROXY`/`HTTPS_PROXY`, which is set in
  the agent containers and would send meshgen's traffic to the proxy instead
  of loctel.
- **The command line gains an optional `--size N`** (health checks per
  service, spans in the huge trace, records in the log flood), because the
  plan's flags give those three scenarios no way to be sized. Defaults: 20,
  100,000 and 10,000. `--scenario`, `--transport` and `--endpoint` stay
  required, as in the plan.
- **Dependencies.** `rand 0.10` and `rand_chacha 0.10` (the current releases;
  `rand` without default features, so nothing reads the OS entropy source),
  `reqwest 0.13`.
