# 0049: The performance harness and the CI budget check

Date: 2026-10-22
Status: accepted

Milestone 5, Task 6. The plan fixes the budgets (Global Constraints), the crate
(`loctel-perf`, `publish = false`, bin `loctel-perf`), the command (`cargo xtask perf
[--only ID] [--json PATH]`), the CI job (`perf`, beside `gate`) and the interfaces of
the bulk generators (`meshgen::bulk_logs(n, seed)` and `bulk_traces(n_spans, seed)`).
This file records what it left open, the measurements the first run gave, and the one
budget that failed (`logs_at`, decision 0050).

## Decisions

### The crate and the table

- **A library and a binary.** `loctel_perf::budget` is the table (`Budget`, `Limit`,
  `Measurement`, `Report`) and is pure; the scenarios are modules (`logs`, `traces`,
  `ingest`, `startup`, `frame`) over a `load` module that builds the data; `run::measure`
  runs the ones asked for; `main.rs` is the command line. The plan's unit test
  `budget_table_flags_breaches` is in `budget.rs`; the others pin the table to the plan's
  values (a change to a limit is a change to a test, and a decision).
- **One row per budget, `ingest` has three.** The plan's table puts three conditions in
  one row (rate, nothing dropped, p99). They are three budgets with the id `ingest`
  (`--only ingest` runs and shows all three), so each has its own number and its own
  verdict. A `Measurement` names the `&'static Budget` it is for, so a number cannot be
  held to another's limit, and `Report::new` puts the rows in the table's order whatever
  order the scenarios finished in.
- **Strict where the plan says "<".** `Limit::Below` is strict (250 ms is a miss),
  `AtLeast` and `AtMost` are not. A number that is not a number is a breach.
  "1.5 GB" is 1,500,000,000 bytes, the smaller of the two readings, and the table says
  `< 1.50 GB` (decimal, like the status box).
- **Exit codes.** 0 every budget held, 1 a budget was missed, 2 a scenario could not run
  (it did not find the data it built, or the loctel binary is missing). A scenario never
  reports a number for data it did not hold: after loading, `expect_held` fails the run
  unless the store holds every span and log that was loaded.
- **A smoke scale.** `Scale::SMOKE` (200,000 logs, 100,000 spans, a one second ingest) is
  what `crates/loctel-perf/tests/smoke.rs` runs in a debug build in seven seconds, so the
  gate holds that every scenario runs and finds its data; `--smoke` runs it by hand. Its
  numbers mean nothing and nothing compares them with a budget.

### The data

- **Bulk generators** (`crates/loctel-meshgen/src/bulk.rs`): lazy iterators of `Batch`es of
  1,000 items (`in_batches_of` changes it, `starting_at` moves the first time) on streams
  of their own, deterministic per seed. Logs are 20 us apart (50,000 a second, the design's
  target) over the five services of the mesh: 35% debug, 50% info, 10% warn, 5% error; a
  quarter carry a trace and a span ID (sixteen consecutive logs share a trace); about 1 in
  100 of `payments-api`'s says `timeout` (a warn or an error), so
  `{service_name="payments-api"} |= "timeout"` finds its first 50 among the newest 25,000
  lines and not by luck in the first one. Traces are ten spans over four services (a
  request through `web-bff`, `orders-api`, `inventory-api` and `payments-api`), one
  starting every 200 us, 1% failed (the call chain from the failed leaf to the root is in
  error, so the root fails). A trace never splits across batches while the batch size is a
  multiple of ten. The shape and the mix are mine, not the plan's: a change to them moves
  the numbers of the budgets, and the tests in `crates/loctel-meshgen/tests/bulk.rs` pin
  them.
- **The clock follows the data.** The big stores run on a `ManualClock` that is set to the
  latest time of each batch, so that traces complete 10 s later, spill 30 s after that and
  logs seal by count and by age as they would in a long run, in the seconds a check has. A
  store on the system clock would hold 10M spans hot (nothing is complete in the first
  ten seconds) and measure memory, not the budget. `settle` then lets 90 s pass and runs the
  spill step, so that `traceql_first_page` searches spans that are in segment files, like
  the trace budget it is.
- **The limits are raised for the budget stores.** `max_traces` 20,000,000,
  `max_log_records` 100,000,000 and a 64 GiB disk cap, because the app's defaults (200,000
  traces, 5,000,000 logs, 4 GiB) would evict half of what a budget over ten million logs
  is meant to read. This is the harness's config, not a change to the app's defaults (a
  store that holds ten million logs, like the one Task 7's test builds, has to raise them
  too). **Superseded in part by decision 0053:** the app does keep 20,000,000 logs when it
  has a run directory, and the harness takes its log limit from that
  (`StoreConfig::SPILLING_MAX_LOG_RECORDS`) instead of 100,000,000; the trace limit and the
  disk cap stay raised.
- **Spans first, then logs, in the store of the log budgets.** The 1M spans of `rss_10m`
  go in first; the 200 seconds of logs after them carry the clock far enough that the
  spans are complete and on disk when the number is read, which is what a long run looks
  like. The 10M spans of `traceql_first_page` are a store of their own (they would add
  nothing to the log budgets and double the resident size).

### The scenarios

- **`rss_10m` is read right after the load**, in the first scenario of a process that has
  built nothing else, before any query has filled the cache of decoded chunks (decision
  0046: 564 MB after a round of queries). It reads `VmRSS` of `/proc/self/status`, and is
  "not measured" off Linux, with the reason in the table. **920 MB**: more than the 230 MB
  that decision 0046 measured for logs alone, because a million hot spans are in memory
  until they are complete and 30 s old, and the allocator does not give all of that back.
- **`ingest` is a closed loop.** A real engine runs in the process with its run directory
  and one discard rule that matches nothing in the data (`{ span.url.path = "/health" }`,
  so every batch is judged and none is cut). Two senders, each a keep-alive HTTP/1.1
  connection, post OTLP protobuf (no compression) as fast as the engine answers,
  alternating a batch of 1,000 spans and a batch of 1,000 logs, for 20 s; a thread encodes
  the batches ahead of them. The budget is a floor under what loctel sustains, so the
  scenario measures what it sustains instead of offering exactly 50,000 a second (a paced
  load at the budget would read 49,9xx from the last in-flight request, and says nothing of
  the margin). The rate is items the engine counted as accepted over the time from the
  first request to the last answer; the latency is of every request from sending it to
  reading its answer; "dropped" is every item sent that the engine did not count as
  accepted (failed in transit, rejected in an answer, cut by the rule). Items the store
  evicts for its limits are not dropped: they were accepted.
- **`trace_open_*` open the same 100,000-span trace.** Hot: the store has no segment files.
  Cold: five traces (seeds 1 to 5) are spilled and each is read once, so that no run finds
  its trace in the cache of 64 decoded traces; the median is of five different reads.
  Opening is `store.trace(id)` and `Waterfall::new`, as the trace screen does it.
- **`histogram` asks the way the Logs screen has to**: from a whole second, in buckets of a
  whole number of seconds (decision 0046: any other window reads a chunk per bucket edge,
  1.7 s). The answer is checked against `log_count` before it is timed.
- **`first_frame` is a process per run** (`loctel-perf --first-frame-once`, five of them,
  the median reported). The first frame of a process pays for what is set up once (the
  Vulkan driver, the glyph atlases, the pipelines) and is the only one anybody sees, so the
  second window of a process is not it. What is timed is `Driver::new` (headless platform,
  window, `MainWindow::new`, first frame) and the capture of that frame, with the
  checkout traces and health checks in the store; a frame of one colour fails the run.
- **`startup_headless` starts the real release `loctel --headless`**, five times, on scratch
  settings and rules files and a scratch `TMPDIR` (so it never touches the real ones and
  has no run directories to clear), and times from `spawn` to the arrival of the third
  listener line, stamped by a reader thread. It stops it with SIGTERM, as a user would.

### The build

- **One release build for both binaries**: `cargo build --release -p loctel-perf -p
  loctel-app`. `loctel-perf` needs `loctel-ui`'s `screenshot` feature (the first-frame
  budget draws the main window headlessly), and one `cargo build` gives every package the
  union of the features, so the `loctel` that `startup_headless` starts has GPUI's test
  support compiled in beside the shipped code. It is never touched in headless mode. A
  second build of GPUI for the other feature set costs minutes and gigabytes, and the
  difference is in code that does not run. Building `-p loctel-perf` alone (a different
  feature union) rebuilds the dependencies that differ, three minutes on 4 CPUs: always
  build the two together, which is what `cargo xtask perf` does.
- **CI**: a job `perf` on `ubuntu-24.04` beside `gate`, `setup-linux.sh`, the cache,
  `cargo xtask perf --json perf.json`, and the upload of `perf.json` as the `perf` artifact
  even when the check fails (`if: always()`). The first run builds in release for about
  eight minutes; the cache brings later ones down to the check itself (two minutes here).

## What the first run measured

On this container (4 vCPU, release), before and after decision 0050:

| budget | first run | budget | after the fix |
|---|---|---|---|
| `ingest`, items a second | 417,345 | >= 50,000 | 389,296 |
| `ingest`, dropped | 0 | 0 | 0 |
| `ingest`, p99 of a batch | 83 ms | < 250 ms | 95 ms |
| `trace_open_hot` | 144 ms | < 250 ms | 137 ms |
| `trace_open_cold` | 185 ms | < 250 ms | 201 ms |
| `logs_count` | 0.00 ms | < 50 ms | 0.005 ms |
| `logs_at`, p95 (p50) | **211 ms** (165 ms) | < 20 ms | 4.9 ms (2.8 ms) |
| `histogram` | 0.01 ms | < 100 ms | 0.015 ms |
| `traceql_first_page` | 99 ms | < 1 s | 110 ms |
| `logql_first_page` | 2.9 ms | < 1 s | 3.1 ms |
| `startup_headless` | 5.6 ms | < 300 ms | 5.4 ms |
| `first_frame` | 510 ms | < 1.5 s | 504 ms |
| `rss_10m` | 926 MB | < 1.5 GB | 920 MB |

`trace_open_hot` and `_cold` have the least room (hot 120 to 137 ms, cold 175 to 201 ms of
250 ms, over several runs). Where the time goes, hot: 44 ms to copy the 100,000 spans out of
the store and 75 ms for `Waterfall::new` (a row of strings per span); cold adds the decode
of the trace (about 45 ms more, and the waterfall measured 90 ms in the same process). No
budget was changed, and a CI runner much slower than this container would be the first to
show these two.

## Not done

- A paced ingest at 50,000 items a second with the latency measured from the intended send
  time (the open-loop test of coordinated omission). The closed loop shows the margin (8x)
  and the p99 under saturation, which is the harder case.
- `first_frame` on the Mac (the target is 500 ms; the budget here is for software
  rendering). It stays a line of the Mac checklist.
- The p50 and the spread of the medians are in the progress lines on stderr, not in the
  JSON.
