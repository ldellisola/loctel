---
name: perf-budgets
description: "Run, read and fix loctel's performance budgets (cargo xtask perf, the `perf` job of ci.yml). Use whenever the `perf` job is red, a change touches the store, the ingest path, the segment format, the Logs or trace data paths or the startup, you want to know whether a change made loctel slower, or a budget has to be measured, profiled or (with a recorded decision) changed. Covers running one budget, reading the table, profiling a breach with perf or cargo flamegraph in release, and the rule that budgets are never raised to get green."
---

# The performance budgets

Design §10.3 turns the throughput and latency targets into a check that fails CI:
`cargo xtask perf` builds `loctel-perf` and `loctel` in **release**, measures every
budget on this machine (4 CPUs, no GPU, like the CI runner) and prints a table of
measured against budget. It exits non-zero when one is missed. The table of
budgets, what each scenario builds and how it measures is in
[`docs/areas/testing.md`](../../../docs/areas/testing.md) ("Performance budgets");
the code is `crates/loctel-perf/` (`src/budget.rs` is the table).

## The rule

**A budget is never raised to get green.** A missed budget means the code got
slower, or never met it: fix the code. A budget changes only with a decision file
in `docs/decisions/` that carries the measurements (before, after, what was
tried) and says why the target itself was wrong, and that is acceptable only for
`first_frame`, which is measured on software rendering (lavapipe) while the real
target is Apple Silicon. Do not edit `src/budget.rs`, the plan or the design to
make a number fit, do not shrink a scenario's data (10M logs is the budget), do not
skip a row, and do not run the check in debug: the numbers mean nothing there.

## Run it

```bash
cargo xtask perf                          # every budget, a few minutes after the release build
cargo xtask perf --only logs_at           # the budgets of one id (`ingest` has three rows)
cargo xtask perf --json perf.json         # also write the report as JSON (what CI uploads)
```

The ids are `ingest`, `trace_open_hot`, `trace_open_cold`, `logs_count`,
`logs_at`, `histogram`, `traceql_first_page`, `logql_first_page`,
`startup_headless`, `first_frame` and `rss_10m`; an unknown one says which exist.
The first release build takes about 8 minutes on 4 CPUs (about 2.5 GB of
`target/release`); after that a run is the data build (about a minute for the log
store, as much for the span store) and the measurements. Progress goes to stderr,
one line per step (`perf [   42.1s] ...`), the table to stdout. Exit codes: 0 every
budget held, 1 a budget was missed, 2 a scenario could not run (the message says
why; a scenario also fails when the store does not hold what it loaded, so a
budget is never measured on data that was evicted).

Build with both packages in one command (`cargo build --release -p loctel-perf -p loctel-app`,
which `cargo xtask perf` does): the two need different features of the same crates,
and building one alone changes them and rebuilds GPUI for three minutes. To run the
binary by hand after that build: `target/release/loctel-perf --only logs_at` (it finds
`target/release/loctel` beside itself for `startup_headless`).

`cargo run -p loctel-perf -- --smoke` is a tiny run (200,000 logs, a one second
ingest) that shows the scenarios work; its numbers mean nothing. `cargo test -p
loctel-perf` runs the same tiny scale in debug, which is what the gate holds.

## Read the table

```
id               measure                                   budget      measured    verdict
logs_at          logs_at(offset, 50), 100 random offsets   < 20 ms     38.4 ms     BREACH
```

`measured` is in the budget's unit; `verdict` is `ok`, `BREACH` or `n/a` (not
measurable here, with the reason beside it: `rss_10m` reads `/proc`, so only on
Linux). Timings are the median of 5 runs, except the percentiles (`logs_at` is the
p95 of 100 offsets, `ingest` has a p99). Look at the `perf` lines above the table
too: they say what each scenario found (lines counted, traces on disk, the p50
beside a p95), which usually shows where the time went.

In CI the table is in the log of the `perf` job, and `perf.json` is the artifact of
the run (`rows[].id`, `measured`, `budget`, `verdict`; `breaches`), kept when the
job fails.

## Profile a breach

Release builds carry no debug info by default, so ask for line tables, then run
only the budget that broke, under `perf` or `cargo flamegraph`:

```bash
export CARGO_PROFILE_RELEASE_DEBUG=line-tables-only
cargo build --release -p loctel-perf -p loctel-app
perf record -F 999 --call-graph dwarf -- target/release/loctel-perf --only logs_at
perf report --no-children | head -60
# or, if cargo-flamegraph is installed:
cargo flamegraph --release -p loctel-perf -- --only logs_at     # writes flamegraph.svg
```

(`perf` comes from `linux-tools-generic`; a container may not allow it. Without it,
time the steps yourself: the scenarios print progress, and a unit of work can be
timed in a scratch test with `Instant`. The bulk data is `loctel_meshgen::bulk_logs`
and `bulk_traces`, and `loctel_perf::load` builds a store the way the scenarios
do.) Then:

1. Find the function that owns the time and check it against what the budget
   needs. The numbers in `docs/decisions/0046-log-chunks.md` say what a chunk read
   costs and why.
2. Fix it in the store, the engine or the model, with a test first like any
   change, and keep the store's public API and every existing test as they are.
3. Run the budget again, then the whole `cargo xtask perf` (a fix for one can cost
   another), then the quality gate.
4. Say what you found in the decision log when the fix changes a design choice
   (a format, a cache, a lock), with before and after numbers.

## When a budget is only flaky

The scenarios are deterministic (seeded data, a clock that follows the data), but
timings are not. If a budget passes by a hair locally and fails in CI, run
`cargo xtask perf --only <id>` five times and look at the spread before deciding
anything: a median that sits within 10% of its limit has no room for a noisy
runner, which is a reason to make the code faster, not to move the limit. Never
add retries to the check.
