# 0020: End-to-end tests of the rules and the queries

Date: 2026-10-21
Status: accepted

## Context

The Milestone 2 plan (Task 9) adds six end-to-end tests and
`Harness::start_with_rules(completeness, rules_path)`. It fixes the test names
and what each proves, and leaves open how a test gets its rules file, how it
sends a trace's batches one at a time, and how exact its numbers are. Decision
0007 has the choices of the harness itself; these are the additions, and the
ones a later agent might be tempted to undo.

## Decision

- **`start_with_rules` takes the path, and the test writes the file first.** The
  signature is `start_with_rules(completeness, rules_path: impl Into<PathBuf>)`.
  `start` stays as it was and has no rules (`rules_path: None`), which is what
  every M1 test and decision 0016 want: loctel decides nothing. Both go through
  one private starter, so the two cannot drift.
- **The rules file is the text of `RulesFile::EXAMPLE_RULE`.** The health-check
  tests write that constant to `rules.toml` as it is (it is a complete rules
  file: one `[[rule]]` table), instead of building a `RuleSpec` and saving it. The
  example in a new file is then proved to work as written, through real OTLP, and
  the disable test edits `enabled = true` to `false` in that very text, the way a
  user would. `loctel-e2e` needs `tempfile` for the directory, as a
  dev-dependency (the tests use it, the library does not).
- **Counts are exact, including `dropped_fraction`.** HealthChecks{5} over five
  services is 25 traces, 25 spans and 25 logs dropped; the checkout keeps 15
  spans and 7 logs, so the fraction is 50/72, asserted to within 1e-9 and not
  only `> 0`. These are consequences of the frozen scenarios (decision 0005), and
  a change to a scenario should fail here as it should. The plan's "5 times the
  services" is the 5 probes of `count` for each of the 5 services.
- **The late-span test replays the same trace.** A dropped trace's late spans
  can only be tested if the trace comes again, so after the first send (the real
  scenario, with its pauses: the root last, so what was stored goes) the test
  generates `LateOutOfOrder` with the same seed, which is the same trace ID, and
  sends its three batches in the opposite order, root first, one request each,
  looking at the status after every one. This breaks the "own seed for every
  send" rule of `testing.md` on purpose, and the test says so. It sends the
  batches through `loctel_meshgen::send` with a `Generated` of one batch (its
  fields are public), not through `Harness::send`. A mutant that skipped the
  rules' memory failed it, which is how it was checked.
- **The queries are the plan's, plus the structure that matters.** The
  Grafana-style `>>` query matches both checkouts, because the successful one has
  a `POST /charges` below `web-bff` too. The test says so and also asks for the
  erroring spans below `web-bff`, which only the failing checkout has, and for the
  query with its operands swapped, which matches nothing and is not an error.
  `matched_span_ids` are resolved with `get_span`, so the ids are proved usable,
  not only present.
- **Tool errors are asserted whole.** `invalid_query_is_tool_error` checks the
  exact text (`invalid TraceQL: at column 22: select is not supported yet`), not
  a prefix, since the position and the wording are the agent-facing promise
  (design §6, plan Review Focus 3), and that the session answers the next call.

## Consequences

- A test of the rules writes its own `rules.toml` in a `tempfile::tempdir()`;
  `docs/areas/testing.md` says so under "Writing one".
- Changing a message of the parsers (or `CheckoutFailure`) fails these tests
  first, which is the point of testing the whole path.
- Nothing here is verified on a Mac, and Claude Code itself has not used the
  `query` arguments: items 5 and 6 of the Milestone 2 Mac checklist do that.
