//! The Settings window's form: the text of the three port fields and the
//! login switch, and whether they are ports loctel can bind. Plain Rust, with no
//! GPUI in it: the messages are exact (the window shows them under the fields)
//! and pinned in `tests/model_misc.rs`.
//!
//! The disk cap is not on the form: it is not enforced until the spill-to-disk
//! store exists (M5), so Settings does not offer it.

use loctel_engine::{Ports, Settings};

/// Under a field whose text is not a port.
pub const PORT_RANGE: &str = "Enter a port from 1 to 65535";

/// Under every field that has the same port as another.
pub const DUPLICATE_PORTS: &str = "Each endpoint needs its own port";

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
