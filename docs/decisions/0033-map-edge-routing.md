# 0033: Map edges are routed through the gaps, not under the nodes

Date: 2026-10-22
Status: accepted

## Context

The service map drew every edge as one cubic bézier from the caller's right-edge
middle to the callee's left-edge middle, with the nodes drawn on top (decision
0026 accepted that a line between columns that are not neighbours passes behind
the nodes in between). In the checkout `orders-api` (column 1) calls
`postgresql` (column 3) and the curve ran behind `inventory-api` and
`payments-api`. The nodes are opaque, so only a stray piece showed, between the
bottom of one and the top of the other with no arrowhead: it read as a call from
`inventory-api` to `payments-api` that does not exist, on the map that §3.1 relies
on to show which service broke. Its arrowhead also landed on `postgresql` where
the `payments-api` edge's did. The whole-branch review found it in every home
reference.

## Decision

- **Routes are model, not view.** `model::map_layout::route_edges(&MapLayout)`
  returns, per edge, an `EdgeRoute`: where it starts, then `Line` and `Cubic`
  pieces, the last a curve that arrives level (the arrowhead points right into
  the node). The view only draws it (and the label's place), so every route is
  tested without a window, and the view's old curve code and `MIN_BEND` moved to
  the model.
- **An edge to the next column is unchanged**: the same cubic, so the references
  of the other edges did not move.
- **A longer edge runs along a lane.** Nodes only ever fill the rows, 64 px
  high every 96, so the 32 px gap under each row (and the margin above the first
  row and under the last) is free in every column. The edge leaves along a curve
  into the lane at the left edge of the first column in between, runs straight to
  the right edge of the last, and curves down or up to the callee. The lane is
  the one nearest the middle of the two ends (half way takes the lower), so the
  edge does not wander. No new rows or dummy nodes are made: they would move
  nodes, which the layout promises never to do.
- **A same-column or backward edge gets a lane too**, with a bulge out of the
  caller's right edge and one into the callee's left edge from outside
  (`LOOP_BULGE`, 40 px; the curve reaches three quarters of it, inside the 40 px
  margin). Such edges exist because a node keeps its place when a call appears
  that would put it elsewhere, and in a loop (`a` calls `b` calls `a`) the edge
  closing it goes backwards. They used to be drawn across the nodes too.
- **Edges that share a lane are spread over it**: 0, -5, +5, -8 and +8 px from
  the middle in the order of the edges, so two lines never lie on each other and
  the farthest is 8 px clear of the nodes either side.
- **The test is the contract.** `tests/model_map.rs` ("where the edges are
  drawn") pins the checkout's exact route and then asserts, for the checkout, 300
  random maps and 150 maps that grow with remembered places, that every route
  starts and ends on the nodes' edges, arrives level, stays inside the picture,
  is under no node and at least 6 px from every node it does not join. Making the
  long edge one curve again fails the checkout test.

## Consequences

- The six home references that held the stray piece were regenerated (and
  `shell_*` for the toolbar, decision 0032). The lane of `orders-api` to
  `postgresql` is the gap between `inventory-api` and `payments-api`, and its
  arrowhead lands on `postgresql` level, merging with the `payments-api` one: two
  callers, one port, as every node with two callers has.
- A very busy map has many long edges and some lanes crowd; the 5 offsets repeat
  after five edges in one gap. If that matters a later change can widen the rows
  or route by crossing count; the model's tests say what must stay true.
