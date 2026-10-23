//! The Settings form's validation and the menu bar popover's content (M4 plan,
//! Task 3). Plain values: the messages and strings here are exact, because the
//! Settings window and the popover only lay them out.

use std::net::SocketAddr;

use loctel_engine::{
    DropScope, EngineStatus, ListenerState, ListenerStatus, Ports, RuleId, RuleLanguage, RuleSpec,
    RuleStatus, Settings, StoreStats,
};
use loctel_ui::StatusModel;
use loctel_ui::menu_text::rules_line;
use loctel_ui::model::popover::{Pill, Popover, Tone, from_status};
use loctel_ui::model::settings::{
    BYTES_PER_GB, DISK_CAP_LABEL, DISK_CAP_RANGE, DUPLICATE_PORTS, EndpointState, Field, Form,
    LISTENING, PORT_RANGE, endpoint_state, failures, usage_line,
};

// --- the Settings form ------------------------------------------------------------

const RANGE: &str = "Enter a port from 1 to 65535";
const DUPLICATE: &str = "Each endpoint needs its own port";
const CAP_RANGE: &str = "Enter a size from 1 to 100 GB";
const GB: u64 = 1024 * 1024 * 1024;

fn form(grpc: &str, http: &str, mcp: &str) -> Form {
    Form {
        grpc: grpc.to_owned(),
        http: http.to_owned(),
        mcp: mcp.to_owned(),
        disk_cap_gb: "4".to_owned(),
        start_at_login: None,
    }
}

/// A form whose ports are the defaults and whose disk cap field holds `text`.
fn cap_form(text: &str) -> Form {
    Form {
        disk_cap_gb: text.to_owned(),
        ..form("4317", "4318", "4319")
    }
}

fn ports(grpc: u16, http: u16, mcp: u16) -> Ports {
    Ports {
        otlp_grpc: grpc,
        otlp_http: http,
        mcp,
    }
}

#[test]
fn the_messages_are_the_plans() {
    assert_eq!(PORT_RANGE, RANGE);
    assert_eq!(DUPLICATE_PORTS, DUPLICATE);
}

#[test]
fn the_form_starts_from_the_saved_ports() {
    let form = Form::from(&Settings::default(), None);
    assert_eq!(form.grpc, "4317");
    assert_eq!(form.http, "4318");
    assert_eq!(form.mcp, "4319");
    assert_eq!(form.validate(), Ok(ports(4317, 4318, 4319)));
}

#[test]
fn start_at_login_is_what_the_system_says_and_absent_without_a_login_item() {
    // The settings file has its own `start_at_login`; the form never reads it:
    // the system is the source of truth, and where there is no login item
    // (Linux) there is no switch at all.
    let settings = Settings {
        start_at_login: true,
        ..Settings::default()
    };
    assert_eq!(Form::from(&settings, None).start_at_login, None);
    assert_eq!(
        Form::from(&settings, Some(false)).start_at_login,
        Some(false)
    );
    assert_eq!(
        Form::from(&Settings::default(), Some(true)).start_at_login,
        Some(true)
    );
}

#[test]
fn the_form_shows_the_ports_that_were_saved() {
    let settings = Settings {
        otlp_grpc_port: 14_317,
        otlp_http_port: 14_318,
        mcp_port: 65_535,
        ..Settings::default()
    };
    let form = Form::from(&settings, None);
    assert_eq!(
        (form.grpc.as_str(), form.http.as_str(), form.mcp.as_str()),
        ("14317", "14318", "65535")
    );
    assert_eq!(form.validate(), Ok(ports(14_317, 14_318, 65_535)));
}

#[test]
fn ports_from_1_to_65535_are_accepted() {
    assert_eq!(form("1", "2", "65535").validate(), Ok(ports(1, 2, 65_535)));
}

#[test]
fn blanks_around_a_port_are_ignored() {
    assert_eq!(
        form(" 4317 ", "\t4318", "4319\n").validate(),
        Ok(ports(4317, 4318, 4319))
    );
}

#[test]
fn what_is_not_a_port_from_1_to_65535_is_refused_with_the_range_message() {
    for text in [
        "",
        "  ",
        "abc",
        "0",
        "00",
        "65536",
        "70000",
        "99999999999999999999",
        "-1",
        "+4318",
        "4 318",
        "4318.5",
        "4,318",
        "0x10fe",
        "\u{664}\u{663}",
        "4318a",
    ] {
        assert_eq!(
            form(text, "4318", "4319").validate(),
            Err(vec![(Field::Grpc, RANGE.to_owned())]),
            "{text:?}"
        );
    }
}

#[test]
fn every_field_gets_its_own_message_in_field_order() {
    assert_eq!(
        form("x", "4318", "").validate(),
        Err(vec![
            (Field::Grpc, RANGE.to_owned()),
            (Field::Mcp, RANGE.to_owned()),
        ])
    );
    assert_eq!(
        form("0", "65536", "-").validate(),
        Err(vec![
            (Field::Grpc, RANGE.to_owned()),
            (Field::Http, RANGE.to_owned()),
            (Field::Mcp, RANGE.to_owned()),
        ])
    );
}

#[test]
fn duplicate_ports_are_refused_on_each_field_that_shares_one() {
    assert_eq!(
        form("4318", "4318", "4319").validate(),
        Err(vec![
            (Field::Grpc, DUPLICATE.to_owned()),
            (Field::Http, DUPLICATE.to_owned()),
        ])
    );
    assert_eq!(
        form("4317", "4318", "4317").validate(),
        Err(vec![
            (Field::Grpc, DUPLICATE.to_owned()),
            (Field::Mcp, DUPLICATE.to_owned()),
        ])
    );
    assert_eq!(
        form("4318", "4319", "4319").validate(),
        Err(vec![
            (Field::Http, DUPLICATE.to_owned()),
            (Field::Mcp, DUPLICATE.to_owned()),
        ])
    );
    assert_eq!(
        form("5000", "5000", "5000").validate(),
        Err(vec![
            (Field::Grpc, DUPLICATE.to_owned()),
            (Field::Http, DUPLICATE.to_owned()),
            (Field::Mcp, DUPLICATE.to_owned()),
        ])
    );
}

#[test]
fn ports_are_compared_as_numbers_not_as_text() {
    assert_eq!(
        form("4318", " 04318", "4319").validate(),
        Err(vec![
            (Field::Grpc, DUPLICATE.to_owned()),
            (Field::Http, DUPLICATE.to_owned()),
        ])
    );
}

#[test]
fn a_bad_port_does_not_hide_a_duplicate_among_the_good_ones() {
    assert_eq!(
        form("4318", "4318", "nope").validate(),
        Err(vec![
            (Field::Grpc, DUPLICATE.to_owned()),
            (Field::Http, DUPLICATE.to_owned()),
            (Field::Mcp, RANGE.to_owned()),
        ])
    );
}

#[test]
fn two_bad_ports_that_read_the_same_are_not_duplicates() {
    // Neither is a port, so neither shares one.
    assert_eq!(
        form("x", "x", "4319").validate(),
        Err(vec![
            (Field::Grpc, RANGE.to_owned()),
            (Field::Http, RANGE.to_owned()),
        ])
    );
}

#[test]
fn the_login_switch_is_not_part_of_the_ports() {
    let mut f = form("4317", "4318", "4319");
    f.start_at_login = Some(true);
    assert_eq!(f.validate(), Ok(ports(4317, 4318, 4319)));
}

// --- the disk cap (M5 plan, Task 8) ------------------------------------------------

#[test]
fn disk_cap_validation() {
    // The message is the plan's, and a GB is 2^30 bytes (the unit `format::bytes`
    // and the default cap of 4 GiB use, so the default reads `4`).
    assert_eq!(DISK_CAP_RANGE, CAP_RANGE);
    assert_eq!(BYTES_PER_GB, GB);

    // A whole number of GB from 1 to 100, blanks around it ignored, compared as
    // a number (`04` is 4), is the cap in bytes.
    for (text, gb) in [
        ("1", 1),
        ("4", 4),
        ("100", 100),
        (" 8 ", 8),
        ("\t12\n", 12),
        ("04", 4),
        ("0100", 100),
    ] {
        assert_eq!(cap_form(text).validate_disk_cap(), Ok(gb * GB), "{text:?}");
    }

    // Anything else is the message: empty, zero, over the limit, not digits
    // only, a number that does not fit.
    for text in [
        "",
        "  ",
        "0",
        "00",
        "101",
        "1000",
        "-1",
        "+4",
        "1.5",
        "4,5",
        "4 GB",
        "4GB",
        "abc",
        "1e2",
        "0x10",
        "\u{664}",
        "99999999999999999999999",
    ] {
        assert_eq!(
            cap_form(text).validate_disk_cap(),
            Err(CAP_RANGE),
            "{text:?}"
        );
    }
}

#[test]
fn the_disk_cap_is_not_one_of_the_ports_and_the_form_needs_both_valid() {
    // `validate` is about the ports only (its results are pinned above), so a bad
    // cap does not hide behind a port and a bad port does not hide behind the cap.
    let bad_cap = cap_form("0");
    assert_eq!(bad_cap.validate(), Ok(ports(4317, 4318, 4319)));
    assert!(!bad_cap.is_valid());

    let bad_port = Form {
        grpc: "x".to_owned(),
        ..cap_form("4")
    };
    assert!(bad_port.validate().is_err());
    assert_eq!(bad_port.validate_disk_cap(), Ok(4 * GB));
    assert!(!bad_port.is_valid());

    assert!(cap_form("4").is_valid());
}

#[test]
fn the_form_starts_from_the_saved_cap_in_whole_gb() {
    let cap = |bytes: u64| {
        Form::from(
            &Settings {
                disk_cap_bytes: bytes,
                ..Settings::default()
            },
            None,
        )
        .disk_cap_gb
    };
    assert_eq!(Form::from(&Settings::default(), None).disk_cap_gb, "4");
    assert_eq!(cap(GB), "1");
    assert_eq!(cap(100 * GB), "100");
    // A file edited by hand can hold what the window does not offer. The field
    // shows the next whole GB up (never a cap lower than the file's), and Apply
    // leaves the file's value alone until the field is edited.
    assert_eq!(cap(GB + 1), "2");
    assert_eq!(cap(GB + GB / 2), "2");
    assert_eq!(cap(512 * 1024 * 1024), "1");
    // Out of range, and so refused until it is changed.
    assert_eq!(cap(0), "0");
    assert_eq!(cap(200 * GB), "200");
    assert_eq!(cap(u64::MAX), (u64::MAX.div_ceil(GB)).to_string());
}

#[test]
fn the_storage_line_says_what_is_in_use_of_the_cap() {
    assert_eq!(DISK_CAP_LABEL, "Disk cap");
    assert_eq!(usage_line(0, 4 * GB), "Using 0 B of 4 GB");
    assert_eq!(usage_line(GB * 13 / 10, 4 * GB), "Using 1.3 GB of 4 GB");
    assert_eq!(usage_line(512 * 1024, 4 * GB), "Using 512 KB of 4 GB");
    assert_eq!(usage_line(12 * GB, 100 * GB), "Using 12 GB of 100 GB");
    // A cap the window cannot write (a file edited by hand) is said as it is.
    assert_eq!(usage_line(GB, GB + GB / 2), "Using 1 GB of 1.5 GB");
}

// --- the Settings window's endpoint lines (M4 plan, Task 6) -----------------------

#[test]
fn the_fields_have_the_plans_labels_and_name_their_listeners() {
    assert_eq!(Field::ALL, [Field::Grpc, Field::Http, Field::Mcp]);
    assert_eq!(
        Field::ALL.map(Field::label),
        ["OTLP gRPC port", "OTLP HTTP port", "MCP port"]
    );
    // The names the engine gives its listeners, in the engine's order.
    assert_eq!(
        Field::ALL.map(Field::listener),
        ["otlp-grpc", "otlp-http", "mcp"]
    );
}

#[test]
fn an_endpoint_says_listening_or_why_it_failed() {
    assert_eq!(LISTENING, "listening");
    let listeners = vec![
        listening("otlp-grpc", 4317),
        failed("otlp-http", 4400),
        listening("mcp", 4319),
    ];
    assert_eq!(
        endpoint_state(&listeners, Field::Grpc),
        Some(EndpointState {
            ok: true,
            text: "listening".to_owned(),
        })
    );
    // The OS's own wording around "in use" differs between platforms; the
    // window says it the way the menu does.
    assert_eq!(
        endpoint_state(&listeners, Field::Http),
        Some(EndpointState {
            ok: false,
            text: "failed: address in use".to_owned(),
        })
    );
    assert_eq!(
        endpoint_state(&listeners, Field::Mcp).map(|state| state.ok),
        Some(true)
    );
}

#[test]
fn an_unrecognised_failure_is_shown_as_the_engine_gave_it_cut_to_forty_characters() {
    let listeners = vec![ListenerStatus {
        name: "mcp",
        requested: address(80),
        state: ListenerState::Failed(
            "Permission denied (os error 13) while binding to 127.0.0.1:80".to_owned(),
        ),
    }];
    assert_eq!(
        endpoint_state(&listeners, Field::Mcp),
        Some(EndpointState {
            ok: false,
            text: "failed: Permission denied (os error 13) while bi".to_owned(),
        })
    );
    let short = vec![ListenerStatus {
        name: "mcp",
        requested: address(80),
        state: ListenerState::Failed("Permission denied (os error 13)".to_owned()),
    }];
    assert_eq!(
        endpoint_state(&short, Field::Mcp).map(|state| state.text),
        Some("failed: Permission denied (os error 13)".to_owned())
    );
}

#[test]
fn a_listener_the_engine_does_not_report_has_no_state() {
    assert_eq!(endpoint_state(&[], Field::Http), None);
    assert_eq!(endpoint_state(&[listening("mcp", 4319)], Field::Grpc), None);
}

#[test]
fn the_failures_of_a_set_of_listeners_name_each_one_and_its_reason() {
    assert_eq!(failures(&healthy()), Vec::<String>::new());
    assert_eq!(
        failures(&[
            failed("otlp-grpc", 4317),
            listening("otlp-http", 4318),
            failed("mcp", 4319),
        ]),
        vec![
            "otlp-grpc: address in use".to_owned(),
            "mcp: address in use".to_owned(),
        ]
    );
}

// --- the popover ------------------------------------------------------------------

fn address(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

fn listening(name: &'static str, port: u16) -> ListenerStatus {
    ListenerStatus {
        name,
        requested: address(port),
        state: ListenerState::Listening(address(port)),
    }
}

fn failed(name: &'static str, port: u16) -> ListenerStatus {
    ListenerStatus {
        name,
        requested: address(port),
        state: ListenerState::Failed("Address already in use (os error 98)".to_owned()),
    }
}

fn healthy() -> Vec<ListenerStatus> {
    vec![
        listening("otlp-grpc", 4317),
        listening("otlp-http", 4318),
        listening("mcp", 4319),
    ]
}

fn engine(paused: bool, listeners: Vec<ListenerStatus>) -> EngineStatus {
    EngineStatus {
        paused,
        listeners,
        spans_per_sec: 412.5,
        logs_per_sec: 1_234.0,
        stats: StoreStats {
            traces: 7,
            spans: 90,
            logs: 1_200_000,
            spans_received: 500,
            logs_received: 2_000,
            ..StoreStats::default()
        },
        rules: vec![],
        rules_file_error: None,
        dropped_spans: 0,
        dropped_logs: 0,
        dropped_fraction: 0.0,
        disk_bytes: 1_395_864_371,
        disk_cap_bytes: 4 * 1024 * 1024 * 1024,
        spill_error: None,
    }
}

#[test]
fn receiving_says_receiving_in_the_ok_tone() {
    let popover = from_status(&engine(false, healthy()), None);
    assert_eq!(
        popover.pill,
        Pill {
            text: "Receiving",
            tone: Tone::Ok
        }
    );
    assert!(!popover.paused);
}

#[test]
fn paused_says_paused_in_the_warn_tone() {
    let popover = from_status(&engine(true, healthy()), None);
    assert_eq!(
        popover.pill,
        Pill {
            text: "Paused",
            tone: Tone::Warn
        }
    );
    assert!(popover.paused);
}

#[test]
fn a_failed_listener_says_port_problem_in_the_error_tone() {
    let listeners = vec![
        listening("otlp-grpc", 4317),
        failed("otlp-http", 4318),
        listening("mcp", 4319),
    ];
    let popover = from_status(&engine(false, listeners), None);
    assert_eq!(
        popover.pill,
        Pill {
            text: "Port problem",
            tone: Tone::Error
        }
    );
}

#[test]
fn the_pill_prefers_a_failed_listener_over_paused_over_receiving() {
    let failing = vec![failed("mcp", 4319)];
    // Failed and paused: the failure wins, as in the status line.
    let both = from_status(&engine(true, failing.clone()), None);
    assert_eq!(both.pill.text, "Port problem");
    assert_eq!(both.pill.tone, Tone::Error);
    // The pause is still said where it is shown on its own: the action row.
    assert!(both.paused);

    let paused = from_status(&engine(true, healthy()), None);
    assert_eq!(paused.pill.text, "Paused");

    let receiving = from_status(&engine(false, healthy()), None);
    assert_eq!(receiving.pill.text, "Receiving");
}

#[test]
fn the_four_stats_are_in_the_plans_order_with_its_labels() {
    // Since Milestone 5 the fourth tile is the disk (the mockup's), not the
    // log lines held.
    let popover = from_status(&engine(false, healthy()), None);
    assert_eq!(
        popover.stats,
        [
            ("413".to_owned(), "spans per second".to_owned()),
            ("1.2k".to_owned(), "log lines per second".to_owned()),
            ("7".to_owned(), "traces held".to_owned()),
            ("1.3 GB".to_owned(), "on disk, cap 4 GB".to_owned()),
        ]
    );
    assert!(!popover.disk_full);
}

#[test]
fn the_disk_tile_is_the_bytes_on_disk_and_the_cap() {
    let tile = |disk_bytes: u64, disk_cap_bytes: u64| {
        let mut status = engine(false, healthy());
        status.disk_bytes = disk_bytes;
        status.disk_cap_bytes = disk_cap_bytes;
        from_status(&status, None).stats[3].clone()
    };
    let (kb, mb, gb) = (1024, 1024 * 1024, GB);
    // Nothing on disk (a spill-less run reads the same), the default cap.
    assert_eq!(
        tile(0, 4 * gb),
        ("0 B".to_owned(), "on disk, cap 4 GB".to_owned())
    );
    assert_eq!(tile(512 * kb, 4 * gb).0, "512 KB");
    assert_eq!(tile(250 * mb, 4 * gb).0, "250 MB");
    assert_eq!(tile(gb * 13 / 10, 4 * gb).0, "1.3 GB");
    assert_eq!(tile(12 * gb, 100 * gb).1, "on disk, cap 100 GB");
    // The cap is the one in force, as the engine says it, whole GB or not.
    assert_eq!(tile(gb, gb * 3 / 2).1, "on disk, cap 1.5 GB");
}

#[test]
fn a_spill_error_says_disk_full_in_the_tile_and_the_pill_stays_about_ports() {
    let mut status = engine(false, healthy());
    status.spill_error = Some("No space left on device (os error 28)".to_owned());
    let popover = from_status(&status, None);
    // The tile says it, and keeps the label (the cap is still what it was).
    assert_eq!(
        popover.stats[3],
        ("Disk full".to_owned(), "on disk, cap 4 GB".to_owned())
    );
    assert!(popover.disk_full);
    // The other tiles are untouched, and the pill is still about the ports.
    assert_eq!(popover.stats[2].0, "7");
    assert_eq!(
        popover.pill,
        Pill {
            text: "Receiving",
            tone: Tone::Ok
        }
    );

    // A port problem is still the pill's, with or without a full disk.
    let failing = vec![failed("otlp-http", 4318)];
    let both = from_status(
        &EngineStatus {
            listeners: failing.clone(),
            ..status.clone()
        },
        None,
    );
    assert_eq!(both.pill.text, "Port problem");
    assert!(both.disk_full);
    let ports_only = from_status(&engine(false, failing), None);
    assert_eq!(ports_only.pill.text, "Port problem");
    assert!(!ports_only.disk_full);

    // Paused with a full disk is paused.
    let paused = from_status(
        &EngineStatus {
            paused: true,
            ..status
        },
        None,
    );
    assert_eq!(paused.pill.text, "Paused");
    assert!(paused.disk_full);
}

#[test]
fn rates_use_the_menus_format_and_counts_a_thousands_separator() {
    let mut status = engine(false, healthy());
    status.spans_per_sec = 0.4;
    status.logs_per_sec = 1_300_000.0;
    status.stats.traces = 1_234;
    status.disk_bytes = 0;

    let stats = from_status(&status, None).stats;

    assert_eq!(stats[0].0, "0");
    assert_eq!(stats[1].0, "1.3M");
    assert_eq!(stats[2].0, "1,234");
    assert_eq!(stats[3].0, "0 B");
}

#[test]
fn the_endpoints_are_the_three_listeners_with_their_addresses() {
    let popover = from_status(&engine(false, healthy()), None);
    assert_eq!(
        popover.endpoints,
        vec![
            ("OTLP gRPC", "127.0.0.1:4317".to_owned(), true),
            ("OTLP HTTP", "127.0.0.1:4318".to_owned(), true),
            // The MCP row is the address an agent connects to: what Copy copies.
            ("MCP", "http://127.0.0.1:4319/mcp".to_owned(), true),
        ]
    );
}

#[test]
fn an_endpoint_shows_the_port_it_really_bound_and_a_failed_one_the_port_asked_for() {
    let listeners = vec![
        ListenerStatus {
            name: "otlp-grpc",
            requested: address(0),
            state: ListenerState::Listening(address(41_317)),
        },
        failed("otlp-http", 4318),
        listening("mcp", 4319),
    ];

    let popover = from_status(&engine(false, listeners), None);

    assert_eq!(
        popover.endpoints,
        vec![
            ("OTLP gRPC", "127.0.0.1:41317".to_owned(), true),
            ("OTLP HTTP", "127.0.0.1:4318".to_owned(), false),
            ("MCP", "http://127.0.0.1:4319/mcp".to_owned(), true),
        ]
    );
}

#[test]
fn a_failed_mcp_listener_has_no_url_to_show() {
    let listeners = vec![
        listening("otlp-grpc", 4317),
        listening("otlp-http", 4318),
        failed("mcp", 4319),
    ];
    let popover = from_status(&engine(false, listeners), None);
    assert_eq!(
        popover.endpoints[2],
        ("MCP", "127.0.0.1:4319".to_owned(), false)
    );
}

#[test]
fn a_status_without_listeners_has_no_endpoint_rows() {
    assert!(
        from_status(&engine(false, vec![]), None)
            .endpoints
            .is_empty()
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

#[test]
fn the_rules_line_is_the_menus() {
    let mut status = engine(false, healthy());
    assert_eq!(from_status(&status, None).rules_line, "No discard rules on");

    status.rules = vec![rule(1, true, None), rule(2, false, None)];
    status.dropped_fraction = 0.31;
    assert_eq!(
        from_status(&status, None).rules_line,
        "1 discard rule on \u{b7} 31% dropped"
    );

    status.rules = vec![rule(1, true, None), rule(2, true, Some("at column 1: x"))];
    assert_eq!(
        from_status(&status, None).rules_line,
        "2 discard rules on \u{b7} 1 has an error"
    );

    status.rules_file_error = Some("bad toml".to_owned());
    let popover = from_status(&status, None);
    assert_eq!(popover.rules_line, "Discard rules file has an error");
    // Whatever it is, it is the line the macOS menu and the status view say.
    assert_eq!(
        popover.rules_line,
        rules_line(&StatusModel::from_status(&status))
    );
}

#[test]
fn start_at_login_passes_through() {
    let status = engine(false, healthy());
    assert_eq!(from_status(&status, None).start_at_login, None);
    assert_eq!(from_status(&status, Some(true)).start_at_login, Some(true));
    assert_eq!(
        from_status(&status, Some(false)).start_at_login,
        Some(false)
    );
}

#[test]
fn a_popover_that_changes_nothing_compares_equal() {
    // The view redraws only when the model changed.
    let status = engine(false, healthy());
    let a: Popover = from_status(&status, Some(true));
    assert_eq!(a, from_status(&status, Some(true)));
    assert_ne!(a, from_status(&status, Some(false)));
    let mut busier = status.clone();
    busier.stats.traces += 1;
    assert_ne!(a, from_status(&busier, Some(true)));
}
