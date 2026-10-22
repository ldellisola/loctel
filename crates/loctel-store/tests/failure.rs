//! Failure semantics (plan Task 4, design 9.4): which trace failed, and in
//! which span.
//!
//! The plan's own tests come first. The rest pin the corners the design
//! leaves to the builder: the tie-break, the peer, hostile parent IDs and the
//! exception fallback.

mod fixtures;
mod modes;

use fixtures::*;
use loctel_model::{Span, SpanKind};
use loctel_store::{TraceStatus, classify, trace_exception};
use modes::each_mode;

/// `root(err) -> payments POST /charges (err, leaf)`, the plan's first case.
fn checkout_with_failing_payment() -> Vec<Span> {
    vec![
        error(span(1, 1, None, "web-bff", "POST /api/checkout", 0, 500)),
        error(span(
            1,
            2,
            Some(1),
            "payments-api",
            "POST /charges",
            100,
            50,
        )),
    ]
}

#[test]
fn root_error_means_failed() {
    each_mode(|store, _c| {
        let spans = checkout_with_failing_payment();
        let (status, failed_in) = classify(&spans, Some(sid(1)));
        assert_eq!(status, TraceStatus::Failed);
        let failed_in = failed_in.unwrap();
        assert_eq!(failed_in.span_id, sid(2));
        assert_eq!(failed_in.service, "payments-api");
        assert_eq!(failed_in.span_name, "POST /charges");
        assert_eq!(failed_in.more, 0);

        // The store reports the same thing on every snapshot.
        store.ingest_spans(spans);
        let t = store.trace(tid(1)).unwrap();
        assert_eq!(t.status, TraceStatus::Failed);
        assert_eq!(t.failed_in, Some(failed_in));
    });
}

#[test]
fn inner_error_with_ok_root_is_errors_inside() {
    each_mode(|store, _c| {
        let spans = vec![
            span(1, 1, None, "web-bff", "POST /api/checkout", 0, 500),
            error(span(
                1,
                2,
                Some(1),
                "payments-api",
                "POST /charges",
                100,
                50,
            )),
            span(1, 3, Some(1), "orders-api", "POST /orders", 200, 50),
        ];
        let (status, failed_in) = classify(&spans, Some(sid(1)));
        assert_eq!(status, TraceStatus::ErrorsInside);
        let failed_in = failed_in.unwrap();
        assert_eq!(failed_in.span_id, sid(2));
        assert_eq!(failed_in.more, 0);

        store.ingest_spans(spans);
        let t = store.trace(tid(1)).unwrap();
        assert_eq!(t.status, TraceStatus::ErrorsInside);
        assert_eq!(t.failed_in, Some(failed_in));
    });
}

#[test]
fn exception_event_alone_is_not_error() {
    each_mode(|store, _c| {
        let spans = vec![
            with_exception(
                span(1, 1, None, "web-bff", "POST /api/checkout", 0, 500),
                "RetryableError",
                "first attempt timed out",
            ),
            with_exception(
                span(1, 2, Some(1), "payments-api", "POST /charges", 100, 50),
                "TimeoutError",
                "slow",
            ),
        ];
        assert_eq!(classify(&spans, Some(sid(1))), (TraceStatus::Ok, None));

        store.ingest_spans(spans);
        let t = store.trace(tid(1)).unwrap();
        assert_eq!(t.status, TraceStatus::Ok);
        assert_eq!(t.failed_in, None);
    });
}

#[test]
fn failed_in_picks_earliest_lowest_error() {
    // root(err) -> payments(err, start 200) ; root -> inventory(start 300) -> db(err, start 310)
    let spans = vec![
        error(span(1, 1, None, "web-bff", "POST /api/checkout", 0, 500)),
        error(span(
            1,
            2,
            Some(1),
            "payments-api",
            "POST /charges",
            200,
            50,
        )),
        span(1, 3, Some(1), "inventory-api", "GET /stock", 300, 100),
        error(span(1, 4, Some(3), "postgres", "SELECT stock", 310, 20)),
    ];
    let (status, f) = classify(&spans, Some(sid(1)));
    assert_eq!(status, TraceStatus::Failed);
    let f = f.unwrap();
    assert_eq!(f.service, "payments-api");
    assert_eq!(f.more, 1);
}

#[test]
fn missing_root_then_root_arrives() {
    each_mode(|store, _c| {
        // Only an erroring child so far: the root has not arrived.
        store.ingest_spans(vec![error(span(
            1,
            2,
            Some(1),
            "payments-api",
            "POST /charges",
            100,
            50,
        ))]);
        let t = store.trace(tid(1)).unwrap();
        assert_eq!(t.root, None);
        assert_eq!(t.status, TraceStatus::ErrorsInside);
        assert_eq!(t.failed_in.unwrap().span_id, sid(2));

        // The root arrives erroring: now the request failed.
        store.ingest_spans(vec![error(span(
            1,
            1,
            None,
            "web-bff",
            "POST /api/checkout",
            0,
            500,
        ))]);
        let t = store.trace(tid(1)).unwrap();
        assert_eq!(t.root, Some(sid(1)));
        assert_eq!(t.status, TraceStatus::Failed);
        assert_eq!(t.failed_in.unwrap().span_id, sid(2));
    });
}

#[test]
fn exception_falls_back_to_nearest_ancestor() {
    // The failed-in client span has no exception; its parent has one, and so
    // does the root, which is further away.
    each_mode(|store, _c| {
        store.ingest_spans(vec![
            error(with_exception(
                span(1, 1, None, "web-bff", "POST /api/checkout", 0, 500),
                "RootError",
                "from the root",
            )),
            error(with_exception(
                span(1, 2, Some(1), "orders-api", "POST /orders", 50, 400),
                "OrdersError",
                "from the parent",
            )),
            error(with_kind(
                span(1, 3, Some(2), "orders-api", "POST payments", 100, 50),
                SpanKind::Client,
            )),
        ]);
        let t = store.trace(tid(1)).unwrap();
        assert_eq!(t.failed_in.as_ref().unwrap().span_id, sid(3));
        let exception = trace_exception(&t).unwrap();
        assert_eq!(exception.type_name, "OrdersError");
        assert_eq!(exception.message, "from the parent");
    });
}

// --- the exception choice, in full ---

#[test]
fn failed_in_span_own_exception_wins_over_ancestors() {
    each_mode(|store, _c| {
        store.ingest_spans(vec![
            error(with_exception(
                span(1, 1, None, "web-bff", "POST /api/checkout", 0, 500),
                "RootError",
                "from the root",
            )),
            error(with_exception(
                span(1, 2, Some(1), "payments-api", "POST /charges", 100, 50),
                "CardDeclined",
                "from the failed-in span",
            )),
        ]);
        let t = store.trace(tid(1)).unwrap();
        assert_eq!(trace_exception(&t).unwrap().type_name, "CardDeclined");
    });
}

#[test]
fn exception_falls_back_to_first_in_start_order() {
    // The failed-in span and its ancestors have no exception. A handled
    // exception on a sibling branch is still the best thing to show, and the
    // earliest one wins.
    each_mode(|store, _c| {
        store.ingest_spans(vec![
            error(span(1, 1, None, "web-bff", "POST /api/checkout", 0, 500)),
            error(span(
                1,
                2,
                Some(1),
                "payments-api",
                "POST /charges",
                100,
                50,
            )),
            with_exception(
                span(1, 4, Some(1), "inventory-api", "GET /stock", 300, 20),
                "Later",
                "second",
            ),
            with_exception(
                span(1, 3, Some(1), "orders-api", "GET /order", 200, 20),
                "Earlier",
                "first",
            ),
        ]);
        let t = store.trace(tid(1)).unwrap();
        assert_eq!(t.failed_in.as_ref().unwrap().span_id, sid(2));
        assert_eq!(trace_exception(&t).unwrap().type_name, "Earlier");
    });
}

#[test]
fn exception_of_a_trace_without_errors_is_the_first_handled_one() {
    each_mode(|store, _c| {
        store.ingest_spans(vec![
            span(1, 1, None, "web-bff", "POST /api/checkout", 0, 500),
            with_exception(
                span(1, 2, Some(1), "payments-api", "POST /charges", 100, 50),
                "Retried",
                "then succeeded",
            ),
        ]);
        let t = store.trace(tid(1)).unwrap();
        assert_eq!(t.status, TraceStatus::Ok);
        assert_eq!(trace_exception(&t).unwrap().type_name, "Retried");
    });
}

#[test]
fn no_exception_anywhere_is_none() {
    each_mode(|store, _c| {
        store.ingest_spans(checkout_with_failing_payment());
        let t = store.trace(tid(1)).unwrap();
        assert_eq!(trace_exception(&t), None);
    });
}

// --- more than one candidate ---

#[test]
fn only_the_lowest_error_of_a_branch_is_a_candidate() {
    // root(err) -> a(err) -> b(err) -> c(err): one chain, one candidate.
    let spans = vec![
        error(span(1, 1, None, "s1", "root", 0, 100)),
        error(span(1, 2, Some(1), "s2", "a", 10, 80)),
        error(span(1, 3, Some(2), "s3", "b", 20, 60)),
        error(span(1, 4, Some(3), "s4", "c", 30, 40)),
    ];
    let (status, f) = classify(&spans, Some(sid(1)));
    assert_eq!(status, TraceStatus::Failed);
    let f = f.unwrap();
    assert_eq!(f.span_id, sid(4));
    assert_eq!(f.more, 0);
}

#[test]
fn more_counts_every_other_candidate() {
    let spans = vec![
        span(1, 1, None, "web-bff", "root", 0, 500),
        error(span(1, 2, Some(1), "a", "first", 100, 10)),
        error(span(1, 3, Some(1), "b", "second", 200, 10)),
        error(span(1, 4, Some(1), "c", "third", 300, 10)),
    ];
    let (_, f) = classify(&spans, Some(sid(1)));
    let f = f.unwrap();
    assert_eq!(f.span_id, sid(2));
    assert_eq!(f.more, 2);
}

#[test]
fn start_tie_goes_to_the_lowest_span_id() {
    let spans = vec![
        span(1, 1, None, "web-bff", "root", 0, 500),
        error(span(1, 9, Some(1), "late-id", "x", 100, 10)),
        error(span(1, 5, Some(1), "low-id", "y", 100, 10)),
    ];
    let (_, f) = classify(&spans, Some(sid(1)));
    assert_eq!(f.unwrap().service, "low-id");
}

#[test]
fn input_order_does_not_change_the_answer() {
    let mut spans = vec![
        error(span(1, 1, None, "web-bff", "root", 0, 500)),
        error(span(
            1,
            2,
            Some(1),
            "payments-api",
            "POST /charges",
            200,
            50,
        )),
        span(1, 3, Some(1), "inventory-api", "GET /stock", 300, 100),
        error(span(1, 4, Some(3), "postgres", "SELECT", 310, 20)),
    ];
    let expected = classify(&spans, Some(sid(1)));
    spans.reverse();
    assert_eq!(classify(&spans, Some(sid(1))), expected);
}

#[test]
fn skewed_child_starting_before_its_parent_is_still_a_descendant() {
    // The child's clock runs ahead, so it starts before the root does. It is
    // the lowest error and the only candidate: parent IDs decide, not times.
    let spans = vec![
        error(span(1, 1, None, "web-bff", "root", 100, 50)),
        error(span(1, 2, Some(1), "payments-api", "skewed", 40, 10)),
    ];
    let (status, f) = classify(&spans, Some(sid(1)));
    assert_eq!(status, TraceStatus::Failed);
    let f = f.unwrap();
    assert_eq!(f.span_id, sid(2));
    assert_eq!(f.more, 0);
}

#[test]
fn erroring_orphan_counts_as_an_error() {
    // The parent never arrived, so the span hangs off nothing, but the root
    // is fine: errors inside.
    let spans = vec![
        span(1, 1, None, "web-bff", "root", 0, 500),
        error(span(1, 3, Some(2), "payments-api", "orphan", 100, 10)),
    ];
    let (status, f) = classify(&spans, Some(sid(1)));
    assert_eq!(status, TraceStatus::ErrorsInside);
    assert_eq!(f.unwrap().span_id, sid(3));
}

// --- peer ---

#[test]
fn peer_comes_from_client_and_producer_spans_only() {
    let client = error(with_attr(
        with_kind(
            span(1, 2, Some(1), "orders-api", "POST", 10, 10),
            SpanKind::Client,
        ),
        "server.address",
        "card-gateway.example",
    ));
    let producer = error(with_attr(
        with_kind(
            span(2, 2, Some(1), "orders-api", "publish", 10, 10),
            SpanKind::Producer,
        ),
        "peer.service",
        "rabbitmq",
    ));
    let server = error(with_attr(
        with_kind(
            span(3, 2, Some(1), "orders-api", "handle", 10, 10),
            SpanKind::Server,
        ),
        "server.address",
        "ignored.example",
    ));
    for (spans, expected) in [
        (vec![client], Some("card-gateway.example")),
        (vec![producer], Some("rabbitmq")),
        (vec![server], None),
    ] {
        let (_, f) = classify(&spans, None);
        assert_eq!(f.unwrap().peer.as_deref(), expected);
    }
}

#[test]
fn client_span_without_address_has_no_peer() {
    let spans = vec![error(with_kind(
        span(1, 2, Some(1), "orders-api", "POST", 10, 10),
        SpanKind::Client,
    ))];
    assert_eq!(classify(&spans, None).1.unwrap().peer, None);
}

// --- degenerate input ---

#[test]
fn empty_trace_is_ok() {
    assert_eq!(classify(&[], None), (TraceStatus::Ok, None));
}

#[test]
fn erroring_root_alone_is_failed_in_itself() {
    let spans = vec![error(span(1, 1, None, "web-bff", "root", 0, 10))];
    let (status, f) = classify(&spans, Some(sid(1)));
    assert_eq!(status, TraceStatus::Failed);
    assert_eq!(f.unwrap().span_id, sid(1));
}

#[test]
fn unknown_root_id_is_not_erroring() {
    // A root ID that matches no span cannot make the trace Failed.
    let spans = vec![error(span(1, 2, Some(1), "a", "x", 0, 10))];
    assert_eq!(classify(&spans, Some(sid(9))).0, TraceStatus::ErrorsInside);
}

#[test]
fn span_that_is_its_own_parent_does_not_hide_itself() {
    // Malformed (ingest rejects it, the store must survive it): the span is
    // not its own descendant, so it is still a lowest error.
    let alone = vec![error(span(1, 1, Some(1), "a", "self-parent", 0, 10))];
    let (status, f) = classify(&alone, None);
    assert_eq!(status, TraceStatus::ErrorsInside);
    assert_eq!(f.unwrap().span_id, sid(1));

    // Next to a well-formed erroring span it counts as a candidate: it is
    // the one named (it starts first) and the other is "+1 more".
    let both = vec![
        error(span(1, 1, Some(1), "a", "self-parent", 0, 10)),
        error(span(1, 2, None, "b", "root", 5, 10)),
    ];
    let (_, f) = classify(&both, Some(sid(2)));
    let f = f.unwrap();
    assert_eq!(f.span_id, sid(1));
    assert_eq!(f.more, 1);
}

#[test]
fn parent_cycle_terminates_and_still_names_a_span() {
    // 1 -> 2 -> 1, both erroring: every erroring span has an erroring
    // descendant, so there is no lowest error. The earliest erroring span
    // is named instead of nothing.
    let spans = vec![
        error(span(1, 1, Some(2), "a", "one", 5, 10)),
        error(span(1, 2, Some(1), "b", "two", 0, 10)),
    ];
    let (status, f) = classify(&spans, None);
    assert_eq!(status, TraceStatus::ErrorsInside);
    let f = f.unwrap();
    assert_eq!(f.span_id, sid(2));
    assert_eq!(f.more, 1);
}

#[test]
fn parent_cycle_does_not_hang_the_exception_walk() {
    each_mode(|store, _c| {
        store.ingest_spans(vec![
            error(span(1, 1, Some(2), "a", "one", 5, 10)),
            error(span(1, 2, Some(1), "b", "two", 0, 10)),
        ]);
        let t = store.trace(tid(1)).unwrap();
        assert_eq!(trace_exception(&t), None);
    });
}

#[test]
fn deep_chain_does_not_overflow_the_stack() {
    // A 20,000-deep chain with the error at the bottom: classification must
    // not recurse.
    let depth: u32 = 20_000;
    let spans: Vec<Span> = (0..depth)
        .map(|i| {
            let mut s = span(1, 1, None, "a", "n", u64::from(i), 1);
            s.span_id = loctel_model::SpanId(u64::from(i + 1).to_be_bytes());
            s.parent_span_id = (i > 0).then(|| loctel_model::SpanId(u64::from(i).to_be_bytes()));
            if i == depth - 1 {
                s.status = loctel_model::StatusCode::Error;
            }
            s
        })
        .collect();
    let root = Some(spans[0].span_id);
    let (status, f) = classify(&spans, root);
    assert_eq!(status, TraceStatus::ErrorsInside);
    assert_eq!(f.unwrap().span_id, spans[depth as usize - 1].span_id);
}

#[test]
fn status_serializes_as_snake_case() {
    assert_eq!(
        serde_json::to_string(&TraceStatus::ErrorsInside).unwrap(),
        "\"errors_inside\""
    );
}
