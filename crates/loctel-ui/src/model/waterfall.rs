//! The waterfall of the trace view: the spans of one trace as rows in tree
//! order, with the bar each one draws, what is folded away, what is selected
//! and what the find box matches. Plain Rust over the store's
//! [`TraceSnapshot`], with no GPUI in it: the view lays out the
//! [`WaterfallRow`]s it is given, and every rule of the waterfall (the order,
//! the fractions, the placeholder of design 4, the errors path, the colours)
//! is tested with plain assertions in `tests/model_waterfall.rs`.
//!
//! A [`Waterfall`] is built from a snapshot ([`Waterfall::new`]) and brought up
//! to date as more spans arrive ([`Waterfall::rebuild`]); what the user did
//! survives that, keyed by span ID: the folded spans, the selected span, the
//! errors filter. The tree is walked with an explicit stack, so a trace of
//! 100,000 spans (or a chain as deep as a trace can be) builds in a few
//! passes and cannot overflow the stack.
//!
//! The rows are the *visible* ones: the spans under a folded span are not in
//! [`Waterfall::rows`], and with the errors filter on neither is any span that
//! is not erroring and has no erroring span below it.

use std::collections::{HashMap, HashSet};

use loctel_model::{AttrValue, Span, SpanId, SpanKind, attr};
use loctel_store::TraceSnapshot;

use super::colors::ServiceColors;
use super::format;

/// What the placeholder row for spans whose parent has not arrived is called,
/// and what its [`WaterfallRow::peer_note`] says (design 4, "Order and
/// lateness").
pub const MISSING_PARENT: &str = "missing parent";

/// The thinnest a bar is, as a share of the trace: a span with no length, or
/// one too short to see, still shows a sliver.
pub const MIN_WIDTH_FRAC: f32 = 0.003;

/// The span ID of the placeholder row. All zeros is what OTLP uses for "no
/// span", and ingest drops such spans, so no real span has it.
const PLACEHOLDER_ID: SpanId = SpanId([0; 8]);

/// Marks "no index" in the tables that hold indexes.
const NONE: usize = usize::MAX;

/// Separates the pieces of a row's search text. A find text that holds it
/// matches nothing, so a match can never straddle two pieces.
const SEPARATOR: char = '\u{0}';

/// One line of the waterfall.
#[derive(Clone, Debug, PartialEq)]
pub struct WaterfallRow {
    /// The span, or the placeholder's ([`is_placeholder`](Self::is_placeholder)).
    pub span_id: SpanId,
    /// How deep the span is in the tree, 0 for a root. Stops at 65,535.
    pub depth: u16,
    /// Whether the span has children, shown or not.
    pub has_children: bool,
    /// Whether its children are folded away.
    pub collapsed: bool,
    pub name: String,
    pub service: String,
    /// The service's number in [`ServiceColors`], unreduced as in
    /// [`TraceRow`](super::trace_table::TraceRow): the view takes `% 5`.
    pub color: usize,
    /// The other side of a call, for the tertiary note after the service:
    /// the database a client span queries (`postgresql`), the broker it
    /// publishes to or consumes from (`rabbitmq`), else the host it calls
    /// (`card-gateway`). It is [`MISSING_PARENT`] on the placeholder row.
    pub peer_note: Option<String>,
    /// Where the bar starts, as a share of the trace (0 to 1).
    pub start_frac: f32,
    /// How wide the bar is, as a share of the trace: at least
    /// [`MIN_WIDTH_FRAC`], and never so far right that the bar leaves the
    /// track (`start_frac + width_frac <= 1`).
    pub width_frac: f32,
    /// The span's length as [`format::duration`] writes it.
    pub duration: String,
    /// The span's status is `Error`. An exception alone does not make it one.
    pub is_error: bool,
    /// Where the span's `exception` events happened, as shares of the trace
    /// (0 to 1), in the order of the events.
    pub exception_fracs: Vec<f32>,
}

impl WaterfallRow {
    /// Whether this is the placeholder that the spans whose parent has not
    /// arrived hang under, and not a span of the trace. It has the name and
    /// the note [`MISSING_PARENT`] (say it once), no service, no bar and no
    /// length, and [`color`](Self::color) 0 that the view should not draw as
    /// the first service's.
    pub fn is_placeholder(&self) -> bool {
        self.span_id == PLACEHOLDER_ID
    }
}

/// A span of the trace, or the placeholder, in the full tree (every row,
/// folded or not).
#[derive(Clone, Debug)]
struct Node {
    row: WaterfallRow,
    /// The position of the parent in the full tree (always before this one).
    parent: Option<usize>,
    /// One past the last descendant in the full tree: the subtree is
    /// `self..end`.
    end: usize,
    /// This span is erroring, or a span below it is.
    on_error_path: bool,
    /// The lower-cased name, service and attribute values, for find.
    search: Box<str>,
}

/// The waterfall of one trace.
#[derive(Clone, Debug)]
pub struct Waterfall {
    /// Every row of the tree, in order, whatever is folded.
    nodes: Vec<Node>,
    /// A span ID to its position in `nodes`.
    index: HashMap<SpanId, usize>,
    ruler: [String; 5],
    /// The spans folded away. Only spans with children are in it.
    collapsed: HashSet<SpanId>,
    selected: Option<SpanId>,
    errors_only: bool,
    /// The rows shown, in order.
    rows: Vec<WaterfallRow>,
    /// The position in `nodes` of each row shown (ascending).
    shown: Vec<usize>,
}

/// What the tree of a snapshot comes to.
struct Tree {
    nodes: Vec<Node>,
    index: HashMap<SpanId, usize>,
    ruler: [String; 5],
}

impl Waterfall {
    /// The waterfall of `snapshot`, with every span open and nothing
    /// selected.
    ///
    /// - **Order.** Depth first. The children of a span, and the roots, go by
    ///   start time and then span ID. A trace has one root, but a span with no
    ///   parent counts as a root, so another one is a second tree after the
    ///   first.
    /// - **Missing parents.** The spans whose parent is not in the trace (yet)
    ///   hang, one level down and in the same order, under one placeholder row
    ///   named [`MISSING_PARENT`] at depth 0 that comes after every root's
    ///   subtree. It is there only while there are such spans. A span that no
    ///   root or missing parent leads to (a parent loop, which a real exporter
    ///   does not produce) goes there too, so every span has a row.
    /// - **Colours.** `colors` is asked for every service in order of first
    ///   span, as the trace table does, so a service has one number in both.
    pub fn new(snapshot: &TraceSnapshot, colors: &mut ServiceColors) -> Waterfall {
        let tree = build(snapshot, colors);
        let mut waterfall = Waterfall {
            nodes: tree.nodes,
            index: tree.index,
            ruler: tree.ruler,
            collapsed: HashSet::new(),
            selected: None,
            errors_only: false,
            rows: Vec::new(),
            shown: Vec::new(),
        };
        waterfall.refresh();
        waterfall
    }

    /// Takes in a newer snapshot of the same trace (spans have arrived)
    /// without losing what the user did, which is kept by span ID:
    ///
    /// - the folded spans stay folded, so a late child of a folded span is
    ///   folded away with it;
    /// - the selected span stays selected, wherever its row has moved to (rows
    ///   come in above it, a placeholder goes away when the root arrives);
    /// - the errors filter stays.
    ///
    /// A span that is no longer in the trace is forgotten: it is not folded and
    /// not selected any more.
    pub fn rebuild(&mut self, snapshot: &TraceSnapshot, colors: &mut ServiceColors) {
        let tree = build(snapshot, colors);
        self.nodes = tree.nodes;
        self.index = tree.index;
        self.ruler = tree.ruler;
        let index = &self.index;
        self.collapsed.retain(|id| index.contains_key(id));
        if self.selected.is_some_and(|id| !index.contains_key(&id)) {
            self.selected = None;
        }
        self.refresh();
    }

    /// The rows shown, top to bottom: every span that is not under a folded
    /// span and, with the errors filter on, is on the way to an error.
    pub fn rows(&self) -> &[WaterfallRow] {
        &self.rows
    }

    /// Folds the children of `id` away, or opens them again. A span with no
    /// children and an ID that is not in the trace do nothing.
    pub fn toggle(&mut self, id: SpanId) {
        let Some(&at) = self.index.get(&id) else {
            return;
        };
        if !self.nodes[at].row.has_children {
            return;
        }
        if !self.collapsed.remove(&id) {
            self.collapsed.insert(id);
        }
        self.refresh();
    }

    /// Folds every span that has children: only the roots (and the
    /// placeholder) are left.
    pub fn collapse_all(&mut self) {
        self.collapsed = self
            .nodes
            .iter()
            .filter(|node| node.row.has_children)
            .map(|node| node.row.span_id)
            .collect();
        self.refresh();
    }

    /// Opens every span.
    pub fn expand_all(&mut self) {
        self.collapsed.clear();
        self.refresh();
    }

    /// Shows only the spans that are erroring and the spans above them (the
    /// way to each error), or everything. A trace with no erroring span shows
    /// no row at all. It combines with folding: a folded span hides its
    /// errors.
    pub fn set_errors_path_only(&mut self, on: bool) {
        if self.errors_only != on {
            self.errors_only = on;
            self.refresh();
        }
    }

    /// The rows (positions in [`rows`](Self::rows)) whose span has `text` in
    /// its name, its service or the value of any of its attributes (a number
    /// as the inspector writes it), ignoring case. Only the rows shown are
    /// looked at: a span under a folded one is not a match. Keys, the status
    /// message, events and the resource are not searched, and no text finds
    /// nothing.
    pub fn find(&self, text: &str) -> Vec<usize> {
        if text.is_empty() || text.contains(SEPARATOR) {
            return Vec::new();
        }
        let needle = text.to_lowercase();
        self.shown
            .iter()
            .enumerate()
            .filter(|&(_, &at)| self.nodes[at].search.contains(&needle))
            .map(|(row, _)| row)
            .collect()
    }

    /// Selects the span `id` and opens every span above it, so that its row
    /// is shown. An ID that is not in the trace changes nothing. The errors
    /// filter is not touched: a span that is not on the way to an error stays
    /// without a row until the filter is off.
    pub fn select(&mut self, id: SpanId) {
        let Some(&at) = self.index.get(&id) else {
            return;
        };
        self.selected = Some(id);
        let mut opened = false;
        let mut up = self.nodes[at].parent;
        while let Some(parent) = up {
            opened |= self.collapsed.remove(&self.nodes[parent].row.span_id);
            up = self.nodes[parent].parent;
        }
        if opened {
            self.refresh();
        }
    }

    /// The selected span. A new waterfall has none. It stays selected when a
    /// folded span hides it (see [`row_of`](Self::row_of)).
    pub fn selected(&self) -> Option<SpanId> {
        self.selected
    }

    /// Where the span `id` is among the rows shown, or `None` when it is not
    /// in the trace or no row shows it (under a folded span, or off the
    /// errors path).
    pub fn row_of(&self, id: SpanId) -> Option<usize> {
        let &at = self.index.get(&id)?;
        self.shown.binary_search(&at).ok()
    }

    /// The labels under the ruler: at 0, 25, 50, 75 and 100% of the trace's
    /// length, as [`format::duration`] writes them, except the first, which is
    /// `0`: `["0", "460 ms", "920 ms", "1.38 s", "1.84 s"]`. A trace with no
    /// length has `0` five times.
    pub fn ruler(&self) -> [String; 5] {
        self.ruler.clone()
    }

    /// Works out the rows shown from the tree and what is folded and
    /// filtered.
    fn refresh(&mut self) {
        self.rows.clear();
        self.shown.clear();
        let mut at = 0;
        while at < self.nodes.len() {
            let node = &self.nodes[at];
            if self.errors_only && !node.on_error_path {
                // Nothing below can be on the path either.
                at = node.end;
                continue;
            }
            let folded = node.row.has_children
                && !self.collapsed.is_empty()
                && self.collapsed.contains(&node.row.span_id);
            let mut row = node.row.clone();
            row.collapsed = folded;
            self.rows.push(row);
            self.shown.push(at);
            at = if folded { node.end } else { at + 1 };
        }
    }
}

/// The span to select when a trace opens: the one that explains the failure
/// (`failed_in`), else the root. `None` for a trace with neither (no span has
/// failed and the root has not arrived).
pub fn initial_selection(snapshot: &TraceSnapshot) -> Option<SpanId> {
    snapshot
        .failed_in
        .as_ref()
        .map(|failed| failed.span_id)
        .or(snapshot.root)
}

/// Where the trace starts and ends: the earliest span start and the latest span
/// end, each end counted no earlier than its own span's start (the store's
/// rule, so the length is the one of the trace table's row). `(0, 0)` for no
/// span.
pub(crate) fn extent(spans: &[Span]) -> (u64, u64) {
    if spans.is_empty() {
        return (0, 0);
    }
    let mut start = u64::MAX;
    let mut end = 0;
    for span in spans {
        start = start.min(span.start_unix_nano);
        end = end.max(span.end_unix_nano.max(span.start_unix_nano));
    }
    (start, end)
}

/// One slot of the walk: a span (or the placeholder, `span == NONE`), how deep
/// it is and the slot of its parent (`NONE` at the top).
struct Slot {
    span: usize,
    depth: u16,
    parent: usize,
}

/// The depth-first walk, with its own stack.
struct Walk<'a> {
    /// `kids[first[i]..first[i + 1]]` are the children of span `i`, in order.
    first: &'a [usize],
    kids: &'a [usize],
    visited: Vec<bool>,
    slots: Vec<Slot>,
    stack: Vec<(usize, u16, usize)>,
}

impl Walk<'_> {
    /// Visits `top` and everything below it that has not been visited, in
    /// pre-order.
    fn walk(&mut self, top: usize, depth: u16, parent: usize) {
        self.stack.push((top, depth, parent));
        while let Some((span, depth, parent)) = self.stack.pop() {
            if std::mem::replace(&mut self.visited[span], true) {
                continue;
            }
            let here = self.slots.len();
            self.slots.push(Slot {
                span,
                depth,
                parent,
            });
            // Pushed last to first, so the first child is popped first.
            let below = depth.saturating_add(1);
            for &child in self.kids[self.first[span]..self.first[span + 1]]
                .iter()
                .rev()
            {
                if !self.visited[child] {
                    self.stack.push((child, below, here));
                }
            }
        }
    }
}

fn build(snapshot: &TraceSnapshot, colors: &mut ServiceColors) -> Tree {
    let spans = &snapshot.spans;
    let count = spans.len();
    let (trace_start, trace_end) = extent(spans);
    let total = trace_end - trace_start;

    // The order everything goes in: by start, then span ID. The store hands
    // the spans over like that, so this usually costs one look.
    let mut order: Vec<usize> = (0..count).collect();
    if !spans.is_sorted_by_key(|span| (span.start_unix_nano, span.span_id)) {
        order.sort_by_key(|&i| (spans[i].start_unix_nano, spans[i].span_id));
    }

    // A service's colour is its place in the order of first spans.
    let mut color_of = vec![0; count];
    for &i in &order {
        color_of[i] = colors.index(spans[i].service());
    }

    // Who is whose parent. A span with no parent, or its own, is a root; one
    // whose parent is not in the trace is an orphan.
    let mut position: HashMap<SpanId, usize> = HashMap::with_capacity(count);
    for (i, span) in spans.iter().enumerate() {
        position.entry(span.span_id).or_insert(i);
    }
    let mut parent = vec![NONE; count];
    let mut roots = Vec::new();
    let mut orphans = Vec::new();
    for &i in &order {
        match spans[i].parent_span_id {
            None => roots.push(i),
            Some(id) if id == spans[i].span_id => roots.push(i),
            Some(id) => match position.get(&id) {
                Some(&p) if p != i => parent[i] = p,
                _ => orphans.push(i),
            },
        }
    }

    // The children of every span, in `order`, in one array (a row of
    // `kids` per span, `first` says where each starts).
    let mut first = vec![0usize; count + 1];
    for &p in &parent {
        if p != NONE {
            first[p + 1] += 1;
        }
    }
    for i in 0..count {
        first[i + 1] += first[i];
    }
    let mut cursor = first.clone();
    let mut kids = vec![0usize; first[count]];
    for &i in &order {
        let p = parent[i];
        if p != NONE {
            kids[cursor[p]] = i;
            cursor[p] += 1;
        }
    }

    // The walk: the roots, then the placeholder (once something needs it) with
    // the orphans and last anything the walk did not get to.
    let mut walk = Walk {
        first: &first,
        kids: &kids,
        visited: vec![false; count],
        slots: Vec::with_capacity(count + 1),
        stack: Vec::new(),
    };
    for &root in &roots {
        walk.walk(root, 0, NONE);
    }
    let mut placeholder = NONE;
    for top in orphans.into_iter().chain(order.iter().copied()) {
        if walk.visited[top] {
            continue;
        }
        if placeholder == NONE {
            placeholder = walk.slots.len();
            walk.slots.push(Slot {
                span: NONE,
                depth: 0,
                parent: NONE,
            });
        }
        walk.walk(top, 1, placeholder);
    }
    let slots = walk.slots;

    // The rows.
    let mut nodes: Vec<Node> = Vec::with_capacity(slots.len());
    for slot in &slots {
        let parent = (slot.parent != NONE).then_some(slot.parent);
        let node = if slot.span == NONE {
            placeholder_node(parent)
        } else {
            let i = slot.span;
            let span = &spans[i];
            let has_children = first[i + 1] > first[i];
            Node {
                row: span_row(
                    span,
                    slot.depth,
                    color_of[i],
                    has_children,
                    trace_start,
                    total,
                ),
                parent,
                end: 0,
                on_error_path: false,
                search: search_text(span),
            }
        };
        nodes.push(node);
    }

    // Parents come before their children, so one pass from the back sums up
    // each subtree and carries an error up to every span above it.
    let mut size = vec![1usize; nodes.len()];
    for at in (0..nodes.len()).rev() {
        if nodes[at].row.is_error {
            nodes[at].on_error_path = true;
        }
        if let Some(up) = nodes[at].parent {
            size[up] += size[at];
            if nodes[at].on_error_path {
                nodes[up].on_error_path = true;
            }
        }
    }
    for (at, node) in nodes.iter_mut().enumerate() {
        node.end = at + size[at];
    }

    let mut index = HashMap::with_capacity(nodes.len());
    for (at, node) in nodes.iter().enumerate() {
        index.entry(node.row.span_id).or_insert(at);
    }

    Tree {
        nodes,
        index,
        ruler: ruler(total),
    }
}

/// The row of the placeholder: it says what is missing and has nothing to
/// draw.
fn placeholder_node(parent: Option<usize>) -> Node {
    Node {
        row: WaterfallRow {
            span_id: PLACEHOLDER_ID,
            depth: 0,
            has_children: true,
            collapsed: false,
            name: MISSING_PARENT.to_owned(),
            service: String::new(),
            color: 0,
            peer_note: Some(MISSING_PARENT.to_owned()),
            start_frac: 0.0,
            width_frac: 0.0,
            duration: String::new(),
            is_error: false,
            exception_fracs: Vec::new(),
        },
        parent,
        end: 0,
        on_error_path: false,
        search: MISSING_PARENT.into(),
    }
}

fn span_row(
    span: &Span,
    depth: u16,
    color: usize,
    has_children: bool,
    trace_start: u64,
    total: u64,
) -> WaterfallRow {
    let offset = span.start_unix_nano.saturating_sub(trace_start);
    let (start_frac, width_frac) = bar(offset, span.duration_nanos(), total);
    WaterfallRow {
        span_id: span.span_id,
        depth,
        has_children,
        collapsed: false,
        name: span.name.clone(),
        service: span.service().to_owned(),
        color,
        peer_note: peer_note(span).map(str::to_owned),
        start_frac,
        width_frac,
        duration: format::duration(span.duration_nanos()),
        is_error: span.is_error(),
        exception_fracs: span
            .events
            .iter()
            .filter(|event| event.name == "exception")
            .map(|event| {
                let at = event.time_unix_nano.saturating_sub(trace_start);
                share(at, total)
            })
            .collect(),
    }
}

/// `part` as a share of `total`, from 0 to 1 (0 when there is no total).
fn share(part: u64, total: u64) -> f32 {
    if total == 0 {
        return 0.0;
    }
    ((part as f64 / total as f64) as f32).clamp(0.0, 1.0)
}

/// Where a bar starts and how wide it is, as shares of the trace: at least
/// [`MIN_WIDTH_FRAC`] wide, and moved left if it would leave the track. With no
/// length to the trace, a sliver at the start.
fn bar(offset: u64, duration: u64, total: u64) -> (f32, f32) {
    if total == 0 {
        return (0.0, MIN_WIDTH_FRAC);
    }
    let width = share(duration, total).max(MIN_WIDTH_FRAC);
    let start = share(offset, total).min(1.0 - width);
    (start, width)
}

fn ruler(total: u64) -> [String; 5] {
    if total == 0 {
        return ["0", "0", "0", "0", "0"].map(str::to_owned);
    }
    // In u128, so that three quarters of any length does not overflow.
    let at = |quarters: u128| format::duration((u128::from(total) * quarters / 4) as u64);
    ["0".to_owned(), at(1), at(2), at(3), at(4)]
}

/// The note after the service: the other side of the call, the way the
/// service map names it. A client span names its database, else its broker,
/// else the host it calls; a producer its broker, else the host; a consumer
/// its broker. Spans that are the service's own work have none.
fn peer_note(span: &Span) -> Option<&str> {
    match span.kind {
        SpanKind::Client => database_system(span)
            .or_else(|| messaging_system(span))
            .or_else(|| span.peer()),
        SpanKind::Producer => messaging_system(span).or_else(|| span.peer()),
        SpanKind::Consumer => messaging_system(span),
        SpanKind::Unspecified | SpanKind::Internal | SpanKind::Server => None,
    }
}

/// `db.system.name`, else the legacy `db.system`.
fn database_system(span: &Span) -> Option<&str> {
    text_attr(span, "db.system.name").or_else(|| text_attr(span, "db.system"))
}

fn messaging_system(span: &Span) -> Option<&str> {
    text_attr(span, "messaging.system")
}

/// The attribute `key`, when it is a string that is not empty.
fn text_attr<'a>(span: &'a Span, key: &str) -> Option<&'a str> {
    match attr(&span.attributes, key) {
        Some(AttrValue::Str(text)) if !text.is_empty() => Some(text),
        _ => None,
    }
}

/// What find looks in for a span: its name, its service and the value of each
/// attribute, lower-cased and joined by [`SEPARATOR`].
fn search_text(span: &Span) -> Box<str> {
    let mut text = String::new();
    push_lower(&mut text, &span.name);
    text.push(SEPARATOR);
    push_lower(&mut text, span.service());
    for (_, value) in &span.attributes {
        text.push(SEPARATOR);
        match value {
            AttrValue::Str(value) => push_lower(&mut text, value),
            other => push_lower(&mut text, &other.to_display_string()),
        }
    }
    text.into_boxed_str()
}

/// Adds `s` to `out`, lower-cased (a plain copy and a pass over the bytes for
/// ASCII, which nearly all of it is).
fn push_lower(out: &mut String, s: &str) {
    if s.is_ascii() {
        let from = out.len();
        out.push_str(s);
        out[from..].make_ascii_lowercase();
    } else {
        out.push_str(&s.to_lowercase());
    }
}
