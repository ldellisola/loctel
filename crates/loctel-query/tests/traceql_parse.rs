//! TraceQL lexer and parser: what parses, what does not and where the error is,
//! precedence, and the whole-trace classification. The grammar followed is
//! Tempo's `pkg/traceql/expr.y` (see `docs/areas/query.md`).

use loctel_model::{SpanKind, StatusCode};
use loctel_query::traceql::{
    self, Attribute, AttributeScope, CompareOp, FieldExpr, Intrinsic, Operand, PipelineStage,
    SpansetExpr, SpansetOp, Static, TraceQuery, parse,
};
use loctel_query::{Pattern, QueryError, parse_duration};

/// The queries the plan says must parse.
const MUST_PASS: &[&str] = &[
    r#"{ }"#,
    r#"{ span.http.request.method = "GET" }"#,
    r#"{ resource.service.name = "payments-api" && status = error }"#,
    r#"{ .http.response.status_code >= 500 }"#,
    r#"{ duration > 500ms }"#,
    r#"{ name =~ "POST.*" }"#,
    r#"{ kind = server } >> { status = error }"#,
    r#"{ resource.service.name = "web-bff" } >> { name = "POST /charges" }"#,
    r#"{ resource.service.name = "orders-api" } > { resource.service.name = "payments-api" }"#,
    r#"{ rootServiceName = "web-bff" && traceDuration > 1s }"#,
    r#"{ status = error } | count() > 3"#,
    r#"{ name = "SELECT carts" } ~ { kind = client }"#,
    r#"{ span:name = "POST /charges" }"#,
    r#"{ resource.service.name = "web-bff" } !>> { status = error }"#,
    r#"{ span."http.route" = "/api/checkout" }"#,
    r#"({ kind = server } && { kind = client }) || { status = error }"#,
    r#"{ span.order.id != nil }"#,
    r#"{ duration > 1h30m }"#,
];

/// Queries from Tempo's own `test_examples.yaml` that fall inside the subset
/// loctel supports (the rest is in `UNSUPPORTED`).
const TEMPO_EXAMPLES: &[&str] = &[
    "{ true }",
    "{ !true }",
    "{ true && false }",
    "{ true || false }",
    "{ 1 = 2 }",
    "{ 1 <= 1.1 }",
    "{ -1 = 2 }",
    r#"{ "test" =~ "test" }"#,
    r#"{ "test" !~ "test" }"#,
    "{ .a }",
    "{ !.a }",
    "{ .a && false }",
    "{ .a || true }",
    r#"{ .a != "test" }"#,
    r#"{ resource.a != 3 }"#,
    r#"{ !("test" != .c || ((true && .b) || 3 < .a)) }"#,
    "{ status = ok }",
    "{ status = unset }",
    "{ status != error }",
    r#"{ statusMessage = "" }"#,
    r#"{ statusMessage =~ ".*" }"#,
    "{ kind = consumer }",
    "{ kind = unspecified }",
    "{ kind = producer }",
    "{ kind = internal }",
    "{ traceDuration > 1s }",
    r#"{ rootName != "foo" }"#,
    "{ 1 < 1h }",
    "{ span.foo != 3.0 }",
    r#"{ ."foo bar" = "v" }"#,
    r#"{ span."foo bar" = "v" }"#,
    r#"{ resource."foo bar" = "v" }"#,
    r#"{ ."a\\b" = "v" }"#,
    r#"{ ."a\"b" = "v" }"#,
    "{ trace:duration > 2s }",
    r#"{ trace:rootName = "a" }"#,
    r#"{ trace:rootService = "a" }"#,
    "{ span:duration > 2s }",
    "{ span:status = error }",
    "{ span:kind = client }",
    r#"{ span:statusMessage = "status message" }"#,
    "{ true } && { true }",
    "{ true } || { true }",
    "{ true } >> { true }",
    "{ true } << { true }",
    "{ true } > { true }",
    "{ true } < { true }",
    "{ true } ~ { true }",
    "{ true } !> { true }",
    "{ true } !< { true }",
    "{ true } !~ { true }",
    "{ true } !>> { true }",
    "{ true } !<< { true }",
    r#"{.attr="a" || .attr="b"}"#,
    r#"{.attr=~"a.*" || .attr=~"b.*"}"#,
    "{ true } | count() = 1",
    "{}",
    "{ } | count() >= 2 | count() < 10",
    "  { name = \"x\" }  ",
];

#[test]
fn must_pass_queries_parse() {
    for q in MUST_PASS {
        if let Err(e) = parse(q) {
            panic!("`{q}` should parse, but failed: {e}");
        }
    }
}

#[test]
fn tempo_examples_in_the_supported_subset_parse() {
    for q in TEMPO_EXAMPLES {
        if let Err(e) = parse(q) {
            panic!("`{q}` should parse, but failed: {e}");
        }
    }
}

#[test]
fn parse_errors_have_positions() {
    let e = parse("{ name = }").unwrap_err();
    assert_eq!(e.position, 9, "column 10 is the `}}`: {e}");
    assert!(e.message.starts_with("expected a value"), "{e}");
    assert_eq!(e.to_string(), format!("at column 10: {}", e.message));

    // At the very end of the query: the message is exactly "expected `}`".
    let e = parse(r#"{ name = "x""#).unwrap_err();
    assert_eq!(e.position, 12, "the end of the query");
    assert_eq!(e.message, "expected `}`");

    let e = parse(r#"{ nme = "x" }"#).unwrap_err();
    assert_eq!(e.position, 2, "column 3");
    assert_eq!(e.message, "unknown intrinsic `nme`");
    assert_eq!(
        e.to_string(),
        "at column 3: unknown intrinsic `nme`",
        "Display is one-based"
    );
}

#[test]
fn unsupported_features_are_errors_with_position() {
    let e = parse("{ status = error } | select(span.foo)").unwrap_err();
    assert_eq!(e.position, 21, "column 22");
    assert_eq!(e.message, "select is not supported yet");
    assert_eq!(e.to_string(), "at column 22: select is not supported yet");

    let e = parse("{ } | rate()").unwrap_err();
    assert_eq!(e.position, 6);
    assert_eq!(e.message, "rate is not supported yet");
}

/// Every recognised-but-unsupported feature: (query, position of the feature,
/// the feature as the message names it).
#[test]
fn every_unsupported_feature_names_itself_and_its_position() {
    let cases: &[(&str, usize, &str)] = &[
        ("{ } | by(name)", 6, "by"),
        ("{ } | avg(duration) > 1s", 6, "avg"),
        ("{ } | min(duration) > 1s", 6, "min"),
        ("{ } | max(duration) > 1s", 6, "max"),
        ("{ } | sum(duration) > 1s", 6, "sum"),
        ("{ } | rate()", 6, "rate"),
        ("{ } | count_over_time()", 6, "count_over_time"),
        (
            "{ } | quantile_over_time(duration, 0.9)",
            6,
            "quantile_over_time",
        ),
        (
            "{ } | histogram_over_time(duration)",
            6,
            "histogram_over_time",
        ),
        ("{ } | min_over_time(duration)", 6, "min_over_time"),
        ("{ } | max_over_time(duration)", 6, "max_over_time"),
        ("{ } | avg_over_time(duration)", 6, "avg_over_time"),
        ("{ } | sum_over_time(duration)", 6, "sum_over_time"),
        ("{ } | coalesce()", 6, "coalesce"),
        ("{ } | topk(10)", 6, "topk"),
        ("{ } | bottomk(10)", 6, "bottomk"),
        ("{ } | compare({ status = error })", 6, "compare"),
        // At the start of a query, and with whitespace before the paren.
        ("select(.a)", 0, "select"),
        ("by (name) | { }", 0, "by"),
        ("{ } | count() > 1 | by (name)", 20, "by"),
        // Query hints come last.
        ("{ } with(most_recent=true)", 4, "with"),
        ("{ } | count() > 1 with (x=1)", 18, "with"),
        // Scopes and intrinsics that exist in Tempo but not here.
        (r#"{ event.name = "x" }"#, 2, "the `event.` scope"),
        (r#"{ link.foo = "x" }"#, 2, "the `link.` scope"),
        (
            r#"{ instrumentation.name = "x" }"#,
            2,
            "the `instrumentation.` scope",
        ),
        (r#"{ parent.span.foo = "x" }"#, 2, "the `parent.` scope"),
        (
            r#"{ span.a = "x" && event.b = "y" }"#,
            18,
            "the `event.` scope",
        ),
        (r#"{ event:name = "x" }"#, 2, "`event:name`"),
        (r#"{ link:traceID = "x" }"#, 2, "`link:traceID`"),
        (
            r#"{ instrumentation:version = "x" }"#,
            2,
            "`instrumentation:version`",
        ),
        (r#"{ span:id = "x" }"#, 2, "`span:id`"),
        (r#"{ span:parentID = "x" }"#, 2, "`span:parentID`"),
        (r#"{ trace:id = "x" }"#, 2, "`trace:id`"),
        ("{ childCount > 1 }", 2, "`childCount`"),
        ("{ parent = nil }", 2, "`parent`"),
        // Structural union operators.
        ("{ } &> { }", 4, "the structural union operator `&>`"),
        ("{ } &>> { }", 4, "the structural union operator `&>>`"),
        ("{ } &< { }", 4, "the structural union operator `&<`"),
        ("{ } &<< { }", 4, "the structural union operator `&<<`"),
        ("{ } &~ { }", 4, "the structural union operator `&~`"),
        // Arithmetic.
        ("{ .a + 1 > 2 }", 5, "arithmetic"),
        ("{ .a * 2 > 2 }", 5, "arithmetic"),
        ("{ duration > 1s - 1ms }", 16, "arithmetic"),
        ("{ .a / 2 > 2 }", 5, "arithmetic"),
        ("{ .a ^ 2 > 2 }", 5, "arithmetic"),
        // A pipeline stage that is a spanset filter, or a pipeline in
        // parentheses.
        ("{ } | { status = error }", 6, "a spanset filter after `|`"),
        (
            "({ } | count() > 1) >> { }",
            5,
            "a pipeline inside parentheses",
        ),
    ];
    let mut wrong = Vec::new();
    for (query, position, feature) in cases {
        let expected = format!("{feature} is not supported yet");
        match parse(query) {
            Err(e) if e.position == *position && e.message == expected => {}
            Err(e) => wrong.push(format!(
                "`{query}`: got {e} at {}, wanted `{expected}` at {position}",
                e.position
            )),
            Ok(_) => wrong.push(format!(
                "`{query}` parsed, wanted `{expected}` at {position}"
            )),
        }
    }
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
}

#[test]
fn needs_whole_trace_classification() {
    let needs = |q: &str| parse(q).unwrap().needs_whole_trace();
    assert!(!needs(r#"{ name = "x" }"#));
    assert!(!needs("{ }"));
    assert!(
        !needs(r#"({ name = "x" })"#),
        "parentheses alone change nothing"
    );
    assert!(!needs(
        r#"{ resource.service.name = "a" && (status = error || duration > 1s) }"#
    ));
    assert!(needs(r#"{ name = "a" } >> { name = "b" }"#));
    assert!(needs(r#"{ name = "a" } > { name = "b" }"#));
    assert!(needs(r#"{ name = "a" } ~ { name = "b" }"#));
    assert!(needs(r#"{ name = "a" } !~ { name = "b" }"#));
    assert!(needs(r#"{ name = "a" } && { name = "b" }"#));
    assert!(needs(r#"{ name = "a" } || { name = "b" }"#));
    assert!(needs(r#"{ rootName = "x" }"#));
    assert!(needs(r#"{ rootServiceName = "x" }"#));
    assert!(needs("{ traceDuration > 1s }"));
    assert!(needs(r#"{ trace:rootName = "x" }"#));
    assert!(needs(r#"{ trace:rootService = "x" }"#));
    assert!(needs("{ trace:duration > 1s }"));
    assert!(
        needs(r#"{ name = "a" && !(rootName = "x") }"#),
        "nested use counts"
    );
    assert!(needs("{ } | count() > 1"));
    assert!(needs("{ status = error } | count() > 3"));
    // A span-level duration is not the trace duration.
    assert!(!needs("{ duration > 1s }"));
    assert!(!needs("{ span:duration > 1s }"));
}

#[test]
fn duration_literals() {
    assert_eq!(parse_duration("500ms"), Some(500_000_000));
    assert_eq!(parse_duration("1.5s"), Some(1_500_000_000));
    assert_eq!(parse_duration("1h30m"), Some(5_400_000_000_000));
    assert_eq!(parse_duration("5x"), None);
}

#[test]
fn duration_units_and_edges() {
    assert_eq!(parse_duration("1ns"), Some(1));
    assert_eq!(parse_duration("2us"), Some(2_000));
    assert_eq!(parse_duration("2µs"), Some(2_000), "micro sign");
    assert_eq!(parse_duration("2μs"), Some(2_000), "Greek mu");
    assert_eq!(parse_duration("3ms"), Some(3_000_000));
    assert_eq!(parse_duration("4s"), Some(4_000_000_000));
    assert_eq!(parse_duration("5m"), Some(300_000_000_000));
    assert_eq!(parse_duration("6h"), Some(21_600_000_000_000));
    assert_eq!(parse_duration("0s"), Some(0));
    assert_eq!(parse_duration("0.3ms"), Some(300_000));
    assert_eq!(parse_duration(".5s"), Some(500_000_000));
    assert_eq!(parse_duration("1m30.5s"), Some(90_500_000_000));
    assert_eq!(parse_duration("1h1m1s1ms1us1ns"), Some(3_661_001_001_001));
    // Not durations.
    assert_eq!(parse_duration(""), None);
    assert_eq!(parse_duration("5"), None, "a unit is required");
    assert_eq!(parse_duration("ms"), None);
    assert_eq!(
        parse_duration("1.s"),
        Some(1_000_000_000),
        "digits after the dot are optional"
    );
    assert_eq!(parse_duration("1..5s"), None);
    assert_eq!(parse_duration("1 s"), None);
    assert_eq!(parse_duration(" 1s"), None);
    assert_eq!(parse_duration("-1s"), None);
    assert_eq!(parse_duration("1s2"), None, "a trailing number has no unit");
    assert_eq!(parse_duration("1d"), None);
    // Too big for u64 nanoseconds: None, never a wrap-around.
    assert_eq!(parse_duration("99999999999999999999h"), None);
    assert_eq!(parse_duration("18446744073709551616ns"), None);
    assert_eq!(parse_duration("18446744073709551615ns"), Some(u64::MAX));
}

#[test]
fn query_error_is_a_display_error() {
    let e = QueryError {
        message: "boom".into(),
        position: 2,
    };
    assert_eq!(e.to_string(), "at column 3: boom");
    let as_error: &dyn std::error::Error = &e;
    assert_eq!(as_error.to_string(), "at column 3: boom");
    assert_eq!(e.clone(), e);
}

#[test]
fn pattern_anchors_and_compares() {
    let anchored = Pattern::anchored("POST").unwrap();
    assert!(anchored.is_match("POST"));
    assert!(!anchored.is_match("POST /charges"), "anchored at both ends");
    assert!(!anchored.is_match("XPOST"));
    assert_eq!(
        anchored.source(),
        "POST",
        "the source is what the user wrote"
    );

    let alt = Pattern::anchored("a|b").unwrap();
    assert!(alt.is_match("a") && alt.is_match("b"));
    assert!(
        !alt.is_match("ab"),
        "the anchors apply to the whole alternation"
    );
    assert!(!alt.is_match("xa"));

    let loose = Pattern::unanchored("attempt").unwrap();
    assert!(loose.is_match("retry attempt 2"));
    assert_eq!(loose.source(), "attempt");

    assert_eq!(anchored, Pattern::anchored("POST").unwrap());
    assert_ne!(
        anchored,
        Pattern::unanchored("POST").unwrap(),
        "anchoring is part of equality"
    );
    assert_ne!(anchored, Pattern::anchored("GET").unwrap());

    // Bad regexes are errors, and unbalanced groups cannot escape the anchors.
    assert!(Pattern::anchored("(").is_err());
    assert!(Pattern::anchored("a)|(b").is_err());
    assert!(Pattern::unanchored("[").is_err());
}

// ---- the AST ----------------------------------------------------------------

fn filter_of(q: &str) -> FieldExpr {
    match parse(q).unwrap_or_else(|e| panic!("`{q}`: {e}")).spanset {
        SpansetExpr::Filter(f) => f,
        other => panic!("`{q}` is not a single filter: {other:?}"),
    }
}

fn attr(scope: AttributeScope, name: &str) -> Operand {
    Operand::Attribute(Attribute {
        scope,
        name: name.to_string(),
    })
}

fn string(s: &str) -> Operand {
    Operand::Static(Static::Str(s.to_string()))
}

fn int(n: i64) -> Operand {
    Operand::Static(Static::Int(n))
}

fn cmp(op: CompareOp, lhs: Operand, rhs: Operand) -> FieldExpr {
    FieldExpr::Compare { op, lhs, rhs }
}

fn and(a: FieldExpr, b: FieldExpr) -> FieldExpr {
    FieldExpr::And(Box::new(a), Box::new(b))
}

fn or(a: FieldExpr, b: FieldExpr) -> FieldExpr {
    FieldExpr::Or(Box::new(a), Box::new(b))
}

/// `{ name = "<x>" }`, a filter that is easy to tell apart in a spanset tree.
fn named(x: &str) -> SpansetExpr {
    SpansetExpr::Filter(cmp(
        CompareOp::Eq,
        Operand::Intrinsic(Intrinsic::Name),
        string(x),
    ))
}

fn binary(op: SpansetOp, lhs: SpansetExpr, rhs: SpansetExpr) -> SpansetExpr {
    SpansetExpr::Binary {
        op,
        lhs: Box::new(lhs),
        rhs: Box::new(rhs),
    }
}

fn spanset_of(q: &str) -> SpansetExpr {
    parse(q).unwrap_or_else(|e| panic!("`{q}`: {e}")).spanset
}

#[test]
fn an_empty_filter_is_true() {
    assert_eq!(
        filter_of("{ }"),
        FieldExpr::Operand(Operand::Static(Static::Bool(true)))
    );
    assert_eq!(filter_of("{}"), filter_of("{ true }"));
}

#[test]
fn attribute_scopes_and_names() {
    use AttributeScope::{Any, Resource, Span};
    assert_eq!(
        filter_of(r#"{ span.http.request.method = "GET" }"#),
        cmp(
            CompareOp::Eq,
            attr(Span, "http.request.method"),
            string("GET")
        )
    );
    assert_eq!(
        filter_of("{ resource.service.name = \"a\" }"),
        cmp(CompareOp::Eq, attr(Resource, "service.name"), string("a"))
    );
    assert_eq!(
        filter_of("{ .http.response.status_code >= 500 }"),
        cmp(
            CompareOp::Gte,
            attr(Any, "http.response.status_code"),
            int(500)
        )
    );
    // A quoted name, and a name with characters that are legal unquoted.
    assert_eq!(
        filter_of(r#"{ span."http status" = "x" }"#),
        cmp(CompareOp::Eq, attr(Span, "http status"), string("x"))
    );
    assert_eq!(
        filter_of(r#"{ ."a\"b" = "x" }"#),
        cmp(CompareOp::Eq, attr(Any, "a\"b"), string("x"))
    );
    assert_eq!(
        filter_of(r#"{ ."a\\b" = "x" }"#),
        cmp(CompareOp::Eq, attr(Any, "a\\b"), string("x"))
    );
    assert_eq!(
        filter_of(r#"{ span.http-status/code = 1 }"#),
        cmp(CompareOp::Eq, attr(Span, "http-status/code"), int(1))
    );
    // No spaces needed around operators, as in Tempo: the name ends at an
    // operator character.
    assert_eq!(
        filter_of(r#"{span.a="x"||span.b=~"y"}"#),
        or(
            cmp(CompareOp::Eq, attr(Span, "a"), string("x")),
            FieldExpr::Regex {
                negated: false,
                lhs: attr(Span, "b"),
                pattern: Pattern::anchored("y").unwrap(),
            }
        )
    );
}

#[test]
fn intrinsics_old_and_scoped_forms_agree() {
    let pairs = [
        ("name", "span:name"),
        ("status", "span:status"),
        ("statusMessage", "span:statusMessage"),
        ("kind", "span:kind"),
        ("duration", "span:duration"),
        ("rootName", "trace:rootName"),
        ("rootServiceName", "trace:rootService"),
        ("traceDuration", "trace:duration"),
    ];
    for (old, scoped) in pairs {
        assert_eq!(
            filter_of(&format!("{{ {old} = 1 }}")),
            filter_of(&format!("{{ {scoped} = 1 }}")),
            "{old} vs {scoped}"
        );
    }
    assert_eq!(
        filter_of("{ rootServiceName = 1 }"),
        cmp(
            CompareOp::Eq,
            Operand::Intrinsic(Intrinsic::RootServiceName),
            int(1)
        )
    );
}

#[test]
fn static_values() {
    let value = |q: &str| match filter_of(q) {
        FieldExpr::Compare { rhs, .. } => rhs,
        other => panic!("`{q}`: {other:?}"),
    };
    assert_eq!(value(r#"{ .a = "x" }"#), string("x"));
    assert_eq!(
        value(r#"{ .a = "a\"b\\c\nd\te" }"#),
        string("a\"b\\c\nd\te")
    );
    assert_eq!(
        value("{ .a = `a\\b\"c` }"),
        string("a\\b\"c"),
        "backticks are raw"
    );
    assert_eq!(value("{ .a = 42 }"), int(42));
    assert_eq!(value("{ .a = -42 }"), int(-42));
    assert_eq!(value("{ .a = 1.5 }"), Operand::Static(Static::Float(1.5)));
    assert_eq!(value("{ .a = .5 }"), Operand::Static(Static::Float(0.5)));
    assert_eq!(value("{ .a = -1.5 }"), Operand::Static(Static::Float(-1.5)));
    assert_eq!(
        value("{ .a = 500ms }"),
        Operand::Static(Static::Duration(500_000_000))
    );
    assert_eq!(
        value("{ .a = 1h30m }"),
        Operand::Static(Static::Duration(5_400_000_000_000))
    );
    assert_eq!(value("{ .a = true }"), Operand::Static(Static::Bool(true)));
    assert_eq!(
        value("{ .a = false }"),
        Operand::Static(Static::Bool(false))
    );
    assert_eq!(
        value("{ .a = ok }"),
        Operand::Static(Static::Status(StatusCode::Ok))
    );
    assert_eq!(
        value("{ .a = error }"),
        Operand::Static(Static::Status(StatusCode::Error))
    );
    assert_eq!(
        value("{ .a = unset }"),
        Operand::Static(Static::Status(StatusCode::Unset))
    );
    for (word, kind) in [
        ("unspecified", SpanKind::Unspecified),
        ("internal", SpanKind::Internal),
        ("server", SpanKind::Server),
        ("client", SpanKind::Client),
        ("producer", SpanKind::Producer),
        ("consumer", SpanKind::Consumer),
    ] {
        assert_eq!(
            value(&format!("{{ kind = {word} }}")),
            Operand::Static(Static::Kind(kind)),
            "{word}"
        );
    }
}

#[test]
fn nil_comparisons_become_existence_tests() {
    use AttributeScope::Span;
    let order = attr(Span, "order.id");
    assert_eq!(
        filter_of("{ span.order.id != nil }"),
        FieldExpr::Exists(order.clone())
    );
    assert_eq!(
        filter_of("{ span.order.id = nil }"),
        FieldExpr::NotExists(order.clone())
    );
    assert_eq!(
        filter_of("{ nil != span.order.id }"),
        FieldExpr::Exists(order.clone())
    );
    assert_eq!(
        filter_of("{ nil = span.order.id }"),
        FieldExpr::NotExists(order)
    );
    assert_eq!(
        filter_of("{ nil = nil }"),
        FieldExpr::Operand(Operand::Static(Static::Bool(false)))
    );
    for q in [
        "{ span.a > nil }",
        "{ span.a =~ nil }",
        "{ nil < span.a }",
        "{ nil }",
    ] {
        assert!(parse(q).is_err(), "`{q}` must not parse");
    }
    let e = parse("{ span.a > nil }").unwrap_err();
    assert_eq!(e.position, 9, "at the operator");
    assert!(e.message.contains("nil"), "{e}");
}

#[test]
fn regex_operands_are_compiled_and_anchored() {
    use AttributeScope::Span;
    let FieldExpr::Regex {
        negated,
        lhs,
        pattern,
    } = filter_of(r#"{ span.url.path =~ "/health(/.*)?" }"#)
    else {
        panic!("expected a regex node");
    };
    assert!(!negated);
    assert_eq!(lhs, attr(Span, "url.path"));
    assert!(pattern.is_match("/health"));
    assert!(pattern.is_match("/health/live"));
    assert!(!pattern.is_match("/api/health"), "anchored at the start");
    assert!(!pattern.is_match("/healthy"), "anchored at the end");
    assert_eq!(pattern.source(), "/health(/.*)?");

    let FieldExpr::Regex { negated, .. } = filter_of(r#"{ name !~ "x" }"#) else {
        panic!("expected a regex node");
    };
    assert!(negated);

    // Backticks keep regex escapes readable.
    let FieldExpr::Regex { pattern, .. } = filter_of(r"{ name =~ `GET /\d+` }") else {
        panic!("expected a regex node");
    };
    assert!(pattern.is_match("GET /42"));
    assert!(!pattern.is_match("GET /x"));
}

#[test]
fn regex_errors_point_at_the_pattern() {
    let e = parse(r#"{ name =~ "(" }"#).unwrap_err();
    assert_eq!(e.position, 10, "the string literal");
    assert!(e.message.starts_with("invalid regular expression"), "{e}");
    assert!(
        !e.message.contains('\n'),
        "one line, no regex-crate caret art: {e:?}"
    );

    let e = parse("{ name =~ name }").unwrap_err();
    assert_eq!(e.position, 10);
    assert!(e.message.contains("string"), "{e}");
    let e = parse("{ name !~ 5 }").unwrap_err();
    assert_eq!(e.position, 10);
}

#[test]
fn bare_boolean_and_negation() {
    use AttributeScope::Span;
    assert_eq!(
        filter_of("{ span.cache.hit }"),
        FieldExpr::Operand(attr(Span, "cache.hit"))
    );
    assert_eq!(
        filter_of("{ !span.cache.hit }"),
        FieldExpr::Not(Box::new(FieldExpr::Operand(attr(Span, "cache.hit"))))
    );
    assert_eq!(
        filter_of("{ !(status = error) }"),
        FieldExpr::Not(Box::new(cmp(
            CompareOp::Eq,
            Operand::Intrinsic(Intrinsic::Status),
            Operand::Static(Static::Status(StatusCode::Error))
        )))
    );
    assert_eq!(
        filter_of("{ !!span.a }"),
        FieldExpr::Not(Box::new(FieldExpr::Not(Box::new(FieldExpr::Operand(
            attr(Span, "a")
        )))))
    );
    // `!` takes a whole condition. `!a = b` is ambiguous (Tempo reads it as
    // `(!a) = b`), so it is an error rather than a guess.
    let e = parse("{ !span.a = 1 }").unwrap_err();
    assert_eq!(e.position, 2, "at the `!`");
    assert!(e.message.contains('!'), "{e}");
    // Only attributes and true/false are boolean on their own.
    for q in [
        "{ name }",
        "{ duration }",
        r#"{ "x" }"#,
        "{ 5 }",
        "{ status }",
    ] {
        assert!(parse(q).is_err(), "`{q}` must not parse");
    }
    let e = parse("{ name }").unwrap_err();
    assert_eq!(e.position, 7, "where a comparison should be");
    assert!(
        e.message.starts_with("expected a comparison operator"),
        "{e}"
    );
}

#[test]
fn comparison_operators() {
    let ops = [
        ("=", CompareOp::Eq),
        ("!=", CompareOp::Neq),
        (">", CompareOp::Gt),
        ("<", CompareOp::Lt),
        (">=", CompareOp::Gte),
        ("<=", CompareOp::Lte),
    ];
    for (text, op) in ops {
        assert_eq!(
            filter_of(&format!("{{ .a {text} 1 }}")),
            cmp(op, attr(AttributeScope::Any, "a"), int(1)),
            "{text}"
        );
    }
    // A comparison can have a value on the left.
    assert_eq!(
        filter_of("{ 3 < .a }"),
        cmp(CompareOp::Lt, int(3), attr(AttributeScope::Any, "a"))
    );
    // Comparisons do not chain.
    let e = parse("{ .a = 1 = 2 }").unwrap_err();
    assert_eq!(e.position, 9);
    assert!(e.message.contains("chain"), "{e}");
}

#[test]
fn field_precedence_follows_tempo() {
    use AttributeScope::Any;
    let a = || cmp(CompareOp::Eq, attr(Any, "a"), int(1));
    let b = || cmp(CompareOp::Eq, attr(Any, "b"), int(2));
    let c = || cmp(CompareOp::Eq, attr(Any, "c"), int(3));

    // Comparisons bind tighter than `&&` and `||`.
    assert_eq!(filter_of("{ .a = 1 && .b = 2 }"), and(a(), b()));
    // `&&` and `||` are ONE precedence level, left-associative: expr.y has
    // `%left AND OR` on a single line. So these are not the usual C reading.
    assert_eq!(
        filter_of("{ .a = 1 || .b = 2 && .c = 3 }"),
        and(or(a(), b()), c())
    );
    assert_eq!(
        filter_of("{ .a = 1 && .b = 2 || .c = 3 }"),
        or(and(a(), b()), c())
    );
    assert_eq!(
        filter_of("{ .a = 1 && .b = 2 && .c = 3 }"),
        and(and(a(), b()), c())
    );
    // Parentheses override.
    assert_eq!(
        filter_of("{ .a = 1 || (.b = 2 && .c = 3) }"),
        or(a(), and(b(), c()))
    );
    assert_eq!(
        filter_of("{ (.a = 1 || .b = 2) && .c = 3 }"),
        and(or(a(), b()), c())
    );
}

#[test]
fn spanset_precedence_follows_tempo() {
    use SpansetOp::*;
    let (a, b, c) = (|| named("a"), || named("b"), || named("c"));
    let q = |x: &str, op: &str, y: &str| format!(r#"{{ name = "{x}" }} {op} {{ name = "{y}" }}"#);

    assert_eq!(spanset_of(&q("a", ">>", "b")), binary(Descendant, a(), b()));

    // Structural operators bind tighter than `&&` and `||`.
    assert_eq!(
        spanset_of(r#"{ name = "a" } && { name = "b" } >> { name = "c" }"#),
        binary(And, a(), binary(Descendant, b(), c()))
    );
    assert_eq!(
        spanset_of(r#"{ name = "a" } >> { name = "b" } && { name = "c" }"#),
        binary(And, binary(Descendant, a(), b()), c())
    );
    // Structural operators are left-associative among themselves.
    assert_eq!(
        spanset_of(r#"{ name = "a" } >> { name = "b" } ~ { name = "c" }"#),
        binary(Sibling, binary(Descendant, a(), b()), c())
    );
    assert_eq!(
        spanset_of(r#"{ name = "a" } ~ { name = "b" } >> { name = "c" }"#),
        binary(Descendant, binary(Sibling, a(), b()), c())
    );
    // `&&` and `||` share a level here too, left-associative.
    assert_eq!(
        spanset_of(r#"{ name = "a" } || { name = "b" } && { name = "c" }"#),
        binary(And, binary(Or, a(), b()), c())
    );
    assert_eq!(
        spanset_of(r#"{ name = "a" } && { name = "b" } || { name = "c" }"#),
        binary(Or, binary(And, a(), b()), c())
    );
    // Parentheses override, and a parenthesised single filter is just the filter.
    assert_eq!(
        spanset_of(r#"{ name = "a" } >> ({ name = "b" } && { name = "c" })"#),
        binary(Descendant, a(), binary(And, b(), c()))
    );
    assert_eq!(spanset_of(r#"(({ name = "a" }))"#), a());
}

#[test]
fn every_structural_operator_maps_to_its_variant() {
    use SpansetOp::*;
    let ops = [
        ("&&", And),
        ("||", Or),
        (">", Child),
        ("<", Parent),
        (">>", Descendant),
        ("<<", Ancestor),
        ("~", Sibling),
        ("!>", NotChild),
        ("!<", NotParent),
        ("!>>", NotDescendant),
        ("!<<", NotAncestor),
        ("!~", NotSibling),
    ];
    for (text, op) in ops {
        assert_eq!(
            spanset_of(&format!(r#"{{ name = "a" }} {text} {{ name = "b" }}"#)),
            binary(op, named("a"), named("b")),
            "{text}"
        );
        // No whitespace needed around the operator either.
        assert_eq!(
            spanset_of(&format!(r#"{{ name = "a" }}{text}{{ name = "b" }}"#)),
            binary(op, named("a"), named("b")),
            "{text} without spaces"
        );
    }
}

#[test]
fn pipelines() {
    let q: TraceQuery = parse("{ status = error } | count() > 3").unwrap();
    assert_eq!(
        q.stages,
        vec![PipelineStage::Count {
            op: CompareOp::Gt,
            value: 3.0
        }]
    );
    let q = parse("{ } | count() >= 2 | count() != 7.5").unwrap();
    assert_eq!(
        q.stages,
        vec![
            PipelineStage::Count {
                op: CompareOp::Gte,
                value: 2.0
            },
            PipelineStage::Count {
                op: CompareOp::Neq,
                value: 7.5
            },
        ]
    );
    assert_eq!(parse("{ }").unwrap().stages, vec![]);
    // The pipeline applies to a whole spanset expression.
    let q = parse(r#"{ name = "a" } >> { name = "b" } | count() = 1"#).unwrap();
    assert!(matches!(q.spanset, SpansetExpr::Binary { .. }));
    assert_eq!(q.stages.len(), 1);
    // Whitespace inside `count( )` is fine.
    assert!(parse("{ } | count( ) > 1").is_ok());
}

#[test]
fn pipeline_errors() {
    for (q, position) in [
        ("{ } | count() > ", 16),   // number missing, at the end
        ("{ } | count() > 1s", 16), // a duration is not a number
        ("{ } | count() =~ 1", 14), // not a count comparison
        ("{ } | count > 1", 12),    // parentheses missing
        ("{ } | count(1) > 1", 12), // count takes no argument
        ("{ } | count()", 13),      // comparison missing, at the end
        ("{ } | ", 6),              // stage missing, at the end
        ("{ } | foo", 6),           // not a stage
        ("{ } count() > 1", 4),     // no pipe
        ("{ } | 3 < count()", 6),   // the number-first form is not supported
    ] {
        let e = parse(q).unwrap_err();
        assert_eq!(e.position, position, "`{q}`: {e}");
    }
    let e = parse("{ } | foo").unwrap_err();
    assert_eq!(e.message, "expected `count()`, found `foo`");
}

#[test]
fn parse_errors_table() {
    let cases: &[(&str, usize, &str)] = &[
        ("", 0, "expected `{`"),
        ("   ", 3, "expected `{`"),
        ("name = \"x\"", 0, "expected `{`, found `name`"),
        (
            "{ name = \"x\" } }",
            15,
            "expected `|` or end of query, found `}`",
        ),
        ("{ name = \"x\" } >>", 17, "expected `{`"),
        ("{ name = \"x\" } >> name", 18, "expected `{`, found `name`"),
        ("({ name = \"x\" }", 15, "expected `)`"),
        ("{ name = \"x\" ", 13, "expected `}`"),
        ("{ name = \"x\" name }", 13, "expected `}`, found `name`"),
        ("{ name = \"x\" && }", 16, "expected a value, found `}`"),
        (
            "{ name = \"x\" && status }",
            23,
            "expected a comparison operator, found `}`",
        ),
        ("{ (name = \"x\" }", 14, "expected `)`, found `}`"),
        ("{ name = = }", 9, "expected a value, found `=`"),
        ("{ name == \"x\" }", 8, "expected a value, found `=`"),
        (
            "{ name ! \"x\" }",
            7,
            "expected a comparison operator, found `!`",
        ),
        (
            "{ name = \"x\" >> status = error }",
            13,
            "expected `}`, found `>>`",
        ),
        ("{ } | | { }", 6, "expected `count()`, found `|`"),
        ("{ } , { }", 4, "expected `|` or end of query, found `,`"),
        (
            "{ span. = 1 }",
            7,
            "expected an attribute name after `span.`",
        ),
        ("{ . = 1 }", 3, "expected an attribute name after `.`"),
        (
            "{ resource.service.name = \"x\" && span.\"a }",
            38,
            "unterminated quoted attribute name",
        ),
        ("{ name = \"x }", 9, "unterminated string"),
        ("{ name = `x }", 9, "unterminated string"),
        ("{ name = \"a\\qb\" }", 11, "invalid escape sequence `\\q`"),
        (
            "{ name = 5x }",
            9,
            "invalid duration `5x` (units are ns, us, µs, ms, s, m, h)",
        ),
        ("{ name = 1.2.3 }", 9, "invalid number `1.2.3`"),
        (
            "{ name = 99999999999999999999 }",
            9,
            "integer `99999999999999999999` is out of range",
        ),
        (
            "{ name = -1s }",
            9,
            "a negative duration is not supported yet",
        ),
        ("{ name = @ }", 9, "unexpected character `@`"),
        ("{ name = $x }", 9, "unexpected character `$`"),
        ("{ name & }", 7, "unexpected character `&`"),
        (
            "{ http.status = 500 }",
            2,
            "unknown intrinsic `http` (attributes need a scope: `span.http`, `resource.http` or `.http`)",
        ),
        ("{ span:foo = 1 }", 2, "unknown intrinsic `span:foo`"),
        ("{ span: = 1 }", 2, "unknown intrinsic `span:`"),
        ("{ rootService = 1 }", 2, "unknown intrinsic `rootService`"),
        ("{ Name = 1 }", 2, "unknown intrinsic `Name`"),
        // A keyword value cannot be followed by an attribute.
        ("{ span.a = ok.b }", 13, "expected `}`, found `.b`"),
    ];
    let mut wrong = Vec::new();
    for (q, position, message) in cases {
        match parse(q) {
            Ok(ast) => wrong.push(format!("`{q}` should fail, parsed as {ast:?}")),
            Err(e) if e.position == *position && e.message == *message => {}
            Err(e) => wrong.push(format!(
                "`{q}`: got `{}` at {}, wanted `{message}` at {position}",
                e.message, e.position
            )),
        }
    }
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
}

#[test]
fn the_first_problem_in_the_query_is_the_one_reported() {
    // A bad character later in the text does not hide an earlier syntax error.
    let e = parse("{ name = } @").unwrap_err();
    assert_eq!(e.position, 9);
    assert!(e.message.starts_with("expected a value"), "{e}");
    // And a bad character is reported when it is the first problem.
    let e = parse("{ name = @ }").unwrap_err();
    assert_eq!(e.position, 9);
    assert_eq!(e.message, "unexpected character `@`");
    // Even an unterminated string at the end loses to an earlier error.
    let e = parse(r#"{ name = } "abc"#).unwrap_err();
    assert_eq!(e.position, 9);
}

#[test]
fn positions_are_character_indexes_not_bytes() {
    // `é` and `µ` are two bytes each; positions still count characters.
    let e = parse(r#"{ name = "é" } | x"#).unwrap_err();
    assert_eq!(e.position, 17);
    let e = parse(r#"{ span."µ" = 1 } | rate()"#).unwrap_err();
    assert_eq!(e.position, 19);
    assert_eq!(e.message, "rate is not supported yet");
    // Non-ASCII attribute names and values are fine.
    assert!(parse(r#"{ span.größe = "µs" }"#).is_ok());
    assert!(parse("{ duration > 2µs }").is_ok());
    assert!(parse("{ duration > 2μs }").is_ok());
}

#[test]
fn truncated_and_mangled_queries_never_panic() {
    let mut all: Vec<&str> = MUST_PASS.to_vec();
    all.extend(TEMPO_EXAMPLES);
    all.push(r#"{ span."a b" =~ `x\d` } | count() > 1 with (a=b)"#);
    for q in all {
        let chars: Vec<char> = q.chars().collect();
        for cut in 0..=chars.len() {
            let prefix: String = chars[..cut].iter().collect();
            if let Err(e) = parse(&prefix) {
                assert!(
                    e.position <= cut,
                    "`{prefix}`: position {} is past the end ({cut}): {e}",
                    e.position
                );
                assert!(!e.message.is_empty());
            }
            let suffix: String = chars[cut..].iter().collect();
            if let Err(e) = parse(&suffix) {
                assert!(e.position <= chars.len() - cut, "`{suffix}`: {e}");
            }
        }
    }
    // A few hostile inputs.
    for q in [
        "\0",
        "{{{{{{",
        "}}}}",
        "((((((",
        "\"",
        "`",
        "{ \"\\",
        "{ .\"\\",
        "{ span.\"",
        "{ ~~~ }",
        "{ a ",
        "| | |",
        "{ } &",
        "{ } &&",
        "{ } !",
        "{ } !!",
        &"(".repeat(5000),
        &"{ .a = 1 }".repeat(300).replace("}{", "} && {"),
    ] {
        let _ = parse(q);
    }
}

#[test]
fn deeply_nested_input_is_an_error_not_a_stack_overflow() {
    let deep = format!("{}{{ }}{}", "(".repeat(20_000), ")".repeat(20_000));
    let e = parse(&deep).unwrap_err();
    assert!(e.message.contains("too deeply nested"), "{e}");
    let deep_field = format!("{{ {}.a{} }}", "(".repeat(20_000), ")".repeat(20_000));
    let e = parse(&deep_field).unwrap_err();
    assert!(e.message.contains("too deeply nested"), "{e}");
    let deep_not = format!("{{ {}.a }}", "!".repeat(20_000));
    let e = parse(&deep_not).unwrap_err();
    assert!(e.message.contains("too deeply nested"), "{e}");
    // Reasonable nesting is fine.
    assert!(parse(&format!("{}{{ }}{}", "(".repeat(20), ")".repeat(20))).is_ok());
}

#[test]
fn a_huge_chain_of_operators_is_an_error_not_a_deep_tree() {
    // Left-associative chains build a left-leaning tree as deep as the chain
    // is long, which the evaluator would recurse through. The parser stops it.
    let spansets = vec!["{ }"; 2_000].join(" && ");
    let e = parse(&spansets).unwrap_err();
    assert!(e.message.contains("too many operators"), "{e}");
    let fields = format!("{{ {} }}", vec![".a"; 2_000].join(" || "));
    let e = parse(&fields).unwrap_err();
    assert!(e.message.contains("too many operators"), "{e}");
    // Hundreds are fine.
    let ok = vec!["{ }"; 400].join(" && ");
    assert!(parse(&ok).is_ok());
    let ok = format!("{{ {} }}", vec![".a"; 400].join(" || "));
    assert!(parse(&ok).is_ok());
}

#[test]
fn the_module_exposes_parse_and_the_ast_root() {
    // `traceql::parse` and `traceql::TraceQuery` are the entry points other
    // crates use.
    let q: traceql::TraceQuery = traceql::parse("{ }").unwrap();
    assert_eq!(q, parse("{ }").unwrap());
    assert_eq!(q.clone(), q);
}
