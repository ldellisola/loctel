//! The status line the menu bar header shows, the rules line under it, and the
//! rate format they use. The strings are exact: the macOS menu and the status
//! view both print them.

use loctel_ui::menu_text::{format_rate, rules_line, status_line};
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
        rules_on: 0,
        rules_with_errors: 0,
        rules_file_error: false,
        dropped_fraction: 0.0,
    }
}

/// A model that differs from the quiet one only in what the rules line reads.
fn with_rules(
    rules_on: usize,
    rules_with_errors: usize,
    rules_file_error: bool,
    dropped_fraction: f64,
) -> StatusModel {
    StatusModel {
        rules_on,
        rules_with_errors,
        rules_file_error,
        dropped_fraction,
        ..model(false, all_listening(), 0.0, 0.0)
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

#[test]
fn rules_line_cases() {
    // The file could not be used.
    assert_eq!(
        rules_line(&with_rules(0, 0, true, 0.0)),
        "Discard rules file has an error"
    );
    // A rule is on and its query does not parse.
    assert_eq!(
        rules_line(&with_rules(1, 1, false, 0.0)),
        "1 discard rule on \u{b7} 1 has an error"
    );
    // Nothing is on.
    assert_eq!(
        rules_line(&with_rules(0, 0, false, 0.0)),
        "No discard rules on"
    );
    // Rules are working: the share of what arrived that they dropped, as a
    // whole percent.
    assert_eq!(
        rules_line(&with_rules(1, 0, false, 0.31)),
        "1 discard rule on \u{b7} 31% dropped"
    );
    assert_eq!(
        rules_line(&with_rules(3, 0, false, 0.0)),
        "3 discard rules on \u{b7} 0% dropped"
    );
}

#[test]
fn rules_line_file_error_wins() {
    // Over a rule error, over "no rules on" and over a share dropped: the
    // rules in force are the last good ones, and the line must not hide that
    // the file on disk is not what is running.
    assert_eq!(
        rules_line(&with_rules(2, 1, true, 0.5)),
        "Discard rules file has an error"
    );
    assert_eq!(
        rules_line(&with_rules(2, 0, true, 0.5)),
        "Discard rules file has an error"
    );
    assert_eq!(
        rules_line(&with_rules(0, 0, true, 0.0)),
        "Discard rules file has an error"
    );
}

#[test]
fn rules_line_rule_error_wins_over_the_share_dropped() {
    assert_eq!(
        rules_line(&with_rules(3, 1, false, 0.9)),
        "3 discard rules on \u{b7} 1 has an error"
    );
    // More than one: "has an error" would be wrong.
    assert_eq!(
        rules_line(&with_rules(3, 2, false, 0.9)),
        "3 discard rules on \u{b7} 2 have errors"
    );
}

#[test]
fn rules_line_percentage_is_rounded_to_a_whole_number() {
    assert_eq!(
        rules_line(&with_rules(1, 0, false, 0.3)),
        "1 discard rule on \u{b7} 30% dropped"
    );
    // Under half a percent is still "0%", over it rounds up.
    assert_eq!(
        rules_line(&with_rules(1, 0, false, 0.004)),
        "1 discard rule on \u{b7} 0% dropped"
    );
    assert_eq!(
        rules_line(&with_rules(1, 0, false, 0.006)),
        "1 discard rule on \u{b7} 1% dropped"
    );
    assert_eq!(
        rules_line(&with_rules(1, 0, false, 0.996)),
        "1 discard rule on \u{b7} 100% dropped"
    );
}

#[test]
fn rules_line_never_shows_a_percentage_that_is_not_one() {
    // Nothing that is not a share leaks a "NaN", a minus sign or "250%": a
    // value that is not a number reads 0, as in `format_rate`, and a share
    // outside 0 to 1 is cut to it.
    for (fraction, shown) in [
        (f64::NAN, "0%"),
        (f64::INFINITY, "0%"),
        (-0.2, "0%"),
        (2.5, "100%"),
    ] {
        assert_eq!(
            rules_line(&with_rules(1, 0, false, fraction)),
            format!("1 discard rule on \u{b7} {shown} dropped"),
            "fraction {fraction}"
        );
    }
}
