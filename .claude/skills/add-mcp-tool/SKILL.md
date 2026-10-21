---
name: add-mcp-tool
description: "Add or change a tool of the loctel MCP server (crates/loctel-mcp). Use whenever an agent-facing capability is added to loctel, such as a new tool, a new argument or a new field in a tool result. Covers the DTO, the tool method, the dispatch arm, the tests, the name lists and the docs table."
---

# Add an MCP tool

The MCP server is read-only, compact by default, and fixed in its names and
arguments because agents depend on them (design section 8, plan Task 9). Read
`docs/areas/mcp.md` first: it has the tool table, the sizing rules and the
conventions every tool follows. A tool never clears data, pauses receiving or
changes rules.

## Steps

1. **Decide the contract before writing code.** Name (snake_case), arguments,
   and what it returns. Defaults must be small: take `limit` and `cursor` if the
   result is a list, clamp `limit` to a documented cap, and keep the default
   answer under `ANSWER_TEXT_BUDGET` (24,000 characters, about 6k tokens, under
   the design's 8k; see step 6). If it filters by time, take `since` and use
   `parse_since`.
2. **The DTO** in `crates/loctel-mcp/src/dto.rs`. Derive `Serialize` and
   `JsonSchema` (the schema becomes the tool's `outputSchema`), and put a doc
   comment on every field: agents read them. Follow the conventions: IDs as hex
   strings, times through `rfc3339`, durations through `millis`, enums as
   `snake_case`, a key that can be absent with
   `#[serde(skip_serializing_if = "Option::is_none")]`, and any long message
   through `truncate` (2,000 characters). Build it from the store's types; do not
   re-derive failure, completeness or rates (design 9.4 lives in
   `loctel_store::classify`).
3. **The arguments** as a struct in `crates/loctel-mcp/src/tools.rs`: `Deserialize`
   and `JsonSchema`, with `#[serde(deny_unknown_fields)]` so a misspelt argument
   is an error, and a doc comment per field.
4. **The tool method** on `LoctelTools` in `tools.rs`, inside the
   `#[tool_router]` impl:

   ```rust
   #[tool(
       description = "What it does and when to use it, for an agent.",
       annotations(title = "...", read_only_hint = true, destructive_hint = false,
                   idempotent_hint = true, open_world_hint = false)
   )]
   fn my_tool(&self, Parameters(args): Parameters<MyArgs>) -> Result<Json<MyResult>, String> { ... }
   ```

   Return `Result<Json<Dto>, String>`: `Json` gives `structuredContent` plus the
   same JSON as text and the output schema, and the `Err` text is what the agent
   sees with `isError: true` (word it so the agent can fix its call). Unknown
   trace or span: the exact texts `trace not found` and `span not found`.
5. **The dispatch arm.** Add `"my_tool" => with_args(args, |a| self.my_tool(a)),`
   to `LoctelTools::dispatch` (no arguments: `self.my_tool().into_call_tool_result()`).
   The test `call_dispatches_every_registered_tool` fails if you forget.
6. **The tests.**
   - `crates/loctel-mcp/tests/tools.rs`: a test for the behavior, on data from
     `loctel_meshgen` (`load(&store, &Scenario::..., seed, base)`). Use
     `Fixture::call`, which also checks the result against the tool's output
     schema. Add the name to `ALL_TOOLS` and a line to
     `results_have_structured_content_and_text`, and cover the limits, the error
     texts and the empty case.
   - `crates/loctel-mcp/tests/budget.rs`: add the tool to `default_answers_fit_budget`
     (a tool that needs IDs gets the worst ones the fixture has). The test
     calls every tool with default arguments on `CheckoutFailure`,
     `HealthChecks{50}` and `HugeTrace{10000}` and fails when one answer is
     over `ANSWER_TEXT_BUDGET`. Lower the tool's default `limit` until it fits,
     never the budget, and record the new default in `docs/areas/mcp.md` and a
     decision file. A new field in an existing result needs no new test, but
     run the file: it can push a default answer over.
   - `crates/loctel-mcp/tests/http.rs`: add the name to `EXPECTED_TOOLS`, and fix
     the counts that say nine (`serves_2025_11_25_clients_with_a_session`).
     `there_are_exactly_nine_read_only_tools` in `tools.rs` is the other one.
   - `crates/loctel-e2e/tests/`: only when the change alters what an agent sees
     in a whole flow (the failure answer, the marker and `since`, the service
     map, pause). Use `Harness::tool` and extend the matching test; see "End-to-end
     tests" in `docs/areas/testing.md`. A new tool needs no e2e test of its own.
   - Write the test first and watch it fail.
7. **The docs.** The tools table in `docs/areas/mcp.md` (and its DTO list,
   conventions and sizing rules when you changed them), and the sentence in the
   server `instructions` (the `#[tool_handler]` attribute) if agents should know
   about the tool up front. If the tool needs new store data, extend the store
   API and `docs/areas/store.md` first, in its own commit.
8. **Run the gate:** `cargo fmt --check`, `cargo clippy --workspace --all-targets
   -- -D warnings`, `cargo test --workspace`. Never skip or loosen a test.

## Things that go wrong

- **A new field that is sometimes absent** needs `skip_serializing_if`, and for
  a `Vec` also `#[serde(default)]`: schemars drops only an `Option`'s key from
  the schema's `required` list, so without `default` every result fails its
  output schema (`TraceSummaryDto::matched_span_ids` is the example).
- **A query argument** (`query` on `search_traces` and `search_logs`) is parsed
  once, as given: a blank one is no query, and a parse error is the tool error
  `invalid TraceQL: at column N: ...` or `invalid LogQL: at column N: ...`. Do
  not trim the text before parsing, or the column is wrong.

- **A result must be an object, never a bare array.** MCP 2025-11-25 requires
  `outputSchema.type == "object"` and an object `structuredContent`, and the
  TypeScript SDK Claude Code uses parses `tools/list` with `z.literal("object")`:
  one tool with an array root makes the client drop the whole list. Wrap a list
  in a struct (`{services: [...]}`, see `ServiceListDto` and
  `docs/decisions/0011-list-services-returns-an-object.md`). The test
  `every_output_schema_has_root_type_object` fails otherwise.
- `Option<T>` fields are nullable and not required in the generated schema. A key
  that is always present and can be `null` needs
  `#[schemars(required, schema_with = "nullable_string")]` (see `dto.rs`), because
  `required` alone drops the `null`.
- A tool must not block on I/O or hold the store for long: the store's read lock
  is taken per call.
- Do not return unbounded data. Cap lists, page them with a keyset cursor, and
  cut long text.
