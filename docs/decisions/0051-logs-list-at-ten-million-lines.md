# 0051: The Logs list at ten million lines

Date: 2026-10-22
Status: accepted

Milestone 5, Task 7. The plan fixes the interfaces (`model::window::RowWindow` with
`new`, `top`, `scroll_px`, `jump_fraction`, `visible`, `set_total(total, Anchor)` and
`thumb`; `Anchor::{Top, Row(LogId), Fraction}`), what the list does for an indexed
search (`log_count` and `logs_at` through a window, its own 8 px scrollbar, the visible
rows and a viewport on each side, the tail anchored), what every other search does (the
paged list of Milestone 4, with its footer), the footer (`10,000,000 lines · newest
first`) and the words of a trace that was evicted. This file records what it left open,
and the three places where the plan and the code that exists meet.

## Decisions

### A list is windowed only when it has more lines than a paged list holds

The plan says an indexed search "uses `log_count` plus `logs_at` through a `RowWindow`".
Taken for every indexed search, it would also turn the default view of a store of 207
lines into a window, and the Milestone 4 tests pin what the paged list does with it:
`rows()` holds every row of a list under 20,000 lines (the tail's join of 237 rows, the
`500+ lines` count, the page that is read near the end, the expanded line that moves down
by what the tail adds). Those tests also state a product rule worth keeping: a small
list is complete in memory, a line expanded in it is part of the list, and the Kit's
scrollbar and the `list` of variable heights do all they did.

So the window starts where the paged list ends. `model::logs::WINDOWED_ABOVE` is
`MAX_ROWS`, 20,000: **an indexed search that matches more lines than that is windowed;
any other list is paged, as before.** The mode is decided every time the search is
read from its first page (a change of the query, the toggles or the range, `Run`,
`Clear all`, leaving the lines around a log), by counting the lines (`log_count`, which
costs about the number of chunks) in the same background read that would have read the
first page. A paged list whose tail reaches the cap reads again, which makes it
windowed. The list never changes mode behind the user's back for any other reason, so a
total that hovers near 20,000 does not flip it; and a windowed list stays windowed
until the search is read again.

The one Milestone 4 test this touches is `the_list_stops_at_20000_rows_with_a_footer`,
which loaded 21,000 lines with no query and asserted the cap and its footer. Such a list
is windowed now, by the plan's own rule, and its footer is the total. The test's
**assertions are unchanged**: its setup now opens the screen with `{service_name=~".+"}`,
a regex on the service that the store cannot count and that matches every line, which is
the plan's "any other query: the M4 paging (newest 20,000) as before, with the footer".
The windowed list over the same 21,000 lines is pinned by `logs_scale.rs`
(`the_paged_list_stays_paged_up_to_the_cap`, `indexed_and_query_paths_switch`). No other
test and no reference screenshot of Milestone 4 changed.

### What `Anchor::Row(LogId)` does

`RowWindow` is pure and holds no rows, so it cannot find where a `LogId` went. The
anchor's contract is that of a newest-first list: lines that arrive are above the row,
so a total that grew moves the window down by the growth; a total that shrank lost its
oldest lines, which are below, so it does not move. The model uses it as the estimate
and then makes it exact: a read by the row under the top line counts the matching lines
newer than it (a second `log_count`, with `since` moved just past the row), reads the
rows from there, and finds the row among them (ties of time, a line that arrived late
above it, lines that left the window at its old end are all inside the margin of a
viewport) and `set_top`s the position it is at. A read that was out while the user
scrolled applies its total and its rows but not its position. `set_top`,
`scroll_rows`, `set_viewport_px`, `page_rows`, `drawn_rows`, `max_top`, `fraction` and
`fraction_for_thumb_centre` (a press on the track puts the middle of the thumb under the
pointer) are additions to the plan's list.

### The tail of a windowed list does not pause

In the paged list scrolling away pauses the tail, so that nothing is put above rows being
read. In a windowed list nothing needs to be: the row stays under the top line while the
total grows (`tail_keeps_anchor_when_scrolled`). The pill `New lines · Jump to newest`
still shows away from the top (a click puts the window at the top), and the toolbar's
amber dot with it, since `tail_paused()` means "not following the newest". With the tail
off the list is frozen at the newest line it had (the reads leave out `until` that time),
so scrolling it is stable; turning the tail on catches up with the same row on top.

### The histogram counts instead of scanning

The M4 histogram filter carried the parsed query, which the store can only answer by
evaluating it on every line in the window: a read of ten million lines every two
seconds. When the search is indexed (`indexed_filter`) the histogram is now read with the
equivalent `LogFilter` of a service and levels, which `log_histogram` answers from what
it keeps about each chunk. The same lines match, so the paged list's histogram is
unchanged (the Milestone 4 tests that read it pass as they were); a search with a line
filter still scans, as the plan says.

### Level matchers are the level labels

`level="error"` keeps the severity `Error` and not `Fatal`, though the `ERR` toggle holds
both: LogQL's `level` label has one value per severity (`unknown`, `trace`, `debug`,
`info`, `warn`, `error`, `fatal`), and `indexed_filter` judges each of the seven against
the matcher, so all four kinds of matcher (`=`, `!=`, `=~`, `!~`) are indexed, and the
set is narrowed by the toggles. A query the indexed path takes keeps every line the
paged list's query would have kept.

### The trace that the disk cap removed

`LoadState::Evicted` is new: a trace that was on the screen and is gone from the store
(`TraceRead::Gone` with a snapshot held). Before, the screen kept what it showed; now it
says `This trace is no longer held (the disk cap removed it)` (`NO_LONGER_HELD`, the
plan's words) in the middle of the body, drops the failure badge and `Complete`, and
keeps the header with its `Back` button (the plan's `Back` button is the header's: a
second one in the body would be two of the same part). The words are the plan's even
for a trace that left because of the trace limit or `Clear all`. A trace that comes back
shows again (`a_trace_that_comes_back_shows_again`).

### The 10M test builds its store with an optimised store crate

The plan: the 10M test "must take under 60 s in debug. If it can't, build its store with
fewer, longer chunks through a test-only bulk path, and say so in the decision log."
Measured in a debug build, ingesting ten million bulk logs took 47 s (4.7 us a log: the
per-log work of the open chunk, a `BTreeMap`, a `HashSet` and the counts, all unoptimised),
and compressing is a C library built with `-O0`. Fewer, longer chunks do not help (a chunk
of a million logs: 3.7 us a log). So the workspace `Cargo.toml` builds **`loctel-store`
and `zstd-sys` with `opt-level = 3` in the dev profile**: 1.1 us a log, the ingest of ten
million in 11 s. Debug assertions, overflow checks and the line tables stay as they were;
only the speed of two crates changes, which also shortens every store-heavy test of the
workspace. No test-only way into the store was added. The test makes and converts its
batches on a second thread (`loctel_meshgen::bulk_logs` through the real OTLP converter,
as `cargo xtask perf` does), which is most of its 30 s.

## Measurements

Debug build, this container (4 CPUs):

| | |
|---|---|
| ingest of 10,000,000 bulk logs, store unoptimised | 47 s |
| the same, store and `zstd-sys` at `opt-level = 3` | 11 s |
| `ten_million_scroll_to_middle_dark`, whole | 31 s |
| the rest of `logs_scale.rs` (11 tests) | 6 s |

## Not verified

How the scrollbar's press and drag, and the wheel of a trackpad, feel on a Mac is unverified
(the tests drive GPUI's own events on Linux), like the rest of the Mac-only behaviour.
