# 0026: Service map view choices

Date: 2026-10-22
Status: accepted

## Context

Milestone 3 Task 6 fixes what `views::MapView` draws and how it is used: nodes
and edges on a `canvas`, pan by drag, zoom with ctrl or cmd plus the wheel or a
pinch (0.5 to 2.0), a `Fit` button, a click on a service that filters the table,
and an empty-map message. The plan leaves open what "fit" means, what the wheel
alone does, what a click on something that is not a service does, how big the
map's panel is, and how the tests find a node. They are written down so that a
later agent does not "fix" them.

## Decision

- **The map is fitted until the user moves it.** A new map has no camera of its
  own: at each frame the camera is computed from the view and the picture
  (`Camera::fit`): full size from the top left corner, or scaled down (never
  below 0.5, never above 1) when the picture does not fit. It is not centred
  and not enlarged, because the layout is stable ("nothing jumps", design 9.2):
  a new column of nodes adds picture to the right, and a centred or enlarged
  map would shift every node when it came. A fitted map follows a resize. The
  first pan or zoom stores a camera and the map stays where the user put it,
  through resizes and new data, until `Fit` (or `MapView::fit`) drops it.
- **The wheel alone pans.** The plan asks for pan by drag and zoom by ctrl or
  cmd plus the wheel or a pinch; the Mac checklist (Task 11) says "two-finger
  drag to pan". A two-finger drag on a trackpad arrives as wheel events, so the
  wheel without a modifier pans. The map's panel has nothing else to scroll, so
  nothing is taken from anything. Ctrl or cmd plus the wheel zooms by
  `e^(0.005 x pixels)` around the pointer (the wheel turned up zooms in, as
  in browsers and map apps), a pinch by `1 + delta`; both keep the point under
  the pointer fixed and clamp at 0.5 and 2.0.
- **A click is a press that moves less than 3 px.** A drag may start on a node
  and then pans without selecting. The click is decided on release, at the
  release position.
- **Only a service can be selected.** A database, a queue or an external host
  has no filter the table can follow (`service_filter` is a `service.name`
  query), so a click on one does nothing: it neither selects it nor clears the
  service that is selected. A click on the selected service or on empty space
  clears the selection, through `select_service(None)`, which leaves a query the
  user typed alone.
- **Everything is on one canvas, including the text.** The plan says the map is
  our own `canvas`. Names, subtitles and labels are shaped and painted with the
  text system (`shape_line`, cut with `LineWrapper::truncate_line` and an
  ellipsis to the room in the node), so zoom scales the text with the boxes and
  a long service name never widens a node. Nodes are drawn after the edges. (This
  entry first said a line between non-neighbouring columns passes behind the
  nodes in between and that routing around them was not done. That drew a stray
  piece between two nodes that read as a call that does not exist; decision
  0033 routes those edges through the gaps between the rows.)
- **Edge labels sit in a small pill.** The label is above the curve's midpoint
  as the plan says, in a `panel`-filled pill with a hairline, and drawn after
  the nodes so a steep curve does not strike through the text.
- **The home screen is two panels, the map 52% of the height.** At the default
  window the map has 427 px, enough for the checkout's 416, so the screenshot
  and a typical mesh show the map at full size; the table keeps the rest. At
  the smallest window (562 px of content) the map shrinks to fit (0.7 for the
  checkout) rather than scroll. Until Task 7 the table's panel holds the word
  `Traces`.
- **`MainWindow` owns the `HomeModel` and the `MapView`.** Both are made when
  the window is, so the 500 ms refresh runs for the window's life (the plan
  says "while the window is visible") and the map keeps its camera and its
  node places when the Trace screen is shown and left. `main.home()` and
  `main.map()` give tests the entities.
- **How a test finds a node.** `MapView::node_center(id, cx)` is the layout's
  node centre through the camera and the area of the latest frame, in window
  coordinates, so a test clicks "the node's centre, from the layout" with real
  input and no hard-coded pixels. `fit_button_center()` does the same for the
  button, measured by a canvas the way the shell measures its glass regions.
- **The harness gained `Driver::drag`, `scroll_with` and `pinch`.** The plan
  lists `click`, `scroll`, `type_text`, `key`, `advance` and `capture`; the map
  needs a drag, a ctrl-scroll and a pinch to be tested through GPUI's own event
  path.
- **The test data's clock moves after the traces.** `common::home_store()` feeds
  the three scenarios with the store's clock at `BASE` and then advances it 70
  s. A trace is complete only after its quiet period (10 s) has passed on the
  store's clock, and `card-gateway` is a node of the map only for a complete
  trace; with the clock set past `BASE` from the start the map has no
  `card-gateway` and the screenshot would show a different picture from the
  topology the plan lists.

## Consequences

- The map's rates read `0.0 req/s` in the screenshots: a few traces counted over
  a 15 minute window. That is what the layout model's formula gives
  (`count / window_secs`), not a bug of the view.
- The Mac behaviour of pinch and two-finger drag is unverified until the M3
  check (`docs/areas/ui.md`, "Unverified").
