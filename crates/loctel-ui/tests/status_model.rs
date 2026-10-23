//! How the engine's status becomes what the view and the menu show.

use std::net::SocketAddr;

use loctel_engine::{
    DropScope, EngineStatus, ListenerState, ListenerStatus, RuleId, RuleLanguage, RuleSpec,
    RuleStatus, StoreStats,
};
use loctel_ui::{ListenerLine, StatusModel};

fn address(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

#[test]
fn from_status_maps_listening_and_failed_listeners() {
    let status = EngineStatus {
        paused: true,
        listeners: vec![
            ListenerStatus {
                name: "otlp-grpc",
                requested: address(0),
                // Port 0 was asked for; the view shows the port it got.
                state: ListenerState::Listening(address(41_317)),
            },
            ListenerStatus {
                name: "otlp-http",
                requested: address(4318),
                state: ListenerState::Failed("Address already in use (os error 98)".to_owned()),
            },
        ],
        spans_per_sec: 412.5,
        logs_per_sec: 3.0,
        stats: StoreStats {
            traces: 7,
            spans: 90,
            logs: 1_200,
            spans_received: 500,
            logs_received: 2_000,
            ..StoreStats::default()
        },
        rules: vec![],
        rules_file_error: None,
        dropped_spans: 0,
        dropped_logs: 0,
        dropped_fraction: 0.0,
        disk_bytes: 0,
        disk_cap_bytes: 4 * 1024 * 1024 * 1024,
        spill_error: None,
    };

    let model = StatusModel::from_status(&status);

    assert_eq!(
        model,
        StatusModel {
            paused: true,
            listeners: vec![
                ListenerLine {
                    name: "otlp-grpc".to_owned(),
                    address: "127.0.0.1:41317".to_owned(),
                    ok: true,
                    error: None,
                },
                ListenerLine {
                    // A failed listener keeps the address it was asked for.
                    name: "otlp-http".to_owned(),
                    address: "127.0.0.1:4318".to_owned(),
                    ok: false,
                    error: Some("Address already in use (os error 98)".to_owned()),
                },
            ],
            spans_per_sec: 412.5,
            logs_per_sec: 3.0,
            traces_held: 7,
            logs_held: 1_200,
            rules_on: 0,
            rules_with_errors: 0,
            rules_file_error: false,
            dropped_fraction: 0.0,
        }
    );
}

fn rule(id: u64, enabled: bool, error: Option<&str>) -> RuleStatus {
    RuleStatus {
        id: RuleId(id),
        spec: RuleSpec {
            name: format!("rule {id}"),
            enabled,
            language: RuleLanguage::TraceQl,
            query: "{ name = \"x\" }".to_owned(),
            scope: DropScope::Trace,
        },
        error: error.map(str::to_owned),
        dropped_spans: 0,
        dropped_logs: 0,
        dropped_traces: 0,
    }
}

fn status_with_rules(
    rules: Vec<RuleStatus>,
    file_error: Option<&str>,
    fraction: f64,
) -> EngineStatus {
    EngineStatus {
        paused: false,
        listeners: vec![],
        spans_per_sec: 0.0,
        logs_per_sec: 0.0,
        stats: StoreStats::default(),
        rules,
        rules_file_error: file_error.map(str::to_owned),
        dropped_spans: 0,
        dropped_logs: 0,
        dropped_fraction: fraction,
        disk_bytes: 0,
        disk_cap_bytes: 4 * 1024 * 1024 * 1024,
        spill_error: None,
    }
}

#[test]
fn from_status_counts_the_rules_that_are_on_and_their_errors() {
    let status = status_with_rules(
        vec![
            rule(1, true, None),
            rule(2, true, Some("at column 3: unexpected token")),
            rule(3, false, None),
            // A draft saved switched off still reports its error, but it is
            // not on, so it is not something the menu warns about.
            rule(4, false, Some("at column 1: unexpected token")),
        ],
        None,
        0.31,
    );

    let model = StatusModel::from_status(&status);

    assert_eq!(model.rules_on, 2, "enabled rules, valid or not");
    assert_eq!(
        model.rules_with_errors, 1,
        "enabled rules that do not parse"
    );
    assert!(!model.rules_file_error);
    assert_eq!(model.dropped_fraction, 0.31);
}

#[test]
fn from_status_reports_a_rules_file_error_as_a_flag() {
    let status = status_with_rules(
        vec![rule(1, true, None)],
        Some("rules.toml: expected a table at line 4"),
        0.0,
    );

    let model = StatusModel::from_status(&status);

    // The text stays in the engine's status: the menu line is fixed words.
    assert!(model.rules_file_error);
    assert_eq!(model.rules_on, 1);
    assert_eq!(model.rules_with_errors, 0);
}

#[test]
fn from_status_with_no_rules_says_so() {
    let model = StatusModel::from_status(&status_with_rules(vec![], None, 0.0));
    assert_eq!(model.rules_on, 0);
    assert_eq!(model.rules_with_errors, 0);
    assert!(!model.rules_file_error);
}
