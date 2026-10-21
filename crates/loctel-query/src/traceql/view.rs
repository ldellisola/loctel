//! A trace's spans with the parent links indexed once.
//!
//! The evaluator asks structural questions ("is this span below that one?")
//! about every pair of spans a query names, so a trace is indexed up front:
//! each span's parent becomes an index, and the tree is numbered as a nested
//! set so that ancestry is a pair of integer comparisons.

use std::collections::HashMap;

use loctel_model::{Span, SpanId};

/// The spans of one trace, indexed for structural queries.
///
/// Indices are positions in the slice given to [`TraceView::new`], and every
/// method speaks in them. The slice is a trace in whatever state it has
/// reached: spans arrive over time, so parents may be missing, and a view
/// copes with that. It also copes with data that should not occur (a span that
/// is its own parent, a loop of parents, two spans with one ID) without
/// panicking or looping, because a rules file must never be able to hang
/// ingest:
///
/// - **A span whose parent is not in the slice** is the root of its own
///   subtree: [`parent`](Self::parent) is `None`, and its descendants stay its
///   descendants.
/// - **A span that names itself as its parent** is treated the same way.
/// - **A loop of parents** is cut at its lowest-numbered span, which then
///   becomes the root of the rest of the loop.
/// - **Two spans with one ID:** children attach to the first of them.
///
/// Building a view takes O(n) time and memory and never recurses, so a trace
/// of any depth is fine.
pub struct TraceView<'a> {
    spans: &'a [Span],
    /// The index of each span's parent, `None` for a root of a subtree.
    parent: Vec<Option<usize>>,
    /// The nested-set numbering: the position of each span in a preorder walk
    /// of the forest (roots, and each node's children, in index order).
    left: Vec<usize>,
    /// The position of the last span of each span's subtree. A span `d` is
    /// below `a` exactly when `left[a] < left[d] && left[d] <= last[a]`.
    last: Vec<usize>,
    /// The span at each preorder position (the inverse of `left`).
    order: Vec<usize>,
    /// The trace's root, by the store's rule.
    root: Option<usize>,
}

impl<'a> TraceView<'a> {
    /// Indexes the parents of `spans` and numbers the tree.
    pub fn new(spans: &'a [Span]) -> TraceView<'a> {
        let count = spans.len();

        let mut by_id: HashMap<SpanId, usize> = HashMap::with_capacity(count);
        for (i, span) in spans.iter().enumerate() {
            by_id.entry(span.span_id).or_insert(i);
        }
        let mut parent: Vec<Option<usize>> = spans
            .iter()
            .enumerate()
            .map(|(i, span)| {
                span.parent_span_id
                    .and_then(|id| by_id.get(&id).copied())
                    .filter(|&p| p != i)
            })
            .collect();
        break_cycles(&mut parent);

        // Children of each span, in index order, packed into one array.
        let mut first_child = vec![0usize; count + 1];
        for p in parent.iter().flatten() {
            first_child[p + 1] += 1;
        }
        for i in 0..count {
            first_child[i + 1] += first_child[i];
        }
        let mut next_slot = first_child.clone();
        let mut children = vec![0usize; first_child[count]];
        for (i, p) in parent.iter().enumerate() {
            if let Some(p) = p {
                children[next_slot[*p]] = i;
                next_slot[*p] += 1;
            }
        }

        // Preorder walk of every tree of the forest, with an explicit stack.
        let mut order = Vec::with_capacity(count);
        let mut left = vec![0usize; count];
        let mut stack = Vec::new();
        for root in (0..count).filter(|&i| parent[i].is_none()) {
            stack.push(root);
            while let Some(node) = stack.pop() {
                left[node] = order.len();
                order.push(node);
                // Reversed, so that the lowest index is walked first.
                stack.extend(
                    children[first_child[node]..first_child[node + 1]]
                        .iter()
                        .rev(),
                );
            }
        }
        debug_assert_eq!(order.len(), count, "every span is in some tree");

        // A subtree's size, counted bottom-up: a preorder walk puts every
        // span after its parent, so walking it backwards visits children first.
        let mut size = vec![1usize; count];
        for &node in order.iter().rev() {
            if let Some(p) = parent[node] {
                size[p] += size[node];
            }
        }
        let last = (0..count).map(|i| left[i] + size[i] - 1).collect();

        // The store's root rule: the parentless span that started first, a
        // tie going to the lowest span ID. A span that names a missing parent
        // is not parentless, so it is not the trace's root.
        let root = spans
            .iter()
            .enumerate()
            .filter(|(_, span)| span.parent_span_id.is_none())
            .min_by_key(|(_, span)| (span.start_unix_nano, span.span_id))
            .map(|(i, _)| i);

        TraceView {
            spans,
            parent,
            left,
            last,
            order,
            root,
        }
    }

    /// The spans, as given.
    pub fn spans(&self) -> &'a [Span] {
        self.spans
    }

    /// The index of span `i`'s parent. `None` for the trace's root, for a span
    /// whose parent is missing, and for an `i` out of range.
    pub fn parent(&self, i: usize) -> Option<usize> {
        self.parent.get(i).copied().flatten()
    }

    /// Whether span `a` is a strict ancestor of span `d` (a parent, a
    /// grandparent, and so on): never true for `a == d`, nor for an index out
    /// of range. Two integer comparisons, whatever the distance.
    pub fn is_ancestor(&self, a: usize, d: usize) -> bool {
        match (self.left.get(a), self.left.get(d)) {
            (Some(&left_a), Some(&left_d)) => left_a < left_d && left_d <= self.last[a],
            _ => false,
        }
    }

    /// The root of the trace: the earliest-starting span with no parent (a tie
    /// goes to the lowest span ID), as the store reckons it. `None` while no
    /// parentless span has arrived.
    pub fn root(&self) -> Option<usize> {
        self.root
    }

    /// The spans in preorder: every span comes after its parent, and a span's
    /// whole subtree follows it in one run.
    pub(super) fn preorder(&self) -> &[usize] {
        &self.order
    }

    /// The position of the last span of `i`'s subtree in [`preorder`](Self::preorder).
    pub(super) fn last(&self, i: usize) -> usize {
        self.last[i]
    }
}

/// Cuts every loop in the parent links, so that the links form a forest.
///
/// Each span has at most one parent, so a component holds at most one loop.
/// Walking up from every span finds it; the loop is cut at its lowest index,
/// which loses that span's parent link and makes it a root.
fn break_cycles(parent: &mut [Option<usize>]) {
    const UNSEEN: u8 = 0;
    const ON_PATH: u8 = 1;
    const DONE: u8 = 2;

    let mut state = vec![UNSEEN; parent.len()];
    let mut path = Vec::new();
    for start in 0..parent.len() {
        if state[start] != UNSEEN {
            continue;
        }
        path.clear();
        let mut node = start;
        loop {
            match state[node] {
                UNSEEN => {
                    state[node] = ON_PATH;
                    path.push(node);
                    match parent[node] {
                        Some(p) => node = p,
                        None => break,
                    }
                }
                ON_PATH => {
                    // Walked into this very path: it is a loop, from the first
                    // visit of `node` to the end of the path.
                    if let Some(at) = path.iter().position(|&n| n == node)
                        && let Some(&cut) = path[at..].iter().min()
                    {
                        parent[cut] = None;
                    }
                    break;
                }
                _ => break,
            }
        }
        for &visited in &path {
            state[visited] = DONE;
        }
    }
}
