# 0050: A chunk of logs on disk is a run of blocks

Date: 2026-10-22
Status: accepted. Changes decision 0044 (the record of a chunk of logs) and the
`logs_at` of decision 0046.

Milestone 5, Task 6. The `logs_at` budget (`logs_at(offset, 50)` at 100 random offsets
over ten million logs, p95 under 20 ms) failed on its first measurement, as decision 0046
had said it would. The plan says: fix the code, not the budget.

## What was measured

`cargo xtask perf`, release, 10,000,000 logs (the bulk generator of
`loctel-meshgen`), 152 sealed chunks of 65,536 on disk:

| | p50 | p95 |
|---|---|---|
| `logs_at`, 50 rows, a random position | 165 ms | 211 ms |

with three to eight chunk decodes in a call (a decode is 35 to 40 ms: zstd, postcard and
turning the wire form into `LogRecord`s) and a cache of eight chunks that a random
position never finds its chunk in. Two things are in it:

1. **The search read chunks.** Decision 0046's binary search over the ids at which a
   count can change evaluates `at_or_above(id)` for about nine ids, and every id that
   falls inside a chunk that overlaps its neighbour reads that chunk to count what is
   above it. Batches of an exporter always straddle the point at which a chunk seals,
   so chunks always overlap by a batch's span of time (20 ms here), and every bound is
   in an overlap: 3 to 8 chunk reads to find where the position is.
2. **One read is already twice the budget.** Even with a search that reads only the chunk
   the position is in, 35 ms for 65,536 logs is not under 20 ms, and the walk then steps
   over up to a chunk's worth of logs to reach the row.

The two ways decision 0046 listed were a faster decode or a record of independent blocks.
A decode twice as fast would still leave a single chunk read at the budget, and the cost
grows with the size of the chunk; blocks make the cost independent of it.

## Decision

- **A chunk is written as a record of blocks** (`Writer::append_blocks`, `Reader::read_block`
  and `read_blocks`): `[u32 len][u32 crc32 of the body][ [u32 n][n u32 lengths][n zstd
  frames] ]`. Each block is an `encode_logs` blob of `StoreConfig::log_block_rows` logs (1,024;
  the test config makes it 3) compressed on its own, so one block is read without the
  other 63. The record header and its checksum are the record's and are checked on every
  read of any block (about 0.1 ms for a chunk). `Writer::append` and `Reader::read` are
  unchanged: traces are still one zstd frame, and a record of blocks read with `read` is a
  decode error. Segment sizes grow by about 6% (287 to 305 MB for ten million logs: blocks of
  1,024 compress a little worse than one of 65,536).
- **Every block has its metadata in memory** (`BlockMeta`): its first and last id, and the
  count of its logs per service and level (at most 5 x 7 cells for the mesh). A chunk keeps
  them in order beside its own metadata: 10,000 blocks at ten million logs, about 3 MB.
- **`logs_at` for an indexed filter works on units**: the blocks of a chunk written as
  blocks, and every other chunk whole (the open one, one in memory, one that lost its oldest
  logs, which is read as one run again because its first block is no longer the one that was
  written). Units whose ids overlap form a **group**; every log of a group is newer than every
  log of the groups after it. The groups are counted from the newest, from the metadata
  (`count_matching` of the unit), which skips the position's `offset` without reading a log
  until the group the position is in; that group's units are read (a block or two: or the
  overlapping blocks of two chunks), the matching ids sorted, and the rows for the ids that are
  returned are cloned in a second pass. A unit that a window edge cuts through is read to
  be counted, once per edge: the one thing a window costs.
- **A cache of 32 decoded blocks** (`BLOCK_CACHE`) beside the cache of 8 decoded chunks, so
  scrolling a screenful (one or two blocks) decodes nothing after the first read. The
  chunk cache and `LogChunkInfo::decodes` are what they were (a read of a whole chunk, now a
  read of its blocks one after the other); the blocks read singly are `block_decodes`, and
  `blocks` is how many blocks the chunks on disk are made of. Every existing test of the crate
  passes with its assertions unchanged, in both modes, and the spilling mode now runs them
  over blocks of 3 logs, which meet a group of overlapping blocks all the time.
- **Everything else still reads whole chunks**: `log_count` at a window edge, the histogram,
  search, `Store::log`, `logs_around`, removal and eviction call `decoded()` as before. The
  Logs list of Task 7 is `log_count` and `logs_at`; the other reads are not in a budget, and
  the existing tests pin them (for example, a window with two edges decodes two chunks).

## What it costs now

`cargo xtask perf`, same data: `logs_at` p50 2.8 ms and p95 4.9 ms (budget 20 ms). Of that,
about 1.5 ms is making the units of ten thousand blocks and sorting them by their newest id
on every call, which a cache of the order (rebuilt when a chunk seals or goes) would remove;
nothing needs it yet. A selective filter reads more blocks for the same 50 rows (one in
seventeen logs passing is some 17 blocks), none of them a block that has no log that passes.

## Not done

- `Store::log(id)`, `logs_around` and the window edges of `log_count` could read a block
  and not a chunk (the metadata has the ids of each block). They are not in a budget; the
  existing tests pin the chunk-level reads (`log_count_decodes_only_the_chunks_at_the_edges
  _of_the_window`), so changing them is a change to those assertions, not an addition.
- A record of blocks has no checksum of its own per block: every read of a block checks
  the record's. Reading a chunk's 63 other blocks' bytes for the checksum is what the
  mmap has resident anyway; a per-block checksum is a change to a record that a Mac with
  a cold file cache would profit from.
