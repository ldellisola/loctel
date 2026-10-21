//! The ingest filter: what `RuleSet` drops, what it tells the caller to remove,
//! what it remembers and what it counts.

mod fixtures;

use std::time::Duration;

use fixtures::*;
use loctel_meshgen::Scenario;
use loctel_model::Span;
use loctel_query::traceql;
use loctel_rules::{DROPPED_TTL, DropScope, RuleSet};
use loctel_store::Removed;

const BASE: u64 = 2_000_000_000_000;

const HEALTH_QUERY: &str = r#"{ span.url.path = "/health" }"#;

/// A health-check trace of three spans: only its root says `/health`.
fn health_trace(trace: u8) -> Vec<Span> {
    vec![
        with_attr(
            span(trace, 1, None, "web", "GET /health", 0, 5),
            "url.path",
            "/health",
        ),
        span(trace, 2, Some(1), "web", "check db", 1, 2),
        span(trace, 3, Some(1), "web", "check cache", 2, 2),
    ]
}

fn checkout_failure() -> Vec<Span> {
    scenario_spans(&Scenario::CheckoutFailure, 1, BASE)
}

#[test]
fn no_rules_keep_everything_in_order() {
    let w = World::new(vec![]);
    let spans = checkout_failure();

    let verdict = w.ingest_spans(spans.clone());

    assert_eq!(verdict.keep, spans);
    assert!(verdict.drop_traces.is_empty());
    assert!(verdict.drop_spans.is_empty());
    assert_eq!(w.store.stats().spans, 15);
    let logs = scenario_logs(&Scenario::CheckoutFailure, 1, BASE);
    assert_eq!(w.ingest_logs(logs.clone()), logs);
    assert_eq!(w.rules.totals().spans, 0);
    assert_eq!(w.rules.totals().logs, 0);
}

#[test]
fn per_span_rule_drops_matching_trace() {
    let w = World::new(vec![trace_rule("Health", HEALTH_QUERY)]);
    let other = span(2, 10, None, "web", "GET /orders", 0, 5);
    let mut health = health_trace(1);
    // The matching root comes in the middle of the batch: the child before it
    // and the child after it go too.
    let batch = vec![
        health.remove(1),
        other.clone(),
        health.remove(0),
        health.remove(0),
    ];

    let verdict = w.ingest_spans(batch);

    assert_eq!(verdict.keep, vec![other]);
    assert_eq!(verdict.drop_traces, vec![tid(1)]);
    assert!(verdict.drop_spans.is_empty());
    assert_eq!(w.store.stats().spans, 1);
    assert_eq!(w.counts(0), counts(1, 3, 0));
    assert_eq!(w.rules.totals().spans, 3);
}

#[test]
fn whole_trace_drop_covers_stored_and_late_spans() {
    let w = World::new(vec![trace_rule("Health", HEALTH_QUERY)]);
    let mut health = health_trace(1);
    let root = health.remove(0);
    let child_2 = health.remove(0);
    let child_3 = health.remove(0);

    // 1. Children that do not match on their own are kept and stored, and so
    //    is a log of the trace.
    let verdict = w.ingest_spans(vec![child_2, child_3]);
    assert_eq!(verdict.keep.len(), 2);
    assert!(verdict.drop_traces.is_empty());
    assert_eq!(w.store.stats().spans, 2);
    let early_log = in_span(log("web", "checking", 1), 1, 2);
    assert_eq!(w.ingest_logs(vec![early_log]).len(), 1);
    assert_eq!(w.store.stats().logs, 1);

    // 2. The matching root arrives last and condemns the whole trace.
    let verdict = w.rules.filter_spans(vec![root], &w.store);
    assert!(verdict.keep.is_empty());
    assert_eq!(verdict.drop_traces, vec![tid(1)]);

    // 3. The caller removes what is stored, and reports it. The rule's ID is
    //    in the status.
    let removed = w.store.remove_trace(tid(1));
    assert_eq!(removed, Removed { spans: 2, logs: 1 });
    let id = w.rules.statuses()[0].id;
    w.rules.record_removed(id, removed);
    assert_eq!(w.store.stats().spans, 0);

    // 4. A later child of the trace is dropped on arrival.
    let late = span(1, 4, Some(1), "web", "late child", 3, 1);
    let verdict = w.ingest_spans(vec![late]);
    assert!(verdict.keep.is_empty());
    assert!(
        verdict.drop_traces.is_empty(),
        "the trace is already being dropped"
    );
    assert_eq!(w.store.stats().spans, 0);

    // 5. So is a log that carries the trace ID, with or without a span.
    let kept = w.ingest_logs(vec![
        in_span(log("web", "late log", 4), 1, 4),
        in_trace(log("web", "trace-level log", 4), 1),
        in_trace(log("web", "other trace", 4), 2),
    ]);
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].body, "other trace");

    // 6. Counted once as a trace, and every span of it counted as a span: the
    //    two stored, the root, the late child.
    let status = &w.rules.statuses()[0];
    assert_eq!(status.dropped_traces, 1);
    assert_eq!(status.dropped_spans, 4);
    // The stored log that went with the trace and the two late ones.
    assert_eq!(status.dropped_logs, 3);
    assert_eq!(w.rules.totals().spans, 4);
    assert_eq!(w.rules.totals().logs, 3);
}

#[test]
fn late_out_of_order_trace_is_dropped_when_its_root_arrives() {
    // The real scenario: leaves, then the middle, then the root, 14 spans.
    let w = World::new(vec![trace_rule(
        "Root",
        r#"{ name = "POST /api/checkout" }"#,
    )]);
    let batches = scenario_batches(&Scenario::LateOutOfOrder, 1, BASE);
    assert_eq!(batches.len(), 3);
    let (all_spans, all_logs) = batches.iter().fold((0, 0), |(spans, logs), (s, l)| {
        (spans + s.len(), logs + l.len())
    });
    assert_eq!(all_spans, 14);

    for (position, (spans, logs)) in batches.into_iter().enumerate() {
        let before = spans.len();
        let verdict = w.ingest_spans(spans);
        w.ingest_logs(logs);
        if position < 2 {
            assert_eq!(verdict.keep.len(), before, "batch {position} has no match");
        } else {
            assert!(verdict.keep.is_empty());
            assert_eq!(verdict.drop_traces.len(), 1);
        }
    }

    let stats = w.store.stats();
    assert_eq!(
        (stats.spans, stats.logs),
        (0, 0),
        "nothing of the trace is left"
    );
    let status = &w.rules.statuses()[0];
    assert_eq!(status.dropped_traces, 1);
    assert_eq!(status.dropped_spans, all_spans as u64);
    assert_eq!(status.dropped_logs, all_logs as u64);
}

#[test]
fn structural_rule_uses_stored_spans() {
    let query = r#"{ kind = server } >> { name = "MGET" }"#;
    let spans = checkout_failure();
    let trace = spans[0].trace_id;
    let mget = named(&spans, "MGET").clone();

    // On its own the cache call has no server span above it: kept.
    let alone = World::new(vec![trace_rule("Cache under a server", query)]);
    let verdict = alone.ingest_spans(vec![mget.clone()]);
    assert_eq!(verdict.keep.len(), 1);
    assert!(verdict.drop_traces.is_empty());

    // With its ancestors stored earlier, it matches, and the whole trace goes.
    let w = World::new(vec![trace_rule("Cache under a server", query)]);
    let verdict = w.ingest_spans(without(&spans, "MGET"));
    assert_eq!(verdict.keep.len(), 14, "nothing there matches without MGET");
    assert_eq!(w.store.stats().spans, 14);

    let verdict = w.ingest_spans(vec![mget]);

    assert!(verdict.keep.is_empty());
    assert_eq!(verdict.drop_traces, vec![trace]);
    assert_eq!(w.store.stats().spans, 0);
    assert_eq!(w.counts(0), counts(1, 15, 0));
}

#[test]
fn whole_trace_rule_judges_each_trace_on_its_own() {
    let query = r#"{ kind = server } >> { name = "db" }"#;
    let w = World::new(vec![trace_rule("Db under a server", query)]);
    let server = |trace: u8| {
        with_kind(
            span(trace, 1, None, "web", "handle", 0, 10),
            loctel_model::SpanKind::Server,
        )
    };
    // Trace 1 has the pattern, trace 2 has the db span but no server above it.
    let batch = vec![
        server(1),
        span(1, 2, Some(1), "web", "db", 1, 2),
        span(2, 1, None, "web", "handle", 0, 10),
        span(2, 2, Some(1), "web", "db", 1, 2),
    ];

    let verdict = w.ingest_spans(batch);

    assert_eq!(verdict.drop_traces, vec![tid(1)]);
    let kept: Vec<_> = verdict.keep.iter().map(|s| s.trace_id).collect();
    assert_eq!(kept, vec![tid(2), tid(2)]);
    assert_eq!(w.counts(0), counts(1, 2, 0));
}

#[test]
fn count_rule_counts_stored_and_incoming_spans() {
    // Whole-trace through a pipeline: more than three erroring spans.
    let w = World::new(vec![trace_rule(
        "Many errors",
        "{ status = error } | count() > 3",
    )]);
    let spans = checkout_failure();
    let (errors, fine): (Vec<Span>, Vec<Span>) = spans.into_iter().partition(Span::is_error);
    assert_eq!(errors.len(), 7);
    let mut first = fine;
    first.extend(errors[..3].iter().cloned());

    let verdict = w.ingest_spans(first);
    assert_eq!(
        verdict.keep.len(),
        11,
        "three errors are not more than three"
    );

    let verdict = w.ingest_spans(vec![errors[3].clone()]);

    assert!(verdict.keep.is_empty());
    assert_eq!(verdict.drop_traces.len(), 1);
    assert_eq!(w.store.stats().spans, 0);
    assert_eq!(w.counts(0), counts(1, 12, 0));
}

#[test]
fn span_scope_drops_descendants_including_late_ones() {
    let w = World::new(vec![span_rule(
        "Stock lookups",
        r#"{ resource.service.name = "inventory-api" && kind = server }"#,
    )]);
    let spans = checkout_failure();
    let trace = spans[0].trace_id;
    let stock = named(&spans, "GET /stock/{sku}").clone();
    let mget = named(&spans, "MGET").clone();
    let orders_client = named(&spans, "GET").clone();

    // The cache call arrives before its parent: an orphan, kept and stored.
    let verdict = w.ingest_spans(without(&spans, "GET /stock/{sku}"));
    assert_eq!(verdict.keep.len(), 14);
    assert!(verdict.drop_spans.is_empty());
    assert_eq!(w.store.stats().spans, 14);

    // The matching span arrives. It goes, and so does what is stored below it.
    let verdict = w.ingest_spans(vec![stock.clone()]);
    assert!(verdict.keep.is_empty());
    assert!(verdict.drop_traces.is_empty(), "only a span subtree goes");
    assert_eq!(verdict.drop_spans, vec![(trace, mget.span_id)]);
    assert_eq!(w.store.stats().spans, 13);
    assert_eq!(w.counts(0), counts(0, 2, 0));

    // Later descendants are dropped on arrival, whether their parent is the
    // matched span or one of its descendants. In one batch, a child may even
    // come before its parent.
    let under_stock = child_of(&stock, 200, stock.span_id, "late child");
    let under_mget = child_of(&mget, 201, mget.span_id, "late grandchild");
    let grandchild_first = child_of(&mget, 203, sid(202), "grandchild of a late child");
    let late_child = child_of(&stock, 202, stock.span_id, "late child, later in the batch");
    let verdict = w.ingest_spans(vec![under_stock, under_mget, grandchild_first, late_child]);
    assert!(verdict.keep.is_empty(), "{:?}", verdict.keep);
    assert_eq!(w.counts(0), counts(0, 6, 0));
    // The IDs of those are remembered too: a grandchild in a later batch.
    let deeper = child_of(&mget, 204, sid(203), "deeper");
    assert!(w.ingest_spans(vec![deeper]).keep.is_empty());
    assert_eq!(w.counts(0), counts(0, 7, 0));
    assert_eq!(w.store.stats().spans, 13);

    // A sibling of the matched span is not a descendant.
    let sibling = child_of(&orders_client, 205, orders_client.span_id, "sibling-ish");
    assert_eq!(w.ingest_spans(vec![sibling]).keep.len(), 1);

    // The logs of the dropped spans go as well: those of the matched span, and
    // of a descendant that was only ever stored. Other logs stay.
    let kept = w.ingest_logs(vec![
        log("inventory-api", "no trace context", 1),
        in_ids(
            log("inventory-api", "in the matched span", 1),
            trace,
            stock.span_id,
        ),
        in_ids(
            log("inventory-api", "in the cache call", 2),
            trace,
            mget.span_id,
        ),
        in_ids(
            log("orders-api", "in the client span", 3),
            trace,
            orders_client.span_id,
        ),
    ]);
    let bodies: Vec<&str> = kept.iter().map(|l| l.body.as_str()).collect();
    assert_eq!(bodies, ["no trace context", "in the client span"]);
    assert_eq!(w.counts(0).logs, 2);
}

#[test]
fn span_scope_with_a_structural_rule_drops_only_the_matched_spans() {
    // The matched span is the cache call, which has no descendants.
    let w = World::new(vec![span_rule(
        "Cache under a server",
        r#"{ kind = server } >> { name = "MGET" }"#,
    )]);
    let spans = checkout_failure();

    let verdict = w.ingest_spans(spans);

    assert_eq!(verdict.keep.len(), 14);
    assert!(verdict.keep.iter().all(|s| s.name != "MGET"));
    assert!(verdict.drop_traces.is_empty());
    assert!(verdict.drop_spans.is_empty());
    assert_eq!(w.store.stats().spans, 14);
    assert_eq!(w.counts(0), counts(0, 1, 0));
}

#[test]
fn span_scope_drops_the_incoming_subtree_of_a_match() {
    // `POST /charges` and everything below it, all in one batch.
    let w = World::new(vec![span_rule("Charges", r#"{ name = "POST /charges" }"#)]);

    let verdict = w.ingest_spans(checkout_failure());

    // Spans 10 to 13: the server span, its db call and the two gateway calls.
    assert_eq!(verdict.keep.len(), 11);
    for dropped in ["POST /charges", "SELECT payment_methods"] {
        assert!(verdict.keep.iter().all(|s| s.name != dropped), "{dropped}");
    }
    assert_eq!(w.counts(0), counts(0, 4, 0));
}

#[test]
fn logql_rule_drops_matching_logs_only() {
    let w = World::new(vec![log_rule(
        "Retries",
        r#"{service_name="payments-api"} |= "retrying""#,
    )]);
    let logs = scenario_logs(&Scenario::CheckoutFailure, 1, BASE);
    assert_eq!(logs.len(), 7);

    let kept = w.ingest_logs(logs);

    assert_eq!(kept.len(), 6);
    assert!(kept.iter().all(|l| !l.body.contains("retrying")));
    assert_eq!(w.counts(0), counts(0, 0, 1));
    assert_eq!(w.rules.totals().logs, 1);
    // A log rule never touches spans, and a span rule never touches a log that
    // has no remembered trace.
    let verdict = w.ingest_spans(checkout_failure());
    assert_eq!(verdict.keep.len(), 15);
    assert!(verdict.drop_traces.is_empty());
}

#[test]
fn disabled_rule_does_nothing() {
    let mut trace = trace_rule("Health", HEALTH_QUERY);
    trace.enabled = false;
    let mut logs = log_rule("Everything", r#"{service_name=~".+"}"#);
    logs.enabled = false;
    let w = World::new(vec![trace, logs]);

    let verdict = w.ingest_spans(health_trace(1));
    let kept = w.ingest_logs(vec![log("web", "kept", 1)]);

    assert_eq!(verdict.keep.len(), 3);
    assert_eq!(kept.len(), 1);
    assert_eq!(w.counts(0), counts(0, 0, 0));
    assert_eq!(w.counts(1), counts(0, 0, 0));
    // A disabled rule with a good query has no error to report.
    let statuses = w.rules.statuses();
    assert!(
        statuses
            .iter()
            .all(|s| s.error.is_none() && !s.spec.enabled)
    );
}

#[test]
fn invalid_query_disables_only_that_rule() {
    let typo = r#"{ nme = "x" }"#;
    let w = World::new(vec![
        trace_rule("Typo", typo),
        log_rule("Empty selector", "{}"),
        trace_rule("Health", HEALTH_QUERY),
        log_rule("Retries", r#"{service_name="web"} |= "retry""#),
    ]);

    let statuses = w.rules.statuses();
    // The error is the parser's own text, with the position.
    let expected = traceql::parse(typo).unwrap_err().to_string();
    assert_eq!(statuses[0].error.as_deref(), Some(expected.as_str()));
    assert!(expected.contains("unknown intrinsic"), "{expected}");
    assert!(statuses[1].error.is_some());
    assert_eq!(statuses[2].error, None);
    assert_eq!(statuses[3].error, None);

    // The other rules still drop.
    let verdict = w.ingest_spans(health_trace(1));
    assert!(verdict.keep.is_empty());
    let kept = w.ingest_logs(vec![log("web", "retry later", 1), log("web", "fine", 2)]);
    assert_eq!(kept.len(), 1);
    assert_eq!(w.counts(0), counts(0, 0, 0));
    assert_eq!(w.counts(1), counts(0, 0, 0));
    assert_eq!(w.counts(2), counts(1, 3, 0));
    assert_eq!(w.counts(3), counts(0, 0, 1));
}

#[test]
fn a_disabled_invalid_rule_still_reports_its_error() {
    // An editor saving a draft wants to see what is wrong with it.
    let mut draft = trace_rule("Draft", "{ status = ");
    draft.enabled = false;
    let w = World::new(vec![draft]);

    assert!(w.rules.statuses()[0].error.is_some());
}

#[test]
fn first_rule_in_file_order_gets_the_count() {
    let w = World::new(vec![
        trace_rule("First", HEALTH_QUERY),
        trace_rule("Second", r#"{ name = "GET /health" }"#),
        log_rule("First log rule", r#"{service_name="web"}"#),
        log_rule("Second log rule", r#"{service_name="web"} |= "x""#),
    ]);

    let verdict = w.ingest_spans(health_trace(1));
    let kept = w.ingest_logs(vec![log("web", "x marks the spot", 1)]);

    assert!(verdict.keep.is_empty());
    assert!(kept.is_empty());
    assert_eq!(w.counts(0), counts(1, 3, 0));
    assert_eq!(w.counts(1), counts(0, 0, 0));
    assert_eq!(w.counts(2), counts(0, 0, 1));
    assert_eq!(w.counts(3), counts(0, 0, 0));
    // The totals count each span and log once.
    assert_eq!(w.rules.totals().spans, 3);
    assert_eq!(w.rules.totals().logs, 1);
}

#[test]
fn a_remembered_trace_counts_on_the_rule_that_dropped_it() {
    // The second rule would also match the late child; the memory credits the
    // rule that dropped the trace, whatever the file order is.
    let w = World::new(vec![
        trace_rule("Late children", r#"{ name = "late child" }"#),
        trace_rule("Health", HEALTH_QUERY),
    ]);
    let mut health = health_trace(1);
    w.ingest_spans(vec![health.remove(0)]);
    assert_eq!(w.counts(1), counts(1, 1, 0));

    w.ingest_spans(vec![span(1, 4, Some(1), "web", "late child", 3, 1)]);

    assert_eq!(w.counts(0), counts(0, 0, 0));
    assert_eq!(w.counts(1), counts(1, 2, 0));
}

#[test]
fn memory_expires_after_ttl() {
    let w = World::new(vec![trace_rule("Health", HEALTH_QUERY)]);
    let mut health = health_trace(1);
    assert!(w.ingest_spans(vec![health.remove(0)]).keep.is_empty());
    assert_eq!(DROPPED_TTL, Duration::from_secs(10 * 60));

    // Nine minutes on, the trace is still remembered.
    w.clock.advance(Duration::from_secs(9 * 60));
    let verdict = w.ingest_spans(vec![health.remove(0)]);
    assert!(verdict.keep.is_empty());
    assert!(
        w.ingest_logs(vec![in_trace(log("web", "early", 1), 1)])
            .is_empty()
    );

    // Just under the limit, still; at the limit, forgotten.
    w.clock
        .advance(Duration::from_secs(60) - Duration::from_nanos(1));
    assert!(
        w.ingest_spans(vec![span(1, 5, Some(1), "web", "just in time", 3, 1)])
            .keep
            .is_empty()
    );
    w.clock.advance(Duration::from_nanos(1));
    let late = span(1, 6, Some(1), "web", "too late", 3, 1);
    assert_eq!(w.ingest_spans(vec![late]).keep.len(), 1);

    // After 11 minutes, a child that does not match on its own is kept, and so
    // is a log of the trace.
    w.clock.advance(Duration::from_secs(60));
    let verdict = w.ingest_spans(vec![health.remove(0)]);
    assert_eq!(verdict.keep.len(), 1);
    assert_eq!(
        w.ingest_logs(vec![in_trace(log("web", "late", 2), 1)])
            .len(),
        1
    );
    assert_eq!(w.store.stats().spans, 2);
}

#[test]
fn remembered_spans_expire_too() {
    let w = World::new(vec![span_rule("Cache", r#"{ name = "cache" }"#)]);
    w.ingest_spans(vec![
        span(1, 1, None, "web", "handle", 0, 10),
        span(1, 2, Some(1), "web", "cache", 1, 2),
    ]);
    let under_cache = |id: u8| span(1, id, Some(2), "web", "below", 2, 1);

    assert!(w.ingest_spans(vec![under_cache(3)]).keep.is_empty());
    w.clock.advance(DROPPED_TTL + Duration::from_secs(1));

    assert_eq!(w.ingest_spans(vec![under_cache(4)]).keep.len(), 1);
    let logs = w.ingest_logs(vec![in_span(log("web", "in cache", 1), 1, 2)]);
    assert_eq!(logs.len(), 1);
}

#[test]
fn replace_keeps_counters_for_unchanged_rules() {
    let health = trace_rule("Health", HEALTH_QUERY);
    let retries = log_rule("Retries", r#"{service_name="web"} |= "retry""#);
    let w = World::new(vec![health.clone(), retries.clone()]);
    w.ingest_spans(health_trace(1));
    w.ingest_logs(vec![log("web", "retry", 1)]);
    let before = w.rules.statuses();
    assert_eq!(before.len(), 2);
    assert_ne!(before[0].id, before[1].id);

    // The same rules in another order keep their IDs and counters.
    w.rules.replace(vec![retries.clone(), health.clone()]);
    let after = w.rules.statuses();
    assert_eq!(after[0].id, before[1].id);
    assert_eq!(after[1].id, before[0].id);
    assert_eq!(after[0].dropped_logs, 1);
    assert_eq!((after[1].dropped_traces, after[1].dropped_spans), (1, 3));

    // Turning a rule off keeps them, and the flag changes.
    let mut off = health.clone();
    off.enabled = false;
    w.rules.replace(vec![retries.clone(), off]);
    let toggled = w.rules.statuses();
    assert_eq!(toggled[1].id, before[0].id);
    assert!(!toggled[1].spec.enabled);
    assert_eq!(toggled[1].dropped_spans, 3);
    // ... and it really is off now.
    assert_eq!(w.ingest_spans(health_trace(2)).keep.len(), 3);

    // Changing the query is a new rule: a new ID and counters from zero.
    let mut edited = health.clone();
    edited.query = r#"{ span.url.path = "/healthz" }"#.to_string();
    w.rules.replace(vec![retries.clone(), edited]);
    let edited = w.rules.statuses();
    assert_eq!(edited[0].id, before[1].id);
    assert_ne!(edited[1].id, before[0].id);
    assert_ne!(edited[1].id, before[1].id);
    assert_eq!(edited[1].dropped_spans, 0);

    // Changing the scope or the language does the same.
    let mut scoped = health.clone();
    scoped.scope = DropScope::Span;
    w.rules.replace(vec![retries.clone(), scoped]);
    assert_ne!(w.rules.statuses()[1].id, before[0].id);

    // A rule that is gone from the list is gone from the statuses, and the
    // totals, which count since launch, keep what it dropped.
    w.rules.replace(vec![retries]);
    assert_eq!(w.rules.statuses().len(), 1);
    assert_eq!(w.rules.totals().spans, 3);
    assert_eq!(w.rules.totals().logs, 1);

    // Identical rules are told apart: each keeps its own ID and counters.
    let twin = trace_rule("Twin", HEALTH_QUERY);
    w.rules.replace(vec![twin.clone(), twin.clone()]);
    let twins = w.rules.statuses();
    assert_ne!(twins[0].id, twins[1].id);
}

#[test]
fn replace_keeps_what_was_dropped_in_memory() {
    let w = World::new(vec![trace_rule("Health", HEALTH_QUERY)]);
    let mut health = health_trace(1);
    w.ingest_spans(vec![health.remove(0)]);

    // Rules replaced by none at all: the trace that was dropped stays dropped
    // for its time.
    w.rules.replace(vec![]);
    let verdict = w.ingest_spans(vec![health.remove(0)]);

    assert!(verdict.keep.is_empty());
    assert!(w.rules.statuses().is_empty());
    // The rule is gone, so only the totals know about the span.
    assert_eq!(w.rules.totals().spans, 2);
}

#[test]
fn ids_are_assigned_when_a_rule_first_appears() {
    let w = World::new(vec![trace_rule("A", HEALTH_QUERY)]);
    let first = w.rules.statuses()[0].id;

    w.rules
        .replace(vec![trace_rule("A", HEALTH_QUERY), trace_rule("B", "{ }")]);
    let both = w.rules.statuses();
    assert_eq!(both[0].id, first);
    assert_ne!(both[1].id, first);

    // A rule that left and came back is a new rule: IDs are never reused.
    w.rules.replace(vec![]);
    w.rules.replace(vec![trace_rule("B", "{ }")]);
    let back = w.rules.statuses()[0].id;
    assert_ne!(back, first);
    assert_ne!(back, both[1].id);
}

#[test]
fn remove_dropped_credits_the_rule_that_dropped_each_thing() {
    let w = World::new(vec![
        trace_rule("Health", HEALTH_QUERY),
        span_rule("Cache", r#"{ name = "cache" }"#),
    ]);
    // Stored first, with a log each. Trace 1 is a health check without its
    // root yet. In trace 2, a span below the cache span arrived before the
    // cache span itself: an orphan.
    let mut health = health_trace(1);
    let health_root = health.remove(0);
    w.ingest_spans(health);
    w.ingest_spans(vec![
        span(2, 1, None, "web", "handle", 0, 10),
        span(2, 3, Some(2), "web", "below the cache", 2, 1),
    ]);
    w.ingest_logs(vec![
        in_span(log("web", "in the health check", 1), 1, 2),
        in_span(log("web", "below the cache", 2), 2, 3),
        in_span(log("web", "kept", 3), 2, 1),
    ]);
    assert_eq!(w.store.stats().spans, 4);
    assert_eq!(w.store.stats().logs, 3);

    // One batch: the health root, which condemns trace 1, and the cache span,
    // which condemns what is stored below it in trace 2.
    let verdict = w.rules.filter_spans(
        vec![health_root, span(2, 2, Some(1), "web", "cache", 1, 5)],
        &w.store,
    );
    assert!(verdict.keep.is_empty());
    assert_eq!(verdict.drop_traces, vec![tid(1)]);
    assert_eq!(verdict.drop_spans, vec![(tid(2), sid(3))]);
    let removed = w.rules.remove_dropped(&verdict, &w.store);

    assert_eq!(removed, Removed { spans: 3, logs: 2 });
    let stats = w.store.stats();
    assert_eq!((stats.traces, stats.spans, stats.logs), (1, 1, 1));
    // The health rule: the trace, the root and the two stored spans, and the
    // stored log. The cache rule: the cache span and the one below it, and the
    // log of that one.
    assert_eq!(w.counts(0), counts(1, 3, 1));
    assert_eq!(w.counts(1), counts(0, 2, 1));
    assert_eq!(w.rules.totals().spans, 5);
    assert_eq!(w.rules.totals().logs, 2);
}

#[test]
fn record_removed_adds_to_the_rule_and_the_totals() {
    let w = World::new(vec![trace_rule("Health", HEALTH_QUERY)]);
    let id = w.rules.statuses()[0].id;

    w.rules.record_removed(id, Removed { spans: 5, logs: 2 });

    let status = &w.rules.statuses()[0];
    assert_eq!((status.dropped_spans, status.dropped_logs), (5, 2));
    assert_eq!(status.dropped_traces, 0);
    assert_eq!(w.rules.totals().spans, 5);
    assert_eq!(w.rules.totals().logs, 2);
    // A rule that does not exist (any more) still counts in the totals.
    w.rules.replace(vec![]);
    w.rules.record_removed(id, Removed { spans: 1, logs: 1 });
    assert_eq!(w.rules.totals().spans, 6);
    assert_eq!(w.rules.totals().logs, 3);
}

#[test]
fn rule_set_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<RuleSet>();
}

#[test]
fn rules_apply_from_several_threads() {
    // The sink is called from the gRPC and HTTP receivers at once.
    let w = std::sync::Arc::new(World::new(vec![trace_rule("Health", HEALTH_QUERY)]));
    let handles: Vec<_> = (1..=4u8)
        .map(|thread| {
            let w = w.clone();
            std::thread::spawn(move || {
                for round in 0..50u8 {
                    // A trace of its own for every round of every thread.
                    let mut spans = health_trace(thread * 50 + round);
                    spans.truncate(2);
                    let verdict = w.rules.filter_spans(spans, &w.store);
                    assert!(verdict.keep.is_empty());
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }

    let status = &w.rules.statuses()[0];
    assert_eq!(status.dropped_traces, 200);
    assert_eq!(status.dropped_spans, 400);
}

/// The example rule of `rules.toml`, as the file parser reads it.
fn example_rule() -> loctel_rules::RuleSpec {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.toml");
    std::fs::write(&path, loctel_rules::RulesFile::EXAMPLE_RULE).unwrap();
    let mut rules = loctel_rules::RulesFile::load(&path).unwrap().unwrap();
    assert_eq!(rules.len(), 1);
    rules.remove(0)
}

#[test]
fn example_rule_drops_health_checks_and_leaves_the_checkout_alone() {
    let w = World::new(vec![example_rule()]);
    let health = Scenario::HealthChecks { count: 3 };
    let mut spans = scenario_spans(&health, 2, BASE);
    assert_eq!(spans.len(), 15);
    let checkout = checkout_failure();
    // The checkout in the middle of the probes.
    spans.splice(7..7, checkout.clone());
    let mut logs = scenario_logs(&health, 2, BASE);
    logs.extend(scenario_logs(&Scenario::CheckoutFailure, 1, BASE));

    let verdict = w.ingest_spans(spans);
    let kept = w.ingest_logs(logs);

    assert_eq!(verdict.keep, checkout);
    assert_eq!(verdict.drop_traces.len(), 15);
    assert_eq!(kept.len(), 7, "only the checkout's logs remain");
    assert!(kept.iter().all(|l| !l.body.contains("/health")));
    assert_eq!(w.counts(0), counts(15, 15, 15));
    let stats = w.store.stats();
    assert_eq!((stats.traces, stats.spans, stats.logs), (1, 15, 7));
}

#[test]
fn a_big_batch_is_judged_in_one_pass() {
    // 2,000 health-check traces in two batches of 1,000, with a rule that drops
    // the whole trace and one that needs the whole trace: no work per pair of
    // spans, so this is quick even in a debug build.
    let w = World::new(vec![
        trace_rule("Structural", r#"{ kind = server } >> { kind = client }"#),
        example_rule(),
    ]);
    let batches = scenario_span_batches(&Scenario::HealthChecks { count: 400 }, 2, BASE);
    assert_eq!(
        batches.iter().map(Vec::len).collect::<Vec<_>>(),
        [1000, 1000]
    );

    for batch in batches {
        let verdict = w.ingest_spans(batch);
        assert!(verdict.keep.is_empty());
        assert_eq!(verdict.drop_traces.len(), 1000);
    }

    // The structural rule looked at every trace and found nothing.
    assert_eq!(w.counts(0), counts(0, 0, 0));
    assert_eq!(w.counts(1), counts(2000, 2000, 0));
    assert_eq!(w.store.stats().spans, 0);
}
