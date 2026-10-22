# loctel Milestone 5 (scale: spill to disk, disk cap, performance budgets): Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** loctel takes a lot of data without growing in memory:

- Completed traces and sealed log chunks move to temporary segment files.
- Disk use stays under the **4 GB** cap, which is user-configurable, by
  dropping the oldest data whole.
- The performance targets from §10.3 become an automated budget check in
  CI.
- The Logs list scrolls through 10 million lines.
- Settings gains the disk cap and the popover the disk tile.
- The light theme passes a contrast check.

**Architecture:** The store keeps its **public API** (M1–M4), and every
existing store test keeps passing. Internally it becomes:

- **Traces:**
  - a **metadata index** of every trace held, in memory, ordered by start
  - **hot traces** in memory: incomplete or recently touched
  - **cold traces** in compressed **segment files**
  - a late span for a cold trace brings that trace back hot
- **Logs:** an in-memory **open chunk**, plus **sealed chunks** in segment
  files, with per-chunk metadata (time range, counts per level and service)
  that answers counts, offsets and the histogram without decoding.
- **Disk cap:** enforced by deleting the oldest segment files whole.
- **Lifetime:** everything lives in a per-run directory, deleted on quit;
  directories left by a crash are deleted at the next start.
- **Performance:** a `loctel-perf` harness, run by `cargo xtask perf`,
  measures the budgets in release mode and fails CI when one is missed.

**Tech Stack:** the additions are `postcard` (serde, compact binary),
`zstd` (level 1), `memmap2` (reads of sealed segments) and `crc32fast`
(segment checksums). Everything else is unchanged.

**Spec:**

- `docs/superpowers/specs/2026-10-21-loctel-design.md`: §5 (storage), §10.3
  (performance tests), §4 (throughput), §9.2 (Settings, popover), §9.3
  (look)
- `docs/areas/store.md`, `docs/areas/ui.md`
- the M1–M4 plans
- `AGENTS.md`

## Global Constraints

Everything in the M1–M4 Global Constraints still holds. M5 adds:

- **The store's public API is frozen.** M5 adds to it but never changes an
  existing signature or result. Every test in `crates/loctel-store/tests/`
  must pass **unchanged in its assertions**, and must also run against a
  spilling store (see Task 2).
- **Values:**

  | name | value |
  |---|---|
  | `SPILL_AFTER` | 30 s after a trace is complete |
  | `HOT_MAX_SPANS` | 2,000,000 spans in hot traces before early spill of the oldest complete ones |
  | `SEGMENT_MAX_BYTES` | 64 MiB, then the segment rolls |
  | `LOG_CHUNK_MAX` | 65,536 records or 10 s, whichever comes first, then the chunk seals |
  | `CHUNK_CACHE` | 8 decoded chunks, least recently used |
  | zstd level | 1 |
  | default disk cap | 4 GiB (`Settings::disk_cap_bytes`) |
  | allowed disk cap | 1–100 GB in Settings |
- **Run directory:**
  - `std::env::temp_dir()/loctel/run-<pid>-<start_unix_secs>/`
  - At start, every sibling `run-<pid>-*` whose pid isn't a live process is
    deleted.
  - At shutdown, the run's own directory is deleted.
  - `StoreConfig::spill_dir: Option<PathBuf>`: `None` keeps everything in
    memory (tests and headless runs can choose). The app passes `Some`.
- **Disk-cap semantics:**
  - `disk bytes` is the total size of the segment files.
  - Over the cap, delete the **oldest whole segment file**, its traces and
    logs with it, until under the cap.
  - Traces never straddle segments, so eviction stays by whole trace.
  - Changing the cap applies at once.
- **Every segment record** carries a CRC32. A record that fails its check
  is skipped and logged once per segment, never a panic.
- **Performance budgets** (`cargo xtask perf`, release build, on
  `ubuntu-24.04` with 4 vCPU):

  | id | measure | budget |
  |---|---|---|
  | `ingest` | OTLP HTTP protobuf, batches of 1,000 spans plus 1,000 logs, one discard rule on, spill on, for 20 s | sustained ≥ 50,000 items/s accepted, 0 dropped, p99 batch latency < 250 ms |
  | `trace_open_hot` and `trace_open_cold` | `store.trace(id)` plus `Waterfall::new` for a 100,000-span trace | < 250 ms each |
  | `logs_count` | `log_count` over 10M logs, filter = levels plus service | < 50 ms |
  | `logs_at` | `logs_at(offset, 50)` at 100 random offsets over 10M logs | p95 < 20 ms |
  | `histogram` | 48 buckets, 10M logs, indexed filter | < 100 ms |
  | `traceql_first_page` | `{ status = error }` over 10M spans (about 1% erroring traces), limit 20 | < 1 s |
  | `logql_first_page` | `{service_name="payments-api"} |= "timeout"` over 10M logs, limit 50 | < 1 s |
  | `startup_headless` | process start to the last listener line, release `loctel --headless` | < 300 ms |
  | `first_frame` | main window construction plus first headless render (lavapipe), release | < 1.5 s (Mac target 500 ms, checked by hand) |
  | `rss_10m` | resident memory after 10M logs and 1M spans with spill on | < 1.5 GB |

  Each timing is the **median of 5 runs**, except the p95 and p99 lines and
  `ingest`. A breach fails the check with a table of measured against
  budget.
- **Contrast** (WCAG relative-luminance formula), in both themes:
  - `text`, `text_secondary` and `text_tertiary` on `bg` and on `panel`:
    ≥ 4.5
  - `error_text`, `warn` and `accent` (as text) on `panel`: ≥ 4.5
  - `border` against `panel`: ≥ 1.3, so it stays visible

## Review Focus

1. **A late span or log for data already spilled** (a slow service, a
   retried exporter). Expect the trace rehydrated, completeness reset, the
   old blob dead (not double-counted), and logs merged in time order across
   chunks. *Tests:* Task 2 `late_span_rehydrates_cold_trace`, and Task 3
   `late_log_lands_in_time_order_across_chunks`.
2. **A crash mid-write**: a truncated last record in a segment. Loading,
   search and eviction skip it and nothing panics. *Test:* Task 1
   `truncated_record_is_skipped`.
3. **The disk cap lowered below current use while the app runs.** Expect
   the oldest segments to go at once, the counters to stay consistent, and
   an open trace view to be told its trace is gone, not crash. *Tests:*
   Task 4 `lowering_cap_evicts_now_and_keeps_stats_consistent`, and Task 7
   `evicted_open_trace_shows_gone`.
4. **The temp volume filling up** (a spill write fails with ENOSPC). Expect
   the store to stop spilling, keep data in memory up to its caps, report
   `spill_error` in the status, and keep ingesting without a crash. *Test:*
   Task 4 `spill_write_failure_keeps_ingesting_and_reports`.
5. **Discard rules and `remove_trace` on cold data** (a whole-trace drop
   whose stored spans are spilled). Expect the trace gone from the index and
   its blob dead, and the counts right. *Test:* Task 2
   `remove_trace_on_cold_trace`.

---

## File map (M5)

```
crates/loctel-model/src/*.rs                 + serde Deserialize on Span, LogRecord, Resource, ids, AttrValue
crates/loctel-store/src/segment.rs           NEW: file format, writer, reader (mmap), CRC, zstd
crates/loctel-store/src/run_dir.rs           NEW: run directory, stale cleanup
crates/loctel-store/src/trace_index.rs       NEW: TraceMeta, ordered index, hot/cold
crates/loctel-store/src/log_chunks.rs        NEW: open chunk, sealed chunks, meta, cache, merge
crates/loctel-store/src/{store.rs, logs.rs, config.rs, lib.rs}
crates/loctel-store/tests/modes.rs           NEW: the both-modes harness; existing tests call it
crates/loctel-store/tests/{spill.rs, disk_cap.rs, logs_scale.rs}
crates/loctel-engine/src/lib.rs              run dir, disk cap from settings, status disk fields
crates/loctel-mcp/src/{dto.rs, tools.rs}     disk fields in get_status; recent_failures single pass
crates/loctel-perf/                          NEW: perf harness binary
xtask/src/main.rs                            `perf` subcommand
.github/workflows/ci.yml                     `perf` job
crates/loctel-ui/src/{model/*, views/logs_view.rs, views/settings_view.rs, views/popover_view.rs, theme.rs}
docs/areas/{store.md, ui.md, testing.md}, docs/decisions/, docs/milestones/*, AGENTS.md
.claude/skills/perf-budgets/SKILL.md         NEW
```

---

### Task 1: Segment files and the run directory

**Files:**
- Modify: `crates/loctel-model/src/*.rs`. Add `Deserialize` next to every
  `Serialize`, round-tripping exactly.
- Create: `crates/loctel-store/src/{segment.rs, run_dir.rs}`
- Modify: the workspace `Cargo.toml` (`postcard`, `zstd`, `memmap2` and
  `crc32fast` pins)
- Test: `crates/loctel-store/tests/segment.rs`, plus
  `crates/loctel-model/tests/serde_roundtrip.rs`

**Interfaces:**
- Produces (crate-public, `pub(crate)` unless tests need them; use
  `#[doc(hidden)] pub` for test access):
  - `segment::Kind { Traces, Logs }`
  - `segment::Writer`:
    - `create(dir: &Path, kind: Kind, seq: u64) -> io::Result<Writer>`.
      The file is `traces-{seq:06}.seg` or `logs-{seq:06}.seg`.
    - `append(&mut self, payload: &[u8]) -> io::Result<Location>`, where
      `Location { seq: u64, offset: u64, len: u32 }`
    - `bytes(&self) -> u64`
    - `seal(self) -> io::Result<()>`
  - The record format is `[u32 len][u32 crc32 of the compressed bytes][zstd(postcard(payload))]`.
  - `segment::Reader`:
    - `open(path) -> io::Result<Reader>` (mmap)
    - `read(&self, loc: Location) -> Result<Vec<u8>, SegmentError>`, where
      `SegmentError` is `Truncated`, `Checksum` or `Decode`
    - `iter(&self) -> impl Iterator<Item = Result<(Location, Vec<u8>), SegmentError>>`
  - `encode_trace(spans: &[Span]) -> Vec<u8>` and
    `decode_trace(bytes) -> Result<Vec<Span>, SegmentError>`. Resources are
    interned per blob, so a resource is written once per trace.
  - `encode_logs(records: &[LogRecordWithId])` and `decode_logs`, the same
    way for chunks.
  - `run_dir::RunDir`:
    - `create(base: &Path) -> io::Result<RunDir>`: deletes stale siblings
      as Global Constraints say, and judges pid liveness with
      `kill(pid, 0)` on unix
    - `path(&self)`
    - deletes its directory on `Drop`
- [ ] **Step 1: Write the failing tests.**
  - `model_serde_roundtrip_all_types`
  - `segment_append_and_read_back`
  - `trace_blob_roundtrip_interns_resources`: a 15-span CheckoutFailure
    trace round-trips equal, and the blob holds each service's resource
    once
  - `log_chunk_roundtrip`
  - `checksum_mismatch_is_reported`
  - `truncated_record_is_skipped`: `iter` yields the good records and one
    `Truncated` error, and nothing panics
  - `run_dir_deletes_stale_siblings_and_itself_on_drop`: a fake sibling
    dir with a dead pid, such as 999999, is removed
- [ ] **Step 2: Watch them fail. Step 3: Implement. Step 4: Watch them
  pass.** Run `cargo test -p loctel-model -p loctel-store`.
- [ ] **Step 5: Docs.** Add a "Segment files" section to
  `docs/areas/store.md`, and the format decision to `docs/decisions/`.
  **Commit.** Message: `feat(store): segment files and run directory`.

### Task 2: The trace index, spill and rehydration, and the both-modes harness

**Files:**
- Create: `crates/loctel-store/src/trace_index.rs`
- Modify: `crates/loctel-store/src/{store.rs, config.rs, trace.rs, summary.rs}`
- Create: `crates/loctel-store/tests/modes.rs`, exposing
  `pub fn each_mode(f: impl Fn(Store, Arc<ManualClock>))`
- Modify: every existing file in `crates/loctel-store/tests/` that builds a
  `Store`. **Only their setup changes**, to run their bodies through
  `each_mode`, so each runs in memory and spilling. The assertions stay
  byte-for-byte the same.
- Test: `crates/loctel-store/tests/spill.rs`

**Interfaces:**
- Produces:
  - `StoreConfig` gains these fields, with the M1–M4 defaults otherwise
    unchanged:
    - `spill_dir: Option<PathBuf>` (default `None`)
    - `spill_after: Duration` (30 s)
    - `hot_max_spans: usize` (2,000,000)
    - `segment_max_bytes: u64` (64 MiB)
    - `disk_cap_bytes: u64` (4 GiB)
  - `StoreConfig::spilling_for_tests(dir: PathBuf) -> StoreConfig`:
    `spill_after = 0`, a 4 KiB segment size, `hot_max_spans = 16`. It
    exercises every cold path.
  - `TraceMeta`, internal: id, start, end, span count, a service set,
    status, failed-in, first exception, `last_arrival`, and
    `location: Hot | Cold(Location)`.
  - **The ordered index:** a `BTreeSet<(start, TraceId)>` plus a
    `HashMap<TraceId, TraceMeta>`. `search_traces` walks it newest first and
    filters on metadata (time, service, errors only, minimum duration)
    without touching span data. A TraceQL query decodes candidates newest
    first and stops when the page is full.
  - **Spill:** a background step runs on each ingest call and at most once
    a second. It moves complete traces past `spill_after` (or, past
    `hot_max_spans`, the oldest complete ones) to the open traces segment.
  - **Rehydration:** a span for a cold trace decodes it back hot and marks
    the old blob dead; dead bytes count toward the disk total until their
    segment is evicted.
  - **Cold reads:** `trace`, `summary`, `with_trace_spans`, `scan_traces`,
    `remove_trace`, `remove_spans` and `service_map` work for cold traces,
    decoding through a small LRU of decoded traces (64).
  - `Store::spilled_traces(&self) -> usize` (for tests and status)
- [ ] **Step 1: Write the failing tests.**
  - `modes.rs`, plus the conversion of the existing tests: they fail to
    compile until `spilling_for_tests` exists.
  - `spill.rs`:
    - `complete_trace_spills_and_reads_back_equal`
    - `late_span_rehydrates_cold_trace`
    - `search_order_and_cursor_identical_across_modes`
    - `remove_trace_on_cold_trace`
    - `traceql_search_decodes_newest_first_and_stops_at_page`: count the
      decodes with a test hook; for a page of 20 with matches near the top,
      fewer than 100
    - `service_map_identical_across_modes`
- [ ] **Step 2: Watch them fail. Step 3: Implement. Step 4: Run** `cargo
  test -p loctel-store`, where every existing and new test passes in both
  modes. Then run the full gate.
- [ ] **Step 5: Docs.** Add a "Hot and cold traces" section to
  `docs/areas/store.md`, and a decision for the index and spill policy.
  **Commit.** Message: `feat(store): trace index, spill to segments and
  rehydration`.

### Task 3: Log chunks, counts, offsets and the histogram from metadata

**Files:**
- Create: `crates/loctel-store/src/log_chunks.rs`
- Modify: `crates/loctel-store/src/{logs.rs, store.rs}`
- Test: `crates/loctel-store/tests/logs_scale.rs`. The existing log tests
  already run in both modes through Task 2's harness.

**Interfaces:**
- Produces:
  - `ChunkMeta`, internal: id range, min and max time, count,
    `level_counts[7]`, `service_counts: HashMap<ServiceIdx, u32>`, a
    per-second time histogram (a sparse `Vec<(u32 second_offset, u32 count, u32 errors)>`),
    and `location: Open | Sealed(Location)`.
  - **Search:** sealed chunks are sorted within themselves by
    `(time, seq)`. Chunks can overlap in time (late logs), so search merges
    the overlapping chunks with a heap, newest first. `search_logs_keyed`,
    `logs_around`, `log`, `logs_for_span` and `remove_*` work across chunks.
  - **Indexed filters:** a `LogFilter` is *indexed* when it sets only
    `since`, `until`, `levels` and `service`.
  - `Store::log_count(&self, f: &LogFilter) -> Option<u64>`: `Some` for an
    indexed filter. It works from the chunk metadata, decoding only the
    chunks that straddle the window edges. `None` otherwise.
  - `Store::logs_at(&self, f: &LogFilter, offset: u64, limit: usize) -> Vec<(LogId, LogRecord)>`:
    newest first. An indexed filter finds the chunk by prefix sums and
    decodes at most two. Any other filter scans and is documented as slow.
  - `log_histogram` uses the per-second metadata for indexed filters and
    decodes nothing; other filters scan as in M4.
  - **Cache:** the decoded chunk cache holds `CHUNK_CACHE` chunks.
- [ ] **Step 1: Write the failing tests.**
  - `chunk_seals_at_count_and_time`
  - `late_log_lands_in_time_order_across_chunks`
  - `log_count_indexed_matches_scan`: random filters over 200k logs, with
    `log_count` equal to a brute-force count
  - `logs_at_matches_paged_search`: for offsets 0, 1, 4,999, 65,535,
    65,536 and the last, `logs_at` equals the same position in a keyed
    paged walk
  - `log_count_none_for_body_filter`
  - `histogram_from_meta_equals_scan`
  - `chunk_cache_bounded`
- [ ] **Step 2: Watch them fail. Step 3: Implement. Step 4: Run** `cargo
  test -p loctel-store`, in both modes, then the full gate.
- [ ] **Step 5: Docs, then commit.** Message: `feat(store): log chunks with
  indexed counts, offsets and histogram`.

### Task 4: Disk cap, memory pressure, spill errors, engine and MCP plumbing

**Files:**
- Modify: `crates/loctel-store/src/store.rs`
- Modify: `crates/loctel-engine/src/lib.rs`. The app's engine config sets
  `spill_dir` to a `RunDir`, the cap comes from `Settings::disk_cap_bytes`,
  and `EngineStatus` gains three fields: `disk_bytes: u64`,
  `disk_cap_bytes: u64` and `spill_error: Option<String>`.
- Modify: `crates/loctel-app/src/{cli.rs, main.rs}`. Add
  `--no-spill`, which keeps everything in memory; the default is to spill,
  into the run dir.
- Modify: `crates/loctel-mcp/src/{dto.rs, tools.rs}`. `get_status` gains
  `disk_bytes`, `disk_cap_bytes` and `spill_error`.
- Test: `crates/loctel-store/tests/disk_cap.rs`, plus
  `crates/loctel-engine/tests/spill.rs`

**Interfaces:**
- Produces:
  - `Store::set_disk_cap(&self, bytes: u64)`: applies at once
  - `Store::disk_bytes(&self) -> u64`
  - `StoreStats` gains `spilled_traces`, `disk_bytes` and
    `spill_error: Option<String>`. It's a struct, so this is additive.
  - `EngineHandle::set_disk_cap(bytes)`, which `AppBackend::apply_settings`
    calls when the cap changes
  - `Settings::disk_cap_bytes` is applied at start
  - **A spill write failure:** spilling stops and `spill_error` is set. The
    store keeps data in memory up to `max_traces` and `max_log_records`.
    The next successful write clears the error after the operator frees
    space; retry once a minute.
- [ ] **Step 1: Write the failing tests.**
  - `cap_evicts_oldest_segments_whole`
  - `lowering_cap_evicts_now_and_keeps_stats_consistent`
  - `spill_write_failure_keeps_ingesting_and_reports`: point `spill_dir` at
    a directory made read-only after the first segment, or inject a
    failing writer through a test hook
  - `engine_status_reports_disk`
  - `engine_shutdown_deletes_run_dir`
  - `mcp_status_has_disk_fields`
- [ ] **Step 2: Watch them fail. Step 3: Implement. Step 4: Run the full
  gate.**
- [ ] **Step 5: Docs.** Update the storage sections of `docs/areas/store.md`
  and `docs/areas/mcp.md`. **Commit.** Message: `feat(store,engine): disk
  cap, spill errors and status`.

### Task 5: Search hot paths

**Files:**
- Modify: `crates/loctel-mcp/src/tools.rs`. `recent_failures` makes one
  pass over the index, newest first, and stops at its limits.
- Modify: `crates/loctel-store/src/store.rs`, if anything still sorts the
  whole trace set
- Test: `crates/loctel-mcp/tests/tools.rs` (extended), plus
  `crates/loctel-store/tests/spill.rs` (extended)

**Interfaces:**
- `search_traces`, `recent_failures` and `services` must not sort or clone
  every held trace per call. Assert it with a test hook that counts
  summaries built:
  - `recent_failures_builds_bounded_summaries`: 50,000 traces, 3 failed
    among the newest 100, and `limit 5` builds at most a few hundred
    summaries, not 50,000
  - `search_first_page_builds_at_most_limit_plus_one`
- [ ] **Steps 1–4: test first, implement, run the full gate. Step 5:
  Commit.** Message: `perf(store,mcp): bounded work on search and recent
  failures`.

### Task 6: The performance harness and the CI budget check

**Files:**
- Create: `crates/loctel-perf/` (`publish = false`; bin `loctel-perf`).
  Each budget in Global Constraints is a scenario with its measurement.
- Modify: `xtask/src/main.rs`. Add `cargo xtask perf [--only <id>] [--json PATH]`,
  which builds `loctel-perf` in release and runs it, then prints the
  table. It exits non-zero on a breach.
- Modify: `.github/workflows/ci.yml`. Add a job `perf` on `ubuntu-24.04`,
  in parallel with `gate`, that runs `bash script/setup-linux.sh`, then
  `cargo xtask perf --json perf.json`, and uploads `perf.json` as an
  artifact.
- Create: `.claude/skills/perf-budgets/SKILL.md`, covering how to run one
  budget, read the table, profile a breach (`perf` or `cargo flamegraph`
  under release), and the rule that budgets are only raised with a recorded
  decision, never to get green.
- Create: `docs/areas/testing.md`, a "Performance budgets" section with the
  table

**Interfaces:**
- `loctel-perf` scenarios build their data **in-process** through the
  store, or through OTLP for `ingest`. They use meshgen generators (add a
  `meshgen::bulk_logs(n, seed)` and `meshgen::bulk_traces(n_spans, seed)`
  that stream batches without holding everything) and `spill_dir` in a temp
  dir.
- `rss_10m` reads `/proc/self/status` (`VmRSS`) on Linux. On other OSes it
  reports "not measured".
- `first_frame` uses the UI crate's headless harness, in release.
- [ ] **Step 1: Write the failing test.** A unit test in `loctel-perf`,
  `budget_table_flags_breaches`: a fake measurement over its budget gives a
  breach row and exit code 1.
- [ ] **Step 2: Implement the scenarios. Step 3: Run** `cargo xtask perf`
  locally. Every budget must pass on this 4-CPU container. If one fails,
  **fix the code**, not the budget. Raising a budget needs a decision file
  with measurements, and is only acceptable for `first_frame` on
  software rendering.
- [ ] **Step 4: Lint.** Run `actionlint .github/workflows/*.yml`.
- [ ] **Step 5: Commit.** Message: `test(perf): performance budgets as a CI
  check`.

### Task 7: The Logs list at 10 million lines

**Files:**
- Modify: `crates/loctel-ui/src/app/logs_data.rs` and
  `crates/loctel-ui/src/views/logs_view.rs`
- Create: `crates/loctel-ui/src/model/window.rs`
- Modify: `crates/loctel-ui/src/views/trace_view.rs`. When the open trace
  is evicted, it says `This trace is no longer held (the disk cap removed
  it)` with a `Back` button.
- Test: `crates/loctel-ui/tests/logs_scale.rs`, plus snapshots

**Interfaces:**
- Produces:
  - `model::window::RowWindow`:
    - `new(total: u64, row_px: f32, viewport_px: f32)`
    - `top(&self) -> u64`
    - `scroll_px(&mut self, dy: f32)`: in rows, accumulating sub-row
      remainders
    - `jump_fraction(&mut self, f: f64)`, for the scrollbar
    - `visible(&self) -> Range<u64>`
    - `set_total(&mut self, total: u64, keep: Anchor)`, where `Anchor` is
      `Top`, `Row(LogId)` or `Fraction`
    - `thumb(&self) -> (f32 start_frac, f32 len_frac)`, with the thumb at
      least 24 px

    It's all pure and in u64 and f64, so there are no f32 precision limits.
  - **LogsModel, indexed filters** (no LogQL query, or only a stream
    selector with `service_name=` and/or level labels that maps to an
    indexed `LogFilter`):
    - uses `log_count` plus `logs_at` through a `RowWindow`
    - draws its own scrollbar (an 8 px track and a thumb from `thumb()`)
    - reads only the visible rows plus one viewport on each side
    - live tail keeps the top anchored while at the top, and keeps the row
      under the viewport's top line otherwise
  - **Any other query:** the M4 paging (newest 20,000) as before, with the
    footer.
  - The footer shows the total, such as `10,000,000 lines · newest first`.
- [ ] **Step 1: Write the failing tests.**
  - `row_window_math`: scrolling, fractions, the thumb, totals that grow
    with each anchor, and totals up to 10^10 without precision loss
  - `ten_million_scroll_to_middle_dark`: a spilling store with 10M bulk
    logs, built once in the test with a ManualClock and cached in a temp
    dir if slow. Jump to 0.5 and capture `logs_10m_middle_dark.png`, whose
    footer reads `10,000,000 lines · newest first` and whose rows are
    continuous.
  - `indexed_and_query_paths_switch`
  - `tail_keeps_anchor_when_scrolled`
  - `evicted_open_trace_shows_gone`
- [ ] **Step 2: Watch them fail. Step 3: Implement.** **View** the new
  PNGs. **Step 4: Run the full gate.** The 10M test must take under 60 s in
  debug. If it can't, build its store with fewer, longer chunks through a
  test-only bulk path, and say so in the decision log.
- [ ] **Step 5: Commit.** Message: `feat(ui): Logs list scrolls through
  ten million lines`.

### Task 8: Disk cap in Settings, disk tile in the popover, light theme contrast

**Files:**
- Modify: `crates/loctel-ui/src/model/{settings.rs, popover.rs}`,
  `crates/loctel-ui/src/views/{settings_view.rs, popover_view.rs}` and
  `crates/loctel-ui/src/theme.rs`
- Test: the existing settings and popover tests (extended), a new
  `crates/loctel-ui/tests/contrast.rs`, and regenerated references

**Interfaces:**
- `settings::Form` gains `disk_cap_gb: String`, validated 1–100 with the
  message `Enter a size from 1 to 100 GB`. Apply sets it, saves it and
  calls `set_disk_cap`. The Settings window gets a `Storage` section with
  the input and `Using 1.3 GB of 4 GB` under it.
- The popover's fourth tile becomes `1.3 GB` / `on disk, cap 4 GB`, which
  is the mockup's tile, replacing `log lines held`. With `spill_error`, the
  tile shows `Disk full` in `error_text`, and the pill shows `Port problem`
  only for ports.
- `format::bytes(u64) -> String`: `0 B`, `512 KB`, `1.3 GB`, `12 GB`, with
  one decimal under 10.
- **Contrast:**
  - `contrast::ratio(a: Rgba, b: Rgba) -> f64` (WCAG)
  - `tests/contrast.rs` asserts every pair in Global Constraints for
    `Theme::dark()` and `Theme::light()`
  - fix the failing tokens in `theme.rs` without changing their hue family,
    then regenerate every reference whose pixels change and **view each
    one**
- [ ] **Step 1: Write the failing tests:**
  - `disk_cap_validation`
  - `apply_sets_disk_cap`
  - `popover_disk_tile_and_disk_full`
  - `bytes_format`
  - `theme_contrast_dark` and `theme_contrast_light`
- [ ] **Step 2: Watch them fail. Step 3: Implement.** Regenerate and
  **view** the references. **Step 4: Run the full gate.**
- [ ] **Step 5: Commit.** Message: `feat(ui): disk cap in Settings, disk
  tile, light theme contrast`.

### Task 9: Docs, Mac checklist, roadmap done

**Files:**
- Modify: `docs/areas/{store.md, ui.md, testing.md}`,
  `docs/architecture.md`, `AGENTS.md` (the commands table gains
  `cargo xtask perf`, and the skills table `perf-budgets`),
  `docs/milestones/{roadmap.md, mac-checklist.md}` and
  `.claude/skills/run-and-screenshot/SKILL.md` (`--no-spill`)

- [ ] **Step 1: Docs.** Describe the storage architecture end to end, with
  an ASCII diagram, and the budgets table. In the roadmap, link the M5 plan
  and mark all milestones delivered, with Mac checks pending.
- [ ] **Step 2: The Mac checklist.** Add a **Milestone 5** section, keeping
  the others:
  1. **Startup:** quit loctel, then open it. The menu bar icon appears at
     once. Open loctel and the window appears in under half a second (by
     feel).
  2. **Big trace:** send `meshgen --scenario huge-trace`, open it, and
     scroll top to bottom fast. It stays smooth, with no blank rows. Wait
     a minute (it spills) and open it again: it opens just as fast.
  3. **Many logs:** run `meshgen --scenario log-flood --size 2000000`, then
     drag the Logs scrollbar from top to bottom and back. Rows appear
     without stalls. Type a LogQL query, and results appear within a
     second.
  4. **Disk:**
     - The popover shows the disk tile growing as data arrives.
     - In Settings, set the cap to 1 GB and Apply. The tile drops under
       1 GB within seconds.
     - In Finder, open `$TMPDIR/loctel/` (Go to Folder, then `$TMPDIR`):
       one `run-…` folder with `.seg` files.
     - Quit loctel. The folder is gone.
  5. **Memory:** in Activity Monitor, with a steady meshgen stream for 10
     minutes, loctel's memory levels off instead of climbing.
  6. **Light theme:** switch to Light and look over every screen. Text is
     readable everywhere; report anything faint.
- [ ] **Step 3: Run the full gate, `script/check-macos.sh` and `cargo xtask
  perf`.**
- [ ] **Step 4: Commit.** Message: `docs: M5 storage, budgets and Mac
  checklist`.

---

## After the last task (M5 ship)

1. **Whole-branch review** against the spec (§5, §10.3, §4, §9.2, §9.3) and
   this plan. It covers the both-modes test run, a `cargo xtask perf` run
   and the changed PNGs. Fix what it finds.
2. **Open the PR** from `claude/m5-scale`. Get the Linux CI `gate` and
   `perf` jobs green.
3. **Squash-merge** with the title
   `feat: milestone 5, spill to disk, disk cap and performance budgets`.
   Watch the release until it's published with the `.dmg` and the M1–M5
   checklist sections.
4. **Tell the user** that every milestone has shipped, and that the Mac
   checklists are what remains.
