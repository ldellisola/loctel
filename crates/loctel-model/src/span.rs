//! Spans and their events.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::attr::{Attributes, deserialize_attributes, serialize_attributes, str_attr};
use crate::exception::ExceptionInfo;
use crate::ids::{SpanId, TraceId};
use crate::resource::Resource;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanKind {
    Unspecified,
    Internal,
    Server,
    Client,
    Producer,
    Consumer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusCode {
    Unset,
    Ok,
    Error,
}

/// A timestamped event on a span, for example an `exception`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpanEvent {
    pub time_unix_nano: u64,
    pub name: String,
    #[serde(
        serialize_with = "serialize_attributes",
        deserialize_with = "deserialize_attributes"
    )]
    pub attributes: Attributes,
}

/// One unit of work in a trace.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Span {
    pub trace_id: TraceId,
    pub span_id: SpanId,
    pub parent_span_id: Option<SpanId>,
    pub name: String,
    pub kind: SpanKind,
    pub start_unix_nano: u64,
    pub end_unix_nano: u64,
    pub status: StatusCode,
    pub status_message: String,
    #[serde(
        serialize_with = "serialize_attributes",
        deserialize_with = "deserialize_attributes"
    )]
    pub attributes: Attributes,
    pub events: Vec<SpanEvent>,
    pub resource: Arc<Resource>,
    pub scope_name: String,
}

/// The event name that semantic conventions use for exceptions.
const EXCEPTION_EVENT: &str = "exception";

impl Span {
    /// End minus start. Clock skew between hosts can put the end before the
    /// start; that clamps to 0 instead of wrapping or panicking.
    pub fn duration_nanos(&self) -> u64 {
        self.end_unix_nano.saturating_sub(self.start_unix_nano)
    }

    /// Erroring means status `Error`. An exception event alone does not
    /// count: it may have been handled or retried (design 9.4).
    pub fn is_error(&self) -> bool {
        self.status == StatusCode::Error
    }

    /// The emitting service's name (`unknown_service` when unnamed).
    pub fn service(&self) -> &str {
        self.resource.service_name()
    }

    /// The first event named `exception`, read as an [`ExceptionInfo`]
    /// (missing attributes come out empty).
    pub fn exception(&self) -> Option<ExceptionInfo> {
        self.events
            .iter()
            .find(|event| event.name == EXCEPTION_EVENT)
            .map(|event| ExceptionInfo::from_attributes(&event.attributes))
    }

    /// The remote party a client span talks to: `server.address`, else
    /// `peer.service`.
    pub fn peer(&self) -> Option<&str> {
        str_attr(&self.attributes, "server.address")
            .or_else(|| str_attr(&self.attributes, "peer.service"))
    }
}
