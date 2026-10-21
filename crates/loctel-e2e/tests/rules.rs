//! The discard rules through the whole path (design section 7, M2 plan
//! Task 9): a `rules.toml` on disk, real OTLP in, and a real MCP client
//! reading what is left and what loctel says it dropped.
//!
//! Each test writes its own rules file in a temporary directory and starts the
//! engine on it with `Harness::start_with_rules`. The rule is the one a new
//! `rules.toml` shows commented out (`RulesFile::EXAMPLE_RULE`), so these tests
//! also prove that the example in the file works as written.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use loctel_e2e::{Harness, TransportKind, now_unix_nano};
use loctel_engine::RulesFile;
use loctel_meshgen::{Generated, Scenario, generate};
use serde_json::{Value, json};

/// The engine polls the rules file every 2 s; a test waits for a few polls.
const RELOAD_WAIT: Duration = Duration::from_secs(10);

/// Writes `text` as the rules file of a test.
fn write_rules(dir: &Path, text: &str) -> std::path::PathBuf {
    let path = dir.join("rules.toml");
    fs::write(&path, text).expect("the rules file is written");
    path
}

/// How many of `traces` (a `search_traces` answer) are health checks.
fn health_traces(found: &Value) -> usize {
    found["traces"]
        .as_array()
        .unwrap_or_else(|| panic!("no traces in {found}"))
        .iter()
        .filter(|trace| trace["root_name"] == "GET /health")
        .count()
}

/// Polls `get_status` until `condition` holds of it, for [`RELOAD_WAIT`].
async fn wait_for_status(harness: &Harness, what: &str, condition: impl Fn(&Value) -> bool) {
    let deadline = Instant::now() + RELOAD_WAIT;
    loop {
        let status = harness.tool("get_status", json!({})).await;
        if condition(&status) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "gave up waiting for {what}; last status: {status}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn health_rule_silences_health_checks_over_otlp() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_rules(dir.path(), RulesFile::EXAMPLE_RULE);
    let harness = Harness::start_with_rules(Duration::from_millis(200), path);

    // Five probes for each of the five services over gRPC, then a failing
    // checkout over HTTP JSON: the noise and the signal, by different roads.
    let health = harness
        .send(
            &Scenario::HealthChecks { count: 5 },
            TransportKind::Grpc,
            1,
            0.0,
        )
        .await;
    let checkout = harness
        .send(&Scenario::CheckoutFailure, TransportKind::HttpJson, 2, 0.0)
        .await;
    harness.wait_complete(&checkout.trace_ids[0]).await;
    assert_eq!(
        health.trace_ids.len(),
        25,
        "5 probes for each of 5 services"
    );

    // The traces: the checkout is there and no health check is.
    let found = harness.tool("search_traces", json!({ "limit": 100 })).await;
    let traces = found["traces"].as_array().unwrap();
    assert_eq!(traces.len(), 1, "{found}");
    assert_eq!(traces[0]["trace_id"], checkout.trace_ids[0].to_string());
    assert_eq!(traces[0]["root_name"], "POST /api/checkout", "{found}");
    assert_eq!(health_traces(&found), 0, "{found}");
    for trace in &health.trace_ids {
        let missing = harness
            .mcp()
            .call("get_trace", json!({ "trace_id": trace.to_string() }))
            .await;
        assert!(missing.is_error, "health trace {trace} was stored");
        assert_eq!(missing.text, "trace not found");
    }

    // The logs: the checkout's seven and not one `GET /health 200`.
    let logs = harness.tool("search_logs", json!({ "limit": 500 })).await;
    let logs = logs["logs"].as_array().unwrap();
    assert_eq!(logs.len(), 7, "{logs:?}");
    assert!(
        logs.iter().all(|log| log["message"] != "GET /health 200"),
        "{logs:?}"
    );
    assert!(
        logs.iter()
            .all(|log| log["trace_id"] == checkout.trace_ids[0].to_string()),
        "{logs:?}"
    );

    // What loctel says about it: one rule, 5 probes of each of the 5
    // services dropped (a trace each), and a share of the traffic gone.
    let status = harness.tool("get_status", json!({})).await;
    let rules = status["rules"].as_array().unwrap();
    assert_eq!(rules.len(), 1, "{status}");
    let rule = &rules[0];
    assert_eq!(rule["name"], "Health checks", "{rule}");
    assert_eq!(rule["language"], "traceql", "{rule}");
    assert_eq!(rule["scope"], "trace", "{rule}");
    assert_eq!(rule["enabled"], true, "{rule}");
    assert!(rule.get("error").is_none_or(Value::is_null), "{rule}");
    assert_eq!(rule["dropped_traces"], 5 * 5, "{rule}");
    assert_eq!(rule["dropped_spans"], 25, "{rule}");
    assert_eq!(rule["dropped_logs"], 25, "{rule}");
    assert_eq!(status["rules_file_error"], Value::Null, "{status}");
    // 25 spans and 25 logs dropped, 15 spans and 7 logs kept.
    let fraction = status["dropped_fraction"].as_f64().unwrap();
    assert!(fraction > 0.0, "{status}");
    assert!((fraction - 50.0 / 72.0).abs() < 1e-9, "{status}");
    assert_eq!(status["traces_held"], 1, "{status}");
    assert_eq!(status["spans_held"], 15, "{status}");
    assert_eq!(status["logs_held"], 7, "{status}");
}

#[tokio::test]
async fn late_spans_of_dropped_trace_never_appear() {
    let dir = tempfile::tempdir().unwrap();
    // The rule matches the root by name, and `LateOutOfOrder` sends the root
    // last: the other 13 spans (and their 6 logs) are stored before the rule
    // can see it. The default scope is the whole trace.
    let path = write_rules(
        dir.path(),
        r#"[[rule]]
name = "Checkout roots"
enabled = true
language = "traceql"
query = '{ name = "POST /api/checkout" }'
"#,
    );
    let harness = Harness::start_with_rules(Duration::from_millis(200), path);

    // With its real pauses, so each of the three batches is its own request.
    let late = harness
        .send(&Scenario::LateOutOfOrder, TransportKind::HttpJson, 1, 1.0)
        .await;
    let trace_id = late.trace_ids[0].to_string();

    // The trace is gone whole, the spans stored before the match included.
    let status = harness.tool("get_status", json!({})).await;
    assert_eq!(status["traces_held"], 0, "{status}");
    assert_eq!(status["spans_held"], 0, "{status}");
    assert_eq!(status["logs_held"], 0, "{status}");
    let rule = &status["rules"][0];
    assert_eq!(
        rule["scope"], "trace",
        "the scope defaults to trace: {rule}"
    );
    assert_eq!(rule["dropped_traces"], 1, "{rule}");
    assert_eq!(rule["dropped_spans"], 14, "{rule}");
    assert_eq!(rule["dropped_logs"], 6, "{rule}");

    // The same trace comes again (the same seed is the same trace ID), this
    // time with the root first and the rest of it straggling in after it, one
    // request at a time, as an exporter that retries late would send it. None
    // of those later spans matches the rule by itself, yet none is stored,
    // because loctel remembers that the trace was dropped.
    let again = generate(&Scenario::LateOutOfOrder, 1, now_unix_nano());
    assert_eq!(again.trace_ids, late.trace_ids);
    assert_eq!(again.batches.len(), 3, "deep spans, middle spans, root");
    let transport = harness.otlp(TransportKind::Grpc);
    for index in [2, 1, 0] {
        let batch = Generated {
            batches: vec![again.batches[index].clone()],
            trace_ids: vec![],
        };
        loctel_meshgen::send(&batch, &transport, 0.0)
            .await
            .unwrap_or_else(|e| panic!("sending batch {index} failed: {e:#}"));
        let status = harness.tool("get_status", json!({})).await;
        assert_eq!(status["traces_held"], 0, "after batch {index}: {status}");
        assert_eq!(status["spans_held"], 0, "after batch {index}: {status}");
        assert_eq!(status["logs_held"], 0, "after batch {index}: {status}");
    }
    let status = harness.tool("get_status", json!({})).await;
    let rule = &status["rules"][0];
    assert_eq!(rule["dropped_traces"], 1, "one trace, dropped once: {rule}");
    assert_eq!(rule["dropped_spans"], 28, "{rule}");
    assert_eq!(rule["dropped_logs"], 12, "{rule}");

    let missing = harness
        .mcp()
        .call("get_trace", json!({ "trace_id": trace_id }))
        .await;
    assert!(missing.is_error, "the dropped trace is stored again");
    assert_eq!(missing.text, "trace not found");
    let found = harness.tool("search_traces", json!({})).await;
    assert_eq!(found["traces"], json!([]), "{found}");
    let logs = harness
        .tool("search_logs", json!({ "trace_id": trace_id }))
        .await;
    assert_eq!(logs["logs"], json!([]), "{logs}");
}

#[tokio::test]
async fn disabling_rule_in_file_restores_flow() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_rules(dir.path(), RulesFile::EXAMPLE_RULE);
    let harness = Harness::start_with_rules(Duration::from_millis(200), path.clone());

    // With the rule on, health checks do not arrive.
    harness
        .send(
            &Scenario::HealthChecks { count: 2 },
            TransportKind::HttpJson,
            1,
            0.0,
        )
        .await;
    let found = harness.tool("search_traces", json!({ "limit": 100 })).await;
    assert_eq!(health_traces(&found), 0, "{found}");
    let status = harness.tool("get_status", json!({})).await;
    assert_eq!(status["rules"][0]["enabled"], true, "{status}");
    assert_eq!(status["rules"][0]["dropped_traces"], 10, "{status}");

    // The user edits the file and saves it: `enabled = false`. Within a few
    // seconds loctel has noticed, with no restart and no click.
    let edited = RulesFile::EXAMPLE_RULE.replace("enabled = true", "enabled = false");
    assert_ne!(edited, RulesFile::EXAMPLE_RULE);
    fs::write(&path, edited).unwrap();
    wait_for_status(&harness, "the rule to be turned off", |status| {
        status["rules"][0]["enabled"] == false
    })
    .await;

    // Health checks flow again, and the rule keeps what it dropped before.
    harness
        .send(
            &Scenario::HealthChecks { count: 2 },
            TransportKind::HttpJson,
            2,
            0.0,
        )
        .await;
    let found = harness.tool("search_traces", json!({ "limit": 100 })).await;
    assert_eq!(health_traces(&found), 10, "{found}");
    let logs = harness
        .tool(
            "search_logs",
            json!({ "contains": "GET /health", "limit": 500 }),
        )
        .await;
    assert_eq!(logs["logs"].as_array().unwrap().len(), 10, "{logs}");

    let status = harness.tool("get_status", json!({})).await;
    assert_eq!(status["rules_file_error"], Value::Null, "{status}");
    let rule = &status["rules"][0];
    assert_eq!(rule["enabled"], false, "{rule}");
    assert_eq!(rule["dropped_traces"], 10, "the old count is kept: {rule}");
    assert_eq!(rule["dropped_spans"], 10, "{rule}");
}
