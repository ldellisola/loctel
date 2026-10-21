//! The nine MCP tools, called directly through `LoctelTools::call` (plan
//! Task 9). The data is the fake mesh loaded into a store through the real
//! converter, with a manual clock moved past the quiet period, so every trace
//! is complete.
//!
//! The first nine tests are the plan's own. The rest pin what the plan fixes
//! in its tables: the argument and result shapes, the limits, `since`, the
//! two kinds of failure, truncation and the error texts. The last section is
//! Milestone 2's (plan Task 7): the `query` arguments and the rules in
//! `get_status`; the answer size budget has its own file, `budget.rs`.

mod common;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use common::*;
use loctel_mcp::{LoctelTools, RuleDto, RuleLanguageDto, RuleScopeDto, StatusDto, StatusSource};
use loctel_meshgen::Scenario;
use loctel_meshgen::checkout::{EXCEPTION_MESSAGE, EXCEPTION_STACKTRACE, EXCEPTION_TYPE};
use loctel_model::{Severity, SpanKind};
use loctel_store::{ManualClock, Store};
use rmcp::model::CallToolResult;
use serde_json::{Value, json};

const ALL_TOOLS: [&str; 9] = [
    "get_status",
    "get_marker",
    "list_services",
    "get_service_map",
    "search_traces",
    "recent_failures",
    "get_trace",
    "get_span",
    "search_logs",
];

struct Fixture {
    tools: LoctelTools,
    store: Store,
    clock: Arc<ManualClock>,
    /// Each tool's output schema, compiled. Every result `call` returns is
    /// checked against it.
    validators: HashMap<String, jsonschema::Validator>,
}

impl Fixture {
    fn new() -> Fixture {
        Fixture::with_status(fake_status())
    }

    /// A fixture whose `get_status` says what `status` says.
    fn with_status(status: Arc<dyn StatusSource>) -> Fixture {
        let (store, clock) = manual_store();
        let tools = LoctelTools::new(store.clone(), status);
        let validators = LoctelTools::definitions()
            .into_iter()
            .map(|tool| {
                let schema = Value::Object((**tool.output_schema.as_ref().unwrap()).clone());
                let validator = jsonschema::validator_for(&schema)
                    .unwrap_or_else(|e| panic!("{}: bad output schema: {e}", tool.name));
                (tool.name.to_string(), validator)
            })
            .collect();
        Fixture {
            tools,
            store,
            clock,
            validators,
        }
    }

    /// A fixture holding the failing checkout, complete.
    fn with_checkout_failure() -> (Fixture, String) {
        let fixture = Fixture::new();
        let ids = load(&fixture.store, &Scenario::CheckoutFailure, 1, BASE);
        settle(&fixture.clock);
        (fixture, ids[0].to_string())
    }

    /// The structured content of a successful call, which matches the tool's
    /// output schema.
    async fn call(&self, name: &str, args: Value) -> Value {
        let result = self.tools.call(name, args).await;
        assert_ne!(result.is_error, Some(true), "{name} failed: {result:?}");
        let structured = result
            .structured_content
            .unwrap_or_else(|| panic!("{name} has no structured content"));
        let errors: Vec<String> = self.validators[name]
            .iter_errors(&structured)
            .map(|error| error.to_string())
            .collect();
        assert!(errors.is_empty(), "{name}: {errors:?}\n{structured}");
        structured
    }

    async fn call_error(&self, name: &str, args: Value) -> String {
        let result = self.tools.call(name, args).await;
        assert_eq!(
            result.is_error,
            Some(true),
            "{name} should fail: {result:?}"
        );
        assert!(
            result.structured_content.is_none(),
            "an error carries text only"
        );
        text_of(&result)
    }
}

fn text_of(result: &CallToolResult) -> String {
    assert_eq!(result.content.len(), 1, "one text block: {result:?}");
    result.content[0]
        .as_text()
        .unwrap_or_else(|| panic!("not text: {result:?}"))
        .text
        .clone()
}

fn ids(items: &Value, key: &str) -> Vec<String> {
    items
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item[key].as_str().unwrap().to_owned())
        .collect()
}

// --- the plan's tests -----------------------------------------------------------

#[tokio::test]
async fn recent_failures_names_failed_in_and_exception() {
    let (fixture, trace_id) = Fixture::with_checkout_failure();

    let r = fixture.call("recent_failures", json!({})).await;

    let f = &r["failed"][0];
    assert_eq!(f["trace"]["trace_id"], trace_id);
    assert_eq!(f["trace"]["status"], "failed");
    assert_eq!(f["trace"]["failed_in"]["service"], "payments-api");
    assert_eq!(f["trace"]["failed_in"]["peer"], "card-gateway");
    assert_eq!(f["trace"]["failed_in"]["more"], 1);
    assert_eq!(f["exception"]["type"], EXCEPTION_TYPE);
    assert_eq!(f["exception"]["message"], EXCEPTION_MESSAGE);
    assert_eq!(f["exception"]["stacktrace"], EXCEPTION_STACKTRACE);
    assert!(f["error_logs"].as_array().unwrap().len() >= 2);
    assert_eq!(r["failed"].as_array().unwrap().len(), 1);
    assert_eq!(r["errors_inside"], json!([]));
}

#[tokio::test]
async fn search_traces_caps_limit_at_100() {
    let fixture = Fixture::new();
    // 30 probes for each of the five services: 150 traces of one span.
    load(
        &fixture.store,
        &Scenario::HealthChecks { count: 30 },
        1,
        BASE,
    );
    settle(&fixture.clock);

    let r = fixture
        .call("search_traces", json!({ "limit": 1000 }))
        .await;
    assert_eq!(r["traces"].as_array().unwrap().len(), 100);
    assert!(r["next_cursor"].is_string(), "more than 100 exist");

    // The default is 20.
    let r = fixture.call("search_traces", json!({})).await;
    assert_eq!(r["traces"].as_array().unwrap().len(), 20);
    assert!(r["next_cursor"].is_string());

    // The cursor leads to the rest, without repeating anything.
    let mut seen = HashSet::new();
    let mut cursor = Value::Null;
    loop {
        let r = fixture
            .call("search_traces", json!({ "limit": 100, "cursor": cursor }))
            .await;
        for id in ids(&r["traces"], "trace_id") {
            assert!(seen.insert(id), "a trace came twice");
        }
        cursor = r["next_cursor"].clone();
        if cursor.is_null() {
            break;
        }
    }
    assert_eq!(seen.len(), 150);
}

#[tokio::test]
async fn since_marker_filters_older() {
    let fixture = Fixture::new();
    load(&fixture.store, &Scenario::CheckoutSuccess, 1, BASE);
    load(
        &fixture.store,
        &Scenario::CheckoutFailure,
        1,
        BASE + 60 * SECOND,
    );
    fixture.clock.set(BASE + 30 * SECOND);
    let marker = fixture.call("get_marker", json!({})).await["marker"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(marker, (BASE + 30 * SECOND).to_string());
    fixture.clock.set(BASE + 120 * SECOND);

    // Everything, then only what started after the marker.
    let all = fixture.call("search_traces", json!({})).await;
    assert_eq!(all["traces"].as_array().unwrap().len(), 2);
    let after = fixture
        .call("search_traces", json!({ "since": marker }))
        .await;
    let traces = after["traces"].as_array().unwrap();
    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0]["status"], "failed");

    // The failures tool honours it too: the failure is after the marker, so it
    // is found; after a later marker, nothing is.
    let r = fixture
        .call("recent_failures", json!({ "since": marker }))
        .await;
    assert_eq!(r["failed"].as_array().unwrap().len(), 1);
    let later = (BASE + 90 * SECOND).to_string();
    let r = fixture
        .call("recent_failures", json!({ "since": later }))
        .await;
    assert_eq!(r["failed"], json!([]));

    // An RFC 3339 time works the same as a marker: 08:00:30 is the marker.
    let r = fixture
        .call("search_traces", json!({ "since": "2027-01-15T08:00:30Z" }))
        .await;
    assert_eq!(r["traces"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn get_trace_summary_collapses_large_trace() {
    let fixture = Fixture::new();
    let trace_id = load(
        &fixture.store,
        &Scenario::HugeTrace { spans: 10_000 },
        1,
        BASE,
    )[0]
    .to_string();
    settle(&fixture.clock);

    let r = fixture
        .call("get_trace", json!({ "trace_id": trace_id }))
        .await;

    let spans = r["spans"].as_array().unwrap();
    assert!(spans.len() <= 200, "{} spans", spans.len());
    // The tree is complete with fan-out 10: 1 + 10 + 100 spans at depth 0 to 2.
    assert_eq!(spans.len(), 111);
    assert!(r["omitted"].as_u64().unwrap() > 0);
    assert_eq!(r["omitted"], 10_000 - 111);
    assert!(r["next_cursor"].is_string());
    assert_eq!(r["summary"]["span_count"], 10_000);
    assert_eq!(r["summary"]["status"], "ok");
    // Nothing deeper than the third level came with the first page.
    assert!(
        spans
            .iter()
            .all(|s| s["start_offset_ms"].as_f64().unwrap() < 20.0)
    );

    // Following the cursor reaches every other span exactly once.
    let mut seen: HashSet<String> = ids(&r["spans"], "span_id").into_iter().collect();
    let mut cursor = r["next_cursor"].clone();
    let mut pages = 0;
    while !cursor.is_null() {
        let page = fixture
            .call(
                "get_trace",
                json!({ "trace_id": trace_id, "limit": 2000, "cursor": cursor }),
            )
            .await;
        for id in ids(&page["spans"], "span_id") {
            assert!(seen.insert(id), "a span came twice");
        }
        cursor = page["next_cursor"].clone();
        pages += 1;
        assert!(page["omitted"].is_u64());
        assert_eq!(cursor.is_null(), page["omitted"] == 0, "cursor iff more");
    }
    assert_eq!(seen.len(), 10_000);
    assert_eq!(pages, 5, "9,889 left, 2,000 at a time");
}

#[tokio::test]
async fn get_trace_full_pages_with_cursor() {
    let (fixture, trace_id) = Fixture::with_checkout_failure();

    let mut seen: Vec<String> = Vec::new();
    let mut offsets: Vec<f64> = Vec::new();
    let mut omitted_per_page = Vec::new();
    let mut cursor = Value::Null;
    loop {
        let page = fixture
            .call(
                "get_trace",
                json!({ "trace_id": trace_id, "detail": "full", "limit": 4, "cursor": cursor }),
            )
            .await;
        assert_eq!(page["summary"]["span_count"], 15);
        assert!(page["spans"].as_array().unwrap().len() <= 4);
        seen.extend(ids(&page["spans"], "span_id"));
        offsets.extend(
            page["spans"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s["start_offset_ms"].as_f64().unwrap()),
        );
        omitted_per_page.push(page["omitted"].as_u64().unwrap());
        cursor = page["next_cursor"].clone();
        if cursor.is_null() {
            break;
        }
    }

    assert_eq!(seen.len(), 15);
    assert_eq!(seen.iter().collect::<HashSet<_>>().len(), 15, "no repeats");
    assert_eq!(omitted_per_page, [11, 7, 3, 0]);
    assert!(
        offsets.windows(2).all(|w| w[0] <= w[1]),
        "oldest first across pages: {offsets:?}"
    );
    assert_eq!(offsets[0], 0.0);

    // One page with everything has no cursor and nothing omitted.
    let all = fixture
        .call(
            "get_trace",
            json!({ "trace_id": trace_id, "detail": "full" }),
        )
        .await;
    assert_eq!(all["spans"].as_array().unwrap().len(), 15);
    assert_eq!(all["omitted"], 0);
    assert!(all["next_cursor"].is_null());
}

#[tokio::test]
async fn get_span_includes_logs_and_exception() {
    let (fixture, trace_id) = Fixture::with_checkout_failure();
    let trace = fixture
        .call(
            "get_trace",
            json!({ "trace_id": trace_id, "detail": "full" }),
        )
        .await;
    let charge = trace["spans"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "POST /charges")
        .expect("the charge span");
    assert_eq!(charge["service"], "payments-api");
    assert_eq!(charge["kind"], "server");
    assert_eq!(charge["status"], "error");
    assert_eq!(charge["exception_type"], EXCEPTION_TYPE);
    let span_id = charge["span_id"].as_str().unwrap();

    let r = fixture
        .call(
            "get_span",
            json!({ "trace_id": trace_id, "span_id": span_id }),
        )
        .await;

    let span = &r["span"];
    assert_eq!(span["trace_id"], trace_id);
    assert_eq!(span["span_id"], span_id);
    assert_eq!(span["name"], "POST /charges");
    assert_eq!(span["service"], "payments-api");
    assert_eq!(span["status"], "error");
    assert_eq!(span["status_message"], EXCEPTION_MESSAGE);
    assert_eq!(span["attributes"]["order.id"], 8812);
    assert_eq!(span["resource"]["service.name"], "payments-api");
    assert_eq!(span["resource"]["service.version"], "1.4.0");
    assert_eq!(span["exception"]["type"], EXCEPTION_TYPE);
    assert_eq!(span["exception"]["message"], EXCEPTION_MESSAGE);
    assert_eq!(span["exception"]["stacktrace"], EXCEPTION_STACKTRACE);
    assert_eq!(span["events"][0]["name"], "exception");
    assert_eq!(
        span["events"][0]["attributes"]["exception.type"],
        EXCEPTION_TYPE
    );
    assert_eq!(span["duration_ms"], 1586.0);
    assert!(
        span["start"]
            .as_str()
            .unwrap()
            .starts_with("2027-01-15T08:00:00.2")
    );

    // Its four log lines, oldest first.
    let logs = r["logs"].as_array().unwrap();
    assert_eq!(logs.len(), 4);
    let levels: Vec<&str> = logs.iter().map(|l| l["level"].as_str().unwrap()).collect();
    assert_eq!(levels, ["info", "warn", "error", "error"]);
    assert!(logs.iter().all(|l| l["service"] == "payments-api"));
    assert!(logs.iter().all(|l| l["span_id"] == span_id));
    assert!(logs.iter().all(|l| l["trace_id"] == trace_id));
}

#[tokio::test]
async fn search_logs_filters_level_and_text() {
    let (fixture, trace_id) = Fixture::with_checkout_failure();

    let all = fixture.call("search_logs", json!({})).await;
    assert_eq!(all["logs"].as_array().unwrap().len(), 7);
    assert!(all["next_cursor"].is_null());
    let times = ids(&all["logs"], "time");
    assert!(
        times.windows(2).all(|w| w[0] >= w[1]),
        "newest first: {times:?}"
    );

    let warn = fixture
        .call("search_logs", json!({ "min_level": "warn" }))
        .await;
    assert_eq!(warn["logs"].as_array().unwrap().len(), 4);
    let error = fixture
        .call("search_logs", json!({ "min_level": "error" }))
        .await;
    assert_eq!(error["logs"].as_array().unwrap().len(), 3);
    assert!(
        error["logs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|l| l["level"] == "error")
    );

    // Text matches ignoring case, and combines with the level.
    let gateway = fixture
        .call("search_logs", json!({ "contains": "GATEWAY" }))
        .await;
    assert_eq!(gateway["logs"].as_array().unwrap().len(), 3);
    let both = fixture
        .call(
            "search_logs",
            json!({ "contains": "gateway", "min_level": "error" }),
        )
        .await;
    assert_eq!(both["logs"].as_array().unwrap().len(), 2);

    // Service and trace.
    let orders = fixture
        .call("search_logs", json!({ "service": "orders-api" }))
        .await;
    assert_eq!(orders["logs"].as_array().unwrap().len(), 3);
    let in_trace = fixture
        .call("search_logs", json!({ "trace_id": trace_id }))
        .await;
    assert_eq!(in_trace["logs"].as_array().unwrap().len(), 7);
    let elsewhere = fixture
        .call(
            "search_logs",
            json!({ "trace_id": "00000000000000000000000000000001" }),
        )
        .await;
    assert_eq!(elsewhere["logs"], json!([]));

    // Pages: two at a time, newest first, nothing repeated.
    let first = fixture.call("search_logs", json!({ "limit": 2 })).await;
    assert_eq!(first["logs"].as_array().unwrap().len(), 2);
    let second = fixture
        .call(
            "search_logs",
            json!({ "limit": 2, "cursor": first["next_cursor"] }),
        )
        .await;
    assert_eq!(second["logs"].as_array().unwrap().len(), 2);
    assert_ne!(first["logs"][0], second["logs"][0]);
    assert!(first["logs"][1]["time"].as_str() >= second["logs"][0]["time"].as_str());

    // The cap is 500, and `since` is applied to the log's own time.
    let capped = fixture
        .call("search_logs", json!({ "limit": 100_000 }))
        .await;
    assert_eq!(capped["logs"].as_array().unwrap().len(), 7);
    let late = fixture
        .call(
            "search_logs",
            json!({ "since": (BASE + 1_500 * MS).to_string() }),
        )
        .await;
    assert_eq!(late["logs"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn unknown_trace_is_tool_error() {
    let (fixture, trace_id) = Fixture::with_checkout_failure();
    let unknown = "00000000000000000000000000000001";

    assert_eq!(
        fixture
            .call_error("get_trace", json!({ "trace_id": unknown }))
            .await,
        "trace not found"
    );
    assert_eq!(
        fixture
            .call_error(
                "get_span",
                json!({ "trace_id": unknown, "span_id": "0000000000000001" })
            )
            .await,
        "trace not found"
    );
    assert_eq!(
        fixture
            .call_error(
                "get_span",
                json!({ "trace_id": trace_id, "span_id": "0000000000000001" })
            )
            .await,
        "span not found"
    );
}

#[tokio::test]
async fn results_have_structured_content_and_text() {
    let (fixture, trace_id) = Fixture::with_checkout_failure();
    let span_id = fixture
        .call("get_trace", json!({ "trace_id": trace_id }))
        .await["spans"][0]["span_id"]
        .as_str()
        .unwrap()
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
    let names: Vec<&str> = calls.iter().map(|(name, _)| *name).collect();
    assert_eq!(names, ALL_TOOLS);

    for (name, args) in calls {
        let result = fixture.tools.call(name, args.clone()).await;

        assert_eq!(result.is_error, Some(false), "{name}: {result:?}");
        let structured = result
            .structured_content
            .clone()
            .unwrap_or_else(|| panic!("{name}: no structured content"));
        assert!(
            structured.is_object(),
            "{name}: structuredContent must be an object (MCP 2025-11-25): {structured}"
        );
        // The text is the same JSON.
        let text: Value = serde_json::from_str(&text_of(&result)).unwrap();
        assert_eq!(
            text, structured,
            "{name}: text and structuredContent differ"
        );

        // And it matches the output schema the tool advertises (`call` checks).
        assert_eq!(fixture.call(name, args).await, structured, "{name}");
    }
}

// --- the tool list ----------------------------------------------------------------

#[test]
fn there_are_exactly_nine_read_only_tools() {
    let definitions = LoctelTools::definitions();

    let mut names: Vec<String> = definitions.iter().map(|t| t.name.to_string()).collect();
    names.sort();
    let mut expected: Vec<String> = ALL_TOOLS.iter().map(|n| (*n).to_owned()).collect();
    expected.sort();
    assert_eq!(names, expected);

    for tool in &definitions {
        assert!(
            tool.description.as_ref().is_some_and(|d| d.len() > 20),
            "{} needs a description an agent can act on",
            tool.name
        );
        assert!(
            tool.output_schema.is_some(),
            "{} has no output schema",
            tool.name
        );
        let annotations = tool.annotations.as_ref().expect("annotations");
        assert_eq!(annotations.read_only_hint, Some(true), "{}", tool.name);
    }
}

#[test]
fn every_output_schema_has_root_type_object() {
    // MCP 2025-11-25 requires `outputSchema.type == "object"`, and the
    // TypeScript SDK Claude Code uses parses `tools/list` against
    // `z.literal("object")`: one tool with an array root makes the whole list
    // fail, and the agent sees none of the tools. (2026-07-28 would allow it.)
    for tool in LoctelTools::definitions() {
        let schema = tool.output_schema.as_ref().expect("output schema");
        assert_eq!(
            schema.get("type"),
            Some(&json!("object")),
            "{}: the output schema's root must be `type: object`: {schema:?}",
            tool.name
        );
    }
}

#[tokio::test]
async fn call_dispatches_every_registered_tool() {
    // `call` routes by name on its own, so a tool added to the router and not
    // to `call` must not pass unnoticed.
    let fixture = Fixture::new();
    for tool in LoctelTools::definitions() {
        let result = fixture.tools.call(&tool.name, json!({})).await;
        let text = result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.clone())
            .unwrap_or_default();
        assert!(
            !text.starts_with("unknown tool"),
            "{} is registered but `call` does not know it",
            tool.name
        );
    }
}

#[tokio::test]
async fn an_unknown_tool_is_a_tool_error() {
    let fixture = Fixture::new();
    let text = fixture.call_error("drop_database", json!({})).await;
    assert_eq!(text, "unknown tool: drop_database");
}

#[tokio::test]
async fn bad_arguments_are_tool_errors_that_say_what_is_wrong() {
    let fixture = Fixture::new();

    let text = fixture
        .call_error("search_traces", json!({ "limit": "many" }))
        .await;
    assert!(
        text.starts_with("failed to deserialize parameters:"),
        "{text}"
    );
    // A misspelt argument is reported, not silently ignored.
    let text = fixture
        .call_error("search_traces", json!({ "min_duration": 5 }))
        .await;
    assert!(text.contains("min_duration"), "{text}");
    // A required one is missing.
    let text = fixture.call_error("get_trace", json!({})).await;
    assert!(text.contains("trace_id"), "{text}");
    // Not an object at all.
    let text = fixture.call_error("get_status", json!("hello")).await;
    assert!(
        text.starts_with("failed to deserialize parameters:"),
        "{text}"
    );
    // `null` and an omitted argument object both mean "no arguments".
    fixture.call("get_status", Value::Null).await;
    fixture.call("list_services", json!({})).await;
    // Malformed IDs and levels say which argument.
    let text = fixture
        .call_error("get_trace", json!({ "trace_id": "abc" }))
        .await;
    assert!(text.contains("trace_id"), "{text}");
    let text = fixture
        .call_error(
            "get_span",
            json!({ "trace_id": "00000000000000000000000000000001", "span_id": "xyz" }),
        )
        .await;
    assert!(text.contains("span_id"), "{text}");
    let text = fixture
        .call_error("search_logs", json!({ "min_level": "loud" }))
        .await;
    assert!(
        text.contains("min_level") || text.contains("loud"),
        "{text}"
    );
    let text = fixture
        .call_error("search_logs", json!({ "trace_id": "nothex" }))
        .await;
    assert!(text.contains("trace_id"), "{text}");
}

// --- since ----------------------------------------------------------------------------

#[tokio::test]
async fn since_takes_a_marker_or_rfc3339_and_nothing_else() {
    let fixture = Fixture::new();
    for tool in [
        "list_services",
        "get_service_map",
        "search_traces",
        "recent_failures",
        "search_logs",
    ] {
        // Both forms work.
        fixture
            .call(tool, json!({ "since": "1800000000000000000" }))
            .await;
        fixture
            .call(tool, json!({ "since": "2027-01-15T08:00:00.5+01:00" }))
            .await;
        // Anything else is a tool error that names `since`.
        for bad in [
            "yesterday",
            "",
            "12.5",
            "-5",
            "+5",
            "2027-01-15",
            "99999999999999999999999",
        ] {
            let text = fixture.call_error(tool, json!({ "since": bad })).await;
            assert!(text.contains("since"), "{tool} {bad:?}: {text}");
        }
        // A JSON number is not a marker: markers are strings.
        fixture
            .call_error(tool, json!({ "since": 1_800_000_000_000_000_000u64 }))
            .await;
    }
}

// --- status, marker, services, map -------------------------------------------------------

#[tokio::test]
async fn get_status_reports_what_the_source_says() {
    let fixture = Fixture::new();

    let r = fixture.call("get_status", json!({})).await;

    assert_eq!(r["paused"], true);
    assert_eq!(r["spans_per_sec"], 12.5);
    assert_eq!(r["logs_per_sec"], 3.0);
    assert_eq!(r["traces_held"], 4);
    assert_eq!(r["spans_held"], 60);
    assert_eq!(r["logs_held"], 28);
    assert_eq!(
        r["listeners"][0],
        json!({"name": "otlp-grpc", "address": "127.0.0.1:4317", "state": "listening"}),
        "no `error` key on a healthy listener"
    );
    assert_eq!(r["listeners"][1]["state"], "failed");
    assert_eq!(
        r["listeners"][1]["error"],
        "Address already in use (os error 98)"
    );
}

#[tokio::test]
async fn get_marker_is_the_stores_time_in_both_forms() {
    let fixture = Fixture::new();

    let r = fixture.call("get_marker", json!({})).await;

    assert_eq!(r["marker"], "1800000000000000000");
    assert_eq!(r["time"], "2027-01-15T08:00:00.000Z");

    fixture.clock.set(BASE + 1_234 * MS + 567_890);
    let r = fixture.call("get_marker", json!({})).await;
    assert_eq!(r["marker"], "1800000001234567890");
    assert_eq!(r["time"], "2027-01-15T08:00:01.234Z", "cut, not rounded");
}

#[tokio::test]
async fn list_services_counts_requests_and_errors() {
    let (fixture, _) = Fixture::with_checkout_failure();

    let r = fixture.call("list_services", json!({})).await;

    // An object, not a bare array: MCP 2025-11-25 clients accept nothing else.
    assert_eq!(r.as_object().unwrap().len(), 1, "only `services`: {r}");
    let services = r["services"].as_array().unwrap();
    let names: Vec<&str> = services
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "inventory-api",
            "notifications-worker",
            "orders-api",
            "payments-api",
            "web-bff"
        ]
    );
    let by_name = |name: &str| services.iter().find(|s| s["name"] == name).unwrap();
    assert_eq!(by_name("payments-api")["requests"], 1);
    assert_eq!(by_name("payments-api")["errors"], 1);
    assert_eq!(by_name("payments-api")["error_rate"], 1.0);
    assert_eq!(by_name("inventory-api")["errors"], 0);
    assert_eq!(by_name("inventory-api")["error_rate"], 0.0);
    // The last span of the web-bff started at the root's start.
    assert_eq!(by_name("web-bff")["last_seen"], "2027-01-15T08:00:00.030Z");
    assert_eq!(
        by_name("notifications-worker")["last_seen"],
        "2027-01-15T08:00:01.802Z"
    );

    // A window after everything is empty.
    let r = fixture
        .call(
            "list_services",
            json!({ "since": (BASE + 10 * SECOND).to_string() }),
        )
        .await;
    assert_eq!(r, json!({ "services": [] }));
}

#[tokio::test]
async fn get_service_map_returns_nodes_and_edges() {
    let (fixture, _) = Fixture::with_checkout_failure();

    let r = fixture.call("get_service_map", json!({})).await;

    let node = |id: &str| {
        r["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["id"] == id)
            .unwrap_or_else(|| panic!("no node {id}: {r}"))
    };
    assert_eq!(node("payments-api")["kind"], "service");
    assert_eq!(node("postgresql")["kind"], "database");
    assert_eq!(node("rabbitmq")["kind"], "queue");
    assert_eq!(node("card-gateway")["kind"], "external");
    assert_eq!(node("card-gateway")["errors"], 2);
    let edge = r["edges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["from"] == "orders-api" && e["to"] == "payments-api")
        .expect("orders-api to payments-api");
    assert_eq!(edge["calls"], 1);
    assert_eq!(edge["errors"], 1);
    assert!(
        !r["edges"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["from"] == "orders-api" && e["to"] == "notifications-worker"),
        "the queue sits between them"
    );

    let empty = fixture
        .call(
            "get_service_map",
            json!({ "since": (BASE + 10 * SECOND).to_string() }),
        )
        .await;
    assert_eq!(empty, json!({ "nodes": [], "edges": [] }));
}

// --- traces ---------------------------------------------------------------------------------

#[tokio::test]
async fn search_traces_filters_by_service_errors_and_duration() {
    let fixture = Fixture::new();
    load(&fixture.store, &Scenario::CheckoutSuccess, 1, BASE);
    load(
        &fixture.store,
        &Scenario::CheckoutFailure,
        1,
        BASE + 10 * SECOND,
    );
    load(
        &fixture.store,
        &Scenario::HealthChecks { count: 2 },
        1,
        BASE + 20 * SECOND,
    );
    settle(&fixture.clock);
    let count = |r: &Value| r["traces"].as_array().unwrap().len();

    assert_eq!(count(&fixture.call("search_traces", json!({})).await), 12);
    let errors = fixture
        .call("search_traces", json!({ "errors_only": true }))
        .await;
    assert_eq!(count(&errors), 1);
    assert_eq!(errors["traces"][0]["status"], "failed");
    let workers = fixture
        .call(
            "search_traces",
            json!({ "service": "notifications-worker" }),
        )
        .await;
    assert_eq!(count(&workers), 4, "the two checkouts and its two probes");
    let slow = fixture
        .call("search_traces", json!({ "min_duration_ms": 1000 }))
        .await;
    assert_eq!(count(&slow), 2);
    let none = fixture
        .call("search_traces", json!({ "min_duration_ms": 5000.5 }))
        .await;
    assert_eq!(count(&none), 0);
    assert!(none["next_cursor"].is_null());
    // `errors_only: false` is the same as leaving it out.
    let all = fixture
        .call(
            "search_traces",
            json!({ "errors_only": false, "service": "web-bff" }),
        )
        .await;
    assert_eq!(count(&all), 4, "checkout x2 and its two probes");

    // Newest first, and each summary has the plan's fields.
    let first = &fixture.call("search_traces", json!({ "limit": 1 })).await["traces"][0];
    assert_eq!(first["root_name"], "GET /health");
    assert_eq!(first["span_count"], 1);
    assert_eq!(first["complete"], true);
    assert_eq!(first["status"], "ok");
    assert!(first.get("failed_in").is_none(), "{first}");
    assert!(first.get("first_exception").is_none(), "{first}");
    let failed = &errors["traces"][0];
    assert_eq!(failed["root_name"], "POST /api/checkout");
    assert_eq!(failed["root_service"], "web-bff");
    assert_eq!(failed["start"], "2027-01-15T08:00:10.000Z");
    assert_eq!(failed["duration_ms"], 1840.0);
    assert_eq!(failed["span_count"], 15);
    assert_eq!(
        failed["services"],
        json!([
            "web-bff",
            "orders-api",
            "inventory-api",
            "payments-api",
            "notifications-worker"
        ])
    );
    assert_eq!(failed["failed_in"]["service"], "payments-api");
    assert_eq!(failed["failed_in"]["span_name"], "POST");
    assert_eq!(failed["failed_in"]["peer"], "card-gateway");
    assert_eq!(failed["failed_in"]["more"], 1);
    assert_eq!(failed["failed_in"]["span_id"].as_str().unwrap().len(), 16);
    assert_eq!(failed["first_exception"]["type"], EXCEPTION_TYPE);
    assert_eq!(failed["first_exception"]["message"], EXCEPTION_MESSAGE);
    assert!(
        failed["first_exception"].get("stacktrace").is_none(),
        "summaries stay small"
    );
    assert_eq!(failed["trace_id"].as_str().unwrap().len(), 32);
}

#[tokio::test]
async fn an_incomplete_trace_says_so() {
    let fixture = Fixture::new();
    load(&fixture.store, &Scenario::CheckoutFailure, 1, BASE);
    // Only a second has passed: the quiet period is 10 s.
    fixture.clock.advance(std::time::Duration::from_secs(1));

    let r = fixture.call("search_traces", json!({})).await;

    assert_eq!(r["traces"][0]["complete"], false);
}

#[tokio::test]
async fn errors_inside_is_reported_apart_from_failed() {
    let fixture = Fixture::new();
    // A request that worked: the call to payments failed and was retried.
    let spans = vec![
        span(1, 1, None, "web", "GET /cart", SpanKind::Server, (0, 100)),
        erroring(
            with_exception(
                span(1, 2, Some(1), "web", "POST", SpanKind::Client, (10, 30)),
                "TimeoutException",
                "no answer",
                "at web.Pay()",
            ),
            "timeout",
        ),
        span(1, 3, Some(1), "web", "POST", SpanKind::Client, (50, 30)),
    ];
    fixture.store.ingest_spans(spans);
    load(
        &fixture.store,
        &Scenario::CheckoutFailure,
        1,
        BASE + 5 * SECOND,
    );
    settle(&fixture.clock);

    let r = fixture.call("recent_failures", json!({})).await;

    assert_eq!(r["failed"].as_array().unwrap().len(), 1);
    assert_eq!(r["failed"][0]["trace"]["status"], "failed");
    let inside = r["errors_inside"].as_array().unwrap();
    assert_eq!(inside.len(), 1);
    assert_eq!(inside[0]["status"], "errors_inside");
    assert_eq!(inside[0]["failed_in"]["span_name"], "POST");
    assert_eq!(inside[0]["failed_in"]["service"], "web");
    assert_eq!(inside[0]["first_exception"]["type"], "TimeoutException");
    // Both show up when the agent only wants errors.
    let errors = fixture
        .call("search_traces", json!({ "errors_only": true }))
        .await;
    assert_eq!(errors["traces"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn recent_failures_limits_and_orders_newest_first() {
    let fixture = Fixture::new();
    let mut by_age = Vec::new();
    for n in 0..8u64 {
        let ids = load(
            &fixture.store,
            &Scenario::CheckoutFailure,
            100 + n,
            BASE + n * SECOND,
        );
        by_age.push(ids[0].to_string());
    }
    settle(&fixture.clock);
    let failed_ids = |r: &Value| -> Vec<String> {
        r["failed"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["trace"]["trace_id"].as_str().unwrap().to_owned())
            .collect()
    };

    let default = fixture.call("recent_failures", json!({})).await;
    assert_eq!(failed_ids(&default).len(), 5, "the default limit is 5");
    let expected: Vec<String> = by_age.iter().rev().take(5).cloned().collect();
    assert_eq!(failed_ids(&default), expected, "newest first");

    let three = fixture.call("recent_failures", json!({ "limit": 3 })).await;
    assert_eq!(failed_ids(&three).len(), 3);
    let capped = fixture
        .call("recent_failures", json!({ "limit": 500 }))
        .await;
    assert_eq!(failed_ids(&capped).len(), 8, "20 at most, and only 8 exist");
    let one = fixture.call("recent_failures", json!({ "limit": 0 })).await;
    assert_eq!(failed_ids(&one).len(), 1, "at least one");
}

#[tokio::test]
async fn recent_failures_gives_the_oldest_ten_error_logs_oldest_first() {
    let fixture = Fixture::new();
    let spans = vec![erroring(
        span(1, 1, None, "api", "GET /x", SpanKind::Server, (0, 500)),
        "boom",
    )];
    fixture.store.ingest_spans(spans);
    let mut logs = vec![
        log("api", 5, Severity::Info, "starting", Some((1, 1))),
        log("api", 6, Severity::Warn, "slow", Some((1, 1))),
        // On another trace: not this one's.
        log("api", 7, Severity::Error, "other trace", Some((2, 9))),
        // No trace context at all.
        log("api", 8, Severity::Error, "no context", None),
    ];
    for i in 0..15u64 {
        logs.push(log(
            "api",
            10 + i,
            if i % 2 == 0 {
                Severity::Error
            } else {
                Severity::Fatal
            },
            &format!("failure {i:02}"),
            Some((1, 1)),
        ));
    }
    fixture.store.ingest_logs(logs);
    settle(&fixture.clock);

    let r = fixture.call("recent_failures", json!({})).await;

    let error_logs = r["failed"][0]["error_logs"].as_array().unwrap();
    assert_eq!(error_logs.len(), 10);
    let messages: Vec<&str> = error_logs
        .iter()
        .map(|l| l["message"].as_str().unwrap())
        .collect();
    let expected: Vec<String> = (0..10).map(|i| format!("failure {i:02}")).collect();
    assert_eq!(messages, expected, "the first ten, oldest first");
    assert_eq!(error_logs[0]["level"], "error");
    assert_eq!(error_logs[1]["level"], "fatal");
    assert_eq!(error_logs[0]["service"], "api");
    assert_eq!(error_logs[0]["span_id"], span_id(1).to_string());
    // No exception event on the failed-in span: the key is absent.
    assert!(r["failed"][0].get("exception").is_none(), "{r}");
    // The failed-in span in brief.
    let brief = &r["failed"][0]["failed_in_span"];
    assert_eq!(brief["name"], "GET /x");
    assert_eq!(brief["kind"], "server");
    assert_eq!(brief["status"], "error");
    assert_eq!(brief["start_offset_ms"], 0.0);
    assert_eq!(brief["duration_ms"], 500.0);
    assert!(brief.get("parent_span_id").is_none());
    assert!(brief.get("exception_type").is_none());
}

#[tokio::test]
async fn get_trace_summary_includes_the_path_to_the_failure() {
    let (fixture, trace_id) = Fixture::with_checkout_failure();

    let r = fixture
        .call("get_trace", json!({ "trace_id": trace_id }))
        .await;

    // The path from the root to the failed-in span (web-bff POST /api/checkout,
    // POST, orders-api POST /orders, POST, payments-api POST /charges, POST)
    // plus every span at depth 2 or less (those, and validate_token): 7 of 15.
    let spans = r["spans"].as_array().unwrap();
    assert_eq!(spans.len(), 7, "{r}");
    assert_eq!(r["omitted"], 8);
    assert!(r["next_cursor"].is_string());
    let failed_in = r["summary"]["failed_in"]["span_id"].as_str().unwrap();
    assert!(spans.iter().any(|s| s["span_id"] == failed_in));
    let names: Vec<&str> = spans.iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"validate_token"));
    assert!(names.contains(&"POST /charges"));
    assert!(!names.contains(&"SELECT carts"), "depth 3, off the path");
    // The page reads in start order, and each span says where it hangs.
    let offsets: Vec<f64> = spans
        .iter()
        .map(|s| s["start_offset_ms"].as_f64().unwrap())
        .collect();
    assert!(offsets.windows(2).all(|w| w[0] <= w[1]), "{offsets:?}");
    assert!(
        spans[0].get("parent_span_id").is_none(),
        "the root has none"
    );
    assert!(spans[1]["parent_span_id"].is_string());
    assert_eq!(r["summary"]["status"], "failed");
    assert_eq!(r["summary"]["complete"], true);

    // With a tight limit the path wins over the other top-level spans.
    let tight = fixture
        .call("get_trace", json!({ "trace_id": trace_id, "limit": 6 }))
        .await;
    let tight_spans = tight["spans"].as_array().unwrap();
    assert_eq!(tight_spans.len(), 6);
    assert!(tight_spans.iter().any(|s| s["span_id"] == failed_in));
    assert!(tight_spans.iter().all(|s| s["name"] != "validate_token"));
    assert_eq!(tight["omitted"], 9);
}

#[tokio::test]
async fn a_cursor_belongs_to_one_detail_level() {
    let (fixture, trace_id) = Fixture::with_checkout_failure();
    let summary = fixture
        .call("get_trace", json!({ "trace_id": trace_id }))
        .await;

    let text = fixture
        .call_error(
            "get_trace",
            json!({ "trace_id": trace_id, "detail": "full", "cursor": summary["next_cursor"] }),
        )
        .await;
    assert!(text.contains("cursor"), "{text}");
    let text = fixture
        .call_error(
            "get_trace",
            json!({ "trace_id": trace_id, "cursor": "garbage" }),
        )
        .await;
    assert!(text.contains("cursor"), "{text}");
}

// --- sizing ------------------------------------------------------------------------------------

#[tokio::test]
async fn long_messages_and_stack_traces_are_cut_at_2000_characters() {
    let fixture = Fixture::new();
    let long = "é".repeat(3_000);
    let exactly = "x".repeat(2_000);
    let cut = format!("{}…(truncated)", "é".repeat(2_000));
    fixture.store.ingest_spans(vec![with_exception(
        erroring(
            span(1, 1, None, "api", "GET /x", SpanKind::Server, (0, 10)),
            "boom",
        ),
        "Boom",
        &long,
        &long,
    )]);
    fixture.store.ingest_logs(vec![
        log("api", 5, Severity::Error, &long, Some((1, 1))),
        log("api", 6, Severity::Error, &exactly, Some((1, 1))),
    ]);
    settle(&fixture.clock);

    let failures = fixture.call("recent_failures", json!({})).await;
    let f = &failures["failed"][0];
    assert_eq!(f["exception"]["message"], cut.as_str());
    assert_eq!(f["exception"]["stacktrace"], cut.as_str());
    assert_eq!(f["trace"]["first_exception"]["message"], cut.as_str());
    assert_eq!(f["error_logs"][0]["message"], cut.as_str());
    assert_eq!(
        f["error_logs"][1]["message"],
        exactly.as_str(),
        "2000 is not cut"
    );

    let logs = fixture.call("search_logs", json!({})).await;
    assert_eq!(logs["logs"][1]["message"], cut.as_str());

    let span = fixture
        .call(
            "get_span",
            json!({ "trace_id": trace_id(1).to_string(), "span_id": span_id(1).to_string() }),
        )
        .await;
    assert_eq!(span["span"]["exception"]["stacktrace"], cut.as_str());
    assert_eq!(
        span["span"]["events"][0]["attributes"]["exception.stacktrace"],
        cut.as_str(),
        "the event carries the same text"
    );
    assert_eq!(span["logs"][0]["message"], cut.as_str());
}

#[tokio::test]
async fn get_span_without_events_or_logs_is_small() {
    let fixture = Fixture::new();
    fixture.store.ingest_spans(vec![span(
        1,
        1,
        None,
        "api",
        "GET /x",
        SpanKind::Server,
        (0, 10),
    )]);

    let r = fixture
        .call(
            "get_span",
            json!({ "trace_id": trace_id(1).to_string(), "span_id": span_id(1).to_string() }),
        )
        .await;

    assert_eq!(r["logs"], json!([]));
    assert_eq!(r["span"]["events"], json!([]));
    assert_eq!(r["span"]["attributes"], json!({}));
    assert_eq!(r["span"]["status"], "unset");
    assert!(r["span"].get("exception").is_none());
    assert!(r["span"].get("parent_span_id").is_none());
}

#[tokio::test]
async fn orphans_are_listed_even_without_their_parent() {
    let fixture = Fixture::new();
    // The parent (span 1) has not arrived: span 2 hangs from nothing yet.
    fixture.store.ingest_spans(vec![
        span(1, 2, Some(1), "api", "child", SpanKind::Internal, (5, 10)),
        span(
            1,
            3,
            Some(2),
            "api",
            "grandchild",
            SpanKind::Internal,
            (6, 2),
        ),
    ]);

    let r = fixture
        .call("get_trace", json!({ "trace_id": trace_id(1).to_string() }))
        .await;

    assert_eq!(r["spans"].as_array().unwrap().len(), 2);
    assert_eq!(r["summary"]["root_name"], Value::Null);
    assert_eq!(r["summary"]["root_service"], Value::Null);
    assert_eq!(r["omitted"], 0);
}

// --- query arguments (Milestone 2) -------------------------------------------------------

/// The user's example: spans named X with an ancestor from service Y.
const STRUCTURAL_QUERY: &str =
    r#"{ resource.service.name = "web-bff" } >> { name = "POST /charges" }"#;

/// The LogQL example the `search_logs` description gives.
const LOGQL_EXAMPLE: &str = r#"{service_name="payments-api"} |= "declined" | level = "error""#;

/// The column an error text names, after `prefix` ("invalid TraceQL").
fn column_of(text: &str, prefix: &str) -> usize {
    let rest = text
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix(": at column "))
        .unwrap_or_else(|| panic!("`{prefix}: at column N: ...` expected, got {text:?}"));
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits
        .parse()
        .unwrap_or_else(|_| panic!("no column number in {text:?}"))
}

#[tokio::test]
async fn search_traces_with_traceql_structural_query() {
    let (fixture, trace_id) = Fixture::with_checkout_failure();
    // Traces the query must pass over.
    load(
        &fixture.store,
        &Scenario::HealthChecks { count: 3 },
        2,
        BASE + 5 * SECOND,
    );
    settle(&fixture.clock);
    // The span the query is after, found without the query: the one server
    // span called `POST /charges`.
    let full = fixture
        .call(
            "get_trace",
            json!({ "trace_id": trace_id, "detail": "full" }),
        )
        .await;
    let charges: Vec<&str> = full["spans"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|span| span["name"] == "POST /charges")
        .map(|span| span["span_id"].as_str().unwrap())
        .collect();
    assert_eq!(charges.len(), 1, "{full}");

    let r = fixture
        .call("search_traces", json!({ "query": STRUCTURAL_QUERY }))
        .await;

    assert_eq!(ids(&r["traces"], "trace_id"), vec![trace_id.clone()]);
    assert_eq!(r["traces"][0]["matched_span_ids"], json!(charges));
    assert_eq!(r["traces"][0]["status"], "failed", "a summary as usual");
    assert!(r["next_cursor"].is_null());

    // Without a query there are no matched spans, and the key is not there.
    let all = fixture.call("search_traces", json!({})).await;
    assert_eq!(all["traces"].as_array().unwrap().len(), 16);
    for trace in all["traces"].as_array().unwrap() {
        assert!(trace.get("matched_span_ids").is_none(), "{trace}");
    }
    // Other tools never carry them either: `get_trace` is a summary of the
    // trace, not of a search.
    let one = fixture
        .call("get_trace", json!({ "trace_id": trace_id }))
        .await;
    assert!(one["summary"].get("matched_span_ids").is_none());

    // A query that matches nothing finds nothing, without an error.
    let none = fixture
        .call(
            "search_traces",
            json!({ "query": r#"{ resource.service.name = "no-such-service" } >> { }"# }),
        )
        .await;
    assert_eq!(none["traces"], json!([]));
    assert!(none["next_cursor"].is_null());

    // A blank query is no query, as a blank `service` or `cursor` is.
    for blank in ["", "  \n"] {
        let r = fixture
            .call("search_traces", json!({ "query": blank, "limit": 100 }))
            .await;
        assert_eq!(r["traces"].as_array().unwrap().len(), 16, "{blank:?}");
    }
}

#[tokio::test]
async fn search_traces_query_combines_with_filters_and_pages() {
    let fixture = Fixture::new();
    load(&fixture.store, &Scenario::CheckoutSuccess, 1, BASE);
    let failed = load(
        &fixture.store,
        &Scenario::CheckoutFailure,
        1,
        BASE + 10 * SECOND,
    );
    settle(&fixture.clock);
    let query = r#"{ name = "POST /charges" }"#;
    let count = |r: &Value| r["traces"].as_array().unwrap().len();

    // Both checkouts have the span; newest first, one at a time.
    let first = fixture
        .call("search_traces", json!({ "query": query, "limit": 1 }))
        .await;
    assert_eq!(count(&first), 1);
    assert_eq!(first["traces"][0]["trace_id"], failed[0].to_string());
    assert_eq!(
        first["traces"][0]["matched_span_ids"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let second = fixture
        .call(
            "search_traces",
            json!({ "query": query, "limit": 1, "cursor": first["next_cursor"] }),
        )
        .await;
    assert_eq!(count(&second), 1);
    assert_ne!(
        second["traces"][0]["trace_id"],
        first["traces"][0]["trace_id"]
    );
    assert!(
        second["next_cursor"].is_null(),
        "two traces match, so the second page is the last"
    );

    // Every other argument still applies, and all must hold.
    let errors = fixture
        .call(
            "search_traces",
            json!({ "query": query, "errors_only": true }),
        )
        .await;
    assert_eq!(ids(&errors["traces"], "trace_id"), [failed[0].to_string()]);
    let service = fixture
        .call(
            "search_traces",
            json!({ "query": query, "service": "no-such-service" }),
        )
        .await;
    assert_eq!(count(&service), 0);
    let since = fixture
        .call(
            "search_traces",
            json!({ "query": query, "since": (BASE + 5 * SECOND).to_string() }),
        )
        .await;
    assert_eq!(count(&since), 1);

    // A query can count spans, like Tempo's pipelines.
    let busy = fixture
        .call(
            "search_traces",
            json!({ "query": "{ kind = client } | count() > 100" }),
        )
        .await;
    assert_eq!(count(&busy), 0);
}

#[tokio::test]
async fn search_traces_invalid_traceql_is_tool_error_with_column() {
    let fixture = Fixture::new();

    // A feature Grafana has and loctel does not: named, with its position.
    let text = fixture
        .call_error(
            "search_traces",
            json!({ "query": "{ status = error } | select(span.foo)" }),
        )
        .await;
    assert_eq!(
        text,
        "invalid TraceQL: at column 22: select is not supported yet"
    );

    // A typo: the column is the one `loctel_query` reports (1-based).
    let text = fixture
        .call_error("search_traces", json!({ "query": "{ name = }" }))
        .await;
    assert_eq!(column_of(&text, "invalid TraceQL"), 10, "{text}");
    let text = fixture
        .call_error("search_traces", json!({ "query": r#"{ name = "x""# }))
        .await;
    assert!(text.starts_with("invalid TraceQL: at column "), "{text}");
    let text = fixture
        .call_error("search_traces", json!({ "query": "name = 1" }))
        .await;
    assert_eq!(column_of(&text, "invalid TraceQL"), 1, "{text}");

    // It is a tool error even beside a good `since`, and a LogQL query is not
    // a TraceQL one.
    let text = fixture
        .call_error(
            "search_traces",
            json!({ "since": "1800000000000000000", "query": r#"{service_name="a"}"# }),
        )
        .await;
    assert!(text.starts_with("invalid TraceQL: at column "), "{text}");
}

#[tokio::test]
async fn search_logs_with_logql() {
    let (fixture, trace_id) = Fixture::with_checkout_failure();
    let call = |args: Value| fixture.call("search_logs", args);
    let count = |r: &Value| r["logs"].as_array().unwrap().len();

    // A stream selector: the service's logs, newest first.
    let payments = call(json!({ "query": r#"{service_name="payments-api"}"# })).await;
    assert_eq!(count(&payments), 4);
    assert!(
        payments["logs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|log| log["service"] == "payments-api")
    );
    let times = ids(&payments["logs"], "time");
    assert!(times.windows(2).all(|w| w[0] >= w[1]), "{times:?}");

    // A label filter on the severity label.
    let errors =
        call(json!({ "query": r#"{service_name="payments-api"} | level = "error""# })).await;
    assert_eq!(count(&errors), 2);
    assert!(
        errors["logs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|log| log["level"] == "error")
    );
    // Line filters: `|~` is unanchored, `!=` drops lines that contain the text.
    let attempt =
        call(json!({ "query": r#"{service_name="payments-api"} |~ "attempt [0-9]""# })).await;
    assert_eq!(count(&attempt), 1);
    assert!(
        attempt["logs"][0]["message"]
            .as_str()
            .unwrap()
            .contains("attempt")
    );
    let rest = call(json!({ "query": r#"{service_name="payments-api"} != "card-gateway""# })).await;
    assert_eq!(count(&rest), 1);
    // Label matchers are anchored.
    let partial = call(json!({ "query": r#"{service_name=~"payments"}"# })).await;
    assert_eq!(count(&partial), 0);
    let anchored = call(json!({ "query": r#"{service_name=~"payments.*"}"# })).await;
    assert_eq!(count(&anchored), 4);
    // The log's trace stays linked.
    assert!(
        payments["logs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|log| log["trace_id"] == trace_id)
    );

    // Every other argument still applies, and all must hold.
    let both = call(json!({
        "query": r#"{service_name="payments-api"}"#,
        "min_level": "error",
        "contains": "gateway",
    }))
    .await;
    assert_eq!(count(&both), 2);
    let other_service = call(json!({
        "query": r#"{service_name="payments-api"}"#,
        "service": "orders-api",
    }))
    .await;
    assert_eq!(count(&other_service), 0);
    let in_trace = call(json!({
        "query": r#"{service_name="payments-api"}"#,
        "trace_id": "00000000000000000000000000000001",
    }))
    .await;
    assert_eq!(count(&in_trace), 0);

    // Pages: two at a time, nothing repeated, the cursor ends.
    let first = call(json!({ "query": r#"{service_name=~".+"}"#, "limit": 4 })).await;
    assert_eq!(count(&first), 4);
    let second = call(json!({
        "query": r#"{service_name=~".+"}"#,
        "limit": 4,
        "cursor": first["next_cursor"],
    }))
    .await;
    assert_eq!(count(&second), 3, "seven logs in all");
    assert!(second["next_cursor"].is_null());

    // A blank query is no query.
    let blank = call(json!({ "query": "" })).await;
    assert_eq!(count(&blank), 7);
}

#[tokio::test]
async fn search_logs_invalid_logql_is_tool_error() {
    let fixture = Fixture::new();

    let text = fixture
        .call_error(
            "search_logs",
            json!({ "query": r#"{service_name="x"} | line_format "{{.a}}""# }),
        )
        .await;
    assert_eq!(
        text,
        "invalid LogQL: at column 22: line_format is not supported yet"
    );

    let text = fixture
        .call_error("search_logs", json!({ "query": "{}" }))
        .await;
    assert_eq!(column_of(&text, "invalid LogQL"), 1, "{text}");
    assert!(text.contains("at least one matcher"), "{text}");

    // A TraceQL query is not a LogQL one (`resource.service.name` is no label).
    let text = fixture
        .call_error("search_logs", json!({ "query": STRUCTURAL_QUERY }))
        .await;
    assert!(text.starts_with("invalid LogQL: at column "), "{text}");
}

#[test]
fn the_search_tools_describe_their_query_language() {
    let definitions = LoctelTools::definitions();
    let describe = |name: &str| -> String {
        let tool = definitions.iter().find(|tool| tool.name == name).unwrap();
        let schema = Value::Object((*tool.input_schema).clone());
        format!(
            "{} {}",
            tool.description.as_deref().unwrap_or_default(),
            schema["properties"]["query"]["description"]
                .as_str()
                .unwrap_or_default()
        )
    };

    let traces = describe("search_traces");
    assert!(traces.contains("TraceQL"), "{traces}");
    assert!(traces.contains("Tempo"), "{traces}");
    assert!(
        traces.contains(STRUCTURAL_QUERY),
        "one structural example: {traces}"
    );
    assert!(traces.contains("anchored"), "{traces}");

    let logs = describe("search_logs");
    assert!(logs.contains("LogQL"), "{logs}");
    assert!(logs.contains("Loki"), "{logs}");
    assert!(logs.contains(LOGQL_EXAMPLE), "one example: {logs}");
    assert!(logs.contains("anchored"), "{logs}");

    // An example an agent copies must work.
    loctel_query::traceql::parse(STRUCTURAL_QUERY).expect("the TraceQL example parses");
    loctel_query::logql::parse(LOGQL_EXAMPLE).expect("the LogQL example parses");
}

#[tokio::test]
async fn status_lists_rules_and_dropped_fraction() {
    let status = StatusDto {
        rules: vec![
            RuleDto {
                name: "Health checks".to_owned(),
                language: RuleLanguageDto::TraceQl,
                scope: Some(RuleScopeDto::Trace),
                enabled: true,
                error: None,
                dropped_spans: 12,
                dropped_logs: 3,
                dropped_traces: 4,
            },
            RuleDto {
                name: "Noisy spans".to_owned(),
                language: RuleLanguageDto::TraceQl,
                scope: Some(RuleScopeDto::Span),
                enabled: true,
                error: Some("at column 3: expected an operator".to_owned()),
                dropped_spans: 0,
                dropped_logs: 0,
                dropped_traces: 0,
            },
            RuleDto {
                name: "Debug logs".to_owned(),
                language: RuleLanguageDto::LogQl,
                scope: None,
                enabled: false,
                error: None,
                dropped_spans: 0,
                dropped_logs: 41,
                dropped_traces: 0,
            },
        ],
        rules_file_error: Some("TOML parse error at line 3, column 1".to_owned()),
        dropped_fraction: 0.3125,
        ..sample_status()
    };
    let fixture = Fixture::with_status(Arc::new(FakeStatus(status)));

    let r = fixture.call("get_status", json!({})).await;

    assert_eq!(
        r["rules"],
        json!([
            {
                "name": "Health checks",
                "language": "traceql",
                "scope": "trace",
                "enabled": true,
                "dropped_spans": 12,
                "dropped_logs": 3,
                "dropped_traces": 4
            },
            {
                "name": "Noisy spans",
                "language": "traceql",
                "scope": "span",
                "enabled": true,
                "error": "at column 3: expected an operator",
                "dropped_spans": 0,
                "dropped_logs": 0,
                "dropped_traces": 0
            },
            {
                "name": "Debug logs",
                "language": "logql",
                "enabled": false,
                "dropped_spans": 0,
                "dropped_logs": 41,
                "dropped_traces": 0
            }
        ]),
        "`scope` only for TraceQL, `error` only when there is one"
    );
    assert_eq!(
        r["rules_file_error"],
        "TOML parse error at line 3, column 1"
    );
    assert_eq!(r["dropped_fraction"], 0.3125);
    // The rest of the status is as before.
    assert_eq!(r["paused"], true);
    assert_eq!(r["traces_held"], 4);
}

#[tokio::test]
async fn status_without_rules_says_so() {
    // The default source (no rules, a good file): an empty list, a `null`
    // file error that is still there, and nothing dropped.
    let fixture = Fixture::new();

    let r = fixture.call("get_status", json!({})).await;

    assert_eq!(r["rules"], json!([]));
    assert_eq!(r.get("rules_file_error"), Some(&Value::Null), "{r}");
    assert_eq!(r["dropped_fraction"], 0.0);
}

#[tokio::test]
async fn status_cuts_long_rule_text_at_2000_characters() {
    let long = "x".repeat(2_500);
    let cut = format!("{}…(truncated)", "x".repeat(2_000));
    let status = StatusDto {
        rules: vec![RuleDto {
            name: long.clone(),
            language: RuleLanguageDto::LogQl,
            scope: None,
            enabled: true,
            error: Some(long.clone()),
            dropped_spans: 0,
            dropped_logs: 0,
            dropped_traces: 0,
        }],
        rules_file_error: Some(long),
        ..sample_status()
    };
    let fixture = Fixture::with_status(Arc::new(FakeStatus(status)));

    let r = fixture.call("get_status", json!({})).await;

    assert_eq!(r["rules"][0]["name"], cut.as_str());
    assert_eq!(r["rules"][0]["error"], cut.as_str());
    assert_eq!(r["rules_file_error"], cut.as_str());
}
