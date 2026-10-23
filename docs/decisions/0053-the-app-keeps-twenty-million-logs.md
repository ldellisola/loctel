# 0053: The app keeps twenty million logs

Date: 2026-10-23
Status: accepted

Milestone 5, whole-branch review (round 1). Design 9.2 says the Logs list "must handle 10M+
lines" and section 5 says the store supports "random access by row position, for scrolling
10 million log lines". Everything of Milestone 5 was built and measured for that (the log
chunks and blocks, `logs_at`, the windowed list, the budgets over ten million logs, the
`logs_10m_middle_dark` screenshot), but the app never got there: the engine passed the
store the limits of Milestone 1 unchanged, `max_log_records` 5,000,000 and `max_traces`
200,000, with spilling on. The count limit, not the 4 GiB disk cap, decides what is evicted
(five million bulk logs are 150 MB on disk), so a real run evicted the oldest logs at five
million. The 10M list, budgets and screenshot held only in harnesses that raised the limits
themselves (`loctel-perf` to 100M logs and 20M traces, the Logs screen's 10M test the same);
decision 0049 said so, as "the harness's config, not a change to the app's defaults". This
file changes that for logs, and says why not for traces.

## Decision

**A store that has a run directory keeps up to 20,000,000 logs.** The store's own default
does not move (`StoreConfig::default()` is still 200,000 traces and 5,000,000 logs: the
limits of a store in memory, which the tests of M1 to M4 pin and which `--no-spill` and a
run directory that cannot be made keep). Two things are new:

- `StoreConfig::SPILLING_MAX_LOG_RECORDS` (20,000,000) and
  `StoreConfig::with_spill_limits(self)`, which gives `max_log_records` that value when it
  still has the default and leaves any other value alone (a test that wants a store that
  forgets after four logs; a harness that holds more).
- `Engine::start` calls it right after it has made the run directory, and not on the
  fallback to memory. The app passes the engine the default store config, so the app is
  where the limit takes effect, and `crates/loctel-engine/tests/spill.rs`
  (`a_spilling_engine_keeps_more_logs_than_a_store_in_memory_does`) feeds an engine five
  million and a hundred thousand logs and checks that none was evicted.

The number is twice the 10M of the design, so that "10M+" has room, and it is what the
memory budget supports. Measured in release with `loctel-perf`'s own bulk data and limits
raised out of the way (a throwaway probe, not kept), the resident size after the load:

| Held | Resident |
|---|---|
| 10M logs | 221 MB |
| 1M spans + 10M logs (what `rss_10m` measures) | 910 MB |
| 1M spans + 20M logs | **1,061 MB** |
| 2M spans (200,000 traces, the trace limit) + 10M logs | 1,633 MB |
| 2M spans + 20M logs | 1,784 MB |

A log costs about 15 bytes of memory once it is in a segment, so each ten million more
cost about 150 MB. At the 20M the budget of `rss_10m` (under 1.5 GB at 10M logs and 1M
spans) still has 440 MB to spare, and on disk the same store (the 20M logs and the 1M
spans) is 551 MB of the 4 GiB cap (a real log is bigger than a bulk one, so with real data
the cap is nearer). The last two rows are not a result of this change: two million spans
in memory and then in segments already take 1.4 GB resident, because the allocator keeps
what the hot spans used (`hot_max_spans` is 2,000,000), and the logs add their 150 MB per
ten million to that. Lowering that peak is a question about the hot side of the store, not
about this limit.

### Traces: 200,000 stays

`max_traces` is not raised. Measured the same way, a store of 10 million spans (a million
traces at ten spans each) took 2.4 GB resident and 20 million took 3.7 GB: a cold trace
costs about 1.3 KB of memory, because the index keeps its whole `TraceSummary` (decision
0045). A count that held ten million spans would break the memory budget by itself, and
nothing in the design asks for it: the "10M+" of 9.2 is about log lines, and the 10M
stored items of the search budget (TraceQL or LogQL, first page in 1 s) are a store the
harness builds, which `traceql_first_page` measures with its own raised trace limit
(20,000,000), as it always did. What the app keeps is 200,000 traces, which at the ten
spans a trace of the budgets are two million spans, and a TraceQL search over fewer
spans than the budget's is no slower. If the memory of a cold trace is ever brought
down (a smaller index entry, the summary read from the segment on demand), this limit
should be raised with it, and this file is where to say so.

### The harnesses use the app's limit

`loctel-perf` (`load::store_config`) and the 10M tests of the Logs screen
(`crates/loctel-ui/tests/logs_scale.rs`) now take `max_log_records` from
`StoreConfig::SPILLING_MAX_LOG_RECORDS` instead of a number of their own, so that they run
under what the app ships, and `expect_held` fails if that limit ever drops below what a
budget loads. Their other raised limits (`max_traces`, the 64 GiB disk cap) stay, for
the reasons above. This supersedes the 100M logs of decision 0049.

## Consequences

- The Logs list of a running app scrolls through 10 million lines and keeps going to 20
  million, by count; the disk cap (4 GiB by default, Settings) can end it earlier when the
  lines are big, as the design says, and it evicts the oldest segment whole.
- Memory grows by about 150 MB for every ten million logs held, which is the number to
  look at before moving `SPILLING_MAX_LOG_RECORDS`.
- Nothing about the Mac is new: this is in the engine and the store, which run on Linux.
  Whether a Mac with 8 GB is comfortable at 1 to 1.8 GB resident is on the Milestone 5
  checklist's "memory levels off" line.
