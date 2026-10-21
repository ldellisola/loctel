//! Log severity.

use serde::Serialize;

/// OTLP severity, collapsed to the six levels loctel shows. Ordered from
/// `Unspecified` (lowest) to `Fatal`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Unspecified,
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    Fatal,
}

impl Severity {
    /// Maps an OTLP `SeverityNumber`: 1-4 Trace, 5-8 Debug, 9-12 Info,
    /// 13-16 Warn, 17-20 Error, 21-24 Fatal, anything else Unspecified.
    pub fn from_number(number: i32) -> Self {
        match number {
            1..=4 => Severity::Trace,
            5..=8 => Severity::Debug,
            9..=12 => Severity::Info,
            13..=16 => Severity::Warn,
            17..=20 => Severity::Error,
            21..=24 => Severity::Fatal,
            _ => Severity::Unspecified,
        }
    }

    /// The Loki `level` label value; `unknown` for `Unspecified`.
    pub fn level_label(&self) -> &'static str {
        match self {
            Severity::Unspecified => "unknown",
            Severity::Trace => "trace",
            Severity::Debug => "debug",
            Severity::Info => "info",
            Severity::Warn => "warn",
            Severity::Error => "error",
            Severity::Fatal => "fatal",
        }
    }
}
