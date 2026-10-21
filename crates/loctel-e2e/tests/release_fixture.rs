//! The release job posts `fixtures/one-span.json` to the loctel it just built
//! (`.github/workflows/release.yml`, plan Task 13). `curl -f` only checks the
//! status code, and OTLP answers 200 even when it rejects the items
//! (`partial_success`), so this test is what keeps the fixture a valid trace
//! request that loctel stores.

use std::time::Duration;

use loctel_e2e::{Harness, TransportKind};
use loctel_meshgen::Transport;
use serde_json::{Value, json};

/// Exactly the bytes the release job sends.
const ONE_SPAN: &str = include_str!("../fixtures/one-span.json");

#[tokio::test]
async fn one_span_fixture_is_accepted_and_stored() {
    let harness = Harness::start(Duration::from_millis(200));
    let Transport::HttpJson(base) = harness.otlp(TransportKind::HttpJson) else {
        panic!("the HTTP JSON transport is an HttpJson");
    };

    // The same request as the job's `curl -sf -X POST -H 'content-type:
    // application/json' --data @one-span.json .../v1/traces`.
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let response = http
        .post(format!("{base}/v1/traces"))
        .header("content-type", "application/json")
        .body(ONE_SPAN)
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success(), "{}", response.status());
    let answer: Value = response.json().await.unwrap();
    let rejected = &answer["partialSuccess"]["rejectedSpans"];
    assert!(
        rejected.is_null() || rejected == "0" || rejected == 0,
        "loctel rejected the fixture's span: {answer}"
    );

    // And it is there, as one complete trace, when an agent asks.
    let fixture: Value = serde_json::from_str(ONE_SPAN).expect("the fixture is JSON");
    let trace_id = fixture["resourceSpans"][0]["scopeSpans"][0]["spans"][0]["traceId"]
        .as_str()
        .expect("the fixture's span has a traceId");
    harness.wait_complete(trace_id).await;
    let trace = harness
        .tool("get_trace", json!({ "trace_id": trace_id, "limit": 1 }))
        .await;
    assert_eq!(trace["summary"]["span_count"], 1, "{trace}");
    assert_eq!(trace["summary"]["root_service"], "release-smoke", "{trace}");
    assert_eq!(trace["summary"]["status"], "ok", "{trace}");
}
