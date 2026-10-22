# 0046: Log chunks, and counts from their metadata

Date: 2026-10-22
Status: accepted

Milestone 5, Task 3. The plan fixes the shape (an open chunk and sealed chunks,
`ChunkMeta`, indexed filters, `Store::log_count` and `Store::logs_at`, a histogram
from per-second metadata, a cache of 8 decoded chunks, chunks of 65,536 logs or
10 s) and names. This file records what it left open, the places where the plan
had to be read, and the numbers that Tasks 6 and 7 start from.

## Context

Task 1 built the segment files and Task 2 moved traces into them. Logs were still
one `BTreeMap` of every log held. Task 3 moves them into chunks behind a public
API that must not change: every test of the crate runs unchanged in both modes,
and the plan's new calls (`log_count`, `logs_at`) are additions.

## Decisions

### Chunks and when they seal

- The **open chunk** is the old log store, kept as it was (a `BTreeMap` by
  `LogId`, an index of the logs of each span and each trace), with a `ChunkMeta`
  kept up to date beside it. A **sealed chunk** is its logs sorted by `LogId` in one
  record: a record of a logs segment (the format of decision 0044), or an
  `Arc<Vec<_>>` in memory when the store has no segment files or the write failed.
  Sealed chunks have no span or trace index.
- **Seal at 65,536 logs (checked after each insert, so a batch may seal several
  chunks) or 10 s on the store's clock since the first log of the chunk.** The age
  is checked at the start of every `ingest_logs` and by the spill step. That step
  used to return at once when the store has no segment files; it now seals logs
  first, and the `spill_dir`-less early return in `spill_if_due` is gone, so a store
  in memory seals by age too (the chunk is then an immutable `Arc` in memory). One
  call a second at most, as before.
- **The two numbers are `StoreConfig` fields** (`log_chunk_max`, `log_chunk_age`),
  not constants, with the plan's values as defaults, and
  **`spilling_for_tests` sets `log_chunk_max` to 7** (the plan lists three changes to
  the test config; this is a fourth). The plan wants the existing log tests to run
  in both modes and says the test config "exercises every cold path". With chunks of
  65,536 none of them would ever leave the open chunk. At 7, every log test in the
  crate meets sealed chunks, chunks that overlap (any test with a late log) and
  trimmed chunks, against assertions that were written for one `BTreeMap`; the suite
  passed unchanged on the first run, which is the check that the old API holds. 7 is
  odd and prime, so that the sizes the tests use (2, 3, 5, 10) do not line up with
  where a chunk seals.
- **A late log goes to the open chunk** whatever its time (the plan says chunks can
  overlap). It is not placed in the sealed chunk it belongs to, which would mean
  writing that chunk again for every late log.

### What the plan's `ChunkMeta` was missing

The plan lists `level_counts[7]`, `service_counts` and a per-second histogram of
`(second_offset, count, errors)`. With those alone, a filter that names both levels
and a service (the Logs screen's usual one, and the filter of the plan's `logs_count`
budget) cannot be counted from the metadata of a chunk: the two marginals do not
give the joint count. So:

- **The per-second entries are cells of `(second, service, level)` with a count**
  (`Vec<Cell>`, sorted). The plan's `count` and `errors` of a second are sums over
  them. `level_counts` and `service_counts` are kept as well (the fast path for a
  filter that names one of them, and for passing a chunk over). The second is a
  `u64`: no offset to overflow for a log with a time of year 2200, or a replay of
  an old file into a new chunk.
- **`first` and `last`** (the chunk's range of ids) and **`traces`** (sorted, for a
  sealed chunk) are in the metadata as well. The range is what the merge and the
  position search order chunks by; `traces` is what lets `logs_for_span`, search by
  `trace_id` and the removals skip chunks, in place of the per-trace index the open
  chunk has (an index of every log of every trace would be a `LogId` of 16 bytes
  and a map entry per log: gigabytes at ten million).
- **`ServiceIdx`** is a `u32` per service name, assigned when first seen and never
  forgotten (the services of a run are a few dozen). A log's service is a string
  compare against the filter's name when a chunk is read, and a number when only the
  metadata is.

### Exact answers, and what that costs

The plan says the histogram "uses the per-second metadata for indexed filters and
decodes nothing", and tests that it "equals scan". Both can only hold when no bucket
edge falls inside a second that has logs, because a second is the finest the
metadata knows. The existing tests (written for exact times, in milliseconds)
and the API (frozen) rule out an answer that differs from a scan's, so:

- **Every answer equals a scan's.** The metadata is used wherever it is exact, and
  the chunk is read where it is not: a chunk that a window edge cuts through (for
  `log_count`, `logs_at` and the histogram), and a chunk that a histogram bucket edge
  falls inside a second of. **A window and a bucket count that make every bucket a
  whole number of seconds from a whole second read nothing**; any other window reads up
  to one chunk per bucket edge. The cost of an odd window is the cost of reading
  47 chunks (1.7 s at ten million logs, below), and the Logs screen of Task 7 should
  snap the window it asks for to whole seconds.
- The alternative is to count a second's logs in the bucket of the second's start, with no
  reads. That differs from a scan by the logs of the one second at each bucket edge, and
  from the old answer for any window below a second, which the M4 tests pin
  (`histogram_edges_and_empty_inputs`). It was not taken, and nothing stops a later
  decision from taking it for chunks of 65,536 logs only.

### `logs_at`

The plan says to find the chunk by prefix sums and decode at most two. That holds when
chunks do not overlap in time. With a batch exporter they do, always, by a few seconds: so
prefix sums over chunks do not locate a position. What is done instead:

- Let `at_or_above(b)` be the count of matching logs with id `b` or above, from the
  metadata (chunks inside the window), plus reads of the chunks `b` or the window falls
  inside. The ids at which it can change are the first id of each chunk and the one after
  its last; **a binary search over those** finds the last one with no more than `offset`
  matching logs at or above it. The walk (the merge) starts just below it and steps over
  the remaining matches.
- Without overlap, no chunk has any of those ids inside it: **the search reads nothing and
  the walk reads the chunk the position is in, and the next one if the rows run over: at
  most two**, as the plan says. With overlap, it also reads the chunks that overlap the
  ids it compares with, a few. Offset 0 skips the search.
- A filter that is not indexed gets a plain walk, as the plan says ("scans, documented as
  slow").

### The merge

Search, `logs_around`, `scan_logs`, `logs_at` and the histogram of a filter that is not
indexed all walk **one `Cursor`**: a heap with an entry per chunk that can have a log in
range, keyed by the chunk's next log (before it is read, by the end of its range that the
walk starts from), either direction. A chunk is read when its entry comes to the top.
The plan says "merges the overlapping chunks with a heap, newest first"; this reads
only the ones the walk reaches, and does not look at which overlap. The cursor lends
each log (a method of its own, not `Iterator`) so that a search clones only what it
returns.

A search skips, without reading, the chunks the metadata rules out: a service the chunk has
no log of (the filter's, and the exact `service_name` of a LogQL selector), no log at a
level the filter keeps (`levels` and `min_severity`), none of the `trace_id`.

### Eviction and removal

- **`max_log_records` keeps its meaning exactly**: past it the oldest logs by id go, after
  the batch. The tests that hold it, among them a random run at five chunk sizes with
  caps from 60 to 400, compare every read with a plain model.
- A sealed chunk that loses some of its oldest logs is **trimmed from the front**: its
  `skip` counts them out and its metadata is corrected from the logs it reads (once, then
  from the cache). It is not written again. The alternative was to write it again each time
  (a batch evicts at the cap, so every batch would write 65,536 logs); a chunk that is
  trimmed to nothing is dropped, and a chunk that cannot be read is dropped whole.
- **Dedup keys stay one set for the whole store** (a `HashSet<u64>` of the keys, hashed with
  themselves, since they are hashes: 9 bytes a slot, about 150 MB at ten million). A
  per-chunk set was the other way, but a retry can arrive for any time, and the check would
  have to ask every chunk whose range holds it. The keys of evicted and removed logs are
  recomputed from the logs, which is why eviction reads the chunk it trims.
- **A removal from a sealed chunk writes the chunk again** without the logs (the old record
  dead), because a record cannot be changed in place. It is per chunk that has the trace
  in its `traces` list; the rules remove what has just arrived, in the open chunk, where
  removal is what it was.

### Cache, damage, failed writes

- The cache is 8 decoded chunks, keyed by the position of the record, as for traces, behind
  its own mutex because reads take the store's read lock. Reads that look at many chunks once
  (`scan_logs`, removal) read through it and do not fill it.
- A chunk whose record cannot be read stays in the list: reads skip it, and the counts still
  hold its logs until eviction drops it. Making the counts follow would mean a read that
  writes. It is a crash-mid-write case that the files of a run, deleted at the end of it, are
  not meant to see often.
- **A failed write keeps the chunk in memory** and sets `spill_error` (the same field as for
  traces; a success of either kind clears it). Moving the kept chunks to disk when space
  comes back, the once-a-minute retry, and the memory limit that goes with it are Task 4's.

## What it costs (the numbers Task 6 and 7 start from)

One-off, on this container (4 vCPU, release): 10,000,000 logs, 8 services, 70-byte bodies, a
trace and a span ID on each; 152 sealed chunks; segment files on.

| | |
|---|---|
| ingest | 530,000 logs/s, one thread; 230 MB resident after |
| `log_count`, levels and service | 0.17 ms (budget 50 ms) |
| `log_count` with a window edge in two chunks | 73 ms |
| `logs_at`, 50 rows at a random offset | p50 32 ms, p95 about 40 ms (budget p95 20 ms) |
| `log_histogram`, 48 buckets of whole seconds | 0.24 ms (budget 100 ms) |
| `log_histogram`, 48 buckets with edges inside seconds | 1.7 s |
| first page of a LogQL query for one service | 0.8 ms (budget 1 s) |

**Reading a chunk is about 33 ms** (65,536 logs, 8.4 MB decoded: 6 ms of zstd, 20 ms of postcard,
12 ms turning the wire form into `LogRecord`s) and is the whole of the `logs_at` time, which
misses its budget by a factor of two. The plan's budget needs one of: a faster
`decode_logs` (one pass into the records instead of two, hand-written for the log blob); a
record of **independent blocks** (say 1,024 logs each, with the first id of each in the
metadata), so that a position reads one block, not 65,536 logs; or both. Neither is done
here: both change the segment format of Task 1, and the first measurement of a budget is
Task 6's. `logs_at` takes its rows through `LogStore::decoded`, which returns a whole chunk;
a block-wise reader replaces that one function and the `Chunk::window` that follows it.

## Consequences

- Memory is the open chunk, the metadata of every sealed chunk (a few KB each plus its
  traces, 16 bytes per trace in it), the dedup keys (about 15 bytes a log) and at most 8
  decoded chunks. At ten million logs: 230 MB resident after ingest, and 564 MB after a
  round of queries that filled the cache (so a decoded chunk of 65,536 logs is about 40 MB).
- A query that scans (`contains`, a LogQL line filter, `span_id`) over a lot of logs reads
  every chunk: 33 ms a chunk, about 5 s at ten million. The Logs screen's M4 paging
  (the newest 20,000 matches) stops as soon as it has them; a query with no match reads all.
- The Logs screen's histogram needs a window in whole seconds to be free; Task 7 should snap
  it (the bucket width `ceil(range / 48)` seconds, `since` a whole second).
- `StoreConfig` gained two fields. Struct literals with `..StoreConfig::default()` (all of
  the workspace's) are unaffected.
