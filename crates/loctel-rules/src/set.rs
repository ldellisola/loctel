//! The rules at work: `RuleSet` judges every batch of spans and logs on its way
//! into the store (design §7).
//!
//! The semantics, in the order a span meets them (`docs/areas/rules.md` has the
//! reasons):
//!
//! 1. **A remembered trace.** The trace was dropped by a rule earlier, so the
//!    span goes, and counts on that rule.
//! 2. **A remembered parent.** The span's parent was dropped (a span-scope
//!    match, or a descendant of one), so the span is a late descendant: it
//!    goes, and its own ID is remembered for its children.
//! 3. **The rules, in file order.** A single-span query judges the span; a
//!    query that needs the whole trace is evaluated once per trace, over the
//!    stored spans of the trace followed by the batch's.
//! 4. Otherwise the span is kept.
//!
//! Everything is decided under one lock, so batches from the gRPC and HTTP
//! receivers see each other's drops. The caller removes what the verdict lists
//! from the store (`RuleSet::remove_dropped`) before it stores the rest.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use loctel_model::{LogRecord, Span, SpanId, TraceId};
use loctel_query::QueryError;
use loctel_query::logql::{self, LogQuery};
use loctel_query::traceql::{self, TraceQuery, TraceView};
use loctel_store::{Clock, Removed, Store};
use parking_lot::Mutex;

use crate::memory::Memory;
use crate::spec::{DropScope, RuleLanguage, RuleSpec};

/// How long the rules remember that a trace (or a span) was dropped: data that
/// arrives for it in that time is dropped too.
pub const DROPPED_TTL: Duration = Duration::from_secs(10 * 60);

/// The most trace IDs, and the most span IDs, remembered at once (each kind has
/// its own memory of this size). Past it the oldest are forgotten first.
pub const DROPPED_MAX: usize = 100_000;

/// A rule's identity for as long as it stays in the rule list: assigned when
/// the rule first appears and kept, across [`RuleSet::replace`], for a rule
/// with the same name, language, query and scope, wherever it moves to. IDs are
/// never reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RuleId(pub u64);

/// A rule and how it is doing.
#[derive(Clone, Debug, PartialEq)]
pub struct RuleStatus {
    pub id: RuleId,
    pub spec: RuleSpec,
    /// Why the query does not parse (the parser's own text, with the column),
    /// or `None`. A rule with an error does nothing.
    pub error: Option<String>,
    /// Spans dropped by this rule since it appeared: the incoming ones, plus
    /// the stored ones the caller removed for it.
    pub dropped_spans: u64,
    /// Logs dropped by this rule, counted the same way.
    pub dropped_logs: u64,
    /// Traces this rule dropped whole.
    pub dropped_traces: u64,
}

/// What [`RuleSet::filter_spans`] decided about a batch.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpanVerdict {
    /// The spans to store, in the order they came.
    pub keep: Vec<Span>,
    /// Traces dropped by this batch: the caller must remove what the store
    /// holds of them, spans and logs.
    pub drop_traces: Vec<TraceId>,
    /// Stored spans dropped by this batch (their trace stays): the caller must
    /// remove them, and the logs emitted in them.
    pub drop_spans: Vec<(TraceId, SpanId)>,
}

/// Everything the rules have dropped since launch, whichever rule dropped it
/// (and including rules that have since been removed).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DroppedTotals {
    pub spans: u64,
    pub logs: u64,
}

/// A compiled query.
#[derive(Clone)]
pub(crate) enum Compiled {
    Trace(Arc<TraceQuery>),
    Log(Arc<LogQuery>),
}

/// Parses a rule's query in the rule's language.
pub(crate) fn compile(spec: &RuleSpec) -> Result<Compiled, QueryError> {
    match spec.language {
        RuleLanguage::TraceQl => traceql::parse(&spec.query).map(|q| Compiled::Trace(Arc::new(q))),
        RuleLanguage::LogQl => logql::parse(&spec.query).map(|q| Compiled::Log(Arc::new(q))),
    }
}

/// `root` and every span below it, over the spans described by `links`
/// (span ID and parent ID). A parent that is not among the links is simply
/// not there: the span below it is still below it. Safe against loops.
pub(crate) fn subtree_ids(
    links: impl IntoIterator<Item = (SpanId, Option<SpanId>)>,
    roots: &[SpanId],
) -> HashSet<SpanId> {
    let mut children: HashMap<SpanId, Vec<SpanId>> = HashMap::new();
    for (id, parent) in links {
        if let Some(parent) = parent {
            children.entry(parent).or_default().push(id);
        }
    }
    let mut doomed: HashSet<SpanId> = HashSet::new();
    let mut stack = Vec::new();
    for &root in roots {
        if doomed.insert(root) {
            stack.push(root);
        }
    }
    while let Some(node) = stack.pop() {
        for &child in children.get(&node).into_iter().flatten() {
            if doomed.insert(child) {
                stack.push(child);
            }
        }
    }
    doomed
}

#[derive(Clone, Copy, Debug, Default)]
struct Counters {
    spans: u64,
    logs: u64,
    traces: u64,
}

struct RuleEntry {
    id: RuleId,
    spec: RuleSpec,
    /// The query, or the parser's error text.
    compiled: Result<Compiled, String>,
    counters: Counters,
}

/// A trace and the rule that dropped some of it (`None` when the memory no
/// longer says).
type TraceAndRule = (TraceId, Option<RuleId>);

/// What makes a rule "the same rule" across `replace`.
type Identity = (String, RuleLanguage, String, DropScope);

fn identity(spec: &RuleSpec) -> Identity {
    (
        spec.name.clone(),
        spec.language,
        spec.query.clone(),
        spec.scope,
    )
}

struct State {
    rules: Vec<RuleEntry>,
    by_id: HashMap<RuleId, usize>,
    next_id: u64,
    traces: Memory<TraceId>,
    spans: Memory<(TraceId, SpanId)>,
    totals: DroppedTotals,
}

impl State {
    fn new(specs: Vec<RuleSpec>) -> State {
        let ttl = u64::try_from(DROPPED_TTL.as_nanos()).unwrap_or(u64::MAX);
        let mut state = State {
            rules: Vec::new(),
            by_id: HashMap::new(),
            next_id: 1,
            traces: Memory::new(ttl, DROPPED_MAX),
            spans: Memory::new(ttl, DROPPED_MAX),
            totals: DroppedTotals::default(),
        };
        state.replace(specs);
        state
    }

    fn fresh(&mut self, spec: RuleSpec) -> RuleEntry {
        let id = RuleId(self.next_id);
        self.next_id += 1;
        let compiled = compile(&spec).map_err(|e| e.to_string());
        RuleEntry {
            id,
            spec,
            compiled,
            counters: Counters::default(),
        }
    }

    fn replace(&mut self, specs: Vec<RuleSpec>) {
        let mut old: HashMap<Identity, VecDeque<RuleEntry>> = HashMap::new();
        for entry in std::mem::take(&mut self.rules) {
            old.entry(identity(&entry.spec))
                .or_default()
                .push_back(entry);
        }
        let mut rules = Vec::with_capacity(specs.len());
        for spec in specs {
            let same = old.get_mut(&identity(&spec)).and_then(VecDeque::pop_front);
            rules.push(match same {
                // Only `enabled` can differ; the query is the same, so it need
                // not be parsed again.
                Some(mut entry) => {
                    entry.spec = spec;
                    entry
                }
                None => self.fresh(spec),
            });
        }
        self.by_id = rules
            .iter()
            .enumerate()
            .map(|(position, entry)| (entry.id, position))
            .collect();
        self.rules = rules;
    }

    /// Adds to a rule's counters (if it is still a rule) and to the totals.
    fn credit(&mut self, rule: RuleId, spans: u64, logs: u64, traces: u64) {
        if let Some(&position) = self.by_id.get(&rule) {
            let counters = &mut self.rules[position].counters;
            counters.spans += spans;
            counters.logs += logs;
            counters.traces += traces;
        }
        self.totals.spans += spans;
        self.totals.logs += logs;
    }

    /// The enabled, valid TraceQL rules, in file order.
    fn active_trace_rules(&self) -> Vec<ActiveTrace> {
        self.rules
            .iter()
            .filter(|rule| rule.spec.enabled)
            .filter_map(|rule| match &rule.compiled {
                Ok(Compiled::Trace(query)) => Some(ActiveTrace {
                    id: rule.id,
                    scope: rule.spec.scope,
                    query: query.clone(),
                }),
                _ => None,
            })
            .collect()
    }

    /// The enabled, valid LogQL rules, in file order.
    fn active_log_rules(&self) -> Vec<(RuleId, Arc<LogQuery>)> {
        self.rules
            .iter()
            .filter(|rule| rule.spec.enabled)
            .filter_map(|rule| match &rule.compiled {
                Ok(Compiled::Log(query)) => Some((rule.id, query.clone())),
                _ => None,
            })
            .collect()
    }
}

struct ActiveTrace {
    id: RuleId,
    scope: DropScope,
    query: Arc<TraceQuery>,
}

/// The discard rules: the list, what each has dropped, and the memory of what
/// was dropped. Share it by reference (it is `Send + Sync`).
pub struct RuleSet {
    clock: Arc<dyn Clock>,
    state: Mutex<State>,
}

impl RuleSet {
    /// A rule set holding `specs`, in order. `clock` times the memory of
    /// dropped data (use the store's clock, so that tests move both at once).
    pub fn new(specs: Vec<RuleSpec>, clock: Arc<dyn Clock>) -> RuleSet {
        RuleSet {
            clock,
            state: Mutex::new(State::new(specs)),
        }
    }

    /// Makes `specs` the rules, in that order. A rule that is the same as one
    /// held now (same name, language, query and scope; whether it is enabled
    /// may differ) keeps its [`RuleId`] and its counters. What was dropped is
    /// still remembered, whether or not the rule that dropped it is kept.
    pub fn replace(&self, specs: Vec<RuleSpec>) {
        self.state.lock().replace(specs);
    }

    /// Every rule with its counters and the error of its query, in file order.
    pub fn statuses(&self) -> Vec<RuleStatus> {
        self.state
            .lock()
            .rules
            .iter()
            .map(|rule| RuleStatus {
                id: rule.id,
                spec: rule.spec.clone(),
                error: rule.compiled.as_ref().err().cloned(),
                dropped_spans: rule.counters.spans,
                dropped_logs: rule.counters.logs,
                dropped_traces: rule.counters.traces,
            })
            .collect()
    }

    /// What all the rules together have dropped since launch.
    pub fn totals(&self) -> DroppedTotals {
        self.state.lock().totals
    }

    /// Adds what the caller removed from the store for `rule` to its counters
    /// (and to the totals): the stored spans and logs that a verdict said to
    /// remove. A rule that is no longer in the list counts in the totals only.
    /// [`remove_dropped`](Self::remove_dropped) does this for a whole verdict.
    pub fn record_removed(&self, rule: RuleId, removed: Removed) {
        self.state
            .lock()
            .credit(rule, removed.spans as u64, removed.logs as u64, 0);
    }

    /// Judges a batch of spans (see the module docs for the order), using the
    /// store to see the spans of a trace that arrived earlier. Nothing is
    /// changed in the store: the verdict lists what to store (`keep`) and what
    /// the store holds that must now go (`drop_traces`, `drop_spans`; the
    /// caller does that with [`remove_dropped`](Self::remove_dropped), and
    /// then stores `keep`).
    ///
    /// The caller must do the three steps for one batch before it judges the
    /// next (the engine's sink holds a lock for that), or a span kept by this
    /// batch could be stored after its trace was removed.
    ///
    /// Cost: with no rules and nothing remembered, none. A single-span rule
    /// costs one evaluation per span. A rule that needs the whole trace costs a
    /// copy of the trace's stored spans and an evaluation of the trace, once
    /// per trace in the batch.
    pub fn filter_spans(&self, spans: Vec<Span>, store: &Store) -> SpanVerdict {
        let mut state = self.state.lock();
        let now = self.clock.now_unix_nano();
        state.traces.expire(now);
        state.spans.expire(now);
        let active = state.active_trace_rules();
        if active.is_empty() && state.traces.is_empty() && state.spans.is_empty() {
            return SpanVerdict {
                keep: spans,
                ..SpanVerdict::default()
            };
        }

        let mut pass = Pass::new(&mut state, store, now, spans);
        pass.apply_memory();
        for rule in &active {
            if pass.live == 0 {
                break;
            }
            pass.apply_rule(rule);
        }
        pass.finish()
    }

    /// Judges a batch of logs and returns the ones to keep, in order. A log
    /// goes when its trace was dropped, when its span was dropped, or when an
    /// enabled LogQL rule matches it (the first in file order is credited).
    pub fn filter_logs(&self, logs: Vec<LogRecord>) -> Vec<LogRecord> {
        let mut state = self.state.lock();
        let now = self.clock.now_unix_nano();
        state.traces.expire(now);
        state.spans.expire(now);
        let active = state.active_log_rules();
        if active.is_empty() && state.traces.is_empty() && state.spans.is_empty() {
            return logs;
        }

        let mut keep = Vec::with_capacity(logs.len());
        for log in logs {
            let remembered = log
                .trace_id
                .and_then(|trace| state.traces.get(&trace, now))
                .or_else(|| match (log.trace_id, log.span_id) {
                    (Some(trace), Some(span)) => state.spans.get(&(trace, span), now),
                    _ => None,
                });
            let dropper = remembered.or_else(|| {
                active
                    .iter()
                    .find(|(_, query)| {
                        query.service_hint().is_none_or(|svc| svc == log.service())
                            && query.matches(&log)
                    })
                    .map(|(id, _)| *id)
            });
            match dropper {
                Some(rule) => state.credit(rule, 0, 1, 0),
                None => keep.push(log),
            }
        }
        keep
    }

    /// Does what a verdict asks of the store: removes the spans in
    /// `drop_spans` (and their logs) and the traces in `drop_traces` (spans and
    /// logs), and credits what was removed to the rule that dropped each
    /// (found in the memory of dropped data, which the verdict has just
    /// filled). Returns the total removed. The plan's `record_removed` is
    /// this, per rule, for a caller that knows the rule.
    ///
    /// The spans go first, on purpose: a span-scope rule may have condemned
    /// stored spans of a trace that a later trace-scope rule of the same batch
    /// then dropped whole. The spans are the earlier rule's ("a span is counted
    /// once, on the first rule that dropped it"), so they must be removed, and
    /// credited, before `remove_trace` sweeps up what is left for the trace's
    /// rule. (A span-scope rule that comes after a trace-scope one finds
    /// nothing of that trace live, so it never condemns anything in it.)
    ///
    /// Call it after `filter_spans` and before storing `keep`.
    pub fn remove_dropped(&self, verdict: &SpanVerdict, store: &Store) -> Removed {
        // Who dropped what, read under the lock; the removals run outside it.
        let (traces, spans) = {
            let state = self.state.lock();
            let traces: Vec<(TraceId, Option<RuleId>)> = verdict
                .drop_traces
                .iter()
                .map(|&trace| (trace, state.traces.peek(&trace)))
                .collect();
            let mut spans: Vec<(TraceAndRule, Vec<SpanId>)> = Vec::new();
            let mut position: HashMap<TraceAndRule, usize> = HashMap::new();
            for &(trace, span) in &verdict.drop_spans {
                let key = (trace, state.spans.peek(&(trace, span)));
                let at = *position.entry(key).or_insert_with(|| {
                    spans.push((key, Vec::new()));
                    spans.len() - 1
                });
                spans[at].1.push(span);
            }
            (traces, spans)
        };

        let mut total = Removed::default();
        let mut credits: Vec<(Option<RuleId>, Removed)> = Vec::new();
        // Spans before traces (see above): what a span-scope rule condemned is
        // credited to it, and `remove_trace` takes only what is left.
        for ((trace, rule), ids) in spans {
            credits.push((rule, store.remove_spans(trace, &ids)));
        }
        for (trace, rule) in traces {
            credits.push((rule, store.remove_trace(trace)));
        }

        let mut state = self.state.lock();
        for (rule, removed) in credits {
            total.spans += removed.spans;
            total.logs += removed.logs;
            let (spans, logs) = (removed.spans as u64, removed.logs as u64);
            match rule {
                Some(rule) => state.credit(rule, spans, logs, 0),
                None => {
                    state.totals.spans += spans;
                    state.totals.logs += logs;
                }
            }
        }
        total
    }
}

/// One call of `filter_spans`: the batch with the spans dropped so far taken
/// out of it, and what has to be removed from the store.
struct Pass<'a> {
    state: &'a mut State,
    store: &'a Store,
    now: u64,
    /// The batch; a span that is dropped leaves `None`.
    slots: Vec<Option<Span>>,
    /// How many slots are still `Some`.
    live: usize,
    drop_traces: Vec<TraceId>,
    drop_spans: Vec<(TraceId, SpanId)>,
    /// Stored spans that a rule of this pass has already condemned: later rules
    /// of the pass must not see them, and they are listed once.
    condemned: HashSet<(TraceId, SpanId)>,
    /// The slots of each trace of the batch, traces in order of first
    /// appearance. Filled by `ensure_groups`, when first needed.
    groups: Groups,
    groups_built: bool,
    /// The slots by `(trace, parent span ID)`. Filled by `ensure_children`,
    /// when first needed.
    children: HashMap<(TraceId, SpanId), Vec<usize>>,
    children_built: bool,
}

#[derive(Default)]
struct Groups {
    traces: Vec<(TraceId, Vec<usize>)>,
    position: HashMap<TraceId, usize>,
}

impl<'a> Pass<'a> {
    fn new(state: &'a mut State, store: &'a Store, now: u64, spans: Vec<Span>) -> Self {
        let live = spans.len();
        Pass {
            state,
            store,
            now,
            slots: spans.into_iter().map(Some).collect(),
            live,
            drop_traces: Vec::new(),
            drop_spans: Vec::new(),
            condemned: HashSet::new(),
            groups: Groups::default(),
            groups_built: false,
            children: HashMap::new(),
            children_built: false,
        }
    }

    fn finish(self) -> SpanVerdict {
        SpanVerdict {
            keep: self.slots.into_iter().flatten().collect(),
            drop_traces: self.drop_traces,
            drop_spans: self.drop_spans,
        }
    }

    /// Takes span `i` out of the batch, crediting `rule` with a dropped span.
    fn take_slot(&mut self, i: usize, rule: RuleId) -> Option<Span> {
        let span = self.slots[i].take()?;
        self.live -= 1;
        self.state.credit(rule, 1, 0, 0);
        Some(span)
    }

    fn ensure_groups(&mut self) {
        if self.groups_built {
            return;
        }
        self.groups_built = true;
        for (i, slot) in self.slots.iter().enumerate() {
            let Some(span) = slot else { continue };
            let at = *self
                .groups
                .position
                .entry(span.trace_id)
                .or_insert_with(|| {
                    self.groups.traces.push((span.trace_id, Vec::new()));
                    self.groups.traces.len() - 1
                });
            self.groups.traces[at].1.push(i);
        }
    }

    /// The batch's slots that are children of `parent` of `trace`.
    fn incoming_children(&mut self, trace: TraceId, parent: SpanId) -> Vec<usize> {
        if !self.children_built {
            self.children_built = true;
            for (i, slot) in self.slots.iter().enumerate() {
                if let Some(span) = slot
                    && let Some(parent) = span.parent_span_id
                {
                    self.children
                        .entry((span.trace_id, parent))
                        .or_default()
                        .push(i);
                }
            }
        }
        self.children
            .get(&(trace, parent))
            .cloned()
            .unwrap_or_default()
    }

    /// Steps 1 and 2: spans of a remembered trace, and late descendants of a
    /// remembered span.
    fn apply_memory(&mut self) {
        if self.state.traces.is_empty() && self.state.spans.is_empty() {
            return;
        }
        for i in 0..self.slots.len() {
            let Some(span) = &self.slots[i] else { continue };
            let (trace, parent) = (span.trace_id, span.parent_span_id);
            if let Some(rule) = self.state.traces.get(&trace, self.now) {
                self.take_slot(i, rule);
            } else if let Some(parent) = parent
                && let Some(rule) = self.state.spans.get(&(trace, parent), self.now)
            {
                self.drop_incoming_subtree(i, rule);
            }
        }
    }

    /// Drops span `i` and, through the batch, everything below it (a child
    /// may come before its parent in a batch), remembering every ID.
    fn drop_incoming_subtree(&mut self, i: usize, rule: RuleId) {
        let mut stack = vec![i];
        while let Some(at) = stack.pop() {
            let Some(span) = self.take_slot(at, rule) else {
                continue;
            };
            self.state
                .spans
                .remember((span.trace_id, span.span_id), rule, self.now);
            stack.extend(self.incoming_children(span.trace_id, span.span_id));
        }
    }

    /// Step 3 for one rule.
    fn apply_rule(&mut self, rule: &ActiveTrace) {
        if rule.query.needs_whole_trace() {
            self.apply_whole_trace_rule(rule);
        } else {
            self.apply_single_span_rule(rule);
        }
    }

    /// A rule whose query judges each span on its own.
    fn apply_single_span_rule(&mut self, rule: &ActiveTrace) {
        // Span scope: the matched spans, by trace, handled together below.
        let mut matched: Vec<(TraceId, Vec<SpanId>)> = Vec::new();
        let mut position: HashMap<TraceId, usize> = HashMap::new();
        for i in 0..self.slots.len() {
            let Some(span) = &self.slots[i] else { continue };
            if rule.query.matches_span(span) != Some(true) {
                continue;
            }
            let (trace, id) = (span.trace_id, span.span_id);
            match rule.scope {
                DropScope::Trace => self.drop_trace(rule.id, trace),
                DropScope::Span => {
                    let at = *position.entry(trace).or_insert_with(|| {
                        matched.push((trace, Vec::new()));
                        matched.len() - 1
                    });
                    matched[at].1.push(id);
                }
            }
        }
        for (trace, roots) in matched {
            self.drop_subtrees(rule.id, trace, &roots);
        }
    }

    /// A rule whose query needs the rest of the trace: each trace of the batch
    /// that still has spans is judged as it would stand once the batch is
    /// stored.
    fn apply_whole_trace_rule(&mut self, rule: &ActiveTrace) {
        self.ensure_groups();
        for group in 0..self.groups.traces.len() {
            let trace = self.groups.traces[group].0;
            let live = self.live_slots(trace);
            if live.is_empty() {
                continue;
            }
            let combined = self.trace_as_it_would_stand(trace, &live);
            let matched = rule.query.evaluate(&TraceView::new(&combined));
            if matched.is_empty() {
                continue;
            }
            match rule.scope {
                DropScope::Trace => self.drop_trace(rule.id, trace),
                DropScope::Span => {
                    let roots: Vec<SpanId> = matched.iter().map(|&m| combined[m].span_id).collect();
                    self.drop_subtrees(rule.id, trace, &roots);
                }
            }
        }
    }

    /// The batch's slots of `trace` that are still live. Needs `ensure_groups`.
    fn live_slots(&self, trace: TraceId) -> Vec<usize> {
        self.groups
            .position
            .get(&trace)
            .map(|&group| {
                self.groups.traces[group]
                    .1
                    .iter()
                    .copied()
                    .filter(|&i| self.slots[i].is_some())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The spans the store holds of `trace` (less those this pass has already
    /// condemned), then the batch's live spans of it that the store does not
    /// have yet.
    fn trace_as_it_would_stand(&self, trace: TraceId, live: &[usize]) -> Vec<Span> {
        let condemned = &self.condemned;
        let mut combined: Vec<Span> = self
            .store
            .with_trace_spans(trace, |stored| {
                stored
                    .iter()
                    .filter(|span| !condemned.contains(&(trace, span.span_id)))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let held: HashSet<SpanId> = combined.iter().map(|span| span.span_id).collect();
        combined.extend(
            live.iter()
                .filter_map(|&i| self.slots[i].as_ref())
                .filter(|span| !held.contains(&span.span_id))
                .cloned(),
        );
        combined
    }

    /// A trace-scope match: the whole trace goes. Every incoming span of it is
    /// dropped now, the caller removes what is stored, and the trace is
    /// remembered.
    fn drop_trace(&mut self, rule: RuleId, trace: TraceId) {
        if self.state.traces.get(&trace, self.now).is_some() {
            return;
        }
        self.state.traces.remember(trace, rule, self.now);
        self.drop_traces.push(trace);
        self.state.credit(rule, 0, 0, 1);
        self.ensure_groups();
        for i in self.live_slots(trace) {
            self.take_slot(i, rule);
        }
    }

    /// A span-scope match: `roots` and everything below them, among the
    /// stored spans of the trace and the batch's. Incoming ones are dropped now,
    /// stored ones go in `drop_spans` for the caller to remove, and every ID
    /// is remembered, so that descendants arriving later are dropped too.
    fn drop_subtrees(&mut self, rule: RuleId, trace: TraceId, roots: &[SpanId]) {
        let condemned = &self.condemned;
        let stored: Vec<(SpanId, Option<SpanId>)> = self
            .store
            .with_trace_spans(trace, |held| {
                held.iter()
                    .filter(|span| !condemned.contains(&(trace, span.span_id)))
                    .map(|span| (span.span_id, span.parent_span_id))
                    .collect()
            })
            .unwrap_or_default();
        self.ensure_groups();
        let live = self.live_slots(trace);
        let incoming: Vec<(usize, SpanId, Option<SpanId>)> = live
            .iter()
            .filter_map(|&i| {
                self.slots[i]
                    .as_ref()
                    .map(|span| (i, span.span_id, span.parent_span_id))
            })
            .collect();

        let doomed = subtree_ids(
            stored
                .iter()
                .copied()
                .chain(incoming.iter().map(|&(_, id, parent)| (id, parent))),
            roots,
        );

        for (id, _) in stored {
            if doomed.contains(&id) {
                self.drop_spans.push((trace, id));
                self.condemned.insert((trace, id));
                self.state.spans.remember((trace, id), rule, self.now);
            }
        }
        for (i, id, _) in incoming {
            if doomed.contains(&id) {
                self.take_slot(i, rule);
                self.state.spans.remember((trace, id), rule, self.now);
            }
        }
    }
}
