# Query languages (`loctel-query`)

TraceQL for traces and LogQL for logs, parsed and evaluated in pure Rust. The
crate depends only on `loctel-model` (and `regex`), so it builds and tests on
Linux with no UI. Design references: §6 (querying), §7 (discard rules, which
are written in these languages) and §8 (MCP `query` arguments).

Syntax and semantics follow Grafana: Tempo's TraceQL and Loki's LogQL. A query
pasted from Grafana should work unchanged. Where loctel does not support a
feature, the parser says so with the feature's name and position, and never
gives results that quietly differ from Grafana's.

Status (Milestone 2, in progress): the shared pieces and the **TraceQL lexer
and parser** are implemented. The TraceQL evaluator, LogQL and the rules that
use them follow (the M2 plan, `superpowers/plans/2026-10-21-loctel-m2-query-rules.md`,
Tasks 2 to 7). This page grows with each of them.

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
