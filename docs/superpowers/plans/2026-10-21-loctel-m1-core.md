# loctel Milestone 1 (headless core, MCP, menu bar shell, releases): Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the first installable loctel. It is a menu bar app that
receives OTLP traces and logs, assembles them, works out which service
failed, and answers AI agents over MCP. Every merge to main publishes an
unsigned `.dmg` release. Agents can build, test and screenshot it on Linux.

**Architecture:** A Cargo workspace.

- **Pure-Rust core:** `loctel-model`, `loctel-store`, `loctel-ingest`,
  `loctel-mcp` and `loctel-engine`. It builds and tests on Linux with no
  graphics.
- **Thin GPUI layer:** `loctel-ui`, tested by headless pixel-exact
  screenshots.
- **macOS integration crate:** `loctel-macos`, with the status item and
  login item.
- **Binary:** `loctel-app`.
- **Test tooling:** `loctel-meshgen` (a fake microservice mesh) feeds
  everything, and `loctel-e2e` proves it end to end through real OTLP and a
  real MCP client.

**Tech Stack:**

- Rust 1.98.1
- tokio
- tonic 0.14 and axum 0.8 (OTLP)
- opentelemetry-proto 0.33.1
- rmcp 3.6 (MCP spec 2026-07-28, Streamable HTTP)
- GPUI via `gpui-pre`, `gpui-pre-platform` and `gpui-pre-wgpu` =0.3.9
- `gpui-tray` 0.2
- objc2 and `objc2-service-management`
- GitHub Actions (`ubuntu-24.04` for checks, `macos-26` for releases only)

**Spec:** `docs/superpowers/specs/2026-10-21-loctel-design.md`. Read it
before any task. Section numbers below (§) refer to it.

## Milestones (the whole build)

Each milestone gets its own detailed plan file when it starts, written from
the spec and this roadmap. Each one ends with a release whose notes carry a
"Check on your Mac" list (§10.4).

| # | Milestone | Delivers | Mac checklist highlights |
|---|---|---|---|
| **M1** | **Headless core, MCP, menu bar shell, releases** (this file) | ingest, assembly, failure semantics, service map data, MCP tools, meshgen, e2e tests, screenshot harness, status item menu, login item, CI, `.dmg` releases | install via Open Anyway; menu bar icon, no Dock icon; Docker container and bare-metal service both reach loctel; Claude Code connects to MCP and answers "why did my last request fail"; start at login |
| M2 | Query languages and discard rules | TraceQL and LogQL parsers and evaluators (§6); rules engine at ingest with persistence and preview (§7); MCP `query` arguments; rule counts in status | a health-check rule silences real health checks; a TraceQL structural query from Grafana works unchanged |
| M3 | Main window: Map & traces, Trace view | GPUI Kit UI per Home A and Trace mockups (§9.2), glass sidebar and toolbar (§9.3), Dock/⌘-Tab switching, screenshot tests | glass looks right; window opens fast; Dock/⌘-Tab behaviour |
| M4 | Logs, Discard rules screen, popover, Settings | remaining screens (§9.2); popover under the icon; settings UI | popover anchoring; settings changes take effect |
| M5 | Scale | spill-to-disk segment store and 4 GB cap (§5); performance budgets as CI tests (§10.3); light theme | smooth scrolling of the 100k-span trace and 10M logs; startup times |

## Global Constraints

- **Ports:** OTLP gRPC **4317** and OTLP HTTP **4318** (`/v1/traces`,
  `/v1/logs`, protobuf and JSON). MCP **4319** at `http://127.0.0.1:4319/mcp`.
  All three configurable.
- **Binding:** listeners bind `127.0.0.1` by default (§4). MCP is always
  loopback and validates `Origin` (§8).
- **MCP:** spec **2026-07-28**, Streamable HTTP, no auth, **read-only**.
  Compact by default, with explicit `limit` and `cursor`. Results carry
  `structuredContent` plus the same JSON as text (§8).
- **OTLP:** request size limit raised to **64 MiB** on both gRPC and HTTP;
  gzip accepted. Metrics are accepted and dropped. Invalid items are
  reported via `partial_success`.
- **Trace completeness:** complete after **10 s** without a new span for that
  trace (configurable; tests use small values) (§4).
- **Failure semantics** exactly as §9.4:
  - erroring = status `Error`; an exception event alone doesn't count
  - Failed = the root span is erroring; ErrorsInside = some other span is
    erroring
  - failed-in = among the erroring spans with no erroring descendants, the
    one that started earliest
- **No persistence of telemetry.** Only `settings.toml` (and rules in M2)
  persist.
- **Platform:** macOS 26+ on Apple Silicon is the product. Linux x86_64
  exists for development and tests only.
- **Pinned crates:** `gpui-pre`, `gpui-pre-platform` and `gpui-pre-wgpu`
  `=0.3.9`. Upgrades are deliberate, in their own PR.
- **Rust toolchain** pinned in `rust-toolchain.toml` to `1.98.1`.
- **Quality gate:**
  - `cargo fmt --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace`

  All must pass before any merge. Never skip, ignore or loosen a test to get
  green.
- **macOS CI runner:** used **only** by the release job on pushes to main,
  never on PRs (§10.3).
- **Commits:** conventional style (`feat(store): …`). End every commit
  message with:
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`
  `Claude-Session: https://claude.ai/code/session_01DTbbjC6k9cSgJWN8hpHwZG`
- **Docs travel with code:** each task updates the doc it names, plus
  `docs/architecture.md` when it adds a crate.

## Review Focus

These are the inputs most likely to bite real use that the spec doesn't spell
out. Each one has a test in its owning task.

1. **Exporter retries resend identical spans or logs.** Expect them stored
   once and counted once. *Test:* Task 3 `duplicate_span_is_ignored`.
2. **Malformed IDs** (all-zero trace ID, an 7-byte span ID, a parent equal
   to itself). Expect the items rejected via `partial_success`, with no
   crash. *Test:* Task 6 `rejects_invalid_ids_with_partial_success`.
3. **Clock skew between containers** (a child starting before its parent,
   end before start). Expect no panic, a duration clamped to 0, and children
   still under their parent. *Tests:* Task 2 `duration_clamps_negative` and
   Task 3 `skewed_child_still_assembles`.
4. **Large gzip batches above the library defaults** (more than 4 MiB
   uncompressed). Expect them accepted. *Test:* Task 6
   `http_gzip_large_request_accepted`.
5. **A resource without `service.name`.** Expect it shown as
   `unknown_service`, never dropped. *Tests:* Task 2
   `service_name_defaults_to_unknown_service` and Task 5
   `unknown_service_appears_in_map`.

---

## File map (M1)

```
Cargo.toml                         workspace, [workspace.dependencies] pins
rust-toolchain.toml                1.98.1 + rustfmt, clippy; target aarch64-apple-darwin
AGENTS.md, CLAUDE.md               agent guide (CLAUDE.md = "@AGENTS.md")
docs/architecture.md               crate map + data flow
docs/areas/{ingest,store,mcp,ui,macos,release,testing}.md
docs/decisions/NNNN-*.md           decision log
docs/milestones/roadmap.md         milestone table (copied from this plan)
docs/milestones/mac-checklist.md   current milestone's "Check on your Mac" list
script/setup-linux.sh              apt + rustup setup, idempotent
.claude/settings.json              SessionStart hook -> script/setup-linux.sh (remote only)
.claude/skills/{run-and-screenshot,update-screenshots,add-mcp-tool,investigate-ci}/SKILL.md
.github/workflows/ci.yml           Linux gate (PRs + main)
.github/workflows/release.yml      macOS bundle + dmg + smoke + GitHub release (main only)
crates/loctel-model/               domain types, OTLP-independent
crates/loctel-store/               in-memory store: assembly, failure, search, logs, service map
crates/loctel-ingest/              OTLP gRPC + HTTP receivers, proto->model conversion
crates/loctel-engine/              wires store+ingest+mcp, settings, pause, rates
crates/loctel-mcp/                 rmcp server + tools + DTOs
crates/loctel-meshgen/             fake mesh scenarios (lib + `meshgen` bin)
crates/loctel-e2e/                 end-to-end tests only (publish = false)
crates/loctel-ui/                  GPUI views (M1: StatusView, menu text), screenshot harness
crates/loctel-macos/               status item, activation policy, login item (cfg macos)
crates/loctel-app/                 `loctel` binary (headless + GUI), bundle metadata
xtask/                             screenshots, next-version, release-notes, mcp-smoke
```

---

### Task 1: Workspace, toolchain, Linux setup, CI gate, agent guide skeleton

**Files:**
- Create: `Cargo.toml`, `rust-toolchain.toml`, `.gitignore`
- Create: `crates/loctel-model/Cargo.toml`, `crates/loctel-model/src/lib.rs`
  (placeholder module with one test)
- Create: `script/setup-linux.sh`, `.claude/settings.json`
- Create: `.github/workflows/ci.yml`
- Create: `AGENTS.md`, `CLAUDE.md`, `docs/architecture.md`,
  `docs/decisions/0001-tech-stack.md`, `docs/milestones/roadmap.md`
- Modify: `README.md` (one paragraph on what loctel is, plus a link to
  AGENTS.md)

**Interfaces:**
- Produces:
  - workspace `members = ["crates/*", "xtask"]`, `resolver = "3"`,
    `edition = "2024"`
  - every later crate's dependency comes from `[workspace.dependencies]`
    (exact pins listed in this plan's tasks)

- [ ] **Step 1: Write `script/setup-linux.sh`.** It must be idempotent, use
  `set -euo pipefail`, run apt only when a package is missing, and install
  exactly these packages:
  - `build-essential pkg-config clang`
  - `libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libx11-xcb-dev`
  - `libfontconfig-dev libfreetype-dev`
  - `libvulkan1 mesa-vulkan-drivers libegl1 libegl-mesa0`
  - `xvfb xdotool imagemagick protobuf-compiler`

  It then runs `rustup show` (installs the pinned toolchain) and
  `rustup target add aarch64-apple-darwin`.
- [ ] **Step 2: Wire the hook.** Add `.claude/settings.json` with a
  `SessionStart` hook running `bash script/setup-linux.sh`, guarded so it
  only runs when `CLAUDE_CODE_REMOTE=true`. The `session-start-hook` skill
  documents the format.
- [ ] **Step 3: Create the workspace.** Write the root `Cargo.toml`
  (workspace only, with `[workspace.lints.clippy] all = "warn"`),
  `rust-toolchain.toml` (`channel = "1.98.1"`,
  `components = ["rustfmt", "clippy"]`,
  `targets = ["aarch64-apple-darwin"]`) and `loctel-model` with
  `#[test] fn workspace_builds() {}`.
- [ ] **Step 4: Run the gate locally.**

  ```bash
  bash script/setup-linux.sh
  cargo fmt --check
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  ```

  Expected: all succeed, `1 passed`.
- [ ] **Step 5: Write `.github/workflows/ci.yml`.** It triggers on
  `pull_request` and `push` to `main`, with one job on `ubuntu-24.04`:
  1. checkout
  2. `bash script/setup-linux.sh`
  3. `Swatinem/rust-cache@v2`
  4. fmt, then clippy, then `cargo test --workspace`
  5. on failure, `actions/upload-artifact@v4` with
     `target/screenshot-diffs/**` (name `screenshot-diffs`)
- [ ] **Step 6: Lint the workflow.** Download the `actionlint` binary
  (release tarball) into the scratch directory, then run
  `actionlint .github/workflows/ci.yml`. Expected: no output.
- [ ] **Step 7: Write the agent docs.**
  - `AGENTS.md`:
    - what loctel is (two sentences + spec link)
    - the crate map (pointing to `docs/architecture.md`)
    - the commands above
    - house rules, verbatim from the spec §10.1:
      - never merge on red
      - never skip, disable or loosen a test
      - update docs and the decision log in the same PR
      - the merging agent watches the release job
      - Mac-only features stay "unverified" until a milestone check
  - `CLAUDE.md` containing exactly `@AGENTS.md`.
  - `docs/decisions/0001-tech-stack.md`: summarise spec §11 with today's
    date.
  - `docs/milestones/roadmap.md`: copy of the Milestones table above.
- [ ] **Step 8: Commit.** Message: `chore: workspace, Linux setup, CI gate
  and agent guide`.

### Task 2: Domain model (`loctel-model`)

**Files:**
- Modify: `crates/loctel-model/src/lib.rs` (re-exports)
- Create: `crates/loctel-model/src/{ids.rs, attr.rs, resource.rs, span.rs, log.rs, severity.rs, exception.rs, labels.rs}`
- Test: `crates/loctel-model/tests/model.rs`

**Interfaces:**
- Produces (all `Clone + Debug + PartialEq`; serde `Serialize` on all):
  - `TraceId([u8; 16])`, `SpanId([u8; 8])`:
    - `from_hex(&str) -> Result<Self, IdError>`
    - `from_bytes(&[u8]) -> Result<Self, IdError>`
    - `is_valid(&self) -> bool` (not all zero)
    - `Display` as lowercase hex
    - `Hash + Eq + Ord + Copy`
  - `enum AttrValue { Str(String), Bool(bool), Int(i64), Double(f64), Bytes(Vec<u8>), Array(Vec<AttrValue>), Map(Vec<(String, AttrValue)>) }`:
    - `fn to_display_string(&self) -> String`. Str is as-is, Int and Bool
      via `to_string`, Double uses Rust `{}`, Bytes is hex, Array and Map
      are compact JSON.
  - `type Attributes = Vec<(String, AttrValue)>`, plus
    `fn attr<'a>(attrs: &'a Attributes, key: &str) -> Option<&'a AttrValue>`
  - `struct Resource { pub attributes: Attributes }`, plus
    `fn service_name(&self) -> &str` (falls back to `"unknown_service"`)
  - `enum SpanKind { Unspecified, Internal, Server, Client, Producer, Consumer }`
  - `enum StatusCode { Unset, Ok, Error }`
  - `struct SpanEvent { pub time_unix_nano: u64, pub name: String, pub attributes: Attributes }`
  - `struct Span`, with these fields:
    - `trace_id: TraceId`, `span_id: SpanId`, `parent_span_id: Option<SpanId>`
    - `name: String`, `kind: SpanKind`
    - `start_unix_nano: u64`, `end_unix_nano: u64`
    - `status: StatusCode`, `status_message: String`
    - `attributes: Attributes`, `events: Vec<SpanEvent>`
    - `resource: Arc<Resource>`, `scope_name: String`

    and these methods:
    - `duration_nanos(&self) -> u64` (saturating)
    - `is_error(&self) -> bool` (status == Error)
    - `service(&self) -> &str`
    - `exception(&self) -> Option<ExceptionInfo>` (first event named
      `"exception"`)
    - `peer(&self) -> Option<&str>` (`server.address`, else `peer.service`)
  - `enum Severity { Unspecified, Trace, Debug, Info, Warn, Error, Fatal }`:
    - `from_number(i32)`: 1–4 Trace, 5–8 Debug, 9–12 Info, 13–16 Warn,
      17–20 Error, 21–24 Fatal, otherwise Unspecified
    - `level_label(&self) -> &'static str`: `trace`, `debug`, `info`,
      `warn`, `error`, `fatal`, or `unknown` for Unspecified
    - `Ord`
  - `struct LogRecord`, with these fields:
    - `time_unix_nano: u64`, `observed_unix_nano: u64`
    - `severity: Severity`, `severity_text: String`
    - `body: String`, `attributes: Attributes`
    - `trace_id: Option<TraceId>`, `span_id: Option<SpanId>`
    - `resource: Arc<Resource>`, `scope_name: String`

    and these methods:
    - `service(&self) -> &str`
    - `exception(&self) -> Option<ExceptionInfo>` (from `exception.*`
      attributes)
    - `dedup_key(&self) -> u64` (hash of the time, body, trace ID, span ID
      and service)
  - `struct ExceptionInfo { pub type_name: String, pub message: String, pub stacktrace: Option<String> }`
  - `fn loki_label_name(key: &str) -> String`: every character outside
    `[A-Za-z0-9_]` becomes `_`

- [ ] **Step 1: Write the failing tests** in `tests/model.rs`:

```rust
#[test] fn trace_id_hex_roundtrip() {
    let id = TraceId::from_hex("4bf92f3577b34da6a3ce929d0e0e4736").unwrap();
    assert_eq!(id.to_string(), "4bf92f3577b34da6a3ce929d0e0e4736");
    assert!(TraceId::from_hex("abc").is_err());
    assert!(!TraceId::from_bytes(&[0u8; 16]).unwrap().is_valid());
}
#[test] fn service_name_defaults_to_unknown_service() {
    assert_eq!(Resource { attributes: vec![] }.service_name(), "unknown_service");
}
#[test] fn duration_clamps_negative() {
    let mut s = span_fixture(); s.start_unix_nano = 2_000; s.end_unix_nano = 1_000;
    assert_eq!(s.duration_nanos(), 0);
}
#[test] fn span_exception_from_event() { /* event "exception" with exception.type/message/stacktrace -> ExceptionInfo with all three */ }
#[test] fn exception_event_alone_is_not_error() { /* status Unset + exception event -> is_error()==false, exception().is_some() */ }
#[test] fn log_exception_from_attributes() { /* exception.type + exception.message attrs -> Some(..) */ }
#[test] fn severity_mapping() {
    assert_eq!(Severity::from_number(9), Severity::Info);
    assert_eq!(Severity::from_number(17).level_label(), "error");
    assert_eq!(Severity::from_number(0).level_label(), "unknown");
}
#[test] fn loki_label_names() { assert_eq!(loki_label_name("service.name"), "service_name"); }
#[test] fn attr_display_strings() { /* Int(42)->"42", Array([Int(1),Str("a")])->"[1,\"a\"]" */ }
```

- [ ] **Step 2: Watch them fail.** Run `cargo test -p loctel-model`.
  Expected: FAIL to compile (types missing).
- [ ] **Step 3: Implement the types.** Put them in the listed files.
  `span_fixture()` lives in `tests/model.rs`.
- [ ] **Step 4: Watch them pass.** Run `cargo test -p loctel-model`.
  Expected: PASS (9 tests).
- [ ] **Step 5: Commit.** Message: `feat(model): domain types for spans,
  logs, resources`.

### Task 3: Store — trace assembly, completeness, dedup, eviction (`loctel-store`)

**Files:**
- Create: `crates/loctel-store/Cargo.toml` (deps: `loctel-model`,
  `parking_lot`)
- Create: `crates/loctel-store/src/{lib.rs, clock.rs, config.rs, trace.rs, store.rs}`
- Test: `crates/loctel-store/tests/{fixtures.rs, assembly.rs}`
- Create: `docs/areas/store.md`

**Interfaces:**
- Consumes: `loctel_model::{Span, TraceId, SpanId}`.
- Produces:
  - Clocks:
    - `trait Clock: Send + Sync { fn now_unix_nano(&self) -> u64; }`
    - `SystemClock`
    - `ManualClock`, with `new(start: u64)`, `advance(Duration)` and
      `set(u64)`
  - `struct StoreConfig { pub max_traces: usize /* 200_000 */, pub max_log_records: usize /* 5_000_000 */, pub completeness_quiet: Duration /* 10s */ }`
    with `Default`.
  - `#[derive(Clone)] struct Store` (an `Arc` inside, `Send + Sync`):
    - `new(config: StoreConfig, clock: Arc<dyn Clock>) -> Store`
    - `ingest_spans(&self, spans: Vec<Span>)`
    - `trace(&self, id: TraceId) -> Option<TraceSnapshot>`
    - `stats(&self) -> StoreStats`
    - `clear(&self)`
  - `struct TraceSnapshot`:
    - `pub trace_id: TraceId`
    - `pub spans: Vec<Span>` (sorted by `start_unix_nano`, then `span_id`)
    - `pub root: Option<SpanId>`
    - `pub orphans: Vec<SpanId>`
    - `pub complete: bool`
    - `pub status: TraceStatus` and `pub failed_in: Option<FailedIn>`. The
      types are declared in `src/failure.rs` in this task (see Task 4 for
      their fields). Task 4 fills them; until then they are always `Ok` and
      `None`.
    - `fn children(&self, id: SpanId) -> impl Iterator<Item = &Span>`
  - `struct StoreStats { pub traces: usize, pub spans: usize, pub logs: usize, pub spans_received: u64, pub logs_received: u64 }`
- Behaviour:
  - root = the span with `parent_span_id == None`. If there are several,
    the earliest start wins.
  - orphan = a span whose parent ID isn't in the trace.
  - complete = `now - last_arrival >= completeness_quiet`.
  - eviction removes the oldest trace (by first arrival) whole, once
    `traces > max_traces`.

- [ ] **Step 1: Write the fixtures and failing tests.** `tests/fixtures.rs`
  holds `span(trace: u8, id: u8, parent: Option<u8>, service: &str, name: &str, start_ms: u64, dur_ms: u64) -> Span`
  and an `error(span) -> Span` helper. In `tests/assembly.rs`:

```rust
#[test] fn late_child_before_parent_assembles() {
    let (store, _clock) = store_with_manual_clock();
    store.ingest_spans(vec![span(1, 2, Some(1), "orders-api", "POST /orders", 5, 10)]);
    assert_eq!(store.trace(tid(1)).unwrap().orphans, vec![sid(2)]);
    store.ingest_spans(vec![span(1, 1, None, "web-bff", "POST /api/checkout", 0, 20)]);
    let t = store.trace(tid(1)).unwrap();
    assert_eq!(t.spans.len(), 2); assert_eq!(t.root, Some(sid(1))); assert!(t.orphans.is_empty());
}
#[test] fn complete_after_quiet_period() {
    let (store, clock) = store_with_manual_clock(); // quiet = 10s
    store.ingest_spans(vec![span(1, 1, None, "a", "x", 0, 1)]);
    clock.advance(Duration::from_secs(9));  assert!(!store.trace(tid(1)).unwrap().complete);
    clock.advance(Duration::from_secs(1));  assert!(store.trace(tid(1)).unwrap().complete);
    store.ingest_spans(vec![span(1, 2, Some(1), "a", "y", 0, 1)]);
    assert!(!store.trace(tid(1)).unwrap().complete);
}
#[test] fn duplicate_span_is_ignored() {
    let (store, _c) = store_with_manual_clock();
    let s = span(1, 1, None, "a", "x", 0, 1);
    store.ingest_spans(vec![s.clone()]); store.ingest_spans(vec![s]);
    assert_eq!(store.trace(tid(1)).unwrap().spans.len(), 1);
    assert_eq!(store.stats().spans, 1);
}
#[test] fn skewed_child_still_assembles() { /* child start < parent start, end < start: no panic, child under parent via children() */ }
#[test] fn evicts_oldest_whole_traces() { /* max_traces=2, ingest traces 1,2,3 (2 spans each) -> trace(1) None, 2 and 3 have 2 spans, stats.traces==2 */ }
#[test] fn clear_empties_store() { /* after clear: stats all zero except *_received */ }
```

- [ ] **Step 2: Watch them fail.** Run `cargo test -p loctel-store`.
  Expected: compile failure.
- [ ] **Step 3: Implement.** Use
  `parking_lot::RwLock<Inner>`, where `Inner` holds
  `HashMap<TraceId, TraceEntry>` plus a `VecDeque<TraceId>` in arrival
  order. `TraceEntry` holds:
  - its spans
  - a `HashMap<SpanId, usize>` index
  - `first_arrival` and `last_arrival` (clock time)

  Dedup is by `span_id` within a trace. Keep `Store`'s public API exactly as
  listed: M5 replaces the internals.
- [ ] **Step 4: Watch them pass.** Run `cargo test -p loctel-store`.
  Expected: PASS.
- [ ] **Step 5: Docs.** Write `docs/areas/store.md` covering the API,
  assembly rules, completeness and eviction. Add the crate to
  `docs/architecture.md`.
- [ ] **Step 6: Commit.** Message: `feat(store): trace assembly, completeness,
  dedup and eviction`.

### Task 4: Store — failure semantics, trace summaries, trace search

**Files:**
- Create: `crates/loctel-store/src/{summary.rs, page.rs}`
- Modify: `crates/loctel-store/src/failure.rs` (add `classify` and
  `trace_exception` next to the types from Task 3)
- Modify: `crates/loctel-store/src/store.rs` (fill `status`/`failed_in`; add
  `search_traces`)
- Test: `crates/loctel-store/tests/{failure.rs, search_traces.rs}`

**Interfaces:**
- Produces:
  - `enum TraceStatus { Ok, ErrorsInside, Failed }` (serde
    `rename_all = "snake_case"`)
  - `struct FailedIn { pub span_id: SpanId, pub service: String, pub span_name: String, pub peer: Option<String>, pub more: usize }`
  - `fn classify(spans: &[Span], root: Option<SpanId>) -> (TraceStatus, Option<FailedIn>)`
    (pure, in `failure.rs`)
  - `fn trace_exception(snapshot: &TraceSnapshot) -> Option<ExceptionInfo>`:
    the failed-in span's exception if any, else the nearest ancestor's on
    the path to the root, else the first exception in start order
  - `struct TraceSummary`:
    - `pub trace_id: TraceId`
    - `pub root_name: Option<String>`, `pub root_service: Option<String>`
    - `pub start_unix_nano: u64`, `pub duration_nanos: u64`
    - `pub span_count: usize`
    - `pub services: Vec<String>` (in first-seen order)
    - `pub status: TraceStatus`, `pub failed_in: Option<FailedIn>`
    - `pub first_exception: Option<ExceptionInfo>`
    - `pub complete: bool`
  - `struct TraceFilter { pub since_unix_nano: Option<u64>, pub service: Option<String>, pub errors_only: bool, pub min_duration_nanos: Option<u64> }`
    with `Default`
  - `struct PageRequest { pub limit: usize, pub cursor: Option<String> }`
    and `struct Page<T> { pub items: Vec<T>, pub next_cursor: Option<String> }`
  - `Store::search_traces(&self, f: &TraceFilter, p: &PageRequest) -> Page<TraceSummary>`:
    - newest first, by trace start
    - the cursor is opaque: `"{start_unix_nano}-{trace_id}"` of the last
      item (keyset)
    - `errors_only` includes both `Failed` and `ErrorsInside`
    - `service` matches if any span has that service
  - `Store::summary(&self, id: TraceId) -> Option<TraceSummary>`
- The algorithm, as §9.4:
  1. erroring = `is_error()`
  2. status = Failed if the root is erroring; else ErrorsInside if any span
     is erroring; else Ok (a missing root plus errors means ErrorsInside)
  3. candidates = erroring spans with no erroring descendant
  4. failed-in = the candidate with the minimum `(start_unix_nano, span_id)`
  5. `more = candidates.len() - 1`
  6. `peer = span.peer()` for Client and Producer spans

- [ ] **Step 1: Write the failing tests** in `tests/failure.rs`:

```rust
#[test] fn root_error_means_failed() { /* root(err) -> child payments POST /charges(err leaf): Failed, failed_in.service=="payments-api", span_name=="POST /charges", more==0 */ }
#[test] fn inner_error_with_ok_root_is_errors_inside() { /* root Unset, one child Error -> ErrorsInside, failed_in=that child */ }
#[test] fn exception_event_alone_is_not_error() { /* only an exception event, all Unset -> Ok, failed_in None */ }
#[test] fn failed_in_picks_earliest_lowest_error() {
    // root(err) -> payments(err, start 200) ; root -> inventory(start 300) -> db(err, start 310)
    let (status, f) = classify(&spans, Some(sid(1)));
    assert_eq!(status, TraceStatus::Failed);
    let f = f.unwrap(); assert_eq!(f.service, "payments-api"); assert_eq!(f.more, 1);
}
#[test] fn missing_root_then_root_arrives() { /* ingest erroring child only -> ErrorsInside; ingest erroring root -> Failed */ }
#[test] fn exception_falls_back_to_nearest_ancestor() { /* failed-in client span has no exception; its parent has one -> trace_exception returns parent's */ }
```

  and in `tests/search_traces.rs`:
  - `newest_first`
  - `errors_only_includes_both_kinds`
  - `service_filter_matches_any_span`
  - `min_duration_filter`
  - `since_filter_excludes_older`
  - `cursor_pages_without_duplicates`: 5 traces, limit 2, gives pages of
    2, 2 and 1, then `next_cursor == None`

- [ ] **Step 2: Watch them fail.** Run `cargo test -p loctel-store`.
  Expected: compile failure.
- [ ] **Step 3: Implement.** Write `classify`, `trace_exception`, summaries
  and search. `TraceSnapshot.status` and `failed_in` are computed on read.
- [ ] **Step 4: Watch them pass.** Run `cargo test -p loctel-store`.
  Expected: PASS.
- [ ] **Step 5: Docs.** Add a "Failure semantics" section to
  `docs/areas/store.md` that quotes §9.4.
- [ ] **Step 6: Commit.** Message: `feat(store): failure semantics, trace
  summaries and search`.

### Task 5: Store — logs, service map, services

**Files:**
- Create: `crates/loctel-store/src/{logs.rs, service_map.rs}`
- Modify: `crates/loctel-store/src/store.rs`
- Test: `crates/loctel-store/tests/{logs.rs, service_map.rs}`

**Interfaces:**
- Produces:
  - `Store::ingest_logs(&self, logs: Vec<LogRecord>)`. Dedup is by
    `dedup_key`, and the oldest records are evicted past `max_log_records`.
  - `struct LogFilter { pub since_unix_nano: Option<u64>, pub service: Option<String>, pub min_severity: Option<Severity>, pub contains: Option<String>, pub trace_id: Option<TraceId>, pub span_id: Option<SpanId> }`
    with `Default`. `contains` is a case-insensitive substring match on
    the body.
  - `Store::search_logs(&self, f: &LogFilter, p: &PageRequest) -> Page<LogRecord>`:
    newest first, with a keyset cursor.
  - `Store::logs_for_span(&self, t: TraceId, s: SpanId) -> Vec<LogRecord>`:
    oldest first.
  - `struct TimeWindow { pub since_unix_nano: u64 }`
  - `enum NodeKind { Service, Database, Queue, External }`
  - `struct ServiceNode { pub id: String, pub kind: NodeKind, pub requests: u64, pub errors: u64 }`
  - `struct ServiceEdge { pub from: String, pub to: String, pub calls: u64, pub errors: u64 }`
  - `struct ServiceMap { pub nodes: Vec<ServiceNode>, pub edges: Vec<ServiceEdge> }`
    (sorted by id, then `(from, to)`)
  - `Store::service_map(&self, w: TimeWindow) -> ServiceMap`
  - `struct ServiceStats { pub name: String, pub requests: u64, pub errors: u64, pub last_seen_unix_nano: u64 }`
    and `Store::services(&self, w: TimeWindow) -> Vec<ServiceStats>`
    (Service nodes only)
- Service map rules (§9.2), counted over spans starting inside the window:
  - **Service nodes:** one per `service()`. `requests` and `errors` count
    that service's Server and Consumer spans.
  - **Service-to-service edges:** a span whose parent is in another
    service gives an edge `parent.service → span.service`. It counts as an
    error if the parent (Client or Producer) is erroring, otherwise if the
    child is. **Skip** this edge when both spans carry
    `messaging.system`.
  - **Database:** a Client span with `db.system.name` (or the legacy
    `db.system`) gives a Database node named by that value, and an edge
    from its service. The edge's errors come from the client span.
  - **Queue:** a Producer span with `messaging.system` gives a Queue node
    and an edge `service → queue`. A Consumer span with `messaging.system`
    gives an edge `queue → service`.
  - **External:** a Client span with no db or messaging attribute, and no
    child in another service, *in a complete trace*, gives an External node
    named by `peer()`, or `"unknown"`, plus an edge.

- [ ] **Step 1: Write the failing tests.**

  `tests/logs.rs`:
  - `search_logs_newest_first_with_filters`
  - `contains_is_case_insensitive`
  - `min_severity_filters_lower_levels`
  - `logs_for_span_links_by_ids`
  - `duplicate_log_is_ignored`
  - `log_eviction_drops_oldest`

  `tests/service_map.rs`:
  - `service_edges_from_cross_service_parent_child`
  - `database_node_from_db_system_name`
  - `queue_node_routes_messaging`: gives `orders-api→rabbitmq` and
    `rabbitmq→notifications-worker`, and no direct edge between the two
    services
  - `external_node_only_after_complete`: `card-gateway` is absent before
    the quiet period and present after it
  - `entry_span_error_counts`
  - `window_excludes_old_spans`
  - `unknown_service_appears_in_map`
  - `services_list_has_counts_and_last_seen`
- [ ] **Step 2: Watch them fail.** Run `cargo test -p loctel-store`.
  Expected: compile failure.
- [ ] **Step 3: Implement.** Logs live in a time-ordered `Vec`, with
  binary-searched insertion for late arrivals plus a
  `HashMap<(TraceId, SpanId), Vec<idx>>` index (rebuilt on eviction). The
  service map is computed on demand from the traces in the window.
- [ ] **Step 4: Watch them pass.** Run `cargo test -p loctel-store`.
  Expected: PASS.
- [ ] **Step 5: Docs.** Add "Logs" and "Service map" sections to
  `docs/areas/store.md`.
- [ ] **Step 6: Commit.** Message: `feat(store): logs, service map and
  services`.

### Task 6: OTLP ingest (`loctel-ingest`)

**Files:**
- Create: `crates/loctel-ingest/Cargo.toml`. Workspace pins:
  - `opentelemetry-proto = { version = "=0.33.1", default-features = false, features = ["gen-tonic","trace","logs","metrics","with-serde"] }`
  - `tonic = { version = "0.14.6", features = ["server","gzip"] }`
  - `axum = "0.8.9"`
  - `tower-http = { version = "0.6", features = ["decompression-gzip","limit"] }`
  - `tokio`, `prost`, `serde_json`, `flate2` (tests)
- Create: `crates/loctel-ingest/src/{lib.rs, convert.rs, sink.rs, grpc.rs, http.rs, listener.rs}`
- Test: `crates/loctel-ingest/tests/{convert.rs, servers.rs}`
- Create: `docs/areas/ingest.md`

**Interfaces:**
- Consumes: `loctel_model::*`.
- Produces:
  - `trait Sink: Send + Sync + 'static { fn spans(&self, spans: Vec<Span>); fn logs(&self, logs: Vec<LogRecord>); }`
  - `struct Converted<T> { pub items: Vec<T>, pub rejected: u64, pub error_message: Option<String> }`
  - `fn spans_from_request(req: ExportTraceServiceRequest) -> Converted<Span>`
    and `fn logs_from_request(req: ExportLogsServiceRequest) -> Converted<LogRecord>`.
    Rules:
    - reject a span with an invalid trace or span ID
    - treat a parent ID that is all zero, or equal to the span's own ID, as
      no parent
    - a log body that isn't a string becomes the compact JSON of its
      AnyValue
  - `struct IngestConfig { pub grpc_addr: SocketAddr, pub http_addr: SocketAddr, pub max_message_bytes: usize /* 64 MiB */ }`
  - `enum ListenerState { Listening(SocketAddr), Failed(String) }` and
    `struct ListenerStatus { pub name: &'static str, pub requested: SocketAddr, pub state: ListenerState }`
    (also used by MCP and the engine)
  - `async fn start(config: IngestConfig, sink: Arc<dyn Sink>) -> IngestServers`,
    where
    `struct IngestServers { pub grpc: ListenerStatus /* name "otlp-grpc" */, pub http: ListenerStatus /* "otlp-http" */ }`
    and `fn shutdown(self)`. A failure to bind one listener never stops the
    other.
  - HTTP:
    - `POST /v1/traces`, `/v1/logs` and `/v1/metrics`
    - `application/x-protobuf` and `application/json` (OTLP/JSON: hex IDs,
      int64 as strings, integer enums, unknown fields ignored)
    - `Content-Encoding: gzip`
    - the response uses the request's content type and carries
      `partial_success` when anything was rejected
  - gRPC: `TraceService`, `LogsService` and `MetricsService` (metrics
    always answer OK and are dropped).

- [ ] **Step 1: Write the failing tests.**

  `tests/convert.rs`:
  - `converts_resource_scope_and_span_fields`: covers name, kind, status,
    attributes, events, the resource's service name and the scope name
  - `rejects_invalid_ids_with_partial_success`: an all-zero trace ID and a
    7-byte span ID give `rejected == 2`, and the valid span is kept
  - `self_parent_becomes_root`
  - `log_body_map_becomes_json`
  - `severity_number_maps`

  `tests/servers.rs`, each with port 0 on `127.0.0.1` and a
  `RecordingSink`:
  - `grpc_export_reaches_sink`: use the generated tonic client
  - `http_protobuf_export_reaches_sink`
  - `http_json_export_reaches_sink`: the trace ID is sent as a hex string
    and the response content type is JSON
  - `http_gzip_large_request_accepted`: about 10 MiB of uncompressed spans,
    gzip, gives 200 and every span reaches the sink
  - `metrics_are_accepted_and_dropped`: over HTTP and gRPC, both OK, the
    sink sees nothing
  - `port_in_use_reports_failed_listener`: pre-bind a `TcpListener`, then
    start with that HTTP port. HTTP is `Failed(_)` and gRPC is
    `Listening(_)`.
  - `partial_success_reported_over_http`
- [ ] **Step 2: Watch them fail.** Run `cargo test -p loctel-ingest`.
  Expected: compile failure.
- [ ] **Step 3: Implement.** Follow the interfaces above. Raise tonic's
  `max_decoding_message_size` and axum's `DefaultBodyLimit` to
  `max_message_bytes`.
- [ ] **Step 4: Watch them pass.** Run `cargo test -p loctel-ingest`.
  Expected: PASS.
- [ ] **Step 5: Docs.** Write `docs/areas/ingest.md` covering the
  endpoints, formats, limits and the gotchas from §4.
- [ ] **Step 6: Commit.** Message: `feat(ingest): OTLP gRPC and HTTP
  receivers`.

### Task 7: Engine, settings, pause, rates, headless binary

**Files:**
- Create: `crates/loctel-engine/Cargo.toml` (deps: store, ingest, `toml`,
  `serde`, `dirs`, `tokio`, `anyhow`)
- Create: `crates/loctel-engine/src/{lib.rs, settings.rs, rates.rs, sink.rs}`
- Create: `crates/loctel-app/Cargo.toml` (bin name `loctel`; deps: engine,
  `clap` 4, `tokio`, `tracing-subscriber`)
- Create: `crates/loctel-app/src/{main.rs, cli.rs}`
- Test: `crates/loctel-engine/tests/{settings.rs, engine.rs, rates.rs}`,
  `crates/loctel-app/tests/headless.rs`

**Interfaces:**
- Produces:
  - `struct Settings`, with these fields (serde `default` on the struct, so
    missing keys take their defaults):
    - `otlp_grpc_port: u16` (4317)
    - `otlp_http_port: u16` (4318)
    - `mcp_port: u16` (4319)
    - `disk_cap_bytes: u64` (4 GiB)
    - `start_at_login: bool` (true)
    - `first_launch_done: bool` (false)

    and these functions:
    - `Settings::default_path() -> PathBuf`: `dirs::config_dir()/loctel/settings.toml`.
      On macOS that is `~/Library/Application Support/loctel/`.
    - `load_or_default(path: &Path) -> Settings`
    - `save(&self, path: &Path) -> anyhow::Result<()>`: creates the
      directories as needed
  - `struct EngineConfig { pub bind_ip: IpAddr /* 127.0.0.1 */, pub settings: Settings, pub store: StoreConfig }`
  - `struct Engine;` with
    `fn start(config: EngineConfig) -> anyhow::Result<EngineHandle>`. It
    owns a multi-thread tokio runtime on its own threads, and is callable
    from a non-async main.
  - `#[derive(Clone)] struct EngineHandle`:
    - `store(&self) -> &Store`
    - `status(&self) -> EngineStatus`
    - `set_paused(&self, paused: bool)`
    - `is_paused(&self) -> bool`
    - `mcp_url(&self) -> Option<String>` (from Task 9; returns `None` until
      then)
    - `shutdown(&self)`
  - `struct EngineStatus { pub paused: bool, pub listeners: Vec<ListenerStatus>, pub spans_per_sec: f64, pub logs_per_sec: f64, pub stats: StoreStats }`
  - `RateMeter` (a 5-second sliding window):
    `new()`, `record(at_unix_nano: u64, count: u64)` and
    `per_sec(now_unix_nano: u64) -> f64`
  - CLI: `loctel [--headless] [--grpc-port N] [--http-port N] [--mcp-port N] [--settings PATH] [--completeness-ms N]`.
    In headless mode it prints one JSON line per listener to stdout:
    `{"event":"listener","name":"otlp-grpc","state":"listening","address":"127.0.0.1:4317"}`
    or `"state":"failed","error":"…"`. It runs until SIGINT or SIGTERM,
    then exits 0. Without `--headless` it is a placeholder until Tasks 11
    and 12; for now it behaves like headless.
- Pause: the engine's `Sink` drops everything while paused. The ingest
  servers still answer success.

- [ ] **Step 1: Write the failing tests.**
  - `settings_defaults_when_missing`
  - `settings_partial_toml_fills_defaults`
  - `settings_roundtrip`
  - `rate_meter_window`: record 100 at t=0 and 100 at t=4s; at t=4s the
    rate is 40/s, and at t=10s it is 0
  - `engine_ingests_and_reports_status`: ports 0, HTTP JSON post, then
    `status().stats.spans == 1` and both OTLP listeners are `Listening`
  - `paused_engine_drops_but_accepts`
  - `headless_binary_starts_and_stops`: spawn the built binary with ports
    0, read 2 listener lines, send SIGTERM, expect exit status 0
- [ ] **Step 2: Watch them fail.** Run
  `cargo test -p loctel-engine -p loctel-app`. Expected: compile failure.
- [ ] **Step 3: Implement** to the interfaces.
- [ ] **Step 4: Watch them pass.** Run
  `cargo test -p loctel-engine -p loctel-app`. Expected: PASS.
- [ ] **Step 5: Docs.** Update `docs/architecture.md` with the data flow
  and the headless mode.
- [ ] **Step 6: Commit.** Message: `feat(engine): engine, settings, pause,
  rates and headless binary`.

### Task 8: Fake microservice mesh (`loctel-meshgen`)

**Files:**
- Create: `crates/loctel-meshgen/Cargo.toml` (deps: model,
  `opentelemetry-proto`, `tonic` (client), `reqwest` (rustls, gzip),
  `rand` + `rand_chacha`, `clap`, `tokio`, `anyhow`)
- Create: `crates/loctel-meshgen/src/{lib.rs, scenario.rs, checkout.rs, health.rs, huge.rs, log_flood.rs, send.rs, main.rs}`
  (bin `meshgen`)
- Test: `crates/loctel-meshgen/tests/scenarios.rs`
- Create: `docs/areas/testing.md`

**Interfaces:**
- Produces:
  - `enum Scenario { CheckoutFailure, CheckoutSuccess, HealthChecks { count: usize }, LateOutOfOrder, HugeTrace { spans: usize }, LogFlood { records: usize } }`
  - `struct Batch { pub traces: Option<ExportTraceServiceRequest>, pub logs: Option<ExportLogsServiceRequest>, pub delay_before: Duration }`
  - `struct Generated { pub batches: Vec<Batch>, pub trace_ids: Vec<TraceId> }`
  - `fn generate(s: &Scenario, seed: u64, base_unix_nano: u64) -> Generated`.
    Deterministic: the same inputs give identical bytes.
  - `enum Transport { Grpc(String), HttpProtobuf(String), HttpJson(String) }`
    (the endpoint base URL)
  - `async fn send(g: &Generated, t: &Transport, time_scale: f64) -> anyhow::Result<SendReport { spans: usize, logs: usize }>`.
    It sleeps `delay_before * time_scale`.
  - CLI: `meshgen --scenario <checkout-failure|checkout-success|health-checks|late-out-of-order|huge-trace|log-flood> --transport <grpc|http-protobuf|http-json> --endpoint URL [--repeat N] [--interval-ms N] [--seed N]`
- **`CheckoutFailure` is fixed content.** It must match the mockups,
  because tests and screenshots rely on it. Offsets are in milliseconds
  from `base_unix_nano`. Every service has a resource with `service.name`,
  `service.version = "1.4.0"` and `deployment.environment = "local"`.

| # | parent | service | kind | name | start | dur | status | key attributes |
|---|---|---|---|---|---|---|---|---|
| 1 | — | web-bff | Server | POST /api/checkout | 0 | 1840 | Error | http.route=/api/checkout, http.response.status_code=500 |
| 2 | 1 | web-bff | Internal | validate_token | 4 | 18 | Unset | |
| 3 | 1 | web-bff | Client | POST | 30 | 1780 | Error | server.address=orders-api |
| 4 | 3 | orders-api | Server | POST /orders | 34 | 1772 | Error | http.response.status_code=500 |
| 5 | 4 | orders-api | Client | SELECT carts | 40 | 18 | Unset | db.system.name=postgresql |
| 6 | 4 | orders-api | Client | GET | 62 | 128 | Unset | server.address=inventory-api |
| 7 | 6 | inventory-api | Server | GET /stock/{sku} | 66 | 120 | Unset | |
| 8 | 7 | inventory-api | Client | MGET | 70 | 4 | Unset | db.system.name=redis |
| 9 | 4 | orders-api | Client | POST | 196 | 1594 | Error | server.address=payments-api |
| 10 | 9 | payments-api | Server | POST /charges | 200 | 1586 | Error | order.id=8812; event `exception` at 1782 (below) |
| 11 | 10 | payments-api | Client | SELECT payment_methods | 206 | 24 | Unset | db.system.name=postgresql |
| 12 | 10 | payments-api | Client | POST | 236 | 600 | Error | server.address=card-gateway, error.type=timeout |
| 13 | 10 | payments-api | Client | POST | 840 | 600 | Error | server.address=card-gateway, http.request.resend_count=1 |
| 14 | 4 | orders-api | Producer | order.failed publish | 1794 | 6 | Unset | messaging.system=rabbitmq |
| 15 | 14 | notifications-worker | Consumer | order.failed process | 1802 | 14 | Unset | messaging.system=rabbitmq |

  The exception event on span 10 has:
  - `exception.type=Payments.Api.Gateway.CardGatewayTimeoutException`
  - `exception.message=card-gateway did not respond after 2 attempts`
  - `exception.stacktrace` = the three `at …` lines in the Trace mockup

  Logs (time offset, level, span, body):

  | offset | level | span | body |
  |---|---|---|---|
  | 10 | INF | 4 | "Created order 8812 for cart c-5521" |
  | 190 | INF | 4 | "Requesting payment for order 8812" |
  | 210 | INF | 10 | "Charging card for order 8812, amount 129.00 EUR" |
  | 836 | WRN | 10 | "card-gateway did not answer within 600 ms, retrying (attempt 2 of 2)" |
  | 1440 | ERR | 10 | "card-gateway did not answer within 600 ms" |
  | 1782 | ERR | 10 | "Charge failed for order 8812: CardGatewayTimeoutException: card-gateway did not respond after 2 attempts" |
  | 1800 | ERR | 4 | "Checkout failed for order 8812: payments-api returned 500" |

  The last ERR on span 10 also carries the `exception.*` attributes. The
  expected failed-in span is #12 (payments-api · POST, peer
  `card-gateway`), with `more == 1`. The trace exception comes from #10
  (§9.4 and Task 4).
- The other scenarios:
  - **`CheckoutSuccess`:** the same tree with every status Unset, so 14
    spans, with these changes:
    - no span 13, and no `error.type` on span 12
    - no exception
    - spans 14 and 15 named `order.created publish` and
      `order.created process`
    - INF logs only
  - **`LateOutOfOrder`:** a `CheckoutSuccess` trace sent in 3 batches:
    - the spans at depth 3 and deeper (the leaves) first
    - then the middle spans, after 50 ms
    - then the root (#1), after another 50 ms
  - **`HealthChecks { count }`:** per service, `count` Server spans
    `GET /health` with `url.path=/health`, plus an INF log
    `"GET /health 200"` each.
  - **`HugeTrace { spans }`:** one trace, depth ≤ 8, deterministic fan-out,
    services rotating among the 4 app services, with exactly `spans` spans.
  - **`LogFlood { records }`:** INF and DBG logs spread across the services,
    with no trace context.

- [ ] **Step 1: Write the failing tests:**
  - `checkout_failure_is_deterministic`
  - `checkout_failure_shape`: 15 spans; the parent links as in the table;
    span 10 has the exception event; 7 logs, 4 of them on span 10
  - `late_out_of_order_sends_root_last`: 3 batches; the root is only in
    the last one; the delays are as specified
  - `huge_trace_has_requested_span_count`: 100,000 spans, a single trace
    ID, every parent ID present
  - `health_checks_are_get_health_server_spans`
- [ ] **Step 2: Watch them fail.** Run `cargo test -p loctel-meshgen`.
  Expected: compile failure.
- [ ] **Step 3: Implement.** IDs come from a ChaCha RNG seeded with
  `seed`.
- [ ] **Step 4: Watch them pass.** Run `cargo test -p loctel-meshgen`.
  Expected: PASS.
- [ ] **Step 5: Smoke test by hand.**

  ```bash
  cargo run -p loctel-app -- --headless --completeness-ms 500 &
  cargo run -p loctel-meshgen -- --scenario checkout-failure --transport http-json --endpoint http://127.0.0.1:4318
  ```

  Expected: meshgen reports 15 spans and 7 logs sent. Kill loctel
  afterwards.
- [ ] **Step 6: Docs.** Write `docs/areas/testing.md` covering the
  scenarios, how tests use them, and that `CheckoutFailure` is frozen (any
  change requires updating the screenshots and the e2e expectations).
- [ ] **Step 7: Commit.** Message: `feat(meshgen): deterministic fake mesh
  scenarios and CLI`.

### Task 9: MCP server (`loctel-mcp`) and engine wiring

**Files:**
- Create: `crates/loctel-mcp/Cargo.toml`. Deps:
  - `rmcp = "3.6"`, with the server, macros and streamable-HTTP-server
    features, plus client features under dev-dependencies (use whatever
    features rmcp 3.6 names for these)
  - `axum`, `serde`, `serde_json`, `schemars`, `chrono`, `loctel-store`,
    `loctel-ingest` (for `ListenerStatus`)
- Create: `crates/loctel-mcp/src/{lib.rs, server.rs, tools.rs, dto.rs, origin.rs, since.rs}`
- Modify: `crates/loctel-engine/src/lib.rs`. Start MCP on
  `bind 127.0.0.1:mcp_port` (always loopback), add the `"mcp"` listener to
  `EngineStatus.listeners`, and make `mcp_url()` return
  `http://127.0.0.1:{port}/mcp`.
- Test: `crates/loctel-mcp/tests/{tools.rs, http.rs}`
- Create: `docs/areas/mcp.md`, `.claude/skills/add-mcp-tool/SKILL.md`

**Interfaces:**
- Consumes: the `Store` APIs from Tasks 3–5, and `ListenerStatus`.
- Produces:
  - `trait StatusSource: Send + Sync + 'static { fn status(&self) -> StatusDto; }`
    (the engine implements it)
  - `async fn serve(addr: SocketAddr, store: Store, status: Arc<dyn StatusSource>) -> (ListenerStatus /* name "mcp" */, McpShutdown)`
  - `struct LoctelTools`: an rmcp tool router whose methods are callable
    directly in tests:
    `LoctelTools::new(store, status)` and
    `async fn call(&self, name: &str, args: serde_json::Value) -> rmcp CallToolResult`.
  - The `Origin` check: a request whose `Origin` header has a host other
    than `localhost`, `127.0.0.1` or `[::1]` gets HTTP 403. A request with
    no `Origin` is allowed.
  - `since`: accepts a marker (decimal unix nanoseconds as a string) or an
    RFC 3339 timestamp. Anything else is a tool error.

**The tools.** Names, arguments and limits are fixed. Every tool returns
`structuredContent` matching its output schema, plus the same JSON as text.

| tool | arguments | returns |
|---|---|---|
| `get_status` | — | `{paused, listeners:[{name,address,state,error?}], spans_per_sec, logs_per_sec, traces_held, spans_held, logs_held}` |
| `get_marker` | — | `{marker:"<unix nanos>", time:"<RFC 3339>"}` |
| `list_services` | `since?` | `[{name, requests, errors, error_rate, last_seen}]` |
| `get_service_map` | `since?` | `{nodes:[{id,kind,requests,errors}], edges:[{from,to,calls,errors}]}` |
| `search_traces` | `since?, service?, errors_only?, min_duration_ms?, limit?=20 (max 100), cursor?` | `{traces:[TraceSummaryDto], next_cursor}` |
| `recent_failures` | `since?, limit?=5 (max 20)` | `{failed:[{trace: TraceSummaryDto, failed_in_span: SpanBriefDto, exception?:{type,message,stacktrace}, error_logs:[LogDto] (the trace's error-or-worse logs, oldest first, max 10)}], errors_inside:[TraceSummaryDto]}` |
| `get_trace` | `trace_id, detail?="summary"\|"full", limit?=200 (max 2000), cursor?` | `{summary: TraceSummaryDto, spans:[SpanBriefDto], omitted, next_cursor}` |
| `get_span` | `trace_id, span_id` | `{span:{…all fields, attributes, events, resource, exception?}, logs:[LogDto] (max 50)}` |
| `search_logs` | `since?, service?, min_level?, contains?, trace_id?, limit?=50 (max 500), cursor?` | `{logs:[LogDto], next_cursor}` |

The DTOs:
- **`TraceSummaryDto`:** `trace_id, root_name, root_service, start (RFC 3339), duration_ms, span_count, services, status ("ok"|"errors_inside"|"failed"), failed_in?:{service, span_name, peer?, span_id, more}, first_exception?:{type,message}, complete`
- **`SpanBriefDto`:** `span_id, parent_span_id?, name, service, kind, start_offset_ms, duration_ms, status, exception_type?`
- **`LogDto`:** `time, level, service, message, trace_id?, span_id?`

Truncation and sizing:
- Messages and stack traces are cut at 2,000 characters, with a
  `"…(truncated)"` suffix.
- A `summary`-detail `get_trace` returns the spans on the path from the
  root to the failed-in span, plus every span at depth ≤ 2, capped at
  `limit`. `omitted` counts the rest.
- An unknown trace or span returns a tool result with `isError: true` and
  the text `"trace not found"` or `"span not found"`.

- [ ] **Step 1: Write the failing tests.**

  `tests/tools.rs`. The fixture loads `meshgen::generate(CheckoutFailure)`
  into a `Store` through `loctel_ingest::convert`, with a `ManualClock`
  advanced past the quiet period.

  ```rust
  #[tokio::test] async fn recent_failures_names_failed_in_and_exception() {
      let r = call(&tools, "recent_failures", json!({})).await;
      let f = &r["failed"][0];
      assert_eq!(f["trace"]["failed_in"]["service"], "payments-api");
      assert_eq!(f["trace"]["failed_in"]["peer"], "card-gateway");
      assert_eq!(f["trace"]["failed_in"]["more"], 1);
      assert_eq!(f["exception"]["type"], "Payments.Api.Gateway.CardGatewayTimeoutException");
      assert!(f["error_logs"].as_array().unwrap().len() >= 2);
  }
  ```

  The other tool tests:
  - `search_traces_caps_limit_at_100`
  - `since_marker_filters_older`
  - `get_trace_summary_collapses_large_trace`: a `HugeTrace` of 10,000
    spans gives `spans.len() <= 200`, `omitted > 0`, and
    `next_cursor.is_some()`
  - `get_trace_full_pages_with_cursor`
  - `get_span_includes_logs_and_exception`
  - `search_logs_filters_level_and_text`
  - `unknown_trace_is_tool_error`
  - `results_have_structured_content_and_text`

  `tests/http.rs`:
  - `serves_streamable_http_on_loopback`: an rmcp client over HTTP lists
    exactly the 9 tool names
  - `rejects_foreign_origin`: a raw POST with
    `Origin: https://evil.example` gets 403
  - `allows_localhost_origin`
- [ ] **Step 2: Watch them fail.** Run `cargo test -p loctel-mcp`.
  Expected: compile failure.
- [ ] **Step 3: Implement.** Use rmcp's `StreamableHttpService` nested at
  `/mcp` in an axum `Router`, with the Origin check as middleware. Wire it
  into `Engine::start`. `loctel-engine` dev-depends on `loctel-mcp` for the
  test below.
- [ ] **Step 4: Watch them pass.** Run `cargo test -p loctel-mcp -p loctel-engine`.
  Expected: PASS. Also add the engine test `engine_status_lists_mcp_listener`.
- [ ] **Step 5: Docs.**
  - `docs/areas/mcp.md`: the tools table above, the sizing rules, and how
    to connect Claude Code:
    `claude mcp add --transport http loctel http://127.0.0.1:4319/mcp`
  - `.claude/skills/add-mcp-tool/SKILL.md`: the steps for adding a tool:
    DTO, a tool method, a test in `tests/tools.rs`, the name list in
    `tests/http.rs`, and the docs table.
- [ ] **Step 6: Commit.** Message: `feat(mcp): read-only MCP server with
  trace, log and map tools`.

### Task 10: End-to-end tests (`loctel-e2e`)

**Files:**
- Create: `crates/loctel-e2e/Cargo.toml` (`publish = false`; dev-deps:
  engine, meshgen, rmcp client, tokio)
- Create: `crates/loctel-e2e/src/lib.rs` (a harness)
- Create: `crates/loctel-e2e/tests/{agent_flow.rs, transports.rs, assembly.rs, map.rs, pause.rs}`
- Modify: `docs/areas/testing.md`

**Interfaces:**
- Produces the harness: `struct Harness`, with
  - `start(completeness: Duration) -> Harness`: an engine on ports 0, with
    the store's `completeness_quiet = completeness`
  - `otlp(&self, kind) -> Transport`
  - `mcp(&self) -> McpClient`
  - `async fn tool(&self, name, args) -> serde_json::Value` (calls through
    real Streamable HTTP)
  - `async fn wait_complete(&self, trace_id)`: polls `get_trace` until
    `complete`, with a 10 s timeout

- [ ] **Step 1: Write the failing tests.** Each uses
  `Harness::start(Duration::from_millis(200))`:
  - `checkout_failure_over_each_transport`: for Grpc, HttpProtobuf and
    HttpJson in turn, send `CheckoutFailure`, then `recent_failures`
    names failed-in `payments-api`, peer `card-gateway`, and the exception
    type as in Task 9.
  - `late_spans_assemble_into_one_trace`: `LateOutOfOrder` at
    `time_scale 1.0` gives a `get_trace` with 14 spans, `complete == true`,
    no orphans, and the root present.
  - `service_map_matches_mockup_topology`:
    - the nodes include `web-bff`, `orders-api`, `inventory-api`,
      `payments-api`, `notifications-worker` (Service), `postgresql` and
      `redis` (Database), `rabbitmq` (Queue) and `card-gateway` (External)
    - edge `orders-api→payments-api` has `errors >= 1`
    - there's no direct edge `orders-api→notifications-worker`
  - `agent_flow_since_marker`: `get_marker`, then `CheckoutSuccess`, then
    `CheckoutFailure`. `recent_failures(since)` gives exactly 1 failed and
    0 errors-inside. `search_traces(since)` gives 2.
  - `paused_engine_stores_nothing`
  - `health_checks_are_visible`: until M2's rules exist, they're simply
    stored. Assert that `search_traces(service="orders-api")` includes
    `GET /health`.
- [ ] **Step 2: Watch them fail.** Run `cargo test -p loctel-e2e`.
  Expected: compile failure (the harness is missing).
- [ ] **Step 3: Implement the harness.** If an assertion fails because of a
  real bug elsewhere, fix it there, with a regression test in that crate.
- [ ] **Step 4: Watch them pass.** Run `cargo test -p loctel-e2e`.
  Expected: PASS, with the whole suite under 60 s.
- [ ] **Step 5: Commit.** Message: `test(e2e): real OTLP to MCP end-to-end
  coverage`.

### Task 11: UI crate, status view, headless screenshot harness, Linux window

**Files:**
- Create: `crates/loctel-ui/Cargo.toml`. Deps:
  - `gpui-pre = "=0.3.9"`
  - `gpui-pre-platform = "=0.3.9"`
  - `gpui-pre-wgpu = "=0.3.9"`
  - `loctel-engine`
  - `image`

  Add a feature `screenshot` that enables headless rendering and is used
  by the tests.
- Create: `crates/loctel-ui/src/{lib.rs, theme.rs, status_view.rs, menu_text.rs, testing.rs}`
- Create: `crates/loctel-ui/tests/{screenshots.rs, menu_text.rs}`,
  `crates/loctel-ui/tests/snapshots/status_view_dark.png` (generated)
- Create: `xtask/Cargo.toml`, `xtask/src/main.rs` (subcommands
  `screenshots [--update]` and `run-shot`), and `.cargo/config.toml` alias
  `xtask = "run -p xtask --"`
- Modify: `crates/loctel-app/src/main.rs`. Without `--headless` on Linux,
  open one GPUI window showing `StatusView`, refreshed every 1 s from
  `EngineHandle::status()`.
- Create: `.claude/skills/run-and-screenshot/SKILL.md`,
  `.claude/skills/update-screenshots/SKILL.md`, `docs/areas/ui.md`

**Interfaces:**
- Produces:
  - `struct Theme`, with `fn dark() -> Theme` and `fn light() -> Theme`.
    The dark tokens come from the mockups:
    - bg `#15161a`, panel `#1a1b20`, text `#ececf1`, text_secondary
      `#a3a3ad`, text_tertiary `#8b8b95`
    - accent `#4c9dff`, error `#ff6b63`, error_text `#ff8a84`, warn
      `#f5c26b`, ok `#34c759`
    - service palette `["#4c9dff","#3cc8b4","#f0a04b","#b18cff","#f07fb0"]`

    `light()` is the builder's choice, in the same spirit.
  - `struct StatusModel { pub paused: bool, pub listeners: Vec<ListenerLine>, pub spans_per_sec: f64, pub logs_per_sec: f64, pub traces_held: usize, pub logs_held: usize }`,
    with `struct ListenerLine { pub name: String, pub address: String, pub ok: bool, pub error: Option<String> }`
    and `fn from_status(s: &EngineStatus) -> StatusModel`.
  - `struct StatusView` (GPUI `Render`), with `fn new(model: StatusModel, theme: Theme)`
    and `fn set_model(&mut self, model, cx)`.
  - `menu_text::status_line(s: &StatusModel) -> String`, with exact
    strings:
    - `"Receiving · 412 spans/s · 1.2k logs/s"`
    - `"Paused"`
    - `"Port problem: otlp-http (address in use)"`

    A failed listener wins over paused, and paused wins over receiving.
    The reason in brackets is `address in use` when the OS error contains
    "in use", otherwise the raw error cut to 40 characters.
  - `menu_text::format_rate(f64) -> String`: `412` gives `"412"`, `1234`
    gives `"1.2k"`, `0.4` gives `"0"`.
  - `testing::render_png(size: (u32, u32), build: impl FnOnce(&mut Window, &mut App) -> AnyView) -> image::RgbaImage`.
    It uses `HeadlessAppContext::with_platform(…, gpui_platform::current_headless_renderer)`
    and `capture_screenshot`.
  - `testing::assert_snapshot(name: &str, img: &RgbaImage)`:
    - compares with `tests/snapshots/{name}.png` pixel-exactly
    - `UPDATE_SNAPSHOTS=1` writes the reference instead
    - on a mismatch, writes `target/screenshot-diffs/{name}.actual.png`
      and `{name}.diff.png`, then panics

- [ ] **Step 1: Write the failing tests.**
  - `tests/menu_text.rs`: `status_line_receiving`, `status_line_paused`,
    `status_line_port_problem`, `format_rate_cases`.
  - `tests/screenshots.rs`:
    - `status_view_dark_matches_snapshot`: a fixed model with 3 listeners,
      one of them failed, at 900×600
    - `render_is_deterministic`: render twice and expect equal bytes
- [ ] **Step 2: Watch them fail.** Run
  `cargo test -p loctel-ui --features screenshot`. Expected: compile
  failure.
- [ ] **Step 3: Implement.** Write the view, the menu text and the
  harness. Generate the reference with
  `UPDATE_SNAPSHOTS=1 cargo test -p loctel-ui --features screenshot`, then
  **open the PNG with the Read tool and check it shows the listener lines
  legibly** before committing it.
- [ ] **Step 4: Watch them pass.** Run
  `cargo test -p loctel-ui --features screenshot`. Expected: PASS. Make sure
  the workspace test in CI enables the feature (via `[dev-dependencies]`
  self-feature, or `cargo test --workspace --all-features` in `ci.yml`).
  Then run `cargo test --workspace` and expect it to pass.
- [ ] **Step 5: Real-window screenshot.** `cargo xtask run-shot` builds
  `loctel`, runs it under `xvfb-run` with ports 0, captures the window with
  `import -window root`, and writes `target/run-shot.png`. Run it and open
  the PNG with the Read tool. The status window should be visible. If the
  window is black, add an `xdotool windowmove` nudge (spike gotcha §10.5).
- [ ] **Step 6: Docs and skills.**
  - `docs/areas/ui.md`: the theme tokens and the screenshot workflow.
  - `run-and-screenshot`: how to run the app headless or under Xvfb, send
    meshgen data, and capture.
  - `update-screenshots`: when references may change, the rule that the
    agent must view the new PNGs, and how to read CI diff artifacts.
- [ ] **Step 7: Commit.** Message: `feat(ui): status view, headless
  screenshot harness, Linux window`.

### Task 12: macOS shell: status item menu, accessory app, start at login

**Files:**
- Create: `crates/loctel-macos/Cargo.toml`. All dependencies sit under
  `[target.'cfg(target_os = "macos")'.dependencies]`:
  - `gpui-pre`
  - `gpui-tray = { version = "0.2", default-features = false, features = [<the gpui-pre backend feature>] }`
  - `objc2`, `objc2-app-kit`, `objc2-foundation`
  - `objc2-service-management = "0.3.2"`
  - `loctel-engine`, `loctel-ui` (for `menu_text`)
- Create: `crates/loctel-macos/src/{lib.rs, status_item.rs, login_item.rs}`.
  The crate is empty on other OSes.
- Modify: `crates/loctel-app/src/main.rs`. Without `--headless` on macOS:
  1. set `ActivationPolicy::Accessory` and `QuitMode` explicit
  2. call `loctel_macos::install(cx, engine, settings_path)`
  3. open no window

  (The main window comes in M3.)
- Create: `docs/areas/macos.md`,
  `docs/decisions/0002-macos-check-from-linux.md`

**Interfaces:**
- Produces (macOS only):
  - `fn install(cx: &mut gpui::App, engine: EngineHandle, settings_path: PathBuf)`
  - `login_item::is_enabled() -> bool` and
    `login_item::set_enabled(on: bool) -> anyhow::Result<()>`, both via
    `SMAppService::mainAppService`.
- The menu, in this order:
  1. a disabled header with `status_line(...)` (from Task 11), refreshed
     every 2 s
  2. `Copy MCP address` (copies `engine.mcp_url()`)
  3. `Pause receiving`, or `Resume receiving` while paused
  4. a separator
  5. `Start at login`, with a checkmark from `is_enabled()`
  6. a separator
  7. `Quit loctel` (⌘Q), which calls `engine.shutdown()` and then quits
- The icon: a template image of the three-node glyph from the mockups,
  embedded as PNG at 18×18 and 36×36 (@2x).
- First launch: if `!settings.first_launch_done`, call
  `set_enabled(true)` (log any error), set `first_launch_done = true`, and
  save the settings.

- [ ] **Step 1: Try to type-check the macOS crate from Linux.** Run
  `cargo check --target aarch64-apple-darwin -p loctel-macos -p loctel-app`.
  - If build scripts fail (for example `xcrun` missing for Metal shaders),
    try GPUI's `runtime_shaders` feature.
  - Record exactly what worked or failed in
    `docs/decisions/0002-macos-check-from-linux.md`.
  - If it works, add this check as a step in `ci.yml`.
  - If it can't work, add a separate `macos-check` job to `ci.yml` on
    `macos-26` that only runs on PRs touching `crates/loctel-macos/**` or
    `crates/loctel-app/**` (`paths` filter), running `cargo check`. This is
    the spec §10.3 fallback.
- [ ] **Step 2: Implement.** Follow the interfaces. Keep all
  `objc2`/`unsafe` code inside `loctel-macos`. The header text logic is
  already tested on Linux (Task 11 `menu_text`).
- [ ] **Step 3: Run the check.** Repeat Step 1's check (if it works from
  Linux), plus `cargo clippy --workspace --all-targets -- -D warnings` and
  `cargo test --workspace`. Expected: PASS.
- [ ] **Step 4: Docs.** Write `docs/areas/macos.md`:
  - the menu, the login item and the activation policy
  - a **"Unverified until the M1 Mac check"** list: menu bar icon, no
    Dock icon, the menu actions, start at login
- [ ] **Step 5: Commit.** Message: `feat(macos): menu bar status item,
  accessory app, start at login`.

### Task 13: Release pipeline, M1 Mac checklist, docs pass

**Files:**
- Create: `.github/workflows/release.yml`, `script/bundle-mac.sh`
- Modify: `crates/loctel-app/Cargo.toml`. Add `[package.metadata.bundle]`:
  - name `loctel`
  - identifier `io.github.ldellisola.loctel`
  - `osx_minimum_system_version = "26.0"`
  - icon (generated from the glyph)
  - `osx_info_plist_exts` setting `LSUIElement = true`
- Modify: `xtask/src/main.rs`. Add `next-version`, `release-notes` and
  `mcp-smoke --url URL`.
- Test: `xtask/src/version.rs` and `xtask/src/notes.rs` (unit tests inline)
- Create: `docs/milestones/mac-checklist.md`, `docs/areas/release.md`,
  `.claude/skills/investigate-ci/SKILL.md`
- Modify: `AGENTS.md`, `docs/architecture.md` (final pass for M1)

**Interfaces:**
- Produces:
  - `xtask::version::next_version(tags: &[String]) -> String`: the highest
    `vX.Y.Z` with its patch bumped; with no tags, `v0.1.0`.
  - `xtask::notes::release_notes(subjects: &[String], checklist_md: &str) -> String`:
    a "Changes" list, then a "Check on your Mac" section holding the
    checklist verbatim, then an "Installing" section. The installing
    section has the Open Anyway steps and
    `xattr -dr com.apple.quarantine /Applications/loctel.app`.
  - `cargo xtask mcp-smoke --url U`: connects with the rmcp client and
    expects the tool `recent_failures` to be listed. Exits non-zero
    otherwise.
- `release.yml` triggers on `push` to `main` only, with
  `permissions: contents: write` and
  `concurrency: release, cancel-in-progress: false`. It runs on `macos-26`:
  1. checkout with `fetch-depth: 0`
  2. the toolchain from `rust-toolchain.toml`
  3. `Swatinem/rust-cache@v2`
  4. `cargo build --release -p loctel-app`
  5. `script/bundle-mac.sh "$VERSION"`. It:
     - builds `loctel.app` with `cargo-bundle` (installed with
       `cargo install cargo-bundle --version 0.12.0 --locked`)
     - sets `CFBundleShortVersionString` to the version without the `v`
     - runs `codesign --force --deep --sign - loctel.app`
     - stages `loctel.app` plus a symlink `Applications -> /Applications`
     - runs `hdiutil create -volname loctel -srcfolder <stage> -ov -format UDZO loctel-$VERSION.dmg`
  6. a smoke test:
     - run `loctel.app/Contents/MacOS/loctel --headless --grpc-port 24317 --http-port 24318 --mcp-port 24319 &`
     - `curl -sf -X POST -H 'content-type: application/json' --data @crates/loctel-e2e/fixtures/one-span.json http://127.0.0.1:24318/v1/traces`
     - `cargo xtask mcp-smoke --url http://127.0.0.1:24319/mcp`
     - kill the process
  7. `VERSION=$(cargo xtask next-version)` (computed before step 5)
  8. `gh release create "$VERSION" loctel-$VERSION.dmg --title "loctel $VERSION" --notes-file notes.md`
     (with `GH_TOKEN: ${{ github.token }}`),
     where `notes.md` comes from `cargo xtask release-notes` (subjects
     since the previous tag, plus `docs/milestones/mac-checklist.md`)

- [ ] **Step 1: Write the failing xtask tests.**
  - `next_version_from_no_tags`: gives `v0.1.0`
  - `next_version_bumps_patch_of_highest`: `[v0.1.4, v0.1.10, v0.0.9]`
    gives `v0.1.11`
  - `notes_contain_checklist_and_install_steps`
- [ ] **Step 2: Watch them fail.** Run `cargo test -p xtask`. Expected:
  compile failure.
- [ ] **Step 3: Implement the xtask pieces.** Also add
  `crates/loctel-e2e/fixtures/one-span.json`, a valid OTLP/JSON trace
  request.
- [ ] **Step 4: Watch them pass.** Run `cargo test -p xtask`. Expected:
  PASS.
- [ ] **Step 5: Write the release files.** Write `release.yml` and
  `script/bundle-mac.sh` (`set -euo pipefail`; shellcheck-clean), then run:

  ```bash
  actionlint .github/workflows/*.yml
  shellcheck script/*.sh
  ```

  Expected: no output.
- [ ] **Step 6: Write `docs/milestones/mac-checklist.md` for M1.** Each
  item gets the exact steps and what "right" looks like:
  1. Install from the `.dmg`, then use Open Anyway. loctel starts with a
     menu bar icon and no Dock icon.
  2. The menu header shows "Receiving · 0 spans/s · 0 logs/s".
  3. A service on the Mac with
     `OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317` shows rates in the
     header.
  4. A Docker container with
     `OTEL_EXPORTER_OTLP_ENDPOINT=http://host.docker.internal:4317` does
     too. If it doesn't, report it: the bind address changes (§4).
  5. Run `claude mcp add --transport http loctel http://127.0.0.1:4319/mcp`,
     make a request fail, and ask Claude "why did my last request fail?".
     It should name the service and the exception.
  6. Pause and Resume work.
  7. Start at login: log out and in, and loctel is running.
  8. Quit loctel stops it, and the ports are freed.
- [ ] **Step 7: Docs pass.**
  - `AGENTS.md`: the full commands section, including xtask and meshgen;
    the skills list; the "how a change ships" loop (§10.3): branch, PR,
    CI green, squash merge, watch the release job and fix it if red.
  - `docs/areas/release.md`: the version and notes logic, and the
    Gatekeeper steps.
  - `.claude/skills/investigate-ci/SKILL.md`: find the failing job,
    reproduce locally with the same command, and for screenshot diffs read
    the artifact PNGs.
  - `docs/architecture.md`: the final M1 crate map and data-flow diagram
    (ASCII).
- [ ] **Step 8: Run the full gate.**

  ```bash
  cargo fmt --check
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  ```

  Expected: PASS.
- [ ] **Step 9: Commit.** Message: `ci: macOS release pipeline with
  unsigned dmg, M1 Mac checklist, docs`.

---

## After the last task (M1 ship)

1. **Whole-branch review** by a fresh reviewer against the spec and this
   plan. Fix what it finds.
2. **Open the PR** to `main` with the M1 summary and the Mac checklist.
   Wait for the Linux CI to go green. Fix and push until it does.
3. **Squash-merge.** Watch `release.yml` on `macos-26` until it publishes
   `v0.1.0` with the `.dmg`. If it fails, fix it in a follow-up PR until a
   release exists.
4. **Tell the user** that the M1 release is ready for the Mac checklist.
   M2 starts by writing `docs/superpowers/plans/<date>-loctel-m2-query-rules.md`
   from the spec and the roadmap.
