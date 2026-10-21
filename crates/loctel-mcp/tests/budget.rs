//! The answer size budget (design section 8: small answers first, well under
//! about 8k tokens). Every tool, called with default arguments, must answer in
//! at most `ANSWER_TEXT_BUDGET` characters of text, on the fixtures an agent is
//! most likely to meet: the failing checkout, a pile of health checks and one
//! enormous trace.
//!
//! The text is what an agent reads, so the text is what is counted (the
//! structured content is the same JSON again). Characters, not bytes.

mod common;

use common::*;
use loctel_mcp::{ANSWER_TEXT_BUDGET, LoctelTools};
use loctel_meshgen::Scenario;
use rmcp::model::CallToolResult;
use serde_json::{Value, json};

fn text_of(result: &CallToolResult) -> String {
    assert_eq!(result.content.len(), 1, "one text block: {result:?}");
    result.content[0]
        .as_text()
        .unwrap_or_else(|| panic!("not text: {result:?}"))
        .text
        .clone()
}

/// Calls a tool, which must succeed, and returns its text and its JSON.
async fn answer(tools: &LoctelTools, name: &str, args: Value) -> (String, Value) {
    let result = tools.call(name, args).await;
    assert_ne!(result.is_error, Some(true), "{name} failed: {result:?}");
    let text = text_of(&result);
    let json = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{name}: not JSON: {e}"));
    (text, json)
}

#[test]
fn the_budget_is_24000_characters() {
    // About 6k tokens at 4 characters each: under the 8k of design section 8.
    assert_eq!(ANSWER_TEXT_BUDGET, 24_000);
}

#[tokio::test]
async fn default_answers_fit_budget() {
    let fixtures = [
        ("CheckoutFailure", Scenario::CheckoutFailure),
        ("HealthChecks{50}", Scenario::HealthChecks { count: 50 }),
        ("HugeTrace{10000}", Scenario::HugeTrace { spans: 10_000 }),
    ];
    let mut too_big = Vec::new();
    for (label, scenario) in fixtures {
        let (store, clock) = manual_store();
        let trace_ids = load(&store, &scenario, 1, BASE);
        settle(&clock);
        let tools = LoctelTools::new(store.clone(), fake_status());

        // The tools that name a trace or a span get the worst one there is:
        // the first trace, and its failed-in span if it has one (the one with
        // the exception and the stack trace), else its root.
        let trace_id = trace_ids[0].to_string();
        let (_, trace) = answer(&tools, "get_trace", json!({ "trace_id": trace_id })).await;
        let span_id = trace["summary"]["failed_in"]["span_id"]
            .as_str()
            .or_else(|| trace["spans"][0]["span_id"].as_str())
            .expect("a trace has a span")
            .to_owned();

        let calls = [
            ("get_status", json!({})),
            ("get_marker", json!({})),
            ("list_services", json!({})),
            ("get_service_map", json!({})),
            ("search_traces", json!({})),
            ("recent_failures", json!({})),
            ("get_trace", json!({ "trace_id": trace_id })),
            (
                "get_span",
                json!({ "trace_id": trace_id, "span_id": span_id }),
            ),
            ("search_logs", json!({})),
        ];
        for (name, args) in calls {
            let (text, _) = answer(&tools, name, args).await;
            let length = text.chars().count();
            if length > ANSWER_TEXT_BUDGET {
                too_big.push(format!("{name} on {label}: {length} characters"));
            }
        }
    }
    assert!(
        too_big.is_empty(),
        "default answers over {ANSWER_TEXT_BUDGET} characters:\n{}",
        too_big.join("\n")
    );
}

#[tokio::test]
async fn the_huge_trace_fixture_really_is_huge() {
    // A budget test on an empty store would pass for the wrong reason: the
    // trace must hold its 10,000 spans, and its default page must leave most
    // of them for the cursor.
    let (store, clock) = manual_store();
    let ids = load(&store, &Scenario::HugeTrace { spans: 10_000 }, 1, BASE);
    settle(&clock);
    let tools = LoctelTools::new(store.clone(), fake_status());

    let (_, trace) = answer(
        &tools,
        "get_trace",
        json!({ "trace_id": ids[0].to_string() }),
    )
    .await;

    assert_eq!(trace["summary"]["span_count"], 10_000);
    let shown = trace["spans"].as_array().unwrap().len() as u64;
    assert!(shown > 0);
    assert_eq!(shown + trace["omitted"].as_u64().unwrap(), 10_000);
    assert!(trace["next_cursor"].is_string());
}
