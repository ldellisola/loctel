//! What the menu bar popover shows: the status pill, the four stats, the
//! endpoints, the discard rules line and the state of the pause and login
//! rows. Plain Rust over the engine's status, with no GPUI in it: the view
//! lays out what it is given, and `tests/model_misc.rs` pins every string.
//!
//! The words come from [`menu_text`] where it already has them (the rates, the
//! rules line), so the popover, the status view and the Mac menu bar never
//! disagree about the same state.
//!
//! The fourth stat is the disk (Milestone 5): what the segment files take, over
//! `on disk, cap 4 GB`, or `Disk full` while the engine cannot write to disk.
//! That is the tile's say alone: the pill is only about the ports.

use loctel_engine::EngineStatus;

use crate::StatusModel;
use crate::menu_text::{format_rate, rules_line};

use super::format;
use super::trace_table::thousands;

/// How a pill is coloured: green, amber or red.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Ok,
    Warn,
    Error,
}

/// The status pill at the top of the popover.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pill {
    /// `Receiving`, `Paused` or `Port problem`.
    pub text: &'static str,
    pub tone: Tone,
}

/// The labels of the first three stats, in the order they are drawn (2 by 2).
/// The fourth, the disk, has a label of its own ([`disk_label`]) because it
/// says the cap.
pub const STAT_LABELS: [&str; 3] = ["spans per second", "log lines per second", "traces held"];

/// What the disk tile says instead of the size while the engine cannot write to
/// disk (`EngineStatus::spill_error` is set).
pub const DISK_FULL: &str = "Disk full";

/// The label of the disk tile: `on disk, cap 4 GB`, with the cap in force.
pub fn disk_label(cap_bytes: u64) -> String {
    format!("on disk, cap {}", format::bytes(cap_bytes))
}

/// Everything the popover draws.
#[derive(Clone, Debug, PartialEq)]
pub struct Popover {
    pub pill: Pill,
    /// `(value, label)` for spans per second, log lines per second, traces
    /// held and the disk. The rates are [`format_rate`]'s (`412`, `1.2k`), the
    /// count has a thousands separator (`1,204`), and the disk is the size the
    /// segment files take over `on disk, cap 4 GB` (`1.3 GB`; [`format::bytes`]
    /// and [`disk_label`]), or [`DISK_FULL`] over the same label while
    /// [`disk_full`](Self::disk_full).
    pub stats: [(String, String); 4],
    /// The engine cannot write to disk (`EngineStatus::spill_error` is set): the
    /// view draws the disk tile's value in the error colour.
    pub disk_full: bool,
    /// The three endpoints in the engine's order, as `(name, address, ok)`:
    /// `OTLP gRPC` and `OTLP HTTP` with their `host:port`, and `MCP` with the URL
    /// an agent connects to (`http://127.0.0.1:4319/mcp`, what its `Copy` button
    /// copies). A listener that failed shows the address it was asked for, with
    /// `ok` false.
    pub endpoints: Vec<(&'static str, String, bool)>,
    /// The discard rules line ([`rules_line`]); clicking it opens the rules.
    pub rules_line: String,
    /// Receiving is paused: the action row reads `Resume receiving`.
    pub paused: bool,
    /// The `Start at login` switch, or `None` where there is no login item.
    pub start_at_login: Option<bool>,
}

/// The popover for the engine's `status`. `login` is what the system says about
/// the login item, or `None` where there is none.
///
/// The pill is the first of these that applies, as in the status line: a
/// failed listener (`Port problem`, red: nothing is received on that port),
/// then paused (`Paused`, amber), then `Receiving` (green). A disk that cannot
/// be written to is not a port problem: the disk tile says it.
pub fn from_status(status: &EngineStatus, login: Option<bool>) -> Popover {
    let model = StatusModel::from_status(status);
    let pill = if model.listeners.iter().any(|listener| !listener.ok) {
        Pill {
            text: "Port problem",
            tone: Tone::Error,
        }
    } else if model.paused {
        Pill {
            text: "Paused",
            tone: Tone::Warn,
        }
    } else {
        Pill {
            text: "Receiving",
            tone: Tone::Ok,
        }
    };
    let endpoints = model
        .listeners
        .iter()
        .filter_map(|listener| {
            let (name, address) = match listener.name.as_str() {
                "otlp-grpc" => ("OTLP gRPC", listener.address.clone()),
                "otlp-http" => ("OTLP HTTP", listener.address.clone()),
                "mcp" if listener.ok => ("MCP", format!("http://{}/mcp", listener.address)),
                "mcp" => ("MCP", listener.address.clone()),
                _ => return None,
            };
            Some((name, address, listener.ok))
        })
        .collect();
    let disk_full = status.spill_error.is_some();
    let disk_value = if disk_full {
        DISK_FULL.to_owned()
    } else {
        format::bytes(status.disk_bytes)
    };
    Popover {
        pill,
        stats: [
            (format_rate(model.spans_per_sec), STAT_LABELS[0].to_owned()),
            (format_rate(model.logs_per_sec), STAT_LABELS[1].to_owned()),
            (thousands(model.traces_held), STAT_LABELS[2].to_owned()),
            (disk_value, disk_label(status.disk_cap_bytes)),
        ],
        disk_full,
        endpoints,
        rules_line: rules_line(&model),
        paused: model.paused,
        start_at_login: login,
    }
}
