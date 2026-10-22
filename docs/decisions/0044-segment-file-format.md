# 0044: Segment file format, the model's binary serde shapes and the run directory

Date: 2026-10-22
Status: accepted

Milestone 5, Task 1. The plan names the record format and the interfaces; this
file records what it left open and the three places where the code had to adapt
to what was already there.

## Context

Milestone 5 moves completed traces and sealed log chunks out of memory into
temporary segment files (design §5). Task 1 builds the pieces the store will
spill through: the file format, a writer and a reader, the blobs of spans and
logs, and the per-run directory. The store does not use them yet.

## Decisions

### The record and the files

- **Record:** `[u32 len][u32 crc32 of the compressed bytes][zstd(postcard(payload))]`,
  little-endian, as the plan says. **No file header, footer or version.**
  Segment files live in the run directory and are deleted when the run ends or
  by the next start after a crash; no version of loctel ever reads a file
  another one wrote, so a version number would guard against nothing. Change
  the format freely, and writer and reader together.
- **`Location.len` is the length of the compressed body**, the number in the
  record's own header. A record takes `HEADER_BYTES` (8) more on disk, and
  `Location::disk_bytes` says so, which is what the disk cap will count. `read`
  refuses a `Location` whose `len` is not the header's (`Decode`): it is a
  location for something else.
- **`Writer::append` is one write of a whole record**, at an explicit offset
  (`pwrite`), with no buffer between. A reader opened at any time sees whole
  records, and a failed write is taken back by cutting the file to the last good
  record, so the file always ends on a boundary and the next `Location` is right.
  If the cut-back fails too, the writer refuses further appends instead of
  handing out locations nobody can read.
- **`seal` does not sync.** Segments never outlive the run, so durability buys
  nothing, and an fsync of a 64 MiB segment would stall ingest while the store
  holds its lock. `seal` consumes the writer, so nothing can be appended after.
- **Reading is by memory map** (`memmap2`) of sealed segments. A file that is
  empty is opened as having no records (a zero-length file cannot be mapped).
- **`iter` after damage.** A `Checksum` or `Decode` error is an item and the walk
  goes on, because the header's length is intact. A `Truncated` record ends the
  walk, because nothing can be found after it. The first problem per segment is
  printed once on stderr (`loctel: <file>: ...`, the way the rest of the app
  reports), and counted in `Reader::problems`.
- **One zstd compression context per thread**, reused by every writer, because
  making one costs more than compressing a small trace. `zstd` is built without
  its default features (no legacy formats, no dictionary builder). Records are
  decompressed into one allocation of the size the frame states.
- **Pins:** `postcard` 1.1.3 (`use-std` only), `zstd` 0.14.3, `memmap2` 0.9.12,
  `crc32fast` 1.5.3 and, for `kill(pid, 0)`, `libc` 0.2.191 (already in the
  lockfile).

### The blobs

- A **trace blob** is a postcard struct of a table of resources and the spans,
  each span naming its resource by index. A **log chunk blob** is the same for
  `LogRecordWithId { id: LogId, record: LogRecord }` (the plan names the type
  and not its fields). The id is written out (time and arrival number), not
  derived again from the record, so a chunk comes back with exactly the ids it
  went out with.
- **Resources are interned by value**, not by `Arc` address alone. The spans of
  one service reach the store in many batches, each with its own `Arc` of an
  equal resource; interning by address would write the resource once per batch.
  Addresses are only a cache in front of the value lookup (the key is the
  resource's postcard bytes).
- Encoding borrows from the spans (`Cow`), so a 65,536-record chunk is not
  copied on the way to bytes. Decoding gives every span of a resource one shared
  `Arc` again.
- A blob with bytes left after it is a `Decode` error, so a log blob read as a
  trace blob cannot pass as a short one.

### The model's `Deserialize`, and why it depends on the format

The plan says to add `Deserialize` next to every `Serialize`, "round-tripping
exactly". The existing `Serialize` is plain JSON (ids and bytes as hex strings,
attribute values untagged, attributes as objects), which postcard cannot read
back: postcard is not self-describing, so a value cannot be asked what it is,
and bytes and strings look the same in JSON anyway. Changing the JSON would
change what the MCP tools return, and the plan forbids that. So the impls look
at `is_human_readable`:

- **JSON (human readable):** the shapes are unchanged. Reading goes by shape.
  The gaps are JSON's own, and each is documented on `AttrValue`: a byte value
  was written as a hex string and reads back as a string; `null` (what
  `serde_json` writes for NaN and the infinities) reads back as a NaN double;
  an integer above `i64::MAX` reads as a double.
- **Binary (postcard):** `TraceId` and `SpanId` are their raw bytes (16 and 8
  instead of a 32 and 16 character string), `AttrValue` is a tagged enum (serde's
  `remote` derive on a private mirror), and attributes are a map in order. Every
  type round-trips exactly, bit for bit for doubles, repeated attribute keys
  included.

`crates/loctel-model/tests/serde_roundtrip.rs` pins both.

### The run directory

`RunDir::create(base)` is as the plan says (`run-<pid>-<start_unix_secs>`,
stale `run-<pid>-*` siblings deleted when the pid is not alive, deleted on
`Drop`), with these choices:

- `base` is the directory that holds the run directories (the app passes
  `std::env::temp_dir()/loctel`), and is made when missing. Directories are
  created with mode `0700`: they hold the user's traces and logs, and the system
  temp directory is shared on Linux.
- Only a real directory with a run's name is deleted. A file, a symbolic link
  (never followed) or an odd pid (not a number, 0, too big) is left alone, and a
  pid is judged by `kill(pid, 0)` with `EPERM` meaning alive.
- A name already taken (two runs of one process in one second, which tests do)
  moves the start time on by a second and tries again. Two runs of one process
  share a live pid, so neither deletes the other.
- Not solved, on purpose: a pid reused by an unrelated process keeps a stale
  directory until that process ends, and a program that is always the same pid
  (pid 1 in a container) never sees its own leftovers as stale. Both leave files
  in the temp directory and nothing worse.

## Where the plan met what was already there

- **The macOS check could not build `zstd`.** `script/check-macos.sh` runs
  `cargo clippy --target aarch64-apple-darwin`, and `zstd-sys`'s build script
  compiles C for the target, which a Linux machine cannot (no Apple SDK
  headers, and its `cc` does not know `-arch`). Same situation as decision 0009
  and the same answer, since a check never links or runs anything: the script
  sets `CC_aarch64_apple_darwin` to `script/stub-cc.sh`, which writes an empty
  object for every file it is asked to compile. cc-rs does not recognise the
  stub and uses MSVC-style flags, so the stub understands `-o <file>` and
  `-Fo<file>`. On a Mac, and for the Linux build, the real compiler is used. The
  check proves the Rust type-checks, as before; it still says nothing about how
  zstd behaves on a Mac, which stays unverified until the M5 Mac check (the C is
  the vendored upstream library, built by the release job on `macos-26`).
- **`LogRecordWithId` did not exist.** The plan uses it in `encode_logs` without
  defining it; it is defined in `segment.rs` (`{ id: LogId, record: LogRecord }`).
- **`#[doc(hidden)] pub` modules.** `segment` and `run_dir` are public only for
  the integration tests (the plan's own allowance), so the crate's real API is
  unchanged.

## Consequences

- Later tasks append blobs with `Writer::append` and find them with the
  `Location` it returns; a dead blob (a trace brought back hot) is still bytes
  in its segment until the segment is evicted.
- A damaged record never stops a walk of a segment, so loading and eviction
  need no special case beyond "skip the `Err` item".
- Tests: `crates/loctel-store/tests/segment.rs` (including
  `truncated_record_is_skipped`, the review-focus test for a crash mid-write)
  and `crates/loctel-model/tests/serde_roundtrip.rs`.
