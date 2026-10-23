//! The engine and the disk (Milestone 5, Task 4): the run directory it makes
//! for the store's segment files and deletes when it stops, the disk cap from
//! the settings and from `set_disk_cap`, and what the status (and so the MCP
//! `get_status`) says about it. Real sockets on port 0, the fake mesh over HTTP
//! JSON, and a store that spills as soon as a trace is quiet (50 ms).

use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use loctel_engine::{Engine, EngineConfig, EngineHandle, ListenerState, Settings, StoreConfig};
use loctel_meshgen::{Scenario, Transport, generate};
use loctel_model::{AttrValue, LogRecord, Resource, Severity};

const GIB: u64 = 1024 * 1024 * 1024;

/// Every port 0, a cap of 3 GiB in the settings, and a store that spills at
/// once. With `base` the engine makes its run directory in it.
fn config(base: Option<&Path>) -> EngineConfig {
    EngineConfig {
        settings: Settings {
            otlp_grpc_port: 0,
            otlp_http_port: 0,
            mcp_port: 0,
            disk_cap_bytes: 3 * GIB,
            ..Settings::default()
        },
        store: StoreConfig {
            spill_dir: None,
            completeness_quiet: Duration::from_millis(50),
            ..StoreConfig::spilling_for_tests(PathBuf::new())
        },
        spill_base: base.map(Path::to_path_buf),
        ..EngineConfig::default()
    }
}

fn now_unix_nano() -> u64 {
    let since_epoch = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    u64::try_from(since_epoch.as_nanos()).unwrap()
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

/// Sends the failing checkout (15 spans, 7 logs) over HTTP JSON, dated now.
fn send_checkout(engine: &EngineHandle, seed: u64) {
    let generated = generate(&Scenario::CheckoutFailure, seed, now_unix_nano());
    let transport = Transport::HttpJson(format!("http://{}", http_addr(engine)));
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(loctel_meshgen::send(&generated, &transport, 0.0))
        .unwrap_or_else(|e| panic!("sending the checkout failed: {e:#}"));
}

/// Polls `condition` for up to ten seconds.
fn wait_for(what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(Instant::now() < deadline, "gave up waiting for {what}");
        thread::sleep(Duration::from_millis(25));
    }
}

/// The run directories in `base`: the entries called `run-...`.
fn run_dirs(base: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(base)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("run-"))
        })
        .collect();
    dirs.sort();
    dirs
}

fn segment_bytes(dir: &Path) -> u64 {
    fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap())
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".seg"))
        .map(|entry| entry.metadata().unwrap().len())
        .sum()
}

#[test]
fn engine_status_reports_disk() {
    let base = tempfile::tempdir().unwrap();
    let engine = Engine::start(config(Some(base.path()))).unwrap();

    // Nothing written yet: the cap is the one in the settings.
    let status = engine.status();
    assert_eq!(status.disk_bytes, 0);
    assert_eq!(
        status.disk_cap_bytes,
        3 * GIB,
        "from Settings::disk_cap_bytes"
    );
    assert_eq!(status.spill_error, None);
    assert_eq!(engine.store().disk_cap_bytes(), 3 * GIB);

    // A trace that has gone quiet moves to the run directory, and the status
    // counts the files.
    send_checkout(&engine, 1);
    wait_for("the trace to spill", || {
        engine.status().stats.spilled_traces == 1
    });
    let status = engine.status();
    assert!(status.disk_bytes > 0, "{status:?}");
    assert_eq!(status.stats.disk_bytes, status.disk_bytes);
    assert_eq!(status.disk_cap_bytes, 3 * GIB);
    assert_eq!(status.spill_error, None);
    let runs = run_dirs(base.path());
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert_eq!(segment_bytes(&runs[0]), status.disk_bytes);
    // Still all there, from the file.
    assert_eq!(status.stats.traces, 1);
    assert_eq!(status.stats.spans, 15);

    // Lowering the cap applies at once.
    engine.set_disk_cap(1);
    let status = engine.status();
    assert_eq!(status.disk_cap_bytes, 1);
    assert_eq!(status.disk_bytes, 0, "the file went with the cap");
    assert_eq!(status.stats.traces, 0, "and the trace in it");
    assert_eq!(segment_bytes(&runs[0]), 0);

    // Raising it lets data stay again.
    engine.set_disk_cap(10 * GIB);
    assert_eq!(engine.status().disk_cap_bytes, 10 * GIB);
    send_checkout(&engine, 2);
    wait_for("the next trace to spill", || {
        engine.status().stats.spilled_traces == 1
    });
    assert!(engine.status().disk_bytes > 0);
    engine.shutdown();
}

/// `count` logs of one service, a microsecond apart from `first_micro` on, so
/// that none is another's retry (the store drops those) and the oldest is the
/// first.
fn logs(first_micro: u64, count: usize) -> Vec<LogRecord> {
    let resource = Arc::new(Resource {
        attributes: vec![(
            "service.name".to_owned(),
            AttrValue::Str("bulk-api".to_owned()),
        )],
    });
    (0..count as u64)
        .map(|i| {
            let time = (first_micro + i) * 1_000;
            LogRecord {
                time_unix_nano: time,
                observed_unix_nano: time,
                severity: Severity::Info,
                severity_text: "INFO".to_owned(),
                body: format!("line {}", first_micro + i),
                attributes: vec![],
                trace_id: None,
                span_id: None,
                resource: Arc::clone(&resource),
                scope_name: "bulk".to_owned(),
            }
        })
        .collect()
}

#[test]
fn a_spilling_engine_keeps_more_logs_than_a_store_in_memory_does() {
    // The app's engine has a run directory, and a store that has one is not held
    // to the 5,000,000 logs of a store in memory (decision 0053): the Logs
    // screen is meant to scroll through ten million, and the disk cap decides
    // what goes.
    let in_memory = StoreConfig::default().max_log_records;
    let base = tempfile::tempdir().unwrap();
    let engine = Engine::start(EngineConfig {
        // The store's own sizes (a chunk of 65,536 logs), not the tiny ones the
        // other tests spill with: this is five million logs.
        store: StoreConfig::default(),
        ..config(Some(base.path()))
    })
    .unwrap();

    let total = in_memory + 100_000;
    let batch = 100_000;
    let first = 1_700_000_000_000_000_u64 / 1_000;
    for start in (0..total).step_by(batch) {
        engine
            .store()
            .ingest_logs(logs(first + start as u64, batch.min(total - start)));
    }

    let stats = engine.status().stats;
    assert_eq!(stats.logs_received, total as u64);
    assert_eq!(
        stats.logs, total,
        "the oldest logs were evicted at the limit of a store in memory: {stats:?}"
    );
    engine.shutdown();
}

#[test]
fn engine_without_a_spill_base_keeps_everything_in_memory() {
    let engine = Engine::start(EngineConfig {
        settings: config(None).settings,
        ..EngineConfig::default()
    })
    .unwrap();
    send_checkout(&engine, 1);
    wait_for("the trace", || engine.status().stats.traces == 1);
    let status = engine.status();
    assert_eq!(status.disk_bytes, 0);
    assert_eq!(status.disk_cap_bytes, 3 * GIB, "the cap is still reported");
    assert_eq!(status.spill_error, None);
    assert_eq!(status.stats.spilled_traces, 0);
    engine.shutdown();
}

#[test]
fn engine_shutdown_deletes_run_dir() {
    let base = tempfile::tempdir().unwrap();
    // A run that was killed left its directory, with a segment in it.
    let stale = base.path().join("run-999999-1700000000");
    fs::create_dir(&stale).unwrap();
    fs::write(stale.join("traces-000000.seg"), b"left behind").unwrap();
    // Something that is not a run's is left alone.
    fs::write(base.path().join("notes.txt"), b"mine").unwrap();

    let engine = Engine::start(config(Some(base.path()))).unwrap();
    assert!(
        !stale.exists(),
        "a dead run's directory is deleted at start"
    );
    let runs = run_dirs(base.path());
    assert_eq!(runs.len(), 1, "one directory for this run: {runs:?}");
    let name = runs[0].file_name().unwrap().to_str().unwrap().to_owned();
    assert!(
        name.starts_with(&format!("run-{}-", std::process::id())),
        "{name}"
    );

    send_checkout(&engine, 1);
    wait_for("the trace to spill", || {
        engine.status().stats.spilled_traces == 1
    });
    assert!(segment_bytes(&runs[0]) > 0, "files are in it");

    engine.shutdown();
    assert!(!runs[0].exists(), "the run's directory goes at shutdown");
    assert!(base.path().join("notes.txt").exists());
    // Shutting down again is fine.
    engine.shutdown();
}

#[test]
fn dropping_the_last_handle_deletes_the_run_dir_too() {
    let base = tempfile::tempdir().unwrap();
    let engine = Engine::start(config(Some(base.path()))).unwrap();
    let runs = run_dirs(base.path());
    assert_eq!(runs.len(), 1);
    let clone = engine.clone();
    drop(engine);
    assert!(runs[0].exists(), "a clone is still running the engine");
    drop(clone);
    assert!(!runs[0].exists());
}

#[test]
fn a_run_dir_that_cannot_be_made_is_reported_and_loctel_runs_in_memory() {
    let dir = tempfile::tempdir().unwrap();
    // A file where the base directory should be.
    let blocked = dir.path().join("loctel");
    fs::write(&blocked, b"not a directory").unwrap();

    let engine = Engine::start(config(Some(&blocked))).unwrap();

    let status = engine.status();
    let error = status
        .spill_error
        .expect("the status says why there is no disk");
    assert!(error.contains("run directory"), "{error}");
    send_checkout(&engine, 1);
    wait_for("the trace", || engine.status().stats.traces == 1);
    assert_eq!(engine.status().stats.spilled_traces, 0, "kept in memory");
    assert_eq!(engine.status().disk_bytes, 0);
    engine.shutdown();
}

// --- MCP ---------------------------------------------------------------------------

#[test]
fn mcp_status_carries_the_engine_disk_fields() {
    use rmcp::ServiceExt;
    use rmcp::transport::StreamableHttpClientTransport;
    use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;

    let base = tempfile::tempdir().unwrap();
    let engine = Engine::start(config(Some(base.path()))).unwrap();
    send_checkout(&engine, 1);
    wait_for("the trace to spill", || {
        engine.status().stats.spilled_traces == 1
    });
    let status = engine.status();

    let url = engine.mcp_url().expect("the MCP server is up");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let reported = runtime.block_on(async {
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
    });

    assert_eq!(reported["disk_bytes"], status.disk_bytes);
    assert!(reported["disk_bytes"].as_u64().unwrap() > 0, "{reported}");
    assert_eq!(reported["disk_cap_bytes"], 3 * GIB);
    assert_eq!(reported.get("spill_error"), Some(&serde_json::Value::Null));
    engine.shutdown();
}
