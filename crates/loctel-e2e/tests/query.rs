//! TraceQL and LogQL through the whole path (design section 6, M2 plan
//! Task 9): real OTLP in, and an MCP client that asks the way an agent does,
//! with queries written as in Grafana.

use std::time::Duration;

use loctel_e2e::{Harness, TransportKind};
use loctel_meshgen::Scenario;
use serde_json::{Value, json};

/// A harness holding a failing checkout and a successful one (both are
/// `POST /api/checkout` traces over the same five services), and their trace
/// IDs: the failing one first.
async fn with_both_checkouts() -> (Harness, String, String) {
    let harness = Harness::start(Duration::from_millis(200));
    let failed = harness
        .send(&Scenario::CheckoutFailure, TransportKind::Grpc, 1, 0.0)
        .await;
    let ok = harness
        .send(&Scenario::CheckoutSuccess, TransportKind::HttpJson, 2, 0.0)
        .await;
    harness.wait_complete(&failed.trace_ids[0]).await;
    harness.wait_complete(&ok.trace_ids[0]).await;
    (
        harness,
        failed.trace_ids[0].to_string(),
        ok.trace_ids[0].to_string(),
    )
}

/// The ids of the traces in a `search_traces` answer, sorted.
fn trace_ids(found: &Value) -> Vec<String> {
    let mut ids: Vec<String> = found["traces"]
        .as_array()
        .unwrap_or_else(|| panic!("no traces in {found}"))
        .iter()
        .map(|trace| trace["trace_id"].as_str().unwrap().to_owned())
        .collect();
    ids.sort();
    ids
}

/// The messages of the logs in a `search_logs` answer, sorted.
fn messages(found: &Value) -> Vec<String> {
    let mut messages: Vec<String> = found["logs"]
        .as_array()
        .unwrap_or_else(|| panic!("no logs in {found}"))
        .iter()
        .map(|log| log["message"].as_str().unwrap().to_owned())
        .collect();
    messages.sort();
    messages
}

#[tokio::test]
async fn traceql_structural_query_over_mcp() {
    let (harness, failed, ok) = with_both_checkouts().await;
    let mut both = vec![failed.clone(), ok.clone()];
    both.sort();

    // Without a query, the traces carry no matched spans.
    let all = harness.tool("search_traces", json!({})).await;
    assert_eq!(trace_ids(&all), both, "{all}");
    assert!(
        all["traces"]
            .as_array()
            .unwrap()
            .iter()
            .all(|trace| trace.get("matched_span_ids").is_none()),
        "{all}"
    );

    // The query from the plan, as it would be pasted from Grafana: a
    // `POST /charges` span somewhere below a span of web-bff. Both checkouts
    // have one, and the answer names it.
    let query = r#"{ resource.service.name = "web-bff" } >> { name = "POST /charges" }"#;
    let found = harness
        .tool("search_traces", json!({ "query": query }))
        .await;
    assert_eq!(trace_ids(&found), both, "{found}");
    for trace in found["traces"].as_array().unwrap() {
        let matched = trace["matched_span_ids"].as_array().unwrap();
        assert_eq!(matched.len(), 1, "{trace}");
        let span = harness
            .tool(
                "get_span",
                json!({ "trace_id": trace["trace_id"], "span_id": matched[0] }),
            )
            .await;
        assert_eq!(span["span"]["name"], "POST /charges", "{span}");
        assert_eq!(span["span"]["service"], "payments-api", "{span}");
    }

    // Structure matters, not only attributes: the same two filters the other
    // way round (a web-bff span below a payments-api span) match nothing, and
    // that is an answer, not an error.
    let backwards = harness
        .tool(
            "search_traces",
            json!({ "query": r#"{ resource.service.name = "payments-api" } >> { resource.service.name = "web-bff" }"# }),
        )
        .await;
    assert_eq!(backwards["traces"], json!([]), "{backwards}");

    // An erroring span below web-bff exists only in the failing checkout.
    let errors_below = harness
        .tool(
            "search_traces",
            json!({ "query": r#"{ resource.service.name = "web-bff" } >> { status = error }"# }),
        )
        .await;
    assert_eq!(
        trace_ids(&errors_below),
        vec![failed.clone()],
        "{errors_below}"
    );
    // Spans 3, 4, 9, 10, 12 and 13 of the frozen table: the erroring spans
    // that have a web-bff ancestor (the erroring root has none).
    let matched = errors_below["traces"][0]["matched_span_ids"]
        .as_array()
        .unwrap();
    assert_eq!(matched.len(), 6, "{errors_below}");
    for span_id in matched {
        let span = harness
            .tool(
                "get_span",
                json!({ "trace_id": failed, "span_id": span_id }),
            )
            .await;
        assert_eq!(span["span"]["status"], "error", "{span}");
    }

    // A pipeline, and the query as one more filter beside the others.
    let counted = harness
        .tool(
            "search_traces",
            json!({ "query": "{ status = error } | count() > 3" }),
        )
        .await;
    assert_eq!(trace_ids(&counted), vec![failed.clone()], "{counted}");
    assert_eq!(
        counted["traces"][0]["matched_span_ids"]
            .as_array()
            .unwrap()
            .len(),
        7,
        "{counted}"
    );
    let narrowed = harness
        .tool(
            "search_traces",
            json!({ "query": query, "errors_only": true, "service": "payments-api" }),
        )
        .await;
    assert_eq!(trace_ids(&narrowed), vec![failed], "{narrowed}");
}

#[tokio::test]
async fn logql_query_over_mcp() {
    let (harness, _, _) = with_both_checkouts().await;

    // The failing checkout's error logs of one service.
    let errors = harness
        .tool(
            "search_logs",
            json!({ "query": r#"{service_name="payments-api"} | level = "error""# }),
        )
        .await;
    assert_eq!(
        messages(&errors),
        vec![
            "Charge failed for order 8812: CardGatewayTimeoutException: card-gateway did not respond after 2 attempts",
            "card-gateway did not answer within 600 ms",
        ],
        "{errors}"
    );
    for log in errors["logs"].as_array().unwrap() {
        assert_eq!(log["level"], "error", "{log}");
        assert_eq!(log["service"], "payments-api", "{log}");
    }

    // Selector, regex on the label, line filter: order 8812 in two services,
    // in both checkouts (5 logs of the failing one, 6 of the successful one).
    let order = harness
        .tool(
            "search_logs",
            json!({ "query": r#"{service_name=~"orders-api|payments-api"} |= "8812""# }),
        )
        .await;
    assert_eq!(order["logs"].as_array().unwrap().len(), 5 + 6, "{order}");

    // A line filter regex matches inside the line (unanchored).
    let retry = harness
        .tool(
            "search_logs",
            json!({ "query": r#"{service_name="payments-api"} |~ "attempt [0-9]""# }),
        )
        .await;
    assert_eq!(
        messages(&retry),
        vec!["card-gateway did not answer within 600 ms, retrying (attempt 2 of 2)"],
        "{retry}"
    );

    // A label matcher regex is anchored: "orders" is not "orders-api". No
    // match is an empty answer.
    let anchored = harness
        .tool(
            "search_logs",
            json!({ "query": r#"{service_name=~"orders"}"# }),
        )
        .await;
    assert_eq!(anchored["logs"], json!([]), "{anchored}");

    // Severity as a label, across services, and the query beside `service`.
    let every_error = harness
        .tool("search_logs", json!({ "query": r#"{level="error"}"# }))
        .await;
    assert_eq!(
        every_error["logs"].as_array().unwrap().len(),
        3,
        "{every_error}"
    );
    let one_service = harness
        .tool(
            "search_logs",
            json!({ "query": r#"{level="error"}"#, "service": "orders-api" }),
        )
        .await;
    assert_eq!(
        messages(&one_service),
        vec!["Checkout failed for order 8812: payments-api returned 500"],
        "{one_service}"
    );
}

#[tokio::test]
async fn invalid_query_is_tool_error() {
    let (harness, _, _) = with_both_checkouts().await;
    let mcp = harness.mcp();

    // A typo: the answer says where, counting columns from 1.
    let broken = mcp
        .call("search_traces", json!({ "query": "{ name = }" }))
        .await;
    assert!(broken.is_error, "{broken:?}");
    assert!(broken.structured.is_none(), "{broken:?}");
    assert!(
        broken.text.starts_with("invalid TraceQL: at column 10: "),
        "{broken:?}"
    );

    // Grafana syntax that loctel lacks is an error naming the feature and
    // where it is, never a result that quietly differs from Grafana's.
    let select = mcp
        .call(
            "search_traces",
            json!({ "query": "{ status = error } | select(span.foo)" }),
        )
        .await;
    assert!(select.is_error, "{select:?}");
    assert_eq!(
        select.text,
        "invalid TraceQL: at column 22: select is not supported yet"
    );

    let line_format = mcp
        .call(
            "search_logs",
            json!({ "query": r#"{service_name="x"} | line_format "{{.a}}""# }),
        )
        .await;
    assert!(line_format.is_error, "{line_format:?}");
    assert_eq!(
        line_format.text,
        "invalid LogQL: at column 22: line_format is not supported yet"
    );
    let metrics = mcp
        .call(
            "search_logs",
            json!({ "query": r#"rate({service_name="x"}[5m])"# }),
        )
        .await;
    assert!(metrics.is_error, "{metrics:?}");
    assert_eq!(
        metrics.text,
        "invalid LogQL: at column 1: metrics queries are not supported yet"
    );

    // The session survives a bad query: the next one is answered.
    let fine = harness
        .tool("search_traces", json!({ "query": "{ status = error }" }))
        .await;
    assert_eq!(fine["traces"].as_array().unwrap().len(), 1, "{fine}");
}
