//! The preview: what a rule would have dropped of the data the store holds.

use std::collections::HashSet;
use std::time::Duration;

use loctel_model::{Severity, SpanId, TraceId};
use loctel_query::QueryError;
use loctel_query::traceql::{TraceQuery, TraceView};
use loctel_store::{Store, TraceStatus};

use crate::set::{Compiled, compile, subtree_ids};
use crate::spec::{DropScope, RuleSpec};

/// How far back the rules editor previews: the last five minutes of stored
/// data (design §7). The caller turns it into the `since` that [`preview`]
/// takes.
pub const PREVIEW_WINDOW: Duration = Duration::from_secs(5 * 60);

/// What a rule would have dropped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Preview {
    /// Traces the rule matches (for a TraceQL rule).
    pub traces: u64,
    /// Spans it would drop: every span of the matched traces for the trace
    /// scope; the matched spans and their descendants for the span scope.
    pub spans: u64,
    /// Logs it would drop: for a LogQL rule, the logs it matches; for a TraceQL
    /// rule, the logs of the dropped traces (trace scope) or spans (span
    /// scope), as the rule drops them at ingest.
    pub logs: u64,
    /// Matched traces whose status is not `Ok`, the trace as a whole (also for
    /// the span scope: a rule that drops one span of a failed trace is flagged,
    /// which errs on the side of warning).
    pub error_traces: u64,
    /// Dropped logs at level `error` or above.
    pub error_logs: u64,
}

/// Runs `spec` over the stored traces that start at or after `since_unix_nano`,
/// and the stored logs from then on, and reports what it would have dropped.
/// Only this rule is considered: other rules, whether this one is enabled, and
/// the rules' memory of dropped data are all ignored. Nothing is changed.
///
/// A rule is judged on each trace as the store holds it now, which for a trace
/// that is still arriving may be more than a live rule saw at any one moment.
///
/// The error is the parser's, with the position in the query.
pub fn preview(
    spec: &RuleSpec,
    store: &Store,
    since_unix_nano: u64,
) -> Result<Preview, QueryError> {
    Ok(match compile(spec)? {
        Compiled::Trace(query) => preview_trace_rule(spec.scope, &query, store, since_unix_nano),
        Compiled::Log(query) => {
            let mut result = Preview::default();
            store.scan_logs(since_unix_nano, |log| {
                if query.service_hint().is_none_or(|svc| svc == log.service()) && query.matches(log)
                {
                    result.logs += 1;
                    result.error_logs += u64::from(log.severity >= Severity::Error);
                }
            });
            result
        }
    })
}

fn preview_trace_rule(
    scope: DropScope,
    query: &TraceQuery,
    store: &Store,
    since_unix_nano: u64,
) -> Preview {
    let mut result = Preview::default();
    // What the rule would drop, for the logs: whole traces, or spans.
    let mut dropped_traces: HashSet<TraceId> = HashSet::new();
    let mut dropped_spans: HashSet<(TraceId, SpanId)> = HashSet::new();

    store.scan_traces(since_unix_nano, |trace, spans, status| {
        let matched = query.evaluate(&TraceView::new(spans));
        if matched.is_empty() {
            return;
        }
        result.traces += 1;
        result.error_traces += u64::from(status != TraceStatus::Ok);
        match scope {
            DropScope::Trace => {
                result.spans += spans.len() as u64;
                dropped_traces.insert(trace);
            }
            DropScope::Span => {
                let roots: Vec<SpanId> = matched.iter().map(|&m| spans[m].span_id).collect();
                let doomed = subtree_ids(
                    spans.iter().map(|span| (span.span_id, span.parent_span_id)),
                    &roots,
                );
                for span in spans.iter().filter(|span| doomed.contains(&span.span_id)) {
                    result.spans += 1;
                    dropped_spans.insert((trace, span.span_id));
                }
            }
        }
    });

    if !dropped_traces.is_empty() || !dropped_spans.is_empty() {
        store.scan_logs(since_unix_nano, |log| {
            let dropped = match (log.trace_id, log.span_id) {
                (Some(trace), span) => {
                    dropped_traces.contains(&trace)
                        || span.is_some_and(|span| dropped_spans.contains(&(trace, span)))
                }
                _ => false,
            };
            if dropped {
                result.logs += 1;
                result.error_logs += u64::from(log.severity >= Severity::Error);
            }
        });
    }
    result
}
