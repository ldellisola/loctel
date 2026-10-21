//! Log records.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use serde::Serialize;

use crate::attr::{Attributes, serialize_attributes};
use crate::exception::ExceptionInfo;
use crate::ids::{SpanId, TraceId};
use crate::resource::Resource;
use crate::severity::Severity;

/// One log line with its context.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LogRecord {
    pub time_unix_nano: u64,
    pub observed_unix_nano: u64,
    pub severity: Severity,
    pub severity_text: String,
    pub body: String,
    #[serde(serialize_with = "serialize_attributes")]
    pub attributes: Attributes,
    pub trace_id: Option<TraceId>,
    pub span_id: Option<SpanId>,
    pub resource: Arc<Resource>,
    pub scope_name: String,
}

impl LogRecord {
    /// The emitting service's name (`unknown_service` when unnamed).
    pub fn service(&self) -> &str {
        self.resource.service_name()
    }

    /// The exception carried in `exception.*` attributes, if any of
    /// `exception.type`, `exception.message` or `exception.stacktrace` is
    /// present.
    pub fn exception(&self) -> Option<ExceptionInfo> {
        ExceptionInfo::find_in_attributes(&self.attributes)
    }

    /// Identity for dropping exporter retries: a hash of the time, body,
    /// trace ID, span ID and service. Stable within one process, which is all
    /// the in-memory store needs; do not persist it. Observed time and
    /// attributes are left out on purpose, since a retry may change them.
    pub fn dedup_key(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.time_unix_nano.hash(&mut hasher);
        self.body.hash(&mut hasher);
        self.trace_id.hash(&mut hasher);
        self.span_id.hash(&mut hasher);
        self.service().hash(&mut hasher);
        hasher.finish()
    }
}
