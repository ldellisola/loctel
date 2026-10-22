//! Where each node of the service map goes (design 9.2, M3 plan Task 2).
//!
//! [`layout`] turns the store's [`ServiceMap`] into [`MapLayout`]: every node
//! with its place, size, colour, subtitle and failing flag, and every edge with
//! its counts and label, ready for the view to paint. It is plain arithmetic
//! over plain values, so the topology of the checkout, the loops of a mesh
//! where services call each other and the way the picture holds still while
//! services appear are all tested without a window.
//!
//! The rules, in the order [`layout`] applies them:
//!
//! 1. **Columns** are the longest path from the sources (nodes nobody calls).
//!    A loop is broken by ignoring the edges that close it: a depth-first
//!    search starts at the sources and then at every node it has not reached,
//!    in id order, and an edge into a node that is still being searched is a
//!    back edge. What remains has no loop, so the columns exist.
//! 2. **Rows** within a column go by the middle (barycentre) of the rows of
//!    the nodes that call the node, so lines cross as little as the greedy
//!    order allows. Ties go by id, then kind.
//! 3. **Places**: `x = 40 + column x (176 + 96)`, `y = 32 + row x (64 + 32)`.
//! 4. **Stability**: a [`LayoutMemory`] holds the column and row of every node
//!    drawn. A node in it keeps both. A new node goes to the first free row of
//!    the column it computed, so a new service never pushes the others around.
//!    A node missing from the map keeps its place in the memory for 3
//!    refreshes (nobody takes its row, and it returns to it) and is forgotten
//!    at the fourth, so a gap of a moment does not make the map jump.
//! 5. **Rates** are counts over the window: `count / window_secs`.
//! 6. **Routes** ([`route_edges`]) say where each edge is drawn. An edge to
//!    the next column is one curve. An edge that skips a column, or turns
//!    back, never goes through a node: it leaves along a *lane*, a horizontal
//!    line in the 32 px gap between two rows (or above the first row, or below
//!    the last), travels along it past the columns in between and comes down
//!    to the node it enters. Nodes only ever fill the rows, so a lane is
//!    free of them in every column.
//!
//! `docs/areas/ui.md` ("Map layout") has the details and the reasons.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};

use loctel_store::{NodeKind, ServiceEdge, ServiceMap, ServiceNode};

use super::colors::ServiceColors;
use super::format;

/// A node is 176 by 64 px.
const NODE_W: f32 = 176.0;
const NODE_H: f32 = 64.0;
/// The gap between two columns, and between two rows.
const COLUMN_GAP: f32 = 96.0;
const ROW_GAP: f32 = 32.0;
/// The margins round the picture: the left and right, and the top and bottom.
const MARGIN_X: f32 = 40.0;
const MARGIN_Y: f32 = 32.0;
/// How many refreshes in a row a node can be missing from the map and still
/// have its place kept. The next one forgets it.
const KEPT_REFRESHES: u8 = 3;
/// The distance between the start of one row and the next.
const ROW_PITCH: f32 = NODE_H + ROW_GAP;
/// The distance between the left edge of one column and the next.
const COLUMN_PITCH: f32 = NODE_W + COLUMN_GAP;
/// An edge leaves and enters a node level: the control points of its curves
/// are at least this far out, so a curve that has little room still has a
/// shape.
const MIN_BEND: f32 = 48.0;
/// How far an edge that turns back bulges out of the node it leaves and the
/// node it enters. The curve reaches three quarters of it, which is inside the
/// 40 px margin of the picture.
const LOOP_BULGE: f32 = 40.0;
/// Where the edges that share a lane run, as distances from its middle, in
/// the order the edges take them. The lane is 32 px high between the rows, so
/// the farthest is 8 px from the middle and 8 px clear of the nodes.
const LANE_OFFSETS: [f32; 5] = [0.0, -5.0, 5.0, -8.0, 8.0];

/// A node of the map, placed.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedNode {
    /// The node's id: the service name, database system, queue or host.
    pub id: String,
    pub kind: NodeKind,
    /// The service's first-seen index in [`ServiceColors`] (the palette slot
    /// is `color % 5`). `None` for a database, a queue or an external host,
    /// which have no colour of their own.
    pub color: Option<usize>,
    /// The top left corner, in px from the top left of the picture.
    pub x: f32,
    pub y: f32,
    /// Always 176 by 64.
    pub w: f32,
    pub h: f32,
    /// The second line of the node: `84 req/s · 2.1% err` for a service,
    /// `database · 140 q/s`, `queue · 9 msg/s` or `external · 3 req/s`.
    pub subtitle: String,
    /// Whether the node counted any error.
    pub failing: bool,
}

/// An edge of the map, with the labels the view draws on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlacedEdge {
    /// The ids of the nodes it joins, both placed nodes of the same layout.
    pub from: String,
    pub to: String,
    pub calls: u64,
    pub errors: u64,
    /// Whether any call along the edge errored.
    pub failing: bool,
    /// The call rate (`84 req/s`), only on the edges out of a node of the
    /// first column: one label per entry point, not one on every line.
    pub label: Option<String>,
}

/// The whole map, placed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MapLayout {
    /// In the order of the picture: by column, then by row.
    pub nodes: Vec<PlacedNode>,
    /// In the order of the map: by `(from, to)`.
    pub edges: Vec<PlacedEdge>,
    /// The size of the picture: every node and the margins round them. Both
    /// are 0 for a map with no node.
    pub width: f32,
    pub height: f32,
}

/// Where the nodes were put, kept from one refresh to the next so that they
/// stay there. One lives as long as the map is on screen. Start a fresh one
/// (`LayoutMemory::default()`) to lay the map out again from nothing.
#[derive(Clone, Debug, Default)]
pub struct LayoutMemory {
    places: HashMap<(NodeKind, String), Place>,
}

/// A remembered node: its column and row, and for how many refreshes in a row
/// the map has not had it.
#[derive(Clone, Copy, Debug)]
struct Place {
    layer: usize,
    slot: usize,
    missing: u8,
}

/// Lays `map` out. `window_secs` is the length of the window the map was
/// counted over, for the rates; a value that is not a positive length of time
/// gives zero rates. `colors` is the session's service colours, shared with
/// the trace table: a service the session has not met is numbered here, in the
/// order of the picture. `memory` is updated with what was placed.
///
/// Nodes are taken in the order of their `(id, kind)` and edges of their
/// `(from, to)`, whatever order `map` has them in, so the same map gives the
/// same layout. An edge's ends are ids, and ids are unique within a kind only:
/// an end means the first node with that id (a service before a database, a
/// queue, an external host). An edge to an id that is no node is left out.
pub fn layout(
    map: &ServiceMap,
    window_secs: f64,
    colors: &mut ServiceColors,
    memory: &mut LayoutMemory,
) -> MapLayout {
    let window_secs = if window_secs.is_finite() && window_secs > 0.0 {
        window_secs
    } else {
        0.0
    };

    let mut nodes: Vec<&ServiceNode> = map.nodes.iter().collect();
    nodes.sort_by(|a, b| (&a.id, a.kind).cmp(&(&b.id, b.kind)));
    nodes.dedup_by(|a, b| a.id == b.id && a.kind == b.kind);
    let links = resolve_edges(map, &nodes);

    let columns = Columns::of(nodes.len(), &links);
    let placed = place(&nodes, &columns, memory);

    // Nodes in the order of the picture.
    let mut order: Vec<usize> = (0..nodes.len()).collect();
    order.sort_by_key(|&i| placed[i]);

    let placed_nodes: Vec<PlacedNode> = order
        .iter()
        .map(|&i| {
            let node = nodes[i];
            let (layer, slot) = placed[i];
            PlacedNode {
                id: node.id.clone(),
                kind: node.kind,
                color: (node.kind == NodeKind::Service).then(|| colors.index(&node.id)),
                x: MARGIN_X + layer as f32 * (NODE_W + COLUMN_GAP),
                y: MARGIN_Y + slot as f32 * (NODE_H + ROW_GAP),
                w: NODE_W,
                h: NODE_H,
                subtitle: subtitle(node, window_secs),
                failing: node.errors > 0,
            }
        })
        .collect();

    let placed_edges = links
        .iter()
        .map(|link| PlacedEdge {
            from: nodes[link.from].id.clone(),
            to: nodes[link.to].id.clone(),
            calls: link.edge.calls,
            errors: link.edge.errors,
            failing: link.edge.errors > 0,
            label: (placed[link.from].0 == 0)
                .then(|| format::rate(per_second(link.edge.calls, window_secs), "req/s")),
        })
        .collect();

    // The picture is as big as what is drawn: a node that is only remembered
    // does not count.
    let (columns_used, rows_used) = placed
        .iter()
        .fold((0, 0), |(columns, rows), &(layer, slot)| {
            (columns.max(layer + 1), rows.max(slot + 1))
        });
    let (width, height) = if nodes.is_empty() {
        (0.0, 0.0)
    } else {
        (
            2.0 * MARGIN_X + columns_used as f32 * NODE_W + (columns_used - 1) as f32 * COLUMN_GAP,
            2.0 * MARGIN_Y + rows_used as f32 * NODE_H + (rows_used - 1) as f32 * ROW_GAP,
        )
    };

    MapLayout {
        nodes: placed_nodes,
        edges: placed_edges,
        width,
        height,
    }
}

/// An edge of the map with its ends found among the nodes.
struct Link<'a> {
    /// Indexes into the sorted node list.
    from: usize,
    to: usize,
    edge: &'a ServiceEdge,
}

/// The edges of `map` whose ends are both nodes, sorted by `(from, to)`.
fn resolve_edges<'a>(map: &'a ServiceMap, nodes: &[&ServiceNode]) -> Vec<Link<'a>> {
    // Nodes are sorted, so the first of an id is the one that comes first in
    // kind order.
    let mut first_of_id: HashMap<&str, usize> = HashMap::new();
    for (index, node) in nodes.iter().enumerate() {
        first_of_id.entry(node.id.as_str()).or_insert(index);
    }
    let mut edges: Vec<&ServiceEdge> = map.edges.iter().collect();
    edges.sort_by(|a, b| (&a.from, &a.to).cmp(&(&b.from, &b.to)));
    edges
        .into_iter()
        .filter_map(|edge| {
            Some(Link {
                from: *first_of_id.get(edge.from.as_str())?,
                to: *first_of_id.get(edge.to.as_str())?,
                edge,
            })
        })
        .collect()
}

/// The columns of a graph with its loops broken.
struct Columns {
    /// The column of each node, by longest path from the sources.
    layer: Vec<usize>,
    /// The nodes that call each node, along the edges that were kept.
    callers: Vec<Vec<usize>>,
}

impl Columns {
    fn of(count: usize, links: &[Link<'_>]) -> Columns {
        // Out-edges in node order (the nodes are in id order), without the
        // calls of a node to itself, which say nothing about columns.
        let mut out: Vec<Vec<usize>> = vec![Vec::new(); count];
        let mut called = vec![false; count];
        for link in links.iter().filter(|link| link.from != link.to) {
            out[link.from].push(link.to);
            called[link.to] = true;
        }
        for targets in &mut out {
            targets.sort_unstable();
            targets.dedup();
        }

        // Depth first, from the sources and then from whatever is left (a
        // loop that nothing outside calls). An edge into a node that is still
        // open is a back edge: it closes a loop and is left out. The search
        // keeps its own stack, so a chain of any length is fine.
        #[derive(Clone, Copy, PartialEq)]
        enum Mark {
            Unseen,
            Open,
            Done,
        }
        let mut mark = vec![Mark::Unseen; count];
        let mut callers: Vec<Vec<usize>> = vec![Vec::new(); count];
        // Finished nodes in the order they finished.
        let mut finished = Vec::with_capacity(count);
        let sources = (0..count).filter(|&node| !called[node]);
        for root in sources.chain(0..count) {
            if mark[root] != Mark::Unseen {
                continue;
            }
            mark[root] = Mark::Open;
            let mut stack = vec![(root, 0usize)];
            while let Some(top) = stack.last_mut() {
                let node = top.0;
                let Some(&next) = out[node].get(top.1) else {
                    mark[node] = Mark::Done;
                    finished.push(node);
                    stack.pop();
                    continue;
                };
                top.1 += 1;
                match mark[next] {
                    Mark::Unseen => {
                        callers[next].push(node);
                        mark[next] = Mark::Open;
                        stack.push((next, 0));
                    }
                    // A forward or cross edge: the loop-free graph keeps it.
                    Mark::Done => callers[next].push(node),
                    // A back edge.
                    Mark::Open => {}
                }
            }
        }

        // The kept edges all run from a later finisher to an earlier one, so
        // the reverse of the finishing order has every caller before the
        // nodes it calls: one pass gives the longest paths.
        let mut layer = vec![0usize; count];
        for &node in finished.iter().rev() {
            layer[node] = callers[node]
                .iter()
                .map(|&caller| layer[caller] + 1)
                .max()
                .unwrap_or(0);
        }
        Columns { layer, callers }
    }
}

/// The `(column, row)` of every node, after keeping what `memory` holds,
/// placing the rest, and writing it all back to `memory`.
fn place(
    nodes: &[&ServiceNode],
    columns: &Columns,
    memory: &mut LayoutMemory,
) -> Vec<(usize, usize)> {
    // A node the map does not have this time has missed one more refresh in a
    // row, and past the limit it is forgotten. (The nodes it does have are
    // written back below with their count at zero.)
    let present: HashSet<(NodeKind, &str)> = nodes
        .iter()
        .map(|node| (node.kind, node.id.as_str()))
        .collect();
    memory.places.retain(|(kind, id), place| {
        if present.contains(&(*kind, id.as_str())) {
            return true;
        }
        place.missing += 1;
        place.missing <= KEPT_REFRESHES
    });

    // What is taken: the places of every remembered node, here or missing.
    let mut taken: HashSet<(usize, usize)> = memory
        .places
        .values()
        .map(|place| (place.layer, place.slot))
        .collect();

    let mut placed: Vec<Option<(usize, usize)>> = nodes
        .iter()
        .map(|node| {
            memory
                .places
                .get(&(node.kind, node.id.clone()))
                .map(|place| (place.layer, place.slot))
        })
        .collect();

    // The new nodes, by the column they computed.
    let mut fresh: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (index, place) in placed.iter().enumerate() {
        if place.is_none() {
            fresh.entry(columns.layer[index]).or_default().push(index);
        }
    }

    // Column by column from the left, so that the callers of a new node are
    // placed before it (a caller is in an earlier column, or remembered). The
    // first free row of a column only ever moves down, so each column is
    // walked once.
    let mut first_free: HashMap<usize, usize> = HashMap::new();
    for (layer, group) in fresh {
        let middle = |node: usize| -> Option<f64> {
            let rows: Vec<usize> = columns.callers[node]
                .iter()
                .filter_map(|&caller| placed[caller].map(|(_, slot)| slot))
                .collect();
            (!rows.is_empty()).then(|| rows.iter().sum::<usize>() as f64 / rows.len() as f64)
        };
        // Sort by the middle of the callers' rows (a node nobody calls has
        // none, and goes first), then by id and kind: the order of `nodes`.
        let mut keyed: Vec<(Option<f64>, usize)> =
            group.iter().map(|&node| (middle(node), node)).collect();
        keyed.sort_by(|(a, a_node), (b, b_node)| {
            match (a, b) {
                (Some(a), Some(b)) => a.total_cmp(b),
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Less,
                (Some(_), None) => Ordering::Greater,
            }
            .then(a_node.cmp(b_node))
        });

        let cursor = first_free.entry(layer).or_insert(0);
        for (_, node) in keyed {
            while taken.contains(&(layer, *cursor)) {
                *cursor += 1;
            }
            placed[node] = Some((layer, *cursor));
            taken.insert((layer, *cursor));
        }
    }

    let placed: Vec<(usize, usize)> = placed
        .into_iter()
        .map(|place| place.expect("every node is placed"))
        .collect();
    for (node, &(layer, slot)) in nodes.iter().zip(&placed) {
        memory.places.insert(
            (node.kind, node.id.clone()),
            Place {
                layer,
                slot,
                missing: 0,
            },
        );
    }
    placed
}

/// `count` per second over the window, 0 when there is no window.
fn per_second(count: u64, window_secs: f64) -> f64 {
    if window_secs > 0.0 {
        count as f64 / window_secs
    } else {
        0.0
    }
}

/// The second line of a node.
fn subtitle(node: &ServiceNode, window_secs: f64) -> String {
    let per_sec = per_second(node.requests, window_secs);
    match node.kind {
        NodeKind::Service => format!(
            "{} \u{b7} {}",
            format::rate(per_sec, "req/s"),
            format::error_pct(node.errors, node.requests)
        ),
        NodeKind::Database => format!("database \u{b7} {}", format::rate(per_sec, "q/s")),
        NodeKind::Queue => format!("queue \u{b7} {}", format::rate(per_sec, "msg/s")),
        NodeKind::External => format!("external \u{b7} {}", format::rate(per_sec, "req/s")),
    }
}

// --- routes -------------------------------------------------------------------

/// A point of the picture: `(x, y)` in px from its top left corner.
pub type PicturePoint = (f32, f32);

/// One piece of an edge's path, drawn from where the last piece ended.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RouteSegment {
    /// A straight line to the point.
    Line(PicturePoint),
    /// A cubic Bezier curve to `to`, with the control points `c1` (after the
    /// start) and `c2` (before `to`).
    Cubic {
        c1: PicturePoint,
        c2: PicturePoint,
        to: PicturePoint,
    },
}

impl RouteSegment {
    fn end(self) -> PicturePoint {
        match self {
            RouteSegment::Line(to) | RouteSegment::Cubic { to, .. } => to,
        }
    }
}

/// Where an edge is drawn: the right-edge middle of the node that calls, the
/// left-edge middle of the node that is called, and what is between.
#[derive(Clone, Debug, PartialEq)]
pub struct EdgeRoute {
    pub start: PicturePoint,
    /// At least one. The last one is a curve that arrives level, heading
    /// right, so the arrowhead points into the node.
    pub segments: Vec<RouteSegment>,
}

impl EdgeRoute {
    /// Where the route ends: the callee's left-edge middle.
    pub fn end(&self) -> PicturePoint {
        self.segments.last().map_or(self.start, |last| last.end())
    }

    /// The direction the route arrives in, as a unit vector: along the last
    /// control point (or line) into the end. `(1, 0)` when it has no length.
    pub fn arrival(&self) -> (f32, f32) {
        let end = self.end();
        let before = match self.segments.as_slice() {
            [] => self.start,
            [.., RouteSegment::Cubic { c2, .. }] => *c2,
            [rest @ .., RouteSegment::Line(_)] => rest.last().map_or(self.start, |s| s.end()),
        };
        let (dx, dy) = (end.0 - before.0, end.1 - before.1);
        let length = dx.hypot(dy);
        if length > f32::EPSILON {
            (dx / length, dy / length)
        } else {
            (1.0, 0.0)
        }
    }

    /// Where the rate label of the edge goes (above it): the middle of the
    /// first piece, which is in the gap right of the node that calls.
    pub fn label_anchor(&self) -> PicturePoint {
        match self.segments.first() {
            Some(RouteSegment::Cubic { c1, c2, to }) => (
                (self.start.0 + 3.0 * c1.0 + 3.0 * c2.0 + to.0) / 8.0,
                (self.start.1 + 3.0 * c1.1 + 3.0 * c2.1 + to.1) / 8.0,
            ),
            Some(RouteSegment::Line(to)) => {
                ((self.start.0 + to.0) / 2.0, (self.start.1 + to.1) / 2.0)
            }
            None => self.start,
        }
    }

    /// The route as points: the start, then `per_segment` points along each
    /// piece, the last of the piece included. For checking where a route goes.
    pub fn points(&self, per_segment: usize) -> Vec<PicturePoint> {
        let per_segment = per_segment.max(1);
        let mut points = vec![self.start];
        let mut from = self.start;
        for segment in &self.segments {
            for step in 1..=per_segment {
                let t = step as f32 / per_segment as f32;
                points.push(match *segment {
                    RouteSegment::Line(to) => {
                        (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t)
                    }
                    RouteSegment::Cubic { c1, c2, to } => {
                        let u = 1.0 - t;
                        let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                        (
                            a * from.0 + b * c1.0 + c * c2.0 + d * to.0,
                            a * from.1 + b * c1.1 + c * c2.1 + d * to.1,
                        )
                    }
                });
            }
            from = segment.end();
        }
        points
    }
}

/// The routes of the edges of `layout`, one per edge in the order of
/// `layout.edges`: `None` for an edge that has nothing to draw (a call of a node
/// to itself).
///
/// - **To the next column**: one S-shaped curve, level at both ends.
/// - **Over one column or more**: a curve into a lane, a straight line along
///   the lane and a curve out of it to the node. The lane is the gap between two
///   rows, the one nearest the middle between the two ends, so the edge does not
///   wander. It is only ever in a gap, so it passes between the nodes of the
///   columns in between and under none of them.
/// - **To the same column or back**: a bulge out of the right of the caller, a
///   lane to the left past the callee and a bulge into its left. Nodes can sit
///   in such a pair because a node keeps its place while the calls change.
///
/// Edges that share a lane are spread over it ([`LANE_OFFSETS`]), in the order
/// of `layout.edges`.
pub fn route_edges(layout: &MapLayout) -> Vec<Option<EdgeRoute>> {
    // An edge's end is the first node with that id, and the kinds sort
    // service first.
    let mut by_id: HashMap<&str, &PlacedNode> = HashMap::new();
    for node in &layout.nodes {
        by_id
            .entry(node.id.as_str())
            .and_modify(|first| {
                if node.kind < first.kind {
                    *first = node;
                }
            })
            .or_insert(node);
    }
    let last_row = layout
        .nodes
        .iter()
        .map(|node| ((node.y - MARGIN_Y) / ROW_PITCH).round() as i32)
        .max()
        .unwrap_or(0);

    // How many edges have taken a lane already, by the gap it is in.
    let mut taken: HashMap<i32, usize> = HashMap::new();
    layout
        .edges
        .iter()
        .map(|edge| {
            let from = by_id.get(edge.from.as_str())?;
            let to = by_id.get(edge.to.as_str())?;
            if std::ptr::eq(*from, *to) {
                return None;
            }
            let start = (from.x + from.w, from.y + from.h / 2.0);
            let end = (to.x, to.y + to.h / 2.0);
            let columns = ((to.x - from.x) / COLUMN_PITCH).round() as i32;
            if columns == 1 {
                return Some(EdgeRoute {
                    start,
                    segments: vec![s_curve(start, end)],
                });
            }

            let gap = lane_gap(start.1, end.1, last_row);
            let used = taken.entry(gap).or_insert(0);
            let y = lane_y(gap) + LANE_OFFSETS[*used % LANE_OFFSETS.len()];
            *used += 1;
            let segments = if columns > 1 {
                // The lane starts where the next column does and stops where
                // the one before the callee's ends.
                let lane_start = (start.0 + COLUMN_GAP, y);
                let lane_end = (end.0 - COLUMN_GAP, y);
                vec![
                    s_curve(start, lane_start),
                    RouteSegment::Line(lane_end),
                    s_curve(lane_end, end),
                ]
            } else {
                vec![
                    RouteSegment::Cubic {
                        c1: (start.0 + LOOP_BULGE, start.1),
                        c2: (start.0 + LOOP_BULGE, y),
                        to: (start.0, y),
                    },
                    RouteSegment::Line((end.0, y)),
                    RouteSegment::Cubic {
                        c1: (end.0 - LOOP_BULGE, y),
                        c2: (end.0 - LOOP_BULGE, end.1),
                        to: end,
                    },
                ]
            };
            Some(EdgeRoute { start, segments })
        })
        .collect()
}

/// A curve from `from` to `to` that leaves and arrives level (heading right).
fn s_curve(from: PicturePoint, to: PicturePoint) -> RouteSegment {
    let bend = ((to.0 - from.0) / 2.0).max(MIN_BEND);
    RouteSegment::Cubic {
        c1: (from.0 + bend, from.1),
        c2: (to.0 - bend, to.1),
        to,
    }
}

/// The y of the middle of the gap under row `gap` (-1 is the margin above the
/// first row).
fn lane_y(gap: i32) -> f32 {
    MARGIN_Y + NODE_H + ROW_GAP / 2.0 + gap as f32 * ROW_PITCH
}

/// The gap, from -1 (above the first row) to `last_row` (under the last), whose
/// middle is nearest the middle of `y_from` and `y_to`; half way between two
/// gaps takes the lower.
fn lane_gap(y_from: f32, y_to: f32, last_row: i32) -> i32 {
    let ideal = ((y_from + y_to) / 2.0 - lane_y(0)) / ROW_PITCH;
    ((ideal + 0.5).floor() as i32).clamp(-1, last_row.max(0))
}
