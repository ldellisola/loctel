//! What an agent does with loctel, step by step: take a marker, make a
//! request, ask what happened since (design section 3.2, plan Task 10).

use std::time::Duration;

use loctel_e2e::{Harness, TransportKind};
use loctel_meshgen::Scenario;
use serde_json::json;

#[tokio::test]
async fn agent_flow_since_marker() {
    let harness = Harness::start(Duration::from_millis(200));

    // An earlier request that must not count: it is before the marker.
    let before = harness
        .send(&Scenario::CheckoutFailure, TransportKind::HttpJson, 1, 0.0)
        .await;
    harness.wait_complete(&before.trace_ids[0]).await;

    let marker = harness.tool("get_marker", json!({})).await;
    let marker = marker["marker"].as_str().unwrap().to_owned();

    // The agent's own two requests: one works, one fails.
    let ok = harness
        .send(&Scenario::CheckoutSuccess, TransportKind::HttpJson, 2, 0.0)
        .await;
    let failed = harness
        .send(&Scenario::CheckoutFailure, TransportKind::HttpJson, 3, 0.0)
        .await;
    harness.wait_complete(&ok.trace_ids[0]).await;
    harness.wait_complete(&failed.trace_ids[0]).await;

    let failures = harness
        .tool("recent_failures", json!({ "since": marker }))
        .await;
    assert_eq!(
        failures["failed"].as_array().unwrap().len(),
        1,
        "{failures}"
    );
    assert_eq!(
        failures["errors_inside"].as_array().unwrap().len(),
        0,
        "{failures}"
    );
    assert_eq!(
        failures["failed"][0]["trace"]["trace_id"],
        failed.trace_ids[0].to_string()
    );

    let found = harness
        .tool("search_traces", json!({ "since": marker }))
        .await;
    assert_eq!(found["traces"].as_array().unwrap().len(), 2, "{found}");

    // Without the marker the agent also sees the earlier failure.
    let everything = harness.tool("recent_failures", json!({})).await;
    assert_eq!(
        everything["failed"].as_array().unwrap().len(),
        2,
        "{everything}"
    );
}

#[tokio::test]
async fn health_checks_are_visible() {
    // loctel has no default discard rules (decision 0016): a harness started
    // without a rules file stores health checks, among the real traffic. The
    // rule that silences them is in `tests/rules.rs`.
    let harness = Harness::start(Duration::from_millis(200));
    harness
        .send(
            &Scenario::HealthChecks { count: 4 },
            TransportKind::HttpProtobuf,
            1,
            0.0,
        )
        .await;
    let checkout = harness
        .send(
            &Scenario::CheckoutSuccess,
            TransportKind::HttpProtobuf,
            2,
            0.0,
        )
        .await;
    harness.wait_complete(&checkout.trace_ids[0]).await;

    let found = harness
        .tool("search_traces", json!({ "service": "orders-api" }))
        .await;

    let traces = found["traces"].as_array().unwrap();
    let health: Vec<_> = traces
        .iter()
        .filter(|trace| trace["root_name"] == "GET /health")
        .collect();
    assert_eq!(health.len(), 4, "{found}");
    for trace in &health {
        assert_eq!(trace["root_service"], "orders-api", "{trace}");
    }
    // The checkout that went through orders-api is listed with them.
    assert!(
        traces
            .iter()
            .any(|trace| trace["root_name"] == "POST /api/checkout"),
        "{found}"
    );
}
