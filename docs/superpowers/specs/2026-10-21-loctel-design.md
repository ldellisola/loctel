# loctel: design

Date: 2026-10-21
Status: draft for approval

loctel is a native macOS menu bar app that receives OpenTelemetry traces and
logs from services running on the developer's machine, shows them in a fast
window (service map, traces, logs), and exposes the same data to AI agents
over MCP.

Throughout this document, **"(my call)"** marks a decision the design author
made that the user did not explicitly state. Everything else was decided with
the user.

---

## 1. Intent

### Who it is for and why it exists

The user is a software engineer who works on many projects, almost all of
them microservices instrumented with OpenTelemetry. While debugging, services
run both in Docker and directly on the Mac ("bare metal"). In staging and
production the user relies on Grafana (Tempo for traces, Loki for logs), but
there is nothing comparable locally.

Today they use the OpenTelemetry plugin for JetBrains Rider, which falls short:

- Sometimes a whole trace does not appear.
- Searching and filtering is plain text matching only.
- Rider has to be open.

The moment loctel exists for: **a request fails somewhere in the mesh, and the
user needs to know which service broke and what the exception was.** Last
time, the user could not see which endpoints were called or where the chain
broke, and ended up searching logs by hand to find the exception. loctel
should make that a few seconds' work.

The second user is an **AI coding agent**, through MCP:

1. "Find why my last request failed, and in which service."
2. While coding: the agent changes code, runs the service, sends a request,
   then asks loctel whether it worked (no errors, expected spans, no
   exceptions in the logs).

### How it gets built

loctel is built and maintained **only by AI agents**. Each feature or bug
fix starts as a fresh chat with no memory of earlier work. No human reads the
code or approves pull requests. The repository itself must therefore carry
all context, and the automated checks are the only quality gate (section 10).

### What success looks like

- The user opens loctel from the menu bar after a failed request and, from
  the first screen, sees which service failed. One click takes them to the
  exception and the logs around it.
- Huge traces and millions of log lines scroll without stutter.
- Health-check noise never reaches the screen.
- A coding agent can answer "did my request work?" through MCP without the
  user doing anything.
- A new agent session can pick up any feature or bug from a one-paragraph
  request, and ship it without breaking the app.

---

## 2. Goals, non-goals, anti-goals

### Goals

- Receive OTLP traces and logs from Docker and bare-metal services with no
  extra setup beyond pointing the exporter at loctel.
- Always show complete traces, assembled as their spans arrive in any order.
- Advanced search: TraceQL for traces (including structural queries such as
  "spans named X with an ancestor from service Y") and LogQL for logs.
- Discard rules that drop matching data at ingest, toggleable, kept across
  restarts.
- A realtime service map.
- A local MCP server (latest spec, no auth) for LLM agents.
- High FPS and very fast startup. Stays smooth on huge traces and long logs.
- Native macOS feel using Liquid Glass where the UI framework allows it
  (section 9.3).
- Easy for AI agents to extend and fix safely.

### Non-goals (explicitly out of scope)

- **Metrics** (OTLP metrics are not ingested).
- **Persistence across restarts.** Data is gone when loctel quits. Only
  settings and discard rules persist.
- **Separating projects.** Everything is one big mesh. (If wanted later,
  services could tag themselves with `service.namespace`.)
- Remote access, multiple users, authentication.
- Linux or Windows as products. Linux exists only so agents can build, run
  and screenshot the app (section 10). Intel Macs.
- Code signing and notarization. Unsigned builds are acceptable.
- Auto-update. The user downloads each release manually.
- TraceQL/LogQL metrics queries (`rate()`, `count_over_time()`, and so on)
  in the first version.

### Anti-goals (failure even if it "works")

- A trace shown half-assembled and never filling in, or late spans silently
  lost. This is the Rider plugin's flaw, and loctel must not repeat it.
- Visible jank: dropped frames while scrolling, a slow-to-appear window, or
  the UI blocking on a query or on ingest.
- MCP responses that flood an LLM's context with huge payloads.
- Anything that needs the human to read code, review a PR, or fix a broken
  main branch.
- A change merged without the agent having proved it works (tests, and
  screenshots for UI changes).

---

## 3. Core flows

### 3.1 Human: "my request failed, where?"

1. The user clicks the menu bar icon and chooses **Open loctel** (⌘O).
2. The main window opens on **Map & traces** (Home A in the mockups). The
   service map shows the failing service in red, with its error rate.
3. The user clicks the failing service. The trace table below filters to it
   (the TraceQL bar updates to match). The **Failed in** column names the
   service and span where each trace broke.
4. The user clicks a trace. The waterfall opens with the error path marked,
   and selects the failing span. The right panel shows the exception, its
   stack trace and the attributes, and the span's logs appear under the
   waterfall.
5. From a log line, the user can jump to the surrounding lines or turn it
   into a discard rule.

### 3.2 Agent via MCP

1. The agent calls a tool to get a "since" marker (the current time).
2. It sends its request to the service under test.
3. It asks loctel for traces, errors and logs since the marker. It gets short
   summaries first: did anything fail, where, and with which exception. Then
   it drills into one trace or span when needed.

---

## 4. Ingest

- **Endpoints:** standard OTLP.
  - gRPC on port **4317**.
  - HTTP on port **4318**, at `/v1/traces` and `/v1/logs`, accepting both
    protobuf and JSON.
  - gzip compression must be supported. Request size limits must be raised
    well above the library defaults (4 MiB for tonic, 2 MB for axum).
  - Respond with `partial_success` where appropriate.
  - OTLP/JSON quirks apply: trace and span IDs are hex, int64 values are
    strings, and enums are integers.
  - Metrics requests (`/v1/metrics` and the gRPC metrics service) are
    accepted and dropped, so exporters don't log errors (my call).
- **Reachability:** services on the Mac and in Docker containers (reaching
  the host as `host.docker.internal` under Docker Desktop for Mac) must both
  work without any setup beyond the exporter endpoint.
  - Prefer binding to loopback if that satisfies Docker Desktop. Otherwise
    bind to all interfaces.
  - The builder must verify this on a real Mac and record the result in the
    decision log.
- **Ports** are configurable in Settings. If a port is taken, loctel keeps
  the other endpoints running and shows the problem in the menu bar popover
  and the sidebar status box (my call).
- **Order and lateness:** spans of one trace arrive from several services, in
  batches, in any order, children before parents.
  - Spans join their trace whenever they arrive.
  - A span whose parent hasn't arrived is shown under a "missing parent"
    placeholder until the parent shows up.
  - A trace counts as **complete** once no new span has arrived for it for
    10 seconds. The trace view shows this state, and an open trace keeps
    updating live either way.
- **Logs** are linked to spans through `trace_id` and `span_id` when present.
  Logs without trace context are still stored and searchable.
- **Exceptions** come from span events named `exception` (semantic
  conventions `exception.type`, `exception.message`, `exception.stacktrace`)
  and from log records carrying `exception.*` attributes.
- **Throughput target:** sustain **50,000 spans plus log records per second**
  with discard rules active, without dropping data or stalling the UI.
- **Pause receiving** (menu bar): endpoints stay up and keep answering
  success, but incoming data is dropped. Exporters therefore don't buffer and
  retry (my call).

## 5. Storage

- Nothing persists across restarts. Data lives in memory, and older data
  spills to temporary files on disk so memory stays bounded. The user said it
  "will ingest a lot of data".
  - Spill files live in a per-run temporary directory, deleted on quit.
  - A run that crashed leaves its directory behind; the next launch deletes
    it.
- **Disk cap:** default **4 GB**, configurable in Settings (my call). Over the
  cap, the oldest data is evicted first. Traces are evicted whole, never
  half (my call). The memory budget is the builder's call.
- **Clear all** empties everything immediately.
- The store must support all of these quickly at the target sizes:
  - appending at the ingest rate
  - fetching a whole trace by ID
  - TraceQL evaluation, including ancestor/descendant checks
  - LogQL label and line filtering
  - random access by row position, for scrolling 10 million log lines
  - computing service-map aggregates over a time window
- **Recommended approach** (from research; the builder may refine it):
  - time-partitioned columnar segments, with memory-mapped spill files
  - an index from trace ID to trace
  - nested-set numbering of each trace's spans, so ancestry checks are cheap
  - optionally tantivy for log-body search

  No off-the-shelf store fits well: DuckDB's full-text index doesn't update
  incrementally, SQLite is too slow for scans, and Arrow/DataFusion is heavy.

## 6. Querying

### 6.1 TraceQL (traces)

The syntax and semantics follow Grafana Tempo's TraceQL, so queries carry
over from Grafana. The authoritative grammar is Tempo's
`pkg/traceql/expr.y`. No mature Rust implementation exists, so loctel writes
its own parser and evaluator. The `imbh-lgtm` crate is worth reading as a
reference.

First version must support:

- **Span filters** `{ ... }` with:
  - intrinsics: `name`, `status`, `statusMessage`, `kind`, `duration`,
    `rootName`, `rootServiceName`, `traceDuration`
  - attributes scoped `span.`, `resource.`, or unscoped `.`
  - operators `= != > < >= <= =~ !~`, with `&&` and `||` inside a filter
- **Spanset operators:** `&&` and `||` between filters.
- **Structural operators:** `>` (child), `<` (parent), `>>` (descendant),
  `<<` (ancestor) and `~` (sibling), plus their negated forms if they fall
  out naturally.
- **Pipelines** with `count()` comparisons, e.g. `| count() > 3`.

Deferred: `select()`, `by()`, metrics functions.

### 6.2 LogQL (logs)

The syntax follows Grafana Loki's LogQL. The authoritative grammar is Loki's
`pkg/logql/syntax/syntax.y`. loctel writes its own parser and evaluator.

First version must support:

- stream selectors `{label op "value"}` with `= != =~ !~`
- line filters `|= != |~ !~`
- parsers `| json` and `| logfmt`
- label filters with string and numeric comparisons

Deferred: `line_format`, `label_format`, metrics queries.

**Label mapping**, matching how Loki ingests OTLP so queries carry over:

- Resource and log attributes become labels, with dots replaced by
  underscores (`service.name` → `service_name`).
- Severity becomes `level`, with values `trace`, `debug`, `info`, `warn`,
  `error` and `fatal`.
- The instrumentation scope name becomes `scope_name`.

### 6.3 Shared behaviour

- Syntax errors show inline, with the position of the error.
- Queries never block the UI. Results stream in and the first page appears
  fast. Budget: a TraceQL or LogQL search over 10 million stored items
  returns its first page in under 1 second (my call).
- The map, trace table and logs all honour the selected time range: last
  5 minutes, 15 minutes, 1 hour, or everything stored.

## 7. Discard rules

- A rule has a name, a language, a query, an on/off switch, and a count of
  what it has dropped since launch. The language is TraceQL for spans or
  LogQL for logs.
- Rules are evaluated at ingest. Matching data is **dropped for good**, never
  stored or shown.
- Rules persist across restarts in a human-readable file in the user's
  Application Support folder. The format is the builder's call.
- **Trace rules choose what to drop** (the user saw and accepted this in the
  mockup):
  - **the whole trace** (default), or
  - only the matching span and its descendants.
- A whole-trace drop must work even though spans arrive over time:
  - Once any span of a trace matches, the spans already stored for that
    trace are removed.
  - Later spans with that trace ID are dropped on arrival, for a bounded
    time (my call).
  - Logs carrying that trace ID are dropped too (my call), so a health
    check's logs vanish with its trace.
  - Structural rules are evaluated against the spans present so far, on
    each arrival.
- Turning a rule on affects new data only. It does not purge what is
  already stored (my call).
- **Preview before saving:** the editor runs the rule against the last
  5 minutes of stored data and says what it would have dropped, including
  whether any of it had errors (my call; in the mockup the user saw).
- **Prefill from data:** "Discard lines like this…" on a log line opens the
  editor with a suggested rule (my call; in the mockup).
- The typical first rules are health checks (`/health`, `/alive`, `/ready`)
  and noisy framework logs such as EF Core SQL command logs.
- MCP cannot change rules. It can only see which rules exist and how much
  they drop.

## 8. MCP server

- **Protocol:** MCP spec **2026-07-28** (the latest), over **Streamable HTTP**
  at `http://127.0.0.1:4319/mcp`. The port is configurable.
  - No authentication.
  - Bind to loopback only and validate the `Origin` header, as the spec
    requires for local servers.
  - Also serve older 2025-11-25 clients if the SDK allows.
- **Implementation:** the official Rust SDK `rmcp` (3.6 at time of writing),
  mounted in axum.
- **Read-only.** No tool clears data, pauses ingest, or changes rules.
- **Small answers first.** Every tool returns a compact summary by default,
  with explicit `limit` and `cursor` arguments and a "detail" option to drill
  down.
  - Default responses stay well under ~8k tokens. The Aspire dashboard caps
    at a similar size.
  - Tools return `structuredContent` with an output schema, plus the same
    JSON as text, as the spec recommends.
- **Tools.** The names are the builder's call; the capabilities are required:
  - **Status:** whether ingest is running, rates, counts, ports, and active
    rules with their drop counts.
  - **Marker / now:** the current server time, to use as `since` in other
    tools.
  - **Services:** the list of services with request rate, error rate and
    last seen.
  - **Service map:** nodes and edges with call counts and error rates.
  - **Search traces:** an optional TraceQL query, plus `since`, service,
    errors-only and minimum duration. Returns summaries: trace ID, root
    span, services, start, duration, span count, error flag, **failed in**
    (section 9.4), and the first exception message.
  - **Recent failures:** a convenience tool for "why did my last request
    fail". It returns the most recent failed traces since a marker, each
    with the failed-in span, its exception, and the error logs linked to it.
    It also lists traces with errors inside, flagged separately
    (section 9.4).
  - **Get trace:** a span tree, collapsed to the error path and top levels
    by default, expandable or paginated, with exceptions. Its logs are
    optional.
  - **Get span:** all attributes, events and resource, plus the logs linked
    to the span.
  - **Search logs:** a LogQL query plus `since`. Returns lines with time,
    level, service, message, and trace and span IDs.
- The MCP server runs whenever loctel runs, including headless (section 10).

## 9. User interface

Mockups (private to the user):
<https://claude.ai/artifact/QdYLABvppZc3rHGcdxpUJX>. The user chose
**Home A** ("map above traces"). Home B is rejected. The Trace, Logs,
Discard rules and Menu bar boards were accepted as drawn. The mockups are a
layout reference, not pixel specs. They only show dark mode.

### 9.1 App lifecycle

- loctel starts once and lives in the menu bar. It keeps receiving
  telemetry with no window open, and only stops when the user quits.
- No Dock icon while no window is open. While the main window is open,
  loctel shows in the Dock and ⌘-Tab, so it is easy to switch back to while
  debugging (my call).
- Closing the window doesn't quit the app. ⌘Q or **Quit loctel** quits.
- **Start at login** is a toggle using `SMAppService`, on by default (it was
  shown checked in the accepted mockup).
- **Startup:** the menu bar icon is live, and the endpoints listening, within
  **300 ms** of launch (my call). The main window appears within **500 ms**
  of choosing Open, including the first open after launch.

### 9.2 Screens

**Sidebar** (glass), on every screen:

- **Map & traces**, **Logs** and **Discard rules**.
- A status box: Receiving / Paused / port error, spans/s and logs/s, the
  OTLP ports, and the MCP address.

**Map & traces** (home, Home A):

- **Toolbar:**
  - a Live indicator
  - the time range
  - Pause, which freezes the view while ingest continues
  - Clear all
- **Service map**, live:
  - **Nodes** are services (`service.name`) plus inferred dependencies:
    - databases, from client spans with `db.system`/`db.system.name`
    - queues, from `messaging.system`
    - external hosts, from client spans with no matching server span,
      named by `server.address`
  - **Edges** come from client→server parent/child pairs across services,
    and from producer→consumer pairs.
  - Each node shows its request rate and error rate. Failing nodes and edges
    are red.
  - Auto layout runs left to right and stays stable as nodes appear
    (nothing jumps). Pan and zoom are supported.
  - Clicking a service filters the trace table and writes the matching
    TraceQL into the query bar.
- **Trace table:**
  - the TraceQL bar, with quick toggles (errors only, slower than 1 s)
  - columns: started, root span, **Failed in**, services (colour dots),
    span count, and duration with a bar
  - newest first, virtualized
  - clicking a row opens the trace

**Trace view:**

- **Header:** root span name, a failed-in badge, duration, span and service
  counts, start time, the trace ID with a copy button, and the "complete"
  indicator (section 4).
- **Waterfall:**
  - span tree with collapse/expand, coloured by service, errors marked
  - exception event markers on the bars
  - find-in-trace, an "errors path only" toggle, and collapse all
  - virtualized for 100,000+ spans
- **Inspector panel** (glass-styled) for the selected span:
  - name, service, kind, duration and offset
  - the exception: type, message, stack trace, copy button
  - attributes and resource
  - actions, such as "traces through this span name" and "copy span as
    JSON"
- **Logs of the selected span**, under the waterfall, with "Open in Logs".
- When a failed trace opens, its failed-in span is selected automatically
  (my call).

**Logs:**

- The LogQL bar, level toggles (ERR, WRN, INF, DBG), live tail, the time
  range, and Clear.
- A volume histogram over the time range, with errors highlighted.
- A virtualized list with columns time, level, service (colour dot), message
  and trace link. It must handle 10M+ lines.
- **Expanded line:** all attributes, trace and span IDs, and the actions
  "Open trace", "Show lines around this one" and "Discard lines like this…".

**Discard rules:**

- A list of rules, each with its switch, name, language chip, query, and
  dropped count.
- The total percentage of incoming data dropped.
- An editor panel with:
  - name
  - applies to: traces or logs
  - query
  - whole trace vs span-and-descendants
  - the 5-minute preview
  - Save, Cancel and Delete

**Menu bar popover:**

- the status pill
- spans/s, logs/s, traces held, and disk used against the cap
- the endpoint list, with a copy button for the MCP address
- the rules summary, linking to the rules screen
- **Open loctel** (⌘O), **Pause receiving**, **Clear all data…** (asks for
  confirmation), the **Start at login** toggle, and **Quit loctel** (⌘Q)

**Settings** (minimal):

- OTLP gRPC, OTLP HTTP and MCP ports
- disk cap
- start at login

### 9.3 Look and feel

- Native macOS feel, **Liquid Glass where real**:
  - Apple's real glass (`NSGlassEffectView`) sits behind the **sidebar**,
    the **toolbar** and the **menu bar popover**.
  - Content areas (map, tables, waterfall, logs) are opaque. Their panels
    and buttons are styled to match (translucent fills, soft highlights,
    capsule buttons) but are drawn by GPUI.
  - The user accepted this limit explicitly.
- Follows the system light/dark appearance (my call; confirmed in
  playback). Only the dark look is mocked. The builder designs the light
  look in the same spirit.
- Service colours stay consistent across map, table dots, waterfall and
  logs. Errors are red, and are also marked by an icon or text, never by
  colour alone.
- System font (SF) and system monospace.
- The Linux build uses the same UI without glass. It needs to look
  reasonable, not polished.

### 9.4 What "failed" means (my call)

These rules are used everywhere: trace table, trace header, map colours and
rates, the "errors only" toggle, auto-selection, and MCP.

- **Erroring span:** a span whose status is `Error`.
  - An exception event alone does not make a span erroring. That covers a
    handled exception, or a retry that then succeeded.
  - Such exceptions are still shown: as markers on the waterfall, and in
    the inspector.
- **Failed trace:** a trace whose **root span** (the request's entry point)
  is erroring. This answers "did my request work?".
- **Trace with errors inside:** the root span is fine, but some other span
  is erroring, for example a call that failed and then recovered on retry.
  - It is shown differently from a failed trace: amber "errors inside", not
    red "Failed in".
  - The "errors only" toggle includes both kinds.
  - MCP reports the two separately, so an agent can tell "the request
    failed" from "the request succeeded after errors".
- **Root not arrived yet:** while the root span hasn't arrived, a trace
  with erroring spans counts as "errors inside". It becomes "failed" if the
  root arrives erroring.
- **Failed-in span:** first take the erroring spans with no erroring
  descendants (the lowest error on each branch). The one that **started
  first** wins. If others exist, show "+N more".
  - Example: checkout calls payments, which fails. Later, checkout calls
    inventory, which calls the database, which also fails. Failed in =
    payments, "+1 more".
  - Shown as "service · span name".
- **Service error rate (map node):** the share of that service's entry
  spans (server and consumer spans) in the time range that are erroring.
- **Edge error rate:** the share of the calls along that edge whose client
  or producer span is erroring.

---

## 10. How agents build and maintain it

These are requirements, not suggestions. The user approved all of them.

### 10.1 Repository as memory

- **`AGENTS.md`** at the root (with `CLAUDE.md` pointing to it): what loctel
  is, the architecture map, how to build, run, test and take screenshots,
  and the house rules. The house rules:
  - Never merge on red.
  - Never skip, disable or loosen a test to get green.
  - Update docs and the decision log in the same PR as the change.
- **`docs/`:**
  - this design
  - an architecture overview
  - one short doc per area (ingest, store, query, rules, MCP, UI, macOS
    integration, release)
  - **`docs/decisions/`**: a log of decisions and their reasons, so later
    agents don't undo choices they don't understand
- **Step-by-step guides** for recurring jobs, as repo skills under
  `.claude/skills/`:
  - run the app and take screenshots
  - add an MCP tool
  - add a TraceQL or LogQL feature
  - add or change a screen, and update the reference screenshots
  - upgrade the pinned GPUI snapshot
  - investigate a CI failure
- **Fake microservice mesh** (`meshgen` or similar): sends realistic OTLP
  traces and logs to loctel. Its scenarios must at least include:
  - a checkout flow failing in a downstream service with an exception
  - health-check noise
  - spans arriving late and out of order
  - one 100,000-span trace
  - a log flood at the throughput target

  It is used by tests, benchmarks, screenshots and manual runs.
- **Headless mode** (`loctel --headless`): ingest, store, rules and MCP with
  no UI and no graphics. Used by end-to-end tests and by agents.

### 10.2 Environment

Agents work in Linux cloud containers like the one used for this design:
Ubuntu 24.04, 4 CPUs and 15 GB RAM, with no GPU and no display.

- A **session start hook / setup script** installs everything needed. From
  the spike:
  - `libxkbcommon-dev`, `libxkbcommon-x11-dev`, `libwayland-dev`
  - `mesa-vulkan-drivers` (lavapipe), `libegl1`, `libegl-mesa0`
  - fontconfig and freetype dev packages
  - `xvfb`, `xdotool`, `imagemagick`
  - the Rust toolchain from `rust-toolchain.toml`
- The toolchain is pinned to what the chosen GPUI snapshot requires. Zed
  main needed 1.98.1.
- Build budget: a clean debug build took about 3 minutes in the spike, and
  the target directory reached about 7 GB.

### 10.3 Checks (the only quality gate)

On every PR, all of these must pass before the agent merges its own PR. Main
is protected so only green PRs merge, and no human approval is required.

- **Linux:**
  - `cargo fmt --check`, and `clippy` with warnings denied
  - unit tests, including TraceQL/LogQL parser and evaluator tests checked
    against examples from the Tempo and Loki docs
  - **end-to-end tests:** start loctel headless, drive meshgen scenarios
    through real OTLP (gRPC, HTTP protobuf, HTTP JSON), and assert through
    the MCP client. This covers late-span assembly, discard rules, failed-in
    detection, and the "recent failures" answer.
  - **screenshot tests:** render each screen with fixed meshgen data through
    GPUI's headless renderer (`HeadlessAppContext` + `capture_screenshot`)
    and compare pixel-exactly with the reference PNGs in the repo.
    - The spike showed software rendering is deterministic (lavapipe and
      llvmpipe produced identical images across runs).
    - On failure, CI uploads the actual and diff images.
    - Intentional UI changes regenerate the references in the same PR. The
      agent must look at the new images before committing them.
  - **performance tests** with budgets, failing CI on regression:
    - ingest at 50k items/s
    - opening and laying out a 100,000-span trace
    - random access into 10M log lines
    - query first-page latency

    Frame-rate targets (smooth 60 fps or better scrolling on Apple Silicon)
    can't be measured on software rendering. Measure render-side
    regressions where reliable (for example, frame build times on the macOS
    runner), and keep the absolute fps targets in the docs.
- **macOS** (GitHub `macos-26` arm64 runner):
  - build the app bundle
  - run a smoke test: launch, check the endpoints and MCP answer, quit
  - run any macOS-only unit tests

  macOS-specific code (glass, status item, login item) can only be checked
  here, so keep it small and isolated.
- **Release:** every merge to main builds the `.app` and an unsigned `.dmg`,
  and publishes a **GitHub release** with a new version number.
  - Versioning: semver, patch bump per merge unless the PR says otherwise
    (my call).
  - Sign the bundle ad-hoc (`codesign --force --deep --sign -`), as Zed does
    without a certificate. arm64 code must be signed, and a broken signature
    shows as "damaged".
  - The `.dmg` contains an /Applications symlink.
  - Release notes list the PRs merged.
  - Installing: download the `.dmg`, drag to Applications, and on first
    launch of each version use System Settings → Privacy & Security → **Open
    Anyway** (or `xattr -dr com.apple.quarantine /Applications/loctel.app`).
    The user accepted this.

### 10.4 Known gotchas from the spike (2026-10-21)

- The crates.io `gpui` 0.2.2 is a year old. It has no headless screenshot
  API, and its window stays black under Xvfb unless an X event "nudges" it
  (e.g. `xdotool windowmove`). The weekly snapshot fixes both.
- A `libc` / `xattr 0.2.3` build error (`ENOATTR`) showed up through
  `gpui_http_client` with crates.io gpui. If it appears, pin `libc` with
  `cargo update -p libc --precise 0.2.186`.
- The first headless frame takes about 1.8 s, and later frames about 95 ms
  in debug builds. Keep screenshot tests to a reasonable number of frames.
- With neither Vulkan nor EGL present, headless rendering fails with "No
  usable headless GPU adapter found". The setup script must install them.

---

## 11. Technology (decided)

Versions are as of 2026-10-21. Newer patch versions are fine.

| Concern | Choice |
|---|---|
| Language | Rust |
| UI framework | **GPUI** (Zed's framework), via the weekly **`gpui-pre`** snapshot pinned to an exact version. 0.3.9 matches zed@93d7456 and includes `HeadlessAppContext::capture_screenshot`. Fallback: a pinned Zed git revision. |
| UI components | **GPUI Kit** (`longbridge/gpui-kit`, 0.8.0): virtualized DataTable and VirtualList, Tree, Input, Tabs, Resizable, Sidebar. It pins `gpui-pre`, so the two are upgraded together. |
| Liquid Glass | `NSGlassEffectView` via `objc2`, placed below GPUI's view in a transparent window, with frames synced to the sidebar and toolbar regions and click-through. Prior art: `Rhein-Industries/gpui-macos`. Needle hit a startup deadlock when AppKit called back into GPUI under a window lock, so avoid that. Watch zed#58833 (upstream glass PR, still open). Fallback if glass proves unworkable: GPUI's `Blurred` window background (my call). |
| Menu bar | `gpui-tray` (NSStatusItem), GPUI `ActivationPolicy::Accessory` (switching to Regular while the window is open), and `QuitMode` explicit. The builder must check that the popover can be anchored under the icon. |
| Start at login | `SMAppService.mainApp` via `objc2-service-management` |
| OTLP | `opentelemetry-proto` 0.33.1 (`gen-tonic`, `trace`, `logs`, `with-serde`), `tonic` 0.14 (server, gzip), `axum` 0.8 plus `tower-http` decompression |
| MCP | `rmcp` 3.6, `StreamableHttpService` in axum |
| Query languages | Own parsers and evaluators, from Tempo's and Loki's grammars |
| Storage | Custom segment store (section 5) |
| Packaging | `cargo-bundle` or `cargo-packager` with `LSUIElement`, then `hdiutil` for the DMG |
| Minimum OS | macOS 26 on Apple Silicon. Linux x86_64 for development and tests only. |

### Shape of the code

The builder decides the details. The hard requirement: **the core (ingest,
store, query, rules, MCP) has no UI or graphics dependency, and builds and
tests on Linux alone.** The UI stays thin over it. macOS-only integration
sits behind `cfg(target_os = "macos")` in its own small crate. A plausible
Cargo workspace:

- model types
- ingest
- store
- query (TraceQL and LogQL)
- rules
- MCP
- engine (wires the above, owns threads and runtime)
- UI (GPUI views)
- macOS integration
- the app binary
- meshgen
- an `xtask` for screenshots, benchmarks and bundling

---

## 12. Left to the builder

- store internals, threading/runtime model, and memory budget
- exact crate layout and names
- MCP tool names, argument names and schemas (within section 8)
- service map layout algorithm and visual details
- the light theme
- keyboard shortcuts beyond ⌘O and ⌘Q
- settings and rules file formats and locations
- histogram bucketing, and how long a dropped trace ID is remembered
- exact performance budgets beyond the targets given here, and where each is
  measured
- loopback vs all-interfaces binding for OTLP, decided by testing Docker
  Desktop

## 13. Risks

- **GPUI churn:** it is pre-1.0 and its API breaks often. `gpui-pre` is
  published by a third party (GPUI Kit's maintainer), so if it stops, switch
  to a pinned Zed git revision. Mitigation: one upgrade per PR, guided by
  the upgrade skill and the screenshot tests.
- **Glass integration is fragile:** there's no upstream support, and the
  known deadlock pitfall. Keep it isolated, with the Blurred fallback.
- **TraceQL/LogQL scope:** two languages to implement. Start with the subset
  in section 6 and grow it test-first.
- **macOS-only code is invisible to Linux tests.** The macOS CI smoke test is
  its only guard.
- **CI cost:** if the repository is private, macOS runner minutes are billed
  at a premium. Every PR runs a macOS build.
- **Docker networking:** depends on Docker Desktop behaviour (section 4).
  Other runtimes (OrbStack, Colima) are not a requirement.

---

## 14. Calls made by the design author (for the user to check)

- **What "failed" means** (section 9.4):
  - A trace failed when its root span errors. If only inner spans error,
    the trace is marked "errors inside" instead.
  - An exception event alone doesn't count as an error.
  - Failed in = the earliest of the lowest erroring spans.
- Metrics requests are accepted and dropped, so exporters stay quiet.
- If a port is busy, the other endpoints keep running and the error is shown
  in the UI.
- **Pause receiving** answers success but drops data.
- The disk cap defaults to 4 GB, and the oldest data is evicted first, by
  whole trace.
- Discard rules:
  - A whole-trace drop also drops that trace's logs and its late spans.
  - Turning a rule on doesn't purge existing data.
  - Rules are previewed against the last 5 minutes before saving.
  - A log line can become a rule ("Discard lines like this…").
- Opening a failed trace auto-selects its failed-in span.
- loctel shows in the Dock and ⌘-Tab only while its window is open.
- Start at login is on by default.
- Startup budgets: menu bar live in 300 ms, window in 500 ms. Search budget:
  first page in 1 s over 10M items.
- Follows the system's light/dark setting.
- Releases are semver with a patch bump per merge.
- GPUI's Blurred background is the fallback if real glass fails.
