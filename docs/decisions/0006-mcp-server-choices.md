# 0006: MCP server choices

Date: 2026-10-21
Status: accepted

## Context

The Milestone 1 plan (Task 9) fixes the tools: their names, arguments, limits
and the shape of what they return, and it names the pieces (`serve`,
`LoctelTools`, `StatusSource`, the `Origin` rule, `since`). It leaves open how
rmcp 3.6 is used (the plan says "use whatever features rmcp 3.6 names") and the
behavior at the edges. These are the choices a later agent might be tempted to
undo, and the one risk to check on a Mac.

## Decision

- **rmcp 3.6 with its defaults off.** Features: `server`, `macros` and
  `transport-streamable-http-server`. The client half (`client` and
  `transport-streamable-http-client-reqwest`) is a dev-dependency of the crates
  that test through a client. rmcp 3's API is not the 0.x API: the server info is
  `ServerConfig`, a tool call answers `CallToolResponse`, and
  `StreamableHttpService::new` takes a handler factory, a session manager and a
  `StreamableHttpServerConfig`. The code follows 3.6, and the plan's own
  interfaces are unchanged.
- **Sessions follow rmcp's defaults.** 2026-07-28 clients are served statelessly
  and older ones (2025-11-25, which the design asks us to keep serving) get the
  `initialize` handshake and a session that rmcp drops after 5 idle minutes.
  Sessions are in memory, which is fine for a tool that restarts with the app.
- **`LoctelTools::call` routes by name itself.** rmcp's own router needs a
  `RequestContext`, which needs a `Peer`, and only a running connection can make
  one. The tool methods are plain functions of the store and need none of it, so
  `call` matches the name and calls them, with the argument decoding and error
  text rmcp uses on the wire (`failed to deserialize parameters: ...`). The cost
  is that a new tool must be added to two places, the `#[tool]` method and
  `dispatch`. `call_dispatches_every_registered_tool` fails if one is missed, and
  the `add-mcp-tool` skill says so.
- **The `Origin` check is our middleware, not rmcp's `allowed_origins`.** The
  plan states the rule (host in `localhost`, `127.0.0.1`, `[::1]`, else 403, no
  `Origin` allowed) and it is a security boundary, so it is a small function with
  its own tests, applied to every method and path before rmcp sees the request.
  rmcp's `Host` check stays on at its default (the same three names), which is
  what stops DNS rebinding.
- **`serve` refuses a non-loopback address**, reporting a failed `mcp` listener
  with the reason. The MCP server is read-only and unauthenticated, so being
  loopback-only is made structural, not a promise of the one caller (the engine).
- **`Store::now_unix_nano()` was added to the store** (a one-line, additive
  accessor). `get_marker` has to say "now", and the store is where the clock is
  injected, so the marker and completeness agree and a test that drives the store
  with a `ManualClock` controls the marker. The alternative, a `SystemClock` in
  the MCP crate, would have made the marker untestable and a second clock to keep
  in step.
- **`since` is a string.** A marker is decimal unix nanoseconds as a string
  (they exceed a JavaScript number's exact range), and a JSON number is refused
  with the ordinary type error. An RFC 3339 time must carry an offset (chrono also
  accepts a space for the `T`, and we leave it). A time before 1970 reads as 0
  ("everything") and one beyond 2262 is refused.
- **Arguments are strict, limits are lenient.** Unknown arguments are errors
  (`deny_unknown_fields`, `additionalProperties: false` in the schema), because a
  misspelt filter that is silently ignored makes an agent believe it filtered. A
  `limit` above its cap is clamped to the cap and 0 is read as 1, because an agent
  that asked for too much should get an answer, not a refusal.
- **`get_trace` pages a ranked order.** In `summary` detail the order is: the
  path to the failed-in span, the other spans at depth 2 or less, then the rest,
  each oldest start first, and a page never runs from the first two ranks into the
  third. That gives the plan's "path plus top levels, capped at `limit`" as the
  first page, makes the test's `next_cursor` (present exactly when something is
  `omitted`) lead to the rest, and keeps the path when `limit` is tight. A cursor
  is a keyset over `(rank, start, span id)` with the detail in front, so it is
  stable against new arrivals and refused by the other detail.
- **`error_logs` are the first ten, oldest first.** The plan says "error-or-worse
  logs, oldest first, max 10", read as the ten oldest of the trace's error logs
  (the start of a failure is the part that explains it). The store pages newest
  first, so the tool reads up to 5,000 and turns them round; a trace with more
  than that has its ten oldest of the newest 5,000.
- **Always-present keys that may be `null`** (`root_name`, `root_service`,
  `next_cursor`) are required and nullable in the output schema, with
  `schema_with` because schemars' `required` alone drops the `null`. Every
  successful result in the tests is validated against its tool's schema.
- **Truncation covers every string in span attributes and events**, not only
  messages and stack traces, because an `exception.stacktrace` event attribute is
  the stack trace again and would defeat the 2,000-character cut. Span names are
  not cut.
- **Engine: one `State` for both readers.** `EngineHandle::status()` and the MCP
  `get_status` read the same struct. The MCP listener's own status exists only
  after the server has bound, and the server needs the state to start, so it is
  set once, right after `serve` returns. A `get_status` in that instant lists the
  two OTLP listeners and not yet `mcp`.
- **The headless binary prints three lines now** (`otlp-grpc`, `otlp-http`,
  `mcp`, in that order). The line format is unchanged. Scripts that read the first
  two lines still work, and a script that needs the MCP port reads the third.

## Consequences

- **Risk to check on a Mac:** `list_services` returns a top-level array, as the
  plan's table says, so its output schema has root type `array`. MCP 2026-07-28
  allows that (rmcp 3.6 says output schemas are not limited to objects) but
  2025-11-25 limits them to objects, and a client that validates `tools/list`
  strictly against the older rule could reject the whole list. Linux tests cannot
  show this. If "Claude Code connects and lists the tools" fails at the Milestone
  1 check, wrap the result as `{services: [...]}` (one DTO and the tests that read
  it, plus the table in `docs/areas/mcp.md`), and record it here.
- The tool contract (names, arguments, limits, shapes) is what agents learn from
  `tools/list`; change it only with the plan or a later milestone's plan, and
  update `docs/areas/mcp.md` with it.
- Milestone 2 adds `query` arguments and rule counts. They are new optional
  arguments and keys, so the strict-arguments rule does not hurt existing agents.
