//! The service map an agent reads has the topology the mockups show
//! (design section 9.2, plan Task 10).

use std::time::Duration;

use loctel_e2e::{Harness, TransportKind};
use loctel_meshgen::Scenario;
use serde_json::{Value, json};

fn node<'a>(map: &'a Value, id: &str, kind: &str) -> Option<&'a Value> {
    map["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == id && node["kind"] == kind)
}

fn edge<'a>(map: &'a Value, from: &str, to: &str) -> Option<&'a Value> {
    map["edges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|edge| edge["from"] == from && edge["to"] == to)
}

#[tokio::test]
async fn service_map_matches_mockup_topology() {
    let harness = Harness::start(Duration::from_millis(200));
    let sent = harness
        .send(&Scenario::CheckoutFailure, TransportKind::HttpJson, 1, 0.0)
        .await;
    // External hosts show up once their trace is complete.
    harness.wait_complete(&sent.trace_ids[0]).await;

    let map = harness.tool("get_service_map", json!({})).await;

    for (id, kind) in [
        ("web-bff", "service"),
        ("orders-api", "service"),
        ("inventory-api", "service"),
        ("payments-api", "service"),
        ("notifications-worker", "service"),
        ("postgresql", "database"),
        ("redis", "database"),
        ("rabbitmq", "queue"),
        ("card-gateway", "external"),
    ] {
        assert!(node(&map, id, kind).is_some(), "no {kind} {id} in {map}");
    }
    assert_eq!(map["nodes"].as_array().unwrap().len(), 9, "{map}");

    // The failing call is red on the edge.
    let failing = edge(&map, "orders-api", "payments-api")
        .unwrap_or_else(|| panic!("no orders-api to payments-api edge in {map}"));
    assert!(failing["errors"].as_u64().unwrap() >= 1, "{failing}");

    // The message goes through the broker: orders-api publishes to rabbitmq,
    // the worker consumes from it, and orders-api never calls the worker.
    assert!(edge(&map, "orders-api", "rabbitmq").is_some(), "{map}");
    assert!(
        edge(&map, "rabbitmq", "notifications-worker").is_some(),
        "{map}"
    );
    assert!(
        edge(&map, "orders-api", "notifications-worker").is_none(),
        "orders-api must not call notifications-worker directly: {map}"
    );
}
