# loctel Milestone 2 (query languages and discard rules): Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add three things:

- **TraceQL search** for traces, including the structural queries, written
  exactly as in Grafana.
- **LogQL search** for logs.
- **Discard rules,** written in those two languages and applied as data
  arrives. Rules are saved across restarts and can be previewed before
  saving.

Searches are available to agents through MCP. On the Mac, the menu shows the
rules and opens the rules file for editing. The rules screen itself comes in
M4.

**Architecture:**

- **New crate `loctel-query`** (pure, depends only on `loctel-model`):
  - lexers, parsers and evaluators for both languages
  - errors that carry the position in the query
- **New crate `loctel-rules`:**
  - the rule model and the `rules.toml` file
  - `RuleSet`, which filters spans and logs at ingest
  - the bounded memory of dropped traces and spans
  - the preview
- **`loctel-store`** gains query filters, removal and scan APIs.
- **`loctel-engine`** runs every batch through the `RuleSet` before the store,
  reloads `rules.toml` when it changes, and reports rule counts.
- **`loctel-mcp`** gains `query` arguments and the rule status.

**Tech Stack:** Rust 1.98.1, plus the `regex` crate (RE2-compatible syntax,
close to Go's `regexp`). Everything else is the M1 stack.

**Spec:**

- `docs/superpowers/specs/2026-10-21-loctel-design.md`, mainly §6 (querying)
  and §7 (discard rules).
- The M1 plan `docs/superpowers/plans/2026-10-21-loctel-m1-core.md`, for the
  existing interfaces.
- `AGENTS.md`, for commands and house rules.

## Global Constraints

Everything in the M1 plan's Global Constraints still holds: ports, MCP
rules, the quality gate, commit trailers, no macOS runner on PRs, and docs
travelling with code. M2 adds:

- **Syntax and semantics follow Grafana:** Tempo's TraceQL and Loki's LogQL,
  so queries copied from Grafana work unchanged (§6). The authoritative
  grammars are Tempo `pkg/traceql/expr.y` and Loki
  `pkg/logql/syntax/syntax.y`. The language docs are at grafana.com
  (`/docs/tempo/latest/traceql/` and `/docs/loki/latest/query/`).
- **Regex anchoring:**
  - TraceQL `=~` and `!~` are **fully anchored**: Tempo treats
    `span.foo =~ "bar"` as `^bar$`.
  - LogQL label matchers (`=~`, `!~` in selectors and label filters) are
    **fully anchored**, as in Prometheus.
  - LogQL line filters (`|~`, `!~` on the line) are **unanchored**.
- **Unsupported syntax gives an error,** never a silent mismatch. The error
  names the feature and its position, e.g.
  `at column 23: line_format is not supported yet`.
- **Label mapping** (§6.2):
  - attribute keys become labels with every character outside
    `[A-Za-z0-9_]` turned into `_` (the existing `loki_label_name`)
  - severity becomes `level`, with values `trace`, `debug`, `info`, `warn`,
    `error`, `fatal` and `unknown`
  - the scope name becomes `scope_name`
- **Discard rules** (§7):
  - Dropped data is gone for good.
  - Rules are evaluated in file order, and only enabled, valid ones apply.
  - Turning a rule on does not purge what is already stored.
  - Rules persist in `rules.toml`, next to `settings.toml`.
  - MCP cannot change rules.
- **Constants:**

  | name | value |
  |---|---|
  | `DROPPED_TTL` | 10 minutes |
  | `DROPPED_MAX` | 100,000 remembered IDs (oldest forgotten first) |
  | `RULES_POLL` | 2 seconds |
  | `PREVIEW_WINDOW` | 5 minutes |
  | `MATCHED_SPANS_MAX` | 20 |
  | `ANSWER_TEXT_BUDGET` | 24,000 characters (about 6k tokens, under the §8 target of about 8k) |
- **Default rule** (my call, recorded in `docs/decisions/`): a missing
  `rules.toml` is created with one enabled rule. The user said they want
  health-check noise gone. The rule:
  - `name = "Health checks"`, `language = "traceql"`, `scope = "trace"`
  - `query = '{ span.url.path =~ "/(health|healthz|alive|livez|ready|readyz)(/.*)?" || span.http.route =~ "/(health|healthz|alive|livez|ready|readyz)(/.*)?" }'`

## Review Focus

1. **A rule that drops a whole trace whose root arrives last** (health-check
   spans from several services arrive out of order). Expect every span and
   log of that trace gone, including the ones stored before the match and
   the ones arriving after it. *Test:* Task 5
   `whole_trace_drop_covers_stored_and_late_spans`.
2. **A hand-edited `rules.toml` with a typo** (bad TOML, or a bad query in
   one rule). Expect the bad TOML to keep the previous rules and report the
   error, and a bad query to disable only that rule, with its error. The app
   never crashes and other rules keep applying. *Tests:* Task 6
   `broken_rules_file_keeps_previous_rules` and Task 5
   `invalid_query_disables_only_that_rule`.
3. **Queries pasted from Grafana with features loctel lacks** (`select()`,
   `rate()`, `line_format`). Expect a clear "not supported yet" error with
   its position, never wrong results. *Tests:* Task 1
   `unsupported_features_are_errors_with_position` and Task 3 (same name).
4. **Type mismatches and missing attributes**, e.g.
   `{ span.http.response.status_code = "500" }` against an int attribute, or
   `!=` on an attribute the span lacks. Expect Tempo's behaviour: no match
   and no error. Ints and floats compare numerically. *Test:* Task 2
   `comparisons_follow_tempo_typing`.
5. **A regex that could match a substring.** `{ name =~ "POST" }` must not
   match `POST /charges` (anchored), while LogQL `|~ "attempt"` matches
   inside a line (unanchored). *Tests:* Task 2 `traceql_regex_is_anchored`
   and Task 3 `line_filter_regex_is_unanchored`.

---

## File map (M2)

```
crates/loctel-query/            NEW: QueryError, Pattern, duration literals; traceql/, logql/
  src/lib.rs, src/error.rs, src/pattern.rs, src/duration.rs
  src/traceql/{mod.rs, lexer.rs, ast.rs, parser.rs, eval.rs, view.rs}
  src/logql/{mod.rs, lexer.rs, ast.rs, parser.rs, eval.rs, labels.rs}
  tests/{traceql_parse.rs, traceql_eval.rs, logql.rs, fixtures.rs}
crates/loctel-store/            query filters, matched spans, remove_*, with_trace_spans, scan_*
crates/loctel-rules/            NEW: RuleSpec, rules.toml, RuleSet, dropped memory, preview
crates/loctel-engine/           sink -> RuleSet -> Store; rules path, reload, handle API, status
crates/loctel-mcp/              query args, rules in get_status, answer size budget
crates/loctel-ui/               menu_text::rules_line; StatusView rules row (+ snapshot)
crates/loctel-macos/            menu rows: rules line, "Edit discard rules…"
crates/loctel-app/              --rules PATH
crates/loctel-e2e/tests/rules.rs, tests/query.rs
docs/areas/query.md, docs/areas/rules.md   NEW
.claude/skills/add-query-feature/SKILL.md  NEW
docs/milestones/mac-checklist.md (M1 section kept + M2 section), roadmap.md
```

---

### Task 1: `loctel-query` foundations and the TraceQL parser

**Files:**
- Create: `crates/loctel-query/Cargo.toml` (deps: `loctel-model`, `regex`;
  dev-deps: `loctel-meshgen`, `loctel-ingest`)
- Create: `crates/loctel-query/src/{lib.rs, error.rs, pattern.rs, duration.rs}`
- Create: `crates/loctel-query/src/traceql/{mod.rs, lexer.rs, ast.rs, parser.rs}`
- Test: `crates/loctel-query/tests/traceql_parse.rs`
- Create: `docs/areas/query.md` (the TraceQL section; LogQL comes in Task 3)
- Modify: `docs/architecture.md` (add the crate)

**Interfaces:**
- Produces:
  - `#[derive(Clone, Debug, PartialEq, Eq)] pub struct QueryError { pub message: String, pub position: usize }`.
    `position` is a 0-based character index into the query. `Display` is
    `"at column {position+1}: {message}"`. It also implements
    `std::error::Error`.
  - `#[derive(Clone, Debug)] pub struct Pattern`, a compiled regex:
    - `Pattern::anchored(src: &str) -> Result<Pattern, regex::Error>`
      compiles `^(?:src)$`
    - `Pattern::unanchored(src) -> Result<Pattern, regex::Error>`
    - `is_match(&self, text: &str) -> bool`
    - `source(&self) -> &str`
    - `PartialEq` compares the source and whether it is anchored
  - `pub fn parse_duration(text: &str) -> Option<u64>` (nanoseconds). Units
    are `ns`, `us`, `µs`, `ms`, `s`, `m` and `h`; decimals and compound forms
    such as `1h30m` are allowed.
  - `pub mod traceql`:
    - `pub fn parse(text: &str) -> Result<TraceQuery, QueryError>`
    - `#[derive(Clone, Debug, PartialEq)] pub struct TraceQuery`: the AST
      root, with `pub fn needs_whole_trace(&self) -> bool`. It is true when
      the query uses:
      - a structural operator
      - spanset `&&` or `||` between two filters
      - a pipeline
      - `rootName`, `rootServiceName`, `traceDuration`, or their scoped
        forms

      Otherwise it is false: one filter, judged span by span.
    - The AST types are `pub` (for M3/M4 tooling) but carry no promises
      beyond this plan.
- **The grammar to support** (everything else is `QueryError`):
  - Query: `spanset_expr ( "|" pipeline_stage )*`
  - Spanset expressions:
    - `spanset_filter`
    - `spanset_expr OP spanset_expr`, where OP is one of
      `&& || > < >> << ~ !> !< !>> !<< !~`
    - `( spanset_expr )`
    - Precedence and associativity as in `expr.y`. Check it against the
      grammar and record what you found in `docs/areas/query.md`.
  - Spanset filters:
    - `{ }` (matches every span)
    - `{ field_expr }`
  - Field expressions:
    - `field_expr (&& | ||) field_expr`
    - `!field_expr`
    - `( field_expr )`
    - `operand CMP operand`, where CMP is one of `= != > < >= <= =~ !~`
    - a bare boolean attribute, such as `{ span.cache.hit }`
  - Operands are an attribute, an intrinsic or a static value.
    - **Attributes:**
      - `span.<name>`, `resource.<name>`, or unscoped `.<name>`
      - a name is dotted (`span.http.response.status_code`), or quoted
        after the scope (`span."http status"`)
    - **Intrinsics,** old style: `name`, `status`, `statusMessage`, `kind`,
      `duration`, `rootName`, `rootServiceName`, `traceDuration`.
      Scoped style: `span:name`, `span:status`, `span:statusMessage`,
      `span:kind`, `span:duration`, `trace:rootName`, `trace:rootService`,
      `trace:duration`.
    - **Static values:**
      - strings in `"..."` (with `\"` and `\\` escapes) or backticks
      - integers, floats, durations, `true`, `false`, `nil`
      - status values `ok`, `error`, `unset`
      - kind values `unspecified`, `internal`, `server`, `client`,
        `producer`, `consumer`
  - Pipeline stage: `count() CMP number`.
  - Recognised but unsupported, giving the error `"<feature> is not
    supported yet"` at the feature's position:
    - `select(`, `by(`, `avg(`, `min(`, `max(`, `sum(`
    - `rate(`, `count_over_time(`, `quantile_over_time(`, `histogram_over_time(`
    - `coalesce(`
    - `with(`
    - `event.` and `link.` scopes, and `instrumentation.`

- [ ] **Step 1: Write the failing tests** in `tests/traceql_parse.rs`.
  - `must_pass_queries_parse`: every one of these parses.

    ```text
    { }
    { span.http.request.method = "GET" }
    { resource.service.name = "payments-api" && status = error }
    { .http.response.status_code >= 500 }
    { duration > 500ms }
    { name =~ "POST.*" }
    { kind = server } >> { status = error }
    { resource.service.name = "web-bff" } >> { name = "POST /charges" }
    { resource.service.name = "orders-api" } > { resource.service.name = "payments-api" }
    { rootServiceName = "web-bff" && traceDuration > 1s }
    { status = error } | count() > 3
    { name = "SELECT carts" } ~ { kind = client }
    { span:name = "POST /charges" }
    { resource.service.name = "web-bff" } !>> { status = error }
    { span."http.route" = "/api/checkout" }
    ({ kind = server } && { kind = client }) || { status = error }
    { span.order.id != nil }
    { duration > 1h30m }
    ```

  - `parse_errors_have_positions`: `{ name = }` fails at column 10;
    `{ name = "x"` gives "expected `}`" at its end; `{ nme = "x" }` gives
    "unknown intrinsic `nme`" at column 3.
  - `unsupported_features_are_errors_with_position`:
    `{ status = error } | select(span.foo)` gives "select is not supported
    yet" at column 22, and `{ } | rate()` gives "rate is not supported yet".
  - `needs_whole_trace_classification`: `{ name = "x" }` is false;
    `{ a } >> { b }` (with real filters), `{ rootName = "x" }` and
    `{ } | count() > 1` are true.
  - `duration_literals`: `parse_duration` gives `"500ms"` → 500_000_000,
    `"1.5s"` → 1_500_000_000 and `"1h30m"` → 5_400_000_000_000, and
    `"5x"` → None.
- [ ] **Step 2: Watch them fail.** Run `cargo test -p loctel-query`.
  Expected: compile failure.
- [ ] **Step 3: Implement.** Write a hand-written lexer (with a character
  position on every token) and a recursive-descent parser. Use precedence
  climbing for both the spanset and field levels.
- [ ] **Step 4: Watch them pass.** Run `cargo test -p loctel-query`.
  Expected: PASS.
- [ ] **Step 5: Docs.** Write the TraceQL section of `docs/areas/query.md`:
  the supported subset, the unsupported list, anchoring and precedence. Add
  the crate to `docs/architecture.md`.
- [ ] **Step 6: Commit.** Message: `feat(query): TraceQL lexer and parser`.

### Task 2: The TraceQL evaluator

**Files:**
- Create: `crates/loctel-query/src/traceql/{view.rs, eval.rs}`
- Test: `crates/loctel-query/tests/{fixtures.rs, traceql_eval.rs}`

**Interfaces:**
- Consumes: `Span`, `AttrValue`, `SpanKind` and `StatusCode` from
  `loctel-model`, and Task 1's AST.
- Produces:
  - `pub struct TraceView<'a>` with:
    - `new(spans: &'a [Span]) -> TraceView<'a>`: indexes parents once and
      numbers the tree as a nested set, so ancestry is O(1)
    - `spans(&self) -> &'a [Span]`
    - `parent(&self, i: usize) -> Option<usize>`
    - `is_ancestor(&self, a: usize, d: usize) -> bool`: true when `a` is a
      strict ancestor of `d`
    - `root(&self) -> Option<usize>`: the same root rule as the store, the
      earliest-starting span with no parent

    Spans whose parent is missing are treated as roots of their own
    subtrees.
  - `TraceQuery::evaluate(&self, trace: &TraceView) -> Vec<usize>`: the
    indices of the matched spans, ascending, with no duplicates. A trace
    matches when the result is non-empty.
  - `TraceQuery::matches_span(&self, span: &Span) -> Option<bool>`. It
    returns `Some` only when `!needs_whole_trace()`, and agrees with
    `evaluate` on a one-span view.
- Semantics:
  - **Spanset filter:** the spans for which the field expression is true.
  - **Unscoped `.x`:** the span attribute if present, else the resource
    attribute.
  - **Missing attributes:** any comparison is false, including `!=` (as in
    Tempo). `= nil` is true when the attribute is missing; `!= nil` is true
    when it is present.
  - **Typing:**
    - ints and floats compare numerically
    - strings compare lexically for `<` and `>`
    - durations compare against duration attributes and against
      `duration`, `traceDuration` and the scoped forms
    - booleans support only `=` and `!=`
    - any other type pair is false
  - **`status` and `kind`** compare to their keywords. `statusMessage`
    compares as a string.
  - **Root intrinsics:** `rootName`, `rootServiceName` and `traceDuration`
    hold for every span of a trace whose root qualifies.
    `traceDuration` is the trace's earliest start to its latest end,
    clamped at 0.
  - **Structural operators,** for `A op B`, where the result is spans of B:
    - `>`: B whose parent is in A
    - `<`: B that is the parent of some span in A
    - `>>`: B with an ancestor in A
    - `<<`: B that is an ancestor of some span in A
    - `~`: B with a sibling in A (same parent, not itself)
    - the `!` forms return the B spans without that relationship
  - **`A && B`** (spansets): the union of A and B when both are non-empty,
    else empty. **`A || B`:** the union.
  - **`| count() OP n`:** keeps the whole current result when its size
    satisfies the comparison, else gives empty.

- [ ] **Step 1: Write the fixtures and failing tests.**
  - `tests/fixtures.rs` provides `checkout_failure() -> Vec<Span>`. It
    builds the spans through `loctel_meshgen::generate(&Scenario::CheckoutFailure, 1, 1_000_000_000)`
    and `loctel_ingest::convert::spans_from_request`.
  - It also provides
    `matched(q: &str, spans: &[Span]) -> BTreeSet<(String, String)>`,
    which returns the (service, name) pair of each matched span.
  - `tests/traceql_eval.rs` covers the CheckoutFailure trace from the M1
    plan's Task 8 table. Its 15 spans are numbered there.

```rust
#[test] fn user_example_name_with_ancestor_from_service() {
    assert_eq!(matched(r#"{ resource.service.name = "web-bff" } >> { name = "POST /charges" }"#, &cf()),
               set(&[("payments-api", "POST /charges")]));
}
#[test] fn child_operator() {
    assert_eq!(matched(r#"{ resource.service.name = "orders-api" } > { resource.service.name = "payments-api" }"#, &cf()),
               set(&[("payments-api", "POST /charges")]));
}
#[test] fn sibling_operator() {
    assert_eq!(matched(r#"{ name = "SELECT carts" } ~ { kind = client }"#, &cf()),
               set(&[("orders-api", "GET"), ("orders-api", "POST")]));
}
#[test] fn negated_descendant() {
    assert_eq!(matched(r#"{ resource.service.name = "web-bff" } !>> { status = error }"#, &cf()),
               set(&[("web-bff", "POST /api/checkout")]));
}
#[test] fn traceql_regex_is_anchored() {
    assert!(matched(r#"{ name =~ "POST" }"#, &cf()).iter().all(|(_, n)| n == "POST"));
    assert!(matched(r#"{ name =~ "POST.*" }"#, &cf()).contains(&("payments-api".into(), "POST /charges".into())));
}
#[test] fn comparisons_follow_tempo_typing() {
    assert!(matched(r#"{ span.http.response.status_code = "500" }"#, &cf()).is_empty()); // int attr vs string
    assert_eq!(matched(r#"{ span.http.response.status_code = 500.0 }"#, &cf()).len(), 2); // spans 1 and 4
    assert!(matched(r#"{ span.no.such.attr != "x" }"#, &cf()).is_empty());
}
```

  The remaining tests:
  - `status_and_service_filter`:
    `{ resource.service.name = "payments-api" && status = error }` gives
    exactly #10, #12 and #13.

  `http.response.status_code` must be an int attribute in meshgen, as the
  semantic conventions say. If it isn't, fix meshgen in this task and say
  so in the commit.
  - `duration_filter`: `{ duration > 1s }` gives #1, #3, #4, #9 and #10.
  - `root_intrinsics`: `{ rootServiceName = "web-bff" && traceDuration > 1s }`
    gives all 15 spans.
  - `count_pipeline`: `{ status = error } | count() > 3` gives the 7
    erroring spans, and `| count() > 10` gives none.
  - `spanset_and_or`
  - `empty_filter_matches_all`
  - `matches_span_agrees_with_evaluate`: for each non-structural query
    above, `matches_span` over each span equals `evaluate`.
  - `orphans_are_subtree_roots`
- [ ] **Step 2: Watch them fail.** Run `cargo test -p loctel-query`.
  Expected: FAIL, with `evaluate` missing.
- [ ] **Step 3: Implement.** Evaluate bottom-up over bitsets (`Vec<bool>`
  or `fixedbitset`) of span indices. Structural operators use the
  nested-set numbering.
- [ ] **Step 4: Watch them pass.** Run `cargo test -p loctel-query`.
  Expected: PASS.
- [ ] **Step 5: Docs.** Add a semantics table to `docs/areas/query.md`.
- [ ] **Step 6: Commit.** Message: `feat(query): TraceQL evaluator with
  structural operators`.

### Task 3: LogQL (lexer, parser, labels, evaluator)

**Files:**
- Create: `crates/loctel-query/src/logql/{mod.rs, lexer.rs, ast.rs, parser.rs, eval.rs, labels.rs}`
- Test: `crates/loctel-query/tests/logql.rs`
- Modify: `docs/areas/query.md` (add the LogQL section)

**Interfaces:**
- Produces:
  - `pub mod logql`:
    - `pub fn parse(text: &str) -> Result<LogQuery, QueryError>`
    - `#[derive(Clone, Debug, PartialEq)] pub struct LogQuery`, with:
      - `matches(&self, log: &LogRecord) -> bool`
      - `service_hint(&self) -> Option<&str>`: the value of an exact
        `service_name="..."` selector matcher, if there is one (a
        prefilter for the store)
  - `pub fn labels(log: &LogRecord) -> Vec<(String, String)>`: the log's
    label set, sorted by name. It holds:
    - every resource attribute (mapped name, display string)
    - every log attribute not already present
    - `service_name`
    - `level` (`Severity::level_label`)
    - `scope_name`
    - `trace_id` and `span_id` when the log has them
- **The grammar to support:**
  - Query: `selector pipeline_stage*`
  - Selector: `{ matcher ("," matcher)* }`, where a matcher is
    `label ( = | != | =~ | !~ ) string`. An empty selector `{}` is an error
    ("at least one matcher"), as in Loki.
  - Pipeline stages:
    - a line filter, `( |= | != | |~ | !~ ) string`, matched against the
      body
    - `| json` and `| logfmt`, with no parameters
    - a label filter, `| label_expr`
  - Label expressions:
    - `label_expr (and | or | ",") label_expr` (a comma means `and`)
    - `( label_expr )`
    - `label ( = | != | =~ | !~ ) string`
    - `label ( == | = | != | > | >= | < | <= ) number_or_duration`
  - Strings are `"..."` or backticks.
  - Parsers:
    - `| json` adds the body's JSON fields as labels, flattening nesting
      with `_` (`{"user":{"id":42}}` gives `user_id="42"`). Arrays are
      skipped. A body that isn't JSON adds `__error__="JSONParserErr"` and
      drops nothing.
    - `| logfmt` adds `key=value` pairs, with quoted values supported.
  - A numeric label filter parses the label value as a number or duration.
    If it can't, the filter is false.
  - Errors:
    - `line_format`, `label_format`, `unwrap`, `decolorize`, `drop`, `keep`,
      `pattern` and `regexp` (as parsers), `unpack` and `ip(` give "not
      supported yet" at their position.
    - A query starting with a metric function (`rate(`,
      `count_over_time(`, `sum(`, and so on) gives "metrics queries are not
      supported yet".

- [ ] **Step 1: Write the failing tests** in `tests/logql.rs`. They use the
  CheckoutFailure logs (7 records; see the M1 plan, Task 8), converted
  through `loctel_ingest::convert::logs_from_request`, plus hand-built logs
  for the parsers. Each line is a test case with its expected count:

  ```text
  {service_name="payments-api"}                                  -> 4
  {service_name=~"orders-api|payments-api"} |= "8812"            -> 5
  {service_name="payments-api"} | level = "error"                -> 2
  {service_name="payments-api"} |~ "attempt [0-9]"               -> 1
  {service_name="payments-api"} != "card-gateway"                -> 1
  {service_name="orders-api", deployment_environment="local"}    -> 3
  {level="error"}                                                -> 3
  {service_name=~"orders"}                                       -> 0   (anchored)
  ```

  Then the named tests:
  - `json_parser_and_numeric_filter`: body
    `{"user":{"id":42},"msg":"x"}` with `| json | user_id = 42` gives a
    match, and with `| json | user_id > 50` doesn't.
  - `logfmt_parser_and_duration_filter`: body
    `method=GET status=500 took=12ms` matches both `| logfmt | status >= 500`
    and `| logfmt | took > 10ms`.
  - `line_filter_regex_is_unanchored`
  - `label_matcher_regex_is_anchored`
  - `unsupported_features_are_errors_with_position`:
    `{service_name="x"} | line_format "{{.a}}"` gives "line_format is not
    supported yet" at column 22. `rate({service_name="x"}[5m])` gives
    "metrics queries are not supported yet" at column 1.
  - `empty_selector_is_error`
  - `labels_include_resource_log_attrs_level_scope`
  - `service_hint_exact_only`
- [ ] **Step 2: Watch them fail.** Run `cargo test -p loctel-query`.
  Expected: compile failure.
- [ ] **Step 3: Implement.** The label set is computed lazily, once per log
  per `matches` call. Parsers extend a local copy.
- [ ] **Step 4: Watch them pass.** Run `cargo test -p loctel-query`.
  Expected: PASS.
- [ ] **Step 5: Docs.** Write the LogQL section of `docs/areas/query.md`:
  the subset, the label set, the parsers, anchoring and the unsupported
  list. Create `.claude/skills/add-query-feature/SKILL.md`, covering how to
  add syntax to either language:
  - lexer token, AST node, parser rule, evaluator
  - a parse test with a position, plus an evaluation test on the fixtures
  - remove the feature from the unsupported list
  - update the docs table
- [ ] **Step 6: Commit.** Message: `feat(query): LogQL parser, labels and
  evaluator`.

### Task 4: Store: query filters, matched spans, removal and scans

**Files:**
- Modify: `crates/loctel-store/Cargo.toml` (dep `loctel-query`)
- Modify: `crates/loctel-store/src/{summary.rs, logs.rs, store.rs, trace.rs}`
- Test: `crates/loctel-store/tests/{query_filters.rs, removal.rs}`
- Modify: `docs/areas/store.md`

**Interfaces:**
- Consumes: `loctel_query::{traceql::{TraceQuery, TraceView}, logql::LogQuery}`.
- Produces:
  - `TraceFilter` gains `pub query: Option<Arc<TraceQuery>>`. Drop the
    `Eq` derive if needed; keep `Default`, `Clone`, `Debug` and `PartialEq`.
  - A trace matches when every existing condition holds **and** the query
    (if any) matches the trace's spans, through
    `TraceView::new(spans)` and `evaluate`.
  - `TraceSummary` gains `pub matched_span_ids: Vec<SpanId>`: the first
    `MATCHED_SPANS_MAX` (20) matched spans, in `evaluate` order. It is
    empty when there is no query.
  - `LogFilter` gains `pub query: Option<Arc<LogQuery>>`.
    `query.service_hint()` is used to skip other services cheaply.
  - `Store::remove_trace(&self, id: TraceId) -> Removed`, where
    `#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)] pub struct Removed { pub spans: usize, pub logs: usize }`.
    It removes the trace and every held log carrying that trace ID, in
    O(that trace's spans + logs). Add a trace-to-logs index if needed.
  - `Store::remove_spans(&self, trace: TraceId, ids: &[SpanId]) -> Removed`:
    removes those spans and their logs. Removing the last span removes the
    trace.
  - `Store::with_trace_spans<R>(&self, id: TraceId, f: impl FnOnce(&[Span]) -> R) -> Option<R>`:
    runs `f` on the held spans under the read lock, without copying.
  - `Store::scan_traces(&self, since_unix_nano: u64, f: impl FnMut(TraceId, &[Span], TraceStatus))`:
    every held trace starting at or after `since`.
  - `Store::scan_logs(&self, since_unix_nano: u64, f: impl FnMut(&LogRecord))`.
  - Stats stay consistent: `spans` and `logs` held go down.
    `*_received` doesn't change.

- [ ] **Step 1: Write the failing tests.**
  - `tests/query_filters.rs`:
    - `traceql_query_filters_traces_and_reports_matched_spans`: load
      CheckoutFailure plus CheckoutSuccess, then query
      `{ status = error }`. That gives 1 trace whose `matched_span_ids`
      has 7 entries.
    - `query_combines_with_other_conditions`
    - `logql_query_filters_logs`: `{service_name="payments-api"} | level = "error"`
      gives 2.
    - `matched_span_ids_capped_at_20`: a HugeTrace with `{ }` gives 20.
  - `tests/removal.rs`:
    - `remove_trace_removes_spans_and_logs`: CheckoutFailure gives
      `Removed { spans: 15, logs: 7 }`, `trace()` becomes `None`, and
      `search_logs` returns nothing for that trace.
    - `remove_spans_removes_their_logs`
    - `removing_last_span_removes_trace`
    - `with_trace_spans_sees_held_spans`
    - `scan_traces_respects_since`
    - `scan_logs_respects_since`
- [ ] **Step 2: Watch them fail.** Run `cargo test -p loctel-store`.
  Expected: compile failure.
- [ ] **Step 3: Implement.** Every existing store test must still pass
  unchanged.
- [ ] **Step 4: Watch them pass.** Run `cargo test -p loctel-store`.
  Expected: PASS.
- [ ] **Step 5: Docs.** Update the API section of `docs/areas/store.md`.
- [ ] **Step 6: Commit.** Message: `feat(store): query filters, matched
  spans, removal and scans`.

### Task 5: `loctel-rules`: model, file, ingest filter, preview

**Files:**
- Create: `crates/loctel-rules/Cargo.toml` (deps: `loctel-model`,
  `loctel-query`, `loctel-store`, `serde`, `toml`, `dirs`, `anyhow`,
  `parking_lot`)
- Create: `crates/loctel-rules/src/{lib.rs, spec.rs, file.rs, set.rs, memory.rs, preview.rs}`
- Test: `crates/loctel-rules/tests/{file.rs, filter.rs, preview.rs}`
- Create: `docs/areas/rules.md`, plus
  `docs/decisions/00NN-default-health-check-rule.md` (the next free number)

**Interfaces:**
- Produces:
  - `#[serde(rename_all = "lowercase")] pub enum RuleLanguage { TraceQl, LogQl }`,
    serialized as `"traceql"` and `"logql"`.
  - `#[serde(rename_all = "lowercase")] pub enum DropScope { Trace, Span }`,
    with `Default = Trace`.
  - `#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)] pub struct RuleSpec { pub name: String, pub enabled: bool, pub language: RuleLanguage, pub query: String, #[serde(default)] pub scope: DropScope }`.
    `scope` is ignored for LogQL.
  - `pub struct RulesFile;` with:
    - `default_path() -> PathBuf`: the directory of `Settings::default_path()`
      plus `rules.toml`. Compute it with `dirs::config_dir()/loctel`, the
      same rule as settings.
    - `load(path: &Path) -> anyhow::Result<Option<Vec<RuleSpec>>>`:
      `Ok(None)` if the file is missing.
    - `save(path: &Path, rules: &[RuleSpec]) -> anyhow::Result<()>`: an
      atomic write, like `Settings::save`.
    - `default_rules() -> Vec<RuleSpec>`: exactly the one rule from Global
      Constraints.

    The file format is TOML: an array of tables `[[rule]]` holding
    `name`, `enabled`, `language`, `query` and `scope`. A comment header at
    the top of the written file explains the format and links
    `docs/areas/rules.md`.
  - `#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)] pub struct RuleId(pub u64)`:
    a position-independent ID, assigned when a rule first appears and kept
    for an identical `(name, language, query, scope)` across `replace`.
  - `#[derive(Clone, Debug, PartialEq)] pub struct RuleStatus { pub id: RuleId, pub spec: RuleSpec, pub error: Option<String>, pub dropped_spans: u64, pub dropped_logs: u64, pub dropped_traces: u64 }`.
    `error` is the `QueryError` text when the query doesn't parse.
  - `pub struct RuleSet` (`Send + Sync`; interior mutability):
    - `new(specs: Vec<RuleSpec>, clock: Arc<dyn Clock>) -> RuleSet`
    - `replace(&self, specs: Vec<RuleSpec>)`: keeps the counters of rules
      whose IDs survive. Remembered dropped IDs are kept too.
    - `statuses(&self) -> Vec<RuleStatus>`, in file order
    - `filter_spans(&self, spans: Vec<Span>, store: &Store) -> SpanVerdict`,
      where
      `pub struct SpanVerdict { pub keep: Vec<Span>, pub drop_traces: Vec<TraceId>, pub drop_spans: Vec<(TraceId, SpanId)> }`
      (the last two list stored data the caller must remove)
    - `filter_logs(&self, logs: Vec<LogRecord>) -> Vec<LogRecord>`
    - `totals(&self) -> DroppedTotals { pub spans: u64, pub logs: u64 }`
  - `pub fn preview(spec: &RuleSpec, store: &Store, since_unix_nano: u64) -> Result<Preview, QueryError>`,
    where
    `#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)] pub struct Preview { pub traces: u64, pub spans: u64, pub logs: u64, pub error_traces: u64, pub error_logs: u64 }`.
    It reports what the rule would have dropped of the stored data since
    then, ignoring other rules. `error_traces` counts matched traces whose
    status isn't `Ok`. `error_logs` counts matched logs at `error` or above.
- **`filter_spans` semantics** (§7). For each incoming span, in order:
  1. **Remembered trace:** if its trace ID is in the dropped-trace memory,
     drop it and count it on the rule that dropped the trace.
  2. **Remembered parent:** if its `(trace, parent)` is in the dropped-span
     memory, drop it and remember its own ID too (descendants arriving
     late).
  3. **TraceQL rules, in file order** (enabled and valid only):
     - **Single-span queries** (`matches_span` is `Some`): judge the span
       itself.
     - **Whole-trace queries:** group the batch by trace and evaluate once
       per trace, over the stored spans (from `with_trace_spans`) followed
       by this batch's spans of that trace.
     - **A match with scope Trace:**
       - push the trace ID to `drop_traces` (if not already remembered)
       - remember it for `DROPPED_TTL`, bounded by `DROPPED_MAX`
       - drop every incoming span of that trace
       - increment `dropped_traces` once, and `dropped_spans` by the
         incoming spans dropped plus the stored spans the caller removes.
         The caller reports the stored count back through
         `RuleSet::record_removed(rule, Removed)`; add that method.
     - **A match with scope Span:**
       - drop the matched spans and their descendants, both incoming and
         stored
       - push the stored ones to `drop_spans`
       - remember all of their IDs
  4. Otherwise keep it.

  A span is counted once, on the first rule that dropped it.
- **`filter_logs`:** drop a log when any of these holds, counting it on the
  responsible rule:
  - its trace ID is in the dropped-trace memory
  - its `(trace, span)` is in the dropped-span memory
  - an enabled, valid LogQL rule matches it (first in file order)
- **Memory:** entries expire after `DROPPED_TTL`, judged by the clock. Past
  `DROPPED_MAX`, the oldest entries are forgotten first.

- [ ] **Step 1: Write the failing tests.**
  - `tests/file.rs`:
    - `missing_file_is_none`
    - `roundtrip`
    - `scope_defaults_to_trace`
    - `default_rules_contents`: the name, the exact query and `enabled`
    - `default_health_rule_matches_health_paths_only`: it parses. Against
      `HealthChecks` traces it matches every span. Against CheckoutFailure
      it matches nothing.
  - `tests/filter.rs`, using `ManualClock` and a `Store`:
    - `per_span_rule_drops_matching_trace`
    - `whole_trace_drop_covers_stored_and_late_spans`. Steps and expected
      results:
      1. ingest child spans of a health-check trace that don't themselves
         match (kept and stored)
      2. ingest the matching root, which drops it, and the verdict's
         `drop_traces` contains the trace
      3. the caller removes the stored spans
      4. a later child is dropped
      5. a log with that trace ID is dropped
      6. counts: `dropped_traces == 1`, and `dropped_spans` equals every
         span of the trace
    - `structural_rule_uses_stored_spans`: the rule is
      `{ kind = server } >> { name = "MGET" }`, with the parent stored
      earlier and the child in this batch.
    - `span_scope_drops_descendants_including_late_ones`
    - `logql_rule_drops_matching_logs_only`
    - `disabled_rule_does_nothing`
    - `invalid_query_disables_only_that_rule`: its status has
      `error: Some(..)`, and other rules still drop.
    - `first_rule_in_file_order_gets_the_count`
    - `memory_expires_after_ttl`: after 11 minutes, a span of a once-dropped
      trace that doesn't itself match is kept.
    - `replace_keeps_counters_for_unchanged_rules`
  - `tests/preview.rs`:
    - `preview_counts_traces_spans_logs_and_errors`: with stored
      CheckoutFailure plus HealthChecks{2}, the health rule gives `traces`
      equal to the health traces, `error_traces == 0`, and logs equal to
      the health logs. The rule `{ status = error }` gives
      `error_traces == 1`.
    - `preview_respects_since`
    - `preview_invalid_query_is_error`
- [ ] **Step 2: Watch them fail.** Run `cargo test -p loctel-rules`.
  Expected: compile failure.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Watch them pass.** Run `cargo test -p loctel-rules`.
  Expected: PASS.
- [ ] **Step 5: Docs.**
  - `docs/areas/rules.md`:
    - the file format with an example
    - the semantics above (with whole-trace and span scope)
    - memory and its limits
    - the counting rules
    - preview
  - The decision file for the default rule.
  - Add the crate to `docs/architecture.md`.
- [ ] **Step 6: Commit.** Message: `feat(rules): discard rules model, file,
  ingest filter and preview`.

### Task 6: Engine: rules at ingest, reload, handle API, status, CLI

**Files:**
- Modify: `crates/loctel-engine/Cargo.toml` (dep `loctel-rules`)
- Modify: `crates/loctel-engine/src/{lib.rs, sink.rs}`
- Create: `crates/loctel-engine/src/rules_watch.rs`
- Modify: `crates/loctel-app/src/{cli.rs, main.rs}` (add `--rules PATH`,
  defaulting to `RulesFile::default_path()`)
- Test: `crates/loctel-engine/tests/rules.rs`, plus an extended
  `crates/loctel-app/tests/headless.rs`

**Interfaces:**
- Produces:
  - `EngineConfig` gains `pub rules_path: Option<PathBuf>`. `None` means
    rules live in memory only and start empty; `Default` is `None` for
    tests. The app passes `Some(path)`.
  - **Start:**
    - with `Some(path)` and no file, write `RulesFile::default_rules()` to
      it and use them
    - with an existing file, load it
    - with a file that can't be read or parsed, start with no rules and set
      `rules_file_error`
  - **Reload:** a background task polls the file's modification time every
    `RULES_POLL` (2 s).
    - On a change, load and `RuleSet::replace`.
    - On a parse failure, keep the current rules and set
      `rules_file_error: Some(text)`.
    - On success, clear the error.
    - Writes made by the engine itself update the remembered time, so they
      don't trigger a reload.
  - **The sink,** under a single `parking_lot::Mutex` so batches apply in
    order:
    - for spans: paused check, then `filter_spans`, then `remove_trace` or
      `remove_spans` for the verdict's lists (feeding
      `RuleSet::record_removed`), then `ingest_spans(keep)`
    - for logs: `filter_logs`, then `ingest_logs`
    - rates count what was **accepted**, after the rules
  - `EngineStatus` gains these fields:
    - `pub rules: Vec<RuleStatus>`
    - `pub rules_file_error: Option<String>`
    - `pub dropped_spans: u64` and `pub dropped_logs: u64`, the totals
      since launch
    - `pub dropped_fraction: f64`: dropped divided by (dropped plus
      accepted), over spans and logs together, since launch; 0 when nothing
      has arrived
  - `EngineHandle` gains:
    - `rules(&self) -> Vec<RuleStatus>`
    - `rules_path(&self) -> Option<PathBuf>`
    - `set_rules(&self, specs: Vec<RuleSpec>) -> anyhow::Result<()>`:
      replaces the rules and saves the file when a path is set. Invalid
      queries are allowed and reported through the status, so an editor can
      save drafts.
    - `set_rule_enabled(&self, id: RuleId, enabled: bool) -> anyhow::Result<()>`:
      saves too
    - `preview_rule(&self, spec: &RuleSpec) -> Result<Preview, QueryError>`:
      over the last `PREVIEW_WINDOW`
  - CLI: `--rules PATH`. Headless mode prints one more JSON line at start:
    `{"event":"rules","path":"…","count":N,"error":null}`.

- [ ] **Step 1: Write the failing tests** in `tests/rules.rs`, each with a
  temp dir, ports 0 and `Some(path)`:
  - `first_start_writes_default_rules`
  - `health_checks_are_dropped_end_to_end_in_process`: post HealthChecks
    and CheckoutFailure over HTTP JSON. The store holds only the checkout,
    and `status().rules[0].dropped_traces` equals the health traces.
  - `file_edit_is_picked_up`: set `enabled = false` in the file, wait up to
    5 s, and health checks are stored again.
  - `broken_rules_file_keeps_previous_rules`: write invalid TOML. The rules
    stay as before, `rules_file_error` is `Some`, and the error clears once
    the file is fixed.
  - `set_rules_saves_and_applies`
  - `set_rule_enabled_saves`
  - `preview_rule_over_last_five_minutes`
  - `dropped_fraction_is_reported`
  - `paused_drops_before_rules`: rule counters don't move while paused.

  The headless test gains `headless_prints_rules_line`.
- [ ] **Step 2: Watch them fail.** Run
  `cargo test -p loctel-engine -p loctel-app`. Expected: compile failure.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Watch them pass.** Run the full workspace gate. Expected:
  PASS. M1 e2e tests use `rules_path: None`, so they're unchanged.
- [ ] **Step 5: Docs.** Add rules to `docs/architecture.md` (data flow:
  receivers, then sink, then rules, then store), and the `--rules` flag to
  `AGENTS.md`'s command table.
- [ ] **Step 6: Commit.** Message: `feat(engine): discard rules at ingest
  with live reload`.

### Task 7: MCP: query arguments, rules in status, answer size budget

**Files:**
- Modify: `crates/loctel-mcp/src/{tools.rs, dto.rs}`, plus the engine's
  `StatusSource` implementation
- Test: `crates/loctel-mcp/tests/{tools.rs, budget.rs}`
- Modify: `docs/areas/mcp.md`, plus a new decision file if the default
  `get_trace` limit changes

**Interfaces:**
- Produces:
  - `SearchTracesArgs` gains `query: Option<String>` (TraceQL). It is
    parsed with `loctel_query::traceql::parse`. A parse error is a tool
    error with `isError: true` and the text `"invalid TraceQL: at column N: …"`.
  - `SearchLogsArgs` gains `query: Option<String>` (LogQL). An error gives
    `"invalid LogQL: at column N: …"`.
  - `TraceSummaryDto` gains `matched_span_ids: Vec<String>`, serialized
    only when non-empty.
  - `StatusDto` gains:
    - `rules: Vec<RuleDto { name, language, scope /* "trace"|"span", only for traceql */, enabled, error?, dropped_spans, dropped_logs, dropped_traces }>`
    - `rules_file_error: Option<String>`
    - `dropped_fraction: f64`
  - The tool descriptions say:
    - `search_traces`: TraceQL syntax as in Grafana Tempo, with one example
      structural query, and the fact that regexes are anchored.
    - `search_logs`: LogQL syntax as in Grafana Loki, with one example.
  - **Answer size budget:** the default answer of every tool, as text, is at
    most `ANSWER_TEXT_BUDGET` (24,000 characters) on the fixtures in
    `tests/budget.rs`. If `get_trace`'s summary default doesn't fit on a
    10,000-span `HugeTrace`, lower its default `limit` (keeping the maximum
    of 2000) until it does, and record the new default in `docs/areas/mcp.md`
    and a decision file.

- [ ] **Step 1: Write the failing tests.**
  - In `tests/tools.rs`:
    - `search_traces_with_traceql_structural_query`: the user's example
      gives the CheckoutFailure trace, with `matched_span_ids` holding the
      `POST /charges` span ID.
    - `search_traces_invalid_traceql_is_tool_error_with_column`
    - `search_logs_with_logql`
    - `search_logs_invalid_logql_is_tool_error`
    - `status_lists_rules_and_dropped_fraction`: uses a fake
      `StatusSource` that returns rules.
  - `tests/budget.rs`:
    - `default_answers_fit_budget`: for each tool with default arguments
      on CheckoutFailure, HealthChecks{50} and HugeTrace{10000}, the text
      length is ≤ 24000.
- [ ] **Step 2: Watch them fail.** Run `cargo test -p loctel-mcp`.
  Expected: FAIL.
- [ ] **Step 3: Implement.** Also update the tool-name and argument lists
  in `tests/http.rs` if the schema tests check them.
- [ ] **Step 4: Watch them pass.** Run `cargo test -p loctel-mcp`, then the
  full gate. Expected: PASS.
- [ ] **Step 5: Docs.** Add to `docs/areas/mcp.md`: the `query` arguments
  with examples, the status fields, and the budget rule. Follow the
  `add-mcp-tool` skill for argument changes.
- [ ] **Step 6: Commit.** Message: `feat(mcp): TraceQL and LogQL search,
  rule status, answer budget`.

### Task 8: Menu and status window: rules line, edit rules file

**Files:**
- Modify: `crates/loctel-ui/src/{menu_text.rs, status_view.rs}`
- Modify: the reference PNG under `crates/loctel-ui/tests/snapshots/`
- Modify: `crates/loctel-macos/src/{menu.rs, status_item.rs}`
- Test: `crates/loctel-ui/tests/menu_text.rs`, `crates/loctel-ui/tests/screenshots.rs`,
  and the `crates/loctel-macos` menu unit tests (`menu.rs` already has
  plain-value tests; extend them)

**Interfaces:**
- Produces:
  - `StatusModel` gains `pub rules_on: usize`, `pub rules_with_errors: usize`,
    `pub rules_file_error: bool` and `pub dropped_fraction: f64`, all filled
    by `from_status`.
  - `menu_text::rules_line(m: &StatusModel) -> String`. The exact strings,
    in priority order:
    1. `"Discard rules file has an error"` (`rules_file_error`)
    2. `"1 discard rule on · 1 has an error"` (a rule has an error)
    3. `"No discard rules on"`
    4. `"1 discard rule on · 31% dropped"` and
       `"3 discard rules on · 0% dropped"`, with the percentage rounded to
       an integer
  - The macOS menu becomes, in order:
    1. the status header
    2. a disabled rules header with `rules_line`
    3. `Copy MCP address`
    4. `Pause/Resume receiving`
    5. `Edit discard rules…`
    6. a separator
    7. `Start at login`
    8. a separator
    9. `Quit loctel`

    The new `Command::EditRules` opens `engine.rules_path()` in the default
    text editor (`NSWorkspace` `openURL` with the file URL; if no app
    handles `.toml`, fall back to `open -t <path>`). The file exists,
    because the engine writes the defaults at start. The item is disabled
    when `rules_path()` is `None`.
  - The Linux `StatusView` shows the rules line under the rates.

- [ ] **Step 1: Write the failing tests.** In `tests/menu_text.rs`:
  - `rules_line_cases`: one assertion per exact string above
  - `rules_line_file_error_wins`

  In the macOS crate's plain-value tests: `rows_include_rules_header_and_edit_item`,
  checking the order exactly. These tests are plain Rust and run in the
  Linux gate if `menu.rs` compiles off-macOS. If it doesn't, they run under
  `script/check-macos.sh`; follow what M1 decision 0009 says.
- [ ] **Step 2: Watch them fail.** Run `cargo test -p loctel-ui` and
  `bash script/check-macos.sh --all-targets`. Expected: FAIL.
- [ ] **Step 3: Implement.** Then regenerate the status-view reference with
  `cargo xtask screenshots --update`, and **open the new PNG with the Read
  tool** to check the rules line is legible (the `update-screenshots` skill
  has the steps).
- [ ] **Step 4: Watch them pass.** Run the full gate and
  `bash script/check-macos.sh --all-targets`. Expected: PASS.
- [ ] **Step 5: Docs.** Update the menu list in `docs/areas/macos.md`, and
  add "Edit discard rules…" to its "unverified until the Mac check" list.
- [ ] **Step 6: Commit.** Message: `feat(ui,macos): discard rules line and
  edit rules menu item`.

### Task 9: End-to-end tests, Mac checklist, docs pass

**Files:**
- Create: `crates/loctel-e2e/tests/{rules.rs, query.rs}`
- Modify: `crates/loctel-e2e/src/lib.rs`. Add
  `Harness::start_with_rules(completeness, rules_path)`.
- Modify: `docs/milestones/mac-checklist.md`, `docs/milestones/roadmap.md`,
  `AGENTS.md`, `docs/architecture.md`

**Interfaces:**
- Consumes everything above. Produces tests and docs only.

- [ ] **Step 1: Write the end-to-end tests.** Each runs through real OTLP
  and a real MCP client.
  - `tests/rules.rs`:
    - `default_rule_silences_health_checks_over_otlp`: send HealthChecks{5}
      over gRPC, plus CheckoutFailure over HTTP JSON.
      - `search_traces` has no `GET /health`
      - `search_logs` has no `"GET /health 200"`
      - `get_status.rules[0].dropped_traces == 5 × services`
      - `dropped_fraction > 0`
    - `late_spans_of_dropped_trace_never_appear`: send LateOutOfOrder with
      a rule matching its root by name.
    - `disabling_rule_in_file_restores_flow`
  - `tests/query.rs`:
    - `traceql_structural_query_over_mcp`
    - `logql_query_over_mcp`
    - `invalid_query_is_tool_error`
- [ ] **Step 2: Watch them pass.** Run `cargo test -p loctel-e2e`.
  Expected: PASS. Fix real bugs where they are, with a regression test in
  the owning crate.
- [ ] **Step 3: The Mac checklist.** Restructure
  `docs/milestones/mac-checklist.md` into sections, one per milestone not
  yet confirmed. Keep **"Milestone 1"** exactly as it is (the user hasn't
  done it yet), and add a **"Milestone 2"** section. Each item gives the
  steps and what "right" looks like:
  1. The menu shows `1 discard rule on · …`. **Edit discard rules…** opens
     `rules.toml` in a text editor, showing the Health checks rule.
  2. With your service (or `cargo run -p loctel-meshgen -- --scenario health-checks --transport http-json --endpoint http://127.0.0.1:4318`),
     ask Claude to list recent traces through loctel. No `/health` traces
     appear, and the rules line shows a non-zero percentage.
  3. Set `enabled = false` and save. Within about 5 seconds the line reads
     `No discard rules on`, and new health checks appear.
  4. Break a query (remove a quote) and save. The line reads
     `1 discard rule on · 1 has an error` (or the file-error text), and
     nothing crashes. Fix it again.
  5. Ask Claude to run, through loctel, a TraceQL query you use in Grafana,
     such as `{ resource.service.name = "<yours>" } >> { status = error }`.
     The results make sense.
  6. Ask Claude for logs with a LogQL query, such as
     `{service_name="<yours>"} | level = "error"`.
- [ ] **Step 4: Docs pass.**
  - `roadmap.md`: link the M2 plan from its row.
  - `AGENTS.md`: the new crates in the map pointer, the `add-query-feature`
    skill in the skills table, and `--rules` in the commands.
  - `docs/architecture.md`: the final M2 crate map and data flow.
- [ ] **Step 5: Run the full gate** and `bash script/check-macos.sh --all-targets`.
  Expected: PASS.
- [ ] **Step 6: Commit.** Message: `test(e2e): rules and query end to end;
  M2 Mac checklist and docs`.

---

## After the last task (M2 ship)

1. **Whole-branch review** against the spec (§6, §7, §8) and this plan.
   Fix what it finds.
2. **Open the PR** from `claude/m2-query-rules` to `main`. Get the Linux CI
   green.
3. **Squash-merge.** The title is
   `feat: milestone 2, TraceQL and LogQL search and discard rules`. Watch
   `release.yml` until the new release (`v0.1.1` or later) is published
   with the `.dmg`. Its notes carry both checklist sections.
4. **Tell the user** that the M2 release is ready, and that its notes hold
   the M1 and M2 checklists.
