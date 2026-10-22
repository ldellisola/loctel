//! Domain types for loctel: spans, logs, resources.
//!
//! This crate is independent of OTLP and of any UI. The ingest crate converts
//! protocol messages into these types, and everything downstream (store, MCP,
//! engine, UI) speaks them. All of them implement `serde::Serialize` as plain
//! JSON: IDs and byte values as lowercase hex strings, attributes as JSON
//! objects, enums as `snake_case` strings. All of them also implement
//! `serde::Deserialize` and read back what they wrote: exactly in a binary
//! format such as postcard (which the store's segment files use, where IDs are
//! raw bytes and attribute values are tagged), and in JSON except where JSON
//! cannot tell two things apart (a byte value reads back as the hex string,
//! see [`AttrValue`]).

mod attr;
mod exception;
mod ids;
mod labels;
mod log;
mod resource;
mod severity;
mod span;

pub use attr::{AttrValue, Attributes, attr};
pub use exception::ExceptionInfo;
pub use ids::{IdError, SpanId, TraceId};
pub use labels::loki_label_name;
pub use log::LogRecord;
pub use resource::{Resource, UNKNOWN_SERVICE};
pub use severity::Severity;
pub use span::{Span, SpanEvent, SpanKind, StatusCode};

#[cfg(test)]
mod tests {
    #[test]
    fn workspace_builds() {}
}
