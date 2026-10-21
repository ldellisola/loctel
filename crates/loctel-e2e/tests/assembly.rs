//! Spans that arrive late and out of order still make one whole trace
//! (design section 4, plan Task 10).

use std::collections::HashSet;
use std::time::Duration;

use loctel_e2e::{Harness, TransportKind};
use loctel_meshgen::Scenario;
use serde_json::json;

#[tokio::test]
async fn late_spans_assemble_into_one_trace() {
    let harness = Harness::start(Duration::from_millis(200));

    // The leaves first, the middle 50 ms later, the root 50 ms after that:
    // real pauses, not a compressed clock.
    let sent = harness
        .send(
            &Scenario::LateOutOfOrder,
            TransportKind::HttpProtobuf,
            1,
            1.0,
        )
        .await;
    assert_eq!(sent.trace_ids.len(), 1);
    let trace_id = sent.trace_ids[0].to_string();
    harness.wait_complete(&trace_id).await;

    let trace = harness
        .tool(
            "get_trace",
            json!({ "trace_id": trace_id, "detail": "full" }),
        )
        .await;

    let summary = &trace["summary"];
    assert_eq!(summary["span_count"], 14, "{trace}");
    assert_eq!(summary["complete"], true, "{trace}");
    assert_eq!(summary["status"], "ok", "{trace}");
    // The root arrived last and is the root.
    assert_eq!(summary["root_name"], "POST /api/checkout", "{trace}");
    assert_eq!(summary["root_service"], "web-bff", "{trace}");

    let spans = trace["spans"].as_array().unwrap();
    assert_eq!(spans.len(), 14, "{trace}");
    assert_eq!(trace["omitted"], 0, "{trace}");
    assert!(trace["next_cursor"].is_null(), "{trace}");

    // Exactly one root, and every other span hangs under a span that is there:
    // no orphans.
    let ids: HashSet<&str> = spans
        .iter()
        .map(|span| span["span_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), 14, "span ids are unique");
    let roots: Vec<_> = spans
        .iter()
        .filter(|span| span.get("parent_span_id").is_none_or(|p| p.is_null()))
        .collect();
    assert_eq!(roots.len(), 1, "one root: {trace}");
    assert_eq!(roots[0]["name"], "POST /api/checkout");
    assert_eq!(roots[0]["service"], "web-bff");
    for span in spans {
        if let Some(parent) = span["parent_span_id"].as_str() {
            assert!(
                ids.contains(parent),
                "orphan {} (parent {parent} missing): {trace}",
                span["name"]
            );
        }
    }

    // And it is one trace in the store, not one per batch.
    let found = harness.tool("search_traces", json!({})).await;
    assert_eq!(found["traces"].as_array().unwrap().len(), 1, "{found}");
}
