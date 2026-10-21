# 0014: LogQL choices

Date: 2026-10-21
Status: accepted

## Context

The Milestone 2 plan (Task 3) fixes the interfaces (`logql::parse`,
`LogQuery::matches`, `LogQuery::service_hint`, `logql::labels`), the grammar to
support, the label mapping, the parsers' headline behaviour, the errors, and the
tests. It leaves open the precedence of label filters, what a missing label is,
what the parsers do with clashes, duplicates and bad input, and how the numeric
filters treat a value that is not a number. Loki's grammar (`syntax.y`, `lex.go`),
its pipeline (`pkg/logql/log`) and its logfmt decoder are readable from GitHub,
grafana.com was not needed. Because discard rules delete data on the strength of
these queries, the choices were checked by **running Loki's own pipeline** (a
small Go program over `github.com/grafana/loki/v3` v3.7.9) next to loctel's on
about 5,000 hand-written and 120,000 random (query, line, labels) rows; the
recipe is in `.claude/skills/add-query-feature/loki-differential.md`. The
choices below are the ones a later agent might be tempted to undo.

## Decision

- **Label filter precedence is yacc's, not C's.** `and` binds tighter than `or`,
  both left to right, but `,` and plain juxtaposition have no precedence in
  `syntax.y`, so wherever yacc cannot compare it shifts: what follows a comma or a
  space is grouped first. `a, b or c` is `a and (b or c)`; `a or b, c` is
  `a or (b and c)`; `a and b, c or d` is `a and b and (c or d)`. Loki prints
  exactly these trees, and so does goyacc on the grammar's label filter rules. The
  parser (`label_chain`) is a shift-reduce loop with an explicit stack that
  reduces only for `and` before `and` or `or`, and for `or` before `or`. The truth
  tables in `tests/logql.rs` pin it. If Loki's grammar ever declares a precedence
  for the comma, change the loop and the tests together.
- **Juxtaposition is supported** (`| a="1" b="2"` is `and`), because the grammar
  has it and Loki's docs say a space is an `and`.
- **A missing label is the empty string for string matchers** (selector and label
  filters), as in Prometheus and Loki, so `{foo!="x"}` and `| foo != "x"` match a
  log without `foo`. **A label with an empty value does not exist**, so an empty
  attribute is skipped (before the first-wins rule), `labels()` never lists an
  empty value, and a query cannot tell the two apart. `scope_name`, `trace_id` and
  `span_id` are labels only when the log has them, so an attribute called
  `trace_id` stays on a log with no trace context. `service_name` and `level`
  always exist and win over attributes of the same mapped name, as do the others
  when present; a resource attribute wins over a log attribute, and the first of
  two attributes with one mapped name wins.
- **The selector reads labels straight from the record** (`labels::lookup`, which
  borrows string attributes and compares keys without building the mapped name),
  and **the full label set is built only when `| json` or `| logfmt` runs**, once
  per `matches` call. The plan says the set is "computed lazily, once per log per
  `matches` call"; this is the laziest reading of that, and a rule such as
  `{service_name="web"} |= "GET /health"` runs on every log of a flood without an
  allocation. The cost is two code paths for one precedence rule, so
  `selector_lookup_agrees_with_the_label_set` compares them on every label of
  several logs, and `labels()` and `lookup` must change together.
- **Numeric and duration filters read the label's text by the kind of the
  literal**, as Loki does: a number literal reads Go's `ParseFloat` forms (sign,
  exponent, `_` between digits, `inf`, `nan`), a duration literal reads Go's
  `ParseDuration` forms (signed, plus the bare `0`). A number literal does not read
  `12ms`, a duration literal does not read `500`. **A label that is missing or
  unreadable makes the filter false, `!=` included.** Loki is different for an
  unreadable label: it keeps the line and sets `__error__="LabelFilterErr"`, which
  is why `| json | status >= 500` keeps a plain-text line there. The plan fixes
  false, and false is also the safe side for a rule that deletes data (a rule must
  not remove a line because the line is not what the rule expected). Missing is
  false in both.
- **Stages run in order; a label filter before a parser does not see what the
  parser extracts.** Loki pushes every single-label filter of the whole pipeline
  into the parsers as an optimisation, so `| x != "1" | logfmt` is also applied to
  the `x` that `logfmt` extracts. That makes a misordered query work in Loki and
  match nothing here. Replicating a pushdown quirk is the wrong trade for a tool
  whose rules delete data: loctel reads the pipeline as the language documents it,
  and the difference only ever matches less. Documented in `areas/query.md` and
  pinned by a test.
- **An empty regex in a label filter is anchored** (`| a =~ ""` matches only an
  empty or missing label). Loki's line-filter simplification makes it match every
  line, which is a bug in how it forgets the anchoring; its own selectors and
  Prometheus do it the way loctel does.
- **`| json` and `| logfmt` are ports of Loki's scanner and decoder, not of "JSON"
  and "logfmt".** The parsers that Loki runs are lenient and quirky, and a rule
  that copies a Grafana query should drop the logs Loki would drop. So: JSON
  ignores what follows the closing `}`, accepts a trailing comma, skips arrays by
  matching their brackets, takes a number as the text up to the next delimiter,
  and turns a string with a bad escape into an empty value; logfmt drops a broken
  pair with the rest of its word (an `=` or a quote in an unquoted value, an
  empty key, an unknown escape in quotes) and lets an unterminated quote swallow
  the rest of the line. **The first value extracted for a name wins**, within one
  line and across parsers (Loki records what it extracted and skips it), and a
  name that is already a label of the log gets `_extracted` (so a `level` field
  in the body does not shadow the severity). A body that is not a JSON object adds
  `__error__="JSONParserErr"` and keeps the log. `__error_details__` is not set.
- **The JSON scanner is hand-written, so the crate still depends only on
  `loctel-model` and `regex`.** `serde_json` would re-format numbers (`129.00`
  would become `129.0`, `1e3` would become `1000.0`), which is a silent mismatch
  for `| json | price = "129.00"`, and would not be lenient where Loki is. The
  scanner is about 150 lines, depth-limited to 128 objects (deeper is a
  `JSONParserErr`, never a stack overflow), skips arrays without recursion, and is
  tested on a 512 KiB stack with a body of 10,000 nested objects.
- **`{}` is the only selector error** (the plan's rule). Loki also rejects a
  selector whose matchers are all "empty-compatible" (`{app=~".*"}`, `{app=""}`,
  `{app!="x"}`) because it has an index to protect; loctel has none and a rule
  like `{service_name!="web"} |= "x"` is reasonable, so those parse. Keywords
  (`json`, `by`, `and`) are accepted as label names where Loki rejects them.
- **Recognised-but-unsupported is wider than the plan's list.** Besides the
  plan's stages and metric functions the parser names the whole function list of
  Loki's lexer for metrics queries (and `sum by (` forms, anywhere in a
  parenthesised start), `|>` and `!>` pattern filters, `or` between line filters
  (`|= "a" or "b"`), `| json a="b"` and `| logfmt a` label extraction, logfmt
  flags, byte-size literals (`10MB`), and `ip(` in label filters as well as line
  filters. All exist in Loki, and "not supported yet" at the right column is
  better than a syntax error for a query copied from Grafana. The metrics message
  is the plan's exact `metrics queries are not supported yet`, so it does not go
  through `QueryError::unsupported`, whose "is" would not agree with the plural.
- **A parenthesised query is supported** (`({a="b"} |= "x")`, Loki's `logExpr`
  rule). It costs nothing and an error would be a surprise; the parentheses count
  against the nesting limit.
- **A dotted or hyphenated label name is an error with a hint** (`` invalid label
  name `a.b`: ... (loctel writes it `a_b`) ``). Pasting OpenTelemetry attribute
  names into a selector is the commonest mistake, and Loki's own message
  (`unexpected DOT`) says nothing useful.
- **Limits: 64 levels of parentheses, 1000 operators**, as decision 0012 asked of
  this parser. Because commas and spaces make a right-deep tree, the chain is
  parsed with an explicit stack: a first version recursed and overflowed a 2 MiB
  test thread at 1000 commas in a debug build. The deepest allowed tree is
  evaluated, cloned, compared and dropped on a 1 MiB stack in the tests.
- **Escapes are Go's, for ASCII.** `\a \b \f \n \r \t \v \\ \"`, `\uHHHH`,
  `\UHHHHHHHH`, and `\xHH` and octal up to `\x7f` (higher ones are raw bytes in
  Go and loctel's strings are UTF-8, so they are an error rather than a different
  character). `"\d"` is an error, as in Go; backticks are raw.
- **Durations are `parse_duration`'s**, so `d`, `w` and `y` (which Loki's lexer
  takes in literals) give `invalid duration` at the literal. A label's text is read
  with Go's `time.ParseDuration`, which has no `d` either.
- **`logql::labels` is exported at the crate root as well** as `logql::labels`,
  because the plan lists it beside `pub mod logql` and the file map puts it in
  `logql/labels.rs`; either reading finds it.
- **Fixtures.** `tests/fixtures.rs` gained the log builders (`checkout_failure_logs`,
  `mk_log`, `log_with_attr`, `log_in_service`, `count_logs`) beside the span ones.

## Consequences

- `docs/areas/query.md` has the LogQL grammar, the label set, the semantics table,
  both parsers, the precedence table, the unsupported list and "Where loctel
  differs from Loki", which is the list to read before changing any of this.
- A new Loki feature moves off the unsupported list by the steps of the
  `add-query-feature` skill, and a change to evaluation semantics should be
  re-checked against Loki's pipeline with `loki-differential.md` (about 2 GB of
  Go modules in the scratchpad, a few minutes).
- Task 4 (the store) uses `LogQuery::service_hint` and `matches`; Task 5 (rules)
  uses `matches` on every arriving log and the parse error as the rule's error.
  Both rely on `matches` taking `&self` with no state, which the test
  `matches_does_not_depend_on_what_ran_before` pins.
