//! Shared builders for the rules crate's integration tests.
//!
//! Every test file that needs them does `mod fixtures;` and uses the subset it
//! wants, so unused items are expected here. (Cargo also compiles this file as
//! a test binary of its own, with no tests in it.)
#![allow(dead_code)]

use std::sync::Arc;

use loctel_ingest::{logs_from_request, spans_from_request};
use loctel_meshgen::{Scenario, generate};
use loctel_model::{
    AttrValue, LogRecord, Resource, Severity, Span, SpanId, SpanKind, StatusCode, TraceId,
};
use loctel_rules::{DropScope, RuleLanguage, RuleSet, RuleSpec, SpanVerdict};
use loctel_store::{ManualClock, Store, StoreConfig};

/// Where the manual clock starts: well away from 0 so that "now minus
/// something" never underflows.
pub const CLOCK_START: u64 = 1_000_000_000_000;

/// A trace ID whose 16 bytes all equal `n`.
pub fn tid(n: u8) -> TraceId {
    TraceId([n; 16])
}

/// A span ID whose 8 bytes all equal `n`.
pub fn sid(n: u8) -> SpanId {
    SpanId([n; 8])
}

/// A span of trace `trace` with ID `id`, under `parent` (`None` for a root).
/// Times are in milliseconds and stored as nanoseconds. The span is
/// `Internal`, status `Unset`, with no attributes or events.
pub fn span(
    trace: u8,
    id: u8,
    parent: Option<u8>,
    service: &str,
    name: &str,
    start_ms: u64,
    dur_ms: u64,
) -> Span {
    Span {
        trace_id: tid(trace),
        span_id: sid(id),
        parent_span_id: parent.map(sid),
        name: name.to_string(),
        kind: SpanKind::Internal,
        start_unix_nano: start_ms * 1_000_000,
        end_unix_nano: (start_ms + dur_ms) * 1_000_000,
        status: StatusCode::Unset,
        status_message: String::new(),
        attributes: vec![],
        events: vec![],
        resource: Arc::new(Resource {
            attributes: vec![(
                "service.name".to_string(),
                AttrValue::Str(service.to_string()),
            )],
        }),
        scope_name: "test".to_string(),
    }
}

/// The same span with one more string attribute.
pub fn with_attr(mut span: Span, key: &str, value: &str) -> Span {
    span.attributes
        .push((key.to_string(), AttrValue::Str(value.to_string())));
    span
}

/// The same span with kind `kind`.
pub fn with_kind(mut span: Span, kind: SpanKind) -> Span {
    span.kind = kind;
    span
}

/// The same span with status `Error`.
pub fn error(mut span: Span) -> Span {
    span.status = StatusCode::Error;
    span
}

/// A log line from `service` at `time_ms`. Severity `Info`, no trace context.
pub fn log(service: &str, body: &str, time_ms: u64) -> LogRecord {
    LogRecord {
        time_unix_nano: time_ms * 1_000_000,
        observed_unix_nano: time_ms * 1_000_000,
        severity: Severity::Info,
        severity_text: "INFO".to_string(),
        body: body.to_string(),
        attributes: vec![],
        trace_id: None,
        span_id: None,
        resource: Arc::new(Resource {
            attributes: vec![(
                "service.name".to_string(),
                AttrValue::Str(service.to_string()),
            )],
        }),
        scope_name: "test".to_string(),
    }
}

/// The same log line emitted inside span `span` of trace `trace`.
pub fn in_span(mut log: LogRecord, trace: u8, span: u8) -> LogRecord {
    log.trace_id = Some(tid(trace));
    log.span_id = Some(sid(span));
    log
}

/// The same log line emitted inside the span with these (arbitrary) IDs.
pub fn in_ids(mut log: LogRecord, trace: TraceId, span: SpanId) -> LogRecord {
    log.trace_id = Some(trace);
    log.span_id = Some(span);
    log
}

/// The same log line carrying a trace ID but no span ID.
pub fn in_trace(mut log: LogRecord, trace: u8) -> LogRecord {
    log.trace_id = Some(tid(trace));
    log
}

/// A rule, enabled.
pub fn rule(name: &str, language: RuleLanguage, query: &str, scope: DropScope) -> RuleSpec {
    RuleSpec {
        name: name.to_string(),
        enabled: true,
        language,
        query: query.to_string(),
        scope,
    }
}

/// A TraceQL rule that drops the whole trace.
pub fn trace_rule(name: &str, query: &str) -> RuleSpec {
    rule(name, RuleLanguage::TraceQl, query, DropScope::Trace)
}

/// A TraceQL rule that drops the matching spans and their descendants.
pub fn span_rule(name: &str, query: &str) -> RuleSpec {
    rule(name, RuleLanguage::TraceQl, query, DropScope::Span)
}

/// A LogQL rule.
pub fn log_rule(name: &str, query: &str) -> RuleSpec {
    rule(name, RuleLanguage::LogQl, query, DropScope::Trace)
}

/// A store, the clock it and the rules share, and the rules, wired the way the
/// engine wires them: every batch goes through the rules first.
pub struct World {
    pub store: Store,
    pub clock: Arc<ManualClock>,
    pub rules: RuleSet,
}

impl World {
    pub fn new(specs: Vec<RuleSpec>) -> World {
        let clock = Arc::new(ManualClock::new(CLOCK_START));
        let store = Store::new(StoreConfig::default(), clock.clone());
        let rules = RuleSet::new(specs, clock.clone());
        World {
            store,
            clock,
            rules,
        }
    }

    /// One batch of spans: the rules judge it, what they say to remove is
    /// removed from the store, and the rest is stored. Returns the verdict.
    pub fn ingest_spans(&self, spans: Vec<Span>) -> SpanVerdict {
        let verdict = self.rules.filter_spans(spans, &self.store);
        self.rules.remove_dropped(&verdict, &self.store);
        self.store.ingest_spans(verdict.keep.clone());
        verdict
    }

    /// One batch of logs through the rules and into the store. Returns what
    /// was kept.
    pub fn ingest_logs(&self, logs: Vec<LogRecord>) -> Vec<LogRecord> {
        let kept = self.rules.filter_logs(logs);
        self.store.ingest_logs(kept.clone());
        kept
    }

    /// The counters of the rule at `position` in the file.
    pub fn counts(&self, position: usize) -> Counts {
        let status = &self.rules.statuses()[position];
        Counts {
            traces: status.dropped_traces,
            spans: status.dropped_spans,
            logs: status.dropped_logs,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Counts {
    pub traces: u64,
    pub spans: u64,
    pub logs: u64,
}

pub fn counts(traces: u64, spans: u64, logs: u64) -> Counts {
    Counts {
        traces,
        spans,
        logs,
    }
}

// ---- the fake mesh -----------------------------------------------------------

/// The spans of every trace export in `scenario`, in the order the batches
/// carry them, converted by the real OTLP conversion code.
pub fn scenario_spans(scenario: &Scenario, seed: u64, base_unix_nano: u64) -> Vec<Span> {
    generate(scenario, seed, base_unix_nano)
        .batches
        .into_iter()
        .filter_map(|batch| batch.traces)
        .flat_map(|request| spans_from_request(request).items)
        .collect()
}

/// The log records of every logs export in `scenario`, converted like
/// [`scenario_spans`].
pub fn scenario_logs(scenario: &Scenario, seed: u64, base_unix_nano: u64) -> Vec<LogRecord> {
    generate(scenario, seed, base_unix_nano)
        .batches
        .into_iter()
        .filter_map(|batch| batch.logs)
        .flat_map(|request| logs_from_request(request).items)
        .collect()
}

/// The spans of one trace export at a time: one `Vec` per batch of the
/// scenario, in the order they are sent.
pub fn scenario_span_batches(
    scenario: &Scenario,
    seed: u64,
    base_unix_nano: u64,
) -> Vec<Vec<Span>> {
    generate(scenario, seed, base_unix_nano)
        .batches
        .into_iter()
        .filter_map(|batch| batch.traces)
        .map(|request| spans_from_request(request).items)
        .collect()
}

/// What each batch of `scenario` carries, in the order they are sent: its
/// spans and its logs.
pub fn scenario_batches(
    scenario: &Scenario,
    seed: u64,
    base_unix_nano: u64,
) -> Vec<(Vec<Span>, Vec<LogRecord>)> {
    generate(scenario, seed, base_unix_nano)
        .batches
        .into_iter()
        .map(|batch| {
            let spans = batch
                .traces
                .map(|request| spans_from_request(request).items)
                .unwrap_or_default();
            let logs = batch
                .logs
                .map(|request| logs_from_request(request).items)
                .unwrap_or_default();
            (spans, logs)
        })
        .collect()
}

/// A copy of `template` with a new ID, parent and name: a span of the same
/// trace, service and kind.
pub fn child_of(template: &Span, id: u8, parent: SpanId, name: &str) -> Span {
    let mut span = template.clone();
    span.span_id = sid(id);
    span.parent_span_id = Some(parent);
    span.name = name.to_string();
    span.status = StatusCode::Unset;
    span.events.clear();
    span.attributes.clear();
    span
}

/// The span called `name` (the first one, when several share the name).
pub fn named<'a>(spans: &'a [Span], name: &str) -> &'a Span {
    spans
        .iter()
        .find(|span| span.name == name)
        .unwrap_or_else(|| panic!("no span called {name}"))
}

/// The spans without the one called `name`.
pub fn without(spans: &[Span], name: &str) -> Vec<Span> {
    spans
        .iter()
        .filter(|span| span.name != name)
        .cloned()
        .collect()
}
