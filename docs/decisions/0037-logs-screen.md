# 0037: The Logs screen, where the plan leaves a choice

Date: 2026-10-22
Status: accepted

## Context

Milestone 4 Task 4 builds the Logs screen (`LogsModel`, `LogsView`,
`HistogramView`, the toolbar controls, and the `Open in Logs` button of the trace
view). The plan fixes the interfaces and the exact strings, and the code keeps
all of them. The choices below are the ones it does not settle, or settles in two
ways that cannot both hold, and that a later agent could undo by mistake. Each
names the test that pins it.

## Decision

- **The list is GPUI's variable-height `list`, not a `uniform_list`.** A click
  "expands it inline", and the expanded part (the whole body wrapped, a grid of
  labels, three buttons) is taller than a 30 px row. A `uniform_list` has one
  height for every row. The view owns the `ListState` and keeps it in step with
  the model through `LogsModelEvent` (`Reset`, `Prepended`, `Appended`,
  `Truncated`, `Resized`, `Reveal`, `Top`); a splice leaves the new rows without a
  height, so the view hints them with the row height, and every frame checks that
  the list's count is the model's (a safety net, not the mechanism). The GPUI
  gotcha that shaped it: a splice at the top of a list whose scroll position is
  `Some` shifts the scroll to keep the viewport's rows, so the tail's prepend
  scrolls to the top explicitly, and only when the list was at the top.
  (`tail_dedupes_and_paused_tail_keeps_position`, `clicking_a_row_expands_it_...`)
- **The toolbar is the shell's glass toolbar, with controls the view owns.** The
  plan lists "the toolbar" as the first part of `LogsView`. On the home screen the
  toolbar controls are in the shell's glass strip (decision 0032), so the Logs
  screen does the same: `LogsView::controls()` is an `Entity<LogsControls>` that
  the shell places in its toolbar next to the title `Logs`, and the rest of the
  view is the two panels below it. The range menu is the home toolbar's menu code
  (`range_menu` takes a callback), not a copy.
  (`the_toolbar_has_the_live_tail_pill_the_range_and_clear_all`)
- **The model and the view live for the window; the model reads only while its
  screen shows.** Task 7 wants the logs query and tail kept across screens, which
  an entity made on arrival and dropped on leaving (the trace screen's way) cannot
  do. `MainWindow::new` makes both, with the home screen's service colours, and
  `navigate` tells the model whether its screen is showing (`set_active`): while
  it is not, neither timer reads (a histogram is a pass over the window). Coming
  back it reads the first page the first time and otherwise catches up the way the
  tail does. (`a_hidden_logs_screen_reads_nothing`,
  `the_logs_screen_keeps_its_query_and_rows_while_another_screen_shows`)
- **The count line counts the rows loaded.** `N lines · newest first` has no
  cheap `N` for a list read in pages: the store has no count, and the histogram is
  a pass over the window and is not the list's. So `N` is the number of rows
  loaded, with a `+` while another page is waiting (`500+ lines · newest first`),
  and at the cap the plan's `20,000+ lines`. An exact count is when everything that
  matches is loaded. (`the_count_line_counts_loaded_lines_and_marks_more_to_come`)
- **The live tail joins the newest page, or starts over; it never repeats a
  line** (plan Review Focus 1). `model::logs::merge_head` is the join: the fresh
  page followed by the held rows older than it, so every row is strictly older
  than the one before. A page that ends newer than the newest row held (a flood
  that outran a page) or an empty page (the store was cleared) cannot be joined
  and the list starts over from it; losing the rows below the page costs nothing,
  because the list is at the top when the tail runs. A read that was out when the
  user scrolled away, or turned the tail off, is dropped when it arrives, and a
  read of the next page whose cursor the join moved is dropped too, so a stale
  read cannot repeat rows. (`whatever_arrives_the_joined_list_is_strictly_newest_first`,
  `tail_dedupes_and_paused_tail_keeps_position`)
- **"At the top" is the view's to say.** The list's scroll position lives in the
  view's `ListState`. The view reports it after each frame in which it changed
  (`set_at_top`, deferred out of the frame), and every action that puts the list at
  the top itself (a new search, the pill, `Back to search`) says so with
  `LogsModelEvent::Top`, so the two cannot disagree for more than a frame.
- **The histogram's 2 s timer runs only while the tail is on.** The plan says it
  is recomputed every 2 s; with the tail off the list stands still, and a
  histogram that went on counting new lines under a list that does not show them
  would disagree with it. A change of the query, the levels or the range reads the
  histogram at once, tail or not, and so does switching the tail back on.
  (`the_tail_toggle_stops_the_list_and_the_histogram`,
  `the_histogram_is_read_again_every_two_seconds`)
- **The pill shows whenever the tail is paused by scrolling**, not only when new
  lines are known to have arrived: a paused tail reads nothing, and reading just to
  find out would be the work the pause is there to stop.
- **Lines around a log read oldest first.** The search list is newest first; the
  lines around a log are what you read like a file around a line, so
  `show_around` shows the store's `logs_around` order, with the target highlighted
  and scrolled to the middle, and the count says `oldest first`. 25 lines are read
  on each side (`CONTEXT_BEFORE`, `CONTEXT_AFTER`). `Back to search` and any change
  of the query, the levels or the range read the first page again; the list does
  not remember where it was. (`show_lines_around_this_one_and_back`)
- **The range is the Logs screen's own.** The plan says "the range dropdown
  (shared with home)", and shares the dropdown, not the setting: `LogsModel` has
  `set_range` of its own and the home screen's range is untouched. Both start at
  `Last 15 minutes`.
- **Under `Everything` the histogram starts at the oldest log held.** That needs
  `Store::oldest_log_unix_nano`, added to the store in this task (a lookup at the
  front of the time order; `oldest_log_time_is_the_earliest_held`). Without it a
  window of "everything" has no start to count buckets from.
- **`Open trace` selects the log's span through the trace model.** `Screen::Trace`
  carries only the trace's ID, so `MainWindow::open_trace_at(id, span)` keeps the
  span and gives it to the new trace screen as `TraceModel::prefer_selection`,
  which selects it as soon as the trace has it (the failed-in span stands in while
  the trace is still arriving) until the user selects a span themselves.
  (`open_trace_from_log_selects_span`)
- **Navigating to `Screen::Logs { query: Some(q) }` applies `q` every time.** The
  screen value compares equal to itself, so `navigate` would do nothing the second
  time, after the user had typed something else; it sets the query before it
  compares. (`navigating_to_logs_with_a_query_applies_it_each_time`)
- **`Open in Logs` changed seven trace references.** The span-logs header has a
  button now; the diffs of `trace_dark`, `trace_light`, `trace_charges_selected_dark`,
  `trace_errors_path_only_dark`, `huge_trace_middle_dark`, `long_values_dark` and
  `long_values_shown_whole_dark` were checked to be that button and nothing else.

## Consequences

- Task 7 puts the sidebar entries and shortcuts on top of what is here and
  regenerates the `logs_*` references with the sidebar's new look.
- The 10M-line scroll is M5: the list stops at 20,000 rows, and the per-tick
  cost of the tail is a page of 500 clones, the same at any store size.
- The level badges of the rows follow the plan's colour table (`INF` is
  `text_secondary`), which differs from the span-logs panel's badges (`INF` is the
  accent): they are two lists and keep their own looks.
