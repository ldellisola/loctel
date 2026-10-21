# 0007: End-to-end harness choices

Date: 2026-10-21
Status: accepted

## Context

The Milestone 1 plan (Task 10) fixes the harness interface (`Harness::start`,
`otlp`, `mcp`, `tool`, `wait_complete`) and six tests, and says the tests start
"an engine on ports 0". The design (§10.3) says the end-to-end tests "start
loctel headless". It leaves open how the engine is started, how the clock and
the proxy environment are handled, and which crate depends on what. These are
the choices a later agent might be tempted to undo.

## Decision

- **The engine runs in the test process, not as the `loctel` binary.** The
  harness calls `Engine::start`, which is what the binary's `main` calls, so
  the receivers, the store, the MCP server and the pause switch are the real
  ones. The binary adds argument parsing and printing the listener lines,
  which `crates/loctel-app/tests/headless.rs` already tests against the built
  binary (start, lines, SIGTERM). In-process is what the plan's
  `Harness::start(completeness)` and `engine().set_paused(..)` need: a test
  cannot flip pause on another process, and a process per test adds spawning and
  parsing stdout for no extra coverage of the receivers or MCP. Decision 0004 expected
  the e2e crate to read the headless lines; it does not, and that sentence was
  corrected there.
- **`Harness::start` is synchronous and starts the engine on a thread of its
  own.** `Engine::start` refuses to run inside a tokio runtime (it blocks on
  its own), and every test is a `#[tokio::test]` because `tool` and `send` are
  async. `thread::scope` plus `join` is the shortest way to satisfy both; it
  blocks the test's runtime thread for the time of binding three sockets.
  `EngineHandle::shutdown` is safe to call from inside a runtime (it does not
  wait there), which is what `Drop for Harness` relies on.
- **Real clock, real sockets, small `completeness_quiet` (200 ms).** Completeness
  is judged by arrival time on the store's clock, and the engine uses the system
  clock, so a test that wants a complete trace waits about 200 ms. The harness
  does not inject a manual clock: `loctel-store` and `loctel-mcp` tests already
  do, and the point here is that nothing is faked. `wait_complete` polls every
  25 ms with a 10 s limit, so a slow CI machine fails with a message rather
  than flaking.
- **Every send is dated now.** `Harness::send` generates the scenario at the
  system time, so a marker taken from `get_marker` just before (the store's
  clock, also the system clock) is at or before every span of that scenario,
  and `since` finds exactly the traces sent after it. Tests give each send its
  own seed, because the same scenario and seed is the same trace and the store
  keeps it once (decision 0005).
- **The MCP client does not use a proxy.** The harness builds the reqwest
  client itself (`no_proxy()`) and hands it to rmcp's transport, as
  `loctel-meshgen` does for OTLP. Otherwise a developer's `HTTP_PROXY` without
  a matching `NO_PROXY` would send the request to a proxy instead of
  `127.0.0.1`.
- **`tool` checks the text copy.** Every MCP result carries `structuredContent`
  and the same JSON as text (§8), so `Harness::tool` asserts the two are equal
  on every call, which makes all the end-to-end tests also tests of that
  promise over the real transport. A tool error panics in `tool`; tests of
  error paths use `McpClient::call`, which returns a `ToolOutcome` and keeps
  rmcp's types out of the tests.
- **The harness's dependencies are ordinary dependencies.** The plan lists the
  engine, meshgen, the rmcp client and tokio as dev-dependencies, but the
  harness is the crate's library (`tests/*.rs` share it), and a library cannot
  use its own dev-dependencies. The crate is `publish = false` and nothing
  depends on it, so there is no cost.

## Consequences

- A new test in `loctel-e2e` needs only `loctel_e2e::{Harness, TransportKind}`
  and `loctel_meshgen::Scenario`; see "End-to-end tests" in
  `docs/areas/testing.md`.
- Whatever starts or stops the real binary (the headless line format, signals,
  the release smoke test) is tested in `loctel-app` and `xtask`, not here.
- The frozen `CheckoutFailure` is asserted literally in the e2e tests. Changing
  it means updating them (see `docs/areas/testing.md`).
