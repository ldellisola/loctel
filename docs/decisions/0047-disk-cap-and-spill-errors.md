# 0047: The disk cap, a failing disk, and who owns the run directory

Date: 2026-10-22
Status: accepted

Milestone 5, Task 4. The plan fixes the interfaces (`Store::set_disk_cap`,
`Store::disk_bytes`, the three new fields of `StoreStats`, `EngineHandle::set_disk_cap`,
the three of `EngineStatus` and of the MCP `get_status`, `--no-spill`) and the semantics
("delete the oldest whole segment file, its traces and logs with it, until under the cap";
"on a spill write failure, spilling stops and `spill_error` is set ... retry once a
minute"). This file records what it left open, and the three places where the plan and
the code it meets could not both be kept to the letter.

## Context

Tasks 1 to 3 built the files, spilled traces to them and held logs in chunks that are
written to them. Nothing yet kept the files to a size, and a write that failed was retried
at every step, which on a full volume means an encode and a failed write on every ingest
call. The status could not say any of it.

## Decisions

### The cap

- **Where it is enforced.** `Inner::enforce_disk_cap`, which compares one counter
  (`SegmentSet` keeps the total as it appends and deletes) with the cap, runs at the end of
  `ingest_spans`, `ingest_logs`, `remove_trace`, `remove_spans`, after the spill step that
  precedes the reads, and in `set_disk_cap`. Those are all the places a record is written,
  so the files are under the cap whenever a call returns. The cap itself is an atomic on the
  store (`set_disk_cap` may be called from the UI thread while ingest holds the lock).
- **Oldest first, by number.** Segment numbers count up over traces and logs and are never
  reused, so the lowest number is the oldest file. `SegmentSet::plan_eviction` takes files
  in that order until the projected total is at or under the cap, and nothing more. A file
  of nothing but dead records is **not** taken ahead of an older live one: the plan says
  "the oldest", the tests of Task 2 look at dead bytes staying until their segment goes,
  and the oldest data is what a cap should lose first anyway.
- **The file that is being written** is a segment like any other. If it is the oldest, it
  goes, its writer with it, and the next append starts a new file. So a cap smaller than a
  record keeps nothing on disk and the store still ingests (`a_cap_smaller_than_a_record...`).
  A cap of 0 is not special-cased: the Settings form allows 1 to 100 GB, and the store has no
  reason to guess what a 0 means.
- **What is forgotten.** Every cold trace with a record in a doomed file is forgotten by
  the same function that eviction by `max_traces` uses (`forget_trace`), so every counter
  that tracks held data follows without a second code path. Every sealed chunk in the file
  is dropped by `LogStore::drop_in_segments`, which **reads the chunk first** to forget the
  dedup keys of its logs, as `evict_sealed` does for eviction by count. The alternative, not
  forgetting them, leaks 8 bytes of dedup set per log that ever went and makes a retry of an
  evicted log a silent drop. The cost is the decode of every chunk in the file, under the
  write lock, once per file deleted (see the store document for the numbers).
- **Deleting works on a full volume,** so the cap keeps working when writes fail.

### A failing disk

- **Writes are suspended, not just retried.** After a failed write `Inner::spill_failed` arms
  `retry_at = now + spill_retry` and calls `SegmentSet::suspend`, after which every append
  (and `check_writable`, which the log chunks call **before** encoding a chunk) fails at
  once with the original reason and counts as no attempt. The first write path that runs
  after the minute (`retry_if_due`: the spill step, `ingest_logs`, `remove_*`) resumes
  writes. A failure while suspended keeps the time of the first: the retry is once a minute,
  not a minute after the last failure to give up. The plan says "retry once a minute"; this
  is how it is kept without a timer thread, since the store has none.
- **`StoreConfig::spill_retry`** (60 s) is a config field, not a constant, because the tests
  of Task 2 and 3 (`a_failing_write_leaves_traces_in_memory_and_is_retried`,
  `a_failed_chunk_write_keeps_the_logs_in_memory`) retry within seconds of the clock and must
  pass unchanged. `StoreConfig::spilling_for_tests` sets it to 0 (retry at the next call);
  `disk_cap.rs` sets 60 s to test the minute.
- **A chunk of logs whose write failed stays in memory for good,** also after the disk
  works again. `a_failed_chunk_write_keeps_the_logs_in_memory` (Task 3, which must pass
  unchanged) asserts that after recovery only the two chunks sealed since are on disk. A
  re-spill would be easy and is the better behaviour, but that test says otherwise, and the
  memory it holds is bounded by `max_log_records`. Traces are different: they stay hot, are
  still in the spill step's queue, and go when writes work.
- **The error is cleared by a write that works,** not by a probe (a tiny file created and
  deleted at each retry). A store with nothing waiting to be written keeps the error until it
  has something, which is soon in a store that receives data, and `clear` forgets it. A probe
  would also lie when it succeeds and the real write does not (one block free, a chunk to
  write).
- **Hooks.** `Store::inject_write_failure(bool)` makes every append fail with the OS error of
  a full volume (`ENOSPC`, so the message is the real one), and `failed_write_attempts()`
  counts the appends that tried the disk. The plan allows a read-only directory or a test
  hook; the first does nothing for the root user the tests may run as, and cannot fail a
  write to an open file.

### Where the plan and the code disagreed

1. **`StoreStats` is no longer `Copy`, and its `==` compares five counters.** The plan calls
   the change additive, but `spill_error: Option<String>` ends `Copy`; nothing in the
   workspace needed it. More important: seven tests of Task 2 (`spill.rs`) assert
   `pair.spill.stats() == pair.memory.stats()`, "same counts" of a store that spills and one
   that does not, fed the same data. With `spilled_traces` and `disk_bytes` in a derived
   `PartialEq` they can never pass, and the Global Constraints say they must pass unchanged
   in their assertions. So `PartialEq` is written by hand over `traces`, `spans`, `logs` and
   the two `*_received` totals, with the reason in the type's documentation. The new fields
   are asserted by name wherever they matter. The alternative was changing those seven
   assertions to compare a projection; it would have said the same but is exactly the edit the
   rule forbids.
2. **The engine, not the app, makes the run directory.** The plan says "the app's engine
   config sets `spill_dir` to a `RunDir`". But `engine_shutdown_deletes_run_dir` is an
   engine test and needs the engine to own the directory's life, and the engine is also where
   `Settings::disk_cap_bytes` is applied. So `EngineConfig` has a new field, `spill_base:
   Option<PathBuf>`: `Some(base)` makes `base/run-<pid>-<start>/` through `RunDir::create`
   (which deletes the dead runs' directories), puts its path in `store.spill_dir` and keeps the
   `RunDir` in the engine's running state, deleted at shutdown (and when the last handle
   drops, after the receivers have stopped). `None` is the default and leaves `store.spill_dir`
   alone, so the tests that want a store in memory, or one that spills to a directory of
   their own, still get it. The app passes `default_spill_base()` (`$TMPDIR/loctel`), unless
   `--no-spill`. The config of the store in `EngineConfig::store` stays equal to the default,
   so `no_flags_leave_settings_and_store_defaults_alone` is unchanged.
3. **`Settings::disk_cap_bytes` wins over `store.disk_cap_bytes`** at `Engine::start`: both
   are 4 GiB by default, and the settings are what the user edits.

### What else follows

- A run directory that cannot be made (the temp directory is read-only, a file is where
  `loctel/` should be) does not stop loctel. It runs in memory, prints one line on stderr and
  says so in `EngineStatus::spill_error`: a loctel that does not start is worse than one that
  keeps its data in memory.
- `EngineStatus` repeats `disk_bytes` and `spill_error` that are in `stats`, as the plan
  lists them, and its `spill_error` also carries the run directory's. `EngineStatus` still
  derives `PartialEq` with all the fields, so the UI redraws when only the disk changes.
- The MCP `get_status` has the three keys always, `spill_error` as `null` when all is well
  (the rule of the other nullable keys of that tool), cut at 2,000 characters.
- `AppBackend::apply_settings` calls `EngineHandle::set_disk_cap` after the file is saved (a
  file that cannot be written moves nothing). The Settings form gets the field in Task 8.

## Consequences

- `StoreStats` is not `Copy`: a caller that copied it clones it (none did in the workspace).
- Comparing two `StoreStats` with `==` ignores where the data is held. Whoever wants the
  disk fields compares them by name.
- Every ingest call on a spilling store now does one extra integer comparison under the lock
  it already holds.
- A cap that is lowered while loctel runs does its deletes (and the chunk reads for the dedup
  keys) inside the `set_disk_cap` call, which the Settings window makes on the UI thread: Apply
  waits for the store's lock if ingest or a long read holds it, and for the deletes themselves.
- A long outage on a busy machine holds up to `max_traces` traces and `max_log_records`
  logs in memory (the sizes of M1 to M4), and drops the oldest of them as before: nothing
  else grows.
