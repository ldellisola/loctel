//! The TraceQL evaluator: filters, typing, root intrinsics, structural
//! operators, spanset operators and pipelines, over the fixed `CheckoutFailure`
//! trace (the table in Task 8 of the M1 plan, whose rows the `#n` below name)
//! and over small hand-made traces for the edge cases.

mod fixtures;

use std::collections::BTreeSet;

use fixtures::{
    checkout_failure, in_service, matched, matched_indices, matched_numbers, mk, number,
    scenario_spans, with_attr, with_resource_attr,
};
use loctel_meshgen::Scenario;
use loctel_model::{AttrValue, Span, SpanKind, StatusCode};
use loctel_query::traceql::{self, TraceView};

fn cf() -> Vec<Span> {
    checkout_failure()
}

fn set(pairs: &[(&str, &str)]) -> BTreeSet<(String, String)> {
    pairs
        .iter()
        .map(|(service, name)| ((*service).to_owned(), (*name).to_owned()))
        .collect()
}

fn nums(numbers: &[usize]) -> BTreeSet<usize> {
    numbers.iter().copied().collect()
}

/// The numbers 1 to 15.
fn all() -> BTreeSet<usize> {
    (1..=15).collect()
}

/// The matched span numbers of `query` on the `CheckoutFailure` trace.
fn rows(query: &str) -> BTreeSet<usize> {
    matched_numbers(query, &cf())
}

/// Whether `query` matches any span of the trace made of `spans`.
fn hits(query: &str, spans: &[Span]) -> bool {
    !matched_indices(query, spans).is_empty()
}

/// Whether `query` matches a trace made of the single span `span`.
fn hit(query: &str, span: &Span) -> bool {
    hits(query, std::slice::from_ref(span))
}

// ---- the fixture itself -----------------------------------------------------

#[test]
fn checkout_failure_fixture_has_the_plans_15_spans() {
    let spans = cf();
    assert_eq!(spans.len(), 15);
    let numbers: BTreeSet<usize> = spans.iter().map(number).collect();
    assert_eq!(numbers, all());
}

#[test]
fn http_status_code_is_an_int_attribute() {
    // The plan's `comparisons_follow_tempo_typing` needs an int here, as the
    // semantic conventions say; meshgen already makes it one.
    for span in cf() {
        if let Some((_, value)) = span
            .attributes
            .iter()
            .find(|(key, _)| key == "http.response.status_code")
        {
            assert!(matches!(value, AttrValue::Int(_)), "{value:?}");
        }
    }
}

// ---- the plan's examples ----------------------------------------------------

#[test]
fn user_example_name_with_ancestor_from_service() {
    assert_eq!(
        matched(
            r#"{ resource.service.name = "web-bff" } >> { name = "POST /charges" }"#,
            &cf()
        ),
        set(&[("payments-api", "POST /charges")])
    );
}

#[test]
fn child_operator() {
    assert_eq!(
        matched(
            r#"{ resource.service.name = "orders-api" } > { resource.service.name = "payments-api" }"#,
            &cf()
        ),
        set(&[("payments-api", "POST /charges")])
    );
}

#[test]
fn sibling_operator() {
    assert_eq!(
        matched(r#"{ name = "SELECT carts" } ~ { kind = client }"#, &cf()),
        set(&[("orders-api", "GET"), ("orders-api", "POST")])
    );
}

#[test]
fn negated_descendant() {
    assert_eq!(
        matched(
            r#"{ resource.service.name = "web-bff" } !>> { status = error }"#,
            &cf()
        ),
        set(&[("web-bff", "POST /api/checkout")])
    );
}

#[test]
fn traceql_regex_is_anchored() {
    assert!(
        matched(r#"{ name =~ "POST" }"#, &cf())
            .iter()
            .all(|(_, n)| n == "POST")
    );
    assert!(
        matched(r#"{ name =~ "POST.*" }"#, &cf())
            .contains(&("payments-api".into(), "POST /charges".into()))
    );
}

#[test]
fn comparisons_follow_tempo_typing() {
    assert!(matched(r#"{ span.http.response.status_code = "500" }"#, &cf()).is_empty()); // int attr vs string
    assert_eq!(
        matched(r#"{ span.http.response.status_code = 500.0 }"#, &cf()).len(),
        2
    ); // spans 1 and 4
    assert!(matched(r#"{ span.no.such.attr != "x" }"#, &cf()).is_empty());
}

#[test]
fn status_and_service_filter() {
    assert_eq!(
        rows(r#"{ resource.service.name = "payments-api" && status = error }"#),
        nums(&[10, 12, 13])
    );
}

#[test]
fn duration_filter() {
    assert_eq!(rows("{ duration > 1s }"), nums(&[1, 3, 4, 9, 10]));
    assert_eq!(rows("{ duration <= 6ms }"), nums(&[8, 14]));
    assert_eq!(rows("{ duration = 18ms }"), nums(&[2, 5]));
    assert_eq!(
        rows("{ duration >= 600ms && duration < 1s }"),
        nums(&[12, 13])
    );
}

#[test]
fn root_intrinsics() {
    assert_eq!(
        rows(r#"{ rootServiceName = "web-bff" && traceDuration > 1s }"#),
        all()
    );
    assert_eq!(rows(r#"{ rootName = "POST /api/checkout" }"#), all());
    assert!(rows(r#"{ rootName = "POST" }"#).is_empty());
    assert!(rows(r#"{ rootServiceName = "orders-api" }"#).is_empty());
    assert_eq!(rows(r#"{ rootName =~ "POST /api/.*" }"#), all());
    // The root intrinsics hold for every span, so they combine with the
    // span's own fields.
    assert_eq!(
        rows(r#"{ rootServiceName = "web-bff" && status = error }"#),
        nums(&[1, 3, 4, 9, 10, 12, 13])
    );
    // The trace runs from 0 to 1840 ms.
    assert_eq!(rows("{ traceDuration = 1840ms }"), all());
    assert_eq!(rows("{ traceDuration > 1839ms }"), all());
    assert!(rows("{ traceDuration > 1840ms }").is_empty());
}

#[test]
fn scoped_intrinsics_are_the_same_as_the_old_style() {
    assert_eq!(
        rows(r#"{ span:name = "POST /charges" }"#),
        rows(r#"{ name = "POST /charges" }"#)
    );
    assert_eq!(rows("{ span:status = error }"), rows("{ status = error }"));
    assert_eq!(rows("{ span:kind = server }"), rows("{ kind = server }"));
    assert_eq!(rows("{ span:duration > 1s }"), rows("{ duration > 1s }"));
    assert_eq!(
        rows(r#"{ span:statusMessage = "timeout" }"#),
        rows(r#"{ statusMessage = "timeout" }"#)
    );
    assert_eq!(
        rows(
            r#"{ trace:rootName = "POST /api/checkout" && trace:rootService = "web-bff" && trace:duration > 1s }"#
        ),
        all()
    );
}

#[test]
fn count_pipeline() {
    assert_eq!(
        rows("{ status = error } | count() > 3"),
        nums(&[1, 3, 4, 9, 10, 12, 13])
    );
    assert!(rows("{ status = error } | count() > 10").is_empty());
    assert_eq!(rows("{ status = error } | count() = 7").len(), 7);
    assert_eq!(rows("{ status = error } | count() >= 7").len(), 7);
    assert!(rows("{ status = error } | count() > 7").is_empty());
    assert!(rows("{ status = error } | count() < 7").is_empty());
    assert_eq!(rows("{ status = error } | count() <= 7").len(), 7);
    assert!(rows("{ status = error } | count() != 7").is_empty());
    assert_eq!(rows("{ status = error } | count() != 6").len(), 7);
    // Stages chain, each looking at what the one before kept.
    assert_eq!(
        rows("{ status = error } | count() >= 2 | count() < 10").len(),
        7
    );
    assert!(rows("{ status = error } | count() >= 2 | count() < 7").is_empty());
    // A fractional bound compares as a number.
    assert_eq!(rows("{ status = error } | count() > 6.5").len(), 7);
    // `count() = 0` never keeps anything: there is nothing to keep.
    assert!(rows(r#"{ name = "nothing" } | count() = 0"#).is_empty());
}

#[test]
fn count_applies_to_the_whole_spanset_expression() {
    // `{ kind = server } >> { status = error }` gives six spans.
    let query = "{ kind = server } >> { status = error }";
    assert_eq!(rows(query), nums(&[3, 4, 9, 10, 12, 13]));
    assert_eq!(rows(&format!("{query} | count() = 6")).len(), 6);
    assert!(rows(&format!("{query} | count() > 6")).is_empty());
}

#[test]
fn spanset_and_or() {
    // `&&`: both sides must match something in the trace; the answer is the
    // union of both sides.
    assert_eq!(
        rows("{ status = error } && { kind = producer }"),
        nums(&[1, 3, 4, 9, 10, 12, 13, 14])
    );
    assert!(rows(r#"{ status = error } && { name = "nothing" }"#).is_empty());
    assert!(rows(r#"{ name = "nothing" } && { status = error }"#).is_empty());
    // `||`: the union, even when one side is empty.
    assert_eq!(
        rows(r#"{ name = "validate_token" } || { name = "MGET" }"#),
        nums(&[2, 8])
    );
    assert_eq!(
        rows(r#"{ name = "nothing" } || { name = "MGET" }"#),
        nums(&[8])
    );
    assert!(rows(r#"{ name = "nothing" } || { name = "nowhere" }"#).is_empty());
    // A span on both sides appears once.
    assert_eq!(
        rows(r#"{ name = "MGET" } || { kind = client }"#),
        nums(&[3, 5, 6, 8, 9, 11, 12, 13])
    );
    // Parentheses, as in the plan's parse example.
    assert_eq!(
        rows("({ kind = server } && { kind = client }) || { status = error }"),
        nums(&[1, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13])
    );
}

#[test]
fn empty_filter_matches_all() {
    assert_eq!(rows("{ }"), all());
    assert_eq!(rows("{ true }"), all());
    assert!(rows("{ false }").is_empty());
    assert_eq!(rows("{ } | count() = 15"), all());
    assert!(rows("{ } | count() > 15").is_empty());
}

#[test]
fn matches_span_agrees_with_evaluate() {
    let spans = cf();
    // Every query of the tests above that does not need the whole trace.
    let single_span_queries = [
        "{ }",
        "{ false }",
        r#"{ resource.service.name = "payments-api" && status = error }"#,
        "{ duration > 1s }",
        r#"{ name =~ "POST" }"#,
        r#"{ name =~ "POST.*" }"#,
        r#"{ span.http.response.status_code = "500" }"#,
        r#"{ span.http.response.status_code = 500.0 }"#,
        r#"{ span.no.such.attr != "x" }"#,
        r#"{ span.order.id != nil }"#,
        r#"{ span.order.id = nil }"#,
        r#"{ span:name = "POST /charges" }"#,
        r#"{ .service.name = "web-bff" || .db.system.name = "redis" }"#,
        r#"{ !(status = error) }"#,
        r#"{ kind = server && !(status = error) }"#,
        r#"{ name !~ "POST.*" }"#,
    ];
    let view = TraceView::new(&spans);
    for query in single_span_queries {
        let parsed = traceql::parse(query).unwrap();
        assert!(!parsed.needs_whole_trace(), "{query}");
        let expected = parsed.evaluate(&view);
        for (i, span) in spans.iter().enumerate() {
            assert_eq!(
                parsed.matches_span(span),
                Some(expected.contains(&i)),
                "{query} on span #{}",
                number(span)
            );
            // And it agrees with `evaluate` on a view of that span alone.
            let alone = parsed.evaluate(&TraceView::new(std::slice::from_ref(span)));
            assert_eq!(
                parsed.matches_span(span),
                Some(!alone.is_empty()),
                "{query} on span #{} alone",
                number(span)
            );
        }
    }
}

#[test]
fn matches_span_is_none_when_the_whole_trace_is_needed() {
    let span = &cf()[0];
    for query in [
        "{ .a = 1 } >> { .b = 2 }",
        r#"{ rootName = "x" }"#,
        r#"{ rootServiceName = "web-bff" && traceDuration > 1s }"#,
        "{ } | count() > 1",
        "{ status = error } && { kind = client }",
        "{ status = error } || { kind = client }",
        "{ trace:duration > 1s }",
    ] {
        let parsed = traceql::parse(query).unwrap();
        assert!(parsed.needs_whole_trace(), "{query}");
        assert_eq!(parsed.matches_span(span), None, "{query}");
    }
}

// ---- attributes and scopes --------------------------------------------------

#[test]
fn unscoped_attributes_read_the_span_then_the_resource() {
    // From the fixture: `service.name` is only on the resource, `http.route`
    // only on a span.
    assert_eq!(rows(r#"{ .service.name = "web-bff" }"#), nums(&[1, 2, 3]));
    assert_eq!(rows(r#"{ .http.route = "/api/checkout" }"#), nums(&[1]));
    assert_eq!(
        rows(r#"{ .server.address = "card-gateway" }"#),
        nums(&[12, 13])
    );
    assert_eq!(rows(r#"{ .db.system.name = "redis" }"#), nums(&[8]));

    // With both present, the span's value wins, and the resource's is only a
    // fallback.
    let span = with_resource_attr(
        with_attr(mk(1, None, "a", 0, 1), "x", AttrValue::Str("span".into())),
        "x",
        AttrValue::Str("resource".into()),
    );
    assert!(hit(r#"{ .x = "span" }"#, &span));
    assert!(!hit(r#"{ .x = "resource" }"#, &span));
    assert!(hit(r#"{ span.x = "span" }"#, &span));
    assert!(hit(r#"{ resource.x = "resource" }"#, &span));
    assert!(!hit(r#"{ span.x = "resource" }"#, &span));
    assert!(!hit(r#"{ resource.x = "span" }"#, &span));
    // A span attribute of another type does not fall through to the
    // resource: the span has the attribute.
    let span = with_resource_attr(
        with_attr(mk(1, None, "a", 0, 1), "x", AttrValue::Int(1)),
        "x",
        AttrValue::Str("resource".into()),
    );
    assert!(!hit(r#"{ .x = "resource" }"#, &span));
    assert!(hit("{ .x = 1 }", &span));
}

#[test]
fn scopes_do_not_mix() {
    assert!(rows(r#"{ span.service.name = "web-bff" }"#).is_empty());
    assert!(rows(r#"{ resource.http.route = "/api/checkout" }"#).is_empty());
    assert_eq!(
        rows(r#"{ resource.deployment.environment = "local" }"#),
        all()
    );
    assert_eq!(rows(r#"{ resource.service.version = "1.4.0" }"#), all());
}

#[test]
fn quoted_attribute_names() {
    let span = with_attr(
        mk(1, None, "a", 0, 1),
        "http status",
        AttrValue::Str("ok".into()),
    );
    assert!(hit(r#"{ span."http status" = "ok" }"#, &span));
    assert!(!hit(r#"{ span."http status" = "bad" }"#, &span));
}

#[test]
fn nil_tests_presence() {
    assert_eq!(rows("{ span.order.id != nil }"), nums(&[10]));
    assert_eq!(
        rows("{ span.order.id = nil }"),
        all().difference(&nums(&[10])).copied().collect()
    );
    // `nil` on the left means the same.
    assert_eq!(rows("{ nil != span.order.id }"), nums(&[10]));
    // Intrinsics are always there.
    assert_eq!(rows("{ name != nil }"), all());
    assert!(rows("{ name = nil }").is_empty());
}

// ---- intrinsics -------------------------------------------------------------

#[test]
fn kind_and_status_compare_to_their_keywords() {
    assert_eq!(rows("{ kind = server }"), nums(&[1, 4, 7, 10]));
    assert_eq!(
        rows("{ kind = client }"),
        nums(&[3, 5, 6, 8, 9, 11, 12, 13])
    );
    assert_eq!(rows("{ kind = internal }"), nums(&[2]));
    assert_eq!(rows("{ kind = producer }"), nums(&[14]));
    assert_eq!(rows("{ kind = consumer }"), nums(&[15]));
    assert!(rows("{ kind = unspecified }").is_empty());
    assert_eq!(rows("{ kind != server }").len(), 11);
    assert_eq!(rows("{ status = error }"), nums(&[1, 3, 4, 9, 10, 12, 13]));
    assert_eq!(
        rows("{ status != error }"),
        nums(&[2, 5, 6, 7, 8, 11, 14, 15])
    );
    assert_eq!(
        rows("{ status = unset }"),
        nums(&[2, 5, 6, 7, 8, 11, 14, 15])
    );
    assert!(rows("{ status = ok }").is_empty());
}

#[test]
fn status_and_kind_have_no_order() {
    assert!(rows("{ status > ok }").is_empty());
    assert!(rows("{ status <= error }").is_empty());
    assert!(rows("{ kind > internal }").is_empty());
    assert!(rows("{ kind >= server }").is_empty());
}

#[test]
fn status_message_compares_as_a_string() {
    assert_eq!(rows(r#"{ statusMessage = "timeout" }"#), nums(&[12, 13]));
    assert_eq!(
        rows(r#"{ statusMessage =~ "HTTP.*" }"#),
        nums(&[1, 3, 4, 9])
    );
    assert_eq!(rows(r#"{ statusMessage = "" }"#).len(), 8);
    assert_eq!(rows(r#"{ statusMessage > "timeout" }"#), nums(&[]));
    // Strings compare byte by byte: "HTTP 500" sorts before "I", and so does
    // the empty message.
    assert_eq!(
        rows(r#"{ statusMessage < "I" && statusMessage != "" }"#),
        nums(&[1, 3, 4, 9])
    );
}

#[test]
fn name_compares_lexically() {
    assert_eq!(
        rows(r#"{ name >= "POST" && name < "POSU" }"#),
        nums(&[1, 3, 4, 9, 10, 12, 13])
    );
    assert_eq!(rows(r#"{ name < "H" }"#), nums(&[6, 7]));
    assert_eq!(rows(r#"{ name = "MGET" }"#), nums(&[8]));
    assert_eq!(rows("{ name = `POST /charges` }"), nums(&[10]));
}

#[test]
fn duration_compares_with_another_duration_field() {
    // Span #1 spans the whole trace.
    assert_eq!(rows("{ duration = traceDuration }"), nums(&[1]));
    assert_eq!(rows("{ duration < traceDuration }").len(), 14);
    assert_eq!(rows("{ 1s < duration }"), nums(&[1, 3, 4, 9, 10]));
}

// ---- typing -----------------------------------------------------------------

#[test]
fn numbers_compare_numerically() {
    assert_eq!(rows("{ .http.response.status_code >= 500 }"), nums(&[1, 4]));
    assert_eq!(rows("{ .http.response.status_code = 500 }"), nums(&[1, 4]));
    assert_eq!(
        rows("{ .http.response.status_code > 499.5 }"),
        nums(&[1, 4])
    );
    assert!(
        rows("{ .http.response.status_code < 500.5 && .http.response.status_code > 500.1 }")
            .is_empty()
    );
    assert_eq!(rows("{ .http.request.resend_count >= 1 }"), nums(&[13]));
    assert_eq!(rows("{ span.order.id = 8812 }"), nums(&[10]));
    assert!(rows("{ span.order.id = 8812.5 }").is_empty());
    assert!(rows("{ span.order.id > 8812 }").is_empty());
    assert_eq!(rows("{ span.order.id <= 8812 }"), nums(&[10]));
}

#[test]
fn typed_edge_cases_on_hand_made_spans() {
    let span = mk(1, None, "typed", 0, 1000);
    let span = with_attr(span, "n", AttrValue::Int(5));
    let span = with_attr(span, "f", AttrValue::Double(2.5));
    let span = with_attr(span, "b", AttrValue::Bool(true));
    let span = with_attr(span, "s", AttrValue::Str("abc".into()));
    let span = with_attr(span, "bytes", AttrValue::Bytes(vec![1, 2]));
    let span = with_attr(
        span,
        "arr",
        AttrValue::Array(vec![AttrValue::Str("x".into())]),
    );
    let span = with_attr(span, "map", AttrValue::Map(vec![]));
    let mut span = span;
    span.status = StatusCode::Error;
    span.kind = SpanKind::Server;

    let yes: &[&str] = &[
        // ints and floats compare numerically, in both directions
        "{ span.n = 5 }",
        "{ span.n = 5.0 }",
        "{ span.n > 4.5 }",
        "{ span.n < 5.5 }",
        "{ span.n != 5.5 }",
        "{ span.f = 2.5 }",
        "{ span.f > 2 }",
        "{ span.f < 3 }",
        "{ span.f != 2 }",
        "{ span.n > span.f }",
        "{ 2 = 2.0 }",
        "{ 1 < 2 }",
        // strings
        r#"{ span.s = "abc" }"#,
        r#"{ span.s != "abd" }"#,
        r#"{ span.s > "abb" }"#,
        r#"{ span.s < "abd" }"#,
        r#"{ span.s >= "abc" && span.s <= "abc" }"#,
        r#"{ "a" < "b" }"#,
        // booleans
        "{ span.b = true }",
        "{ span.b != false }",
        "{ span.b }",
        "{ true = true }",
        // durations
        "{ duration = 1s }",
        "{ duration > 999ms && duration < 1001ms }",
        "{ 2s > duration }",
        // regex
        r#"{ span.s =~ "a.c" }"#,
        r#"{ span.s =~ "a|abc" }"#,
        r#"{ span.s !~ "a" }"#,
        r#"{ span.s !~ "b.*" }"#,
        r#"{ "test" =~ "te.t" }"#,
        // presence of values that cannot be compared
        "{ span.bytes != nil }",
        "{ span.arr != nil }",
        "{ span.map != nil }",
        // status and kind against their own keywords
        "{ status = error }",
        "{ kind = server }",
        // `!` negates a comparison that is false on a missing attribute
        r#"{ !(span.nope = "x") }"#,
        r#"{ !(span.n = 6) }"#,
        // && and || share one level, left to right (Tempo's %left AND OR):
        // (true || false) && true
        "{ span.b || span.nope && true }",
    ];
    for query in yes {
        assert!(hit(query, &span), "should match: {query}");
    }

    let no: &[&str] = &[
        // a different type is false, never an error, whatever the operator
        r#"{ span.n = "5" }"#,
        r#"{ span.n != "5" }"#,
        r#"{ span.s = 5 }"#,
        r#"{ span.s != 5 }"#,
        "{ span.b = 1 }",
        "{ span.b != 1 }",
        "{ span.n = true }",
        "{ span.f = true }",
        r#"{ span.b = "true" }"#,
        // booleans have equality only
        "{ span.b > false }",
        "{ span.b >= true }",
        "{ span.b < true }",
        // a duration compares with durations only
        "{ span.n = 1s }",
        "{ span.f < 1s }",
        r#"{ span.s = 1s }"#,
        "{ duration = 1000 }",
        "{ duration = 1.0 }",
        // status and kind do not compare with strings or numbers
        r#"{ status = "error" }"#,
        r#"{ status != "error" }"#,
        "{ status = 2 }",
        r#"{ kind = "server" }"#,
        "{ kind = 2 }",
        "{ status = server }",
        "{ kind = error }",
        // a regex needs a string on the left
        r#"{ span.n =~ "5" }"#,
        r#"{ span.n !~ "5" }"#,
        r#"{ span.b =~ "true" }"#,
        r#"{ duration =~ ".*" }"#,
        r#"{ status =~ ".*" }"#,
        r#"{ span.s =~ "a" }"#,
        r#"{ span.s !~ "a.*" }"#,
        // arrays, bytes and maps are present but compare as nothing
        r#"{ span.arr = "x" }"#,
        r#"{ span.arr != "x" }"#,
        "{ span.arr = nil }",
        r#"{ span.bytes = "01 02" }"#,
        "{ span.map = nil }",
        // missing attributes: every comparison is false, `!=` and `!~` too
        r#"{ span.nope = "x" }"#,
        r#"{ span.nope != "x" }"#,
        "{ span.nope = 1 }",
        "{ span.nope != 1 }",
        "{ span.nope > 1 }",
        "{ span.nope < 1 }",
        r#"{ span.nope =~ ".*" }"#,
        r#"{ span.nope !~ "x" }"#,
        "{ span.nope = span.nope2 }",
        "{ span.nope != nil }",
        // a lone attribute is a condition only when it is the boolean true
        "{ span.nope }",
        "{ span.s }",
        "{ span.n }",
        "{ !span.b }",
        // `!` of a bare attribute that is not a boolean is not true either
        "{ !span.nope }",
        "{ !span.s }",
        "{ !!span.nope }",
        // && with a missing attribute
        "{ span.nope && span.b }",
        "{ span.b && span.nope }",
        // (false || true) && false, since `||` and `&&` share one level
        r#"{ span.n = 1 || span.n = 5 && span.s = "zzz" }"#,
        "{ false }",
        "{ !true }",
        "{ 1 = 2 }",
    ];
    for query in no {
        assert!(!hit(query, &span), "should not match: {query}");
    }

    // Unknown mixed with a definite answer, three-valued: true wins an `||`,
    // false wins an `&&`.
    assert!(hit("{ span.nope || span.b }", &span));
    assert!(hit("{ span.b || span.nope }", &span));
    assert!(!hit("{ span.nope && false }", &span));
    assert!(hit("{ !(span.nope && false) }", &span));
    assert!(hit("{ !!span.b }", &span));
}

#[test]
fn nan_attribute_follows_ieee() {
    let span = with_attr(mk(1, None, "a", 0, 1), "x", AttrValue::Double(f64::NAN));
    assert!(!hit("{ span.x = 1.0 }", &span));
    assert!(!hit("{ span.x > 1.0 }", &span));
    assert!(!hit("{ span.x < 1.0 }", &span));
    assert!(hit("{ span.x != 1.0 }", &span));
    assert!(hit("{ span.x != nil }", &span));
}

#[test]
fn large_ints_compare_exactly() {
    let big = i64::MAX;
    let span = with_attr(mk(1, None, "a", 0, 1), "x", AttrValue::Int(big));
    assert!(hit(&format!("{{ span.x = {big} }}"), &span));
    assert!(!hit(&format!("{{ span.x = {} }}", big - 1), &span));
    assert!(hit(&format!("{{ span.x > {} }}", big - 1), &span));
}

#[test]
fn regex_is_anchored_on_every_use() {
    let spans = cf();
    // `=~` must match the whole value, `!~` is its exact opposite.
    assert!(matched(r#"{ name =~ "POS" }"#, &spans).is_empty());
    assert!(matched(r#"{ name =~ "OST" }"#, &spans).is_empty());
    assert_eq!(matched(r#"{ name !~ "POST" }"#, &spans).len(), 11);
    assert_eq!(rows(r#"{ name =~ "POST|MGET" }"#), nums(&[3, 8, 9, 12, 13]));
    assert_eq!(rows(r#"{ name =~ "a|POST" }"#), nums(&[3, 9, 12, 13]));
    assert_eq!(
        rows(r#"{ resource.service.name =~ "web-.*|orders-api" }"#),
        nums(&[1, 2, 3, 4, 5, 6, 9, 14])
    );
    // An alternation inside the source is not split by the anchors.
    assert_eq!(
        rows(r#"{ kind = server && name =~ "GET.*|POST" }"#),
        nums(&[7])
    );
}

// ---- structural operators ---------------------------------------------------

#[test]
fn parent_operator() {
    // B spans that are the parent of some span in A.
    assert_eq!(
        rows(r#"{ name = "SELECT payment_methods" } < { kind = server }"#),
        nums(&[10])
    );
    assert_eq!(
        rows(r#"{ resource.service.name = "payments-api" } < { kind = client }"#),
        nums(&[9])
    );
    // A leaf is nobody's parent.
    assert!(rows(r#"{ name = "MGET" } < { name = "SELECT carts" }"#).is_empty());
}

#[test]
fn ancestor_operator() {
    assert_eq!(
        rows(r#"{ .db.system.name = "redis" } << { kind = server }"#),
        nums(&[1, 4, 7])
    );
    // Strict: a span is not its own ancestor.
    assert!(rows(r#"{ name = "MGET" } << { name = "MGET" }"#).is_empty());
    assert_eq!(rows(r#"{ name = "MGET" } << { }"#), nums(&[1, 3, 4, 6, 7]));
}

#[test]
fn descendant_operator_is_strict() {
    assert!(rows(r#"{ name = "MGET" } >> { name = "MGET" }"#).is_empty());
    assert_eq!(rows(r#"{ name = "GET /stock/{sku}" } >> { }"#), nums(&[8]));
    assert_eq!(
        rows(r#"{ name = "POST /api/checkout" } >> { }"#),
        all().difference(&nums(&[1])).copied().collect()
    );
    // Only descendants, not cousins.
    assert_eq!(
        rows(r#"{ name = "GET" && kind = client } >> { kind = server }"#),
        nums(&[7])
    );
}

#[test]
fn child_operator_is_direct_only() {
    assert_eq!(
        rows(r#"{ name = "POST /orders" } > { }"#),
        nums(&[5, 6, 9, 14])
    );
    assert!(rows(r#"{ name = "POST /api/checkout" } > { name = "MGET" }"#).is_empty());
    assert_eq!(
        rows(r#"{ kind = server } > { kind = client }"#),
        nums(&[3, 5, 6, 8, 9, 11, 12, 13])
    );
}

#[test]
fn not_child_operator() {
    // Payments spans whose parent is not a server: only #10 (under a client).
    assert_eq!(
        rows(r#"{ kind = server } !> { resource.service.name = "payments-api" }"#),
        nums(&[10])
    );
    // A span with no parent is not a child of anything.
    assert!(rows(r#"{ } !> { name = "POST /api/checkout" }"#).contains(&1));
}

#[test]
fn not_parent_operator() {
    assert_eq!(
        rows(r#"{ status = error } !< { kind = client }"#),
        nums(&[5, 6, 8, 11, 12, 13])
    );
}

#[test]
fn not_ancestor_operator() {
    assert_eq!(
        rows(r#"{ name = "MGET" } !<< { kind = server }"#),
        nums(&[10])
    );
}

#[test]
fn not_sibling_operator() {
    assert_eq!(
        rows(r#"{ name = "SELECT carts" } !~ { kind = client }"#),
        nums(&[3, 5, 8, 11, 12, 13])
    );
}

#[test]
fn sibling_is_never_the_span_itself() {
    // #12 and #13 are each other's only POST sibling; #3 and #9 are POSTs
    // too, but each has no other POST under its parent.
    assert_eq!(
        rows(r#"{ name = "POST" } ~ { name = "POST" }"#),
        nums(&[12, 13])
    );
    // A lone span in A is not its own sibling.
    assert_eq!(rows(r#"{ name = "validate_token" } ~ { }"#), nums(&[3]));
    assert!(rows(r#"{ name = "validate_token" } ~ { name = "validate_token" }"#).is_empty());
}

#[test]
fn structural_operators_with_an_empty_left_side() {
    // Nothing on the left: the positive forms match nothing; the negated forms
    // find every right-hand span lacking the relationship, which is all of
    // them (Tempo does the same).
    let empty = r#"{ name = "nothing" }"#;
    for op in [">", "<", ">>", "<<", "~"] {
        assert!(
            rows(&format!("{empty} {op} {{ kind = server }}")).is_empty(),
            "{op}"
        );
    }
    for op in ["!>", "!<", "!>>", "!<<", "!~"] {
        assert_eq!(
            rows(&format!("{empty} {op} {{ kind = server }}")),
            nums(&[1, 4, 7, 10]),
            "{op}"
        );
    }
}

#[test]
fn structural_operators_with_an_empty_right_side() {
    for op in [">", "<", ">>", "<<", "~", "!>", "!<", "!>>", "!<<", "!~"] {
        assert!(
            rows(&format!(r#"{{ }} {op} {{ name = "nothing" }}"#)).is_empty(),
            "{op}"
        );
    }
}

#[test]
fn structural_operators_chain_left_to_right() {
    // `A ~ B > C` is `(A ~ B) > C`. The clients beside #5 are #6 and #9, and
    // their children are #7 and #10.
    assert_eq!(
        rows(r#"{ name = "SELECT carts" } ~ { kind = client } > { }"#),
        nums(&[7, 10])
    );
    // Parentheses group the other way, `A ~ (B > C)`: nothing beside #5 is a
    // child of a client.
    assert!(rows(r#"{ name = "SELECT carts" } ~ ({ kind = client } > { })"#).is_empty());
    // An empty result on the left of the next operator finds nothing.
    assert!(rows(r#"{ name = "SELECT carts" } >> { } ~ { kind = client }"#).is_empty());
}

#[test]
fn spanset_and_binds_looser_than_structural_operators() {
    // `A && B >> C` is `A && (B >> C)`: the union of A (#8) and the descendants
    // of #4 named SELECT carts (#5). Grouped the other way it would be only #5.
    assert_eq!(
        rows(r#"{ name = "MGET" } && { name = "POST /orders" } >> { name = "SELECT carts" }"#),
        nums(&[5, 8])
    );
    // `A >> B && C` is `(A >> B) && C`: #5 together with #2. Grouped the other
    // way it would be only #5.
    assert_eq!(
        rows(
            r#"{ name = "POST /orders" } >> { name = "SELECT carts" } && { name = "validate_token" }"#
        ),
        nums(&[2, 5])
    );
}

#[test]
fn results_are_ascending_and_unique() {
    let spans = cf();
    for query in [
        "{ }",
        "{ kind = client } || { status = error }",
        "{ } >> { }",
        "{ status = error } && { kind = client }",
        r#"{ name = "POST" } ~ { }"#,
        "{ } !> { }",
    ] {
        let indices = matched_indices(query, &spans);
        let mut sorted = indices.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(indices, sorted, "{query}");
    }
}

// ---- edge cases of the trace shape ------------------------------------------

#[test]
fn orphans_are_subtree_roots() {
    // R <- C, and two orphans (their parents never arrived) with a child each.
    //
    //   R(1) <- C(2)        O(3) <- OC(4)        P(5) <- PC(6)
    //
    // O's parent is ID 90; P's parent is ID 90 too (the same missing span).
    let spans = vec![
        mk(1, None, "R", 0, 100),
        mk(2, Some(1), "C", 10, 10),
        mk(3, Some(90), "O", 20, 50),
        mk(4, Some(3), "OC", 30, 10),
        mk(5, Some(90), "P", 40, 50),
        mk(6, Some(5), "PC", 50, 10),
    ];
    let view = TraceView::new(&spans);

    // An orphan has no parent in the view, but its own children do.
    assert_eq!(view.parent(0), None);
    assert_eq!(view.parent(1), Some(0));
    assert_eq!(view.parent(2), None);
    assert_eq!(view.parent(3), Some(2));
    assert_eq!(view.parent(4), None);
    assert_eq!(view.parent(5), Some(4));

    // The root is the earliest-starting span that declares no parent: R.
    // An orphan declares one, so it is not a root of the trace.
    assert_eq!(view.root(), Some(0));

    assert!(view.is_ancestor(2, 3));
    assert!(!view.is_ancestor(0, 3));
    assert!(!view.is_ancestor(0, 5));
    assert!(!view.is_ancestor(2, 5));

    let find = |q: &str| -> Vec<&str> {
        matched_indices(q, &spans)
            .into_iter()
            .map(|i| spans[i].name.as_str())
            .collect()
    };
    // Ancestry stays inside each subtree.
    assert_eq!(find(r#"{ name = "O" } >> { }"#), ["OC"]);
    assert_eq!(find(r#"{ name = "R" } >> { }"#), ["C"]);
    assert!(find(r#"{ name = "R" } >> { name = "OC" }"#).is_empty());
    assert!(find(r#"{ name = "R" } > { name = "O" }"#).is_empty());
    assert_eq!(find(r#"{ } << { name = "O" }"#), ["O"]);
    assert_eq!(find(r#"{ name = "OC" } < { }"#), ["O"]);
    // A missing parent is not a parent: orphans have no siblings, not even
    // each other (O and P name the same missing span 90 as their parent) or
    // the root.
    assert!(find(r#"{ name = "O" } ~ { name = "P" }"#).is_empty());
    assert!(find(r#"{ name = "R" } ~ { name = "O" }"#).is_empty());
    assert_eq!(find(r#"{ name = "O" } !~ { name = "P" }"#), ["P"]);
    // `!>` finds orphans (no parent in the trace) along with the rest.
    assert_eq!(find(r#"{ name = "R" } !> { name = "O" }"#), ["O"]);
    // The trace-level fields come from the root R.
    assert_eq!(find(r#"{ rootName = "R" && name = "PC" }"#), ["PC"]);
    assert_eq!(find("{ traceDuration = 100ms && name = \"OC\" }"), ["OC"]);
}

#[test]
fn trace_without_a_root_has_no_root_fields() {
    // Only an orphan arrived so far (its parent has not).
    let spans = vec![mk(2, Some(1), "late child", 5, 10)];
    let view = TraceView::new(&spans);
    assert_eq!(view.root(), None);
    assert!(!hits(r#"{ rootName = "x" }"#, &spans));
    assert!(!hits(r#"{ rootName != "x" }"#, &spans));
    assert!(!hits(r#"{ rootName =~ ".*" }"#, &spans));
    assert!(!hits(r#"{ rootServiceName != "x" }"#, &spans));
    // They read as missing: `= nil` is true, `!= nil` false.
    assert!(hits("{ rootName = nil }", &spans));
    assert!(!hits("{ rootName != nil }", &spans));
    assert!(hits("{ rootServiceName = nil }", &spans));
    // The trace's duration needs no root: it is the earliest start to the
    // latest end.
    assert!(hits("{ traceDuration = 10ms }", &spans));
}

#[test]
fn root_is_the_earliest_parentless_span_and_ties_go_to_the_lowest_id() {
    let spans = vec![
        mk(5, None, "late root", 50, 10),
        mk(3, None, "early root b", 10, 10),
        mk(2, None, "early root a", 10, 10),
        mk(4, Some(2), "child", 20, 10),
    ];
    let view = TraceView::new(&spans);
    assert_eq!(view.root(), Some(2)); // ID 2 starts with ID 3, and is lower
    assert_eq!(view.spans().len(), 4);
    // The root's name and service are the trace's.
    let spans: Vec<Span> = spans
        .into_iter()
        .map(|span| in_service(span, "svc"))
        .collect();
    assert!(hits(
        r#"{ rootName = "early root a" && rootServiceName = "svc" }"#,
        &spans
    ));
    assert!(!hits(r#"{ rootName = "late root" }"#, &spans));
    // Several roots are not siblings: they have no parent.
    assert!(!hits(
        r#"{ name = "late root" } ~ { name = "early root a" }"#,
        &spans
    ));
}

#[test]
fn trace_duration_runs_from_the_earliest_start_to_the_latest_end() {
    let spans = vec![
        mk(1, None, "a", 0, 10),
        mk(2, Some(1), "b", 5, 95), // ends at 100
        mk(3, Some(1), "c", 2, 3),
    ];
    assert!(hits("{ traceDuration = 100ms }", &spans));
    assert!(!hits("{ traceDuration = 99ms }", &spans));
    assert!(hits("{ trace:duration >= 100ms }", &spans));

    // A span that ends before it starts (clock skew) does not make a negative
    // or a huge duration: the trace's duration is clamped at 0.
    let mut skewed = mk(1, None, "a", 100, 0);
    skewed.end_unix_nano = skewed.start_unix_nano - 50_000_000;
    assert_eq!(skewed.duration_nanos(), 0);
    assert!(hit("{ traceDuration = 0ms }", &skewed));
    assert!(hit("{ duration = 0ms }", &skewed));
}

#[test]
fn duplicate_span_ids_do_not_break_the_view() {
    // Two spans with one ID (the store would drop the second; a view of an
    // arbitrary slice still must not panic or loop). Children attach to the
    // first one.
    let spans = vec![
        mk(1, None, "root", 0, 100),
        mk(2, Some(1), "twin a", 10, 10),
        mk(2, Some(1), "twin b", 20, 10),
        mk(3, Some(2), "grandchild", 30, 10),
    ];
    let view = TraceView::new(&spans);
    assert_eq!(view.parent(3), Some(1));
    assert!(view.is_ancestor(1, 3));
    assert!(!view.is_ancestor(2, 3));
    assert_eq!(matched_indices("{ }", &spans), [0, 1, 2, 3]);
    assert_eq!(
        matched_indices(r#"{ name = "root" } >> { name = "grandchild" }"#, &spans),
        [3]
    );
}

#[test]
fn parent_cycles_do_not_hang_or_panic() {
    // A <- B <- A: nothing is a root. Bad data, but never a hang.
    let spans = vec![
        mk(1, Some(2), "a", 0, 10),
        mk(2, Some(1), "b", 1, 10),
        mk(3, Some(2), "c", 2, 1),
    ];
    let view = TraceView::new(&spans);
    assert_eq!(view.root(), None);
    assert_eq!(matched_indices("{ }", &spans), [0, 1, 2]);
    // The cycle is cut somewhere, so exactly one of the two is the other's
    // ancestor, and ancestry agrees with the parent links.
    assert_ne!(view.is_ancestor(0, 1), view.is_ancestor(1, 0));
    assert!(!view.is_ancestor(0, 0));
    for a in 0..3 {
        for d in 0..3 {
            let mut walk = view.parent(d);
            let mut found = false;
            while let Some(p) = walk {
                if p == a {
                    found = true;
                    break;
                }
                walk = view.parent(p);
            }
            assert_eq!(view.is_ancestor(a, d), found, "{a} ancestor of {d}");
        }
    }
    // The evaluator terminates on every operator.
    for op in [">", "<", ">>", "<<", "~", "!>", "!<", "!>>", "!<<", "!~"] {
        let _ = matched_indices(&format!("{{ }} {op} {{ }}"), &spans);
    }

    // A span that is its own parent is a root of its own subtree.
    let spans = vec![mk(1, Some(1), "self", 0, 1), mk(2, Some(1), "child", 0, 1)];
    let view = TraceView::new(&spans);
    assert_eq!(view.parent(0), None);
    assert_eq!(view.parent(1), Some(0));
    assert!(view.is_ancestor(0, 1));
    assert_eq!(
        matched_indices(r#"{ name = "self" } > { name = "child" }"#, &spans),
        [1]
    );
}

#[test]
fn an_empty_trace_matches_nothing() {
    let spans: Vec<Span> = Vec::new();
    let view = TraceView::new(&spans);
    assert_eq!(view.root(), None);
    assert!(view.spans().is_empty());
    for query in [
        "{ }",
        "{ } | count() = 0",
        "{ } >> { }",
        "{ } !> { }",
        "{ } && { }",
        "{ } || { }",
        "{ rootName = nil }",
    ] {
        assert!(matched_indices(query, &spans).is_empty(), "{query}");
    }
}

#[test]
fn view_api_is_safe_for_indices_out_of_range() {
    let spans = vec![mk(1, None, "a", 0, 1)];
    let view = TraceView::new(&spans);
    assert_eq!(view.parent(5), None);
    assert!(!view.is_ancestor(0, 5));
    assert!(!view.is_ancestor(5, 0));
    assert!(!view.is_ancestor(0, 0));
}

#[test]
fn a_very_deep_trace_does_not_overflow_the_stack() {
    // A chain of 100,000 spans: every span is the child of the one before.
    let spans: Vec<Span> = (0..100_000u32)
        .map(|i| {
            let mut span = mk(1, None, "n", u64::from(i), 1);
            span.span_id = loctel_model::SpanId((u64::from(i) + 1).to_be_bytes());
            span.parent_span_id = (i > 0).then(|| loctel_model::SpanId(u64::from(i).to_be_bytes()));
            span.name = format!("n{i}");
            span
        })
        .collect();
    let view = TraceView::new(&spans);
    assert_eq!(view.root(), Some(0));
    assert!(view.is_ancestor(0, 99_999));
    assert!(view.is_ancestor(500, 501));
    assert!(!view.is_ancestor(501, 500));
    assert_eq!(view.parent(99_999), Some(99_998));
    let parsed = traceql::parse(r#"{ name = "n0" } >> { }"#).unwrap();
    assert_eq!(parsed.evaluate(&view).len(), 99_999);
    let parsed = traceql::parse(r#"{ name = "n99999" } << { }"#).unwrap();
    assert_eq!(parsed.evaluate(&view).len(), 99_999);
    let parsed = traceql::parse(r#"{ name = "n7" } > { }"#).unwrap();
    assert_eq!(parsed.evaluate(&view), [8]);
    let parsed = traceql::parse(r#"{ name = "n7" } !>> { }"#).unwrap();
    assert_eq!(parsed.evaluate(&view).len(), 8); // n0..=n7
}

// ---- the view against brute force -------------------------------------------

/// Ancestry by walking up the parent links.
fn walk_is_ancestor(view: &TraceView, a: usize, d: usize) -> bool {
    let mut walk = view.parent(d);
    while let Some(p) = walk {
        if p == a {
            return true;
        }
        walk = view.parent(p);
    }
    false
}

#[test]
fn is_ancestor_agrees_with_walking_the_parents() {
    for spans in [
        cf(),
        scenario_spans(&Scenario::HugeTrace { spans: 300 }),
        scenario_spans(&Scenario::CheckoutSuccess),
    ] {
        let view = TraceView::new(&spans);
        for a in 0..spans.len() {
            for d in 0..spans.len() {
                assert_eq!(
                    view.is_ancestor(a, d),
                    walk_is_ancestor(&view, a, d),
                    "{a} ancestor of {d}"
                );
            }
        }
    }
}

#[test]
fn parents_follow_the_declared_parent_ids() {
    let spans = cf();
    let view = TraceView::new(&spans);
    for (i, span) in spans.iter().enumerate() {
        let declared = span
            .parent_span_id
            .map(|id| spans.iter().position(|other| other.span_id == id).unwrap());
        assert_eq!(view.parent(i), declared, "#{}", number(span));
    }
    // #1 is the root.
    assert_eq!(number(&spans[view.root().unwrap()]), 1);
    assert_eq!(view.spans().as_ptr(), spans.as_ptr());
}

#[test]
fn structural_operators_agree_with_brute_force_on_a_big_trace() {
    // 2,000 spans, services rotating among four: the sets are big and spread
    // over the tree.
    let spans = scenario_spans(&Scenario::HugeTrace { spans: 2_000 });
    let view = TraceView::new(&spans);
    let a_query = r#"resource.service.name = "orders-api""#;
    let b_query = r#"resource.service.name = "payments-api""#;
    let is_a = |i: usize| spans[i].service() == "orders-api";
    let is_b = |i: usize| spans[i].service() == "payments-api";
    let n = spans.len();

    let expected = |relation: &dyn Fn(usize) -> bool| -> Vec<usize> {
        (0..n).filter(|&i| is_b(i) && relation(i)).collect()
    };
    let has_a_ancestor = |i: usize| (0..n).any(|a| is_a(a) && view.is_ancestor(a, i));
    let has_a_descendant = |i: usize| (0..n).any(|d| is_a(d) && view.is_ancestor(i, d));
    let parent_in_a = |i: usize| view.parent(i).is_some_and(is_a);
    let is_parent_of_a = |i: usize| (0..n).any(|c| is_a(c) && view.parent(c) == Some(i));
    let has_a_sibling = |i: usize| {
        view.parent(i).is_some()
            && (0..n).any(|s| s != i && is_a(s) && view.parent(s) == view.parent(i))
    };

    let cases: [(&str, &dyn Fn(usize) -> bool); 5] = [
        (">>", &has_a_ancestor),
        ("<<", &has_a_descendant),
        (">", &parent_in_a),
        ("<", &is_parent_of_a),
        ("~", &has_a_sibling),
    ];
    for (op, relation) in cases {
        let query = format!("{{ {a_query} }} {op} {{ {b_query} }}");
        assert_eq!(matched_indices(&query, &spans), expected(relation), "{op}");
        let negated = format!("{{ {a_query} }} !{op} {{ {b_query} }}");
        assert_eq!(
            matched_indices(&negated, &spans),
            expected(&|i| !relation(i)),
            "!{op}"
        );
    }
}

#[test]
fn huge_trace_fixture_is_a_real_tree() {
    // Guards the brute-force test above: the scenario really gives a big tree.
    let spans = scenario_spans(&Scenario::HugeTrace { spans: 2_000 });
    assert_eq!(spans.len(), 2_000);
    let view = TraceView::new(&spans);
    assert_eq!(view.root(), Some(0));
    let deepest = (0..spans.len())
        .map(|i| {
            let mut depth = 0;
            let mut walk = view.parent(i);
            while let Some(p) = walk {
                depth += 1;
                walk = view.parent(p);
            }
            depth
        })
        .max()
        .unwrap();
    assert!(deepest >= 2, "{deepest}");
}
