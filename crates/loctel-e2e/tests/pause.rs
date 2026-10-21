//! A paused loctel keeps answering success and stores nothing (design
//! section 4, plan Task 10).

use std::time::Duration;

use loctel_e2e::{Harness, TransportKind};
use loctel_meshgen::Scenario;
use serde_json::json;

#[tokio::test]
async fn paused_engine_stores_nothing() {
    let harness = Harness::start(Duration::from_millis(200));
    harness.engine().set_paused(true);

    // Over every transport the exporter is told "success" (`send` fails on
    // anything else), so a real exporter would not retry.
    let mut dropped = Vec::new();
    for (seed, kind) in (1u64..).zip(TransportKind::ALL) {
        let sent = harness
            .send(&Scenario::CheckoutFailure, kind, seed, 0.0)
            .await;
        dropped.push(sent.trace_ids[0].to_string());
    }

    // The agent sees a paused loctel that holds nothing.
    let status = harness.tool("get_status", json!({})).await;
    assert_eq!(status["paused"], true, "{status}");
    assert_eq!(status["traces_held"], 0, "{status}");
    assert_eq!(status["spans_held"], 0, "{status}");
    assert_eq!(status["logs_held"], 0, "{status}");
    assert_eq!(status["spans_per_sec"], 0.0, "{status}");
    let listeners = status["listeners"].as_array().unwrap();
    assert!(listeners.iter().all(|l| l["state"] == "listening"));

    let traces = harness.tool("search_traces", json!({})).await;
    assert_eq!(traces["traces"], json!([]), "{traces}");
    let logs = harness.tool("search_logs", json!({})).await;
    assert_eq!(logs["logs"], json!([]), "{logs}");
    let failures = harness.tool("recent_failures", json!({})).await;
    assert_eq!(failures["failed"], json!([]), "{failures}");
    let map = harness.tool("get_service_map", json!({})).await;
    assert_eq!(map["nodes"], json!([]), "{map}");

    // Resuming keeps what arrives next. What was dropped stays dropped.
    harness.engine().set_paused(false);
    let sent = harness
        .send(&Scenario::CheckoutFailure, TransportKind::HttpJson, 4, 0.0)
        .await;
    harness.wait_complete(&sent.trace_ids[0]).await;

    let status = harness.tool("get_status", json!({})).await;
    assert_eq!(status["paused"], false, "{status}");
    assert_eq!(status["traces_held"], 1, "{status}");
    assert_eq!(status["spans_held"], 15, "{status}");
    assert_eq!(status["logs_held"], 7, "{status}");

    let traces = harness.tool("search_traces", json!({})).await;
    assert_eq!(traces["traces"].as_array().unwrap().len(), 1, "{traces}");
    assert_eq!(
        traces["traces"][0]["trace_id"],
        sent.trace_ids[0].to_string()
    );

    for trace_id in &dropped {
        let missing = harness
            .mcp()
            .call("get_trace", json!({ "trace_id": trace_id }))
            .await;
        assert!(missing.is_error, "{trace_id} was dropped while paused");
        assert_eq!(missing.text, "trace not found");
    }
}
