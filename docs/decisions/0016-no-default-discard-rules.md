# 0016: No default discard rules

Date: 2026-10-21
Status: accepted

## Context

Health checks are the noise discard rules exist for (design §7), and the
`HealthChecks` meshgen scenario is written to be silenced by one. The first
version of the Milestone 2 plan therefore had loctel write a `rules.toml` with a
"Health checks" rule already in it, switched on, the first time it ran.

The user decided against that: loctel must not decide what to discard. A
discarded span is gone for good (design §7, "dropped for good"), and a rule
that is on from the first launch drops data the user never asked to lose, for
a reason the user never saw. The plan was changed before any code was written
(commit `de7308d`), and this file records the decision and what follows from
it so that no later agent puts a default back as a convenience.

## Decision

- **A new `rules.toml` holds no rules.** `RulesFile::write_empty` writes only a
  comment header: what the file is, what each key means, a link to
  `docs/areas/rules.md`, and the health-check rule **commented out**, each line
  of `RulesFile::EXAMPLE_RULE` prefixed by `# `. `RulesFile::load` of that file
  gives `Some(vec![])`, so the engine starts with no rules and nothing is
  discarded.
- **The example is in the header, not in the rule list,** so that a person who
  wants it takes one visible step (remove the `# `), and the file shows what the
  format looks like with a realistic query. `EXAMPLE_RULE` is a constant, and
  tests parse it and run it against the health checks and the failing
  checkout, so the example stays correct. The same text is what the
  engine and end-to-end tests use when they need a health-check rule: a test
  that wants the rule on says so.
- **`RulesFile::save` writes the header too,** above whatever rules there are,
  so the explanation does not disappear the first time the rules screen (M4)
  or `set_rules` rewrites the file. Comments a person adds to the file by hand
  are not kept by a save; that is stated in `docs/areas/rules.md`.
- **No other default, anywhere.** The engine, the app and the UI never create a
  rule on their own. "Discard lines like this" (design §7, M4) prefills an
  editor and waits for the user to save.

## Consequences

- A first-time user sees every health check until they write (or uncomment) a
  rule. That is the intended trade: the cost of seeing noise is a rule away,
  and the cost of silently losing data is not recoverable.
- The Mac checklist for Milestone 2 starts from "No discard rules on" and has
  the user enable a rule themselves.
- Tests that need rules build them explicitly. A test that expects a fresh
  engine to drop health checks is a bug in the test.
