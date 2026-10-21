//! The service map and per-service stats (design 9.2 and 9.4), computed on
//! demand from the traces held.
//!
//! The rules, all counted over spans that start inside the window:
//!
//! - A **service node** per `service.name`. Its `requests` and `errors` count
//!   that service's server and consumer spans (its entry points).
//! - A **service-to-service edge** for a span whose parent is in another
//!   service. It is an error if either span is erroring. It is skipped when
//!   both spans carry `messaging.system`: the queue sits between them.
//! - A **database** node for a client span with `db.system.name` (or the
//!   legacy `db.system`), with an edge from its service.
//! - A **queue** node for a producer span with `messaging.system`, with an
//!   edge `service -> queue`, and an edge `queue -> service` for a consumer
//!   span.
//! - An **external** node for a client span with no database or messaging
//!   attribute and no child in another service, once its trace is complete
//!   (until then the remote service's own span may still be on its way).

use std::collections::{HashMap, HashSet};

use loctel_model::{AttrValue, Span, SpanId, SpanKind, attr};
use serde::Serialize;

use crate::trace::TraceEntry;

/// Which spans a map or a services list is computed over: those that start
/// at or after `since_unix_nano`. There is no upper bound; the window runs up
/// to now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TimeWindow {
    pub since_unix_nano: u64,
}

/// What a node of the map stands for.
///
/// The order is the order nodes with the same id sort in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    /// A service that sends telemetry, named by `service.name`.
    Service,
    /// A database, named by `db.system.name`.
    Database,
    /// A message broker, named by `messaging.system`.
    Queue,
    /// A remote host that never reported, named by `server.address` (or
    /// `peer.service`, or `unknown`).
    External,
}

/// One node of the service map.
///
/// For a [`NodeKind::Service`], `requests` and `errors` count its server and
/// consumer spans. For the other kinds they count the calls the node
/// received (the spans that gave it an incoming edge) and how many of those
/// were erroring.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ServiceNode {
    pub id: String,
    pub kind: NodeKind,
    pub requests: u64,
    pub errors: u64,
}

/// A directed edge: `calls` calls from node `from` to node `to`, of which
/// `errors` were erroring. `from` and `to` are node ids.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ServiceEdge {
    pub from: String,
    pub to: String,
    pub calls: u64,
    pub errors: u64,
}

/// The map: nodes sorted by id (then kind), edges by `(from, to)`.
///
/// Every edge's `from` and `to` is the id of a node in `nodes`. Ids are
/// unique within a kind, not across kinds: a database called like a service
/// is a second node with the same id.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ServiceMap {
    pub nodes: Vec<ServiceNode>,
    pub edges: Vec<ServiceEdge>,
}

/// One line of the services list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ServiceStats {
    pub name: String,
    /// Server and consumer spans in the window.
    pub requests: u64,
    /// How many of those are erroring.
    pub errors: u64,
    /// The latest start of any of the service's spans in the window.
    pub last_seen_unix_nano: u64,
}

#[derive(Default)]
struct Counts {
    requests: u64,
    errors: u64,
    /// Latest span start. Only kept for services.
    last_seen: u64,
}

#[derive(Default)]
struct EdgeCounts {
    calls: u64,
    errors: u64,
}

/// Counts built up trace by trace. Names are borrowed from the spans, which
/// the caller keeps locked while it runs.
#[derive(Default)]
pub(crate) struct Aggregate<'a> {
    nodes: HashMap<(NodeKind, &'a str), Counts>,
    edges: HashMap<(&'a str, &'a str), EdgeCounts>,
}

impl<'a> Aggregate<'a> {
    /// Adds what the spans of `entry` that start in `window` contribute.
    /// `complete` is the caller's reading of the clock. With `dependencies`
    /// off only the service nodes are counted, which is all the services
    /// list needs.
    pub(crate) fn add_trace(
        &mut self,
        entry: &'a TraceEntry,
        complete: bool,
        window: TimeWindow,
        dependencies: bool,
    ) {
        let since = window.since_unix_nano;
        // Every span starts at or before the trace's latest end, so a trace
        // that ended before the window has nothing in it.
        if entry.end_unix_nano() < since {
            return;
        }
        // Parents with a child in another service, worked out on first use.
        let mut remote_parents: Option<HashSet<SpanId>> = None;
        for span in entry.spans() {
            if span.start_unix_nano < since {
                continue;
            }
            self.service_span(span);
            if !dependencies {
                continue;
            }
            let service = span.service();

            if let Some(parent) = span.parent_span_id.and_then(|id| entry.span(id))
                && parent.service() != service
                && parent.start_unix_nano >= since
                && !(messaging_system(parent).is_some() && messaging_system(span).is_some())
            {
                self.edge(
                    parent.service(),
                    service,
                    parent.is_error() || span.is_error(),
                );
            }

            match span.kind {
                SpanKind::Client => {
                    if let Some(database) = database_system(span) {
                        self.call(NodeKind::Database, database, service, span.is_error());
                    } else if complete
                        && messaging_system(span).is_none()
                        && !remote_parents
                            .get_or_insert_with(|| parents_with_remote_child(entry))
                            .contains(&span.span_id)
                    {
                        let host = span.peer().unwrap_or("unknown");
                        self.call(NodeKind::External, host, service, span.is_error());
                    }
                }
                SpanKind::Producer => {
                    if let Some(queue) = messaging_system(span) {
                        self.call(NodeKind::Queue, queue, service, span.is_error());
                    }
                }
                SpanKind::Consumer => {
                    if let Some(queue) = messaging_system(span) {
                        // The delivery is an edge out of the queue. The queue
                        // node exists even if no producer was seen.
                        self.node(NodeKind::Queue, queue);
                        self.edge(queue, service, span.is_error());
                    }
                }
                _ => {}
            }
        }
    }

    /// The span's service: a node, its entry-span counts and its last-seen.
    fn service_span(&mut self, span: &'a Span) {
        let counts = self.node(NodeKind::Service, span.service());
        if matches!(span.kind, SpanKind::Server | SpanKind::Consumer) {
            counts.requests += 1;
            if span.is_error() {
                counts.errors += 1;
            }
        }
        counts.last_seen = counts.last_seen.max(span.start_unix_nano);
    }

    /// A call from `service` to a dependency: the dependency's node counts
    /// it as a request, and the edge counts it as a call.
    fn call(&mut self, kind: NodeKind, name: &'a str, service: &'a str, erroring: bool) {
        let counts = self.node(kind, name);
        counts.requests += 1;
        counts.errors += u64::from(erroring);
        self.edge(service, name, erroring);
    }

    fn node(&mut self, kind: NodeKind, name: &'a str) -> &mut Counts {
        self.nodes.entry((kind, name)).or_default()
    }

    fn edge(&mut self, from: &'a str, to: &'a str, erroring: bool) {
        let counts = self.edges.entry((from, to)).or_default();
        counts.calls += 1;
        counts.errors += u64::from(erroring);
    }

    pub(crate) fn into_map(self) -> ServiceMap {
        let mut nodes: Vec<ServiceNode> = self
            .nodes
            .into_iter()
            .map(|((kind, id), counts)| ServiceNode {
                id: id.to_owned(),
                kind,
                requests: counts.requests,
                errors: counts.errors,
            })
            .collect();
        nodes.sort_unstable_by(|a, b| (&a.id, a.kind).cmp(&(&b.id, b.kind)));
        let mut edges: Vec<ServiceEdge> = self
            .edges
            .into_iter()
            .map(|((from, to), counts)| ServiceEdge {
                from: from.to_owned(),
                to: to.to_owned(),
                calls: counts.calls,
                errors: counts.errors,
            })
            .collect();
        edges.sort_unstable_by(|a, b| (&a.from, &a.to).cmp(&(&b.from, &b.to)));
        ServiceMap { nodes, edges }
    }

    /// The service nodes only, sorted by name.
    pub(crate) fn into_services(self) -> Vec<ServiceStats> {
        let mut services: Vec<ServiceStats> = self
            .nodes
            .into_iter()
            .filter(|((kind, _), _)| *kind == NodeKind::Service)
            .map(|((_, name), counts)| ServiceStats {
                name: name.to_owned(),
                requests: counts.requests,
                errors: counts.errors,
                last_seen_unix_nano: counts.last_seen,
            })
            .collect();
        services.sort_unstable_by(|a, b| a.name.cmp(&b.name));
        services
    }
}

/// The IDs of the spans in `entry` that have a direct child in another
/// service.
fn parents_with_remote_child(entry: &TraceEntry) -> HashSet<SpanId> {
    entry
        .spans()
        .iter()
        .filter_map(|span| {
            let parent = entry.span(span.parent_span_id?)?;
            (parent.service() != span.service()).then_some(parent.span_id)
        })
        .collect()
}

/// A non-empty string attribute. Other values are not names, so they count
/// as absent.
fn text_attr<'a>(span: &'a Span, key: &str) -> Option<&'a str> {
    match attr(&span.attributes, key) {
        Some(AttrValue::Str(text)) if !text.is_empty() => Some(text),
        _ => None,
    }
}

/// `db.system.name`, else the legacy `db.system`.
fn database_system(span: &Span) -> Option<&str> {
    text_attr(span, "db.system.name").or_else(|| text_attr(span, "db.system"))
}

fn messaging_system(span: &Span) -> Option<&str> {
    text_attr(span, "messaging.system")
}
