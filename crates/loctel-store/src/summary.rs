//! Trace summaries: the one-row view of a trace that lists, search results
//! and the MCP tools show.

use std::collections::HashMap;

use loctel_model::{ExceptionInfo, SpanId, TraceId};
use serde::Serialize;

use crate::failure::{FailedIn, TraceStatus, classify, pick_exception};
use crate::trace::{TraceEntry, root_span};

/// A trace in one row.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TraceSummary {
    pub trace_id: TraceId,
    /// The root span's name, `None` until the root arrives.
    pub root_name: Option<String>,
    /// The root span's service, `None` until the root arrives.
    pub root_service: Option<String>,
    /// The earliest span start.
    pub start_unix_nano: u64,
    /// From the earliest span start to the latest span end.
    pub duration_nanos: u64,
    pub span_count: usize,
    /// Each service once, in the order of its earliest span start (so the
    /// entry point comes first).
    pub services: Vec<String>,
    pub status: TraceStatus,
    pub failed_in: Option<FailedIn>,
    /// The exception that best explains the trace; see
    /// [`trace_exception`](crate::trace_exception).
    pub first_exception: Option<ExceptionInfo>,
    /// No new span has arrived for `completeness_quiet`.
    pub complete: bool,
}

/// What [`Store::search_traces`](crate::Store::search_traces) keeps. Every
/// condition that is set must hold; the default keeps everything.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TraceFilter {
    /// Keep traces that started at or after this time.
    pub since_unix_nano: Option<u64>,
    /// Keep traces in which any span belongs to this service (exact name).
    pub service: Option<String>,
    /// Keep failed traces and traces with errors inside, drop the rest.
    pub errors_only: bool,
    /// Keep traces lasting at least this long.
    pub min_duration_nanos: Option<u64>,
}

impl TraceEntry {
    /// Summarizes the trace. `complete` is the caller's reading of the clock.
    pub(crate) fn summary(&self, trace_id: TraceId, complete: bool) -> TraceSummary {
        let spans = self.spans();
        let root = root_span(spans);
        let (status, failed_in) = classify(spans, root.map(|span| span.span_id));
        let first_exception = pick_exception(spans, failed_in.as_ref().map(|f| f.span_id));
        TraceSummary {
            trace_id,
            root_name: root.map(|span| span.name.clone()),
            root_service: root.map(|span| span.service().to_owned()),
            start_unix_nano: self.start_unix_nano(),
            duration_nanos: self.duration_nanos(),
            span_count: spans.len(),
            services: self.services_in_start_order(),
            status,
            failed_in,
            first_exception,
            complete,
        }
    }

    /// Whether any span belongs to `service`.
    pub(crate) fn has_service(&self, service: &str) -> bool {
        self.spans().iter().any(|span| span.service() == service)
    }

    fn services_in_start_order(&self) -> Vec<String> {
        // The earliest (start, span ID) of each service. Span IDs are unique
        // in a trace, so the order is total.
        let mut first: HashMap<&str, (u64, SpanId)> = HashMap::new();
        for span in self.spans() {
            let key = (span.start_unix_nano, span.span_id);
            first
                .entry(span.service())
                .and_modify(|best| *best = (*best).min(key))
                .or_insert(key);
        }
        let mut services: Vec<(&str, (u64, SpanId))> = first.into_iter().collect();
        services.sort_unstable_by_key(|&(_, key)| key);
        services
            .into_iter()
            .map(|(name, _)| name.to_owned())
            .collect()
    }
}

/// The keyset cursor of a trace: `"{start_unix_nano}-{trace_id}"`, the
/// position of the last item of a page.
pub(crate) fn encode_cursor(start_unix_nano: u64, trace_id: TraceId) -> String {
    format!("{start_unix_nano}-{trace_id}")
}

/// Reads a cursor made by [`encode_cursor`]. `None` for anything else.
pub(crate) fn decode_cursor(cursor: &str) -> Option<(u64, TraceId)> {
    let (start, id) = cursor.split_once('-')?;
    // `parse` alone would also take a leading `+`.
    if start.is_empty() || !start.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((start.parse().ok()?, TraceId::from_hex(id).ok()?))
}
