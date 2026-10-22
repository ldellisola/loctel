# 0027: Trace table view and query bar choices

Date: 2026-10-22
Status: accepted

## Context

Milestone 3 Task 7 fixes what `views::TraceTableView` shows (a `TraceQL` chip, a
GPUI Kit `Input` and `Run`; the toggles `Errors only` and `Slower than 1 s` with
a count; a Kit `DataTable` with six columns and 36 px rows; two empty states)
and what it does (typing validates live, Enter runs, a click on a row opens the
trace). The plan leaves open how a typed query is applied, how a row click gets
to the shell, how the columns get their widths, what `Clear filters` clears, how
the table is found by a test and a few details of the look. They are written
down so that a later agent does not "fix" them.

## Decision

- **Typing applies, Enter and `Run` read again.** The plan says "typing
  validates live" and "Enter runs it too". `HomeModel::set_query` (Task 5)
  already validates at once and starts a read when the text is empty or parses,
  and a click on the map relies on that (no Enter after a click). So every
  change of the input goes to `set_query`: a valid query filters on the next
  frame or two, an invalid one keeps the last good rows and shows `at column N:
  message`. Enter and `Run` call `refresh_now`, the way to see new traces while
  paused (a paused model reads for the user's changes, but a new trace is not a
  change). Intermediate texts while typing are mostly invalid, so they cost no
  read.
- **The model's text and the input's text are kept equal by copying only on a
  difference.** The input owns its text. The view copies the model's into it
  when they differ (a click on the map, `Clear filters`), with `set_value`,
  which emits no `Change`, so there is no loop; the input's `Change` event goes
  to the model as it is typed. `has_filters`, `clear_filters` and `loaded` are
  small additions to `HomeModel` (the plan's interface stays as it was).
- **A row click is the row's own `on_click`, not the table's `SelectRow`.**
  `TableEvent::SelectRow` also fires when the arrow keys move the selection, and
  a key press must not open a trace. The table's row selection is therefore off
  (`row_selectable(false)`), which also means no stale highlight while rows
  arrive at the top (a selected index would be another trace a moment later);
  the hover tint stays. Keyboard opening of a row is not in the plan and not
  done.
- **The view emits an event; the shell navigates.** `TraceTableEvent::Open(id)`
  is emitted by the view and `MainWindow` subscribes and calls `navigate`. The
  view does not know `Screen`, so it can be tested and reused without the
  shell.
- **The delegate reads the model.** The table's delegate holds the
  `Entity<HomeModel>` and reads `data().rows` when it draws, so there is no
  second copy of up to 1,000 rows to keep fresh and nothing to compare. The view
  only notifies the table when the model changes.
- **Column widths come from the window's width.** Kit's table has fixed column
  widths only. `Trace` and `Failed in` share what the four fixed columns and
  Kit's 12 px last column leave (40% and the rest, at least 140 and 170), from
  `content_width(window width) - 2` (the shell's own layout numbers). A
  measured width would be one frame late, and a screenshot is the second frame
  of a harness window. The result fits the smallest window's panel (748 px)
  without a horizontal scroll. Kit's column resizing and moving and sorting are
  off: the plan has no sort and a fixed order of columns.
- **`Clear filters` clears the query and the two toggles, not the range.** The
  time range is a window with a control of its own (the toolbar, with the home
  screen's other controls), and the empty table does not say which filter
  emptied it. `HomeModel::has_filters` is true for a query that runs (the last
  good one while the text is invalid), `Errors only` or `Slower than 1 s`;
  `clear_filters` also ends a service selection from the map. `No traces yet`
  is for a table with no filter; a table with only a narrow range and older
  traces in the store says `No traces yet` too (the range is not counted).
- **The first frame says nothing.** Until `HomeModel::loaded()` (the first read
  is back) an empty table draws no text, so `No traces yet` does not flash
  before the data. `refreshed_at` could not tell (a test store's clock can
  read 0).
- **Count line.** `N traces · newest first`, in the singular for one (`1 trace`),
  with a thousands separator (`1,000`), and `Showing the newest 1,000` in its
  place when truncated. It is a model function (`trace_table::count_line`) so
  the strings are pinned without a window.
- **Look.**
  - The toggles are Kit's outline `Toggle`s; Kit paints a checked toggle with a
    tint of the text colour that is nearly invisible on our theme, so a toggle
    that is on gets the accent at 16% fill, accent text and an accent border.
  - The duration bar is red for a failed trace, amber with errors inside and
    the accent otherwise (the text `Failed in` and `errors inside` carry the
    meaning too, so colour is not alone).
  - A cell that is a dot row shows at most five dots and `+N` after.
  - The `Duration` column is indented 12 px so the right-aligned span count
    before it does not touch it.
  - The error line takes room: the table moves down by one line while it is
    shown. Reserving the line's room all the time would cost every screen a
    line to save a jump while someone types a broken query.
  - The input and the `Started` column use the theme's `mono_font_family`. On
    Linux nothing monospace is bundled and the harness has no system fonts, so
    the references show them in Inter; a Mac's look is unverified.
- **The tooltip of the dots is GPUI's own `tooltip` with Kit's `Tooltip` view.**
  Kit's managed tooltip (the one its buttons use) is a crate-private trait, so
  a plain element cannot have it; the native tooltip with Kit's styled view
  looks the same. The test hovers the dots, advances the clock and checks that
  pixels of the row above change, and that they do not when the pointer rests
  on the trace's name.
- **How a test finds a part.** `TablePart` (`QueryInput`, `Run`, `ErrorsOnly`,
  `SlowOnly`, `Table`, `ClearFilters`) names the parts; each is wrapped in a box
  with a canvas that records its bounds in the frame's prepaint, and the record
  is cleared at the start of each render, so a part that went (the `Clear
  filters` button of a table with rows) is not found. `row_center(index)` is
  computed from the table's bounds, the 32 px header and the 36 px rows, for an
  unscrolled table. The click tests use real input through `Driver`.
- **An extra reference.** `home_no_match_dark.png` is not in the plan's list; it
  pins the `No traces match` state with its button, which no other reference
  shows. The references are 350 to 440 KB each, well under the 1.5 MB the plan
  allows, so the 1x fallback is not needed.

## Consequences

- The table stays correct while rows arrive: it reads the model at each frame
  and keeps no index.
- A later change to the home screen's layout that changes `content_width` moves
  the columns with it; the test `the_table_is_laid_out_inside_its_panel` checks
  the fit at the default and the smallest window.
- The trace view (Task 8) opens from `Screen::Trace(id)`; the table needs
  nothing more from it. "Traces through this span name" there writes a query
  with `HomeModel::set_query` and the input follows.
