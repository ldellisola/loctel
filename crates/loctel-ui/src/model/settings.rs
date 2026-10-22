//! The Settings window's form: the text of the three port fields and the
//! login switch, and whether they are ports loctel can bind. Plain Rust, with no
//! GPUI in it: the messages are exact (the window shows them under the fields)
//! and pinned in `tests/model_misc.rs`.
//!
//! The disk cap is not on the form: it is not enforced until the spill-to-disk
//! store exists (M5), so Settings does not offer it.
//!
//! The window also says how each endpoint is doing ([`endpoint_state`]) and,
//! after an Apply, which ones could not bind ([`failures`]); the words are here,
//! with the form's, so a plain test pins them.

use loctel_engine::{ListenerState, ListenerStatus, Ports, Settings};

use crate::menu_text::reason;

/// Under a field whose text is not a port.
pub const PORT_RANGE: &str = "Enter a port from 1 to 65535";

/// Under every field that has the same port as another.
pub const DUPLICATE_PORTS: &str = "Each endpoint needs its own port";

/// What an endpoint's line says while its listener is up.
pub const LISTENING: &str = "listening";

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

/// What the form's fields hold right now, as typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Form {
    pub grpc: String,
    pub http: String,
    pub mcp: String,
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
    /// The login switch is not part of it.
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

/// A port from 1 to 65535 written in ASCII digits (blanks around it ignored).
fn parse_port(text: &str) -> Option<u16> {
    let text = text.trim();
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse::<u16>().ok().filter(|&port| port >= 1)
}
