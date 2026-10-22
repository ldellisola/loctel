# 0025: Live home data choices

Date: 2026-10-22
Status: accepted

## Context

Milestone 3 Task 5 fixes the interface of `loctel_ui::app::{TimeRange,
HomeData, HomeModel}`: a model the home screen's views observe, with a 500 ms
refresh whose reads run on GPUI's background executor, and a filter that
combines the time range, two toggles and the query bar, limited to 1,000 rows.
The plan leaves open what pause freezes, what the filter does while the query
text is wrong, what `select_service(None)` clears, who owns the colours and the
map's memory, what time the map's rates are over for "Everything", and how a
slow read behaves. They are written down so that a later agent does not "fix"
them.

## Decision

- **A read runs on the background executor on a copy of its inputs; the pure
  part runs on the foreground.** The search over the traces and the service map
  (`ReadPlan::read`: a `Store` clone, the range, the toggles and the parsed
  query) are the expensive part and take the store's read lock, so they never
  run in a frame. The rows (`trace_table::rows`) and the layout
  (`map_layout::layout`) need `ServiceColors` and `LayoutMemory`, which belong to
  the model and are not `Send`; they are a pass over at most 1,000 summaries and
  a few dozen nodes, and run in `apply` on the foreground. `backend.status()`
  also runs there (an `Rc<dyn UiBackend>` cannot leave the thread); the shell's
  status box already reads it on the foreground once a second.
- **Only the latest read counts, and the timer never stacks reads.**
  `refresh_now` bumps a generation and the result of an older read is dropped
  when it arrives, because the executor may finish two reads in either order
  and the old filter's rows must not land under the new filter's controls
  (`a_replaced_read_is_dropped`). The 500 ms tick skips a period while a read is
  still running, so a slow search costs frames never and threads never.
  Something the user does always reads, even over a running read.
- **Pause stops the timer, not the user.** The view is frozen: no tick reads.
  But a change the user makes while paused (the query, a toggle, the range, a
  click on a service) or `refresh_now` still reads, because the user asked for a
  different list and a table that ignores the filter they just set would look
  broken. Resuming reads at once instead of waiting up to 500 ms.
- **An invalid query keeps the rows and the last good filter.** `set_query`
  validates at once (`HomeData::query` is the state of the text as typed) and
  reads only when the text is empty or parses. While the text is wrong the
  rows stay and the timer keeps reading with the last query that was good, so
  the table is live under the error line instead of frozen. The alternative, a
  frozen table, would also show rows that do not match the text, and would hide
  traces that arrive; the error line already says the text is not what is
  applied.
- **`select_service(None)` clears the selection, not the user's query.** The
  plan says `select_service` writes the service's filter into the query "or
  clears it". A click on empty space while the user has typed their own query
  (`{ status = error }`) would wipe it, which is a loss with no undo. The model
  remembers the service whose filter it wrote; `None` empties the query only
  while it is still that filter, and editing the text ends the selection
  (`selected_service()` is `None` again). A selection followed by `None` behaves
  exactly as the plan says.
- **Settings take the entity's context and read at once.** The plan lists
  `set_errors_only(bool)`, `set_slow_only(bool)`, `set_range(TimeRange)`,
  `set_paused(bool)` and `select_service(Option<String>)` without the context
  that `set_query` and `refresh_now` show. All of them take `cx: &mut
  Context<Self>`: they notify the controls and start the read, so a click is
  answered by the next frame or two and not by the next tick. Setting a value it
  already has does nothing. Reading accessors (`query_text`, `selected_service`,
  `range`, `errors_only`, `slow_only`, `paused`) are added for the controls.
- **The model owns the session's `ServiceColors` and the map's `LayoutMemory`,
  and hands the colours out.** The plan has `HomeData::map: MapLayout`, so the
  model lays the map out, and decision 0022's "the map view owns the memory"
  becomes "the model does": one layout per read, not one per view. The table's
  rows are built before the map, so a service's number is the order of its first
  appearance in the newest traces, then the map's order for any the table did not
  show. `HomeModel::colors()` returns the shared `Rc<RefCell<ServiceColors>>`
  for the trace view's waterfall (Task 8), so one service has one colour
  everywhere for the run.
- **The map is filtered by the range only.** The toggles and the query narrow
  the list; the map stays the picture of everything in the window (a click on
  a service highlights it there and filters the list).
- **A range is a lower bound on the trace's start.** `TimeRange::since(now)` is
  `now` minus the length, saturating at 0, and a trace that started exactly then
  is kept (`TraceFilter::since_unix_nano` is inclusive). A trace that is still
  receiving spans but started earlier is out of the range, like in the store's
  other searches. "Now" is the store's clock, so a `ManualClock` store controls
  it in tests.
- **"Everything" rates are over the time the data covers.** The map's counts
  are divided by the window's length (`map_layout`'s `window_secs`): 300, 900 or
  3,600 for the three ranges. `Everything` has no length, so it is from the oldest
  trace the store holds to now, at least a second, and 0 (zero rates) for an
  empty store. The oldest trace needed a store method,
  `Store::oldest_start_unix_nano()` (a pass over the traces that touches no span;
  tested in `loctel-store`'s `edge_cases.rs`), because the newest 1,000 rows
  cannot say how old the oldest of 200,000 is, and a window taken from them would
  overstate the rates a hundredfold on a busy store.
- **Observers are notified only when what is drawn changed.** `refreshed_at`
  moves on every read but is not drawn, and a read that finds the same map, rows,
  truncation flag and status does not call `notify`, so an idle store costs no
  frames. (`complete` is in the rows, so a trace going quiet does redraw.)
- **The window being visible is the model being alive.** The plan says the
  data refreshes "while the window is visible"; the timer lives as long as the
  entity, which the window owns, so closing the window ends it. Hiding a window
  without closing it (macOS, Task 9) is the lifecycle code's to handle if it
  matters; the model has no window handle to ask.
- **Exactly 1,000 matches is not truncated.** The read asks the store for 1,000
  rows and `truncated` is whether it has a next page, so the footnote `Showing
  the newest 1,000` appears only for the 1,001st trace.

## Consequences

- Every rule above is pinned by a test in `tests/live.rs` that names it. The
  tests run the model in `testing::headless_app()` (a headless app without a
  window) and move its clock; the store has its own `ManualClock`.
- Task 6 and 7's views call the setters with their context, read the accessors
  and observe the entity; they must not compute rows, a layout or a colour of
  their own, and must take colours from `HomeModel::colors()`.
- The store has one more method, with a test and a line in
  `docs/areas/store.md`. The MCP surface is unchanged.
