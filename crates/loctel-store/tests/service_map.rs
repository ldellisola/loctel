//! The service map and the services list (plan Task 5, design 9.2 and 9.4).
//!
//! The plan's eight tests come first. The rest pin the corners the rules
//! leave open and the malformed input the store has to survive.

mod fixtures;

use std::sync::Arc;
use std::time::Duration;

use fixtures::*;
use loctel_model::{Resource, Span, SpanKind, SpanKind::*};
use loctel_store::{
    ManualClock, NodeKind, NodeKind::*, ServiceEdge, ServiceMap, ServiceNode, ServiceStats,
    TimeWindow,
};

const MS: u64 = 1_000_000;

/// A span of `kind` that lasts 10 ms.
fn sp(kind: SpanKind, trace: u8, id: u8, parent: Option<u8>, service: &str, start_ms: u64) -> Span {
    with_kind(span(trace, id, parent, service, "op", start_ms, 10), kind)
}

fn all() -> TimeWindow {
    TimeWindow::default()
}

fn since_ms(ms: u64) -> TimeWindow {
    TimeWindow {
        since_unix_nano: ms * MS,
    }
}

/// Lets every trace in the store go quiet, so it counts as complete.
fn settle(clock: &ManualClock) {
    clock.advance(Duration::from_secs(10));
}

fn nodes(map: &ServiceMap) -> Vec<(&str, NodeKind, u64, u64)> {
    map.nodes
        .iter()
        .map(|n| (n.id.as_str(), n.kind, n.requests, n.errors))
        .collect()
}

fn edges(map: &ServiceMap) -> Vec<(&str, &str, u64, u64)> {
    map.edges
        .iter()
        .map(|e| (e.from.as_str(), e.to.as_str(), e.calls, e.errors))
        .collect()
}

fn find_node<'a>(map: &'a ServiceMap, id: &str) -> Option<&'a ServiceNode> {
    map.nodes.iter().find(|n| n.id == id)
}

fn find_edge<'a>(map: &'a ServiceMap, from: &str, to: &str) -> Option<&'a ServiceEdge> {
    map.edges.iter().find(|e| e.from == from && e.to == to)
}

fn kinds_present(map: &ServiceMap, kind: NodeKind) -> Vec<&str> {
    map.nodes
        .iter()
        .filter(|n| n.kind == kind)
        .map(|n| n.id.as_str())
        .collect()
}

// --- the plan's tests ---

#[test]
fn service_edges_from_cross_service_parent_child() {
    let (store, clock) = store_with_manual_clock();
    store.ingest_spans(vec![
        // web-bff -> orders-api -> payments-api (fails) and inventory-api.
        sp(Server, 1, 1, None, "web-bff", 0),
        sp(Client, 1, 2, Some(1), "web-bff", 5),
        sp(Server, 1, 3, Some(2), "orders-api", 10),
        sp(Client, 1, 4, Some(3), "orders-api", 20),
        error(sp(Server, 1, 5, Some(4), "payments-api", 30)),
        sp(Client, 1, 6, Some(3), "orders-api", 40),
        sp(Server, 1, 7, Some(6), "inventory-api", 45),
    ]);
    settle(&clock);
    let map = store.service_map(all());
    assert_eq!(
        nodes(&map),
        vec![
            ("inventory-api", Service, 1, 0),
            ("orders-api", Service, 1, 0),
            ("payments-api", Service, 1, 1),
            ("web-bff", Service, 1, 0),
        ]
    );
    assert_eq!(
        edges(&map),
        vec![
            ("orders-api", "inventory-api", 1, 0),
            ("orders-api", "payments-api", 1, 1),
            ("web-bff", "orders-api", 1, 0),
        ]
    );
    // A same-service parent and child (the server span and its own client
    // span) make no edge, and every client here has a child in another
    // service, so nothing is inferred as external.
    assert!(find_edge(&map, "web-bff", "web-bff").is_none());
    assert!(kinds_present(&map, External).is_empty());
}

#[test]
fn database_node_from_db_system_name() {
    let (store, clock) = store_with_manual_clock();
    store.ingest_spans(vec![
        sp(Server, 1, 1, None, "orders-api", 0),
        with_attr(
            sp(Client, 1, 2, Some(1), "orders-api", 1),
            "db.system.name",
            "postgresql",
        ),
        error(with_attr(
            sp(Client, 1, 3, Some(1), "orders-api", 2),
            "db.system.name",
            "postgresql",
        )),
        // The legacy attribute name works too.
        with_attr(
            sp(Client, 1, 4, Some(1), "orders-api", 3),
            "db.system",
            "redis",
        ),
    ]);
    settle(&clock);
    let map = store.service_map(all());
    assert_eq!(
        nodes(&map),
        vec![
            ("orders-api", Service, 1, 0),
            ("postgresql", Database, 2, 1),
            ("redis", Database, 1, 0),
        ]
    );
    // The edge's errors come from the client span.
    assert_eq!(
        edges(&map),
        vec![
            ("orders-api", "postgresql", 2, 1),
            ("orders-api", "redis", 1, 0),
        ]
    );
    // A database client is not also an external host, even in a complete
    // trace.
    assert!(kinds_present(&map, External).is_empty());
}

#[test]
fn queue_node_routes_messaging() {
    let (store, clock) = store_with_manual_clock();
    store.ingest_spans(vec![
        sp(Server, 1, 1, None, "orders-api", 0),
        with_attr(
            sp(Producer, 1, 2, Some(1), "orders-api", 5),
            "messaging.system",
            "rabbitmq",
        ),
        with_attr(
            sp(Consumer, 1, 3, Some(2), "notifications-worker", 8),
            "messaging.system",
            "rabbitmq",
        ),
    ]);
    settle(&clock);
    let map = store.service_map(all());
    assert_eq!(find_node(&map, "rabbitmq").unwrap().kind, Queue);
    assert_eq!(
        edges(&map),
        vec![
            ("orders-api", "rabbitmq", 1, 0),
            ("rabbitmq", "notifications-worker", 1, 0),
        ]
    );
    // The producer's span is the consumer's parent, but the queue sits
    // between them: no direct edge.
    assert!(find_edge(&map, "orders-api", "notifications-worker").is_none());
}

#[test]
fn external_node_only_after_complete() {
    let (store, clock) = store_with_manual_clock();
    store.ingest_spans(vec![
        sp(Server, 1, 1, None, "payments-api", 0),
        with_attr(
            sp(Client, 1, 2, Some(1), "payments-api", 5),
            "server.address",
            "card-gateway",
        ),
    ]);
    clock.advance(Duration::from_secs(9));
    let early = store.service_map(all());
    assert!(
        find_node(&early, "card-gateway").is_none(),
        "the gateway's own span may still be on its way: {early:?}"
    );
    assert!(find_edge(&early, "payments-api", "card-gateway").is_none());

    clock.advance(Duration::from_secs(1));
    let done = store.service_map(all());
    assert_eq!(
        nodes(&done),
        vec![
            ("card-gateway", External, 1, 0),
            ("payments-api", Service, 1, 0),
        ]
    );
    assert_eq!(edges(&done), vec![("payments-api", "card-gateway", 1, 0)]);
}

#[test]
fn entry_span_error_counts() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_spans(vec![
        // Entry spans: two servers and a consumer, of which two are erroring.
        sp(Server, 1, 1, None, "orders-api", 0),
        error(sp(Server, 2, 1, None, "orders-api", 10)),
        error(sp(Consumer, 3, 1, None, "orders-api", 20)),
        // Everything else is not an entry span, erroring or not.
        error(sp(Client, 4, 1, None, "orders-api", 30)),
        error(sp(Producer, 5, 1, None, "orders-api", 40)),
        error(sp(Internal, 6, 1, None, "orders-api", 50)),
        error(sp(Unspecified, 7, 1, None, "orders-api", 60)),
    ]);
    let map = store.service_map(all());
    assert_eq!(nodes(&map), vec![("orders-api", Service, 3, 2)]);
}

#[test]
fn window_excludes_old_spans() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_spans(vec![
        // An old request: web-bff -> orders-api, at 0 ms.
        sp(Server, 1, 1, None, "web-bff", 0),
        sp(Client, 1, 2, Some(1), "web-bff", 1),
        sp(Server, 1, 3, Some(2), "orders-api", 2),
        // A new one: inventory-api -> payments-api, at 10 s.
        sp(Server, 2, 1, None, "inventory-api", 10_000),
        sp(Client, 2, 2, Some(1), "inventory-api", 10_001),
        sp(Server, 2, 3, Some(2), "payments-api", 10_002),
    ]);
    let recent = store.service_map(since_ms(5_000));
    assert_eq!(
        nodes(&recent),
        vec![
            ("inventory-api", Service, 1, 0),
            ("payments-api", Service, 1, 0),
        ]
    );
    assert_eq!(
        edges(&recent),
        vec![("inventory-api", "payments-api", 1, 0)]
    );
    assert_eq!(store.service_map(all()).nodes.len(), 4);
    assert_eq!(store.service_map(all()).edges.len(), 2);
    // Spans that start exactly at the window's start are inside it.
    assert_eq!(store.service_map(since_ms(10_000)).nodes.len(), 2);
    assert_eq!(store.service_map(since_ms(10_003)).nodes.len(), 0);
    assert_eq!(store.service_map(since_ms(10_003)).edges.len(), 0);
}

#[test]
fn unknown_service_appears_in_map() {
    let (store, _c) = store_with_manual_clock();
    let mut nameless = sp(Server, 1, 3, Some(2), "ignored", 2);
    nameless.resource = Arc::new(Resource { attributes: vec![] });
    store.ingest_spans(vec![
        sp(Server, 1, 1, None, "web-bff", 0),
        sp(Client, 1, 2, Some(1), "web-bff", 1),
        nameless,
    ]);
    let map = store.service_map(all());
    assert_eq!(
        nodes(&map),
        vec![
            ("unknown_service", Service, 1, 0),
            ("web-bff", Service, 1, 0)
        ]
    );
    assert_eq!(edges(&map), vec![("web-bff", "unknown_service", 1, 0)]);
    let services = store.services(all());
    assert_eq!(
        services.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        vec!["unknown_service", "web-bff"]
    );
}

#[test]
fn services_list_has_counts_and_last_seen() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_spans(vec![
        sp(Server, 1, 1, None, "orders-api", 100),
        error(sp(Server, 2, 1, None, "orders-api", 300)),
        sp(Server, 3, 1, None, "payments-api", 200),
        // web-bff only calls out: it is a service with no requests of its
        // own.
        sp(Client, 4, 1, None, "web-bff", 250),
        // Dependencies are on the map but are not services.
        with_attr(
            sp(Client, 5, 1, None, "orders-api", 120),
            "db.system.name",
            "postgresql",
        ),
    ]);
    assert_eq!(
        store.services(all()),
        vec![
            ServiceStats {
                name: "orders-api".into(),
                requests: 2,
                errors: 1,
                last_seen_unix_nano: 300 * MS,
            },
            ServiceStats {
                name: "payments-api".into(),
                requests: 1,
                errors: 0,
                last_seen_unix_nano: 200 * MS,
            },
            ServiceStats {
                name: "web-bff".into(),
                requests: 0,
                errors: 0,
                last_seen_unix_nano: 250 * MS,
            },
        ]
    );
    // The window applies: only what starts inside it counts, and a service
    // with nothing inside it is not listed.
    assert_eq!(
        store.services(since_ms(250)),
        vec![
            ServiceStats {
                name: "orders-api".into(),
                requests: 1,
                errors: 1,
                last_seen_unix_nano: 300 * MS,
            },
            ServiceStats {
                name: "web-bff".into(),
                requests: 0,
                errors: 0,
                last_seen_unix_nano: 250 * MS,
            },
        ]
    );
}

// --- service-to-service edges ---

#[test]
fn edge_errors_count_when_either_end_is_erroring() {
    let (store, _c) = store_with_manual_clock();
    // Four calls a -> b: the client fails, the server fails, both fail, and
    // neither fails. A call with both ends erroring is one error, not two.
    let call = |trace: u8, client_fails: bool, server_fails: bool| {
        let client = sp(Client, trace, 2, Some(1), "a", 1);
        let server = sp(Server, trace, 3, Some(2), "b", 2);
        vec![
            sp(Server, trace, 1, None, "a", 0),
            if client_fails { error(client) } else { client },
            if server_fails { error(server) } else { server },
        ]
    };
    for (trace, client_fails, server_fails) in [
        (1, true, false),
        (2, false, true),
        (3, true, true),
        (4, false, false),
    ] {
        store.ingest_spans(call(trace, client_fails, server_fails));
    }
    let map = store.service_map(all());
    assert_eq!(edges(&map), vec![("a", "b", 4, 3)]);
    // The node's own counts are about its entry spans only.
    assert_eq!(find_node(&map, "a").unwrap().requests, 4);
    assert_eq!(find_node(&map, "b").unwrap().errors, 2);
}

#[test]
fn a_parent_in_the_same_service_makes_no_edge() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_spans(vec![
        sp(Server, 1, 1, None, "a", 0),
        sp(Internal, 1, 2, Some(1), "a", 1),
        sp(Internal, 1, 3, Some(2), "a", 2),
    ]);
    assert!(store.service_map(all()).edges.is_empty());
}

#[test]
fn a_parent_of_any_kind_in_another_service_makes_an_edge() {
    // Not every hop is instrumented with client and server spans: the edge
    // follows the service change, not the span kinds.
    let (store, _c) = store_with_manual_clock();
    store.ingest_spans(vec![
        sp(Internal, 1, 1, None, "a", 0),
        sp(Internal, 1, 2, Some(1), "b", 1),
    ]);
    assert_eq!(edges(&store.service_map(all())), vec![("a", "b", 1, 0)]);
}

#[test]
fn a_child_whose_parent_has_not_arrived_makes_no_edge_until_it_does() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_spans(vec![sp(Server, 1, 3, Some(2), "b", 2)]);
    let early = store.service_map(all());
    assert_eq!(nodes(&early), vec![("b", Service, 1, 0)]);
    assert!(early.edges.is_empty());
    store.ingest_spans(vec![
        sp(Server, 1, 1, None, "a", 0),
        sp(Client, 1, 2, Some(1), "a", 1),
    ]);
    assert_eq!(edges(&store.service_map(all())), vec![("a", "b", 1, 0)]);
}

#[test]
fn a_skewed_child_still_gets_its_edge() {
    // The child's clock is behind its parent's: it "starts" first and
    // "ends" before it starts. The parent ID still says who called whom.
    let (store, _c) = store_with_manual_clock();
    let mut child = sp(Server, 1, 3, Some(2), "b", 2);
    child.end_unix_nano = 1;
    store.ingest_spans(vec![
        sp(Server, 1, 1, None, "a", 10),
        sp(Client, 1, 2, Some(1), "a", 20),
        child,
    ]);
    assert_eq!(edges(&store.service_map(all())), vec![("a", "b", 1, 0)]);
}

// --- databases ---

#[test]
fn db_system_name_wins_over_the_legacy_attribute() {
    let (store, _c) = store_with_manual_clock();
    let client = with_attr(
        with_attr(
            sp(Client, 1, 1, None, "orders-api", 0),
            "db.system",
            "mysql",
        ),
        "db.system.name",
        "postgresql",
    );
    store.ingest_spans(vec![client]);
    assert_eq!(
        kinds_present(&store.service_map(all()), Database),
        vec!["postgresql"]
    );
}

#[test]
fn only_client_spans_make_database_nodes() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_spans(vec![
        with_attr(
            sp(Server, 1, 1, None, "a", 0),
            "db.system.name",
            "postgresql",
        ),
        with_attr(
            sp(Internal, 2, 1, None, "a", 0),
            "db.system.name",
            "postgresql",
        ),
        with_attr(
            sp(Producer, 3, 1, None, "a", 0),
            "db.system.name",
            "postgresql",
        ),
    ]);
    let map = store.service_map(all());
    assert_eq!(nodes(&map), vec![("a", Service, 1, 0)]);
    assert!(map.edges.is_empty());
}

#[test]
fn an_empty_db_system_is_no_database() {
    let (store, clock) = store_with_manual_clock();
    store.ingest_spans(vec![with_attr(
        sp(Client, 1, 1, None, "a", 0),
        "db.system.name",
        "",
    )]);
    settle(&clock);
    // No database name: the call has no db attribute, so it is an unmatched
    // client call like any other, named "unknown".
    let map = store.service_map(all());
    assert!(kinds_present(&map, Database).is_empty());
    assert_eq!(kinds_present(&map, External), vec!["unknown"]);
}

// --- queues ---

#[test]
fn queue_edge_errors_follow_the_producer_and_the_consumer() {
    let (store, _c) = store_with_manual_clock();
    let queue = |s: Span| with_attr(s, "messaging.system", "kafka");
    store.ingest_spans(vec![
        // Two sends, one of which fails; two deliveries, the other fails.
        queue(sp(Producer, 1, 1, None, "orders-api", 0)),
        queue(error(sp(Producer, 2, 1, None, "orders-api", 10))),
        queue(sp(Consumer, 3, 1, None, "worker", 20)),
        queue(error(sp(Consumer, 4, 1, None, "worker", 30))),
    ]);
    let map = store.service_map(all());
    assert_eq!(
        edges(&map),
        vec![("kafka", "worker", 2, 1), ("orders-api", "kafka", 2, 1)]
    );
    // The queue received two sends, one failed. The worker's own node counts
    // its two deliveries as entry spans.
    assert_eq!(
        nodes(&map),
        vec![
            ("kafka", Queue, 2, 1),
            ("orders-api", Service, 0, 0),
            ("worker", Service, 2, 1),
        ]
    );
}

#[test]
fn a_consumer_alone_still_gets_its_queue_node() {
    // An edge never points at a node the map does not have.
    let (store, _c) = store_with_manual_clock();
    store.ingest_spans(vec![with_attr(
        sp(Consumer, 1, 1, None, "worker", 0),
        "messaging.system",
        "rabbitmq",
    )]);
    let map = store.service_map(all());
    assert_eq!(
        nodes(&map),
        vec![("rabbitmq", Queue, 0, 0), ("worker", Service, 1, 0)]
    );
    assert_eq!(edges(&map), vec![("rabbitmq", "worker", 1, 0)]);
}

#[test]
fn the_cross_service_edge_is_skipped_only_when_both_spans_carry_messaging() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_spans(vec![
        // A producer whose child is an ordinary server span of another
        // service (no messaging attribute on it): a real call, so an edge.
        with_attr(
            sp(Producer, 1, 1, None, "a", 0),
            "messaging.system",
            "rabbitmq",
        ),
        sp(Server, 1, 2, Some(1), "b", 1),
    ]);
    let map = store.service_map(all());
    assert_eq!(edges(&map), vec![("a", "b", 1, 0), ("a", "rabbitmq", 1, 0)]);
}

#[test]
fn a_client_span_with_messaging_system_is_neither_queue_nor_external() {
    // Only a producer or consumer makes a queue edge, and a client span with
    // a messaging attribute is not an unmatched external call either.
    let (store, clock) = store_with_manual_clock();
    store.ingest_spans(vec![with_attr(
        sp(Client, 1, 1, None, "a", 0),
        "messaging.system",
        "sqs",
    )]);
    settle(&clock);
    let map = store.service_map(all());
    assert_eq!(nodes(&map), vec![("a", Service, 0, 0)]);
    assert!(map.edges.is_empty());
}

// --- external hosts ---

#[test]
fn external_node_is_named_by_the_peer_or_unknown() {
    let (store, clock) = store_with_manual_clock();
    store.ingest_spans(vec![
        with_attr(
            sp(Client, 1, 1, None, "a", 0),
            "server.address",
            "stripe.example",
        ),
        with_attr(
            sp(Client, 2, 1, None, "a", 1),
            "peer.service",
            "legacy-peer",
        ),
        // `server.address` wins over `peer.service`.
        with_attr(
            with_attr(
                sp(Client, 3, 1, None, "a", 2),
                "server.address",
                "both.example",
            ),
            "peer.service",
            "ignored",
        ),
        // The client span fails, and the edge counts it.
        error(sp(Client, 4, 1, None, "a", 3)),
        sp(Client, 5, 1, None, "a", 4),
    ]);
    settle(&clock);
    let map = store.service_map(all());
    assert_eq!(
        kinds_present(&map, External),
        vec!["both.example", "legacy-peer", "stripe.example", "unknown"]
    );
    assert_eq!(find_edge(&map, "a", "unknown").unwrap().calls, 2);
    assert_eq!(find_edge(&map, "a", "unknown").unwrap().errors, 1);
    assert_eq!(find_node(&map, "unknown").unwrap().kind, External);
    assert_eq!(find_node(&map, "unknown").unwrap().requests, 2);
    assert_eq!(find_node(&map, "unknown").unwrap().errors, 1);
}

#[test]
fn a_new_span_makes_the_trace_incomplete_and_its_external_node_goes_away() {
    let (store, clock) = store_with_manual_clock();
    store.ingest_spans(vec![with_attr(
        sp(Client, 1, 1, None, "a", 0),
        "server.address",
        "gateway",
    )]);
    settle(&clock);
    assert_eq!(
        kinds_present(&store.service_map(all()), External),
        vec!["gateway"]
    );
    store.ingest_spans(vec![sp(Internal, 1, 2, Some(1), "a", 1)]);
    assert!(kinds_present(&store.service_map(all()), External).is_empty());
    settle(&clock);
    assert_eq!(
        kinds_present(&store.service_map(all()), External),
        vec!["gateway"]
    );
}

#[test]
fn a_client_with_a_child_in_another_service_is_not_external() {
    let (store, clock) = store_with_manual_clock();
    store.ingest_spans(vec![
        with_attr(
            sp(Client, 1, 1, None, "a", 0),
            "server.address",
            "b.example",
        ),
        sp(Server, 1, 2, Some(1), "b", 1),
    ]);
    settle(&clock);
    let map = store.service_map(all());
    assert!(kinds_present(&map, External).is_empty());
    assert_eq!(edges(&map), vec![("a", "b", 1, 0)]);
}

#[test]
fn a_child_in_the_same_service_does_not_match_the_remote_call() {
    let (store, clock) = store_with_manual_clock();
    store.ingest_spans(vec![
        with_attr(sp(Client, 1, 1, None, "a", 0), "server.address", "gateway"),
        sp(Internal, 1, 2, Some(1), "a", 1),
    ]);
    settle(&clock);
    assert_eq!(
        kinds_present(&store.service_map(all()), External),
        vec!["gateway"]
    );
}

#[test]
fn only_client_spans_make_external_nodes() {
    let (store, clock) = store_with_manual_clock();
    let addressed = |kind: SpanKind, trace: u8| {
        with_attr(
            sp(kind, trace, 1, None, "a", 0),
            "server.address",
            "somewhere",
        )
    };
    store.ingest_spans(vec![
        addressed(Server, 1),
        addressed(Internal, 2),
        addressed(Consumer, 3),
        addressed(Unspecified, 4),
    ]);
    settle(&clock);
    assert!(kinds_present(&store.service_map(all()), External).is_empty());
}

#[test]
fn the_external_edge_is_windowed_by_the_client_span() {
    let (store, clock) = store_with_manual_clock();
    store.ingest_spans(vec![
        with_attr(sp(Client, 1, 1, None, "a", 0), "server.address", "old-host"),
        with_attr(
            sp(Client, 2, 1, None, "a", 100),
            "server.address",
            "new-host",
        ),
    ]);
    settle(&clock);
    let map = store.service_map(since_ms(50));
    assert_eq!(kinds_present(&map, External), vec!["new-host"]);
    assert_eq!(edges(&map), vec![("a", "new-host", 1, 0)]);
}

// --- windows ---

#[test]
fn a_call_counts_only_when_both_of_its_spans_start_in_the_window() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_spans(vec![
        sp(Server, 1, 1, None, "a", 20_000),
        sp(Client, 1, 2, Some(1), "a", 20_001),
        sp(Server, 1, 3, Some(2), "b", 20_005),
    ]);
    // The window opens between the client call and the server span: the
    // server span is in it, but the call began before it. The map never has
    // an edge whose end is missing.
    let map = store.service_map(since_ms(20_003));
    assert_eq!(nodes(&map), vec![("b", Service, 1, 0)]);
    assert!(map.edges.is_empty());
    assert_eq!(store.service_map(since_ms(20_001)).edges.len(), 1);
}

#[test]
fn a_trace_that_ends_before_the_window_is_skipped() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_spans(vec![
        sp(Server, 1, 1, None, "a", 0),
        sp(Server, 1, 2, Some(1), "a", 5),
    ]);
    assert_eq!(store.service_map(since_ms(1_000)), ServiceMap::default());
    assert!(store.services(since_ms(1_000)).is_empty());
}

#[test]
fn a_zero_length_span_that_starts_exactly_at_the_window_start_is_inside() {
    // The trace ends where the window begins: nothing may skip it early.
    let (store, _c) = store_with_manual_clock();
    store.ingest_spans(vec![with_kind(
        span(1, 1, None, "a", "instant", 100, 0),
        Server,
    )]);
    assert_eq!(
        nodes(&store.service_map(since_ms(100))),
        vec![("a", Service, 1, 0)]
    );
    assert_eq!(store.services(since_ms(100)).len(), 1);
    assert!(store.service_map(since_ms(101)).nodes.is_empty());
}

#[test]
fn a_trace_that_spans_the_window_counts_only_the_spans_inside() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_spans(vec![
        sp(Server, 1, 1, None, "a", 0),
        sp(Server, 1, 2, Some(1), "a", 50),
        sp(Server, 1, 3, Some(1), "a", 150),
    ]);
    let map = store.service_map(since_ms(100));
    assert_eq!(nodes(&map), vec![("a", Service, 1, 0)]);
}

// --- shape of the result ---

#[test]
fn an_empty_store_has_an_empty_map() {
    let (store, _c) = store_with_manual_clock();
    assert_eq!(store.service_map(all()), ServiceMap::default());
    assert!(store.services(all()).is_empty());
}

#[test]
fn clear_empties_the_map() {
    let (store, _c) = store_with_manual_clock();
    store.ingest_spans(vec![sp(Server, 1, 1, None, "a", 0)]);
    store.clear();
    assert_eq!(store.service_map(all()), ServiceMap::default());
    assert!(store.services(all()).is_empty());
}

#[test]
fn nodes_are_sorted_by_id_and_edges_by_from_then_to() {
    let (store, clock) = store_with_manual_clock();
    // Arrival order is the opposite of sorted order, and dependencies sort
    // in among the services by name.
    store.ingest_spans(vec![
        sp(Server, 1, 1, None, "zeta", 0),
        sp(Client, 1, 2, Some(1), "zeta", 1),
        sp(Server, 1, 3, Some(2), "alpha", 2),
        with_attr(
            sp(Client, 1, 4, Some(1), "zeta", 3),
            "db.system.name",
            "mid-db",
        ),
        sp(Client, 1, 5, Some(1), "zeta", 4),
        sp(Server, 1, 6, Some(5), "beta", 5),
    ]);
    settle(&clock);
    let map = store.service_map(all());
    let ids: Vec<&str> = map.nodes.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(ids, vec!["alpha", "beta", "mid-db", "zeta"]);
    assert_eq!(
        edges(&map),
        vec![
            ("zeta", "alpha", 1, 0),
            ("zeta", "beta", 1, 0),
            ("zeta", "mid-db", 1, 0),
        ]
    );
}

#[test]
fn services_are_the_service_nodes_of_the_map() {
    let (store, clock) = store_with_manual_clock();
    store.ingest_spans(vec![
        sp(Server, 1, 1, None, "web-bff", 0),
        sp(Client, 1, 2, Some(1), "web-bff", 1),
        sp(Server, 1, 3, Some(2), "orders-api", 2),
        error(with_attr(
            sp(Client, 1, 4, Some(3), "orders-api", 3),
            "db.system.name",
            "postgresql",
        )),
        with_attr(
            sp(Producer, 1, 5, Some(3), "orders-api", 4),
            "messaging.system",
            "rabbitmq",
        ),
        with_attr(
            sp(Client, 1, 6, Some(3), "orders-api", 5),
            "server.address",
            "card-gateway",
        ),
        sp(Server, 2, 1, None, "inventory-api", 500),
    ]);
    settle(&clock);
    for window in [
        all(),
        since_ms(1),
        since_ms(3),
        since_ms(400),
        since_ms(501),
    ] {
        let map = store.service_map(window);
        let from_map: Vec<(String, u64, u64)> = map
            .nodes
            .iter()
            .filter(|n| n.kind == Service)
            .map(|n| (n.id.clone(), n.requests, n.errors))
            .collect();
        let services: Vec<(String, u64, u64)> = store
            .services(window)
            .into_iter()
            .map(|s| (s.name, s.requests, s.errors))
            .collect();
        assert_eq!(services, from_map, "window {window:?}");
    }
}

#[test]
fn a_dependency_named_like_a_service_is_a_separate_node() {
    // Names are not unique across kinds: a broker instrumented as service
    // "kafka" and the queue "kafka" that producers write to are different
    // nodes with the same id. Nothing is merged and nothing panics; consumers
    // that key nodes by id must key by (id, kind). Service sorts first.
    let (store, _c) = store_with_manual_clock();
    store.ingest_spans(vec![
        sp(Server, 1, 1, None, "kafka", 0),
        with_attr(
            sp(Producer, 2, 1, None, "orders-api", 1),
            "messaging.system",
            "kafka",
        ),
    ]);
    let map = store.service_map(all());
    assert_eq!(
        nodes(&map),
        vec![
            ("kafka", Service, 1, 0),
            ("kafka", Queue, 1, 0),
            ("orders-api", Service, 0, 0),
        ]
    );
    assert_eq!(edges(&map), vec![("orders-api", "kafka", 1, 0)]);
}

#[test]
fn the_map_serializes_with_snake_case_kinds() {
    let (store, clock) = store_with_manual_clock();
    store.ingest_spans(vec![
        sp(Server, 1, 1, None, "a", 0),
        with_attr(
            sp(Client, 1, 2, Some(1), "a", 1),
            "db.system.name",
            "postgresql",
        ),
        with_attr(sp(Client, 1, 3, Some(1), "a", 2), "server.address", "ext"),
        with_attr(sp(Producer, 1, 4, Some(1), "a", 3), "messaging.system", "q"),
    ]);
    settle(&clock);
    let json = serde_json::to_value(store.service_map(all())).unwrap();
    assert_eq!(
        json["nodes"],
        serde_json::json!([
            {"id": "a", "kind": "service", "requests": 1, "errors": 0},
            {"id": "ext", "kind": "external", "requests": 1, "errors": 0},
            {"id": "postgresql", "kind": "database", "requests": 1, "errors": 0},
            {"id": "q", "kind": "queue", "requests": 1, "errors": 0},
        ])
    );
    assert_eq!(
        json["edges"][0],
        serde_json::json!({"from": "a", "to": "ext", "calls": 1, "errors": 0})
    );
    let stats = serde_json::to_value(store.services(all())).unwrap();
    assert_eq!(
        stats,
        serde_json::json!([
            {"name": "a", "requests": 1, "errors": 0, "last_seen_unix_nano": 3_000_000}
        ])
    );
}

// --- malformed input ---

#[test]
fn a_span_that_is_its_own_parent_is_survived() {
    let (store, clock) = store_with_manual_clock();
    store.ingest_spans(vec![sp(Server, 1, 1, Some(1), "a", 0)]);
    settle(&clock);
    let map = store.service_map(all());
    assert_eq!(nodes(&map), vec![("a", Service, 1, 0)]);
    assert!(map.edges.is_empty());
}

#[test]
fn a_parent_cycle_across_services_is_survived() {
    let (store, clock) = store_with_manual_clock();
    store.ingest_spans(vec![
        sp(Client, 1, 2, Some(3), "a", 0),
        sp(Client, 1, 3, Some(2), "b", 1),
    ]);
    settle(&clock);
    // Each is the other's parent and child: one call each way.
    let map = store.service_map(all());
    assert_eq!(edges(&map), vec![("a", "b", 1, 0), ("b", "a", 1, 0)]);
    assert!(kinds_present(&map, External).is_empty());
}

#[test]
fn a_deep_trace_does_not_overflow_the_stack() {
    let (store, clock) = store_with_manual_clock();
    let mut spans = Vec::new();
    let depth: u64 = 10_000;
    // Ten-thousand spans deep, alternating between two services. (Span IDs
    // are single bytes in the fixtures, so build the chain by hand.)
    for i in 0..depth {
        let service = if i % 2 == 0 { "even" } else { "odd" };
        let mut s = sp(Server, 1, 1, None, service, i);
        s.span_id = loctel_model::SpanId((i + 1).to_be_bytes());
        s.parent_span_id = (i > 0).then(|| loctel_model::SpanId(i.to_be_bytes()));
        spans.push(s);
    }
    store.ingest_spans(spans);
    settle(&clock);
    let map = store.service_map(all());
    assert_eq!(
        edges(&map),
        vec![
            ("even", "odd", depth / 2, 0),
            ("odd", "even", depth / 2 - 1, 0)
        ]
    );
}
