# UI (`loctel-ui`) and the screenshot workflow

The GPUI layer, kept thin on purpose: views draw what the engine reports and
hold no logic of their own. Design references: §9.2 (screens), §9.3 (look and
feel), §10.3 (screenshot tests) and §10.5 (gotchas from the spike).

Status: Milestones 1 and 2 have one view, the **status view**, and the Linux
window that shows it. Milestone 2 added the discard rules line to it and to the
macOS menu; the rules screen is Milestone 4. The main window (Map & traces, Trace view) is Milestone 3
(in progress: the pure models in `src/model`, the window's shell in
`src/app` and the home screen's live data are done, the screens are not; see
"Models", "The main window" and "Live data" below), the remaining screens and the popover Milestone 4. The macOS menu bar
item is in `loctel-macos`; it only borrows the strings from `menu_text`.

GPUI comes as the weekly `gpui-pre` snapshot, `gpui-pre`, `gpui-pre-platform`
and `gpui-pre-wgpu`, all pinned to exactly `=0.3.9` (zed@93d7456), and GPUI Kit
(`gpui-kit =0.8.0`, which depends on that same `gpui-pre`) for the components of
the main window. Upgrading is its own change: the API moves, and the screenshots
are what shows you what moved (the `upgrade-gpui` skill has the steps; decision
0024 has why Kit).

## What is in the crate

| Item | What it is |
|---|---|
| `StatusModel`, `ListenerLine` | plain values for the view and the menu. `StatusModel::from_status(&EngineStatus)` builds one. A failed listener keeps the address it was asked for, and its error. The rules fields: `rules_on` (enabled rules, whether or not their queries parse), `rules_with_errors` (the enabled ones that do not parse; a switched-off draft with a bad query is not counted, so it never exceeds `rules_on`), `rules_file_error` (a flag: `rules.toml` could not be used; the text stays in `EngineStatus`) and `dropped_fraction` (0 to 1, since launch) |
| `StatusView` | the GPUI view: a headline pill, four numbers (spans/s, logs/s, traces held, logs held), the discard rules line under them (a dot and the words of `rules_line`; red dot and text when the file or a rule has an error, green when rules are on, grey when none is) and the endpoint list. `StatusView::new(model, theme)`, `set_model(model, cx)` (redraws only when the model changed) and `refresh_from(engine, cx)` (once a second, until the view is dropped) |
| `open_status_window(engine, cx)` | opens the 900 by 600 window on an `App`: dark theme, refreshed every second from `EngineHandle::status()`. This is what `loctel` (no `--headless`) shows on Linux |
| `menu_text::status_line(&StatusModel)` | the one-line status: `Receiving · 412 spans/s · 1.2k logs/s`, `Paused` or `Port problem: otlp-http (address in use)`. A failed listener wins over paused, paused over receiving. The reason is `address in use` when the OS error contains "in use", otherwise the raw error cut to 40 characters. The first failed listener is the one named |
| `menu_text::rules_line(&StatusModel)` | the discard rules in one line, the first that applies: `Discard rules file has an error`; `1 discard rule on · 1 has an error` (for more than one, `3 discard rules on · 2 have errors`); `No discard rules on`; `1 discard rule on · 31% dropped` or `3 discard rules on · 0% dropped`. The percentage is the share the rules dropped of what arrived since launch, rounded to a whole number; a value that is not a number reads `0%` and one outside 0 to 1 is cut to it. A file error wins over a rule error, a rule error over "no rules on" |
| `menu_text::format_rate(f64)` | `412` is `412`, `1234` is `1.2k`, `0.4` is `0`. One decimal under ten thousand, none above, never `.0` (`1k`, `10k`, `1.3M`); NaN, infinity and negatives read `0` |
| `model` | the pure models of the main window: `format`, `colors`, `trace_table`, `query_bar`, `map_layout`, `waterfall`, `inspector` (see "Models" below) |
| `app` | the main window's shell (see "The main window"): `MainWindow`, `Screen`, `UiBackend`, `open_main_window`, `Backdrop`/`GlassRegion`, `kit_theme`; and the home screen's live data (see "Live data"): `HomeModel`, `HomeData`, `TimeRange` |
| `Theme` | the colours below. `Theme::dark()`, `Theme::light()` and `Theme::for_appearance(WindowAppearance)`. It is also a GPUI global once `kit_theme::apply` has run |
| `KitAssets` | GPUI Kit's bundled icons, to register with `Application::with_assets` |
| `register_fonts(cx)`, `FONT_FAMILY` | the bundled typeface (below) |
| `testing` (feature `screenshot`) | `render_png` and `assert_snapshot`; for the main window `render_window_png`, `Driver`, `headless_app`, `FixedBackend`, `RecordingBackdrop` (see "Screenshot tests") |

The strings in `menu_text` are the contract: the macOS menu header and the
status view's pill both use `status_line`, the macOS menu's rules header and the
view's rules row both use `rules_line`, and `crates/loctel-ui/tests/menu_text.rs`
pins them.

## Models

`loctel_ui::model` holds everything the main window shows that is not a pixel.
The models are plain Rust over the store's types (`TraceSummary`, `FailedIn`)
and the TraceQL parser, with no GPUI in them: a colour is an index into
`Theme::service_palette`, and a view only lays out the strings and numbers it
is given. The rules of the design (the formats, the failure display of §9.4,
the colours of §9.3) are therefore tested with plain `assert_eq!`s, in
`tests/model_format.rs`, `tests/model_table.rs`, `tests/model_map.rs` and
`tests/model_waterfall.rs`.

### Formats (`model::format`)

These strings are the contract, like those of `menu_text`. Every view uses
them and nothing formats a number or a time by itself.

| What | Function | Rule | Examples |
|---|---|---|---|
| duration | `duration(nanos)` | 1 s or more: two decimals; 1 ms or more: whole ms; otherwise whole µs | `1.84 s`, `212 ms`, `850 µs`, `0 µs` |
| clock | `clock(unix_nano, utc_offset_secs)` | `HH:MM:SS.mmm` in the zone `utc_offset_secs` east of UTC; no date; milliseconds are cut, not rounded | `14:32:07.412` |
| rate | `rate(per_sec, unit)` | 10 or more: whole; below: one decimal (so zero is `0.0`); not a non-negative number reads as zero | `84 req/s`, `0.3 req/s` |
| error share | `error_pct(errors, total)` | one decimal; no errors, or no total, is `0%` | `2.1% err`, `0% err` |
| cut-off text | `truncate_end(s, max_chars)`, `truncate_middle(s, max_chars)` | at most `max_chars` characters, `…` included; text that fits is unchanged | `abc…` (`abcdef`, 4), `ab…ij` (`abcdefghij`, 5) |

Details that the examples do not show, each pinned by a test:

- **The unit of a duration comes from the exact value, then the number is
  rounded in that unit.** The unit never rolls over: 999,999,999 ns is
  `1000 ms` (not `1.00 s`), and 999,500 ns is `1000 µs` (not `1 ms`). Rounding
  is half up on integers, so `1.845 s` is `1.85 s` on every machine.
- **`rate`** has no thousands separator and no `k`: `1234.5` is `1235 req/s`.
  A value that rounds up to 10.0 reads `10 req/s`, not `10.0 req/s`. (The
  status line's `menu_text::format_rate` is a different, compact format for
  the spans/s and logs/s counters.)
- **`error_pct`**: `0% err` means exactly no errors. An error count too small
  to show, one in 100,000, reads `0.0% err`, so the two can be told apart. More
  errors than total (which cannot happen) reads `100.0% err`.
- **Truncation counts characters, not bytes**, so it never cuts inside one.
  It does not know about grapheme clusters. With `max_chars` 0 the result is
  empty and with 1 it is just `…`. `truncate_middle` gives the extra character
  to the start when the kept characters cannot be shared equally (`abc…ij` at
  6). Long span names and attribute values are cut for rows and tooltips with
  these, so a 300-character name never widens the layout.
- **`local_utc_offset()`** is the system zone's offset now, which the app
  passes to `clock`. Tests always pass 0.

### Service colours (`model::colors`)

`ServiceColors` numbers services in the order the session first sees them.
`index(&mut self, service)` assigns the next number the first time and gives the
same one ever after; `peek(&self, service)` looks without assigning. The index
counts on past the palette, and the palette slot is `index % 5`, so the sixth
service shares the first one's colour. One `ServiceColors` lives as long as the
window's data, so a service keeps its colour in the map, the table dots, the
waterfall and the logs for the whole run.

### Trace table rows (`model::trace_table`)

`rows(summaries, &mut colors, utc_offset)` turns the store's summaries into
`TraceRow`s, in the same order, asking `colors` for every service of every row
in order. A row holds each column as the exact string or number the view draws:

| Field | Content |
|---|---|
| `started` | `format::clock` of the earliest span start |
| `root` | the root span's name, or `(root not arrived)` while it has not arrived |
| `failed_in` | the red `Failed in` text, only for a **failed** trace (the root span is erroring) |
| `errors_inside` | the amber `errors inside`, only when the root is fine and another span is erroring |
| `services` | `(name, colour index)` per service, in the order of its first span |
| `span_count`, `complete` | as in the summary |
| `duration`, `duration_fraction` | `format::duration`, and the bar: the duration over the longest of the rows given (0 to 1; 0 when none has any length) |

`failed_in` and `errors_inside` are never both set: the store also names a
lowest erroring span for a trace with errors inside, but §9.4 shows that case
as the amber `errors inside` and keeps `Failed in` for failures. A trace with
only an exception event and no error status has neither (the marker is the
waterfall's).

`failed_in_text(&FailedIn)` is `service · span name` (a middle dot, U+00B7),
then ` peer` for a span with a peer (a client or producer span), then ` +N` when
`more > 0`: `payments-api · POST card-gateway +1`. The text is whole; the view
cuts it to the width it has.

### Query bar (`model::query_bar`)

`validate(text)` reads the bar: `Empty` for text that is all whitespace,
`Valid(Arc<TraceQuery>)` (parsed once, ready for `TraceFilter::query`), or
`Invalid { column, message }`. The column counts from 1 in characters of the
text as typed (blanks included), and the message has no column in it:
`{ status = ` is `Invalid { column: 12, .. }`, the end of the text. A TraceQL
feature that loctel does not support yet is an `Invalid` too, never a quiet
mismatch. The bar keeps the last good results on an `Invalid` (Milestone 3
plan, review focus 4).

`service_filter(service)` is `{ resource.service.name = "<name>" }`, written
into the bar when a service is clicked. `"` and `\` in the name are escaped
with a backslash, the only escapes a name needs, so any name gives a valid
query for exactly that service.

### Map layout (`model::map_layout`)

`layout(&ServiceMap, window_secs, &mut ServiceColors, &mut LayoutMemory)` places
the service map (`Store::service_map`) for the view to paint. It returns a
`MapLayout`: `nodes` (`PlacedNode`: `id`, `kind`, `color`, `x`, `y`, `w`, `h`,
`subtitle`, `failing`), `edges` (`PlacedEdge`: `from`, `to`, `calls`, `errors`,
`failing`, `label`) and the `width` and `height` of the picture (0 and 0 for a
map with no node). Nodes come in the order of the picture, by column and then by
row; edges in the order of the map, by `(from, to)`.

**Where a node goes**, in this order:

1. **Column.** The longest path from the sources, the nodes nobody calls. A
   loop is broken by leaving out the edge that closes it: a depth-first search
   (its own stack, so a chain of any length is fine) starts at the sources in
   id order, then at every node it has not reached, also in id order, and an
   edge into a node that is still being searched is a back edge. For `a` and `b`
   calling each other the edge `b` to `a` is left out, so `a` is in column 0 and
   `b` in column 1. Starting at the sources means the entry point of a mesh
   stays in the first column even when a loop below it has an id that sorts
   before it. A call of a node to itself says nothing about columns. The edges
   left out are still drawn: only the columns ignore them.
2. **Row.** Within a column, by the middle (barycentre) of the rows of the
   nodes that call it, ties by id and then by kind. A node nobody calls has no
   middle and goes first. For the checkout this gives `redis` below
   `inventory-api`, `postgresql` between `orders-api` and `payments-api`,
   `card-gateway` below `payments-api` and `notifications-worker` below
   `rabbitmq`, so most lines run straight across.
3. **Place.** `x = 40 + column x (176 + 96)` and `y = 32 + row x (64 + 32)`,
   each node 176 by 64. The picture is 40 px (left and right) and 32 px (top
   and bottom) bigger than the nodes: `width = 80 + columns x 176 + (columns -
   1) x 96`, and the same for `height`.

| Checkout | Column 0 | Column 1 | Column 2 | Column 3 |
|---|---|---|---|---|
| row 0 | `web-bff` | `orders-api` | `inventory-api` | `redis` |
| row 1 | | | `payments-api` | `postgresql` |
| row 2 | | | `rabbitmq` | `card-gateway` |
| row 3 | | | | `notifications-worker` |

**Staying put.** `LayoutMemory` holds the column and row of every node placed.
One lives as long as the map is on screen, and a new one
(`LayoutMemory::default()`) lays everything out again from nothing, for
example when the set of nodes is another one because the time range changed.

- A node in the memory keeps its column and row, even when a new call would
  put it further right: a map that moves when a call appears is worse than a
  line that runs sideways. The same map laid out from nothing can therefore
  differ from the same map grown call by call.
- A new node goes to the first free row of the column it computed. Rows are
  never made room for, so a new service never moves another one, and a hole
  left by a node that went away stays until a new node fills it.
- A node missing from the map keeps its place in the memory for **3
  refreshes in a row**: nobody takes its row, it is not drawn, and it returns
  to the same place. The fourth refresh without it forgets it, and if it ever
  comes back it is a new node. One refresh with the node in it starts the
  count again.

**What a node says.** Rates are `count / window_secs`, through the one rate rule
of `format` (10 or more whole, below that one decimal); a `window_secs` that is
not a positive number of seconds gives zero rates.

| Kind | Subtitle | Counts |
|---|---|---|
| service | `84 req/s · 2.1% err` | its server and consumer spans, and how many errored |
| database | `database · 140 q/s` | the calls it received |
| queue | `queue · 9.0 msg/s` | the messages published to it |
| external | `external · 3.0 req/s` | the calls it received |

(Below 10 the rate has its decimal, so the plan's `queue · 9 msg/s` reads
`queue · 9.0 msg/s`; decision 0022.) `failing` is `errors > 0`, for a node and
for an edge. The `label` of an edge is its rate (`84 req/s`) when the edge
leaves a node of the first column, and none otherwise. A service's `color` is
its number in the session's `ServiceColors`, the one shared with the trace
table (new services are numbered in the order of the picture); a database,
queue or external host has none.

**Ids.** An edge's ends are ids, and ids are unique within a kind only (a
database can be named like a service). The store's edges carry names and no
kind, so an end means the first node with that id, in the order service,
database, queue, external. An edge to an id that is no node is left out of the
layout, so every placed edge joins two placed nodes.

### Waterfall (`model::waterfall`)

`Waterfall::new(&TraceSnapshot, &mut ServiceColors)` turns one trace into the
rows of the trace view's waterfall, and `rebuild` takes in a newer snapshot of
the same trace as spans arrive. `rows()` is what the view draws: the **visible**
`WaterfallRow`s, top to bottom. A row holds `span_id`, `depth`, `has_children`,
`collapsed`, `name`, `service`, `color` (the unreduced `ServiceColors` index,
the view takes `% 5`), `peer_note`, `start_frac` and `width_frac` (the bar, as
shares of the trace), `duration` (`format::duration`), `is_error` and
`exception_fracs` (where the `exception` events are, as shares of the trace).

**The order.** Depth first; the roots, and the children of a span, go by start
time and then span ID. The walk has its own stack, so a chain of any depth is
fine (`depth` stops at 65,535) and 100,000 spans build in about 0.3 s in a debug
build (the test allows 1.5 s, for building and for rebuilding).

**Spans whose parent has not arrived** hang, one level down, under one
placeholder row at depth 0, `missing parent`, which comes after the last root's
subtree and is there only while such spans are. The placeholder is a
`WaterfallRow` with `is_placeholder()` true (its `span_id` is all zeros, which no
span has), name and `peer_note` `missing parent`, no service, no bar and no
duration; it folds like any row, and with the errors filter on it shows only if
an orphan's subtree has an error. A span with no parent is a root, so a second
one is a second tree before the placeholder. A parent loop (malformed) goes under
the placeholder too, so every span has one row.

**The bar.** `start_frac` is the span's start over the trace's length, and
`width_frac` its length over it, at least 0.003 (`MIN_WIDTH_FRAC`) so that a span
with no length shows, and never so far right that `start_frac + width_frac`
passes 1. The trace runs from the earliest start to the latest end, as the trace
table's duration does. A marker is the event's time on the same scale, from 0 to
1.

**`peer_note`** is the other side of a call, as the service map names it: a
client span's database (`postgresql`, `redis`), else its broker, else the host it
calls (`card-gateway`); a producer's broker (`rabbitmq`) else its host; a
consumer's broker. Other spans have none.

**What the user does** is kept by span ID across `rebuild`, so a trace that is
still arriving does not jump (Milestone 3 plan, review focus 1):

| Call | What it does |
|---|---|
| `toggle(id)` | folds the children of a span that has some, or opens them. A leaf or an unknown ID does nothing |
| `collapse_all()`, `expand_all()` | fold every span that has children now (a span that arrives later is open), or open all |
| `set_errors_path_only(on)` | shows only the erroring spans and the spans above them, or everything. No error means no row. It combines with folding: a folded span hides its errors |
| `select(id)` | selects the span and opens the spans above it. An unknown ID does nothing. The errors filter is not touched |
| `selected()`, `row_of(id)` | the selection (none in a new waterfall), and where a span is among `rows()` (`None` when a fold or the filter hides it; a hidden span stays selected) |
| `find(text)` | the positions in `rows()` whose span has `text` in its name, service or the value of any attribute, ignoring case. Keys, the status message, events and the resource are not searched; no text finds nothing; hidden rows are not looked at |
| `ruler()` | the labels at 0, 25, 50, 75 and 100%: `["0", "460 ms", "920 ms", "1.38 s", "1.84 s"]` |

`rebuild` keeps the folded spans, the selection and the errors filter, and forgets
a span that is no longer in the trace (it is not folded, and not selected).
`initial_selection(&TraceSnapshot)` is the span to select when the trace opens:
the failed-in span, else the root (none if the trace has neither yet).

### Inspector (`model::inspector`)

`span_details(&TraceSnapshot, id)` is the text of the inspector panel for the
span, or `None` for an ID the trace does not have (the placeholder's, too).
`SpanDetails` holds `name`, `service`, `kind` (`Server`, `Client`, `Internal`,
`Producer`, `Consumer`, `Unspecified`), `duration`, `offset` (`starts at +200 ms`,
and `starts at +0 ms` for the root), `status` (`Unset`, `OK` or `Error`, then ` ·
` and the status message when there is one: `Error · HTTP 500`), `exception` (the
span's first `exception` event: type, message, stack trace), and `attributes` and
`resource` as `(key, value)` pairs sorted by key, values whole. The view cuts the
long ones (design 9.2 and the Task 8 plan: over 200 characters, `… (show all)`).
Decision 0023 has the reasons for the details above.

## The main window

`loctel_ui::app` is the shell of the main window (Milestone 3, Task 4): the
screens of Tasks 6 to 8 go into its content area. Decision 0024 has the choices.

**Layout** (logical pixels, the constants are `pub` in `app`):

- The window opens at 1440 by 900 (`DEFAULT_SIZE`) and cannot be made smaller
  than 1000 by 640 (`MIN_SIZE`); its title is `loctel`.
- Everything floats on the window colour (`Theme::bg`) with `INSET` = 10 around
  and between the panels, and `PANEL_RADIUS` = 16 on each.
  - The **sidebar** is `SIDEBAR_WIDTH` = 220 wide, the window's full height less
    the inset. From the top it has `TRAFFIC_LIGHT_SPACE` (52, where the macOS
    traffic lights sit; plain room on Linux), the navigation (one entry,
    `Map & traces`, `NAV_ITEM_HEIGHT` 34, lit with the accent at 18%; Logs and
    Discard rules arrive in Milestone 4) and, at the bottom, the **status
    box**.
  - The status box shows `status_line` and `rules_line` (the words of the macOS
    menu) and a row per endpoint (`OTLP gRPC`, `OTLP HTTP`, `MCP`) with its
    address, a red dot and address when it failed. It reads a `StatusModel` from
    `UiBackend::status()` once a second and redraws only when it changed.
  - To its right, the **toolbar** (`TOOLBAR_HEIGHT` 48) over the **content
    panel**. The toolbar's contents depend on the screen: its title is
    `toolbar_title(&Screen)`: `Map & traces`, or
    `Map & traces › 4bf92f35…0e0e4736` on a trace. The Live indicator, the
    time range, Pause and Clear all arrive with the home screen.

**Screens.** `Screen` is `Home` or `Trace(TraceId)`; `MainWindow::screen()` and
`navigate(screen, cx)` read and change it, and clicking the navigation entry
shows `Home`.

**The backend.** Views never name the engine: they hold an `Rc<dyn UiBackend>`
with `store() -> Store`, `status() -> EngineStatus`, `clear_all()` and
`utc_offset() -> i32` (the system zone's offset in seconds for the clock times;
tests use 0). `impl UiBackend for EngineHandle` is the real one.
`testing::FixedBackend { store, status, utc_offset }` is the test one;
`FixedBackend::new(store)` gives the status of a healthy engine
(`testing::listening_status()`: three listening endpoints, nothing received, no
rules). Store reads that take time go through `store()` clones onto the
background executor, never on the frame.

**Opening it.** `open_main_window(backend, cx, attach_backdrop)` calls
`app::init(cx)` (the bundled font and `gpui_kit::init`, once per app), opens
the window with the options above and builds `MainWindow` inside GPUI Kit's
`Root`. `attach_backdrop` runs inside the new window and returns the
platform's `Backdrop` (macOS Liquid Glass, Task 10) or `None`. It leaves
`reduce_motion` at the system's value. The app must register `KitAssets` with
`Application::with_assets` for Kit's icons to draw.

**Theme.** `Theme::for_appearance` picks dark for `Dark` and `VibrantDark`,
light for the others. `MainWindow` applies it at creation and from the
window's appearance observer (`set_appearance`, which a test calls directly).
`kit_theme::apply(&Theme, cx)` is the one place our tokens meet Kit's:

- It stores the `Theme` as a GPUI global (our views read `cx.global::<Theme>()`
  while they render).
- It switches Kit to its dark or light mode.
- It lays our colours over Kit's `ThemeColor` (`kit_theme::kit_colors`):
  - `background`, `foreground`, `border` and `input` come from `bg`, `text` and
    `border`.
  - `primary`, `ring` and `link` are our `accent`.
  - Kit's `accent` is its hover tint, not ours.
  - The surfaces of the table, list, popover and buttons are `panel`.
  - The selected row is the accent at 18%.
- It sets the font family to `Inter`.

A component that looks wrong in a theme is fixed in `kit_colors`, never with a
literal in a view.

**Glass regions.** `Backdrop::set_regions(&[GlassRegion { bounds, corner_radius }])`
tells a platform where its glass parts are: the sidebar first, then the
toolbar, in window coordinates. The shell measures the real layout (canvas
prepaint callbacks over each panel's border box) and calls it once per frame
in which the pair changed, never otherwise (`RecordingBackdrop` in tests;
`shell_reports_glass_regions_once`, `glass_regions_follow_a_resize`). It
is called inside GPUI's frame, so an implementation only stores the regions
and applies them later (Task 10).

## Live data

`app::HomeModel` (`src/app/data.rs`, Milestone 3 Task 5) is what the home screen
reads: the service map and the trace table's rows, kept fresh from the store.
Decision 0025 has the choices and the reasons. It is an entity the views
observe; `tests/live.rs` pins every rule below in a headless app with no window
(`testing::headless_app()`), moving its clock to fire the timer, over a store on
its own `ManualClock`.

```rust
let home = cx.new(|cx| HomeModel::new(backend, cx));   // reads at once, then every 500 ms
home.update(cx, |home, cx| home.set_errors_only(true, cx));
let rows = &home.read(cx).data().rows;
```

**`HomeData`** (`home.data()`) is the result of the latest read:

| Field | Content |
|---|---|
| `map` | the service map over the time range, placed (`map_layout::layout`), with the colours of the session |
| `rows` | the trace table: the newest matching traces, newest first, at most `HOME_MAX_ROWS` (1,000) |
| `truncated` | more than 1,000 traces matched (exactly 1,000 is not more): the footnote `Showing the newest 1,000` |
| `query` | the query bar's `QueryState` for the text as typed now |
| `status` | the status box's `StatusModel`, read with the data |
| `refreshed_at` | the store's clock at the read, in nanoseconds; 0 until the first read is back |

**The settings** (each takes the entity's context, and does nothing when set to
the value it has):

| Call | Effect |
|---|---|
| `set_range(TimeRange)` | `Last5m`, `Last15m` (the default), `LastHour` or `Everything`; `label()` is `Last 5 minutes`, `Last 15 minutes`, `Last hour`, `Everything`. `since(now)` is the earliest start kept: `now` minus the length (0 if the clock is smaller), `None` for `Everything`; a trace that started exactly then is kept |
| `set_errors_only(bool)` | only failed traces and traces with errors inside |
| `set_slow_only(bool)` | only traces of `SLOW_TRACE` (1 s) or longer, 1 s included |
| `set_query(text)` | validates at once (`data().query` is new when the call returns) and reads when the text is empty or parses |
| `select_service(Some(name))` | writes `service_filter(name)` into the query and remembers the selection (`selected_service()`) |
| `select_service(None)` | clears the selection: the query goes back to empty, but only while it is still that service's filter; a query the user typed is left alone |
| `set_paused(bool)` | freezes the timer (below); un-pausing reads at once |
| `refresh_now()` | reads now, whatever pause says |

The table's filter is the range, errors only, slow only and the valid query
together, all of which must hold, with a limit of 1,000. **The map is filtered
by the range alone**: it is the picture of everything in the window, and the
table is the list of what was asked for. Reading accessors for the controls:
`query_text()` (as typed), `selected_service()`, `range()`, `errors_only()`,
`slow_only()`, `paused()`.

**Refreshing.** A timer reads every `HOME_REFRESH` (500 ms) until the model is
dropped (the window closed), unless paused or a read is still running.
Every read runs on `cx.background_executor()`, on a copy of its inputs (a
`Store` clone and the filter), so a search never blocks a frame; its result
comes back to the foreground, where the rows and the layout are built (they
need the session's colours and the map's memory, which the model owns). A read
that a newer one replaced is dropped when it arrives, so the controls and the
rows never disagree. Something the user does always reads, even over a running
read. The views are told only when the map, the rows, the flag or the status
changed, so an idle store costs no frames.

**Pause** freezes the view while ingest goes on: the timer reads nothing. It
stops the timer, not the user: a query, a toggle, a range or a click on a
service changed while paused still reads, and so does `refresh_now`.

**An invalid query** keeps the previous rows (never an empty table that looks
like "no traces"): `data().query` is `Invalid { column, message }` at once, the
view shows it under the input, and the table keeps reading with the last query
that was good until the text parses again.

**Rates on the map** are counts over the window: 300, 900 or 3,600 seconds for
the three ranges, and for `Everything` from the oldest trace in the store
(`Store::oldest_start_unix_nano`) to now, at least a second (zero rates for an
empty store).

**Colours.** The model owns the session's `ServiceColors` and the map's
`LayoutMemory`. The rows are built before the map, so the first services of the
newest traces get the first colours. `home.colors()` returns the shared
`Rc<RefCell<ServiceColors>>`: a view that colours services (the trace view's
waterfall, the logs) takes it from there, so a service has one colour
everywhere for the run.

## Theme tokens

The dark tokens come from the accepted mockups; a test pins them. The light
ones are the builder's reading of them (the same hues, darkened to hold their
contrast on a pale ground; a test checks that text stays at 4.5:1 or better on
both grounds in both themes). Views read `Theme` fields and never write a hex
literal.

| Token | Dark | Light | Use |
|---|---|---|---|
| `bg` | `#15161a` | `#f4f4f6` | window background |
| `panel` | `#1a1b20` | `#ffffff` | cards and lists |
| `border` | `#2a2b32` | `#dcdce2` | hairlines |
| `text` | `#ececf1` | `#1b1c20` | primary text |
| `text_secondary` | `#a3a3ad` | `#4d4d57` | addresses, state |
| `text_tertiary` | `#8b8b95` | `#62626c` | small labels |
| `accent` | `#4c9dff` | `#0a62d6` | focus, links |
| `error` | `#ff6b63` | `#d92d20` | error dots and fills |
| `error_text` | `#ff8a84` | `#b42318` | error text |
| `warn` | `#f5c26b` | `#9a5b00` | paused |
| `ok` | `#34c759` | `#1a7f37` | listening |
| `service_palette` | `#4c9dff #3cc8b4 #f0a04b #b18cff #f07fb0` | `#0a62d6 #0f7f70 #b45f06 #6f42c1 #c2306f` | one per service, in order, wrapped |

Errors are never marked by colour alone: a failed listener says `Failed` and
the error in words, the pill says `Port problem`, and the rules row says
`has an error` or `Discard rules file has an error`.

The status view's window always uses the dark theme. The main window follows
the system appearance (§9.3): see "The main window".

## The font

`FONT_FAMILY` is `Inter`, and the crate bundles `Inter-Regular.otf` and
`Inter-SemiBold.otf` (`assets/fonts/`, SIL OFL 1.1, licence next to them).
`register_fonts` adds them to a GPUI `App`; the window does it at launch and
the screenshot harness for every render.

Why bundle: a pixel-exact test must not depend on which fonts a machine
happens to have. On Linux the harness builds its text system with
`CosmicTextSystem::new_without_system_fonts`, so the bundled font is the only
one it can see, and a reference made in one container holds on CI. The design
says the product uses the system font (SF) on macOS; that switch belongs to the
Milestone 3 window, not to this view. See
[`decisions/0008-ui-and-screenshot-choices.md`](../decisions/0008-ui-and-screenshot-choices.md).

## Screenshot tests

`testing::render_png(size, build)` draws a view through GPUI's headless
renderer (`HeadlessAppContext` with `gpui_platform::current_headless_renderer`,
which is wgpu on the CPU through Mesa's lavapipe: no GPU and no display) and
returns an `image::RgbaImage`. `build` runs inside the window and returns an
`AnyView`:

```rust
let image = render_png((900, 600), |_window, cx| {
    cx.new(|_| StatusView::new(model, Theme::dark())).into()
});
assert_snapshot("status_view_dark", &image);
```

- **The image is 2x.** The headless platform draws at scale 2, like a Retina
  display, so `(900, 600)` comes back as 1800 by 1200 pixels.
- **`assert_snapshot(name, &img)`** compares with
  `crates/loctel-ui/tests/snapshots/{name}.png` pixel for pixel (decoded RGBA,
  so the PNG encoder does not matter).
  - `UPDATE_SNAPSHOTS=1` writes the reference instead of comparing.
  - On a mismatch, a size change or a missing reference it writes
    `target/screenshot-diffs/{name}.actual.png` and `{name}.diff.png` (red
    where pixels differ, the reference at a quarter brightness elsewhere) and
    panics with the number of differing pixels and the first one's place.
- **Time.** The first frame in a process takes about two seconds, later ones
  about 100 ms (§10.5), and every `render_png` starts a fresh context, so keep
  the number of screenshot tests reasonable. The three tests of the crate take
  about a second together.
- **Determinism** is a test of its own (`render_is_deterministic` renders
  twice and compares bytes, and also that the image is not one flat colour).

### Window screenshots and driving a window

The main window's views need GPUI Kit, the forced theme and input, which a bare
`render_png` does not give. `testing` has three more things for them, used by
`tests/screens.rs` (and `interactions.rs` and `live.rs` later):

```rust
let image = render_window_png((1440, 900), &Theme::dark(), |window, cx| {
    cx.new(|cx| MainWindow::new(backend, None, window, cx))
});
assert_snapshot("shell_dark", &image);   // 2880 by 1800, about 150 KB
```

- **`render_window_png(size, &theme, build)`** is `Driver::new` and one
  `capture`. The harness initialises Kit (`app::init`, with Kit's icon assets),
  puts the view in Kit's `Root`, calls `cx.set_reduce_motion(true)` (Kit's
  scrollbar fade reads real time otherwise, and the pixels change from run to
  run) and **forces the theme** by calling `kit_theme::apply(&theme, cx)` after
  `build` ran. The test platform always reports the light appearance, so a view
  that reads the window's appearance has chosen light by then; the forced theme
  wins. `build` returns an `Entity<V>` (not an `AnyView`).
- **`Driver`** is the same window kept open: `Driver::new(size, &theme, build)`,
  then `click(point)`, `hover(point)`, `scroll(point, dy)` (positive `dy` shows
  later content), `type_text("...")`, `key("enter")` (GPUI keystroke syntax:
  `escape`, `cmd-f`), `advance(Duration)` (moves the test clock, runs the
  timers and tasks due, so the 500 ms and 1 s refreshes fire when you say),
  `resize((w, h))`, `draw()`, `capture()`, `update_root(|view, window, cx| ..)`,
  `read_root(|view| ..)` and `read(|app| ..)` (globals such as Kit's theme).
  Points are logical pixels from the top left of the window. Every input goes
  through `Window::dispatch_event`, the path a real event takes, so hit testing,
  hover and focus behave as they do for a user. Each call runs what is pending
  and draws a frame.
- **`headless_app()`** is the same app with no window (Kit initialised, motion
  reduced): `cx.new(..)` an entity, `update_entity` and `read_entity` it,
  `advance_clock(..)` and `run_until_parked()`. It is for the models that live
  in entities and are tested by moving the clock (`tests/live.rs`); it returns
  the `HeadlessAppContext` itself.
- **`FixedBackend`** (a `UiBackend` over a store you load, with
  `listening_status()` and UTC), and **`RecordingBackdrop`** (every
  `set_regions` call, `calls()`).
- The clock is the test dispatcher's, not the store's: a `ManualClock` store
  moves only when the test advances that clock too.

### The feature

`cargo test -p loctel-ui --features screenshot` is the spelled-out form. The
`screenshot` feature turns on GPUI's `test-support`, the platform crate's
`test-support` (which provides `current_headless_renderer`) and `gpui-pre-wgpu`
(the text system). It is off in the shipped app. `loctel-ui` lists itself as a
dev-dependency with the feature on, so every test, clippy and doc build in the
workspace has it, and `cargo test --workspace` (the gate, and what `ci.yml`
runs) runs the screenshot tests without any flag.

### Commands

```bash
cargo xtask screenshots            # the screenshot tests (and the rest of loctel-ui's)
cargo xtask screenshots --update   # rewrite the references (UPDATE_SNAPSHOTS=1), then list them
cargo xtask run-shot               # the real window under Xvfb, to target/run-shot.png
```

`cargo xtask` is an alias set in `.cargo/config.toml` for `cargo run -p xtask --`.

### Changing a reference

Only an intentional change to what a view draws may change a reference PNG,
and the same change must carry it. **View every new PNG before you commit it**
(the Read tool shows images) and check it shows what you meant, legibly. The
repo skill `update-screenshots` has the steps, and how to read a CI failure.

## The real window

`loctel` without `--headless` on Linux opens the status view in a window and
prints the same startup lines as headless mode (the listeners, then the rules
line), so a script that passed port 0 finds the ports. Closing the window quits. With no `DISPLAY` or
`WAYLAND_DISPLAY` it says so on stderr and runs headless: GPUI would otherwise
hand out windows that draw nothing.

`cargo xtask run-shot` builds `loctel` and runs it under `xvfb-run` (1280 by
800) with every port 0 and a scratch settings file. It waits for the window,
gives it two seconds (the first frame and one refresh), captures the whole
screen with `import -window root` and writes `target/run-shot.png`. A
capture of one flat colour means the window stayed black, the old X11
wake-up bug of crates.io GPUI (§10.5); the script then nudges the window with
`xdotool windowmove` and captures again, and fails if it is still blank. With
`gpui-pre` the window was not black on its first try, so the nudge is a safety
net. Open the PNG with the Read tool: the window sits in the black screen at
about (192, 100), 900 by 600.

The repo skill `run-and-screenshot` covers running with meshgen data.

## Unverified

- The main window's macOS parts: the transparent title bar with the traffic
  lights at (20, 20) over the sidebar's top strip, and how GPUI Kit's
  components look and behave on a Mac. Nothing of the main window has run on a
  Mac yet; the M3 check does it.
- Everything here was checked on Linux with software rendering. How the status
  view looks and behaves on macOS (fonts, scale, window chrome) is unverified
  until a milestone check on a real Mac; the macOS app opens no window in
  Milestone 1 anyway.
- The references were made with Mesa lavapipe on Ubuntu 24.04. If the CI
  runner's Mesa renders anti-aliasing differently, `ci.yml` uploads the actual
  and diff images as the `screenshot-diffs` artifact; see the
  `update-screenshots` skill.
