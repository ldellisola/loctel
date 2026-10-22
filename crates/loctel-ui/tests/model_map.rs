//! The service map layout (M3 plan, Task 2): which column and row each node
//! of the map gets, what its subtitle reads, which nodes and edges are
//! failing, and that the picture stays where it is as the data changes.
//!
//! The numbers in the assertions are the contract, not the constants of the
//! implementation: a node is 176 by 64 px, the columns start at x = 40 and are
//! 272 px apart (176 + a 96 px gap), the rows start at y = 32 and are 96 px
//! apart (64 + a 32 px gap).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use loctel_ingest::spans_from_request;
use loctel_meshgen::{Scenario, generate};
use loctel_store::{
    ManualClock, NodeKind, ServiceEdge, ServiceMap, ServiceNode, Store, StoreConfig, TimeWindow,
};
use loctel_ui::model::colors::ServiceColors;
use loctel_ui::model::map_layout::{LayoutMemory, MapLayout, PlacedEdge, PlacedNode, layout};

/// 2027-01-15T08:00:00Z. The failing checkout starts here.
const BASE: u64 = 1_800_000_000_000_000_000;
const SEED: u64 = 7;

/// The window the hand-made maps are read over, so a node with `n * 60`
/// requests has a rate of `n`.
const WINDOW: f64 = 60.0;

// --- fixtures ----------------------------------------------------------------

/// The service map of the failing checkout, as the store computes it once the
/// trace is complete (so `card-gateway` is a node): 9 nodes and 9 edges.
fn checkout_map() -> ServiceMap {
    let clock = Arc::new(ManualClock::new(BASE));
    let store = Store::new(StoreConfig::default(), clock.clone());
    let generated = generate(&Scenario::CheckoutFailure, SEED, BASE);
    for batch in &generated.batches {
        if let Some(request) = &batch.traces {
            let converted = spans_from_request(request.clone());
            assert_eq!(converted.rejected, 0, "generated spans are valid");
            store.ingest_spans(converted.items);
        }
    }
    // Past the 10 s quiet period.
    clock.advance(Duration::from_secs(60));
    store.service_map(TimeWindow::default())
}

fn node(id: &str, kind: NodeKind, requests: u64, errors: u64) -> ServiceNode {
    ServiceNode {
        id: id.to_owned(),
        kind,
        requests,
        errors,
    }
}

/// A healthy service node.
fn svc(id: &str) -> ServiceNode {
    node(id, NodeKind::Service, 600, 0)
}

fn edge(from: &str, to: &str) -> ServiceEdge {
    ServiceEdge {
        from: from.to_owned(),
        to: to.to_owned(),
        calls: 600,
        errors: 0,
    }
}

/// A map the way the store hands one out: nodes by `(id, kind)`, edges by
/// `(from, to)`.
fn map_of(mut nodes: Vec<ServiceNode>, mut edges: Vec<ServiceEdge>) -> ServiceMap {
    nodes.sort_by(|a, b| (&a.id, a.kind).cmp(&(&b.id, b.kind)));
    edges.sort_by(|a, b| (&a.from, &a.to).cmp(&(&b.from, &b.to)));
    ServiceMap { nodes, edges }
}

/// A map of healthy services and the calls between them.
fn services(ids: &[&str], calls: &[(&str, &str)]) -> ServiceMap {
    map_of(
        ids.iter().map(|id| svc(id)).collect(),
        calls.iter().map(|(from, to)| edge(from, to)).collect(),
    )
}

/// One refresh with fresh colours.
fn lay(map: &ServiceMap, memory: &mut LayoutMemory) -> MapLayout {
    layout(map, WINDOW, &mut ServiceColors::new(), memory)
}

/// One layout from nothing.
fn fresh(map: &ServiceMap) -> MapLayout {
    lay(map, &mut LayoutMemory::default())
}

fn at<'a>(laid: &'a MapLayout, id: &str) -> &'a PlacedNode {
    let mut found = laid.nodes.iter().filter(|node| node.id == id);
    let first = found
        .next()
        .unwrap_or_else(|| panic!("no node {id} in {:?}", ids(laid)));
    assert!(found.next().is_none(), "{id} is placed twice");
    first
}

fn ids(laid: &MapLayout) -> Vec<&str> {
    laid.nodes.iter().map(|node| node.id.as_str()).collect()
}

/// The column of a node, which must sit exactly on the formula.
fn layer_of(node: &PlacedNode) -> usize {
    let layer = (node.x - 40.0) / 272.0;
    assert_eq!(layer.fract(), 0.0, "x is 40 + layer * 272: {node:?}");
    layer as usize
}

/// The row of a node, which must sit exactly on the formula.
fn slot_of(node: &PlacedNode) -> usize {
    let slot = (node.y - 32.0) / 96.0;
    assert_eq!(slot.fract(), 0.0, "y is 32 + slot * 96: {node:?}");
    slot as usize
}

/// `(column, row)` of the node `id`.
fn cell(laid: &MapLayout, id: &str) -> (usize, usize) {
    let node = at(laid, id);
    (layer_of(node), slot_of(node))
}

/// Where every node is, by id.
fn cells(laid: &MapLayout) -> HashMap<String, (usize, usize)> {
    laid.nodes
        .iter()
        .map(|node| (node.id.clone(), (layer_of(node), slot_of(node))))
        .collect()
}

fn edge_of<'a>(laid: &'a MapLayout, from: &str, to: &str) -> &'a PlacedEdge {
    laid.edges
        .iter()
        .find(|edge| edge.from == from && edge.to == to)
        .unwrap_or_else(|| panic!("no edge {from} to {to}"))
}

/// No two nodes share a place, and every node sits on the formula.
fn assert_well_placed(laid: &MapLayout) {
    let mut seen = HashSet::new();
    for node in &laid.nodes {
        let place = (layer_of(node), slot_of(node));
        assert!(seen.insert(place), "two nodes at {place:?}: {laid:?}");
        assert_eq!((node.w, node.h), (176.0, 64.0), "{node:?}");
    }
}

// --- the checkout --------------------------------------------------------------

#[test]
fn checkout_topology_columns() {
    let map = checkout_map();
    assert_eq!(map.nodes.len(), 9, "{map:?}");
    assert_eq!(map.edges.len(), 9, "{map:?}");
    let laid = fresh(&map);
    assert_eq!(laid.nodes.len(), 9);
    assert_well_placed(&laid);

    // Columns come from the longest path from the entry service: `web-bff`
    // calls `orders-api`, which calls three things, and so on down to the
    // databases, the gateway and the worker.
    //
    // Rows within a column follow the middle of the nodes that call them
    // (ties by id): `redis` is below `inventory-api`, `postgresql` between its
    // two callers `orders-api` and `payments-api`, `card-gateway` below
    // `payments-api` and `notifications-worker` below `rabbitmq`, so the
    // lines do not cross.
    let expected: [(&str, NodeKind, usize, usize); 9] = [
        ("web-bff", NodeKind::Service, 0, 0),
        ("orders-api", NodeKind::Service, 1, 0),
        ("inventory-api", NodeKind::Service, 2, 0),
        ("payments-api", NodeKind::Service, 2, 1),
        ("rabbitmq", NodeKind::Queue, 2, 2),
        ("redis", NodeKind::Database, 3, 0),
        ("postgresql", NodeKind::Database, 3, 1),
        ("card-gateway", NodeKind::External, 3, 2),
        ("notifications-worker", NodeKind::Service, 3, 3),
    ];
    for (id, kind, layer, slot) in expected {
        let placed = at(&laid, id);
        assert_eq!(placed.kind, kind, "{id}");
        assert_eq!(placed.x, 40.0 + layer as f32 * (176.0 + 96.0), "x of {id}");
        assert_eq!(placed.y, 32.0 + slot as f32 * (64.0 + 32.0), "y of {id}");
        assert_eq!((placed.w, placed.h), (176.0, 64.0), "{id}");
    }

    // The canvas is the nodes plus the 40 px and 32 px margins all round:
    // four columns and four rows.
    assert_eq!(laid.width, 40.0 + 3.0 * 272.0 + 176.0 + 40.0);
    assert_eq!(laid.height, 32.0 + 3.0 * 96.0 + 64.0 + 32.0);

    // One placed edge per edge of the map, in the map's order.
    let pairs: Vec<(&str, &str)> = laid
        .edges
        .iter()
        .map(|edge| (edge.from.as_str(), edge.to.as_str()))
        .collect();
    let wanted: Vec<(&str, &str)> = map
        .edges
        .iter()
        .map(|edge| (edge.from.as_str(), edge.to.as_str()))
        .collect();
    assert_eq!(pairs, wanted);
}

#[test]
fn failing_flags() {
    let map = checkout_map();
    let laid = fresh(&map);

    // A node fails when it counted an error: the three services whose entry
    // span is erroring, and the gateway whose two calls timed out. The
    // databases, the broker, and the services that answered are fine.
    for id in ["web-bff", "orders-api", "payments-api", "card-gateway"] {
        assert!(at(&laid, id).failing, "{id} is failing");
    }
    for id in [
        "inventory-api",
        "notifications-worker",
        "postgresql",
        "redis",
        "rabbitmq",
    ] {
        assert!(!at(&laid, id).failing, "{id} is fine");
    }

    // An edge fails when a call along it errored: the whole chain down to
    // the payment and the gateway, nothing else.
    for (from, to) in [
        ("web-bff", "orders-api"),
        ("orders-api", "payments-api"),
        ("payments-api", "card-gateway"),
    ] {
        let placed = edge_of(&laid, from, to);
        assert!(placed.failing, "{from} to {to} is failing");
        assert!(placed.errors > 0, "{from} to {to}");
    }
    assert!(edge_of(&laid, "orders-api", "payments-api").failing);
    for (from, to) in [
        ("orders-api", "inventory-api"),
        ("orders-api", "postgresql"),
        ("orders-api", "rabbitmq"),
        ("inventory-api", "redis"),
        ("payments-api", "postgresql"),
        ("rabbitmq", "notifications-worker"),
    ] {
        let placed = edge_of(&laid, from, to);
        assert!(!placed.failing, "{from} to {to} is fine");
        assert_eq!(placed.errors, 0, "{from} to {to}");
    }

    // The counts are the map's own.
    for placed in &laid.edges {
        let source = map
            .edges
            .iter()
            .find(|e| e.from == placed.from && e.to == placed.to)
            .unwrap();
        assert_eq!((placed.calls, placed.errors), (source.calls, source.errors));
        assert_eq!(placed.failing, source.errors > 0);
    }
}

#[test]
fn failing_is_exactly_errors_above_zero() {
    let map = map_of(
        vec![
            node("one-error", NodeKind::Service, 1_000_000, 1),
            node("clean", NodeKind::Service, 1_000_000, 0),
            node("db", NodeKind::Database, 5, 1),
            node("idle", NodeKind::Service, 0, 0),
        ],
        vec![ServiceEdge {
            from: "one-error".to_owned(),
            to: "db".to_owned(),
            calls: 5,
            errors: 1,
        }],
    );
    let laid = fresh(&map);
    assert!(at(&laid, "one-error").failing);
    assert!(at(&laid, "db").failing);
    assert!(!at(&laid, "clean").failing);
    assert!(!at(&laid, "idle").failing);
    assert!(edge_of(&laid, "one-error", "db").failing);
}

#[test]
fn checkout_subtitles() {
    // Over a one second window the counts are the rates.
    let laid = layout(
        &checkout_map(),
        1.0,
        &mut ServiceColors::new(),
        &mut LayoutMemory::default(),
    );
    let subtitle = |id: &str| at(&laid, id).subtitle.as_str();
    assert_eq!(subtitle("web-bff"), "1.0 req/s \u{b7} 100.0% err");
    assert_eq!(subtitle("orders-api"), "1.0 req/s \u{b7} 100.0% err");
    assert_eq!(subtitle("payments-api"), "1.0 req/s \u{b7} 100.0% err");
    assert_eq!(subtitle("inventory-api"), "1.0 req/s \u{b7} 0% err");
    assert_eq!(subtitle("notifications-worker"), "1.0 req/s \u{b7} 0% err");
    // A database, a queue and an external host count the calls they received.
    assert_eq!(subtitle("postgresql"), "database \u{b7} 2.0 q/s");
    assert_eq!(subtitle("redis"), "database \u{b7} 1.0 q/s");
    assert_eq!(subtitle("rabbitmq"), "queue \u{b7} 1.0 msg/s");
    assert_eq!(subtitle("card-gateway"), "external \u{b7} 2.0 req/s");
}

// --- subtitles and labels ---------------------------------------------------------

#[test]
fn subtitles_formats() {
    // Counts over a 60 s window, chosen to give the plan's examples.
    let map = map_of(
        vec![
            node("api", NodeKind::Service, 84 * 60, 106),
            node("pg", NodeKind::Database, 140 * 60, 0),
            node("mq", NodeKind::Queue, 12 * 60, 0),
            node("stripe", NodeKind::External, 25 * 60, 0),
            node("quiet", NodeKind::Service, 0, 0),
            node("slow", NodeKind::Service, 18, 0),
            node("db-with-errors", NodeKind::Database, 600, 60),
        ],
        vec![],
    );
    let laid = fresh(&map);
    let subtitle = |id: &str| at(&laid, id).subtitle.as_str();
    assert_eq!(subtitle("api"), "84 req/s \u{b7} 2.1% err");
    assert_eq!(subtitle("pg"), "database \u{b7} 140 q/s");
    assert_eq!(subtitle("mq"), "queue \u{b7} 12 msg/s");
    assert_eq!(subtitle("stripe"), "external \u{b7} 25 req/s");
    // The rest of the rules of `format`: a service with no request reads 0%,
    // and only services show an error share.
    assert_eq!(subtitle("quiet"), "0.0 req/s \u{b7} 0% err");
    assert_eq!(subtitle("slow"), "0.3 req/s \u{b7} 0% err");
    assert_eq!(subtitle("db-with-errors"), "database \u{b7} 10 q/s");
}

#[test]
fn subtitle_rates_follow_the_one_rate_rule() {
    // The plan's examples read `queue \u{b7} 9 msg/s` and `external \u{b7} 3 req/s`,
    // but the rate rule every view shares (`format::rate`, tested in
    // `model_format`) gives one decimal below 10, so 9 messages a second is
    // `9.0 msg/s`. The subtitle is that rule with the node's unit
    // (decision 0022).
    let map = map_of(
        vec![
            node("mq", NodeKind::Queue, 9 * 60, 0),
            node("stripe", NodeKind::External, 3 * 60, 0),
            node("pg", NodeKind::Database, 5 * 60, 0),
            node("api", NodeKind::Service, 3 * 60, 0),
        ],
        vec![],
    );
    let laid = fresh(&map);
    let subtitle = |id: &str| at(&laid, id).subtitle.as_str();
    assert_eq!(subtitle("mq"), "queue \u{b7} 9.0 msg/s");
    assert_eq!(subtitle("stripe"), "external \u{b7} 3.0 req/s");
    assert_eq!(subtitle("pg"), "database \u{b7} 5.0 q/s");
    assert_eq!(subtitle("api"), "3.0 req/s \u{b7} 0% err");
}

#[test]
fn rates_are_counts_over_the_window() {
    let map = map_of(vec![node("api", NodeKind::Service, 1_200, 0)], vec![]);
    let subtitle = |window: f64| {
        let laid = layout(
            &map,
            window,
            &mut ServiceColors::new(),
            &mut LayoutMemory::default(),
        );
        laid.nodes[0].subtitle.clone()
    };
    assert_eq!(subtitle(60.0), "20 req/s \u{b7} 0% err");
    assert_eq!(subtitle(300.0), "4.0 req/s \u{b7} 0% err");
    assert_eq!(subtitle(900.0), "1.3 req/s \u{b7} 0% err");
    // No window, or one that is not a length of time, is no rate and no panic.
    for window in [0.0, -5.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(subtitle(window), "0.0 req/s \u{b7} 0% err", "{window}");
    }
}

#[test]
fn edge_labels_only_on_edges_out_of_the_first_column() {
    let map = map_of(
        vec![svc("web"), svc("api"), svc("db")],
        vec![
            ServiceEdge {
                from: "web".to_owned(),
                to: "api".to_owned(),
                calls: 84 * 60,
                errors: 0,
            },
            ServiceEdge {
                from: "api".to_owned(),
                to: "db".to_owned(),
                calls: 30 * 60,
                errors: 0,
            },
        ],
    );
    let laid = fresh(&map);
    assert_eq!(cell(&laid, "web"), (0, 0));
    assert_eq!(cell(&laid, "api"), (1, 0));
    assert_eq!(
        edge_of(&laid, "web", "api").label.as_deref(),
        Some("84 req/s")
    );
    assert_eq!(edge_of(&laid, "api", "db").label, None);

    // Of the checkout, only the one edge out of `web-bff` has a label.
    let laid = layout(
        &checkout_map(),
        1.0,
        &mut ServiceColors::new(),
        &mut LayoutMemory::default(),
    );
    let labelled: Vec<(&str, &str)> = laid
        .edges
        .iter()
        .filter(|edge| edge.label.is_some())
        .map(|edge| (edge.from.as_str(), edge.to.as_str()))
        .collect();
    assert_eq!(labelled, vec![("web-bff", "orders-api")]);
    assert_eq!(
        edge_of(&laid, "web-bff", "orders-api").label.as_deref(),
        Some("1.0 req/s")
    );
}

#[test]
fn edge_label_follows_the_column_a_node_was_placed_in() {
    // `late` is first seen alone, so it takes column 0 and keeps it when a
    // call into it appears: the edge out of it is the edge out of column 0.
    let mut memory = LayoutMemory::default();
    lay(&services(&["late"], &[]), &mut memory);
    let laid = lay(
        &services(&["early", "late", "x"], &[("early", "late"), ("late", "x")]),
        &mut memory,
    );
    assert_eq!(cell(&laid, "late").0, 0);
    assert!(edge_of(&laid, "late", "x").label.is_some());
}

// --- colours ------------------------------------------------------------------------

#[test]
fn colors_come_from_the_shared_session_order() {
    let map = checkout_map();
    let mut colors = ServiceColors::new();
    // The trace table met this one first.
    assert_eq!(colors.index("payments-api"), 0);

    let laid = layout(&map, WINDOW, &mut colors, &mut LayoutMemory::default());
    // Services use the number they have, and take the next free ones in the
    // order of the picture (left to right, top to bottom) when new. Databases,
    // queues and external hosts have no colour of their own and take no
    // number.
    let color = |id: &str| at(&laid, id).color;
    assert_eq!(color("payments-api"), Some(0));
    assert_eq!(color("web-bff"), Some(1));
    assert_eq!(color("orders-api"), Some(2));
    assert_eq!(color("inventory-api"), Some(3));
    assert_eq!(color("notifications-worker"), Some(4));
    for id in ["postgresql", "redis", "rabbitmq", "card-gateway"] {
        assert_eq!(color(id), None, "{id}");
        assert_eq!(colors.peek(id), None, "{id} takes no number");
    }
    assert_eq!(colors.peek("notifications-worker"), Some(4));

    // A second refresh changes nothing.
    let again = layout(&map, WINDOW, &mut colors, &mut LayoutMemory::default());
    assert_eq!(again, laid);
}

#[test]
fn a_fresh_session_numbers_services_in_the_order_of_the_picture() {
    let laid = fresh(&checkout_map());
    let colored: Vec<(&str, Option<usize>)> = laid
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node.color))
        .collect();
    assert_eq!(
        colored,
        vec![
            ("web-bff", Some(0)),
            ("orders-api", Some(1)),
            ("inventory-api", Some(2)),
            ("payments-api", Some(3)),
            ("rabbitmq", None),
            ("redis", None),
            ("postgresql", None),
            ("card-gateway", None),
            ("notifications-worker", Some(4)),
        ]
    );
}

// --- cycles, single nodes, empty maps -------------------------------------------------

#[test]
fn cycle_terminates_and_places_all() {
    // a and b call each other, c stands apart.
    let map = services(&["a", "b", "c"], &[("a", "b"), ("b", "a")]);
    let laid = fresh(&map);
    assert_eq!(laid.nodes.len(), 3);
    assert_well_placed(&laid);
    let mut placed = ids(&laid);
    placed.sort_unstable();
    assert_eq!(placed, vec!["a", "b", "c"], "every node exactly once");
    // Both calls are still drawn.
    assert_eq!(laid.edges.len(), 2);
    assert!(laid.edges.iter().any(|e| (&*e.from, &*e.to) == ("a", "b")));
    assert!(laid.edges.iter().any(|e| (&*e.from, &*e.to) == ("b", "a")));

    // The loop is broken at the edge the search closes it with: `c` has no
    // caller, so the search starts there, then at `a`, and `b` calling back
    // into `a` is the back edge that is ignored.
    assert_eq!(cell(&laid, "a"), (0, 0));
    assert_eq!(cell(&laid, "c"), (0, 1));
    assert_eq!(cell(&laid, "b"), (1, 0));
}

#[test]
fn bigger_cycles_and_self_calls_terminate() {
    // A ring with no entry point at all: the search starts at the first id.
    let ring = services(&["a", "b", "c"], &[("a", "b"), ("b", "c"), ("c", "a")]);
    let laid = fresh(&ring);
    assert_well_placed(&laid);
    assert_eq!(cell(&laid, "a"), (0, 0));
    assert_eq!(cell(&laid, "b"), (1, 0));
    assert_eq!(cell(&laid, "c"), (2, 0));
    assert_eq!(laid.edges.len(), 3);

    // A service calling itself (a call between two kinds that share a name
    // looks like this) is one node, one edge, no loop in the layers.
    let own = services(&["a", "b"], &[("a", "a"), ("a", "b")]);
    let laid = fresh(&own);
    assert_well_placed(&laid);
    assert_eq!(cell(&laid, "a"), (0, 0));
    assert_eq!(cell(&laid, "b"), (1, 0));
    assert_eq!(laid.edges.len(), 2);

    // Two rings joined, and a loop hanging off an entry.
    let tangle = services(
        &["a", "b", "c", "d", "e", "f"],
        &[
            ("a", "b"),
            ("b", "c"),
            ("c", "a"),
            ("c", "d"),
            ("d", "e"),
            ("e", "d"),
            ("f", "a"),
        ],
    );
    let laid = fresh(&tangle);
    assert_eq!(laid.nodes.len(), 6);
    assert_well_placed(&laid);
}

#[test]
fn single_node() {
    let map = services(&["only"], &[]);
    let laid = fresh(&map);
    assert_eq!(laid.nodes.len(), 1);
    assert!(laid.edges.is_empty());
    let only = &laid.nodes[0];
    assert_eq!(only.id, "only");
    assert_eq!((only.x, only.y), (40.0, 32.0));
    assert_eq!((only.w, only.h), (176.0, 64.0));
    assert_eq!(only.color, Some(0));
    assert_eq!(only.subtitle, "10 req/s \u{b7} 0% err");
    assert!(!only.failing);
    // One node and the margins round it.
    assert_eq!(
        (laid.width, laid.height),
        (40.0 + 176.0 + 40.0, 32.0 + 64.0 + 32.0)
    );
}

#[test]
fn empty_map_is_an_empty_layout() {
    let laid = fresh(&ServiceMap::default());
    assert!(laid.nodes.is_empty());
    assert!(laid.edges.is_empty());
    assert_eq!((laid.width, laid.height), (0.0, 0.0));
}

#[test]
fn nodes_with_no_edges_stack_in_the_first_column_by_id() {
    let map = services(&["c", "a", "b"], &[]);
    let laid = fresh(&map);
    assert_eq!(cell(&laid, "a"), (0, 0));
    assert_eq!(cell(&laid, "b"), (0, 1));
    assert_eq!(cell(&laid, "c"), (0, 2));
}

// --- the shape of a fresh layout --------------------------------------------------------

#[test]
fn layers_are_the_longest_path_from_the_sources() {
    // A diamond with a shortcut: `d` is reached by a path of two calls and by
    // the shortcut of one, and sits at the end of the longer.
    let map = services(
        &["a", "b", "c", "d", "e"],
        &[("a", "b"), ("a", "c"), ("b", "d"), ("c", "d"), ("a", "d")],
    );
    let laid = fresh(&map);
    assert_eq!(cell(&laid, "a"), (0, 0));
    assert_eq!(cell(&laid, "b"), (1, 0));
    assert_eq!(cell(&laid, "c"), (1, 1));
    assert_eq!(cell(&laid, "d"), (2, 0));
    // Not called by anyone, so a source whatever else is around.
    assert_eq!(cell(&laid, "e"), (0, 1));
}

#[test]
fn rows_follow_the_barycentre_of_the_callers_and_ties_go_by_id() {
    // `x` and `y` are called from the lower of the two entries and `z` from
    // the upper one, so the order of the second column is not the order of
    // the ids.
    let map = services(
        &["entry-1", "entry-2", "x", "y", "z"],
        &[("entry-1", "z"), ("entry-2", "x"), ("entry-2", "y")],
    );
    let laid = fresh(&map);
    assert_eq!(cell(&laid, "entry-1"), (0, 0));
    assert_eq!(cell(&laid, "entry-2"), (0, 1));
    assert_eq!(cell(&laid, "z"), (1, 0));
    assert_eq!(cell(&laid, "x"), (1, 1));
    assert_eq!(cell(&laid, "y"), (1, 2));
}

#[test]
fn a_node_is_placed_after_the_middle_of_all_its_callers() {
    // `join` is called from rows 0 and 2 of the first column: its middle is
    // row 1, after `left` (called from row 0) and before `right` (row 2).
    let map = services(
        &["a", "b", "c", "left", "join", "right"],
        &[("a", "left"), ("a", "join"), ("c", "join"), ("c", "right")],
    );
    let laid = fresh(&map);
    assert_eq!(cell(&laid, "left"), (1, 0));
    assert_eq!(cell(&laid, "join"), (1, 1));
    assert_eq!(cell(&laid, "right"), (1, 2));
}

#[test]
fn edges_to_nodes_that_are_not_there_are_dropped() {
    let map = ServiceMap {
        nodes: vec![svc("a")],
        edges: vec![edge("a", "ghost"), edge("ghost", "a")],
    };
    let laid = fresh(&map);
    assert_eq!(laid.nodes.len(), 1);
    assert!(laid.edges.is_empty(), "nothing to draw them to: {laid:?}");
}

#[test]
fn ids_shared_by_two_kinds_are_two_nodes() {
    // A service and a database both called `payments`. The store's edges carry
    // names only, so the edge into `payments` means the first of them (the
    // service); the database is a node of its own with no line to it.
    let map = map_of(
        vec![
            svc("api"),
            svc("payments"),
            node("payments", NodeKind::Database, 600, 0),
        ],
        vec![edge("api", "payments")],
    );
    let laid = fresh(&map);
    assert_eq!(laid.nodes.len(), 3);
    assert_well_placed(&laid);
    let kinds: Vec<NodeKind> = laid
        .nodes
        .iter()
        .filter(|n| n.id == "payments")
        .map(|n| n.kind)
        .collect();
    assert_eq!(kinds.len(), 2);
    let service = laid
        .nodes
        .iter()
        .find(|n| n.id == "payments" && n.kind == NodeKind::Service)
        .unwrap();
    assert_eq!(layer_of(service), 1, "the edge goes to the service");
    let database = laid
        .nodes
        .iter()
        .find(|n| n.id == "payments" && n.kind == NodeKind::Database)
        .unwrap();
    assert_eq!(layer_of(database), 0);
    assert_eq!(database.color, None);
    assert_eq!(service.color, Some(1));
}

#[test]
fn the_order_of_the_input_does_not_matter() {
    let map = checkout_map();
    let mut shuffled = map.clone();
    shuffled.nodes.reverse();
    shuffled.edges.reverse();
    assert_eq!(fresh(&shuffled), fresh(&map));
}

#[test]
fn deterministic() {
    let map = checkout_map();
    let first = fresh(&map);
    for _ in 0..5 {
        assert_eq!(fresh(&map), first, "the same input, the same layout");
    }
    // And over a run of refreshes with the same memory behaviour.
    let run = || {
        let mut memory = LayoutMemory::default();
        let mut colors = ServiceColors::new();
        let small = services(&["web", "api"], &[("web", "api")]);
        let big = services(&["web", "api", "db"], &[("web", "api"), ("api", "db")]);
        vec![
            layout(&small, WINDOW, &mut colors, &mut memory),
            layout(&big, WINDOW, &mut colors, &mut memory),
            layout(&small, WINDOW, &mut colors, &mut memory),
            layout(&big, WINDOW, &mut colors, &mut memory),
        ]
    };
    assert_eq!(run(), run());
}

// --- stability --------------------------------------------------------------------------

#[test]
fn new_node_does_not_move_existing_ones() {
    // The checkout, found in the order the spans of a real mesh arrive in:
    // callers before callees, but not in the order of the ids.
    let arrival = [
        "web-bff",
        "orders-api",
        "postgresql",
        "payments-api",
        "card-gateway",
        "inventory-api",
        "redis",
        "rabbitmq",
        "notifications-worker",
    ];
    let full = checkout_map();
    let mut memory = LayoutMemory::default();
    let mut colors = ServiceColors::new();
    let mut before: HashMap<String, (usize, usize)> = HashMap::new();
    for count in 1..=arrival.len() {
        let known: HashSet<&str> = arrival[..count].iter().copied().collect();
        let map = ServiceMap {
            nodes: full
                .nodes
                .iter()
                .filter(|n| known.contains(n.id.as_str()))
                .cloned()
                .collect(),
            edges: full
                .edges
                .iter()
                .filter(|e| known.contains(e.from.as_str()) && known.contains(e.to.as_str()))
                .cloned()
                .collect(),
        };
        let laid = layout(&map, WINDOW, &mut colors, &mut memory);
        assert_eq!(laid.nodes.len(), count);
        assert_well_placed(&laid);
        let now = cells(&laid);
        for (id, place) in &before {
            assert_eq!(
                now[id],
                *place,
                "{id} moved when {} arrived",
                arrival[count - 1]
            );
        }
        before = now;
    }
}

#[test]
fn a_new_node_takes_the_first_free_place_of_its_column() {
    let mut memory = LayoutMemory::default();
    let first = lay(
        &services(&["a", "b", "c"], &[("a", "b"), ("a", "c")]),
        &mut memory,
    );
    assert_eq!(cell(&first, "b"), (1, 0));
    assert_eq!(cell(&first, "c"), (1, 1));

    // `aa` sorts before `b` and `c` and is called by `a`, but they do not
    // make room for it: it goes to the next free row.
    let next = lay(
        &services(
            &["a", "aa", "b", "c"],
            &[("a", "aa"), ("a", "b"), ("a", "c")],
        ),
        &mut memory,
    );
    assert_eq!(cell(&next, "aa"), (1, 2));
    assert_eq!(cell(&next, "b"), (1, 0));
    assert_eq!(cell(&next, "c"), (1, 1));
    assert_eq!(next.height, 32.0 + 2.0 * 96.0 + 64.0 + 32.0);
}

#[test]
fn several_new_nodes_in_a_column_go_by_barycentre_then_id() {
    let mut memory = LayoutMemory::default();
    let first = lay(&services(&["a", "b", "x"], &[("a", "x")]), &mut memory);
    assert_eq!(cell(&first, "x"), (1, 0));
    // `b` (row 1 of the first column) now calls `p` and `q`, and `a` (row 0)
    // calls `r`: `r` comes first in the free rows because its caller is
    // higher, then `p` and `q` by id.
    let laid = lay(
        &services(
            &["a", "b", "x", "p", "q", "r"],
            &[("a", "x"), ("a", "r"), ("b", "p"), ("b", "q")],
        ),
        &mut memory,
    );
    assert_eq!(cell(&laid, "x"), (1, 0));
    assert_eq!(cell(&laid, "r"), (1, 1));
    assert_eq!(cell(&laid, "p"), (1, 2));
    assert_eq!(cell(&laid, "q"), (1, 3));
}

#[test]
fn a_remembered_node_keeps_its_column_when_a_longer_path_appears() {
    let mut memory = LayoutMemory::default();
    let first = lay(
        &services(&["a", "b", "db"], &[("a", "b"), ("a", "db")]),
        &mut memory,
    );
    assert_eq!(cell(&first, "db"), (1, 1));
    // `b` now calls `db` too, which on its own would put `db` in column 2.
    // It stays where it was: a map that moves things when a call appears is
    // worse than one with a line that runs sideways.
    let later = lay(
        &services(&["a", "b", "db"], &[("a", "b"), ("a", "db"), ("b", "db")]),
        &mut memory,
    );
    assert_eq!(cell(&later, "db"), (1, 1));
    assert_eq!(cell(&later, "b"), (1, 0));
    // From nothing, it would be in column 2.
    let from_scratch = fresh(&services(
        &["a", "b", "db"],
        &[("a", "b"), ("a", "db"), ("b", "db")],
    ));
    assert_eq!(cell(&from_scratch, "db"), (2, 0));
}

#[test]
fn a_node_that_is_gone_leaves_its_place_empty() {
    // Nobody else moves to fill a gap.
    let mut memory = LayoutMemory::default();
    lay(
        &services(&["a", "b", "c"], &[("a", "b"), ("a", "c")]),
        &mut memory,
    );
    let laid = lay(&services(&["a", "c"], &[("a", "c")]), &mut memory);
    assert_eq!(cell(&laid, "c"), (1, 1));
    assert_eq!(ids(&laid).len(), 2, "the missing node is not drawn");
    assert!(!ids(&laid).contains(&"b"));
    assert_eq!(laid.edges.len(), 1);
}

/// Refresh one of the gap tests: `a` calls `b` and `c`, which take rows 0 and
/// 1 of column 1.
fn start() -> LayoutMemory {
    let mut memory = LayoutMemory::default();
    let first = lay(
        &services(&["a", "b", "c"], &[("a", "b"), ("a", "c")]),
        &mut memory,
    );
    assert_eq!(cell(&first, "b"), (1, 0));
    assert_eq!(cell(&first, "c"), (1, 1));
    memory
}

/// `a` calling `c` and every one of `others`, but not `b`.
fn without_b(others: &[&str]) -> ServiceMap {
    let mut ids = vec!["a", "c"];
    ids.extend_from_slice(others);
    let calls: Vec<(&str, &str)> = ids[1..].iter().map(|id| ("a", *id)).collect();
    services(&ids, &calls)
}

/// `a` calling `b`, `c` and every one of `others`.
fn with_b(others: &[&str]) -> ServiceMap {
    let mut ids = vec!["a", "b", "c"];
    ids.extend_from_slice(others);
    let calls: Vec<(&str, &str)> = ids[1..].iter().map(|id| ("a", *id)).collect();
    services(&ids, &calls)
}

#[test]
fn missing_node_kept_for_three_refreshes() {
    // `b` is missing for three refreshes in a row. Its row is not given away:
    // the new node `d` of the first of them goes to the next row, and so does
    // the new node `e` of the third. When `b` comes back it is where it was.
    let mut memory = start();
    let first = lay(&without_b(&["d"]), &mut memory);
    assert_eq!(cell(&first, "c"), (1, 1), "nobody closes the gap");
    assert_eq!(cell(&first, "d"), (1, 2), "`b`'s row is still `b`'s");
    assert!(!ids(&first).contains(&"b"), "a missing node is not drawn");
    assert_eq!(first.edges.len(), 2);

    let second = lay(&without_b(&["d"]), &mut memory);
    assert_eq!(cell(&second, "d"), (1, 2));

    let third = lay(&without_b(&["d", "e"]), &mut memory);
    assert_eq!(
        cell(&third, "e"),
        (1, 3),
        "the third refresh still keeps it"
    );
    assert!(!ids(&third).contains(&"b"));

    let back = lay(&with_b(&["d", "e"]), &mut memory);
    assert_eq!(cell(&back, "b"), (1, 0), "back where it was");
    assert_eq!(cell(&back, "c"), (1, 1));
    assert_eq!(cell(&back, "d"), (1, 2));
    assert_eq!(cell(&back, "e"), (1, 3));
}

#[test]
fn missing_node_is_forgotten_after_three_refreshes() {
    // The fourth refresh without `b` forgets it: a node that arrives then
    // takes its row, and `b`, if it ever returns, is a new node like any
    // other and goes to the first free row.
    let mut memory = start();
    for _ in 0..3 {
        let kept = lay(&without_b(&["d"]), &mut memory);
        assert_eq!(cell(&kept, "d"), (1, 2));
    }
    let fourth = lay(&without_b(&["d", "e"]), &mut memory);
    assert_eq!(cell(&fourth, "e"), (1, 0), "`b`'s old row is free now");
    assert_eq!(cell(&fourth, "c"), (1, 1));
    assert_eq!(cell(&fourth, "d"), (1, 2));

    let back = lay(&with_b(&["d", "e"]), &mut memory);
    assert_eq!(cell(&back, "b"), (1, 3), "forgotten, so a new node");
    assert_eq!(cell(&back, "e"), (1, 0));
}

#[test]
fn a_node_that_comes_back_starts_the_count_again() {
    // Away for three refreshes, back, away for three, back, and again: never
    // away for four in a row, so never forgotten, however often it goes. The
    // new node of each third refresh must not take its row.
    let mut memory = start();
    let mut others: Vec<String> = Vec::new();
    for round in 0..3 {
        let name = format!("x{round}");
        for refresh in 0..3 {
            if refresh == 2 {
                others.push(name.clone());
            }
            let refs: Vec<&str> = others.iter().map(String::as_str).collect();
            let laid = lay(&without_b(&refs), &mut memory);
            if refresh == 2 {
                assert_eq!(cell(&laid, &name), (1, 2 + round), "{name}");
            }
        }
        let refs: Vec<&str> = others.iter().map(String::as_str).collect();
        let back = lay(&with_b(&refs), &mut memory);
        assert_eq!(cell(&back, "b"), (1, 0), "round {round}");
    }
}

#[test]
fn loops_are_broken_below_the_entry_points() {
    // `z` is the entry point. `a` and `b` call each other, and `z` calls `b`.
    // Searching from the entry first, `a` calling back into `b` is the call
    // that closes the loop, so `b` is the first column after `z` and `a` the
    // next: the picture reads from the entry. Starting the search from `a`
    // because its id sorts first would make `a` look like an entry.
    let map = services(&["a", "b", "z"], &[("z", "b"), ("b", "a"), ("a", "b")]);
    let laid = fresh(&map);
    assert_eq!(cell(&laid, "z"), (0, 0));
    assert_eq!(cell(&laid, "b"), (1, 0));
    assert_eq!(cell(&laid, "a"), (2, 0));
    assert_eq!(laid.edges.len(), 3, "all three calls are still drawn");
}

// --- every promise at once, on many maps ------------------------------------------------

/// A small deterministic generator (xorshift64*), so the maps are the same on
/// every run and every machine.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn kind_of(n: usize) -> NodeKind {
    [
        NodeKind::Service,
        NodeKind::Database,
        NodeKind::Queue,
        NodeKind::External,
    ][n % 4]
}

/// `count` nodes `n00`, `n01`, ... of random kinds and about `2 * count` random
/// calls between them, with loops, self-calls and repeats allowed (repeats
/// are merged, as a map has one edge per pair).
fn random_map(rng: &mut Rng, count: usize) -> ServiceMap {
    let nodes: Vec<ServiceNode> = (0..count)
        .map(|i| {
            let kind = kind_of(rng.below(4));
            node(
                &format!("n{i:02}"),
                kind,
                rng.below(5_000) as u64,
                rng.below(3) as u64,
            )
        })
        .collect();
    let mut pairs = HashSet::new();
    for _ in 0..count * 2 {
        pairs.insert((rng.below(count), rng.below(count)));
    }
    let edges = pairs
        .into_iter()
        .map(|(from, to)| ServiceEdge {
            from: format!("n{from:02}"),
            to: format!("n{to:02}"),
            calls: 1 + rng.below(100) as u64,
            errors: rng.below(2) as u64,
        })
        .collect();
    map_of(nodes, edges)
}

#[test]
fn random_maps_always_terminate_and_place_every_node_once() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for round in 0..300 {
        let count = 1 + rng.below(30);
        let map = random_map(&mut rng, count);
        let laid = fresh(&map);

        assert_eq!(laid.nodes.len(), count, "round {round}: {map:?}");
        assert_well_placed(&laid);
        let placed: HashSet<&str> = ids(&laid).into_iter().collect();
        assert_eq!(placed.len(), count, "round {round}");
        assert_eq!(laid.edges.len(), map.edges.len(), "round {round}");
        for edge in &laid.edges {
            assert!(placed.contains(edge.from.as_str()), "round {round}");
            assert!(placed.contains(edge.to.as_str()), "round {round}");
        }
        // The canvas holds every node.
        for node in &laid.nodes {
            assert!(node.x + node.w + 40.0 <= laid.width, "round {round}");
            assert!(node.y + node.h + 32.0 <= laid.height, "round {round}");
        }
        assert_eq!(fresh(&map), laid, "round {round}: deterministic");
    }
}

#[test]
fn random_maps_that_grow_never_move_a_node() {
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    for round in 0..150 {
        let count = 4 + rng.below(26);
        let full = random_map(&mut rng, count);
        let mut memory = LayoutMemory::default();
        let mut colors = ServiceColors::new();
        let mut before: HashMap<String, (usize, usize)> = HashMap::new();
        // Reveal the nodes (and the calls between those revealed) in a random
        // order, a few at a time, refreshing in between.
        let mut order: Vec<usize> = (0..count).collect();
        for i in (1..count).rev() {
            order.swap(i, rng.below(i + 1));
        }
        let mut shown = 0;
        while shown < count {
            shown = (shown + 1 + rng.below(4)).min(count);
            let known: HashSet<String> =
                order[..shown].iter().map(|i| format!("n{i:02}")).collect();
            let map = ServiceMap {
                nodes: full
                    .nodes
                    .iter()
                    .filter(|n| known.contains(&n.id))
                    .cloned()
                    .collect(),
                edges: full
                    .edges
                    .iter()
                    .filter(|e| known.contains(&e.from) && known.contains(&e.to))
                    .cloned()
                    .collect(),
            };
            let laid = layout(&map, WINDOW, &mut colors, &mut memory);
            assert_eq!(laid.nodes.len(), shown, "round {round}");
            assert_well_placed(&laid);
            let now = cells(&laid);
            for (id, place) in &before {
                assert_eq!(now[id], *place, "round {round}: {id} moved");
            }
            before = now;
        }
    }
}

#[test]
fn acyclic_maps_get_the_longest_path_as_their_column() {
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    for round in 0..200 {
        let count = 1 + rng.below(25);
        let nodes: Vec<ServiceNode> = (0..count).map(|i| svc(&format!("n{i:02}"))).collect();
        // Calls only go to a higher number, so there is no cycle.
        let mut pairs = HashSet::new();
        for _ in 0..count * 2 {
            let (a, b) = (rng.below(count), rng.below(count));
            if a < b {
                pairs.insert((a, b));
            }
        }
        // The column of each node: the longest chain of calls into it.
        let mut expected = vec![0usize; count];
        for to in 0..count {
            for &(from, t) in &pairs {
                if t == to {
                    expected[to] = expected[to].max(expected[from] + 1);
                }
            }
        }
        let edges = pairs
            .iter()
            .map(|&(a, b)| edge(&format!("n{a:02}"), &format!("n{b:02}")))
            .collect();
        let laid = fresh(&map_of(nodes, edges));
        assert_well_placed(&laid);
        for (i, want) in expected.iter().enumerate() {
            assert_eq!(
                cell(&laid, &format!("n{i:02}")).0,
                *want,
                "round {round}: n{i:02}"
            );
        }
        // Every call goes forward.
        for placed in &laid.edges {
            assert!(
                cell(&laid, &placed.from).0 < cell(&laid, &placed.to).0,
                "round {round}: {placed:?}"
            );
        }
    }
}

#[test]
fn a_very_long_chain_does_not_exhaust_the_stack() {
    // The search for cycles walks the whole chain: it must not recurse.
    let count = 10_000;
    let ids: Vec<String> = (0..count).map(|i| format!("n{i:05}")).collect();
    let nodes = ids.iter().map(|id| svc(id)).collect();
    let edges = ids
        .windows(2)
        .map(|pair| edge(&pair[0], &pair[1]))
        .collect();
    let laid = fresh(&map_of(nodes, edges));
    assert_eq!(laid.nodes.len(), count);
    assert_eq!(cell(&laid, &ids[count - 1]), (count - 1, 0));
}

#[test]
fn a_wide_map_is_laid_out_in_reasonable_time() {
    // One service calling 5,000 external hosts: one column of 5,000 rows.
    let count = 5_000;
    let hosts: Vec<String> = (0..count).map(|i| format!("host-{i:04}")).collect();
    let mut nodes: Vec<ServiceNode> = hosts
        .iter()
        .map(|host| node(host, NodeKind::External, 10, 0))
        .collect();
    nodes.push(svc("api"));
    let edges = hosts.iter().map(|host| edge("api", host)).collect();
    let map = map_of(nodes, edges);
    let started = std::time::Instant::now();
    let mut memory = LayoutMemory::default();
    let laid = lay(&map, &mut memory);
    assert_eq!(laid.nodes.len(), count + 1);
    assert_well_placed(&laid);
    // A second refresh with the memory full is as quick.
    assert_eq!(lay(&map, &mut memory), laid);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}
