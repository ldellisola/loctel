---
name: add-query-feature
description: "Add syntax or semantics to loctel's TraceQL or LogQL (crates/loctel-query). Use whenever a query feature moves off the 'not supported yet' list, a new operator, function, stage or literal is added, or a query pasted from Grafana behaves differently from Grafana. Covers the lexer token, the AST node, the parser rule, the evaluator, the tests with positions, the unsupported list and the docs table."
---

# Add a query feature

`loctel-query` parses and evaluates the two languages Grafana users already
write: Tempo's TraceQL and Loki's LogQL. The rule of the crate is that **a query
copied from Grafana either means what it means there, or fails with
`"<feature> is not supported yet"` at the feature's column. It never matches
differently in silence.** Discard rules delete data on the strength of these
queries, so a wrong match is data loss.

Read `docs/areas/query.md` first (the supported subset, the precedence rules,
the semantics tables and the list of known differences), and the decision
records it points at (`docs/decisions/0012` to `0014`). For LogQL there is also
`loki-differential.md` beside this file: how to run Loki's own pipeline next to
loctel's over thousands of (query, line, labels) rows, which is how the LogQL
semantics were checked. The files:

| Language | Lexer | AST | Parser | Evaluator |
|---|---|---|---|---|
| TraceQL | `src/traceql/lexer.rs` | `ast.rs` | `parser.rs` | `eval.rs`, `view.rs` |
| LogQL | `src/logql/lexer.rs` | `ast.rs` | `parser.rs` | `eval.rs`, `labels.rs` |

## Steps

1. **Find the authority.** Do not write the feature from memory of the Grafana
   docs. The grammars are Tempo `pkg/traceql/expr.y` and `lexer.go`, and Loki
   `pkg/logql/syntax/syntax.y` and `lex.go` (all readable at
   `raw.githubusercontent.com/grafana/<tempo|loki>/main/...`; the evaluation
   semantics are in `pkg/traceql/ast_execute.go` and `pkg/logql/log/*.go`).
   Precedence in these grammars comes from `%left` lines and from yacc's default
   of shifting where it cannot compare, which is not the C-like reading. When it
   matters, run the grammar's rules through `goyacc` with a toy lexer (Go is
   installed: `go run golang.org/x/tools/cmd/goyacc@latest`) and see what it
   builds, as `docs/areas/query.md` did for the label filters.
2. **Write the tests first** and watch them fail:
   - a **parse test with a position** in `tests/traceql_parse.rs` or in
     `tests/logql.rs`: the query, the column of the error when it must fail (the
     tables there use "the text the error points at" so nobody counts columns by
     hand), and what the tree is when it must parse;
   - an **evaluation test on the fixtures** in `tests/traceql_eval.rs` or
     `tests/logql.rs`: the `CheckoutFailure` trace and logs (`tests/fixtures.rs`,
     built by meshgen and the real OTLP conversion), with every expected answer
     worked out by hand from the M1 plan's table, plus a hand-made span or log
     for the edge cases;
   - for a feature that changes what an old query means, a test that pins the
     old behaviour.
3. **The lexer.** A token, with its character position (never a byte offset).
   Lexing never fails as a whole: a bad token becomes `Tok::Invalid(error)`, and
   the parser reports it only if it reaches it, so the first problem in the
   query is the one reported.
4. **The AST node.** In `ast.rs`. The types are public for later tooling and
   promise nothing beyond the plan, but derive `Clone, Debug, PartialEq`
   (queries are compared in tests and cloned into rule sets).
5. **The parser rule.** Follow the existing style (precedence climbing in
   TraceQL; a shift-reduce loop for LogQL label filters). Two things to keep:
   - **Limits.** Nesting is capped at 64 levels and operators at 1000 in both
     parsers, because rules live in a file a person edits and the app must never
     crash on it. A rule that recurses needs to count. Test it on a small stack
     (`std::thread::Builder::new().stack_size(...)`), as the limit tests do.
   - **Regexes** are compiled while parsing (`Pattern::anchored` for TraceQL and
     LogQL label matchers, `Pattern::unanchored` for LogQL line filters), so a
     bad regex is an error at its position and a rule with one is disabled, not
     a failure at ingest.
6. **The evaluator.** `matches_span` (TraceQL) must agree with `evaluate` on a
   one-span trace, and `needs_whole_trace` must say true for anything that looks
   beyond one span. For LogQL, the selector reads labels with `labels::lookup`
   and the pipeline builds the full `LabelSet` only when a parser runs; if you
   add a label source, change `labels()` and `lookup` together (the test
   `selector_lookup_agrees_with_the_label_set` fails when they drift).
7. **Take the feature off the unsupported list.** The error table in the parser
   tests (`UNSUPPORTED` in `tests/logql.rs`, `unsupported_features_...` in
   `tests/traceql_parse.rs`) and `UNSUPPORTED_*` in the parser source. A feature
   that was an error and is now supported changes its row of the tests, not just
   the code.
8. **Update the docs in the same change:** the supported-grammar block, the
   semantics table and the "not supported" table in `docs/areas/query.md`, the
   "known differences from Tempo/Loki" list if the feature is only partly the
   same, and a decision record in `docs/decisions/` when you chose between two
   readings of the grammar.
9. **Run the gate:** `cargo fmt --check`, `cargo clippy --workspace --all-targets
   -- -D warnings`, `cargo test --workspace`. Never skip or loosen a test.

## Things that go wrong

- **Precedence by feel.** `a || b && c` is `(a || b) && c` in TraceQL, and
  `a, b or c` is `a and (b or c)` in LogQL. Both are the grammar's, and the
  tests that pin them are there so nobody "fixes" them.
- **Anchoring.** TraceQL `=~` and LogQL label matchers are `^(?:re)$`; only
  LogQL line filters (`|~`, `!~`) are unanchored. `Pattern` knows which it is.
- **Missing values.** In TraceQL a missing attribute makes every comparison
  false, `!=` included. In LogQL a missing label is the empty string for string
  matchers (so `!=` is true) and makes a numeric filter false. Do not unify them.
- **Positions are characters.** `QueryError::position` indexes characters, so
  non-ASCII queries stay right. `at column N` is `position + 1`.
- **A feature that "almost" works.** Prefer the error. If part of a Grafana
  feature cannot be done faithfully (a unit, an escape, a flag), parse the whole
  construct and say what is missing at its position; do not accept the
  construct and ignore the part.
