# UI (`loctel-ui`) and the screenshot workflow

The GPUI layer, kept thin on purpose: views draw what the engine reports and
hold no logic of their own. Design references: §9.2 (screens), §9.3 (look and
feel), §10.3 (screenshot tests) and §10.5 (gotchas from the spike).

Status: Milestone 3 is the main window. It has two screens (the home screen,
the service map above the trace table, and the trace view) in a shell with a
glass sidebar and toolbar, all in `src/app` and `src/views` over the pure models
in `src/model`; Milestone 4 adds a third, the Logs screen (Task 4, see "The Logs
screen"), and a fourth, the Discard rules screen (Task 5, see "The Discard rules
screen"), all three reached from the sidebar (Task 7: `Map & traces`, `Logs` and
`Discard rules`, ⌘1 to ⌘3, with ⌘, for Settings; see "Navigation" under "The main
window") and from each other (the trace view's `Open in Logs`, the Logs screen's
`Discard lines like this…`); "Screens and views at a glance" below is the map of
them, and
the sections after it say how each one works. The window opens from the Linux
`loctel` and from the macOS menu bar item's `Open loctel` (see "The real window"
below and [`macos.md`](macos.md)); the look of the glass parts is done and
tested here, and Apple's glass behind them is `loctel-macos`'s and unverified.
Milestones 1 and 2 had one view, the **status view**, which stays as a
standalone view with its own screenshot tests (Milestone 2 added the discard
rules line to it and to the macOS menu). Milestone 4 adds two windows of their own:
the Settings window (Task 6, see "The Settings window") and the menu bar popover's
view (Task 8, see "The popover"), which is cross-platform and tested here; what
opens it under the menu bar icon is `loctel-macos` (Task 9). The macOS menu bar
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
| `open_status_window(engine, cx)` | opens the 900 by 600 window on an `App`: dark theme, refreshed every second from `EngineHandle::status()`. Since Milestone 3 Task 9 nothing in the app calls it (`loctel` opens the main window instead); the view and its screenshot tests stay as a standalone view |
| `menu_text::status_line(&StatusModel)` | the one-line status: `Receiving · 412 spans/s · 1.2k logs/s`, `Paused` or `Port problem: otlp-http (address in use)`. A failed listener wins over paused, paused over receiving. The reason is `address in use` when the OS error contains "in use", otherwise the raw error cut to 40 characters. The first failed listener is the one named |
| `menu_text::rules_line(&StatusModel)` | the discard rules in one line, the first that applies: `Discard rules file has an error`; `1 discard rule on · 1 has an error` (for more than one, `3 discard rules on · 2 have errors`); `No discard rules on`; `1 discard rule on · 31% dropped` or `3 discard rules on · 0% dropped`. The percentage is the share the rules dropped of what arrived since launch, rounded to a whole number; a value that is not a number reads `0%` and one outside 0 to 1 is cut to it. A file error wins over a rule error, a rule error over "no rules on" |
| `menu_text::format_rate(f64)` | `412` is `412`, `1234` is `1.2k`, `0.4` is `0`. One decimal under ten thousand, none above, never `.0` (`1k`, `10k`, `1.3M`); NaN, infinity and negatives read `0` |
| `model` | the pure models of the main window: `format`, `colors`, `trace_table`, `query_bar`, `map_layout`, `waterfall`, `inspector`, `trace_view`, and (Milestone 4) `logs`, `histogram`, `rules`, `settings`, `popover` (see "Models" below) |
| `app` | the main window's shell (see "The main window"): `MainWindow`, `Screen`, `UiBackend`, `open_main_window`, `Backdrop`/`GlassRegion`, `kit_theme`; the home screen's live data (see "Live data"): `HomeModel`, `HomeData`, `TimeRange`; the open trace's (see "The trace view"): `TraceModel`; the Logs screen's (see "The Logs screen"): `LogsModel`, `LogsControls`; and the Discard rules screen's (see "The Discard rules screen"): `RulesModel`, `RuleEditor`, `PreviewState`; and `open_settings_window` (see "The Settings window") |
| `views` | the views of the main window's screens (see "The service map", "The trace table", "The trace view", "The Logs screen" and "The Discard rules screen"): `MapView`, with its `Camera`, `TraceTableView`, `TraceView` with its `WaterfallView`, `InspectorView` and `SpanLogsView`, `LogsView` with its `HistogramView`, `RulesView` with its `RuleEditorView`, and (see "The Settings window" and "The popover") `SettingsView` and `PopoverView` with its `PopoverActions` |
| `Theme` | the colours below. `Theme::dark()`, `Theme::light()` and `Theme::for_appearance(WindowAppearance)`. It is also a GPUI global once `kit_theme::apply` has run |
| `KitAssets` | the icons to register with `Application::with_assets`: GPUI Kit's default bundle (what its components draw) plus the list and the funnel, which the sidebar uses and the default bundle leaves out (decision 0040). `testing::headless_app` registers it too |
| `register_fonts(cx)`, `FONT_FAMILY` | the bundled typeface (below) |
| `testing` (feature `screenshot`) | `render_png` and `assert_snapshot`; for the main window `render_window_png`, `Driver` (click, drag, scroll, pinch, type), `headless_app`, `FixedBackend` (with `rule_set()`, the rules in memory, for a test that wants them to have dropped something, and `fail_bind` and `fail_save`, which make `apply_settings` report a listener `Failed` or fail to write), `RecordingBackdrop` (see "Screenshot tests") |

The strings in `menu_text` are the contract: the macOS menu header and the
status view's pill both use `status_line`, the macOS menu's rules header and the
view's rules row both use `rules_line`, and `crates/loctel-ui/tests/menu_text.rs`
pins them.

## Screens and views at a glance

The map of the main window: which screens there are, how a user gets from one
to the next, and which view and which model stand behind every part. Each part
has its own section further down, which is where the rules are.

### The screen map

`MainWindow` shows one `Screen` in its content area. The sidebar (with the
status box) and the toolbar are on every screen.

```
 sidebar:  Map & traces (⌘1)  |  Logs (⌘2)  |  Discard rules (⌘3)      ⌘,  opens the Settings window
              |  Map & traces:
              v
 Screen::Home ---------------------------------------------------------------.
   MapView        (top panel)     click a service  -> its filter in the query bar
   TraceTableView (bottom panel)  and the table; click it again, or empty space,
              |                   to take it off
              | click a row (TraceTableEvent::Open)
              v
 Screen::Trace(id)
   header         back button, root name, failure badge, indicator
   WaterfallView + SpanLogsView   (left)
   InspectorView                  (right)
              |
              | Esc, the `Back` button -> the screen the trace was opened from (Home, or Logs)
              | the sidebar entry, or ⌘1 to ⌘3 -> that screen
              | `Traces through this span name` -> Screen::Home with
              |   { name = "..." } written into the query bar
              | `Open in Logs` -> Screen::Logs { query: Some(`{service_name=..} | span_id = ..`) }
              '--------------------------------------------------------------'

 Screen::Logs { query }  (Milestone 4; sidebar `Logs`, ⌘2)
   LogsView   LogQL bar, level toggles, histogram, the list; toolbar: Live tail, range, Clear all
              |  expand a row, then:
              |    `Open trace` (or the trace link of a row) -> Screen::Trace(id), the log's span selected
              |    `Discard lines like this…`                -> Screen::Rules { draft: Some(..) }
              |    `Show lines around this one`              -> context mode, `Back to search` returns
 Screen::Rules { draft }  (Milestone 4; sidebar `Discard rules`, ⌘3)
   RulesView  header and `New rule`, banners, a card per rule (switch, name, chip, count, query);
              |  beside it, while a rule is open: RuleEditorView (fields, live validation, preview)
              |    click a card -> the editor opens on it; the switch toggles it and nothing else
              |    `Save` / `Cancel` / `Delete` (asks first); a file edited meanwhile -> the banner
              |    arriving with a draft (`Discard lines like this…`) -> the editor opens on it, unsaved

 The Settings window  (Milestone 4 Task 6: not a screen but a window of its own, 520 by 360;
                       ⌘, in the main window opens it (Task 7), the popover's `Settings…`
                       will (Task 9))
   SettingsView   three port inputs with the state of their endpoints, the login switch (macOS),
                  `Apply` and `Close`;  app::open_settings_window opens it or focuses the open one

 The menu bar popover  (Milestone 4 Task 8: a panel of its own, 336 wide, that `loctel-macos`
                        opens under the menu bar icon (Task 9); it replaces the M1 and M2 menu)
   PopoverView    title and status pill, four stats, the endpoints (`Copy` on MCP), the rules line,
                  |    the rules line         -> PopoverActions::open_main(Screen::Rules { draft: None })
                  |    `Open loctel`, ⌘O      -> open_main(Screen::Home)
                  |    `Settings…`, ⌘,        -> open_settings()      (both then close())
                  `Pause receiving`, `Clear all data…` (asks inline), `Start at login` (macOS),
                  `Quit loctel` ⌘Q;  every action goes through PopoverActions, so the view runs
                  and is tested without a Mac
```

| From | The user does | To |
|---|---|---|
| any | clicks `Map & traces`, `Logs` or `Discard rules` in the sidebar, or presses ⌘1, ⌘2 or ⌘3 | `Home`, `Logs { query: None }` or `Rules { draft: None }`: each as it was left |
| any | presses ⌘, | the Settings window opens, or comes forward; the screen stays |
| `Home` | clicks a row of the table | `Trace(id)` |
| `Trace` | Esc, or `Back` | the screen the trace was opened from: `Home` as it was left (query, toggles, rows, the map's camera), or `Logs` as it was left (its query, levels, tail, the line that was open) |
| `Trace` | `Traces through this span name` | `Home`, with `span_name_filter(name)` in the bar |
| `Trace` | `Open in Logs` | `Logs { query: Some(span_logs_query(service, span)) }` |
| `Logs` | `Open trace`, or a click on a row's trace link | `Trace(id)` with the log's span selected (`MainWindow::open_trace_at`) |
| `Logs` | `Discard lines like this…` | `Rules { draft: Some(suggest_from_log(record)) }`: the editor opens on it, unsaved |

The home screen's views and its `HomeModel` live as long as the window, so the
map keeps its camera and the table its query, toggles and rows while a trace is
open. The trace
screen is made the first time it is drawn and dropped when the window leaves it,
which also stops its refresh (it starts again from the initial selection on the
next visit).

**The toolbar's controls.** Design §9.2 puts a Live indicator, the time range,
Pause and Clear all in the home screen's toolbar, and they are there
(`HomeControls`, see "The main window": the toolbar). Each one calls the
`HomeModel` (`set_range`, `set_paused`, `clear_all`, see "Live data") and keeps
no state of its own, except whether the range menu is open. Decision 0032 has
the choices; decision 0031 has the two gaps of Milestone 3 that remain.

### Views and the models behind them

| View | File (`src/`) | Reads | Pure model | Section |
|---|---|---|---|---|
| `MainWindow`, sidebar, toolbar (`HomeControls`) | `app/{shell,sidebar,toolbar,nav}.rs` | `UiBackend::status()` once a second; `HomeModel`: range and pause | `StatusModel`, `menu_text`, `NavItem`, `toolbar_title`, `live_label`, `pause_label` | "The main window" |
| `MapView` | `views/map_view.rs` | `HomeModel`: the placed map | `model::map_layout` (with `colors`, `format`) | "The service map" |
| `TraceTableView` | `views/trace_table_view.rs` | `HomeModel`: rows, query state, toggles | `model::trace_table`, `query_bar` (with `format`) | "The trace table" |
| `TraceView` | `views/trace_view.rs` | `TraceModel`: header, load state | `model::trace_view` (`header`, `find_status`) | "The trace view" |
| `WaterfallView` | `views/waterfall_view.rs` | `TraceModel`: the `Waterfall` | `model::waterfall` | "The trace view" |
| `InspectorView` | `views/inspector_view.rs` | `TraceModel`: the span's details | `model::inspector` | "The trace view" |
| `SpanLogsView` | `views/span_logs_view.rs` | `TraceModel`: the span's logs | `model::trace_view` (`log_lines`) | "The trace view" |
| `LogsView` | `views/logs_view.rs` | `LogsModel`: rows, query state, levels, tail | `model::logs` | "The Logs screen" |
| `HistogramView` | `views/histogram_view.rs` | `LogsModel`: the bars | `model::histogram` | "The Logs screen" |
| `LogsControls` | `app/toolbar.rs` | `LogsModel`: tail, range | | "The Logs screen" |
| `RulesView` | `views/rules_view.rs` | `RulesModel`: cards, banners, the share dropped | `model::rules` (`rows`, `dropped_line`, `file_error_line`) | "The Discard rules screen" |
| `RuleEditorView` | `views/rule_editor_view.rs` | `RulesModel`: the editor session (draft, preview, banner) | `model::rules` (`Draft`, `draft_error_line`, `preview_text`) | "The Discard rules screen" |
| `SettingsView` | `views/settings_view.rs` | `UiBackend`: `settings()`, `status()` listeners once a second, `login_item()` | `model::settings` (`Form`, `endpoint_state`, `failures`) | "The Settings window" |
| `StatusView` | `status_view.rs` | `StatusModel` | `menu_text` | "What is in the crate" |

`HomeModel` (`app/data.rs`), `TraceModel` (`app/trace_data.rs`) and `LogsModel`
(`app/logs_data.rs`) are the three entities that read the store: one for the home
screen, one for the open trace, one for the Logs screen. `RulesModel`
(`app/rules_data.rs`) reads the engine's rules (in memory, once a second) and
reads the store only for the editor's preview.

### The data flow

```
 Engine (EngineHandle) --- AppBackend: UiBackend ---> store() / status() / clear_all() / utc_offset() / rules / settings
                                                   |
        HomeModel / TraceModel / LogsModel   read the Store on GPUI's background executor
        (every 500 ms; the trace every 2 s once complete), then on the foreground:
                                                   |
        model::*   pure: strings, rows, positions, colour indices (no GPUI)
                                                   |
        views::*   lay out what the models give them (Theme tokens for colour)
                                                   |
                          pixels (a window, or the headless renderer)
```

Views never name the engine: tests put a `FixedBackend` over a store loaded
from meshgen on a `ManualClock` in its place, which is what makes the screenshots
deterministic. What the user does goes back the same way, as a call on a model
(`set_query`, `select_service`, `select`, `toggle`), and the views redraw from
the model's new state.

## Models

`loctel_ui::model` holds everything the main window shows that is not a pixel.
The models are plain Rust over the store's types (`TraceSummary`, `FailedIn`)
and the TraceQL parser, with no GPUI in them: a colour is an index into
`Theme::service_palette`, and a view only lays out the strings and numbers it
is given. The rules of the design (the formats, the failure display of §9.4,
the colours of §9.3) are therefore tested with plain `assert_eq!`s, in
`tests/model_format.rs`, `tests/model_table.rs`, `tests/model_map.rs`,
`tests/model_waterfall.rs`, `tests/model_trace_view.rs` and, for the Milestone 4
screens, `tests/model_logs.rs`, `tests/model_rules.rs` and `tests/model_misc.rs`
(the Settings form and the popover).

| Model | What it is for | Entry points | Test file |
|---|---|---|---|
| `format` | durations, clock times, rates, error shares, cut-off text | `duration`, `clock`, `rate`, `error_pct`, `truncate_*` | `model_format.rs` |
| `colors` | the colour a service keeps for the run | `ServiceColors::index` | `model_table.rs` |
| `trace_table` | the trace table's rows and `Failed in` text | `rows`, `failed_in_text`, `count_line` | `model_table.rs` |
| `query_bar` | the TraceQL bar's state and the queries a click writes | `validate`, `service_filter`, `span_name_filter` | `model_table.rs` |
| `map_layout` | where each node and edge of the service map goes | `layout` | `model_map.rs` |
| `waterfall` | the trace view's rows, folding, find, errors path | `Waterfall` | `model_waterfall.rs` |
| `inspector` | the selected span's panel and what its buttons copy | `span_details`, `span_json` | `model_trace_view.rs` |
| `trace_view` | the trace header, the find count, a span's log lines | `header`, `find_status`, `log_lines` | `model_trace_view.rs` |
| `logs` | the Logs list's rows, an expanded line's details, the LogQL bar and its error line, `Open in Logs`, the level toggles' state, the count line, the context banner, how the live tail joins a page | `rows`, `details`, `validate`, `error_line`, `span_logs_query`, `level_is_on`, `levels_with`, `count_line`, `context_banner`, `merge_head` | `model_logs.rs` |
| `histogram` | the log volume bars and their four time labels | `bars` | `model_logs.rs` |
| `rules` | the rule cards and their counts, the total dropped, the editor's draft, the log suggestion | `rows`, `dropped_line`, `Draft`, `suggest_from_log` | `model_rules.rs` |
| `settings` | the Settings form's port validation | `Form::from`, `Form::validate` | `model_misc.rs` |
| `popover` | the menu bar popover's pill, stats, endpoints and rows | `from_status` | `model_misc.rs` |

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

`TraceRow::services_text()` is the names of the services joined with `, ` in the
order of the dots (the tooltip of the `Services` column), and
`count_line(shown, truncated)` is the line on the right of the toggle row:
`7 traces · newest first` (`1 trace` for one, `1,000 traces` at the limit), or
`Showing the newest 1,000` instead when more traces matched than the table
holds.

### Query bar (`model::query_bar`)

`validate(text)` reads the bar: `Empty` for text that is all whitespace,
`Valid(Arc<TraceQuery>)` (parsed once, ready for `TraceFilter::query`), or
`Invalid { column, message }`. The column counts from 1 in characters of the
text as typed (blanks included), and the message has no column in it:
`{ status = ` is `Invalid { column: 12, .. }`, the end of the text. A TraceQL
feature that loctel does not support yet is an `Invalid` too, never a quiet
mismatch. The bar keeps the last good results on an `Invalid` (Milestone 3
plan, review focus 4).

`error_line(&QueryState)` is the line under the input: `at column 12: expected
a value` for an `Invalid`, `None` for the other two. `EXAMPLE_QUERY`
(`{ status = error && duration > 1s }`) is the input's placeholder; a test pins
that it parses, so the hint never shows syntax the bar rejects.

`service_filter(service)` is `{ resource.service.name = "<name>" }`, written
into the bar when a service is clicked. `"` and `\` in the name are escaped
with a backslash, the only escapes a name needs, so any name gives a valid
query for exactly that service. `span_name_filter(name)` is `{ name = "<name>" }`,
written when the trace view's `Traces through this span name` is pressed,
escaped the same way (a test parses it for names with quotes, backslashes,
newlines and 300 characters).

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

**Routes.** `route_edges(&MapLayout)` says where each edge is drawn: an
`EdgeRoute` per edge, in the order of `edges`, with `start` (the caller's
right-edge middle), `segments` (`Line` and `Cubic` pieces; the last is a curve
that arrives level) and the helpers `end()`, `arrival()` (the direction of the
arrowhead), `label_anchor()` and `points(n)` (the route sampled, for tests);
`None` for an edge from a node to itself. "The service map" has the rules.

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

Three more functions serve the inspector's cut values and buttons.
`value_preview(value)` is the first `VALUE_PREVIEW_CHARS` (200) characters of a
value longer than that, `None` for one that fits (it counts characters, never
bytes). `exception_text(&ExceptionInfo)` is what the exception card's `Copy`
copies: `Type: message`, a line break and the stack trace as sent (just the part
there is when one is missing). `span_json(&TraceSnapshot, id)` is what `Copy span
as JSON` copies: the model's `Span` serialised, pretty-printed.

### Trace view text (`model::trace_view`)

What the trace screen says outside the waterfall and the inspector, all plain
strings pinned in `tests/model_trace_view.rs`:

- `header(&TraceSnapshot, utc_offset)` is a `TraceHeader`: `root` (the root
  span's name, or `(root not arrived)`) with `root_arrived`; `failure`;
  `summary`, `1.84 s · 15 spans · 5 services · 14:32:07.412` (the trace's
  length as the table's row has it, the span and service counts with a
  thousands separator and singular for one, and the start in the clock of the
  table); `trace_id` (whole, lower-case hex) and `complete`. `failure` is
  `Failure::FailedIn(text)` (the root is erroring; `text` is the table's `Failed
  in` text), `ErrorsInside` or `None`; `Failure::badge()` writes `Failed in
  payments-api · POST card-gateway +1`, `Errors inside`, or nothing.
  `completeness()` is `Receiving spans…` or `Complete`.
- `find_status(text_is_empty, matches, current)` is the count next to the find
  box: nothing for an empty box, `No matches`, `1 match` or `5 matches` while
  the selected span is not one of them, `2 of 5` while it is the second.
- `log_lines(&[LogRecord], utc_offset)` are the rows of a span's logs: `time`
  (the log's time, or its observed time when it has none), `level` (`level_badge`:
  `TRC`, `DBG`, `INF`, `WRN`, `ERR`, `FTL`, `LOG` for none), `severity` (what
  colours the badge) and `message` (the body on one line: a line break becomes a
  space). `logs_title(name)` is `Logs from POST /charges` and `logs_count(n)`
  `4 logs`.

### Logs screen models (`model::logs`, `model::histogram`)

Pinned in `tests/model_logs.rs`, over the fixed fake mesh's seven checkout logs.

- `rows(&[(LogId, LogRecord)], &mut ServiceColors, utc_offset)` are the list's
  rows in the order given (the store gives newest first). A `LogRow` holds `id`
  (the store's `LogId`), `time` (`format::clock` of the log's time, or of its
  observed time when it has none), `level` (a `LevelBadge`: `Err`, `Wrn`, `Inf` or
  `Dbg`, with `label()` `ERR`, `WRN`, `INF`, `DBG`), `service` and its colour
  index `color`, `message`, `trace_short` (the first 8 hex digits of the trace ID),
  `trace_id` and `span_id` (the last three are `None` for a log outside a trace).
- The badge groups the severities as the level toggles do: `ERR` is error and
  fatal, `WRN` warn, `INF` info and unspecified, `DBG` debug and trace.
- `message` is the body on one line: every line break (`\n`, `\r\n`, a lone `\r`)
  becomes ` ⏎ ` and breaks at the very end are dropped. The plan says "the first
  line only, newlines become ` ⏎ `"; the two cannot both hold, and the test
  "a multi-line body shown on one line" settles it: the whole body, flattened, so
  the mark has something to separate. The view cuts it to the width it has.
- `details(&LogRecord)` is the expanded line's grid: `("body", <the whole body>)`
  first, then `loctel_query::logql::labels` (sorted by name: the attributes with
  Loki's label names, `service_name`, `level`, `scope_name`, `trace_id`,
  `span_id`).
- `validate(text)` is a `LogQueryState`: `Empty` for blanks, `Valid(Arc<LogQuery>)`
  or `Invalid { column, message }` (the column counts from 1 in characters as
  typed, like the TraceQL bar's).
- `span_logs_query(service, span)` is `{service_name="payments-api"} | span_id =
  "00f067aa0ba902b7"`, what the trace view's `Open in Logs` writes into the bar;
  `"` and `\` in the service name are escaped.
- Task 4 added the rest of what the screen shows that is not a pixel:
  `PAGE_SIZE` 500, `MAX_ROWS` 20,000, `PREFETCH_ROWS` 50, `EXAMPLE_QUERY` (the
  bar's placeholder, pinned to parse), `error_line` (`at column N: message`, like
  the TraceQL bar's), `level_is_on(LevelSet, LevelBadge)` and `levels_with(set,
  badge, on)` (a toggle flips its own group of severities and nothing else),
  `count_line(loaded, more, capped)` (`1,204 lines · newest first`, `500+ lines ·
  newest first`, `20,000+ lines`), `context_count_line` (`51 lines · oldest
  first`), `footer(capped)` (`Showing the newest 20,000 matching lines`),
  `context_banner(target, held, utc_offset)` (`Lines around 08:00:01.440`, or `This
  line is no longer held`), `BACK_TO_SEARCH`, `NEW_LINES_PILL` and
  `merge_head(held, id, fresh)`, which says how the live tail's newest page joins
  the rows held (below, "The live tail").
- `histogram::bars(&[HistogramBucket], since, until, utc_offset)` is `Bars`:
  `bars` holds `(total, errors)` per bucket as fractions of the tallest bar's
  total (so the error stack is on the same scale, and never above its bar; all 0
  when no bucket has a log), and `labels` are the clock times at the start, a
  third, two thirds and the end of the range (cut to a millisecond, not rounded).

### Rules models (`model::rules`)

Pinned in `tests/model_rules.rs`.

- `rows(&[RuleStatus])` are the cards, in the file's order. A `RuleRow` holds
  `id`, `enabled`, `name`, `chip` (`TRACEQL` or `LOGQL`), `query`, `counts` and
  `error`. `counts` is the text on the right of the card, the first that applies:

  | Case | `counts` | `error` |
  |---|---|---|
  | switched off | `Off` | `None` |
  | on, query does not parse | `Error: <message>` (in `error_text`) | `Some(message)` |
  | trace rule | `48,210 spans dropped` | `None` |
  | trace rule, scope trace, whole traces dropped | `48,210 spans dropped in 602 traces` | `None` |
  | log rule | `96,433 lines dropped` | `None` |

  Counts have a thousands separator, are singular for one (`1 span dropped in 1
  trace`, `1 line dropped`) and plural for none (`0 spans dropped`). A rule that is
  off reads `Off` even with a bad query (as in the menu's rules line, an error only
  counts for a rule that is on), so `error` is set exactly when `counts` shows it.
  A span-scope rule never names traces.
- `dropped_line(fraction)` is `31% of incoming data dropped` (a whole percent,
  cut to 100), `Nothing dropped yet` for 0 (and for NaN, infinity and negatives,
  as `menu_text::rules_line` reads them) and `<1% of incoming data dropped` for a
  share that rounds to 0% but is more than nothing.
- `Draft` is the editor's fields (`name`, `language`, `query`, `scope`,
  `enabled`). `from_spec` and `to_spec` round-trip exactly (nothing is trimmed, so
  saving an unchanged rule keeps its identity). `validate()` is a `DraftCheck`:
  `name_ok` (the name has something in it) and `query` (`Ok(())`, or `Err((column,
  message))` from the draft's language's parser, the column counted from 1; an
  empty query is an error, as the rules engine says). `can_save()` is `name_ok`:
  a query that does not parse can be saved, per M2.
- `suggest_from_log(&LogRecord)` is the rule `Discard lines like this…` opens
  unsaved: name `Lines like "<first 40 characters of the body>…"` (no `…` when the
  body fits), LogQL, query `{service_name="<service>"} |= "<body text>"` where the
  body text is the first 60 characters cut back to the last whitespace if the cut
  falls inside a word (a single longer word is cut at 60), `"` and `\` escaped
  (also in the service name), and on. The body is its first line with any text,
  so a stack trace after the message does not end up in a filter no other line
  matches; a body with no text gives the selector alone. Lengths count characters.

### Settings form and popover (`model::settings`, `model::popover`)

Pinned in `tests/model_misc.rs`.

- `settings::Form` holds the text of the three port fields (`grpc`, `http`,
  `mcp`) and `start_at_login: Option<bool>`. `Form::from(&Settings, login)` fills
  the ports from the saved settings and the switch from what the system says
  (`login`, `None` where there is no login item: the settings file's own
  `start_at_login` is never read).
- `Form::validate()` gives the `Ports` or the problems per `Field` (`Grpc`,
  `Http`, `Mcp`, in that order). A field is digits only, 1 to 65535, blanks around
  it ignored, else `Enter a port from 1 to 65535`. Fields that are ports and share
  a number (as numbers: `4318` and `04318` are the same) each get `Each endpoint
  needs its own port`.
- `popover::from_status(&EngineStatus, login)` is the `Popover` (drawn by
  `views::PopoverView`, see "The popover"). `pill` is the
  first that applies, as in the status line: a failed listener `Port problem`
  (`Tone::Error`), paused `Paused` (`Tone::Warn`), else `Receiving` (`Tone::Ok`).
  `stats` are `(value, label)` for `spans per second` and `log lines per second`
  (`menu_text::format_rate`: `413`, `1.2k`) and `traces held` and `log lines held`
  (thousands separator: `1,200,000`). `endpoints` are `(name, address, ok)` for
  `OTLP gRPC` and `OTLP HTTP` (`127.0.0.1:4317`) and `MCP` (the URL
  `http://127.0.0.1:4319/mcp`, which the Copy button copies); a failed listener
  shows the address it was asked for with `ok` false. `rules_line` is
  `menu_text::rules_line`. `paused` and `start_at_login` pass through.
- The Settings window's endpoint lines (Task 6; in `tests/model_misc.rs` too).
  `Field::ALL` is the three fields in order, `Field::label()` the caption of
  the input (`OTLP gRPC port`, `OTLP HTTP port`, `MCP port`) and
  `Field::listener()` the engine's name for what it moves (`otlp-grpc`,
  `otlp-http`, `mcp`). `endpoint_state(&listeners, field)` is `listening` (ok) or
  `failed: <reason>` (not ok) for that field's listener, or `None` when the engine
  does not report it; the reason is `menu_text`'s: `address in use` when the OS
  error says "in use", otherwise the error cut to 40 characters. `failures(&listeners)`
  is one `otlp-http: address in use` line per failed listener, in the engine's
  order, empty when all are up.

Decision 0036 has the choices these models make where the plan is silent.

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
    traffic lights sit; plain room on Linux), the navigation (three entries,
    `Map & traces`, `Logs` and `Discard rules`, each `NAV_ITEM_HEIGHT` 34 high with
    `NAV_ITEM_GAP` 2 between; an icon (16 px) and the text; the one that is lit has
    the accent at 18% behind it, the text in semibold and the icon in the accent
    colour; see "Navigation" below) and, at the bottom, the **status box**.
  - The status box shows `status_line` and `rules_line` (the words of the macOS
    menu) and a row per endpoint (`OTLP gRPC`, `OTLP HTTP`, `MCP`) with its
    address, a red dot and address when it failed. It reads a `StatusModel` from
    `UiBackend::status()` once a second and redraws only when it changed.
  - To its right, the **toolbar** (`TOOLBAR_HEIGHT` 48) over the **content
    panel**. The toolbar's contents depend on the screen: its title is
    `toolbar_title(&Screen)`: `Map & traces`, or
    `Map & traces › 4bf92f35…0e0e4736` on a trace. On the home screen the four
    controls of design §9.2 are at its right (below); the trace screen has the
    title alone.

**The toolbar's controls** (`HomeControls`, `app/toolbar.rs`; the shell owns it
as `main.toolbar()`). Left to right, 26 px high:

| Control | Looks | A click |
|---|---|---|
| Live | a pill: a green dot and `Live`; an amber dot and `Paused` while the view is frozen (`live_label(paused)`) | nothing (it is an indicator) |
| time range | an outline button with the range (`Last 15 minutes`, `TimeRange::label`) and a caret | opens a menu of the four ranges (`TimeRange::ALL`, the current one lit with the accent); a choice calls `HomeModel::set_range` and closes the menu; a click outside closes it and changes nothing |
| Pause | an outline button with the pause icon and `Pause`; while frozen the accent-filled `Resume` with the play icon (`pause_label(paused)`) | `HomeModel::set_paused(!paused)`: the map and the table stop reading while ingest goes on; `Resume` reads at once |
| Clear all | an outline button with `Clear all` | `HomeModel::clear_all`: empties the store through `UiBackend::clear_all` and reads at once, even while frozen. No confirmation (design §5: "Clear all empties everything immediately"; the macOS menu's `Clear all data…` asks, the window's button is a developer's reset). The filters, the range and the pause stay |

The controls hold no data: the model owns the range and the pause, and the
controls redraw when it changes (`cx.observe`), so a range set from anywhere
shows on the button at the next frame. The range menu is Kit's `Popover`, opened
and closed by the controls (`range_open()`), drawn over the content like any
popup. The Pause button freezes the *view*; the macOS menu's `Pause receiving`
stops *ingest* in the engine and is a different thing (the status box says
`Paused` for that one). For tests, `bounds_of(ToolbarPart)` and `center_of` say
where each part was drawn in the latest frame (`Live`, `Range`, `RangeOption(r)`
while the menu is open, `Pause`, `ClearAll`), `live_text`, `pause_text` and
`range_open` read what they show, and `tests/interactions.rs` ("the toolbar")
clicks them: Pause freezes the table and the map while the store goes on
receiving and `Resume` catches up, the range menu lists the four ranges and each
choice changes the rows, a click outside closes the menu, Clear all empties the
map, the table and the store (and works while frozen).

**Screens.** `Screen` is `Home`, `Trace(TraceId)`, `Logs { query: Option<String> }`
or `Rules { draft: Option<Draft> }` (the last two are Milestone 4's);
`MainWindow::screen()` and `navigate(screen, cx)` read and change it, and
clicking a navigation entry shows its screen (see "Navigation"). The home screen's views live as long
as the window; the trace screen is made when it is first drawn and dropped when
the window leaves it (see "The trace view"). The Logs screen's model and view
live as long as the window too (`main.logs()`, `main.logs_view()`, made in
`MainWindow::new`), so its query, toggles and tail stay while another screen
shows; the model reads the store only while `Screen::Logs` is the one showing
(`navigate` tells it). `navigate(Screen::Logs { query: Some(q) })` puts `q` in the
LogQL bar every time, even when the Logs screen is already showing and the user
typed something else since. `open_trace_at(id, Some(span), cx)` is `navigate` to
the trace of `id` with `span` selected once the trace has it
(`TraceModel::prefer_selection`). The Discard rules screen's model and view live
as long as the window as well (`main.rules()`, `main.rules_view()`), so an
editor with a draft in it stays while another screen shows; the model reads only
while `Screen::Rules` is the one showing, and reads at once when it comes back.
`navigate(Screen::Rules { draft: Some(d) })` opens the editor on `d`, unsaved,
every time (a draft that was open is replaced); `draft: None` shows the screen as
it was left. The toolbar's title is `toolbar_title`:
`Logs` and `Discard rules` join the two above, and on the Logs screen the
controls at its right are `LogsControls`.

**Navigation** (`app/nav.rs`, `app/sidebar.rs`; M4 Task 7, decision 0040).

| Entry | Icon (Lucide, `gpui_kit::assets::IconName`) | Key | Screen a click shows | Lit on |
|---|---|---|---|---|
| `Map & traces` | `Map` | ⌘1 (`cmd-1`) | `Home` | `Home` and every `Trace(_)` |
| `Logs` | `List` | ⌘2 | `Logs { query: None }` | `Logs { .. }` |
| `Discard rules` | `Funnel` | ⌘3 | `Rules { draft: None }` | `Rules { .. }` |

`NavItem` (`ALL`, `label`, `icon`, `shortcut`, `screen`, `is_active`) is the one
list the sidebar, the keys and the tests read. A trace stays under `Map & traces`
even when it was opened from a log (the toolbar says `Map & traces › <id>` there
too); only `Back` knows where it came from.

- **State is kept.** Every screen's views and model live as long as the window
  (the trace screen apart), so switching screens loses nothing for the session:
  the home query, toggles and camera, the Logs query, levels, range, tail and
  expanded line, and an open rules editor with its draft. An entry's `screen`
  carries no query and no draft, which is what makes it "as it was left";
  `Screen::Logs { query: Some(..) }` and `Screen::Rules { draft: Some(..) }` are
  the ways to arrive *with* something, and they replace what was there.
  (`sidebar_switches_screens_and_keeps_state`)
- **Keys.** ⌘1, ⌘2, ⌘3 and ⌘, are GPUI actions (`ShowMapAndTraces`, `ShowLogs`,
  `ShowRules`, `OpenSettings`) bound by `app::init` in the key context
  `MainWindow`, which the shell's root element carries, so they work in the main
  window and nowhere else (the Settings window has no such context). They are
  `cmd-…` on every platform (the Super key on Linux, where
  `xdotool key super+2` sends one). Chords with the command key are not typed
  into an input. The root takes the keyboard at creation, and back whenever what
  had it leaves the frame (`Context::on_focus_lost`: GPUI calls it after a frame
  in which nothing is focused any more, for instance after a click in the table's
  empty area and a switch to Logs, or the dropped trace screen); a click on bare
  window gives it to the root too, since the root tracks focus. So the keys do not
  depend on where the last click was.
  (`keyboard_shortcuts_switch_screens`, `shortcuts_work_after_a_click_in_a_part_of_a_screen_that_goes_away`,
  `the_settings_shortcut_opens_one_settings_window`)
- **Back.** The trace screen's back button says `Back` (`BACK_LABEL`) and so does
  Esc: it returns to the screen the trace was opened *from*, which the shell notes
  when a trace opens from another screen (`trace_origin()`): `Home` for a click on
  a row of the table, `Logs` for `Open trace` or the trace link of a log. What
  comes back is `NavItem::screen()`, so the user's own query in the Logs bar is not
  replaced. A trace opened from Logs, then left by the sidebar, then another trace
  opened from the table, goes back to the table: the origin is noted at each
  opening. (`trace_back_returns_to_logs_when_opened_from_logs`,
  `trace_back_returns_home_when_opened_from_home`)
- **For tests.** `MainWindow::nav_bounds(item)` is where an entry was drawn in
  the latest frame (like the other views' `bounds_of`), `trace_origin()` the
  screen `Back` returns to.
- **Not verified on a Mac:** that ⌘1 to ⌘3 and ⌘, reach the shell from a real
  keyboard (the tests send the keystrokes through GPUI's own dispatch) and do
  not collide with something the system does with them (macos.md, "Unverified
  until the M4 Mac check").

**Lifecycle.** The window is opened by `loctel-app`, not by the shell. On Linux
`loctel` without `--headless` opens it at launch; on macOS the menu's `Open
loctel` does (`loctel_macos::open_or_focus`), which also puts loctel in the Dock
for as long as the window is open. See "The real window" below and
[`macos.md`](macos.md).

**The backend.** Views never name the engine: they hold an `Rc<dyn UiBackend>`.
Its methods:

| Method | What it is |
|---|---|
| `store() -> Store` | the store the receivers fill (a clone is the same store) |
| `status() -> EngineStatus` | pause, endpoints, rates, what is held and the rules |
| `clear_all()` | empties the store |
| `utc_offset() -> i32` | the system zone's offset in seconds for the clock times; tests use 0 |
| `rules() -> Vec<RuleStatus>` | the discard rules in file order, with errors and counts |
| `set_rules(Vec<RuleSpec>)`, `set_rule_enabled(RuleId, bool)` | replace or switch, saved to `rules.toml`; an invalid query is allowed and shows as that rule's error |
| `preview_rule(&RuleSpec) -> Result<Preview, QueryError>` | what the rule would have dropped of the last 5 minutes |
| `rules_generation() -> u64` | how many times the rules were replaced (a save, a switch or a reload of an edited file); the rules editor compares it to notice a change made behind its back |
| `settings() -> Settings` | the saved `settings.toml` (defaults when there is none) |
| `apply_settings(Settings) -> Result<Vec<ListenerStatus>>` | saves, then rebinds the listeners (`EngineHandle::rebind`); a port that cannot be bound is saved and shows `Failed` in the statuses that come back, an `Err` is a file that could not be written (and then nothing moved) |
| `login_item() -> Option<Rc<dyn LoginItem>>` | start at login: `Some` on macOS, `None` on Linux (no switch in Settings) |
| `set_paused(bool)`, `mcp_url() -> Option<String>` | pause and resume; where agents reach MCP |

`AppBackend::new(engine, settings_path, login)` is the real one (the old
`impl UiBackend for EngineHandle` is gone: every caller uses it). `loctel-app`
builds it for the Linux window with no login item, and `loctel-macos`
(`window::open_or_focus`) with `login_item::SystemLoginItem`. Its `settings()`
reads `settings.toml`, not the engine's own settings: a command-line port flag
(`--http-port 0`) changes what the engine listens on, not the file.

`testing::FixedBackend` is the test one; `FixedBackend::new(store)` gives the
status of a healthy engine (`testing::listening_status()`: three listening
endpoints, nothing received, no rules). Everything the window can change is
kept in memory: the rules in a `RuleSet` (so IDs, query errors and
`rules_generation` behave as in the engine, and `preview_rule` runs
`loctel_rules::preview` over its store), the settings (`apply_settings` makes up
three listening listeners at the ports it is given and binds nothing), the pause,
and `pub login` for a test that wants the Start at login switch. `rule_set()` is
the `RuleSet` itself, for a test that wants the rules to have dropped something
(pass data through `filter_spans` and `filter_logs` as the engine's sink does, and
the cards' counts move; the share in `status.dropped_fraction` does not: it is the
`status` field's). A change of the rules behind a view's back is `set_rules`,
which moves `rules_generation` as a reload of the file does. `status()` is
its `status` field with the rules, the pause and the listeners of those calls
laid over it. `tests/app_backend.rs` pins both (`AppBackend` over a real engine
on port 0: saving settings and rebinding, a busy port, a file that cannot be
written, the rules calls, pause and the login item; `FixedBackend` in memory),
and `crates/loctel-engine/tests/rebind.rs` pins `rebind` and `rules_generation`
underneath. Store reads that take time go through `store()` clones onto the
background executor, never on the frame.

**Opening it.** `open_main_window(backend, cx, attach_backdrop)` calls
`app::init(cx)` (the bundled font and `gpui_kit::init`, once per app), opens
the window with the options above and builds `MainWindow` inside GPUI Kit's
`Root`. `attach_backdrop` runs inside the new window and returns the
platform's `Backdrop` (macOS Liquid Glass, `loctel_macos::glass::attach`) or `None`. It leaves
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
and applies them later (macOS: `loctel_macos::glass`, from the main queue;
decision 0030).

**With a backdrop (the glass look).** Giving `MainWindow::new` a backdrop is
what makes the sidebar and the toolbar glass parts. The platform that attaches
one has made the window transparent, and the shell then leaves exactly those two
parts clear:

- The sidebar and the toolbar have **no fill**: a 1 px hairline
  (`Theme::glass_hairline`) round them and, just inside it, a 1 px ring of
  highlight (`Theme::glass_highlight`, concentric, radius 15). Both are
  translucent, so the glass shows through them.
- The **window colour** is painted by a canvas (`window_colour`) as one even-odd
  path: the window rectangle with a rounded hole (`PANEL_RADIUS`) for each region
  measured in that frame. So the gaps, the margin and the slivers outside the
  rounded corners keep the colour, and nothing is painted over the glass. The
  root `div` has no background of its own then.
- Kit's `Root` paints `Theme::bg` over the whole window (its window state
  plugin's style). `MainWindow::new` clears it, deferred one effect cycle (the
  `Root` is made after the view); a window with no `Root` is left alone.

Without a backdrop (Linux, or the macOS `Blurred` fallback) none of that
applies: the panels have `Theme::panel` and `Theme::border`, the root has
`Theme::bg`, and every pixel of the window is painted.

The headless renderer cannot draw glass, and its capture is always opaque (it
clears to black), so the tests put the window over a `Desktop` view that paints
a flat stand-in colour (`desktop(theme)` in `tests/screens.rs`: dark blue on the
dark look, pale blue on the light one) and read pixels:
`glass_parts_show_what_is_behind_and_the_rest_does_not` (what is clear, what is
solid, the hairline and the highlight), `without_a_backdrop_the_panels_have_their_fill`,
and the references `shell_glass_dark.png` and `shell_glass_light.png`. Their
glass areas show the stand-in colour; on a Mac they show Apple's glass.

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
| `clear_filters()` | takes off what `has_filters()` counts: the query is emptied (and a selected service is no longer), `Errors only` and `Slower than 1 s` go off; one read. The range and the pause stay |
| `select_service(Some(name))` | writes `service_filter(name)` into the query and remembers the selection (`selected_service()`) |
| `select_service(None)` | clears the selection: the query goes back to empty, but only while it is still that service's filter; a query the user typed is left alone |
| `set_paused(bool)` | freezes the timer (below); un-pausing reads at once |
| `clear_all()` | empties the store through `UiBackend::clear_all` and reads at once, whatever pause says; the filters, the range and the pause stay (the toolbar's `Clear all`) |
| `refresh_now()` | reads now, whatever pause says |

The table's filter is the range, errors only, slow only and the valid query
together, all of which must hold, with a limit of 1,000. **The map is filtered
by the range alone**: it is the picture of everything in the window, and the
table is the list of what was asked for. Reading accessors for the controls:
`query_text()` (as typed), `selected_service()`, `range()`, `errors_only()`,
`slow_only()`, `paused()`; `has_filters()` (a query that runs, `Errors only` or
`Slower than 1 s`: the range is a window, not a filter) and `loaded()` (the
first read is back, so an empty `data()` means an empty store and not "not read
yet": the table says nothing until then, rather than `No traces yet`).

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

**Rates on the map** are counts over the time the data covers: from the oldest
trace in the store (`Store::oldest_start_unix_nano`) to now, capped at the
range (300, 900 or 3,600 seconds; no cap for `Everything`) and at least a
second. A loctel that started a minute ago divides by that minute, not by the
whole 15 minutes, so a steady 2 req/s reads `2.0 req/s` from the start
(decision 0034). An empty store has zero counts, so zero rates.

**Colours.** The model owns the session's `ServiceColors` and the map's
`LayoutMemory`. The rows are built before the map, so the first services of the
newest traces get the first colours. `home.colors()` returns the shared
`Rc<RefCell<ServiceColors>>`: a view that colours services (the trace view's
waterfall, the logs) takes it from there, so a service has one colour
everywhere for the run.

## The service map

`views::MapView` (`src/views/map_view.rs`, Milestone 3 Task 6) is the top half
of the home screen: the live service map, drawn on one GPUI `canvas` from
`HomeData.map` (the placed layout of "Map layout" above). Decision 0026 has the
choices. `MainWindow` owns the `HomeModel` and the `MapView` over it
(`main.home()`, `main.map()`), so they live as long as the window: going to a
trace and back keeps the map's place.

**Layout of the home screen.** The content area is a column of two floating
panels (radius 16, `INSET` apart): the map panel, `MAP_SHARE` = 52% of the
height (427 px in a 1440 by 900 window, enough for the checkout's 416), and
under it the panel of the trace table ("The trace table" below), which fills the
rest. The Trace screen keeps its one panel.

**What is drawn** (all colours are `Theme` tokens):

| Part | Look |
|---|---|
| node | a rounded quad, radius 14, 176 by 64, filled `panel`, 1 px `border`; the name in semibold 13 and the subtitle (11) cut with `…` to the room left of the dot |
| dot | a circle in the service's palette colour for a service; a grey (`text_tertiary`) rounded square for a database, a queue or an external host |
| failing node | a 2 px `error` border and an `error_text` subtitle |
| not a service | a dashed border |
| selected service | a 3 px `accent` ring just outside the node |
| edge | a 2 px line from the caller's right-edge middle to the callee's left-edge middle along the route of `route_edges` (below), ending in a filled arrowhead (10 long, 10 wide) that points right into the callee; `text_tertiary` at 55% normally, `error` when failing; failing edges are drawn last. An edge to the same node is not drawn |
| edge label | the layout's `label` (`84 req/s`, only on edges out of the first column) in a small pill (`panel` fill, `border`) above the middle of the edge's first curve, over the nodes |
| empty map | `Waiting for telemetry` and `Point OTEL_EXPORTER_OTLP_ENDPOINT at http://localhost:4317` (`EMPTY_TITLE`, `EMPTY_HINT`), centred, instead of the canvas and the Fit button |

**Where an edge goes** (`model::map_layout::route_edges`, decision 0033: pure and
tested without a window, the view only draws what it says). Nodes are drawn after
the edges, so an edge that went under a node would show only as a stray piece
between two others, looking like a call that does not exist; none does:

- **To the next column**: one S-shaped cubic bézier, level at both ends (control
  points half way, at least 48 px out). This is every edge of the checkout but
  one.
- **Over one column or more** (the checkout's `orders-api` to `postgresql`): a
  curve into a *lane*, a straight line along it and a curve out of it into the
  callee. A lane is a gap between two rows, 32 px high, which no node is ever in,
  in every column; the one nearest the middle of the two ends is taken (the
  gap above the first row and the one under the last are lanes too). The line
  starts at the left edge of the first column in between and ends at the right
  edge of the last, and the arrowhead is level into the callee.
- **To the same column or back** (a node keeps its place while its calls change,
  so this happens, and so does a loop `a` calls `b` calls `a`): a bulge out of the
  caller's right edge, a lane back to the left past the callee, and a bulge into
  its left edge from outside.
- **Edges that share a lane** are spread over it, 0, -5, +5, -8, +8 px from its
  middle, in the order of the edges, so two lines never lie on one another.

Every route stays inside the picture, under no node and at least 6 px from every
node it does not join (`tests/model_map.rs`, "where the edges are drawn": the
checkout, 300 random maps and 150 growing ones with remembered places).

**The camera** (`Camera { zoom, pan_x, pan_y }`, pure and tested without a
window): a picture point `(x, y)` shows at `(pan + (x, y) x zoom)` from the
view's top left corner. A new map is *fitted*: `Camera::fit(view, picture)` is
zoom 1 from the corner, or the scale that fits when the picture is bigger,
never below `MIN_ZOOM` (0.5) and never above 1, and never centred, so the map
does not move as services appear. A fitted map follows the window (`resize`).
The first pan or zoom leaves the fit (`is_fitted()` is false) and the camera
stays where the user put it, through resizes; the `Fit` button (top right) or
`fit()` goes back. Zoom is `MIN_ZOOM` to `MAX_ZOOM` (0.5 to 2.0); text and
strokes scale with it.

**Input** (the view registers its handlers on the map's area; the Fit button
occludes its own):

| Input | Effect |
|---|---|
| drag (left button) | pans, once the pointer has moved 3 px; the pointer can start on a node |
| ctrl or cmd plus the wheel | zooms by `e^(0.005 x pixels)` around the pointer (the wheel turned up zooms in); a wheel in lines counts 20 px a line |
| a pinch | zooms by `1 + delta` around the pointer |
| the wheel alone | pans (a two-finger drag on a trackpad) |
| a press that moved under 3 px (a click) on a service | `HomeModel::select_service(Some(id))`: the query bar gets `{ resource.service.name = "..." }` and the table follows; the node gets the ring |
| a click on the selected service, or on empty space | `select_service(None)`: clears the selection (a query the user typed is left alone) |
| a click on a database, a queue or an external host | nothing: they have no filter |

**For tests.** `node_center(id, cx)` (where a node is in the window, from the
latest frame: the layout through the camera), `camera()`, `is_fitted()`,
`view_bounds()`, `fit_button_center()` and `empty_message(cx)`.
`tests/interactions.rs` clicks, drags, scrolls and pinches the real window;
`tests/common/mod.rs` has `home_store()`, the data of the home screenshots (the
failing and the successful checkout and a health check of each service, on a
store clock moved 70 s after the traces so every trace is complete and
`card-gateway` is a node).

**References:** `home_dark.png` and `home_light.png` (the whole home screen),
`home_payments_selected_dark.png` (after a click on `payments-api`: the ring on
the node and the service's filter in the query bar and the table), and
`shell_dark.png` and `shell_light.png` (the home screen of an empty store: the
empty map message and `No traces yet`). The trace table in them is "The trace
table" below. `shell_glass_dark.png` and `shell_glass_light.png` are the same
window with a backdrop attached, over a flat stand-in for the desktop (see "The
main window", "With a backdrop"): the sidebar and the toolbar show the stand-in
colour through them, with the hairline and the highlight round them, and
everything else is as above. The toolbar's controls are in all of them, and
`toolbar_*.png` are crops of the strip (running, with the range menu open, and
frozen), see "The main window".

## The trace table

`views::TraceTableView` (`src/views/trace_table_view.rs`, Milestone 3 Task 7) is
the lower half of the home screen: the TraceQL bar, the two toggles and the
table of traces. Decision 0027 has the choices. `MainWindow` owns it
(`main.table()`) and shows the trace's screen when it emits
`TraceTableEvent::Open(id)`, so the view does not know the shell. It holds no
data: the rows, the query's state and the toggles are the `HomeModel`'s, and
the view writes what the user does into the model and draws what the model
says. (The one thing it keeps is the input's text, which the input owns: the
model's text is copied into the input when something else wrote it, such as a
click on the map, and the input's goes to the model as it is typed.)

**The panel**, top to bottom (the panel is the content width, 1188 px inside its
hairlines at the default window):

| Part | What |
|---|---|
| query row | a `TraceQL` chip (accent at 16%), a GPUI Kit `Input` with a clear button, in the monospace face and with `EXAMPLE_QUERY` as its placeholder, and a primary `Run` button |
| error line | only while the text does not parse: `at column N: message` in `error_text`, 12 px, under the input |
| toggle row | Kit `Toggle`s `Errors only` and `Slower than 1 s` (outline; lit with the accent when on) and, on the right, `7 traces · newest first` or `Showing the newest 1,000` |
| table | a Kit `DataTable`, 6 columns, header 32 px, rows 36 px, no sorting, no resizing or moving of columns, no row selection |

**Typing validates and applies live.** Every change of the input goes to
`HomeModel::set_query`: an empty bar or a query that parses filters the table
at once, one that does not leaves the last good rows (the table never turns into
an empty table that looks like "no traces") and the line under the input says
where. Enter and `Run` read the store again (`refresh_now`), which is the way to
see new traces while paused. The input's clear button empties the query.

**Columns** (`COLUMNS`; `Started`, `Spans`, `Duration` and the dots are fixed
widths):

| Column | Width | Cell |
|---|---|---|
| `Started` | 116 | `TraceRow::started`, the monospace face, 12 px, `text_secondary` |
| `Trace` | about 40% of the rest | the root span's name, semibold; `(root not arrived)` in `text_tertiary` while it has not; cut with an ellipsis |
| `Failed in` | the other 60% | `failed_in` in `error_text`; or `errors inside` in `warn` (amber); empty for a trace that is fine |
| `Services` | 92 | an 8 px dot per service in its colour (`Theme::service_palette`, from the row's colour index), at most 5 and then `+N`; the tooltip is `services_text()` |
| `Spans` | 60 | the span count, right-aligned |
| `Duration` | 140 | the text, and under it a 4 px bar as long as the row's `duration_fraction` (at least 2 px): `error` for a failed trace, `warn` with errors inside, `accent` otherwise |

`column_widths(inner)` gives the six widths for a table `inner` pixels wide:
`Trace` and `Failed in` share what the four others and Kit's 12 px last column
leave (never less than 140 and 170), so the table fills its panel and has no
horizontal scroll down to the smallest window (`inner` of 730; the minimum
window's panel is 748). The width comes from the window's width
(`content_width`), not from a measurement, so the first frame is right.

**Rows.** A click on a row (anywhere in it) emits `TraceTableEvent::Open` for its
trace, and the shell navigates to `Screen::Trace(id)`. A row is not
*selected* (the table's own selection is off): rows arrive at the top while the
table is open and a selected index would point at another trace a moment later.

**Empty.** With no rows: `No traces yet` while nothing is filtered
(`HomeModel::has_filters()` is false); `No traces match` and a `Clear filters`
button (`HomeModel::clear_filters`) when something is. The header stays. Until
the first read is back the table says nothing.

**For tests.** `bounds_of(part)` and `center_of(part)` give where a `TablePart`
(`QueryInput`, `Run`, `ErrorsOnly`, `SlowOnly`, `Table`, `ClearFilters`) is in
the window, measured by a canvas over each part in the latest frame (`None`
when it is not there, as `ClearFilters` is while the table has rows);
`row_center(index)` is the middle of a row of the unscrolled table. Also
`input_text`, `error_line`, `empty_state`, `count_line` and `column_widths`.
`tests/interactions.rs` types into the real input, toggles, clicks rows,
`Run`, the clear button and `Clear filters`, and hovers the dots.

**References:** `home_errors_only_dark.png` (after a click on `Errors only`: one
row, the toggle lit), `home_query_error_dark.png` (`{ status = ` typed: the error
line under the focused input and the seven rows kept), `home_no_match_dark.png`
(a query that matches nothing: `No traces match` and the button), and the
`home_*` and `shell_*` ones above. The monospace face is the theme's
(`mono_font_family`); on Linux nothing monospace is bundled and the harness has
no system fonts, so the references show it in Inter. How it looks on a Mac is
unverified.

## The trace view

`views::TraceView` (`src/views/trace_view.rs`, Milestone 3 Task 8) is the content
of `Screen::Trace(id)`: one trace as a waterfall, with an inspector for the
selected span and that span's logs. Decision 0028 has the choices. The window
makes it the first time it draws the screen (`main.trace_view()`; `navigate`
takes no `Window`, and the view's inputs need one) with the home screen's
service colours, so a service keeps its colour from the map and the table, and
drops it when the window goes to another screen, which also stops its refresh.
`TraceView::new(backend, trace_id, window, cx)` is the same view with colours of
its own; `with_colors` is what the window calls.

**The data** is `app::TraceModel` (`src/app/trace_data.rs`), an entity the
four views observe, like `HomeModel` for the home screen: the trace's
snapshot, the `Waterfall` built from it, the selection, the find text and
matches, the selected span's details and logs. What the views do goes back into
it (`select`, `toggle`, `collapse_all`, `expand_all`, `set_errors_path_only`,
`set_find`, `find_next`), and everything else follows from the selection.
`tests/live.rs` pins the rules below with a trace screen as a window's root
view and a store whose clock the test moves.

- **While the trace is arriving** (the store has not called it complete) it is
  read every 500 ms, and every 2 s once it is. The wait for the next read starts
  when the last one came back, with the interval the trace's new state calls for.
  A new snapshot goes through `Waterfall::rebuild`, which keeps the folded spans,
  the selection and the errors filter by span ID: rows arrive and the
  `missing parent` row goes away when the root comes last, and nothing the user
  did is lost (Milestone 3 plan, review focus 1; `open_trace_fills_in_as_spans_arrive`).
- **Reads never block a frame.** The trace and a span's logs are read on GPUI's
  background executor and applied on the foreground, where the pure models run
  (the header, the waterfall, the details). A complete trace that has the spans
  the store holds is only counted (`Store::with_trace_spans`, under the read
  lock, no copy), so a 100,000-span trace is not copied every two seconds.
  Building the waterfall of such a trace is on the foreground (about 0.3 s in a
  debug build, a fraction of that in release); decision 0028 has why.
- **Selection** starts at `initial_selection` (the failed-in span, else the root)
  as soon as there is one, and the list scrolls to it
  (`TraceModelEvent::Reveal`). `prefer_selection(span)` (what `Open trace` of a
  log calls on a model just made) puts a span of the caller's choice in its place
  once the trace has it, and waits for it while the trace is still arriving
  (the failed-in span stands in meanwhile). After that it is the user's: a click
  on a row, or Enter in the find box, which also ends a pending preference. The selected span's logs are read from the store
  (`logs_for_span`) when the selection moves and with every refresh.
- A trace the store does not have says `This trace is not in the store`
  (`LoadState::Missing`) and keeps polling, so one that arrives shows up.

**The screen**, top to bottom (the panel is the content width, 1188 px at the
default window):

| Part | What |
|---|---|
| header, first line | the `Back` button (Esc does the same; it returns to the screen the trace was opened from), the root span's name (semibold, cut), the badge and, on the right, the indicator |
| header, second line | `1.84 s · 15 spans · 5 services · 14:32:07.412`, the trace ID in the monospace face cut in the middle (`c7aad7a8…77315102`) and a `Copy ID` button |
| badge | red `Failed in service · span name peer +N` with a `!` for a failed trace, amber `Errors inside` for one with errors under a fine root; none otherwise |
| indicator | `Receiving spans…` in the accent colour while the trace may still get spans, `Complete` in green |
| body | a GPUI Kit horizontal split: the waterfall over the logs on the left, the inspector on the right, 340 px at first and draggable from 280 to 520 (`INSPECTOR_WIDTH`, `INSPECTOR_MIN_WIDTH`, `INSPECTOR_MAX_WIDTH`); the waterfall's side is never under 380 |

**The waterfall** (`WaterfallView`, `waterfall_view.rs`). A toolbar with the
`Find` input (220 px), its count (`find_status`), the `Errors path only` toggle
and, on the right, `Collapse all` (`Expand all` while anything is folded). Under
it the ruler (the five labels of `Waterfall::ruler`, over the timeline's
columns) and a `uniform_list` of 28 px rows with a Kit scrollbar. A row:

| Part | What |
|---|---|
| label (42% of the width) | indent `6 + depth × 16`, a chevron (down open, right folded; a click folds or opens without selecting), a 3 px bar in the service's colour, a red `!` circle on an erroring span, the name (ellipsis), and in tertiary text the service, and for a call `→` the other side of it (`payments-api → card-gateway`) |
| timeline | faint lines at a quarter, a half and three quarters; the span's bar at its offset and length in the service colour at 0.75 opacity (full for the selected and for an erroring span, which also has a red outline); a diamond (with a ring in the panel colour) at each `exception` event |
| duration | 72 px, right-aligned |

The selected row has the accent at 18%; a row that matches the find text has
the amber `warn` colour at 12%. Clicking a row selects its span (the
`missing parent` row folds instead: it is not a span). Nothing in a row can
widen the layout: the label column is a share of the width and cuts its text.

**Find.** Typing marks and counts the matches (`2 matches`, `No matches`) without
moving the selection. Enter selects the next match below the selected row (the
first one when the selection is not above one), wrapping round, and scrolls to
it; the count then reads `2 of 5`: which match it is is the position of the
selected row among the matches, so it stays right when the rows change.
Matches are over the rows shown (`Waterfall::find`), and the placeholder is
never one.

**The inspector** (`InspectorView`, `inspector_view.rs`) is a Kit scroll area:
the span's name (wrapped), its service with the colour dot, `Server · 1.59 s ·
starts at +200 ms`, the status (a red `!` and red text for `Error`), the
**exception card** (the type and the stack trace in the monospace face, the
message, `Copy`), `Attributes` and `Resource` as key and value grids sorted by
key (a value over 200 characters is cut and `… (show all)` shows it whole,
`(show less)` cuts it again), and two buttons: `Traces through this span name`
(the home screen with `{ name = "…" }` in the query bar, see `span_name_filter`)
and `Copy span as JSON`. Another span starts with every value cut. Nothing
selected: `Select a span to see its details`.

**The span's logs** (`SpanLogsView`, `span_logs_view.rs`) are the 200 px under the
waterfall: `Logs from POST /charges` and `4 logs`, and at the right of that line
an `Open in Logs` button (a ghost button; only with a span selected), then a row
per log (the time in the monospace face, the level in a badge, the message on one
line cut with an ellipsis and whole in a tooltip) in a `uniform_list`. `No logs
from this span` when it has none, `Select a span to see its logs` with no
selection. `Open in Logs` emits `SpanLogsEvent::OpenInLogs(query)`, which
`TraceView` re-emits as `TraceViewEvent::OpenLogs(query)` and the shell turns into
`navigate(Screen::Logs { query: Some(query) })`; the query is
`span_logs_query(service, span)`, `{service_name="payments-api"} | span_id =
"03feb5f65e799604"`, so the Logs screen lists exactly that span's logs
(`open_in_logs_from_trace_view_sets_query`).

**Keys.** The screen takes the keyboard when it opens. **Esc** goes back (the home
screen as it was left: its query, its toggles, its rows), except that with the
find box focused and not empty it empties the box first. **⌘F** (or Ctrl-F)
focuses the find box. A click on a row, a chevron or a button gives the keys
back to the screen.

**For tests.** `TraceView::model()`, `waterfall_view()`, `inspector_view()`,
`logs_view()` and `split()` reach the parts; `bounds_of(TracePart, cx)` and
`center_of` give where a part (`Back`, `CopyId`, `Find`, `ErrorsPath`,
`ToggleAll`, `List`, `Inspector`, `TracesThrough`, `CopyJson`, `CopyException`,
`ShowAll`, `OpenInLogs`) is in the window in the latest frame (`None` while it is not
there); `WaterfallView::row_center(row)` and `chevron_center` find a row with
the scroll taken into account, `scroll_to_row` and `top_row` scroll it, and
`tests/common/mod.rs` has the helpers the tests share (`open_trace`: navigate
and draw until the trace is read, `row_named`, `selected_details`,
`part_center`).

**References** (all 1440 by 900, at 2x): `trace_dark.png` and `trace_light.png`
(the failing checkout opened: the failed-in span, `POST` to card-gateway at 236 ms,
selected and in the inspector), `trace_charges_selected_dark.png` (a click on
`POST /charges`: the exception card with the stack trace, four logs),
`trace_errors_path_only_dark.png` (the toggle on: the seven erroring spans),
`huge_trace_middle_dark.png` (`HugeTrace` of 100,000 spans scrolled to row
50,000), `long_values_dark.png` (a 300-character name, a 20 KB attribute, a
600-character token, a long exception) and `long_values_shown_whole_dark.png`
(the same with the long value shown whole). All seven were regenerated in
Milestone 4 Task 4 for one visible change, the `Open in Logs` button at the right
of the span-logs header. The colours of the services are the
session's, in the order the home screen first met them (the health checks of
`home_store` come first, so `web-bff` is not blue). The monospace face is Inter
on Linux, as in the table. How the screen looks and scrolls on a Mac is
unverified.

## The Logs screen

`views::LogsView` (`src/views/logs_view.rs`, Milestone 4 Task 4) is the content of
`Screen::Logs { query }`: the LogQL bar, the level toggles, the volume histogram
and the list of lines, over the live data of `app::LogsModel`
(`src/app/logs_data.rs`). Decision 0037 has the choices the plan left open.

**Top to bottom** (two floating panels like the home screen's, the content
width):

| Part | What |
|---|---|
| toolbar (the shell's glass strip, `LogsControls`) | the title `Logs`; at the right a `Live tail` pill (green dot and an accent tint while the tail runs, an amber dot while scrolling away or the lines around a log have stopped it, a hollow ring and no tint when it is off; a click toggles it), the range button (`Last 15 minutes` and its menu: the same menu as the home toolbar's) and `Clear all` (empties the store, as the home one) |
| search | a `LogQL` chip, the input (placeholder `EXAMPLE_QUERY`, a clear button), `Run`; under them `at column N: message` in red while the text does not parse. Typing validates and searches live; an invalid text changes nothing but the error line, and the list keeps the last good query's rows |
| toggles | `ERR` (`error_text`), `WRN` (`warn`), `INF` (`text_secondary`), `DBG` (`text_tertiary`): the letters in the group's colour, a tint and a ring when on, no fill when off (all four on at first). Each is its group of severities (`ERR` error and fatal, `INF` info and unspecified, `DBG` debug and trace). At the right the count (`207 lines · newest first`, `500+ lines · newest first` while more pages wait, `20,000+ lines` at the cap) |
| histogram | `HistogramView`: 48 bars 64 px high with 2 px between them, the errors stacked at the bottom of each bar in `error` and the rest in `border` (a bucket with a line is at least 2 px, an empty one a 1 px stub), and four clock labels under them at the start, a third, two thirds and the end of the window (the window ends now) |
| list | 30 px rows: the time (mono, 96 px), the level badge (44 px column), the service with its colour dot (112 px), the message (mono, one line, an ellipsis), the first 8 hex digits of the trace ID as a link (96 px; none for a log outside a trace) |
| banner | context mode only: `Lines around 08:00:01.440 · Back to search`, or `This line is no longer held · Back to search` |
| pill | while the tail is paused by scrolling: `New lines · Jump to newest`, floating at the top of the list |
| footer | once the list stopped at 20,000 rows: `Showing the newest 20,000 matching lines` |
| empty | `No logs yet`, or `No lines match` when a query or a toggle narrows the list |

**A click on a row expands it** where it stands (one row at a time: another click
moves the expansion, a click on the same row folds it). The row is taller by the
details grid, which is the whole body first (wrapped, line breaks kept: a stack
trace stays a stack trace) and then the labels LogQL sees (`model::logs::details`),
and three buttons: `Open trace` (a log in a trace), `Show lines around this one`
and `Discard lines like this…`. The trace link of a row opens the trace without
expanding the row. The list is GPUI's variable-height `list` (an expanded row is
taller than 30 px, which a `uniform_list` cannot hold); the view owns its
`ListState` and keeps it in step with the rows through the model's events, with a
check in every frame that the list's count is the model's.

**The data** is `LogsModel`. What the user sets (the call, then what it does):

| Call | Effect |
|---|---|
| `set_query(text)` | validates at once (`state()` is new when it returns); an empty bar or a query that parses searches again from the first page, an invalid text only changes the state |
| `set_levels(LevelSet)`, `set_range(TimeRange)` | search again from the first page; `None` levels to the store when all four are on |
| `set_tail(bool)` | the live tail switch; on, it catches up at once |
| `load_more()` | read the next page and append it |
| `expand(Option<LogId>)` | expand or fold a row (an ID not in the list is `None`) |
| `show_around(LogId)`, `back_to_search()` | context mode and back |
| `set_at_top(bool)` | what the view tells it (below) |
| `jump_to_newest()` | the pill: the first page again, the list at the top |
| `refresh_now()` | what Enter and `Run` do: the first page again |
| `clear_all()` | empties the store through the backend and reads again |
| `set_active(bool)` | what the shell tells it; nothing is read while it is `false` |

and what it holds: `rows()`, `record(index)`, `histogram()`, `state()` (the LogQL
bar's `LogQueryState`), `query_text()`, `levels()`, `range()`, `tail()`,
`tail_paused()`, `expanded()`, `expanded_record()`, `expanded_details()`,
`context()` (`ContextInfo { target, held, index }`), `context_target()`,
`banner()`, `loaded()`, `more()`, `capped()`, `footer()`, `count_line()` and
`has_filters()`. Every store read runs on GPUI's background executor on a copy of
its inputs, and a result that a newer read replaced is dropped when it arrives.

- **Paging.** The first page is 500 lines (`PAGE_SIZE`), newest first. A row drawn
  within 50 rows (`PREFETCH_ROWS`) of the end of the loaded rows asks for the next
  page as it is drawn, so a scroll to the end reads one more page; a list with nothing more to load
  ignores the asking. The list holds
  at most 20,000 rows (`MAX_ROWS`); past that it stops, `capped()` is true and the
  footer shows. The count line counts the rows **loaded**, with a `+` while more
  are waiting: the store has no cheap count of what matches, and the histogram
  (which is a pass over the window) is not asked to give one.
- **The live tail** (design 9.2; plan Review Focus 1). While the tail is on, the
  list is at the top and no read of the list is out, the newest page (500) is read
  every 500 ms (`TAIL_REFRESH`) and joined to the rows by `model::logs::merge_head`:
  new lines go on top, once. The join is the fresh page followed by the held rows
  older than it, so every row is strictly older than the one before and **no line
  can be in the list twice**. A page that cannot be joined without a gap (a flood
  that outran a page: the page ends newer than the newest row held) or an empty
  one (the store was cleared) starts the list over from the page. A late line that
  arrived inside the page's range shows once, in its place. The histogram is read
  every 2 s (`HISTOGRAM_REFRESH`) while the tail is on.
- **Scrolling away pauses the tail.** The view reports whether the list is at its
  top (`set_at_top`, after each frame in which that changed); away from the top
  nothing is ever put above the rows being looked at, the pill shows, and a head
  read that was already out when the user scrolled is dropped when it arrives.
  Clicking the pill (`jump_to_newest`), or scrolling back to the top, resumes: the
  first brings the first page, the second joins what came meanwhile. With the tail
  off the list and the histogram stand still. Tests:
  `tail_dedupes_and_paused_tail_keeps_position`,
  `the_tail_toggle_stops_the_list_and_the_histogram`.
- **Lines around** (design 9.2; plan Review Focus 5). `show_around(id)` reads
  `Store::logs_around(id, 25, 25)` (`CONTEXT_BEFORE`, `CONTEXT_AFTER`): the store's
  lines around the log, unfiltered, **oldest first** like a file, with the target
  row highlighted and scrolled into view. The tail and the paging stand still. A
  log that is gone gives `This line is no longer held` and the lines nearest to
  where it was (`lines_around_a_line_that_is_gone_say_so_and_show_what_remains`).
  Changing the query, the levels or the range, or `Back to search`, leaves context
  mode and searches again from the first page (the list does not remember where it
  was).
- **The histogram** is `Store::log_histogram` over the window that ends now: 48
  buckets over the range (`Everything` starts at `Store::oldest_log_unix_nano`),
  for the lines that match the query and the toggles, turned into bars by
  `model::histogram::bars`.
- **The range is the Logs screen's own**: the home screen's range and this one are
  separate settings, both `Last 15 minutes` at first, with the same menu.

**For tests.** `main.logs()` and `main.logs_view()` reach the model and the view;
`LogsView::bounds_of(LogsPart)` and `center_of` give where a part (`QueryInput`,
`Run`, `Level(badge)`, `Histogram`, `List`, `Pill`, `Banner`, `BackToSearch`,
`Footer`, and for an expanded row `OpenTrace`, `ShowAround`, `Discard`) is in the
latest frame, `row_center(index)` and `trace_link_center(index)` find a row,
`scroll_offset()` says where the list is scrolled, and `input_text`, `error_line`,
`banner` and `empty_state` read what it says; `LogsView::controls()` reaches the
toolbar's `LogsControls` (`bounds_of(ToolbarPart::LiveTail | Range | ClearAll)`).
`tests/logs_screen.rs` has the data (`logs_store`: the failing checkout's seven
logs at `BASE` and 200 lines of `LogFlood` spread over the ten minutes before them,
on a clock 30 s after `BASE`) and every test below.

**References** (1440 by 900, at 2x, over that store, with the three-entry sidebar
of Task 7, `Logs` lit): `logs_dark.png` and
`logs_light.png` (the list at the top, newest first, with the checkout's three
errors, the warning and three infos over the flood's `INF` and `DBG` lines; the
histogram with the checkout's bar last, its three errors in red at the bottom),
`logs_expanded_dark.png` (the `Charge failed…` line expanded: the body, the
labels with the stack trace on three lines, and the three buttons),
`logs_query_error_dark.png` (an unclosed selector typed: `at column 29: expected
`,` or `}`` in red under the focused input, the rows the last good query's) and
`logs_context_dark.png` (the lines around the `card-gateway did not answer` error:
the banner, the target highlighted, `28 lines · oldest first`). The monospace face
is Inter on Linux, as in the table. How the screen looks and scrolls on a Mac is
unverified.

## The Discard rules screen

`views::RulesView` (`src/views/rules_view.rs`, Milestone 4 Task 5) is the content
of `Screen::Rules { draft }`: the rules as cards, and beside them the
`views::RuleEditorView` (`src/views/rule_editor_view.rs`) of the one that is open,
over the live data of `app::RulesModel` (`src/app/rules_data.rs`). Design §7 and
`docs/areas/rules.md` say what a rule is and what it does to the data; decision
0038 has the choices the plan left open.

**The list** (one floating panel; the editor panel is beside it while a rule is
open, so the list gives up 380 px + `INSET`):

| Part | What |
|---|---|
| header | the title `Discard rules` (20 px), under it the subtitle `Anything a rule matches is dropped as it arrives and never stored. Rules are kept when loctel restarts.`; on the right `rules::dropped_line` (`31% of incoming data dropped`, `Nothing dropped yet`) and `New rule` |
| banners | the rules file's error, in the error colour: `The rules file has an error: <msg>. loctel keeps using the rules it had.` (`file_error_line`: the message on one line, no double full stop); `Could not save the rules: <reason>` when a switch could not be written; with no rules, `No discard rules. Nothing is dropped unless you add one.` in a neutral box |
| cards | one per rule in the file's order (`rules::rows`): the switch, the name (semibold; dimmer while the rule is off), a `TRACEQL` or `LOGQL` chip and, at the right, the count text (`48,210 spans dropped in 602 traces`, `96,433 lines dropped`, `Off`, or `Error: <message>` in `error_text`), and the query in the monospace face on one line under them. The card of the open rule has an accent ring and tint |

A click on a card opens that rule in the editor (`RulesModel::edit`). The switch
swallows its own click: it calls `set_enabled` (the backend's `set_rule_enabled`,
which saves) and does not open the card.

**The editor** (380 px wide, `PANEL_RADIUS` corners, the panel fill with the glass
hairline and the highlight ring just inside it; the fields scroll, the buttons
are pinned under them):

| Part | What |
|---|---|
| title | `Edit rule`, or `New rule` for one that is not saved yet |
| banner | only while the rules file changed behind the editor: `The rules file changed while you were editing.` with `Keep my draft` and `Discard my draft` |
| `Name` | an input; `Save` needs something in it |
| `Applies to` | a segmented control: `Traces (TraceQL)` or `Logs (LogQL)`; the chosen side is tinted with the accent. The query keeps its text when the side changes: it is read by the other parser from then on |
| `Query` | a multi-line input in the monospace face, with `at column N: message` in `error_text` under it while the text does not parse (nothing while it is blank: a rule that has just been started is not an error yet) |
| scope | traces only: the radio `When a span matches, drop` with `the whole trace it belongs to` and `only the matching span and its children` (`DropScope::Trace` and `Span`). A log rule has none |
| preview box | `Tried on the last 5 minutes` and a sentence, below |
| buttons | `Save`, `Cancel`, and for a saved rule `Delete`, which asks (`Delete this rule?`, `Delete rule`, `Keep rule`) before it removes anything |

**The preview** is `loctel_rules::preview` of the draft over the last five minutes
of the store (`PREVIEW_WINDOW`), as `model::rules::preview_text` words it:

| State | The box says |
|---|---|
| the query does not parse | `Fix the query to see what it would drop.` (at once, no read) |
| a trace rule | `Would have dropped 1,204 spans in 602 traces. None of those traces had errors.` or `... 3 of those traces had errors.` (`1 span in 1 trace. That trace had no errors.` / `That trace had errors.` for one) |
| a log rule | `Would have dropped 96 lines. 2 were errors.` (`None were errors.`, `1 was an error.`, `1 line. It was an error.` / `It was not an error.`) |
| it would have dropped nothing | `Would have dropped nothing.` |
| blank query, or the first result is not back | nothing (the box keeps its height) |

A change of the name or query text waits 300 ms (`PREVIEW_DEBOUNCE`) after the
last keystroke; opening a rule, and a click on the language or the scope, run at
once. The pass runs on GPUI's background executor over a clone of the store, and a
result that a newer edit replaced is dropped when it arrives (a result that is
there stays until the next replaces it, so the box does not flicker while typing).

**Saving.** The editor remembers the rules as they were when it opened (their
IDs and specs). `Save` builds the full list from that snapshot with the draft in
the place of its rule (at the end for a new one) and calls `UiBackend::set_rules`;
the editor closes when it worked and stays, with `Could not save the rules:
<reason>` above the buttons, when it did not. Opening a rule and saving it
unchanged changes nothing (the rule keeps its ID and counters: the engine keeps a
rule's identity by name, language, query and scope). A query that does not parse
is saved (a draft, per M2): its card shows `Error: <message>` and it drops
nothing (plan Review Focus 3; `invalid_rule_saved_shows_error_and_no_preview`).
`Delete` asks, and the yes saves the snapshot less that rule. `Cancel` closes the
editor and saves nothing.

**A rule edited behind the editor's back** (plan Review Focus 2;
`external_change_during_edit_warns`). The editor keeps `rules_generation` from
when it opened. When the number has moved and the editor did not move it (a
reload of the edited file, `set_rules` from elsewhere), the banner shows, the
draft is left exactly as it is, the list shows what the file has now, and `Save`
and `Delete` do nothing until the user decides: `Keep my draft` turns the banner
off, and `Save` then writes the snapshot with the draft over the file, which
**overwrites** what the file got meanwhile (the banner's whole point);
`Discard my draft` closes the editor and leaves the file as it is. The editor's
own writes are not external: a switch on a card (this window's doing) re-takes the
snapshot and the generation (and the draft of that very rule follows its switch,
or saving it would turn the rule back on), so no banner appears for them and the
list saved over is never older than the user's last action.

**Refresh.** The model reads `UiBackend::rules` and `status` (the share dropped,
the file's error) once a second (`RULES_REFRESH`) while the screen shows, and at
once after anything it does and when the screen comes back; it redraws only when
something changed. These are in-memory reads of the engine's rules, not store
reads, so they run on the foreground like the shell's status refresh. The first
frame of a screen that was hidden shows what it read as it came back.

**For tests.** `main.rules()` and `main.rules_view()` reach the model and the view;
`RulesView::bounds_of(RulesPart, cx)` and `center_of` say where a part is in the
latest frame (`NewRule`, `Card(i)`, `Switch(i)`, `Editor`, `NameInput`,
`LanguageTraces`, `LanguageLogs`, `QueryInput`, `ScopeTrace`, `ScopeSpan`,
`Preview`, `Save`, `Cancel`, `Delete`, `ConfirmDelete`, `KeepRule`, `KeepDraft`,
`DiscardDraft`); `banners`, `dropped_line`, `preview_text`, `query_error_line`,
`editor_banner`, `name_text` and `query_text` read what it says. The model's
`rows()`, `selected()`, `editor()` (a `RuleEditor`: `draft()`, `target()`,
`is_new()`, `external_change()`, `confirming_delete()`, `error()`, `preview()`,
`preview_text()`, `can_save()`) and `file_error_line()` read what it holds.
`tests/rules_screen.rs` has the data (`rules_store`: the failing checkout, the
successful one and 600 health checks, on a clock 140 s after the checkout; three
rules; `counted_backend`, which sends 1,500 health checks and 40,000 log lines
through the rules with `FixedBackend::rule_set()` so the cards have counts) and
every test; a change of the file behind the editor is `FixedBackend::set_rules`,
which moves the generation as a reload does.

**References** (1440 by 900, at 2x, with the three-entry sidebar of Task 7,
`Discard rules` lit): `rules_dark.png` and
`rules_light.png` (the three rules, no editor: the 1,500 spans in 1,500 traces of
the health checks, `1,217 lines dropped` for the log rule, the third `Off` with
its switch off, `31% of incoming data dropped`), `rules_editor_dark.png` (the first
rule open: its fields, the whole-trace radio, the preview `Would have dropped 600
spans in 600 traces. None of those traces had errors.`, and `Save`, `Cancel` and
`Delete`), `rules_empty_dark.png` (`No discard rules...` and `Nothing dropped
yet`), `rules_file_error_dark.png` (the red banner over the rules it kept),
`rules_editor_invalid_dark.png` (a new rule with `{ name = ` typed: `at column 10:
expected a value` under the query and `Fix the query to see what it would drop.`
in the preview, no `Delete`) and `rules_editor_changed_dark.png` (the banner with
its two buttons over the fields, `Save` dimmed, and the list showing the fourth
rule the file got). The monospace face is Inter on Linux, as in the table. How
the screen looks and behaves on a Mac is unverified.

## The Settings window

`views::SettingsView` (`src/views/settings_view.rs`, Milestone 4 Task 6) is the
form of the Settings window, and `app::open_settings_window(backend, cx)`
(`src/app/settings_window.rs`) opens it: 520 by 360 logical pixels, centred, not
resizable, titled `Settings`, the view in GPUI Kit's `Root` like the main window.
If a Settings window is already open it focuses that one and returns its handle
(the handle is kept in a GPUI global and checked against `cx.windows()`, so a
window that was closed is forgotten and the next call opens a new one). The view
takes the theme from the window's appearance and follows it, as `MainWindow`
does. The disk cap is not on the form until M5 (nothing enforces it). The form's
rules are `model::settings::Form` (see "Models"); decision 0039 has the choices
the plan left open.

**Layout** (the fields scroll, the buttons are pinned under them; all of it fits
the 360 px even with all three messages showing, which `settings_invalid_dark`
checks):

| Part | What |
|---|---|
| title | `Settings` (18 px, semibold) |
| `Endpoints` | a panel with a row per field: the caption (`OTLP gRPC port`, `OTLP HTTP port`, `MCP port`), a 96 px input, and how that endpoint is doing: a green dot and `listening` in `ok`, or a red dot and `failed: <reason>` in `error_text`. A text that is not a port, or a port two fields share, gets its message (`Enter a port from 1 to 65535`, `Each endpoint needs its own port`) in `error_text` under its input |
| `Startup` | only where `UiBackend::login_item()` is `Some` (macOS): a panel with the `Start loctel at login` switch, set from `LoginItem::is_enabled()` when the window opens |
| buttons | `Apply` (primary; on while the form differs from what was saved and every field is valid) and `Close` (closes the window); beside them, in `error_text`, what the last Apply could not do |

**Apply** (`SettingsView::apply`, the button; nothing happens while it is off):

1. It takes the settings the backend has (`UiBackend::settings()`), puts the three
   ports in them and calls `UiBackend::apply_settings`. That saves `settings.toml`
   and rebinds the listeners whose port changed (`EngineHandle::rebind`). Every
   other setting stays as it was; the login switch is not written to the file (the
   system is the source of truth, see `login_item.rs`).
2. **A port that cannot be bound is not an error of Apply.** It is saved; the
   listener comes back `Failed`; the row says `failed: address in use`; the line
   beside the buttons says `otlp-http: address in use` (`failures`); the other
   listeners are untouched (plan Review Focus 4;
   `apply_rebinds_and_shows_failure`). The form counts as saved, so `Apply`
   goes off again.
3. **A settings file that cannot be written is an error**: `Could not save the
   settings: <reason>` beside the buttons, nothing moved, the form and the switch
   as they were, `Apply` still on to try again. The login item is not touched
   then.
4. **The login switch** takes effect here, not when it is clicked: if it differs
   from what was loaded, `LoginItem::set_enabled(on)` is called after the ports
   are saved. If the system refuses, `Could not change Start at login: <reason>`
   is shown (the ports stay applied) and the switch shows what
   `LoginItem::is_enabled()` says now, whatever was asked.
5. The inputs are rewritten to the ports as saved (`" 4500 "` reads `4500`).

What the last Apply could not do goes away when the form is edited again.

**Refresh.** Once a second (`SETTINGS_REFRESH`) the view reads
`UiBackend::status().listeners` for the row states, and, while the switch is as
loaded (the user has not clicked it), asks the system's login item again, so a
change made in System Settings shows. It never rewrites what is typed in an input,
and a switch the user has clicked stays theirs until Apply. The row states are the
engine's, so while a port is edited and not applied a row still says what is true
of the port it listens on.

**For tests.** `SettingsView` reads: `form()`, `can_apply()`, `endpoint(field)`,
`message(field)`, `notice()`, `sections()`, `input_text(field, cx)` and
`bounds_of(SettingsPart, cx)` / `center_of` (`Input(field)`, `Status(field)`,
`Message(field)`, `LoginSwitch`, `Apply`, `Close`). `tests/settings_popover.rs`
(the popover's tests are in the same file, see "The popover") drives it in a `Driver<SettingsView>` at
520 by 360 over a `FixedBackend`: `fail_bind` makes the HTTP listener `Failed`
the way a port in use does, `fail_save` makes `apply_settings` an error, and the
login item is a `FakeLogin` the test can move behind the window's back. `Driver`
gained `is_open()` for `Close` (a click that closes the window leaves nothing to
draw).

**References** (520 by 360, at 2x: 1040 by 720): `settings_dark.png` and
`settings_light.png` (all three listening, the login switch on, `Apply` dimmed),
`settings_edited_dark.png` (HTTP typed as 4400 and the switch clicked, not applied:
`Apply` lit and the HTTP row still `listening`), `settings_failed_dark.png` (HTTP applied to a port that is in use: the red
row and `otlp-http: address in use` beside the buttons) and
`settings_invalid_dark.png` (the worst case: two fields with the same port and a
third out of range, each with its message, and everything still in view). How the
window looks and behaves on a Mac (its chrome, that it is not resizable, how it
takes the focus from a menu bar app) is unverified.

## The popover

`views::PopoverView` (`src/views/popover_view.rs`, Milestone 4 Task 8) is the menu
bar popover: the panel that replaces the M1 and M2 menu. It is cross-platform on
purpose. The view, its content (`model::popover`) and everything it asks of the
app (`PopoverActions`) are built and screenshotted on Linux; what is left for
`loctel-macos` (Task 9) is to put it in a window under the menu bar icon and to
implement `PopoverActions`. The popover on a Mac is unverified (see "Unverified").
Decision 0041 has the choices the plan left open.

**The panel** is `POPOVER_WIDTH` (336) wide, with corners of `POPOVER_RADIUS` (18),
and as tall as its rows: every row has a fixed height, so `popover_height(login_item)`
is the sum (504 with the `Start at login` row, 476 without, in logical pixels) and
the popover's window is made that size. Swapping `Clear all data…` for its question
changes no height (`the_height_does_not_change_while_it_asks_to_clear`). From the
top:

| Row | What |
|---|---|
| title and pill | `loctel` (16 px, semibold) and the pill: a dot and the words `Receiving` (green), `Paused` (amber) or `Port problem` (red, in `error_text`), as `popover::from_status` says (a failed listener over paused over receiving) |
| stats | four tiles, two by two (56 px high): the value (20 px, semibold) over its label: `412` `spans per second`, `1.2k` `log lines per second`, `1,204` `traces held`, `96,433` `log lines held` |
| endpoints | a group of three 28 px rows: a dot (green, or red for a listener that is not listening), the name (`OTLP gRPC`, `OTLP HTTP`, `MCP`), and the address in the mono face: `127.0.0.1:4317`, and for MCP the URL without its scheme, `127.0.0.1:4319/mcp`. A listener that could not be bound is red and says `127.0.0.1:4318 · failed` (the colour is not the only sign). `Copy` ends the MCP row, only while MCP is listening: there is no address to copy otherwise |
| rules line | a dot, `menu_text::rules_line` (`1 discard rule on · 31% dropped`) and a chevron; red for a rule or a file with an error, grey for none on, green otherwise (the status view's rules row). A click opens the main window on Discard rules |
| divider | |
| actions | `Open loctel` ⌘O, `Pause receiving` or `Resume receiving`, `Clear all data…`, and where `UiBackend::login_item()` is `Some` (macOS) `Start at login` with a switch |
| `Settings…` | ⌘, |
| divider | |
| `Quit loctel` | ⌘Q |

**What it does.** Every action is a call on the `PopoverActions` the view was
given (`Rc<dyn PopoverActions>`): `open_main(Screen)`, `open_settings()`,
`toggle_pause()`, `clear_all()`, `copy_mcp()`, `set_start_at_login(bool)`, `quit()`
and `close()`. The view never opens a window, writes a setting or touches the
clipboard itself.

| The user | The view calls |
|---|---|
| `Open loctel`, ⌘O | `open_main(Screen::Home)`, then `close()` |
| the rules line | `open_main(Screen::Rules { draft: None })`, then `close()` |
| `Settings…`, ⌘, | `open_settings()`, then `close()` |
| `Pause receiving` / `Resume receiving` | `toggle_pause()` (the popover stays open) |
| `Copy` | `copy_mcp()` (stays open) |
| the `Start at login` switch (or its row) | `set_start_at_login(!shown)` (stays open) |
| `Clear all data…` | nothing yet: the row turns into `Clear all traces and logs?` with `Clear` and `Cancel`. `Clear` calls `clear_all()` once (stays open); `Cancel` puts the row back |
| `Quit loctel`, ⌘Q | `quit()` |
| Esc | `close()` (a question that is open is forgotten) |

The popover closes when it loses focus (Task 9: the window's deactivation), on Esc,
and after an action that opens a window, which is the `close()` after those three
rows. It does not close for pause, `Copy`, the switch or a confirmed clear.

**Refreshing.** The popover reads `UiBackend::status()` and the login item when it
opens and every second (`POPOVER_REFRESH`), and **at once after an action that
changes what it shows** (pause, the switch, a confirmed clear), so `Pause receiving`
reads `Resume receiving` as soon as it is clicked. The switch always shows what the
system holds: after `set_start_at_login` the view asks the login item again, so a
refusal leaves it as it was (`the_login_switch_follows_the_system`).

**Keys.** The panel is a key context of its own, `LoctelPopover` (not `Popover`,
which GPUI Kit's popover binds Esc in), with Esc, ⌘O, ⌘, and ⌘Q bound by
`app::init` to the same things as the rows. They work wherever in the panel the
keyboard is: the view takes the focus when it is first drawn and takes it back
whenever what had it leaves the frame (the same as the main window's shell,
decision 0040; GPUI Kit's buttons do not take focus on a click, so a stray focus
is simulated in `the_popover_takes_the_keyboard_back_when_what_had_it_is_gone`).

**Glass.** `PopoverView::with_backdrop(Some(backdrop))` (`loctel_macos::glass::attach`
returns one; it is `None` on Linux and in most tests) makes the panel glass, as the
sidebar and the toolbar of the main window are: no fill, so the platform's glass
shows, with a hairline round it (`Theme::glass_hairline`) and a highlight just
inside (`glass_highlight`), the tiles and the endpoint group translucent
(`Theme::panel` at 45% in the dark look and 55% in the light one) and the window
colour that Kit's `Root` paints over the window cleared on the first frame. The
view then reports **one** `GlassRegion` (the panel's border box, radius 18) to the
backdrop whenever it changes, and never otherwise (`popover_reports_one_glass_region`).
Without a backdrop the panel is opaque: `Theme::bg` with the tiles in `Theme::panel`
and `Theme::border` hairlines. The first frame also takes the theme from the window's
appearance and follows it, as `SettingsView` does: the popover is the first window
of a menu bar app, so no other window has set `Theme` yet.

**For tests.** `PopoverView` reads: `popover()` (the model it draws),
`action_rows()` (the rows as `(words, shortcut)`, top to bottom),
`endpoint_lines()`, `confirming_clear()` and `bounds_of(PopoverPart)` /
`center_of` (`Panel`, `Pill`, `Stat(i)`, `Endpoint(i)`, `CopyMcp`, `RulesLine`,
`OpenMain`, `Pause`, `ClearAll`, `ConfirmClear`, `CancelClear`, `LoginSwitch`,
`Settings`, `Quit`). `tests/settings_popover.rs` (the popover half) drives it in a
`Driver<PopoverView>` at 336 by `popover_height` over a `FixedBackend`, with a
`Recorder` for the `PopoverActions`: it writes each call down and, as the app's
actions do, flips the backend's pause, clears its store and asks the login item
(the `FakeLogin` of the Settings tests, which can refuse), so the tests see the
view read the result back.

**References** (336 by 504, at 2x: 672 by 1008): `popover_dark.png` and
`popover_light.png` (receiving with 412 spans and 1.2k lines per second, one rule on
with 31% dropped, the login switch on), `popover_paused_dark.png` (paused, zeros, `Resume
receiving`, no rules, the switch off), `popover_port_problem_dark.png` (`otlp-http`
failed and a rule with an error), `popover_clear_asks_dark.png` (the row asking) and
`popover_glass_dark.png` / `popover_glass_light.png` (a `RecordingBackdrop` over a flat
stand-in for the desktop: the panel clear, tiles translucent, corners outside the
radius showing the desktop; on a Mac the stand-in is Apple's glass).

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

Two more are derived, with methods and not fields, for the glass parts of the
window: `glass_hairline()` (the text colour at 16% on the dark look and 14% on
the light) and `glass_highlight()` (the text colour at 8% on the dark look, the
white panel colour at 60% on the light one). Both are translucent, so the glass
shows through them.

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
(§9.3) says the product uses the system font (SF) and the system monospace on
macOS. The Milestone 3 plan has no task for that switch, so the main window, GPUI
Kit's components (`kit_theme::apply` sets their font family) and the status view
all use Inter on every platform; the few monospace parts (the query input, the
`Started` column, trace IDs, stack traces) ask for the theme's `mono_font_family`,
which GPUI Kit sets and which the Linux harness cannot find, so the references
show them in Inter (see "The trace table"). Switching a Mac build to SF is a
later change (decision 0031); it would change no Linux reference, because the
harness never sees the system fonts. See
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
`tests/screens.rs`, `interactions.rs` and `live.rs`:

```rust
let image = render_window_png((1440, 900), &Theme::dark(), |window, cx| {
    cx.new(|cx| MainWindow::new(backend, None, window, cx))
});
assert_snapshot("shell_dark", &image);   // 2880 by 1800, about 220 KB
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
  `drag(from, to)` (the left button, pressed, moved in four steps with it held,
  released), `scroll_with(point, dy, modifiers)` (`Modifiers::control()` for a
  ctrl-scroll), `pinch(point, delta)`,
  `resize((w, h))`, `draw()`, `capture()`, `update_root(|view, window, cx| ..)`,
  `read_root(|view| ..)` and `read(|app| ..)` (globals such as Kit's theme), and
  `is_open()` (false once something in the window closed it: a click on the
  Settings window's `Close`; `draw` then has nothing to draw and does not panic).
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
  `listening_status()` and UTC, and the rules, settings, pause and login item
  in memory; `fail_bind` and `fail_save` make `apply_settings` report a listener
  `Failed` or fail to write), and **`RecordingBackdrop`** (every
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

### The references

Every reference of `crates/loctel-ui/tests/snapshots/`, with the test that
owns it, the data it is drawn from and what it shows. All of them were opened
and checked when they were made (Milestone 3, and the Logs, Discard rules, Settings and popover ones in Milestone 4 Tasks 4, 5, 6 and 8): legible text, the forced theme, and
nothing clipped other than the table and the lists that scroll. The main
window's are 1440 by 900 logical pixels (2880 by 1800, 220 to 740 KB each, all
under the 1.5 MB the plan allows, so the capture stays at 2x); the Settings
window's are 520 by 360 (1040 by 720, 60 to 90 KB), and the popover's 336 by 504
(672 by 1008, 110 to 130 KB). "Home store" is
`common::home_store()`: the failing checkout, the successful one 5 s later and
one health check of each service, on a store clock 70 s later, so every trace is
complete; "empty store" has nothing in it.

| Reference | Test | Data | Shows |
|---|---|---|---|
| `status_view_dark` | `screenshots.rs` | a fixed `StatusModel`: `otlp-http` failed, 412 spans/s, 2 rules, 31% dropped | the standalone status view (900 by 600): the red `Port problem` pill, the four numbers, the rules line, the three endpoints with the failure in words |
| `status_view_light` | `screenshots.rs` | the same model, paused, no failed listener, one rule with an error | the same view in the light theme, with the pill `Paused` and the red rules line. It is not the dark picture re-coloured: it covers the other readings of the pill and the rules row |
| `shell_dark`, `shell_light` | `screens.rs` | empty store, healthy engine status | the window shell and the home screen with nothing in it: the sidebar with its three entries (icon and text) and `Map & traces` lit, and the status box, the toolbar with its title and controls, `Waiting for telemetry` over the map and `No traces yet` in the table (`0 traces · newest first`) |
| `shell_glass_dark`, `shell_glass_light` | `screens.rs` | the same, with a `RecordingBackdrop`, over a flat stand-in for the desktop | the glass look: the sidebar and the toolbar clear (the stand-in colour shows through, with a hairline and a highlight ring), the panels solid. On a Mac the stand-in is Apple's glass |
| `home_dark`, `home_light` | `screens.rs` | home store | the whole home screen: the toolbar with its four controls, the checkout's nine nodes in four columns with `web-bff`, `orders-api` and `payments-api` and the edges between them in red, `card-gateway` dashed, `orders-api` to `postgresql` along the lane between `inventory-api` and `payments-api` (not behind them), the `Fit` button, the TraceQL bar, the seven traces newest first with the failed one's `Failed in payments-api · POST card-gateway +1`, the service dots and the duration bars |
| `home_payments_selected_dark` | `interactions.rs` | home store, a click on `payments-api` | the 3 px accent ring on the node, `{ resource.service.name = "payments-api" }` in the bar (with its clear button), and the table filtered to three traces |
| `home_errors_only_dark` | `interactions.rs` | home store, `Errors only` clicked | the toggle lit with the accent, one row (the failed checkout), `1 trace · newest first` |
| `home_query_error_dark` | `interactions.rs` | home store, `{ status = ` typed | the focused input, `at column 12: expected a value` in red under it, and the seven rows kept (the table runs on under the panel's edge, as a scrolling table does) |
| `home_no_match_dark` | `interactions.rs` | home store, a query for a service that does not exist | no error line, an empty table with `No traces match` and the `Clear filters` button, `0 traces · newest first` |
| `toolbar_dark`, `toolbar_light` | `screens.rs` | home store | a crop (2420 by 136) of the toolbar strip: the title, and at the right the green `Live` pill, `Last 15 minutes` with its caret, `Pause` with its icon and `Clear all` |
| `toolbar_menu_dark`, `toolbar_menu_light` | `screens.rs` | home store, a click on the range button | the same strip with the range menu open under it (`Last 5 minutes`, `Last 15 minutes` lit, `Last hour`, `Everything`) over the map, where the long edge `orders-api` to `postgresql` runs through its lane |
| `toolbar_paused_dark`, `toolbar_paused_light` | `screens.rs` | home store, a click on `Pause` | the strip with the amber dot and `Paused` and the accent-filled `Resume` |
| `trace_dark`, `trace_light` | `screens.rs` | home store, the failing checkout opened | the trace view: the `Back` button, the root name, the red `Failed in` badge, `Complete`, `1.84 s · 15 spans · 5 services · 08:00:00.000`, the ruler, 15 waterfall rows in tree order with the failed-in `POST` to `card-gateway` (236 ms) selected, the inspector with its attributes and resource, and `No logs from this span` |
| `trace_charges_selected_dark` | `screens.rs` | the same, a click on `POST /charges` | the inspector's exception card (type, message, stack trace, `Copy`) and the span's four logs with their `INF`, `WRN` and `ERR` badges |
| `trace_errors_path_only_dark` | `screens.rs` | the same, `Errors path only` on | the toggle lit and the seven erroring spans only, the selection kept |
| `huge_trace_middle_dark` | `screens.rs` | meshgen `HugeTrace`, 100,000 spans, scrolled to row 50,000 | `36 ms · 100,000 spans · 4 services`, rows `work item 56102` and on at the middle of the list with the Kit scrollbar there, and no blank rows |
| `long_values_dark` | `screens.rs` | one trace with a 300-character span name, a 20 KB attribute, a 600-character token and a long exception | the name cut with an ellipsis in the header, the badge, the row and the logs title; wrapped in the inspector; every long value cut; the layout unchanged (inspector 340 wide at the panel's edge) |
| `long_values_shown_whole_dark` | `screens.rs` | the same, the inspector scrolled and `… (show all)` pressed on the 20 KB payload | the pane scrolled and the value shown in full, the pane longer and never wider |
| `logs_dark`, `logs_light` | `logs_screen.rs` | the failing checkout's 7 logs and 200 lines of `LogFlood` spread over 10 minutes | the Logs screen: the `Live tail` pill, the range and `Clear all` in the toolbar, the LogQL bar with its placeholder, the four toggles (all on), `207 lines · newest first`, the histogram (48 bars, the checkout's with three red errors at the bottom) and the first rows of the list |
| `logs_expanded_dark` | `logs_screen.rs` | the same, a click on the `Charge failed…` row | that row expanded: the whole body, the labels (the stack trace on three lines), `Open trace`, `Show lines around this one`, `Discard lines like this…` |
| `logs_query_error_dark` | `logs_screen.rs` | the same, `{service_name="payments-api"` typed | the focused input, `at column 29: expected `,` or `}`` in red under it, the rows kept |
| `logs_context_dark` | `logs_screen.rs` | the same, the lines around the 600 ms error | the banner `Lines around 08:00:01.440 · Back to search`, `28 lines · oldest first`, the target highlighted, a scrollbar |
| `rules_dark`, `rules_light` | `rules_screen.rs` | the rules store (failing and successful checkout, 600 health checks), three rules, 1,500 health checks and 40,000 log lines sent through them, 31% dropped | the Discard rules screen: header, subtitle and `New rule`, three cards (`1,500 spans dropped in 1,500 traces`, `1,217 lines dropped`, `Off` with its switch off) |
| `rules_editor_dark` | `rules_screen.rs` | the same rules (only the health checks counted), a click on the first card | that card with its accent ring and the editor beside it: name, `Traces (TraceQL)` chosen, the query, the whole-trace radio, the preview with 600 spans in 600 traces, `Save`, `Cancel`, `Delete` |
| `rules_empty_dark` | `rules_screen.rs` | no rules | `No discard rules. Nothing is dropped unless you add one.` and `Nothing dropped yet` |
| `rules_file_error_dark` | `rules_screen.rs` | three rules, a file error | the red banner `The rules file has an error: ... loctel keeps using the rules it had.` over the three cards |
| `rules_editor_invalid_dark` | `rules_screen.rs` | `New rule`, a name and `{ name = ` typed | the focused query with `at column 10: expected a value` in red under it, `Fix the query to see what it would drop.` in the preview, `Save` and `Cancel` only |
| `rules_editor_changed_dark` | `rules_screen.rs` | the first rule open with a new name typed, the file then changed (its query, a fourth rule) and a second passed | the amber banner `The rules file changed while you were editing.` with `Keep my draft` and `Discard my draft`, the dimmed `Save`, the draft kept, four cards |
| `settings_dark`, `settings_light` | `settings_popover.rs` | a `FixedBackend` with a login item that is on, the default ports (4317, 4318, 4319) | the Settings window (520 by 360): `Settings`, the three ports with a green dot and `listening`, `Startup` with the `Start loctel at login` switch on, `Apply` dimmed and `Close` |
| `settings_edited_dark` | `settings_popover.rs` | the same with the login item off, HTTP typed as `4400` and the switch clicked | the edit not applied: `Apply` lit, the switch on, the HTTP row still `listening` (the old port) |
| `settings_failed_dark` | `settings_popover.rs` | the login item off, HTTP typed as `4400` and `Apply` clicked, with `fail_bind` on `otlp-http` | the red dot and `failed: address in use` on the HTTP row (the input still shows `4400`), `otlp-http: address in use` beside the buttons, `Apply` dimmed again |
| `settings_invalid_dark` | `settings_popover.rs` | the login item on, HTTP typed as `4317` and MCP as `70000` | `Each endpoint needs its own port` under the first two inputs and `Enter a port from 1 to 65535` under the third, the rows still `listening`, `Apply` dimmed, the `Startup` panel still in view above the buttons |
| `popover_dark`, `popover_light` | `settings_popover.rs` | a `FixedBackend` with a login item that is on, 412 spans and 1,234 log lines per second, 1,204 traces and 96,433 log lines held, one rule on with 31% dropped | the popover (336 by 504): `loctel` and the green `Receiving` pill, the four tiles, the three endpoints with `Copy` on MCP (`127.0.0.1:4319/mcp`), `1 discard rule on · 31% dropped` with its chevron, `Open loctel ⌘O`, `Pause receiving`, `Clear all data…`, `Start at login` with its switch on, `Settings… ⌘,` and `Quit loctel ⌘Q` |
| `popover_paused_dark` | `settings_popover.rs` | paused, nothing flowing, no rules, the login item off | the amber `Paused` pill, the zeros, `Resume receiving`, the grey `No discard rules on`, the switch off |
| `popover_port_problem_dark` | `settings_popover.rs` | `otlp-http` failed to bind, one rule on with a query that does not parse | the red `Port problem` pill, the HTTP row red with `127.0.0.1:4318 · failed`, and the red `1 discard rule on · 1 has an error` |
| `popover_clear_asks_dark` | `settings_popover.rs` | the first popover, a click on `Clear all data…` | the row replaced by `Clear all traces and logs?` with the red `Clear` and the outlined `Cancel`, everything else and the height unchanged |
| `popover_glass_dark`, `popover_glass_light` | `settings_popover.rs` | the first popover with a `RecordingBackdrop`, over a flat stand-in for the desktop | the glass look: no fill, the stand-in colour showing in the padding and outside the rounded corners, the hairline and highlight, translucent tiles and endpoint group. On a Mac the stand-in is Apple's glass |

`shell_light`, `shell_glass_light`, `home_light`, `trace_light` and the `toolbar_*_light` ones are the same
picture as their dark twins with the light `Theme`: the same data and the same
layout, so a difference other than colour between a pair is a bug (the
status view's pair differs on purpose, see above). A new screen needs a pair of
its own (a dark and a light one) and, for each other state that matters, a dark
one.

## Adding a screen

A screen is a `Screen` variant, a view in `src/views`, the model behind it in
`src/model` (and, when it reads the store over time, an entity in `src/app`),
and a pair of screenshots. In this order, which keeps the tests first:

1. **The model, test first.** Put every rule that is not a pixel in
   `src/model/<name>.rs` (a plain function over the store's types: the strings,
   the rows, the numbers; colours as indices into `Theme::service_palette`), add
   it to `model/mod.rs`, and pin it with plain assertions in
   `tests/model_<name>.rs`. Use `model::format` for every number and time.
2. **The data entity, if the screen reads the store over time.** Follow
   `HomeModel` or `TraceModel` (`src/app`): build it over `Rc<dyn UiBackend>`,
   read on `cx.background_executor()` and apply the result on the foreground,
   refresh on a timer that stops when the entity is dropped, notify only when
   something changed. Test it in `tests/live.rs` with `testing::headless_app()`
   and a store on a `ManualClock`, moving the clock to fire the timer.
3. **The view.** A GPUI view in `src/views/<name>.rs` that holds no rules: it
   reads the model, lays it out with GPUI Kit's components, and writes what the
   user does back into the model (or emits an event the shell turns into a
   `navigate`). Colours come from `cx.global::<Theme>()` and nothing else. Cut
   long text with `format::truncate_*` and make no layout depend on content
   width. Give it `for tests` accessors (`bounds_of`, `center_of`) in the style
   of the existing views, so a test can click it without guessing pixels.
4. **The shell.** Add the `Screen` variant (`app/shell.rs`), draw it in
   `MainWindow::render`, give it a `toolbar_title` (`app/toolbar.rs`), and, when
   it is a sidebar entry, a `NavItem` (`app/nav.rs`: `ALL`, `label`, `icon`,
   `shortcut`, `screen`, `is_active`), with its action, its binding in
   `nav::bind_keys` and its handler on the shell's root element; the icon comes
   from Kit's Lucide catalog (`gpui_kit::assets::IconName`), and one that the
   default bundle lacks goes into `src/assets.rs` too (the list and the funnel
   did). A screen reached from another one (as a trace is from the
   table) is not an entry: it stays lit under its parent through `is_active`.
   Decide whether its views live for the window (like the home screen's) or are
   made on arrival and dropped on leaving (like the trace's), and say which in
   `ui.md`.
5. **The screenshots, test first.** In `tests/screens.rs` (`interactions.rs`
   when the state needs input, `live.rs` when time passes) render it with
   `render_window_png` or `Driver` over a `FixedBackend`, in `Theme::dark()` and
   `Theme::light()` at `WINDOW` (1440 by 900). Generate the references with
   `cargo xtask screenshots --update` and **open every PNG** before committing
   it; the `update-screenshots` skill has the steps.
6. **The docs, in the same change.** A section in this file (the view, its
   model, the screenshots, what is unverified on a Mac), a row in "Screens and
   views at a glance" and in "The references", the screen map there, the file
   map of the milestone plan if one is open, and a decision in `docs/decisions/`
   for any choice a later agent could undo by mistake. A change that adds a
   crate also updates `docs/architecture.md`.
7. **Run it for real.** `cargo xtask run-shot` (the `run-and-screenshot` skill)
   and look at `target/run-shot.png`: the headless harness and the real window
   share the code, but only the second one has a real event loop.

## The real window

`loctel` without `--headless` on Linux opens the **main window**
(`app::open_main_window(Rc::new(engine), cx, |_| None)`: no backdrop, so the
panels are opaque) and prints the same startup lines as headless mode (the
listeners, then the rules line), so a script that passed port 0 finds the ports.
The app registers `KitAssets` (`Application::with_assets`) so Kit's icons draw.

- **Closing the window quits**, with the engine shut down and exit status 0.
- **SIGINT and SIGTERM quit the same way.** The handlers are installed before
  anything is printed (as in headless mode). A thread waits for the signal and
  tells the app over a channel, because only GPUI's own thread may call
  `cx.quit()`; then `engine.shutdown()` runs and the process exits 0. Before
  Milestone 3 Task 9 a signal killed the window process abruptly.
- With no `DISPLAY` or `WAYLAND_DISPLAY` it says so on stderr and runs
  headless: GPUI would otherwise hand out windows that draw nothing.
- On macOS the window is not opened at launch: see [`macos.md`](macos.md).

`crates/loctel-app/tests/window.rs` (`linux_window_opens_under_xvfb`) runs the
built binary under `xvfb-run` for 3 s and checks that it was still alive, that
the four startup lines were printed, that stderr has no panic, and that SIGTERM
ends it with status 0. Without `xvfb-run` it prints `skipped: no xvfb-run` and
passes; CI installs xvfb, so CI never skips.

`cargo xtask run-shot` builds `loctel` and `meshgen` and runs `loctel` under
`xvfb-run` on a screen of 1440 by 900 (the window's default size, so the window
fills it) with every port 0 and a scratch settings file. It waits for the
window, gives it two seconds (the first frame), reads the OTLP HTTP address from
the listener lines, sends the failing checkout trace (`meshgen --scenario
checkout-failure --transport http-json`), waits two more seconds (the window
reads the store every 500 ms), captures the whole screen with `import -window
root` and writes `target/run-shot.png`. The shot has data: the map, with the
failing services outlined in red, above a table with one failed `POST
/api/checkout`. Xvfb reports the light appearance, so the shot is the light
theme. A
capture of one flat colour means the window stayed black, the old X11
wake-up bug of crates.io GPUI (§10.5); the script then nudges the window with
`xdotool windowmove` and captures again, and fails if it is still blank. With
`gpui-pre` the window was not black on its first try, so the nudge is a safety
net. Open the PNG with the Read tool.

The repo skill `run-and-screenshot` covers running with meshgen data.

## Unverified

- The main window's macOS parts: the transparent title bar with the traffic
  lights at (20, 20) over the sidebar's top strip, and how GPUI Kit's
  components look and behave on a Mac. Nothing of the main window has run on a
  Mac yet; the M3 check does it.
- The trace view on a Mac: how a 100,000-span waterfall scrolls with a trackpad
  and a fast flick (the test scrolls by an offset and by wheel events, the
  M3 check does it by hand), the ⌘F shortcut (the tests send `cmd-f`), and the
  look of GPUI Kit's scroll area, split handle and tooltips there.
- The trace table while traces arrive: the table keeps its scroll position in
  pixels and does not hold the user's place against rows arriving above it, so
  a table scrolled down while traffic flows drifts. Nothing anchors it (the
  tests only check the table at the top). Milestone 3, item 4 of the Mac
  checklist asks whether that is tolerable.
- The service map's input on a Mac: that a pinch, a two-finger drag and a
  cmd-scroll reach the map as `PinchEvent` and wheel events with the amounts
  the code assumes (a trackpad sends pixel deltas, a mouse wheel lines), and
  that the zoom feels right. The tests send the events GPUI documents; a real
  trackpad has not.
- The toolbar's controls on a Mac: that a click on `Pause`, `Clear all` and the
  range button reaches GPUI through the glass host that sits under the
  toolbar, that the range menu opens over the content (it is a GPUI popup in
  the transparent window), and that the buttons, which are opaque, look right
  on the glass.
- The trace table on a Mac: the monospace face of the `Started` column and the
  query input (the theme's `mono_font_family`; Linux shows Inter), Kit's input
  and toggles with a real keyboard and IME, the tooltip of the dots, and that a
  click on a row opens the trace with a trackpad. The tests drive GPUI's own
  event path on Linux only.
- The glass on a Mac: that Apple's glass really shows through the clear
  sidebar and toolbar, and that there is no seam where AppKit's rounded glass
  meets the hole in the window colour. The tests check the shell's side only,
  over a flat colour ([`macos.md`](macos.md), "Unverified until the M3 Mac
  check (Liquid Glass)").
- The main window's lifecycle on macOS (`Open loctel`, the Dock and Cmd-Tab
  while it is open, the Accessory policy after it closes, a reopen from the
  Dock) is listed in [`macos.md`](macos.md) under "Unverified until the M3 Mac
  check".
- The font on a Mac: the window draws in Inter, not in SF, and its monospace
  parts in whatever face Kit's `mono_font_family` finds there (see "The font").
  Whether that looks right next to a Mac's own windows is for the M3 check to
  say.
- The Settings window on a Mac (Milestone 4 Task 6): that the window opens at 520
  by 360 and cannot be resized, that asking for it twice brings the open one to
  the front, how Kit's inputs and switch look and behave in it, and that the
  `Start loctel at login` switch (`SMAppService`) follows System Settings and
  reports a refusal. The tests run Linux's headless platform with a fake login
  item.
- The popover on a Mac (Milestone 4 Task 8, the view; Task 9 hosts it): how the
  panel looks with Apple's glass behind it (the translucent tiles over real
  glass, the hairline and highlight, the clear window colour), that Esc, ⌘O, ⌘, and
  ⌘Q reach the view when the popover is the key window of an app that has no
  menu, and how Kit's switch and buttons look and behave in a panel that is not
  a normal window. The tests run Linux's headless platform with a recording
  `PopoverActions` and a flat stand-in for the glass; where the popover opens, how
  it closes on a click away, and what its actions do on a Mac are `macos.md`'s.
- Everything here was checked on Linux with software rendering. How the status
  view looks and behaves on macOS (fonts, scale, window chrome) is unverified
  until a milestone check on a real Mac.
- The references were made with Mesa lavapipe on Ubuntu 24.04. If the CI
  runner's Mesa renders anti-aliasing differently, `ci.yml` uploads the actual
  and diff images as the `screenshot-diffs` artifact; see the
  `update-screenshots` skill.
