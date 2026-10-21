# MCP server (`loctel-mcp`)

The read-only Model Context Protocol server that lets an AI agent ask loctel
why a request failed. It answers from the store, over Streamable HTTP, on
loopback. Design references: §3.2 (the agent flow), §8 (the server), §9.4
(what "failed" means, which every tool takes from the store's `classify`) and
§7 (MCP cannot change rules).

Status: all nine tools, the `Origin` check, both protocol generations and the
engine wiring are implemented and tested on Linux, including through a real
rmcp client. That **Claude Code connects and answers "why did my last
request fail"** is **unverified** until the Milestone 1 check on a real Mac
(design §10.4); Linux tests cannot run Claude Code.

## Connecting an agent

loctel serves MCP at `http://127.0.0.1:4319/mcp` whenever it runs, headless or
not. The port is `mcp_port` in `settings.toml` (or `--mcp-port`);
`EngineHandle::mcp_url()` gives the real one (the menu bar item "Copy MCP
address" will copy it).

```bash
claude mcp add --transport http loctel http://127.0.0.1:4319/mcp
```

Then ask the agent "why did my last request fail". It should call
`recent_failures`. For one request in particular, it calls `get_marker`, sends
the request, and passes the marker as `since`.

There is no authentication, and none is needed: the server only listens on
`127.0.0.1`, refuses requests whose `Origin` is another site, and refuses a
`Host` that is not `localhost`, `127.0.0.1` or `::1` (DNS rebinding).

## The tools

All nine are read-only (no tool clears data, pauses receiving or touches
rules) and say so in their annotations. Names, arguments and limits are fixed:
agents depend on them. Every result carries `structuredContent` that matches
the tool's output schema, plus the same JSON as text.

| tool | arguments | returns |
|---|---|---|
| `get_status` | none | `{paused, listeners:[{name,address,state,error?}], spans_per_sec, logs_per_sec, traces_held, spans_held, logs_held}` |
| `get_marker` | none | `{marker:"<unix nanos>", time:"<RFC 3339>"}` |
| `list_services` | `since?` | `[{name, requests, errors, error_rate, last_seen}]` |
| `get_service_map` | `since?` | `{nodes:[{id,kind,requests,errors}], edges:[{from,to,calls,errors}]}` |
| `search_traces` | `since?, service?, errors_only?, min_duration_ms?, limit?=20 (max 100), cursor?` | `{traces:[TraceSummaryDto], next_cursor}` |
| `recent_failures` | `since?, limit?=5 (max 20)` | `{failed:[{trace: TraceSummaryDto, failed_in_span: SpanBriefDto, exception?:{type,message,stacktrace}, error_logs:[LogDto]}], errors_inside:[TraceSummaryDto]}` |
| `get_trace` | `trace_id, detail?="summary"\|"full", limit?=200 (max 2000), cursor?` | `{summary: TraceSummaryDto, spans:[SpanBriefDto], omitted, next_cursor}` |
| `get_span` | `trace_id, span_id` | `{span:{all fields, attributes, events, resource, exception?}, logs:[LogDto]}` |
| `search_logs` | `since?, service?, min_level?, contains?, trace_id?, limit?=50 (max 500), cursor?` | `{logs:[LogDto], next_cursor}` |

The DTOs (`src/dto.rs`):

- **`TraceSummaryDto`:** `trace_id, root_name, root_service, start, duration_ms,
  span_count, services, status ("ok"|"errors_inside"|"failed"),
  failed_in?:{service, span_name, peer?, span_id, more},
  first_exception?:{type,message}, complete`. `root_name` and `root_service`
  are `null` until the root span arrives. `failed_in` is present unless the
  status is `ok`, and `more` is the "+N more" of §9.4.
- **`SpanBriefDto`:** `span_id, parent_span_id?, name, service, kind,
  start_offset_ms, duration_ms, status, exception_type?`. Offsets count from
  the start of the trace.
- **`LogDto`:** `time, level, service, message, trace_id?, span_id?`. `level` is
  `unknown`, `trace`, `debug`, `info`, `warn`, `error` or `fatal`.

### Conventions

- **IDs** are lowercase hex (32 characters for a trace, 16 for a span). The
  `trace_id` arguments take either case.
- **Times** are RFC 3339 in UTC to the millisecond, cut and not rounded, so a
  time read back as `since` includes its own item. **Durations and offsets** are
  milliseconds to the microsecond. **Rates** are 0 to 1 to four decimals.
- **Absent keys.** A key marked `?` is left out when it has no value. A key
  without `?` is always there, and is `null` where that can happen (the root of
  a trace that has not arrived, `next_cursor` on the last page).
- **Unknown arguments are errors**, so a misspelt filter is reported and not
  silently ignored (the input schemas say `additionalProperties: false`).
- **`list_services` returns a top-level JSON array**, as the table says, and
  its output schema has root type `array`. The 2026-07-28 spec allows that;
  2025-11-25 limits output schemas to objects. If a client of the older
  generation rejects `tools/list` because of it, wrap the array in an object
  (see `docs/decisions/0006-mcp-server-choices.md`).

### `since`

`since` is a marker from `get_marker` (decimal unix nanoseconds, **as a
string**: nanoseconds do not fit a JavaScript number) or an RFC 3339 time with
an offset. Anything else is a tool error that says what was expected. A time
before 1970 means everything. It filters on when the trace started (and, for
logs, on the log's own time), so a request sent after the marker is found even
while its trace is still open. `get_marker` reads the **store's clock**, the one
completeness is judged by, which is why tests can control it.

`list_services` and `get_service_map` take `since` as the start of their
window; with none, the window is everything stored.

### Sizing rules

Answers stay small by default (design §8: well under about 8k tokens).

- Limits are clamped, never rejected: a `limit` above the cap gets the cap, and
  0 gets 1. An empty `cursor` means the first page.
- **Messages and stack traces are cut at 2,000 characters**, with the suffix
  `…(truncated)`. Characters, not bytes. The cut covers log messages, exception
  messages, stack traces, span status messages and every string inside span
  attributes and events (an `exception.stacktrace` event attribute is the stack
  trace again).
- **`get_trace`, `summary` detail** (the default): the first page is the spans
  on the path from the root to the failed-in span, plus every span at depth 2 or
  less (the root is 0; a span whose parent has not arrived counts as a root).
  If that is more than `limit`, the path comes first. Then the rest follows, in
  start order, by following `next_cursor` with the same `detail`. A page never
  runs on from the collapsed view into the rest, so the first page is the
  collapsed view however small it is. `omitted` is how many spans come after the
  page, and `next_cursor` is `null` exactly when `omitted` is 0.
- **`get_trace`, `full` detail**: every span, oldest start first, `limit` at a
  time. A cursor belongs to the detail that made it; the other detail refuses it
  with a message saying so.
- **Cursors are position markers over live data.** Spans and logs that arrive
  between two pages can shift what a page holds: the search tools page by key
  (so nothing repeats), and a late span that sorts before the boundary is not
  on a page already read. A cursor the store did not produce gives an empty page
  from `search_traces` and `search_logs`, which ends a reader's loop instead of
  restarting it, and an error from `get_trace`.
- **`recent_failures`** reads newest first until it has `limit` failed traces
  and `limit` traces with errors inside. `error_logs` are the trace's logs of
  level error or worse, linked by trace ID, **oldest first, the first 10**.
  `exception` is `trace_exception` (the failed-in span's, else the nearest
  ancestor's, else the first). A failed trace and a trace with errors inside are
  never mixed (§9.4): `failed` means the request failed, `errors_inside` means
  it may have worked.
- **`get_span`** shows at most 50 logs, oldest first.

### Errors

A tool that cannot answer returns a result with `isError: true` and its reason
as the only text, with no `structuredContent`. The texts an agent can rely on:

- `trace not found` and `span not found` (an unknown or evicted ID).
- `invalid trace_id "...": ...`, `invalid span_id "...": ...`.
- `invalid since "...": ...; expected a marker from get_marker ... or an RFC 3339 time ...`.
- `failed to deserialize parameters: ...` for arguments that do not fit (a
  missing required one, an unknown one, the wrong type, an unknown `min_level`).
- `invalid cursor: ...` and `this cursor belongs to detail=...` from `get_trace`.

## Protocol and transport

- **rmcp 3.6** (the official Rust SDK) with its `server`, `macros` and
  `transport-streamable-http-server` features; the client half and its reqwest
  transport are dev-dependencies of the crates that test through a client.
- **Streamable HTTP at `/mcp`**, an rmcp `StreamableHttpService` nested in an
  axum router behind the `Origin` middleware (`src/origin.rs`). Nothing else is
  served (any other path is 404).
- **Both generations.** Clients of MCP 2026-07-28 are served statelessly (no
  session, per-request metadata); clients of 2025-11-25 and earlier get the
  `initialize` handshake and an `Mcp-Session-Id`. rmcp does the negotiation, and
  sessions that sit idle for 5 minutes are dropped.
- **Loopback only.** `serve` refuses an address that is not loopback, and the
  engine always passes `127.0.0.1` whatever `bind_ip` says for OTLP.
- **`Origin`.** A request whose `Origin` names a host other than `localhost`,
  `127.0.0.1` or `[::1]` gets 403, on every method and path. No `Origin` at all
  is allowed (a command line client or an agent sends none; a browser always
  sends one on a cross-site request). `null`, a bare host, credentials and
  anything malformed are refused. The scheme and port do not matter.
- **`Host`.** rmcp refuses a `Host` that is not one of the same names (403).
- **Failure to bind** (the port is taken) is a failed `mcp` listener in the
  engine status and `get_status`, not an error: OTLP keeps running.

## Rust API

```rust
let (listener, shutdown) = loctel_mcp::serve(addr, store, status_source).await;
// listener: ListenerStatus named "mcp"; shutdown: McpShutdown (drop = stop)
```

- `trait StatusSource: Send + Sync + 'static { fn status(&self) -> StatusDto; }`
  is implemented by the engine, which is the only thing that knows whether
  receiving is paused and the rates. The engine reads the same state for
  `EngineHandle::status()`, so the UI and `get_status` cannot disagree.
- `LoctelTools::new(store, status)` is the tool set. `call(name, args)` runs a
  tool without a connection and returns the rmcp `CallToolResult` (this is how
  `tests/tools.rs` works), `definitions()` lists the tools as a client sees
  them. `dispatch` in `src/tools.rs` routes by name and must list every
  `#[tool]` method (a test fails if one is missing).
- `parse_since(&str) -> Result<u64, SinceError>` is the `since` rule.
- `src/trace_view.rs` is how `get_trace` chooses and pages spans.

To add a tool, follow `.claude/skills/add-mcp-tool/SKILL.md`.

## Tests

- `crates/loctel-mcp/tests/tools.rs`: the tools through `call`, on a store fed
  from `loctel-meshgen` through the real converter, with a manual clock moved
  past the quiet period. Every successful result is validated against its
  tool's output schema.
- `crates/loctel-mcp/tests/http.rs`: a real rmcp client over HTTP, both
  protocol generations, the `Origin` and `Host` checks, shutdown, a taken port.
- `crates/loctel-engine/tests/engine.rs`: the `mcp` listener, `mcp_url()`, and
  an rmcp client reading `get_status` and the stored traces through the engine.
- `crates/loctel-app/tests/headless.rs`: the `mcp` line on stdout and a request
  to the printed address.
- `crates/loctel-e2e/tests/`: the tools through real OTLP, as an agent calls
  them: `recent_failures` after a failing checkout over each transport, the
  marker flow (`get_marker`, then `since`), `get_trace` on a late-assembled
  trace, `get_service_map`, `search_traces` and `get_status` under pause. See
  [`testing.md`](testing.md).
- Unit tests in `src/since.rs`, `src/origin.rs`, `src/dto.rs` and
  `src/trace_view.rs`.

## Not yet

- `query` arguments (TraceQL for traces, LogQL for logs) and the active rules
  with their drop counts in `get_status` come with Milestone 2.
