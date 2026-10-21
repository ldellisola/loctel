# 0011: `list_services` returns `{services: [...]}`, not a bare array

Date: 2026-10-21
Status: accepted (changes the Milestone 1 plan's tool table; resolves the
"risk to check on a Mac" in [0006](0006-mcp-server-choices.md))

## Context

The plan's tool table (Task 9) says `list_services` returns
`[{name, requests, errors, error_rate, last_seen}]`: a top-level JSON array. The
first implementation followed it, so the tool's `outputSchema` had root
`type: "array"` and its `structuredContent` was an array. rmcp 3.6 allows
that, and the 2026-07-28 spec allows it, so Linux tests and the rmcp client
were happy. Decision 0006 named the risk and put off the fix until the Mac check.

The server also serves 2025-11-25 sessions (design section 8), and for those:

- the 2025-11-25 schema requires `outputSchema.type == "object"` and an object
  for `structuredContent`;
- the TypeScript MCP SDK that Claude Code uses parses `tools/list` with
  `z.literal("object")` on the output schema's root, so one tool with an array
  root makes the whole `tools/list` fail to parse and the client sees none of
  loctel's nine tools.

That was confirmed against a live 2025-11-25 session (`initialize`,
`tools/list`, `tools/call list_services`): both the schema and the result had an
array root. It would have broken the main Milestone 1 check ("Claude Code
connects and answers 'why did my last request fail'"), and waiting for the Mac
check would have cost a whole release cycle for a one-DTO fix. Nothing on Linux
caught it because the rmcp client, the e2e tests and `mcp-smoke` do not validate
schemas strictly.

## Decision

- **`list_services` returns `{"services": [{name, requests, errors, error_rate,
  last_seen}]}`.** The new DTO is `ServiceListDto { services: Vec<ServiceDto> }`
  in `crates/loctel-mcp/src/dto.rs`; `ServiceDto` is unchanged. The tool name,
  its `since` argument and the fields of each service are as the plan says; only
  the wrapper is new. An empty window answers `{"services": []}`.
- **Every tool's output schema has root `type: "object"`, and every
  `structuredContent` is an object.** This is now the rule for a new tool too
  (the `add-mcp-tool` skill says so), and it is tested at three levels:
  `every_output_schema_has_root_type_object` and the structured-content check in
  `results_have_structured_content_and_text` (`tests/tools.rs`), and the raw
  JSON of a 2025-11-25 session in `serves_2025_11_25_clients_with_a_session`
  (`tests/http.rs`), which reads `tools/list` and a `list_services` call the way
  a strict client does.
- **No compatibility shim for the array.** Nothing shipped with the array
  shape, so no client depends on it.

## Consequences

- `docs/areas/mcp.md` (the tool table and the conventions) and the
  `add-mcp-tool` skill describe the object. The plan's table is out of date on
  this one row; this decision and `mcp.md` are the source of truth.
- The engine test that reads `list_services` over MCP reads
  `services["services"]`.
- The Mac check no longer needs to watch for this particular failure. It still
  has to show that Claude Code connects (design section 10.4).
