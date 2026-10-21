//! The status line the menu bar header shows, and the rate format it uses.
//! The strings are exact: the macOS menu and the status view both print them.

use loctel_ui::menu_text::{format_rate, status_line};
use loctel_ui::{ListenerLine, StatusModel};

fn listening(name: &str, address: &str) -> ListenerLine {
    ListenerLine {
        name: name.to_owned(),
        address: address.to_owned(),
        ok: true,
        error: None,
    }
}

fn failed(name: &str, address: &str, error: &str) -> ListenerLine {
    ListenerLine {
        name: name.to_owned(),
        address: address.to_owned(),
        ok: false,
        error: Some(error.to_owned()),
    }
}

fn model(paused: bool, listeners: Vec<ListenerLine>, spans: f64, logs: f64) -> StatusModel {
    StatusModel {
        paused,
        listeners,
        spans_per_sec: spans,
        logs_per_sec: logs,
        traces_held: 0,
        logs_held: 0,
    }
}

fn all_listening() -> Vec<ListenerLine> {
    vec![
        listening("otlp-grpc", "127.0.0.1:4317"),
        listening("otlp-http", "127.0.0.1:4318"),
        listening("mcp", "127.0.0.1:4319"),
    ]
}

#[test]
fn status_line_receiving() {
    let status = model(false, all_listening(), 412.0, 1234.0);
    assert_eq!(
        status_line(&status),
        "Receiving \u{b7} 412 spans/s \u{b7} 1.2k logs/s"
    );
}

#[test]
fn status_line_paused() {
    let status = model(true, all_listening(), 412.0, 1234.0);
    assert_eq!(status_line(&status), "Paused");
}

#[test]
fn status_line_port_problem() {
    let listeners = vec![
        listening("otlp-grpc", "127.0.0.1:4317"),
        failed(
            "otlp-http",
            "127.0.0.1:4318",
            "Address already in use (os error 98)",
        ),
        listening("mcp", "127.0.0.1:4319"),
    ];
    let status = model(false, listeners, 0.0, 0.0);
    assert_eq!(
        status_line(&status),
        "Port problem: otlp-http (address in use)"
    );
}

#[test]
fn a_failed_listener_wins_over_paused_and_paused_wins_over_receiving() {
    let broken = vec![failed(
        "mcp",
        "127.0.0.1:4319",
        "Address already in use (os error 48)",
    )];
    assert_eq!(
        status_line(&model(true, broken, 5.0, 5.0)),
        "Port problem: mcp (address in use)"
    );
    assert_eq!(
        status_line(&model(true, all_listening(), 5.0, 5.0)),
        "Paused"
    );
}

#[test]
fn port_problem_with_another_error_shows_the_raw_error_cut_to_40_characters() {
    // 50 characters; the first 40 stay.
    let raw = "permission denied while binding the port!!!!!!!!!";
    assert!(raw.chars().count() > 40);
    let status = model(
        false,
        vec![failed("otlp-grpc", "127.0.0.1:80", raw)],
        0.0,
        0.0,
    );
    let expected_reason: String = raw.chars().take(40).collect();
    assert_eq!(
        status_line(&status),
        format!("Port problem: otlp-grpc ({expected_reason})")
    );
}

#[test]
fn a_short_other_error_is_shown_whole_and_the_cut_never_splits_a_character() {
    let short = model(
        false,
        vec![failed("otlp-grpc", "127.0.0.1:80", "permission denied")],
        0.0,
        0.0,
    );
    assert_eq!(
        status_line(&short),
        "Port problem: otlp-grpc (permission denied)"
    );

    // 45 two-byte characters: a byte cut at 40 would land inside one.
    let wide = "\u{e9}".repeat(45);
    let status = model(
        false,
        vec![failed("otlp-grpc", "127.0.0.1:80", &wide)],
        0.0,
        0.0,
    );
    assert_eq!(
        status_line(&status),
        format!("Port problem: otlp-grpc ({})", "\u{e9}".repeat(40))
    );
}

#[test]
fn the_first_failed_listener_is_the_one_named() {
    let listeners = vec![
        failed("otlp-grpc", "127.0.0.1:4317", "Address already in use"),
        failed("otlp-http", "127.0.0.1:4318", "Address already in use"),
    ];
    assert_eq!(
        status_line(&model(false, listeners, 0.0, 0.0)),
        "Port problem: otlp-grpc (address in use)"
    );
}

#[test]
fn format_rate_cases() {
    assert_eq!(format_rate(412.0), "412");
    assert_eq!(format_rate(1234.0), "1.2k");
    assert_eq!(format_rate(0.4), "0");

    // Below the first thousand: whole numbers, rounded.
    assert_eq!(format_rate(0.0), "0");
    assert_eq!(format_rate(0.6), "1");
    assert_eq!(format_rate(999.0), "999");
    // Thousands: one decimal under ten thousand, none from there, no ".0".
    assert_eq!(format_rate(1000.0), "1k");
    assert_eq!(format_rate(9949.0), "9.9k");
    assert_eq!(format_rate(9960.0), "10k");
    assert_eq!(format_rate(52_340.0), "52k");
    assert_eq!(format_rate(999_400.0), "999k");
    // Millions.
    assert_eq!(format_rate(999_600.0), "1M");
    assert_eq!(format_rate(1_250_000.0), "1.3M");
    // Nothing that is not a rate leaks a "NaN" or a minus sign into a menu.
    assert_eq!(format_rate(f64::NAN), "0");
    assert_eq!(format_rate(f64::INFINITY), "0");
    assert_eq!(format_rate(-3.0), "0");
}
