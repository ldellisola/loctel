//! The Settings window's form: the text of the three port fields, the disk cap
//! field and the login switch, and whether they are values loctel can use.
//! Plain Rust, with no GPUI in it: the messages are exact (the window shows them
//! under the fields) and pinned in `tests/model_misc.rs`.
//!
//! The disk cap is in whole GB, from 1 to 100 ([`DISK_CAP_RANGE`]), and a GB is
//! 2^30 bytes ([`BYTES_PER_GB`]): the unit of the default cap (4 GiB reads `4`)
//! and of [`format::bytes`], so the field, the storage line under it and the
//! popover's disk tile all say the same numbers. It is not one of the
//! [`Field`]s, which are the three ports and the listeners they move: its
//! checks are [`Form::validate_disk_cap`] and [`Form::is_valid`].
//!
//! The window also says how each endpoint is doing ([`endpoint_state`]) and,
//! after an Apply, which ones could not bind ([`failures`]), and how much of the
//! cap is in use ([`usage_line`]); the words are here, with the form's, so a
//! plain test pins them.

use loctel_engine::{ListenerState, ListenerStatus, Ports, Settings};

use crate::menu_text::reason;

use super::format;

/// Under a field whose text is not a port.
pub const PORT_RANGE: &str = "Enter a port from 1 to 65535";

/// Under every field that has the same port as another.
pub const DUPLICATE_PORTS: &str = "Each endpoint needs its own port";

/// Under the disk cap field when its text is not a size from 1 to 100 GB.
pub const DISK_CAP_RANGE: &str = "Enter a size from 1 to 100 GB";

/// What an endpoint's line says while its listener is up.
pub const LISTENING: &str = "listening";

/// The caption of the disk cap's input.
pub const DISK_CAP_LABEL: &str = "Disk cap";

/// A GB in the disk cap field, in bytes: 2^30, the unit of the default cap (4
/// GiB) and of [`format::bytes`].
pub const BYTES_PER_GB: u64 = 1024 * 1024 * 1024;

/// The least and the most the field takes, in GB.
pub const DISK_CAP_MIN_GB: u64 = 1;
pub const DISK_CAP_MAX_GB: u64 = 100;

/// A field of the form.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Field {
    /// The OTLP gRPC port.
    Grpc,
    /// The OTLP HTTP port.
    Http,
    /// The MCP port.
    Mcp,
}

impl Field {
    /// Every field, in the order of the form and of the engine's listeners.
    pub const ALL: [Field; 3] = [Field::Grpc, Field::Http, Field::Mcp];

    /// The caption of the field's input.
    pub fn label(self) -> &'static str {
        match self {
            Field::Grpc => "OTLP gRPC port",
            Field::Http => "OTLP HTTP port",
            Field::Mcp => "MCP port",
        }
    }

    /// The name the engine gives the listener this field moves.
    pub fn listener(self) -> &'static str {
        match self {
            Field::Grpc => "otlp-grpc",
            Field::Http => "otlp-http",
            Field::Mcp => "mcp",
        }
    }
}

/// How the listener of a field is doing: the text beside its input, and
/// whether it is good news (the view paints it `ok` or in the error colour).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndpointState {
    pub ok: bool,
    /// `listening`, or `failed: <reason>`.
    pub text: String,
}

/// The state of `field`'s listener among `listeners`, or `None` when the engine
/// does not report one. `failed: address in use` when the operating system says
/// "in use" (whatever its wording), otherwise `failed: ` and the error as the
/// engine gave it, cut to 40 characters.
pub fn endpoint_state(listeners: &[ListenerStatus], field: Field) -> Option<EndpointState> {
    let listener = listeners
        .iter()
        .find(|listener| listener.name == field.listener())?;
    Some(match &listener.state {
        ListenerState::Listening(_) => EndpointState {
            ok: true,
            text: LISTENING.to_owned(),
        },
        ListenerState::Failed(error) => EndpointState {
            ok: false,
            text: format!("failed: {}", reason(Some(error))),
        },
    })
}

/// One line per listener that failed, in the engine's order: `otlp-http:
/// address in use`. It is what the window says after an Apply that could not
/// bind a port; empty when every listener is up.
pub fn failures(listeners: &[ListenerStatus]) -> Vec<String> {
    listeners
        .iter()
        .filter_map(|listener| match &listener.state {
            ListenerState::Listening(_) => None,
            ListenerState::Failed(error) => {
                Some(format!("{}: {}", listener.name, reason(Some(error))))
            }
        })
        .collect()
}

/// How much of the disk cap is in use, as the line under the cap's input says
/// it: `Using 1.3 GB of 4 GB`. `used` is the bytes the segment files take and
/// `cap` the cap in force (what the engine says, not what is being typed).
pub fn usage_line(used: u64, cap: u64) -> String {
    format!("Using {} of {}", format::bytes(used), format::bytes(cap))
}

/// What the form's fields hold right now, as typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Form {
    pub grpc: String,
    pub http: String,
    pub mcp: String,
    /// The disk cap in GB, as typed: a whole number from 1 to 100.
    pub disk_cap_gb: String,
    /// The `Start loctel at login` switch, or `None` where there is no login
    /// item (no switch is shown).
    pub start_at_login: Option<bool>,
}

impl Form {
    /// The form for the saved `settings`. `login` is what the system says about
    /// the login item now (`LoginItem::is_enabled`), or `None` where there is
    /// none: the system is the source of truth, so the settings file's own
    /// `start_at_login` is not read here.
    pub fn from(settings: &Settings, login: Option<bool>) -> Form {
        Form {
            grpc: settings.otlp_grpc_port.to_string(),
            http: settings.otlp_http_port.to_string(),
            mcp: settings.mcp_port.to_string(),
            // The next whole GB up, so the field never shows less than the file
            // holds. A cap the window cannot write (a file edited by hand, say
            // 1.5 GB) is left alone by Apply until the field is edited.
            disk_cap_gb: settings.disk_cap_bytes.div_ceil(BYTES_PER_GB).to_string(),
            start_at_login: login,
        }
    }

    /// The ports the fields name, or what is wrong, field by field in the
    /// order gRPC, HTTP, MCP.
    ///
    /// - A field must be digits only, 1 to 65535, blanks around it ignored:
    ///   anything else is [`PORT_RANGE`].
    /// - Of the fields that are ports, any two with the same number (compared
    ///   as numbers, so `4318` and `04318` are the same) are both
    ///   [`DUPLICATE_PORTS`]. A field that is not a port shares nothing.
    ///
    /// The login switch and the disk cap are not part of it (see
    /// [`validate_disk_cap`](Self::validate_disk_cap)).
    pub fn validate(&self) -> Result<Ports, Vec<(Field, String)>> {
        let fields = [
            (Field::Grpc, parse_port(&self.grpc)),
            (Field::Http, parse_port(&self.http)),
            (Field::Mcp, parse_port(&self.mcp)),
        ];
        let mut problems = Vec::new();
        for (index, (field, port)) in fields.iter().enumerate() {
            match port {
                None => problems.push((*field, PORT_RANGE.to_owned())),
                Some(port) => {
                    let shared = fields
                        .iter()
                        .enumerate()
                        .any(|(other, (_, theirs))| other != index && *theirs == Some(*port));
                    if shared {
                        problems.push((*field, DUPLICATE_PORTS.to_owned()));
                    }
                }
            }
        }
        match (fields[0].1, fields[1].1, fields[2].1) {
            (Some(otlp_grpc), Some(otlp_http), Some(mcp)) if problems.is_empty() => Ok(Ports {
                otlp_grpc,
                otlp_http,
                mcp,
            }),
            _ => Err(problems),
        }
    }
}

impl Form {
    /// The disk cap field's size in bytes, or [`DISK_CAP_RANGE`].
    ///
    /// The field must be digits only, 1 to 100, blanks around it ignored and
    /// compared as a number (`04` is 4): anything else, a decimal (`1.5`), a
    /// unit (`4 GB`) or a number that does not fit included, is the message.
    pub fn validate_disk_cap(&self) -> Result<u64, &'static str> {
        parse_gb(&self.disk_cap_gb)
            .map(|gb| gb * BYTES_PER_GB)
            .ok_or(DISK_CAP_RANGE)
    }

    /// Whether every field holds a value loctel can use: the three ports (see
    /// [`validate`](Self::validate)) and the disk cap. `Apply` is for a form
    /// that is valid.
    pub fn is_valid(&self) -> bool {
        self.validate().is_ok() && self.validate_disk_cap().is_ok()
    }
}

/// A size from 1 to 100 written in ASCII digits (blanks around it ignored).
fn parse_gb(text: &str) -> Option<u64> {
    let text = text.trim();
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse::<u64>()
        .ok()
        .filter(|gb| (DISK_CAP_MIN_GB..=DISK_CAP_MAX_GB).contains(gb))
}

/// A port from 1 to 65535 written in ASCII digits (blanks around it ignored).
fn parse_port(text: &str) -> Option<u16> {
    let text = text.trim();
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse::<u16>().ok().filter(|&port| port >= 1)
}
