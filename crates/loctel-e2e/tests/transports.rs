//! The same failing checkout over each OTLP transport: gRPC, HTTP protobuf and
//! HTTP JSON must give an agent the same answer (plan Task 10).

use std::time::Duration;

use loctel_e2e::{Harness, TransportKind};
use loctel_meshgen::Scenario;
use serde_json::json;

#[tokio::test]
async fn checkout_failure_over_each_transport() {
    let harness = Harness::start(Duration::from_millis(200));

    // One loctel for all three, a seed apiece so that the traces differ and
    // the store's duplicate check keeps every one.
    for (seed, kind) in (1u64..).zip(TransportKind::ALL) {
        // What an agent does: take a marker, make the request, ask what failed
        // since. Each round therefore sees only its own trace.
        let marker = harness.marker().await;
        let sent = harness
            .send(&Scenario::CheckoutFailure, kind, seed, 0.0)
            .await;
        let trace_id = sent.trace_ids[0].to_string();
        harness.wait_complete(&trace_id).await;

        let failures = harness
            .tool("recent_failures", json!({ "since": marker }))
            .await;

        assert_eq!(failures["errors_inside"], json!([]), "{kind:?}: {failures}");
        let failed = failures["failed"].as_array().unwrap();
        assert_eq!(failed.len(), 1, "{kind:?}: {failures}");
        let failure = &failed[0];

        // The trace is this transport's trace, whole and complete.
        let trace = &failure["trace"];
        assert_eq!(trace["trace_id"], trace_id, "{kind:?}");
        assert_eq!(trace["status"], "failed", "{kind:?}");
        assert_eq!(trace["span_count"], 15, "{kind:?}");
        assert_eq!(trace["complete"], true, "{kind:?}");
        assert_eq!(trace["root_name"], "POST /api/checkout", "{kind:?}");
        assert_eq!(trace["root_service"], "web-bff", "{kind:?}");

        // The answer the whole product exists for: failed in payments-api,
        // while calling card-gateway, with the exception that explains it.
        assert_eq!(trace["failed_in"]["service"], "payments-api", "{kind:?}");
        assert_eq!(trace["failed_in"]["span_name"], "POST", "{kind:?}");
        assert_eq!(trace["failed_in"]["peer"], "card-gateway", "{kind:?}");
        assert_eq!(trace["failed_in"]["more"], 1, "{kind:?}");
        assert_eq!(
            failure["failed_in_span"]["service"], "payments-api",
            "{kind:?}"
        );
        assert_eq!(
            failure["exception"]["type"], "Payments.Api.Gateway.CardGatewayTimeoutException",
            "{kind:?}"
        );
        assert_eq!(
            failure["exception"]["message"], "card-gateway did not respond after 2 attempts",
            "{kind:?}"
        );
        assert!(
            failure["error_logs"].as_array().unwrap().len() >= 2,
            "{kind:?}: the logs of the trace arrived too: {failure}"
        );

        // The logs came over the same transport: all seven, linked to the trace.
        let logs = harness
            .tool("search_logs", json!({ "trace_id": trace_id }))
            .await;
        assert_eq!(logs["logs"].as_array().unwrap().len(), 7, "{kind:?}");
    }

    // Three traces in the end, one per transport.
    let all = harness.tool("search_traces", json!({})).await;
    assert_eq!(all["traces"].as_array().unwrap().len(), 3);
}
