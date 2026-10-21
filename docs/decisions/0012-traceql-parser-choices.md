# 0012: TraceQL parser choices

Date: 2026-10-21
Status: accepted

## Context

The Milestone 2 plan (Task 1) fixes the crate, the interfaces (`QueryError`,
`Pattern`, `parse_duration`, `traceql::parse`, `TraceQuery::needs_whole_trace`),
the supported grammar and the errors it must give. It leaves open the shape of
the syntax tree, "precedence and associativity as in `expr.y`" (to be checked
and recorded), and the edges of the grammar. grafana.com was not needed:
Tempo's `pkg/traceql/expr.y`, `lexer.go`, `parse_test.go` and
`test_examples.yaml` are readable from GitHub. These are the choices a later
agent might be tempted to undo.

## Decision

- **`&&` and `||` share one precedence level and are left-associative**, at
  both the spanset and the field level, because `expr.y` declares
  `%left AND OR` on one line. `a || b && c` is `(a || b) && c`. This is the
  surprising one: the usual rule (and binds tighter) would give different
  results for the same query than Tempo does. The plan says queries pasted from
  Grafana must work unchanged, so Tempo's reading wins, and the tests
  (`field_precedence_follows_tempo`, `spanset_precedence_follows_tempo`) say so.
  The structural operators bind tighter than `&&` and `||` between spansets, and
  comparisons bind tighter than `&&` and `||` inside a filter, both also from
  `expr.y`.
- **`!` takes a whole condition; `!a = b` is an error.** Tempo parses it as
  `(!a) = b` (`NOT` is above comparisons in `expr.y`), which compares a boolean
  to something and which nobody writes on purpose. Reading it as `!(a = b)`
  would be a guess. The error says to write `!(a = b)`. `!a`, `!(...)` and
  `!!a` work.
- **`nil` is resolved by the parser.** `x != nil` is `Exists(x)` and `x = nil`
  is `NotExists(x)`, as in Tempo's own tree, so the evaluator has no `nil`
  value to reason about. `nil` with any other operator is an error.
- **A regex on the right of `=~` or `!~` must be a string literal, and is
  compiled while parsing** (anchored, as the plan requires), so a bad regex is a
  `QueryError` at the pattern, with the one-line reason from the `regex` crate,
  and a rule with one is disabled with an error instead of failing at ingest.
  `.a =~ .b` (a pattern read from another attribute) is not supported.
  `Pattern::anchored` also compiles the bare source first, because wrapping
  `a)|(b` in `^(?:...)$` yields a valid pattern that means something else.
- **Lexing never fails on its own.** A bad token becomes an `Invalid` token that
  carries its error, and the parser reports it only if it reaches it, so the
  first problem in the query is the one reported (`{ name = } @` points at the
  `}`, not at the `@`).
- **Recognised-but-unsupported is wider than the plan's list.** Besides the
  plan's functions and scopes the parser names `min_over_time`, `max_over_time`,
  `avg_over_time`, `sum_over_time`, `compare`, `topk`, `bottomk`; the
  `parent.` scope and `event:`, `link:`, `instrumentation:`, `span:id`,
  `span:parentID`, `span:childCount`, `trace:id` intrinsics; the unscoped
  `parent`, `childCount` and `nestedSet*`; the structural union operators
  `&> &< &>> &<< &~`; arithmetic; a filter as a pipeline stage; and a pipeline
  in parentheses. They all exist in Tempo, and "not supported yet" at the right
  column is better than "unknown identifier" for a query copied from Grafana.
- **Negative number literals are supported** (`{ .a > -1 }`, `count() > -1`),
  because they are values, not arithmetic. A negative duration is not (the
  parsed duration is a `u64`), and says so.
- **Limits: 64 levels of nesting, 1000 operators.** The parser recurses on
  parentheses and `!`, and left-associative chains build left-deep trees that
  the evaluator and `Drop` recurse through. Rules live in a file a person
  edits, and "the app never crashes" is a review focus of this milestone, so
  pathological input is a `QueryError`, not a stack overflow. Both numbers are
  far above anything written by hand.
- **No static type validation.** Tempo rejects `{ status = "error" }` or
  `{ rootServiceName = 1 }` after parsing; loctel parses them and, per Task 2,
  "any other type pair is false". Attribute types are only known at run time
  anyway, and the plan fixes the evaluator's semantics. If silent non-matches
  of statically wrong comparisons become a problem, add a validation pass after
  the parser (and decide whether the rules file treats it as an error).
- **Syntax tree.** `TraceQuery { spanset, stages }`; `SpansetExpr::{Filter,
  Binary}`; `FieldExpr::{And, Or, Not, Compare, Regex, Exists, NotExists,
  Operand}`; `Operand::{Attribute, Intrinsic, Static}`; `Static` holds
  `loctel_model::StatusCode` and `SpanKind` for `ok`/`error` and `server`/
  `client`. `{ }` is `{ true }`. Parentheses leave no node. The types are
  public for M3 and M4 tooling and promise nothing beyond the plan.
- **Dev-dependencies `loctel-meshgen` and `loctel-ingest`** are declared now, as
  the plan's Cargo.toml lists them, although Task 1's tests do not use them yet:
  Task 2's fixtures build the CheckoutFailure trace with them.

## Consequences

- `docs/areas/query.md` has the supported grammar, the unsupported list with
  their error text, the precedence table and the known differences from Tempo
  (string escapes, `d`/`w`/`y` duration units, `minInt`/`maxInt`, static type
  validation).
- A user who writes `a || b && c` and expects the C reading gets Tempo's
  result. If Grafana's behaviour ever changes (`expr.y` gains separate
  precedences for `AND` and `OR`), update the two parsers' loops, the tests
  that pin it and `query.md` together.
- The LogQL parser (Task 3) reuses `QueryError`, `Pattern` and `parse_duration`,
  and should do the same for limits.
