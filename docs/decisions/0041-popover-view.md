# 0041: The popover view, where the plan leaves a choice

Date: 2026-10-22
Status: accepted

## Context

Milestone 4 Task 8 builds the menu bar popover's view (`views::PopoverView`) and
the trait of what it asks of the app (`PopoverActions`), cross-platform. The plan
fixes the interfaces (`new(backend, actions, cx)`, the eight methods of the
trait, the one `GlassRegion` through an optional `Backdrop`), the rows, the exact
strings and the 336 px width and 18 px radius, and the code keeps all of them.
The choices below are the ones it does not settle, or settles in a way that two
readings would undo, and that Task 9 (which hosts the view on a Mac) and later
agents could undo by mistake. Each names the test that pins it.

## Decision

- **The view calls `close()` after the actions that open a window, and only
  then.** The plan says the popover closes "when it loses focus, on Esc, or after
  any action that opens a window", and gives the trait a `close()`. The view
  calls `open_main(..)` or `open_settings()` and then `close()` for `Open loctel`,
  `Settings…`, the rules line and ⌘O and ⌘,; Esc calls `close()`; pause, `Copy`,
  the login switch and a confirmed clear leave it open; `quit()` is not followed
  by `close()` (the app is going away). An implementation of `close()` must be
  harmless when the popover is already closing (it also closes on losing focus, so
  Task 9 sees both). (`popover_actions_dispatch`, `the_keyboard_works_in_the_popover`,
  `pause_and_resume_show_at_once_and_leave_the_popover_open`)
- **`PopoverActions` methods have no `cx`.** The plan lists them without one, so
  they are `&self` and the view calls them from inside its event handlers, where
  GPUI's `App` is borrowed. An implementation that must open a window therefore
  cannot borrow the app there; the trait's doc says to hold an `AsyncApp` and do
  the work on the foreground executor. This is for Task 9 to honour, and the
  reason the trait is not given `&mut App`: the view would then need it from every
  handler and the tests would need an app in the recorder.
- **The switch shows what the system holds, and the view re-reads everything after
  an action.** After `set_start_at_login(on)` the view asks the login item again
  (and the status again after `toggle_pause` and `clear_all`) instead of flipping
  its own copy, so a system that refuses leaves the switch where it was, and
  `Pause receiving` reads `Resume receiving` at once, not a second later. This is
  the rule of decision 0039 (the system is the source of truth, nothing mirrors
  it). Unlike Settings, the popover's switch has no `Apply`: it acts when clicked.
  (`the_login_switch_follows_the_system`, `pause_and_resume_show_at_once_and_leave_the_popover_open`)
- **The question replaces the row, at the same height.** `Clear all data…` turns
  into `Clear all traces and logs?` with `Clear` (red) and `Cancel` inside its own
  28 px row, so the popover does not grow and the glass region does not change
  (a new region means a new native frame on a Mac). Esc while it asks forgets the
  question and closes. (`popover_clear_confirms`,
  `the_height_does_not_change_while_it_asks_to_clear`,
  `esc_while_it_asks_forgets_the_question_and_closes`)
- **The panel is as tall as its rows, and says how tall.** Every row has a fixed
  height, and `popover_height(login_item)` (504 with the `Start at login` row,
  476 without) is the sum of the same constants, so Task 9 sizes the popover's
  window with it and `anchor` has a height. The panel is `POPOVER_WIDTH` wide in
  a window of any size; a taller window leaves the rest empty and does not change
  the reported region. (`the_popover_is_336_wide_and_exactly_as_tall_as_its_rows`,
  `the_region_is_the_popover_even_in_a_bigger_window_and_changes_only_with_it`)
- **The backdrop is attached with `with_backdrop`, not `new`.** The plan fixes
  `new(backend, actions, cx)` and says the view reports its region "through an
  optional `Backdrop`". `new` has no window, and `glass::attach` needs one, so the
  caller attaches the backdrop to the new view
  (`cx.new(|cx| PopoverView::new(backend, actions, cx).with_backdrop(backdrop))`).
  With one the panel has no fill, a translucent tile and group fill, a hairline and
  highlight (`Theme::glass_hairline`, `glass_highlight`), and the view clears Kit's
  `Root` background on the first frame, as the shell does; without one it is opaque
  (`Theme::bg`). The one `GlassRegion` is the panel's border box with radius 18,
  reported when it changes. (`popover_reports_one_glass_region`,
  `the_glass_shows_through_where_the_popover_has_no_fill`,
  `without_a_backdrop_the_popover_is_opaque`, `with_a_backdrop_the_window_colour_is_cleared`)
- **The view does the one-time window setup on its first frame.** Because `new`
  has no window, the theme from the window's appearance (and the observer that
  follows it), the keyboard and the cleared window colour are set up when the
  first frame is drawn. This matters on a Mac: the popover is the first window of a
  menu bar app, so nothing else has set the `Theme` global and the view must not
  read it before it has.
- **The keys are GPUI actions in a key context of the view's own, bound by
  `app::init`.** Esc, ⌘O, ⌘, and ⌘Q (the plan: "⌘Q works while the popover has
  focus, and ⌘, opens Settings"; ⌘O and Esc are the rows' and the closing rule's)
  run the same code as the rows, in the context `LoctelPopover`: not `Popover`,
  which GPUI Kit's own popover component binds its keys in. They work in a window
  with no menu, which is what the popover is on a Mac. The view takes the keyboard
  on the first frame and takes it back whenever what had it leaves the frame, as the
  shell does (decision 0040). GPUI Kit's buttons do not take the focus on a click
  (they prevent the default of the mouse down), so in practice only a stray focus
  loses it; the test makes one. (`the_keyboard_works_in_the_popover`,
  `the_popover_takes_the_keyboard_back_when_what_had_it_is_gone`)
- **The MCP row shows the URL without `http://`, and a failed endpoint says
  `failed`.** The model's address for MCP is the whole URL (what `Copy` copies), and
  with its dot, name and `Copy` button it does not fit in 336 px; the row shows
  `127.0.0.1:4319/mcp`. An endpoint that could not be bound is red, and the red is
  not the only sign (design 9.3): the row reads `127.0.0.1:4318 · failed`. The
  reason is Settings': the popover's model does not carry it. There is no `Copy`
  while MCP is not listening: the address would be a dead one.
  (`the_endpoint_rows_show_the_addresses_and_say_failed_in_words`,
  `a_failed_listener_is_a_port_problem_and_a_dead_mcp_address_is_not_offered_to_copy`)
- **The rules line's colour is the status view's.** The popover model carries the
  words (`rules_line`) and not whether they are a problem, so the view keeps the
  `StatusModel` the words come from and colours the dot as `StatusView`'s rules row
  does: red for a file or rule error, grey for none on, green otherwise. The
  model is unchanged (the plan's `Popover` has no such field).
- **Seven references, two beyond the plan's four.** `popover_dark`,
  `popover_light`, `popover_paused_dark` and `popover_port_problem_dark` are the
  plan's. `popover_clear_asks_dark` shows the question, and `popover_glass_dark` and
  `popover_glass_light` show the glass look over a flat stand-in for the desktop (the
  headless renderer cannot draw Apple's glass), as `shell_glass_*` do for the main
  window.

## Consequences

Task 9 builds `Popover` windows of `POPOVER_WIDTH` by `popover_height(true)`,
attaches `glass::attach`'s backdrop with `with_backdrop`, implements
`PopoverActions` (off the borrow of the app, see above), focuses the view
(`PopoverView` is `Focusable`) and closes the window when it is deactivated. It
calls `app::init` first, which binds the popover's keys. The anchoring is
Task 9's and its own decision.
