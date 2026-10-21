//! The result types of failure classification (design 9.4).
//!
//! This module only declares the types. The classification itself is not
//! implemented yet, so a [`TraceSnapshot`](crate::TraceSnapshot) always
//! reports [`TraceStatus::Ok`] and no [`FailedIn`] for now.

use loctel_model::SpanId;
use serde::Serialize;

/// Did the request work? Used everywhere a trace is shown or filtered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceStatus {
    /// No span is erroring.
    Ok,
    /// The root span is fine (or has not arrived) but another span is
    /// erroring.
    ErrorsInside,
    /// The root span is erroring.
    Failed,
}

/// The span that explains a failure: the earliest-starting span among the
/// erroring spans that have no erroring descendant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FailedIn {
    pub span_id: SpanId,
    pub service: String,
    pub span_name: String,
    /// The remote party, for client and producer spans.
    pub peer: Option<String>,
    /// How many other such spans there are ("+N more").
    pub more: usize,
}
