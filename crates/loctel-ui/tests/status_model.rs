//! How the engine's status becomes what the view and the menu show.

use std::net::SocketAddr;

use loctel_engine::{EngineStatus, ListenerState, ListenerStatus, StoreStats};
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
        },
        rules: vec![],
        rules_file_error: None,
        dropped_spans: 0,
        dropped_logs: 0,
        dropped_fraction: 0.0,
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
        }
    );
}
