# Query languages (`loctel-query`)

TraceQL for traces and LogQL for logs, parsed and evaluated in pure Rust. The
crate depends only on `loctel-model` (and `regex`), so it builds and tests on
Linux with no UI. Design references: §6 (querying), §7 (discard rules, which
are written in these languages) and §8 (MCP `query` arguments).

Syntax and semantics follow Grafana: Tempo's TraceQL and Loki's LogQL. A query
pasted from Grafana should work unchanged. Where loctel does not support a
feature, the parser says so with the feature's name and position, and never
gives results that quietly differ from Grafana's.

Status (Milestone 2, in progress): the shared pieces, the **TraceQL lexer,
parser and evaluator** (`TraceView`, `evaluate`, `matches_span`) and **LogQL**
(lexer, parser, label set and evaluator: `logql::parse`, `LogQuery::matches`,
`logql::labels`) are implemented, and the store uses both as search filters
(`TraceFilter::query`, `LogFilter::query`; see [`store.md`](store.md), "Query
filters"). The rules and MCP that use them follow (the M2 plan,
`superpowers/plans/2026-10-21-loctel-m2-query-rules.md`, Tasks 5 to 7). This
page grows with each of them.

## Shared pieces

- **`QueryError { message, position }`.** `position` is a 0-based **character**
  index into the query (not a byte offset), so it is right for non-ASCII
  queries. It can equal the query's length, which means "at the end".
  `Display` is `at column {position + 1}: {message}`, the way an editor counts.
  It implements `std::error::Error`. Every parse failure is one of these, and
  the UI, MCP and the rules file show it as it is.
- **`Pattern`**, a compiled regex, anchored or not, because the two languages
  need both (design §6):

  | Where | Anchoring |
  |---|---|
  | TraceQL `=~`, `!~` | **anchored**: `^(?:src)$`, as in Tempo. `name =~ "POST"` does not match `POST /charges`. |
  | LogQL label matchers (`=~`, `!~` in selectors and label filters) | **anchored**, as in Prometheus |
  | LogQL line filters (`\|~`, `!~` on the line) | **unanchored**: matches anywhere in the line |

  `Pattern::anchored(src)` checks `src` on its own before wrapping it, so a
  source such as `a)|(b` is an error instead of a valid-looking pattern with
  different meaning. `source()` is what the user wrote, without the anchors.
  Two patterns are equal when their source and anchoring are, which lets
  syntax trees that hold patterns be compared in tests.
- **`parse_duration(text) -> Option<u64>`**, in nanoseconds: `500ms`, `1.5s`,
  `1h30m`. Units are `ns`, `us`, `µs` (micro sign or Greek mu), `ms`, `s`, `m`
  and `h`. Every number needs its unit. A value that does not fit `u64`
  nanoseconds is `None`, never a wrapped number. Tempo and Loki also take `d`,
  `w` and `y`; loctel does not yet, and says `invalid duration` at the literal.
- **The regex dialect** is the `regex` crate's: RE2-style, like Go's `regexp`
  that Tempo and Loki use. Neither has backreferences or lookaround. The one
  difference that matters in practice is that `\d`, `\w`, `\s` and `\b` are
  Unicode-aware in the `regex` crate and ASCII-only in Go, so on non-ASCII
  text they can match more here (`[0-9]` is the same in both). A bad regex is
  a `QueryError` at the pattern, with a one-line reason.

## TraceQL (`loctel_query::traceql`)

```rust
use loctel_query::traceql;

let query = traceql::parse(r#"{ resource.service.name = "web-bff" } >> { status = error }"#)?;
query.needs_whole_trace();   // true: a structural operator needs the other spans

// Run it over one trace's spans (here `spans: &[Span]`).
let matched: Vec<usize> = query.evaluate(&traceql::TraceView::new(spans));
```

`parse(text) -> Result<TraceQuery, QueryError>`. The AST (`TraceQuery`,
`SpansetExpr`, `FieldExpr`, `Operand`, ...) is public so later tooling can walk
it, but it promises nothing beyond the plan. A parenthesised group leaves no
node, `{ }` is `{ true }`, `x = nil` is `NotExists(x)` and `x != nil` is
`Exists(x)` (as Tempo's own tree does), and a regex on the right of `=~` is
compiled, anchored, while parsing.

`needs_whole_trace()` is true when the query cannot be judged one span at a
time: a structural operator, `&&` or `||` between two spanset filters, a
pipeline stage, or `rootName`, `rootServiceName`, `traceDuration` (or their
scoped forms `trace:rootName`, `trace:rootService`, `trace:duration`). Otherwise
it is a single filter and each span stands alone. The rules engine uses this to
decide whether a rule can run on each arriving span or must see the trace so
far (design §7).

### What is supported

```
query        = spanset_expr ( "|" "count()" CMP number )*
spanset_expr = filter
             | spanset_expr OP spanset_expr        OP: && || > < >> << ~ !> !< !>> !<< !~
             | "(" spanset_expr ")"
filter       = "{" "}"  |  "{" field_expr "}"
field_expr   = field_expr ( "&&" | "||" ) field_expr
             | "!" field_expr
             | "(" field_expr ")"
             | operand CMP operand                 CMP: = != > < >= <= =~ !~
             | attribute                           (a bare boolean, e.g. { span.cache.hit })
             | "true" | "false"
operand      = attribute | intrinsic | static
```

- **Attributes:** `span.<name>`, `resource.<name>` and unscoped `.<name>`. A
  name is dotted (`span.http.response.status_code`) or quoted after the scope
  (`span."http status"`). Names may contain `-`, `/` and `:`.
- **Intrinsics**, old style: `name`, `status`, `statusMessage`, `kind`,
  `duration`, `rootName`, `rootServiceName`, `traceDuration`. Scoped style:
  `span:name`, `span:status`, `span:statusMessage`, `span:kind`,
  `span:duration`, `trace:rootName`, `trace:rootService`, `trace:duration`.
  Both styles give the same tree.
- **Static values:** strings in `"..."` (escapes `\"` `\\` `\n` `\t` `\r`) or in
  backticks (raw: no escapes, which keeps regexes readable); integers
  (`-5` too); floats; durations (`500ms`, `1h30m`); `true`, `false`, `nil`;
  status `ok`, `error`, `unset`; kind `unspecified`, `internal`, `server`,
  `client`, `producer`, `consumer`.
- **Pipeline:** `| count() CMP number`, with CMP one of `= != > < >= <=`. It
  can repeat: `| count() >= 2 | count() < 10`.
- **`nil`** compares only with `=` and `!=`.
- **Comparisons do not chain** (`a = 1 = 2` is an error) and a lone operand is a
  condition only if it is an attribute or `true`/`false` (`{ name }` is an error).

### What is not supported

Each of these is recognised and gives `"<feature> is not supported yet"` at the
feature's position. The message names the feature, for example
`at column 22: select is not supported yet`.

| Feature | Error names |
|---|---|
| `select(`, `by(`, `avg(`, `min(`, `max(`, `sum(`, `coalesce(` | the function |
| `rate(`, `count_over_time(`, `quantile_over_time(`, `histogram_over_time(`, `min_over_time(`, `max_over_time(`, `avg_over_time(`, `sum_over_time(`, `compare(`, `topk(`, `bottomk(` (metrics) | the function |
| `with(...)` query hints | `with` |
| `event.`, `link.`, `instrumentation.`, `parent.` attribute scopes | e.g. the `event.` scope |
| `event:`, `link:`, `instrumentation:` intrinsics, `span:id`, `span:parentID`, `span:childCount`, `trace:id` | e.g. `` `span:id` `` |
| unscoped `parent`, `childCount`, `nestedSetLeft`, `nestedSetRight`, `nestedSetParent` | e.g. `` `childCount` `` |
| structural union operators `&>` `&<` `&>>` `&<<` `&~` | the operator |
| arithmetic (`+ - * / % ^`, unary minus on anything but a number literal) | arithmetic |
| a negative duration (`-1s`) | a negative duration |
| `{ a } \| { b }` (a filter as a pipeline stage) | a spanset filter after `\|` |
| a pipeline inside parentheses, `({ a } \| count() > 1) >> { b }` | a pipeline inside parentheses |

Anything else that does not fit the grammar is a plain syntax error, such as
`expected a value, found `}`` at the offending token, or `expected `}``
at the end of the query when the closing brace is missing. A bare word that is
not a keyword or an intrinsic is `unknown intrinsic `nme``; when it is followed
by a dot (`http.status = 500`) the message adds that attributes need a scope.

Where loctel is **stricter** than Tempo, it is an error and never a different
answer: `{ !a = b }` (Tempo reads it as `(!a) = b`; write `!(a = b)`),
chained comparisons, and `nil` with anything but `=` and `!=`.

Where it is **looser**, the difference is only in what Tempo rejects after
parsing: Tempo also type-checks statically known operands, so `{ status = "error" }`
or `{ rootServiceName = 1 }` fail validation there. Here they parse and never
match (Task 2: "any other type pair is false"). If that turns out to bite, a
validation pass belongs after the parser, not in it.

Other known differences from Tempo's lexer: string escapes are the five above
(Tempo accepts Go's whole set, `\x41`, `é`...), durations lack `d`, `w` and
`y`, and the `minInt` and `maxInt` statics are not recognised.

### Precedence and associativity

Checked against Tempo's `pkg/traceql/expr.y` (and `lexer.go`, and the examples
in `test_examples.yaml`) on 2026-10-21. The grammar declares, loosest first:

```
%left PIPE
%left AND OR
%left EQ NEQ LT LTE GT GTE NRE RE DESC ANCE SIBL NOT_CHILD NOT_PARENT NOT_DESC NOT_ANCE ...
%nonassoc INTEGER FLOAT
%left ADD SUB
%left NOT
%left MUL DIV MOD
%right POW
```

What follows, and what the parser does:

- **`&&` and `||` are one level, left-associative**, at both the spanset and the
  field level. This is not the usual "and binds tighter than or": `a || b && c`
  is `(a || b) && c`, and `a && b || c` is `(a && b) || c`. loctel follows
  Tempo, because a query that means something in Grafana must mean the same
  here. Use parentheses when you mix them.
- **Between spansets, the structural operators bind tighter than `&&` and
  `||`**, and are left-associative among themselves: `A && B >> C` is
  `A && (B >> C)`, `A >> B && C` is `(A >> B) && C`, and `A >> B ~ C` is
  `(A >> B) ~ C`. (`!~` is the "not sibling" operator between spansets and the
  "not regex" operator inside a filter.)
- **Inside a filter, a comparison binds tighter than `&&` and `||`**:
  `.a = 1 || .b = 2 && .c = 3` compares first, then chains left to right.
- **`!` binds tighter than a comparison** in Tempo, so `!a = b` is `(!a) = b`.
  Nobody means that, so here it is an error that suggests `!(a = b)`; `!a`,
  `!(...)` and `!!a` are fine.
- **A pipeline applies to the whole spanset expression** before it:
  `{ a } >> { b } | count() > 1` counts the spans `>>` returned.
- Parentheses override all of it, and a parenthesised single filter is just that
  filter (`needs_whole_trace` stays false).

### Lexing rules that matter when copying from Grafana

- After `.`, `span.` and `resource.` the lexer reads an **attribute name** that
  runs until whitespace or one of `{}()=~!<>&|^,`. That is why
  `{.a="x"||.b="y"}` needs no spaces and why `span.http-route` is one name. A
  name can hold `"quoted"` parts (`span."http status"`) with `\"` and `\\`.
- A number directly followed by units is one duration (`1h30m`, `500ms`, `2µs`).
  `.5` is a float, as in Tempo.
- A keyword is a keyword only where a value is expected: `span.error` is an
  attribute named `error`.

### Limits

The parser recurses only on parentheses and `!`, and refuses past **64** levels
(`query is too deeply nested`). It also refuses more than **1000** operators in
one query (`query has too many operators`), because a long `a || b || c ...`
chain builds a tree as deep as it is long and the evaluator walks it
recursively. A rules file edited by hand must never be able to overflow the
stack, so both are errors, not crashes.

### Evaluation

```rust
let view = TraceView::new(spans);        // indexes the parents once
let matched = query.evaluate(&view);     // indices into `spans`, ascending, no duplicates
query.matches_span(&span);               // Some(bool) when !needs_whole_trace(), else None
```

`TraceView::new(spans)` indexes the parent links and numbers the tree, so that
a trace of any shape costs O(n) to prepare and ancestry is two integer
comparisons (`is_ancestor(a, d)`). It never recurses, so a chain of 100,000
spans is fine. `evaluate` returns the matched spans; **a trace matches a
query when the result is not empty.** It is linear in the number of spans
times the size of the query: the structural operators make one pass over the
spans in preorder instead of testing pairs (100,000 spans, a structural query:
about 50 ms in a debug build). `matches_span` builds no view and allocates
nothing of its own, and is what the rules engine calls on each arriving span;
it agrees with `evaluate` on a one-span view.

`evaluate` and `matches_span` cannot fail: once a query has parsed, a
comparison that makes no sense is simply false.

| Construct | Meaning |
|---|---|
| `{ expr }` | the spans for which `expr` is true; `{ }` is every span |
| `.x` | the span attribute `x` if the span has it, else the resource attribute |
| `span.x`, `resource.x` | that scope only; a span attribute of another type does not fall through to the resource |
| missing attribute | **every comparison is false**, including `!=` and `!~`; `x = nil` is true, `x != nil` is false |
| present attribute | `x != nil` is true, `x = nil` false (intrinsics are always present, except the root fields below) |
| `int` vs `int`, `float` vs `float`, `int` vs `float` | numeric (an int and a float compare as numbers: `500 = 500.0`) |
| `string` vs `string` | `=`, `!=` by value; `<` `>` `<=` `>=` byte by byte (lexical) |
| `bool` | `=` and `!=` only; `>` and friends are false |
| duration vs duration | numeric, on nanoseconds: `duration`, `traceDuration` (and `trace:duration`) and duration literals |
| `status`, `kind` | against their keywords (`error`, `server`, ...); `=` and `!=` only; against a string or a number they are false |
| `statusMessage`, `name` | strings |
| any other pair of types | **false**, with no error (`{ span.http.response.status_code = "500" }` on an int attribute matches nothing) |
| `=~`, `!~` | the left side must be a string, else false (for `!~` too); fully anchored, so `name =~ "POST"` does not match `POST /charges`; `!~` is exactly the opposite for strings |
| bare attribute `{ span.cache.hit }` | true when it is the boolean `true`; unknown (see below) when missing or not a boolean |
| `!` | negates a condition; `!(span.x = 1)` is true on a span without `x` (the comparison is false there) |
| `a && b`, `a \|\| b` inside a filter | as in Tempo, one precedence level, left to right (parser section); `\|\|` is true if either side is true, `&&` false if either is false |
| `rootName`, `rootServiceName` | the trace's root span's name and service, for every span of the trace; **missing while no root has arrived** (a comparison is false, `rootName = nil` true) |
| `traceDuration` | earliest span start to latest span end of the trace, clamped at 0; needs no root |
| `A > B` | the spans of B whose parent is in A |
| `A < B` | the spans of B that are the parent of some span of A |
| `A >> B` | the spans of B with an ancestor in A |
| `A << B` | the spans of B that are an ancestor of some span of A |
| `A ~ B` | the spans of B that share a parent with a span of A, (a span is never its own sibling) |
| `A !> B`, `A !< B`, `A !>> B`, `A !<< B`, `A !~ B` | the spans of B **without** that relationship. With nothing in A they give every span of B, as Tempo does |
| `A && B` (spansets) | the union of A and B when both are non-empty, else nothing |
| `A \|\| B` (spansets) | the union |
| `... \| count() OP n` | keeps the whole current result when its size satisfies the comparison, else gives nothing. Stages chain, and the stage applies to the whole spanset expression before it |

Two things about "unknown". A bare attribute that is missing, or not a boolean,
is neither true nor false. A span matches only when the filter is **true**, so
`{ span.flag }`, `{ !span.flag }` and `{ span.flag && true }` all match nothing
on a span without `flag`, while `{ span.flag || true }` matches (true wins an
`||`, false wins an `&&`, and `!` of unknown stays unknown). Tempo gets the same
answers by treating `!` of a missing attribute as an error that fails the span.

**The shape of the trace.** A trace is whatever has arrived, so:

- A span whose **parent is missing** is the root of its own subtree: it has no
  parent (`> <` and `~` skip it), its own descendants stay its descendants, and
  it is not *the trace's* root. `root()` is the store's rule: the parentless
  span that started first, ties to the lowest span ID. Several parentless spans
  are not siblings, and neither are an orphan and the root: a missing parent is
  not a parent (Tempo also never matches `~` for spans without a parent).
- Bad data cannot hang or crash it: a span that names itself as its parent is
  a root of its own subtree; a **loop of parents** is cut at its lowest-numbered
  span; with **duplicate span IDs**, children attach to the first.

**Where loctel is narrower than Tempo** (all of them never an error, never a
different answer for a query that is right in both):

- An **int or float attribute does not compare with a duration literal**
  (`{ span.latency_ns > 1s }` matches nothing). Tempo treats durations as
  numbers of nanoseconds there. The plan fixes durations as comparing with
  durations only; changing it means adding the pairs in `compare` in
  `traceql/eval.rs`.
- **Array, bytes and map attributes** are present (`!= nil` is true) but equal
  to nothing and ordered against nothing. Tempo matches an array attribute when
  any of its elements matches.
- `traceDuration` is the plain earliest start to latest end. The store's trace
  duration counts each span's end as no earlier than its own start, which
  differs only when a span's clock is skewed (end before start).
- Tempo shows `<root span not yet received>` for the root fields of a trace
  without a root; here they are missing.

### Tests

`crates/loctel-query/tests/traceql_parse.rs`: the plan's must-parse list, the
supported subset of Tempo's own `test_examples.yaml`, error positions and
messages (every unsupported feature and a table of syntax errors), precedence
and associativity (tree shapes), the AST of each construct, `needs_whole_trace`,
durations, `Pattern`, character-position handling, every truncation of every
query (no panics), and the depth and operator limits. During Task 1 the parser
was also run over the 284 valid, 87 syntax-error and 70 validation-error
queries of Tempo's `test_examples.yaml` (plus its 22 "unsupported" ones):

- every one of the 87 syntax-error queries is rejected here;
- every valid query that is not accepted uses a feature in the table above,
  or is a parenthesised value (`duration < (10s * 20s)`), or a scalar filter
  that starts a query (`count() > 1`), which are plain syntax errors here;
- 13 of the 70 validation-error queries are accepted, all of them statically
  ill-typed comparisons (`{ 1 = "foo" }`, `{ rootServiceName = 1 }`,
  `{ span:status = nil }`), as described above.

`crates/loctel-query/tests/traceql_eval.rs` (with `tests/fixtures.rs`, which
builds the fixed `CheckoutFailure` trace through meshgen and the real OTLP
conversion, and names its spans by their row in the M1 plan's table): the
plan's examples, every operator and the negated forms (with each answer worked
out by hand from the table), typing (a table of queries that must and must not
match a span with every attribute type), missing attributes, unknown logic, the
root fields, `matches_span` against `evaluate`, orphans, loops, duplicate IDs,
an empty trace, a chain of 100,000 spans, and the structural operators against
a brute-force check on a 2,000-span trace. Run it with
`cargo test -p loctel-query --test traceql_eval`.

## LogQL (`loctel_query::logql`)

```rust
use loctel_query::logql;

let query = logql::parse(r#"{service_name="payments-api"} |= "card" | json | status >= 500"#)?;
query.service_hint();                  // Some("payments-api"): a prefilter for the store
query.matches(&log);                   // log: &LogRecord
let labels = logql::labels(&log);      // the label set the query reads, sorted by name
```

`parse(text) -> Result<LogQuery, QueryError>`. `matches` cannot fail: once a
query has parsed, a comparison that makes no sense (a number filter on a label
that is not a number) is simply false. The AST (`LogQuery { matchers, stages }`,
`Stage`, `LineFilter`, `LabelFilter`, `StringMatcher`, `NumberOp`) is public so
later tooling can walk it, but it promises nothing beyond the plan. `,`, a
space and `and` between label filters are all `LabelFilter::And` in the tree,
parentheses leave no node, and every regex is compiled while parsing (so a bad
one is an error at the pattern, and a rule with one is disabled with the error).
`logql::labels` is also exported as `loctel_query::labels`.

`service_hint()` is the value of an exact `service_name="..."` selector matcher
(`=`, not `=~`, not `!=`), or `None`. Every log the query matches comes from
that service, so the store can skip every other one without evaluating
anything.

### What is supported

```
query        = [ "(" ]... selector stage* [ ")" ]...       (parentheses around the whole query)
selector     = "{" matcher ( "," matcher )* "}"             (at least one matcher; no trailing comma)
matcher      = label ( "=" | "!=" | "=~" | "!~" ) string
stage        = ( "|=" | "!=" | "|~" | "!~" ) string          line filter, on the body
             | "|" "json"
             | "|" "logfmt"
             | "|" label_expr
label_expr   = label_expr ( "and" | "or" | "," | (nothing) ) label_expr
             | "(" label_expr ")"
             | label ( "=" | "!=" | "=~" | "!~" ) string
             | label ( "==" | "=" | "!=" | ">" | ">=" | "<" | "<=" ) number
             | label ( "==" | "=" | "!=" | ">" | ">=" | "<" | "<=" ) duration
```

- **Strings** are `"..."` or `` `...` ``. Double-quoted ones take Go's escapes
  (`\a \b \f \n \r \t \v \\ \"`, `\ooo` and `\xHH` for ASCII, `\uHHHH`,
  `\UHHHHHHHH`), and `"\d"` is an error as in Go; backticks are raw, which keeps
  regexes readable (`` |~ `\d+ items` ``).
- **Labels** are letters, digits and `_` (`service_name`, not `service.name`):
  a dotted name is an error that says how loctel spells it
  (`` invalid label name `deployment.environment`: ... (loctel writes it `deployment_environment`) ``).
- **Numbers** are decimals with an optional sign and exponent (`500`, `-1.5`,
  `.5`, `1e-3`); **durations** are `500ms`, `1.5s`, `1h30m` (units `ns us µs ms s
  m h`, signed). `label = 5` and `label == 5` are the numeric comparison;
  `label = "5"` compares text.
- `#` starts a comment that runs to the end of the line. Keywords (`json`, `and`,
  `or`) are case-insensitive, as in Loki.
- A query may be wrapped in parentheses, as Loki allows (`({a="b"} |= "x")`).
- A label named like a function (`ip`, `rate`, `count`, `sum`) is a label:
  functions are only recognised before `(`.

### The label set

What a query reads. Built from the log record (`logql::labels(&log)`, sorted by
name, one entry per name), following design §6.2:

| Label | Value |
|---|---|
| every resource attribute | its key with each character outside `[A-Za-z0-9_]` turned into `_` (`loki_label_name`), and its display string (numbers and booleans as text, bytes as hex, arrays and maps as compact JSON) |
| every log attribute | the same |
| `service_name` | the service (`unknown_service` when the resource has no name) |
| `level` | the severity: `trace`, `debug`, `info`, `warn`, `error`, `fatal`, or `unknown` |
| `scope_name` | the instrumentation scope's name, when it has one |
| `trace_id`, `span_id` | the lowercase hex IDs, when the log has them |

- **Clashes:** the five computed labels win over an attribute of the same mapped
  name (a log attribute called `level` is hidden by the severity; `| json` can
  still reach a `level` field of the body, see below); a resource attribute wins
  over a log attribute; of two attributes with one mapped name the first wins.
  (An attribute called `trace_id` stays when the log has no trace context.)
- **A label with an empty value does not exist**, as in Prometheus and Loki. This
  is why a missing label compares as the empty string, and why an attribute with
  an empty value is skipped before the first-wins rule.
- The selector reads these straight from the record, with no allocation for string
  attributes. The whole set is built once per `matches` call, as a copy, only
  when `| json` or `| logfmt` runs, so one log's extraction never reaches the next
  and a rule such as `{service_name="web"} |= "GET /health"` allocates nothing.
  The test `selector_lookup_agrees_with_the_label_set` fails if the two drift.

### Evaluation

`matches(log)`: every selector matcher holds, and the log gets through every
stage, in order.

| Construct | Meaning |
|---|---|
| `{label = "x"}`, `!=` | exact text; **a missing label is the empty string**, so `{foo=""}` matches a log without `foo` and `{foo!="x"}` matches it too |
| `=~`, `!~` (selector and label filters) | **fully anchored**: `^(?:re)$`. `{service_name=~"orders"}` does not match `orders-api`. `!~` is exactly the opposite |
| `\|= "x"`, `!= "x"` | the body contains the text, case-sensitively; `\|= ""` matches every line and `!= ""` none |
| `\|~ "re"`, `!~ "re"` | **unanchored**: matches anywhere in the line (`\|~ "attempt"` matches `retrying (attempt 2 of 2)`) |
| line filters after `\| json` | still see the original body, not a parsed field |
| `\| label = "x"`, `!=`, `=~`, `!~` | as the selector, on the label set plus what the parsers extracted so far |
| `\| label > 5` (number) | the label's text is read as a number (Go's `ParseFloat`: a sign, an exponent, `_` between digits, `inf` and `nan`); **a missing label, or one that is not a number, makes the filter false**, `!=` included |
| `\| label > 10ms` (duration) | the label's text is read as a Go duration (`12ms`, `1.5s`, `1h30m`, signed, a bare `0`), and compared in nanoseconds; **missing or not a duration is false**. A number literal does not read `12ms`, and a duration literal does not read `500` |
| `a and b`, `a, b`, `a b` | both; `a or b` either; see "Label filter precedence" |
| separate stages | each must hold: `\| a \| b` is `a and b` |
| `\| json`, `\| logfmt` | never drop a log; see "The parsers" |

### The parsers

Both add labels on top of the log's own. Names that are already labels of the log
get `_extracted` after them (a `level` field in the body is `level_extracted`,
because `level` is the severity), as in Loki. **The first value extracted for a
name stays**, within one line and across parsers (`a=1 a=2` gives `a="1"`); an
empty value is no label.

- **`| json`.** The body must be one JSON object. String, number and boolean
  fields become labels named by their key, with nesting flattened by `_`
  (`{"user":{"id":42}}` gives `user_id="42"`). Keys are made into label names like
  attribute keys (each odd character `_`, a leading digit gets a `_` in front, a
  blank key leaves no mark on the name). Arrays and nulls are skipped. Numbers
  keep the text they have in the body (`129.00` stays `129.00`, it is not
  re-formatted). A body that is not a JSON object, or is cut off, adds
  `__error__="JSONParserErr"` (the labels read before the problem stay) and the
  log is kept; `| json | __error__ = ""` drops those. Loki's scanner is lenient in
  places, and loctel follows it so a half-JSON body gives the labels it gives
  there: anything after the closing `}` is ignored, a trailing comma is fine,
  arrays are skipped by matching brackets (not read), a number is whatever runs
  to the next delimiter, and a string with a bad escape is a field with an empty
  value (so no label, and no error). Nesting deeper than **128** objects is an
  error (a hostile body must not overflow the stack).
- **`| logfmt`.** Loki's decoder in its default, lenient mode, ported:
  `key=value` pairs separated by whitespace; a value is `"quoted"` (escapes
  `\" \\ \/ \' \b \f \n \r \t \uXXXX`) or runs to the next whitespace; a key with
  no value or an empty one is skipped. A pair that breaks the format is dropped
  together with the rest of its word, and the line goes on: an empty key (`=x`), a
  quote in a key or in an unquoted value, an `=` in an unquoted value
  (`path=/a?x=1` gives no `path`), an escape that does not exist inside quotes. A
  quote that never closes drops the rest of the line. A line that is not logfmt
  adds nothing and is no error.

### Label filter precedence

Loki's `syntax.y` declares `%left OR` below `%left AND UNLESS` and gives `,` and
plain juxtaposition no precedence at all, so yacc **shifts** wherever it cannot
compare. That makes `and` bind tighter than `or` (both left to right), and makes
a comma or a space an `and` that reaches to the right: whatever follows it is
grouped first, even an `or`. The result is not what "comma means and" suggests:

| Written | Is read as |
|---|---|
| `a or b and c` | `a or (b and c)` |
| `a and b or c` | `(a and b) or c` |
| `a, b or c` and `a b or c` | `a and (b or c)` |
| `a or b, c` and `a or b c` | `a or (b and c)` |
| `a and b, c or d` | `a and b and (c or d)` |
| `(a or b), c` | `(a or b) and c` |
| `a \| b` (two stages) | `a and b` |

The parser (`label_chain` in `logql/parser.rs`) is a shift-reduce loop that
reduces the join on top of its stack only for `and` before `and` or `or`, and for
`or` before `or`; every other case shifts. This was checked three ways: by reading
the grammar, by running its label filter rules through goyacc with a toy lexer,
and by running Loki's own parser over the same queries (it prints the tree, and
`a and b, c or d` comes out `a , (b , (c or d))`). `tests/logql.rs` pins all of
them with truth tables over four labels.

### What is not supported

Each of these is recognised and gives `"<feature> is not supported yet"` at the
feature's position (the word itself, not the `|` before it), for example
`at column 22: line_format is not supported yet`. A parse error the grammar gives
is a plain syntax error (`expected a string, found `}``).

| Feature | Error says |
|---|---|
| a metrics query: starts with `rate(`, `count_over_time(`, `bytes_rate(`, `sum(`, `sum by (`, `topk(`, `label_replace(`, ... (every Loki range, vector and label function) | `metrics queries are not supported yet`, at the function |
| `\| line_format`, `\| label_format`, `\| unwrap`, `\| decolorize`, `\| drop`, `\| keep`, `\| unpack`, `\| pattern "..."`, `\| regexp "..."` | the word, e.g. `line_format is not supported yet` |
| `\| json a="b.c"`, `\| json a` (label extraction expressions) | `json label extraction` |
| `\| logfmt a`, `\| logfmt a="b"` | `logfmt label extraction` |
| `\| logfmt --strict`, `--keep-empty` | `a logfmt flag` |
| `\|= ip("...")`, `!= ip(...)`, `\| addr = ip(...)` | `ip` |
| `\|> "pattern"`, `!> "pattern"` | `` the pattern filter `\|>` `` |
| `\|= "a" or "b"` (several strings in one line filter) | `or between line filters` |
| a byte-size literal, `\| size > 10MB` | `a byte size` |

### Where loctel differs from Loki

Found by running 120,000 random and 5,000 hand-written (query, line, labels)
rows through Loki's own pipeline (v3.7.9) and loctel's side by side (the recipe is
in `.claude/skills/add-query-feature/loki-differential.md`). Everything else agreed.

**Different on purpose, and why:**

- **A number or duration filter that cannot read its label is false.** Loki keeps
  the line and sets `__error__="LabelFilterErr"` (so `| json | status >= 500`
  keeps a plain-text line that has no `status` to read, and an `or` with it is
  true). The plan fixes false for loctel, which is also the safe way round for a
  discard rule: a rule must not delete a line because the line is not what the
  rule expected. A missing label is false in both.
- **A label filter before `| json` or `| logfmt` does not see what the parser
  extracts.** Loki pushes every single-label filter of the pipeline into the
  parsers, so `| x != "1" | logfmt` is also tested against the `x` that `logfmt`
  extracts, and drops the line when it is `1`. That is an optimisation leaking
  into the semantics. loctel runs the stages in order, as the language documents
  it: the filter sees the labels that exist at its stage, so a pipeline written
  that way matches less here, never more.
- **An empty regex in a label filter matches only an empty label.** `| level =~ ""`
  is true for every line in Loki (a line-filter simplification that forgets the
  anchoring); here it is `^(?:)$`, as in Prometheus and in Loki's own selectors.

**Looser than Loki** (loctel accepts what Loki rejects; the answers for what both
accept do not differ):

- `{app=~".*"}`, `{app=""}`, `{app!="x"}`: Loki wants at least one matcher that is
  not "empty-compatible", because it has an index to protect. loctel has none, so
  only `{}` is an error (the plan's rule).
- Words that are keywords in Loki (`json`, `by`, `and`) can be label names in a
  selector, and `and` can be a label name after `|`.

**Narrower than Loki** (an error here, never a different answer):

- The units `d`, `w` and `y` in a duration literal (`1d`): the shared
  `parse_duration` has `ns us µs ms s m h`. A label's text is read with Go's
  `time.ParseDuration`, which has no `d` either.
- A number literal that overflows (`1e999`), and `\x` or octal escapes above
  `\x7f` (Go makes them raw bytes, and loctel's strings are UTF-8).
- `__error_details__` is not set; `__error__` is (`JSONParserErr`).
- `--strict` logfmt, and the features in the table above.

### Limits

The label filter parser recurses only on parentheses and refuses past **64**
levels (`query is too deeply nested`), including parentheses around the whole
query. It also refuses more than **1000** operators (`and`, `or`, `,` and
juxtaposition) in one query, because a chain of them is a tree as deep as it is
long and the evaluator, `Clone`, `PartialEq` and `Drop` walk it recursively. The
chain itself is parsed with an explicit stack, not recursion. A rules file edited
by hand must never be able to overflow the stack, so these are errors, not
crashes (the tests run the deepest allowed tree on a 1 MiB stack). Line filters
and selector matchers are lists, with no limit.

### Tests

`crates/loctel-query/tests/logql.rs` (with the log builders in
`tests/fixtures.rs`, which turns the `CheckoutFailure` scenario's 7 logs into
`LogRecord`s through meshgen and the real OTLP conversion): the plan's table of
counts over those logs, every unsupported feature (message and column) and a
table of syntax errors (each pointing at "the text the error is at", so nobody
counts columns), what parses, the syntax tree of a query, strings and escapes,
the label set (clashes, mapped names, display strings, empty values) with the
selector's direct lookup checked against it, missing labels, numeric and
duration comparisons, label filter precedence as truth tables, both parsers
including the bad input Loki's scanner and decoder are lenient about, the limits
on a small stack, every truncation of every query (no panics) and 20,000 random
strings.
