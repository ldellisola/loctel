# 0022: Service map layout choices

Date: 2026-10-22
Status: accepted

## Context

Milestone 3 Task 2 fixes the interface of `loctel_ui::model::map_layout`
(`PlacedNode`, `PlacedEdge`, `MapLayout`, `LayoutMemory`, `layout`), the
algorithm in five lines and the subtitle formats. The plan leaves open where
the search for loops starts, what "kept for three refreshes" is exactly, what
an edge's ends mean when ids are shared by two kinds, and one of its examples
does not follow the shared rate rule. These are choices a later agent might be
tempted to "fix", so they are written down with the reason.

## Decision

- **Subtitles use `format::rate`, so a rate under 10 has a decimal.** The
  plan's examples `queue · 9 msg/s` and `external · 3 req/s` cannot come from
  the rule of Global Constraints (10 or more whole, otherwise one decimal,
  tested in `model_format` as `1.0 req/s`), so 9 messages a second reads
  `queue · 9.0 msg/s`. Two rate formats on one screen would be worse than one
  example read as a template. The strings with a rate of 10 or more
  (`84 req/s · 2.1% err`, `database · 140 q/s`) are exactly the plan's.
- **The search for loops starts at the sources.** The plan says "ignoring back
  edges found in a DFS in id order". The search runs over the nodes in id
  order, but first from the nodes nobody calls and only then from whatever is
  left (a loop with no entry). Starting at the first id would turn a service
  that sorts first and is only called from a loop into the first column,
  above the real entry point of the mesh. On a map with no loop the order makes
  no difference, so the checkout is the same either way.
- **"Missing for three refreshes" means a node can be absent from three
  refreshes in a row and still have its place; the fourth forgets it.** The
  place is kept in the memory only: the node is not drawn (its counts are not
  known, and a stale node on the map would say something false) but nobody
  takes its row, and it comes back to it. A refresh with the node in it starts
  the count again.
- **A remembered node keeps its column even if a new call says it should be
  further right.** The plan says a node in the memory keeps its layer and slot.
  The cost is a line that sometimes runs sideways or backwards when services
  arrive in an odd order (each service exports on its own, so the callee can
  be seen before the caller), and a map grown call by call can differ from the
  same map laid out from nothing. Resetting the memory (`LayoutMemory::default()`)
  re-lays it out; a view decides when.
- **Holes are not closed.** The rows of a node that was forgotten stay empty
  until a new node fills them, and the picture is as big as the nodes drawn,
  not the places held.
- **An edge's end is the first node with that id.** The store's edges carry
  names and no kind, and ids are unique within a kind only, so an edge into
  `payments` could mean a service or a database of that name. The layout takes
  the first in the order service, database, queue, external. An edge to an id
  that is no node is dropped, so every placed edge joins two placed nodes; a
  call of a node to itself is drawn but ignored for columns.
- **`PlacedNode` has no `layer` field.** The plan's interface is kept exactly;
  the column and row are `(x - 40) / 272` and `(y - 32) / 96`, which is what the
  tests read back. The edge label rule ("only on edges out of a layer-0 node")
  uses the column the node was placed in, remembered or not.
- **Service colours are numbered in the order of the picture.** A service the
  session has not met gets the next number as the layout visits it (column by
  column, row by row), so on a fresh session the map's colours read left to
  right. A service the trace table met first keeps its number. `color` is the
  unreduced index, as in `TraceRow`; the view takes `% 5`.
- **A window that is not a positive length of time gives zero rates** (`0.0
  req/s`), never infinity or NaN.

## Consequences

- Each rule is pinned by a test in `tests/model_map.rs` that names it, and the
  gap tests fail for a count of 2 or 4 refreshes as well as for 3.
- The map view (Task 6) draws what `layout` returns and owns the
  `LayoutMemory`; it must not compute a place of its own.
- A change to the checkout topology or to the rows the barycentre gives moves
  the map screenshots; `checkout_topology_columns` pins every place of the
  fixed mesh first.
