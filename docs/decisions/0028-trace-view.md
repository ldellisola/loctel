# 0028: Trace view choices

Date: 2026-10-22
Status: accepted

## Context

Milestone 3 Task 8 fixes what `views::TraceView` shows (a header, a waterfall
over the selected span's logs, an inspector) and what it does (selection, folding,
find, the errors path, live refresh while the trace arrives). The plan leaves open
how the screen is made by the window, where the data lives, how the refresh is
timed, what Esc does with the find box, how rows are drawn in detail and a few
more. They are written down so that a later agent does not "fix" them.

## Decision

- **The data is a `TraceModel` entity (`src/app/trace_data.rs`), the views are
  thin.** This follows the home screen (`HomeModel` with `MapView` and
  `TraceTableView` over it): the snapshot, the `Waterfall`, the selection, the
  find matches, the details and the logs live in one entity that the four views
  (`TraceView`, `WaterfallView`, `InspectorView`, `SpanLogsView`) observe and
  write into. The plan's file map lists only the four views; the model is the
  one addition to `src/app`. The views never hold a second copy of the rows.
- **The window makes the screen when it first draws it.** `MainWindow::navigate`
  has no `Window` (the plan's signature, Task 4) and the find input needs one.
  `render` makes the `TraceView` for `Screen::Trace(id)` if there is none for
  that ID, with the home screen's `colors()`, and `navigate` drops it when the
  screen changes (the model's refresh stops with it: its chain of timers ends
  when the entity is gone). `main.trace_view()` is `None` until the first
  frame. A trace reopened starts from its initial selection again.
- **`TraceView::new(backend, trace_id, window, cx)` is the plan's. `with_colors`
  is an extra** that takes the session's `ServiceColors`, which is what the
  window calls: `HomeModel::colors()` says "do not make another", and a service
  must keep its colour from the map to the waterfall. `new` makes colours of its
  own, for a view that stands alone (the `live.rs` tests).
- **The views read a `Theme` global that the window sets up.** A trace view used
  as a bare root view (the live tests) must call `kit_theme::apply` first; the
  window does it at creation.
- **Events up, not calls.** `TraceViewEvent::Back` and `Query(text)` go to the
  window, which navigates (`Query` first writes the text into the home model);
  the inspector emits `InspectorEvent::TracesThrough(name)` and `TraceView`
  turns it into `Query(span_name_filter(name))`. Nothing below the window knows
  `Screen`.
- **Refresh timing.** The plan says every 500 ms until complete, then every 2 s.
  Each read, when it comes back, starts the wait for the next one with the
  interval of the state it just brought in (a first version slept before reading
  and used the state of one read earlier, which made a trace that just went
  complete wait one more 500 ms). A read the user asked for (`refresh_now`)
  replaces the wait. A complete trace that holds the spans the model has is only
  counted under the store's read lock (`with_trace_spans`), not copied.
- **The waterfall is built on the foreground.** The store reads are on the
  background executor (the plan's rule), but `Waterfall::new` and `rebuild` need
  the session's `ServiceColors` (an `Rc<RefCell>`, not `Send`) to give services
  their numbers, as `HomeModel::apply` does for the table and the map. A
  100,000-span build is about 0.3 s in a debug build (the model test's budget is
  1.5 s) and a fraction of that in release, once when the trace opens and again
  when spans arrive; the hitch is accepted. If it shows up on a Mac, build on the
  background with a clone of the colours and adopt it when the colours have not
  grown in the meantime.
- **A refresh that brings nothing new does nothing.** A snapshot with the span
  count and completeness the model already has is dropped (so a trace that is
  complete is never rebuilt), and observers are told only when something they
  draw changed.
- **The selection is by span ID and starts at `initial_selection`.** It is made
  as soon as the model has a span to select (a trace opened before its root
  arrives, and with no failure yet, selects the root when it comes), and
  `Reveal` asks the list to scroll to it. After that only the user (a click on a
  row, Enter in find) moves it. The placeholder row cannot be selected (a click
  on it folds it) and `select` ignores an ID with no span.
- **Find.**
  - Typing marks the matching rows (the `warn` colour at 12%) and counts them.
    It does not move the selection: moving it on every keystroke would make the
    inspector and the logs flicker through spans.
  - Enter selects the next match below the selected row, wrapping.
  - The count `2 of 5` is derived: which match the selected span is, if it is
    one, so it is right after any rebuild, fold or click, with no state of its
    own to go stale.
- **Esc goes back, except that it first empties a find box that has the keyboard
  and text.** The plan says Esc does what `Traces` does. A user who types in
  Find and presses Esc means to cancel the search, not to leave the trace and
  lose the place. With the box empty, or not focused, Esc is `Back`. The screen
  takes the keyboard when it opens, and a click on a row or a button gives it
  back, so Esc works without a click and after one. **⌘F** (the platform key) or
  **Ctrl-F** focuses the find box: the tests send `cmd-f`, which on Linux is the
  platform modifier, not Control.
- **Header on two lines.** The plan lists seven things in the "header row"; at
  the default window they are 1,250 px wide and the panel 1,188. The first line
  is the back button, the name, the badge and the indicator; the second the
  numbers, the ID and `Copy ID`. The badge has a `!` in a circle: colour is never
  alone (design 9.3).
- **Rows.**
  - The label column is 42% of the row; the note after the name is the service
    and, for a call, `→` the other side (`payments-api → card-gateway`), at most
    190 px, and cut before the name is (both shrink).
  - An erroring bar is the service colour at full opacity with a 1 px red outline
    (the plan says 0.75 opacity "unless erroring or selected" and gives no
    other colour); the `!` circle carries the meaning.
  - The exception diamond is a path in a `canvas` (a rotated square cannot be
    drawn with a `div`), a 9 px diamond in the text colour inside a ring of the
    panel colour so it reads over a bar and over the background.
  - Faint lines at a quarter, a half and three quarters run down the timeline.
  - The first/last ruler labels sit at the ends of the timeline, the others are
    centred on their lines.
- **The inspector.**
  - Keys are 44% of the row (38% cut `deployment.environment`).
  - Values are cut at 200 characters with `… (show all)` and `(show less)`; the
    expanded state is per value and cleared when another span is selected.
  - The exception's stack trace is one `div` per line (GPUI has no `pre-wrap`),
    which wraps a long line where it has to.
  - GPUI breaks a word that is longer than the line at the character, so a
    300-character name or a 600-character token wraps in the pane and never
    widens it (`long_values_do_not_break_layout`).
  - `Copy span as JSON` is `serde_json::to_string_pretty` of the model's `Span`
    (`serde_json` is a new dependency of `loctel-ui`, already in the lockfile).
- **The logs.**
  - The badge letters are `TRC DBG INF WRN ERR FTL`, and `LOG` for a log with no
    level.
  - A log's body is one line (line breaks become spaces), cut with an ellipsis;
    the whole is in a tooltip.
  - The panel is 200 px, a `uniform_list`.
  - Before the first read of a span's logs the panel shows no `No logs` text.
  - "Open in Logs" (design 9.2) waits for the Logs screen (Milestone 4).
- **New pure code, in the models where the rules belong.**
  - `model::trace_view`: the header, `find_status`, the log lines.
  - `query_bar::span_name_filter`.
  - `inspector::{value_preview, exception_text, span_json}`.
  - `trace_table::thousands` is now `pub(crate)`, so `15 spans` and `100,000
    spans` write numbers as the table's count line does.
  - The tests are in `tests/model_trace_view.rs`.
- **How a test finds a part.** `TracePart` names the parts (`Back`, `CopyId`,
  `Find`, `ErrorsPath`, `ToggleAll`, `List`, `Inspector`, `TracesThrough`,
  `CopyJson`, `CopyException`, `ShowAll`). As on the trace table, each is wrapped
  in a box with a canvas that records its bounds in the frame's prepaint, and
  the record is cleared at the start of each render; each of the three child
  views keeps its own record, and `TraceView::bounds_of` asks the right one.
  `ShowAll` is the last long value's link (a test with two long values clicks the
  last). A row's place is computed from the list's bounds, the scroll offset and
  the 28 px rows; `top_row` also comes from the offset, because
  `UniformListScrollHandle::logical_scroll_top_index` reads the handle's child
  bounds, which a uniform list does not fill.
- **Extra references.** `long_values_shown_whole_dark.png` is not in the plan's
  list; it pins the wrapped 600-character value and the pane's scrollbar. The
  new references are 410 to 740 KB each, under the 1.5 MB the plan allows, so
  the 1x fallback is not needed.

## Consequences

- A change to `content_width` or to the panel's layout moves the trace screen
  with it; `the_trace_screen_has_the_planned_layout` and
  `the_trace_screen_fits_the_smallest_window` check the numbers.
- The waterfall's cost for a very large trace is the foreground build. If a Mac
  shows a hitch, the way out is above.
- Task 9's window lifecycle needs nothing from this screen: closing the window
  drops everything, and the screen's timers end with the entity.
- Every later screen that shows a span or a trace (Logs, "Open trace") can use
  `TraceModel` and `TraceView::with_colors` as they are.
