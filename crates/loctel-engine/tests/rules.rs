//! The discard rules in the running engine: the rules file at start, rules
//! applied at ingest on real sockets, live reload, the handle API and the
//! status. Each test has a directory of its own, every port 0 and a rules path
//! in it, and drives the engine with the fake mesh over HTTP JSON.

use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use loctel_engine::{
    DropScope, Engine, EngineConfig, EngineHandle, ListenerState, RuleId, RuleLanguage, RuleSpec,
    RulesFile, Settings,
};
use loctel_meshgen::{Generated, Scenario, SendReport, Transport, generate};
use serde::Deserialize;

/// How long a test waits for the engine to notice an edit of the file (the
/// poll is every 2 s).
const RELOAD_WAIT: Duration = Duration::from_secs(5);

const NANOS_PER_SEC: u64 = 1_000_000_000;

fn now_unix_nano() -> u64 {
    let since_epoch = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    u64::try_from(since_epoch.as_nanos()).unwrap()
}

/// An engine with every port 0 and the rules in `dir/rules.toml`.
fn start(dir: &Path) -> EngineHandle {
    start_with(Some(dir.join("rules.toml")))
}

fn start_with(rules_path: Option<PathBuf>) -> EngineHandle {
    Engine::start(EngineConfig {
        settings: Settings {
            otlp_grpc_port: 0,
            otlp_http_port: 0,
            mcp_port: 0,
            ..Settings::default()
        },
        rules_path,
        ..EngineConfig::default()
    })
    .expect("the engine starts")
}

fn http_addr(engine: &EngineHandle) -> SocketAddr {
    let listener = engine
        .status()
        .listeners
        .into_iter()
        .find(|l| l.name == "otlp-http")
        .expect("an otlp-http listener");
    match listener.state {
        ListenerState::Listening(addr) => addr,
        ListenerState::Failed(why) => panic!("otlp-http failed: {why}"),
    }
}

/// Sends `scenario` over HTTP JSON, dated `base_unix_nano`, in one go. The
/// engine owns its own runtime, so the send gets a small one of its own.
fn send_at(
    engine: &EngineHandle,
    scenario: &Scenario,
    seed: u64,
    base_unix_nano: u64,
) -> (Generated, SendReport) {
    let generated = generate(scenario, seed, base_unix_nano);
    let transport = Transport::HttpJson(format!("http://{}", http_addr(engine)));
    let report = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(loctel_meshgen::send(&generated, &transport, 0.0))
        .unwrap_or_else(|e| panic!("sending {scenario:?} failed: {e:#}"));
    (generated, report)
}

/// [`send_at`], dated now.
fn send(engine: &EngineHandle, scenario: &Scenario, seed: u64) -> (Generated, SendReport) {
    send_at(engine, scenario, seed, now_unix_nano())
}

const HEALTH: Scenario = Scenario::HealthChecks { count: 2 };

/// The rule the new file shows commented out: it drops health checks.
fn example_rule() -> RuleSpec {
    #[derive(Deserialize)]
    struct Document {
        rule: Vec<RuleSpec>,
    }
    let mut document: Document = toml::from_str(RulesFile::EXAMPLE_RULE).unwrap();
    assert_eq!(document.rule.len(), 1);
    document.rule.remove(0)
}

fn rule(name: &str, language: RuleLanguage, query: &str) -> RuleSpec {
    RuleSpec {
        name: name.to_owned(),
        enabled: true,
        language,
        query: query.to_owned(),
        scope: DropScope::Trace,
    }
}

/// Polls `condition` for up to [`RELOAD_WAIT`].
fn wait_for(what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + RELOAD_WAIT;
    while !condition() {
        assert!(Instant::now() < deadline, "gave up waiting for {what}");
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn first_start_writes_empty_rules_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.toml");
    let engine = start(dir.path());

    // The file exists and shows the example, commented out.
    let text = fs::read_to_string(&path).expect("the file was written");
    for line in RulesFile::EXAMPLE_RULE.lines() {
        assert!(
            text.contains(&format!("# {line}\n")),
            "no `# {line}` in {text}"
        );
    }
    assert_eq!(RulesFile::load(&path).unwrap(), Some(vec![]));
    assert_eq!(engine.rules_path(), Some(path));
    let status = engine.status();
    assert!(status.rules.is_empty(), "{:?}", status.rules);
    assert_eq!(status.rules_file_error, None);
    assert!(engine.rules().is_empty());

    // loctel never decides what to discard: health checks are stored.
    let (health, report) = send(&engine, &HEALTH, 1);
    let stats = engine.store().stats();
    assert_eq!(stats.traces, health.trace_ids.len());
    assert_eq!(stats.spans, report.spans);
    assert_eq!(stats.logs, report.logs);
    let status = engine.status();
    assert_eq!((status.dropped_spans, status.dropped_logs), (0, 0));
    assert_eq!(status.dropped_fraction, 0.0);
    engine.shutdown();
}

#[test]
fn health_checks_are_dropped_end_to_end_in_process() {
    let dir = tempfile::tempdir().unwrap();
    let engine = start(dir.path());
    engine.set_rules(vec![example_rule()]).unwrap();

    let (health, health_report) = send(&engine, &HEALTH, 1);
    let (_, checkout_report) = send(&engine, &Scenario::CheckoutFailure, 2);

    // Only the checkout is stored, spans and logs.
    let stats = engine.store().stats();
    assert_eq!(stats.traces, 1);
    assert_eq!(stats.spans, checkout_report.spans);
    assert_eq!(stats.logs, checkout_report.logs);
    assert_eq!(stats.spans, 15);
    assert_eq!(stats.logs, 7);
    let status = engine.status();
    assert_eq!(status.rules.len(), 1);
    assert_eq!(
        status.rules[0].dropped_traces,
        health.trace_ids.len() as u64
    );
    assert_eq!(status.rules[0].dropped_spans, health_report.spans as u64);
    assert_eq!(status.rules[0].dropped_logs, health_report.logs as u64);
    assert_eq!(status.rules[0].error, None);
    assert_eq!(status.dropped_spans, health_report.spans as u64);
    assert_eq!(status.dropped_logs, health_report.logs as u64);
    engine.shutdown();
}

#[test]
fn a_trace_whose_root_arrives_last_is_dropped_whole() {
    let dir = tempfile::tempdir().unwrap();
    let engine = start(dir.path());
    // Matches only the root, which this scenario sends last: the leaves and
    // the middle spans are stored before the rule can see it.
    let root = rule(
        "Root",
        RuleLanguage::TraceQl,
        r#"{ name = "POST /api/checkout" }"#,
    );
    engine.set_rules(vec![root]).unwrap();

    let (generated, report) = send(&engine, &Scenario::LateOutOfOrder, 1);

    // Everything of the trace is gone, what was stored before the match too.
    let stats = engine.store().stats();
    assert_eq!((stats.traces, stats.spans, stats.logs), (0, 0, 0));
    let rules = engine.rules();
    assert_eq!(rules[0].dropped_traces, 1);
    assert_eq!(rules[0].dropped_spans, report.spans as u64);
    assert_eq!(rules[0].dropped_logs, report.logs as u64);
    assert_eq!(generated.trace_ids.len(), 1);
    engine.shutdown();
}

#[test]
fn an_existing_file_is_loaded_at_start() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.toml");
    RulesFile::save(&path, &[example_rule()]).unwrap();
    let before = fs::read_to_string(&path).unwrap();

    let engine = start(dir.path());

    assert_eq!(engine.rules().len(), 1);
    assert_eq!(engine.rules()[0].spec, example_rule());
    assert_eq!(engine.status().rules_file_error, None);
    send(&engine, &HEALTH, 1);
    assert_eq!(engine.store().stats().spans, 0, "the loaded rule applies");
    // Starting never rewrites a file that is there.
    assert_eq!(fs::read_to_string(&path).unwrap(), before);
    engine.shutdown();
}

#[test]
fn a_broken_file_at_start_means_no_rules_and_an_error_and_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.toml");
    fs::write(&path, "[[rule]\nname = ").unwrap();

    let engine = start(dir.path());

    let status = engine.status();
    assert!(status.rules.is_empty());
    let error = status.rules_file_error.expect("the error is reported");
    assert!(error.contains("rules.toml"), "{error}");
    assert_eq!(fs::read_to_string(&path).unwrap(), "[[rule]\nname = ");
    // The engine runs: nothing is dropped.
    send(&engine, &HEALTH, 1);
    assert!(engine.store().stats().spans > 0);

    // Fixing the file is picked up, and the error goes.
    RulesFile::save(&path, &[example_rule()]).unwrap();
    wait_for("the fixed file", || engine.status().rules.len() == 1);
    assert_eq!(engine.status().rules_file_error, None);
    engine.shutdown();
}

#[test]
fn file_edit_is_picked_up() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.toml");
    let engine = start(dir.path());
    engine.set_rules(vec![example_rule()]).unwrap();
    send(&engine, &HEALTH, 1);
    assert_eq!(engine.store().stats().spans, 0, "the rule drops them");

    // A person switches the rule off in their editor.
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("enabled = true"), "{text}");
    fs::write(&path, text.replace("enabled = true", "enabled = false")).unwrap();

    wait_for("the edit", || !engine.rules()[0].spec.enabled);
    let (health, report) = send(&engine, &HEALTH, 2);
    let stats = engine.store().stats();
    assert_eq!(stats.traces, health.trace_ids.len());
    assert_eq!(stats.spans, report.spans);
    // The rule is the same rule: it kept its counters through the reload.
    assert_eq!(
        engine.rules()[0].dropped_traces,
        generate(&HEALTH, 1, 0).trace_ids.len() as u64
    );
    engine.shutdown();
}

#[test]
fn broken_rules_file_keeps_previous_rules() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.toml");
    let engine = start(dir.path());
    engine.set_rules(vec![example_rule()]).unwrap();
    let before = engine.rules();

    // A typo in the TOML.
    fs::write(&path, "[[rule]\nname = \"Health checks\"\n").unwrap();
    wait_for("the error", || engine.status().rules_file_error.is_some());

    // The rules stay as they were, and still drop.
    let error = engine.status().rules_file_error.unwrap();
    assert!(error.contains("rules.toml"), "{error}");
    assert_eq!(engine.rules(), before);
    assert_eq!(engine.status().rules, before);
    send(&engine, &HEALTH, 1);
    assert_eq!(engine.store().stats().spans, 0);

    // Fixing the file clears the error and applies what it says now.
    let mut off = example_rule();
    off.enabled = false;
    RulesFile::save(&path, &[off]).unwrap();
    wait_for("the error to clear", || {
        engine.status().rules_file_error.is_none()
    });
    assert!(!engine.rules()[0].spec.enabled);
    assert_eq!(engine.rules()[0].id, before[0].id, "the same rule");
    engine.shutdown();
}

#[test]
fn a_bad_query_in_the_file_disables_only_that_rule() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.toml");
    let broken = rule("Broken", RuleLanguage::TraceQl, "{ span.foo = }");
    RulesFile::save(&path, &[broken, example_rule()]).unwrap();

    let engine = start(dir.path());

    // The file is fine; the rule is reported, and the other still applies.
    assert_eq!(engine.status().rules_file_error, None);
    let rules = engine.rules();
    assert!(rules[0].error.is_some(), "{:?}", rules[0]);
    assert_eq!(rules[1].error, None);
    send(&engine, &HEALTH, 1);
    assert_eq!(engine.store().stats().spans, 0);
    engine.shutdown();
}

#[test]
fn set_rules_saves_and_applies() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.toml");
    let engine = start(dir.path());
    let draft = rule("Draft", RuleLanguage::LogQl, r#"{service_name="x"} |= "#);
    let specs = vec![example_rule(), draft.clone()];

    // An invalid query is allowed: an editor can save a draft.
    engine.set_rules(specs.clone()).unwrap();

    // It is in the file, in order, and in the status.
    assert_eq!(RulesFile::load(&path).unwrap(), Some(specs.clone()));
    let rules = engine.rules();
    assert_eq!(
        rules.iter().map(|r| r.spec.clone()).collect::<Vec<_>>(),
        specs
    );
    assert_eq!(rules[0].error, None);
    assert!(rules[1].error.is_some());
    assert_eq!(engine.status().rules, rules);
    // And it applies at once.
    send(&engine, &HEALTH, 1);
    assert_eq!(engine.store().stats().spans, 0);

    // Setting the same rules again keeps the counters.
    engine.set_rules(specs).unwrap();
    assert!(engine.rules()[0].dropped_traces > 0);
    assert_eq!(engine.rules()[0].id, rules[0].id);

    // Clearing the list writes a file with no rules.
    engine.set_rules(vec![]).unwrap();
    assert_eq!(RulesFile::load(&path).unwrap(), Some(vec![]));
    assert!(engine.rules().is_empty());
    engine.shutdown();
}

#[test]
fn set_rules_into_an_unwritable_place_is_an_error_and_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    // The rules path is below a plain file, so it cannot be written.
    let blocker = dir.path().join("blocker");
    fs::write(&blocker, "x").unwrap();
    let engine = start_with(Some(blocker.join("rules.toml")));
    // Start did not manage to write the empty file: it says so.
    assert!(engine.status().rules_file_error.is_some());

    let result = engine.set_rules(vec![example_rule()]);

    assert!(result.is_err());
    assert!(engine.rules().is_empty());
    send(&engine, &HEALTH, 1);
    assert!(engine.store().stats().spans > 0);
    engine.shutdown();
}

#[test]
fn without_a_path_rules_live_in_memory_only() {
    let engine = start_with(None);
    assert_eq!(engine.rules_path(), None);
    assert!(engine.rules().is_empty());
    assert_eq!(engine.status().rules_file_error, None);

    engine.set_rules(vec![example_rule()]).unwrap();

    assert_eq!(engine.rules().len(), 1);
    send(&engine, &HEALTH, 1);
    assert_eq!(engine.store().stats().spans, 0);
    let id = engine.rules()[0].id;
    engine.set_rule_enabled(id, false).unwrap();
    assert!(!engine.rules()[0].spec.enabled);
    engine.shutdown();
}

#[test]
fn set_rule_enabled_saves() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.toml");
    let engine = start(dir.path());
    engine.set_rules(vec![example_rule()]).unwrap();
    let id = engine.rules()[0].id;

    engine.set_rule_enabled(id, false).unwrap();

    assert!(!engine.rules()[0].spec.enabled);
    assert_eq!(engine.rules()[0].id, id, "turning off keeps the identity");
    let saved = RulesFile::load(&path).unwrap().unwrap();
    assert_eq!(saved.len(), 1);
    assert!(!saved[0].enabled);
    let (health, _) = send(&engine, &HEALTH, 1);
    assert_eq!(engine.store().stats().traces, health.trace_ids.len());

    engine.set_rule_enabled(id, true).unwrap();
    assert!(RulesFile::load(&path).unwrap().unwrap()[0].enabled);
    send(&engine, &HEALTH, 2);
    assert_eq!(
        engine.store().stats().traces,
        health.trace_ids.len(),
        "the rule drops again"
    );

    // A rule that is not there is an error, and nothing changes.
    assert!(engine.set_rule_enabled(RuleId(u64::MAX), false).is_err());
    assert!(engine.rules()[0].spec.enabled);
    engine.shutdown();
}

#[test]
fn preview_rule_over_last_five_minutes() {
    let dir = tempfile::tempdir().unwrap();
    let engine = start(dir.path());
    let now = now_unix_nano();
    // Health checks from ten minutes ago are outside the window; the recent
    // ones and a failing checkout are inside it.
    send_at(&engine, &HEALTH, 1, now - 10 * 60 * NANOS_PER_SEC);
    let (recent, recent_report) = send_at(&engine, &HEALTH, 2, now);
    send_at(&engine, &Scenario::CheckoutFailure, 3, now);

    let preview = engine.preview_rule(&example_rule()).unwrap();

    assert_eq!(preview.traces, recent.trace_ids.len() as u64);
    assert_eq!(preview.spans, recent_report.spans as u64);
    assert_eq!(preview.logs, recent_report.logs as u64);
    assert_eq!(preview.error_traces, 0);
    assert_eq!(preview.error_logs, 0);
    // The rule did not run: nothing was removed, nothing counted.
    assert_eq!(
        engine.store().stats().traces,
        2 * recent.trace_ids.len() + 1
    );

    let failing = rule("Errors", RuleLanguage::TraceQl, "{ status = error }");
    let preview = engine.preview_rule(&failing).unwrap();
    assert_eq!(preview.traces, 1);
    assert_eq!(preview.error_traces, 1);

    // A query that does not parse is an error with its position.
    let error = engine
        .preview_rule(&rule("Bad", RuleLanguage::TraceQl, "{ span.x = }"))
        .unwrap_err();
    assert!(error.to_string().contains("column"), "{error}");
    engine.shutdown();
}

#[test]
fn dropped_fraction_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let engine = start(dir.path());
    engine.set_rules(vec![example_rule()]).unwrap();
    assert_eq!(engine.status().dropped_fraction, 0.0, "nothing has arrived");

    // Only what is accepted counts on the other side: the checkout, whole.
    let (_, checkout) = send(&engine, &Scenario::CheckoutFailure, 1);
    let status = engine.status();
    assert_eq!((status.dropped_spans, status.dropped_logs), (0, 0));
    assert_eq!(status.dropped_fraction, 0.0);

    let (_, health) = send(&engine, &HEALTH, 2);

    let status = engine.status();
    assert_eq!(status.dropped_spans, health.spans as u64);
    assert_eq!(status.dropped_logs, health.logs as u64);
    let dropped = (health.spans + health.logs) as f64;
    let accepted = (checkout.spans + checkout.logs) as f64;
    assert!(
        (status.dropped_fraction - dropped / (dropped + accepted)).abs() < 1e-9,
        "{} for {dropped} dropped and {accepted} accepted",
        status.dropped_fraction
    );
    assert!(status.dropped_fraction > 0.0 && status.dropped_fraction < 1.0);
    engine.shutdown();
}

#[test]
fn rates_count_what_the_rules_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let engine = start(dir.path());
    engine.set_rules(vec![example_rule()]).unwrap();

    send(&engine, &HEALTH, 1);

    // Everything was dropped: nothing was received, as far as the rate goes.
    let status = engine.status();
    assert_eq!(status.spans_per_sec, 0.0);
    assert_eq!(status.logs_per_sec, 0.0);

    send(&engine, &Scenario::CheckoutFailure, 2);
    let status = engine.status();
    assert!(status.spans_per_sec > 0.0, "{status:?}");
    assert!(status.logs_per_sec > 0.0, "{status:?}");
    engine.shutdown();
}

#[test]
fn paused_drops_before_rules() {
    let dir = tempfile::tempdir().unwrap();
    let engine = start(dir.path());
    engine.set_rules(vec![example_rule()]).unwrap();

    engine.set_paused(true);
    send(&engine, &HEALTH, 1);

    // A paused engine drops the batch before the rules see it: the rule's
    // counters, the totals and the fraction do not move.
    let status = engine.status();
    assert_eq!(status.rules[0].dropped_traces, 0);
    assert_eq!(status.rules[0].dropped_spans, 0);
    assert_eq!(status.rules[0].dropped_logs, 0);
    assert_eq!((status.dropped_spans, status.dropped_logs), (0, 0));
    assert_eq!(status.dropped_fraction, 0.0);
    assert_eq!(engine.store().stats().spans, 0);

    engine.set_paused(false);
    let (health, _) = send(&engine, &HEALTH, 2);
    assert_eq!(
        engine.status().rules[0].dropped_traces,
        health.trace_ids.len() as u64
    );
    engine.shutdown();
}

// --- MCP `get_status` (plan Task 7) ----------------------------------------------

/// Calls `get_status` with a real MCP client, as an agent does, on a runtime of
/// the test's own (the engine has its own).
fn mcp_status(engine: &EngineHandle) -> serde_json::Value {
    use rmcp::ServiceExt;
    use rmcp::transport::StreamableHttpClientTransport;
    use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;

    let url = engine.mcp_url().expect("the MCP server is up");
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let transport = StreamableHttpClientTransport::from_config(
                StreamableHttpClientTransportConfig::with_uri(url),
            );
            let client = rmcp::model::ClientConfig::default()
                .serve(transport)
                .await
                .unwrap();
            let result = client
                .call_tool(rmcp::model::CallToolRequestParams::new("get_status"))
                .await
                .unwrap();
            let _ = client.cancel().await;
            assert_eq!(result.is_error, Some(false), "{result:?}");
            result.structured_content.expect("structured content")
        })
}

#[test]
fn mcp_get_status_lists_the_rules_the_engine_has() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.toml");
    let engine = start(dir.path());
    let mut span_scope = rule("Slow spans", RuleLanguage::TraceQl, r#"{ name = "SLOW" }"#);
    span_scope.scope = DropScope::Span;
    let mut debug_logs = rule("Debug logs", RuleLanguage::LogQl, r#"{level="debug"}"#);
    debug_logs.enabled = false;
    engine
        .set_rules(vec![
            example_rule(),
            span_scope,
            debug_logs,
            rule("Typo", RuleLanguage::TraceQl, "{ name = }"),
        ])
        .unwrap();
    let (_, checkout) = send(&engine, &Scenario::CheckoutFailure, 1);
    let (health_traces, health) = send(&engine, &HEALTH, 2);

    let status = mcp_status(&engine);

    // The same rules, in the same order, with the same counts as the handle.
    let rules = status["rules"].as_array().unwrap();
    assert_eq!(rules.len(), 4, "{status}");
    let own = engine.status();
    assert_eq!(rules[0]["name"], own.rules[0].spec.name);
    assert_eq!(rules[0]["language"], "traceql");
    assert_eq!(rules[0]["scope"], "trace");
    assert_eq!(rules[0]["enabled"], true);
    assert!(rules[0].get("error").is_none(), "{}", rules[0]);
    assert_eq!(
        rules[0]["dropped_traces"],
        health_traces.trace_ids.len() as u64
    );
    assert_eq!(rules[0]["dropped_spans"], health.spans as u64);
    assert_eq!(rules[0]["dropped_logs"], health.logs as u64);
    assert_eq!(rules[1]["scope"], "span");
    assert_eq!(rules[2]["language"], "logql");
    assert_eq!(rules[2]["enabled"], false);
    assert!(rules[2].get("scope").is_none(), "no scope for LogQL");
    assert_eq!(rules[3]["name"], "Typo");
    assert_eq!(rules[3]["error"], own.rules[3].error.clone().unwrap());
    assert!(
        rules[3]["error"]
            .as_str()
            .unwrap()
            .starts_with("at column "),
        "{}",
        rules[3]
    );

    // The fraction is the engine's own, and no file error is the key with
    // `null`.
    assert_eq!(status["rules_file_error"], serde_json::Value::Null);
    let dropped = (health.spans + health.logs) as f64;
    let accepted = (checkout.spans + checkout.logs) as f64;
    let fraction = status["dropped_fraction"].as_f64().unwrap();
    assert!(
        (fraction - dropped / (dropped + accepted)).abs() < 1e-9,
        "{fraction}"
    );
    assert_eq!(status["spans_held"], checkout.spans as u64);

    // A broken file shows up as the error, and the rules stay.
    fs::write(&path, "[[rule]\n").unwrap();
    wait_for("the error", || engine.status().rules_file_error.is_some());
    let status = mcp_status(&engine);
    assert_eq!(
        status["rules_file_error"],
        engine.status().rules_file_error.unwrap()
    );
    assert_eq!(status["rules"].as_array().unwrap().len(), 4);
    engine.shutdown();
}
