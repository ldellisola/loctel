# 0040: Sidebar navigation and cross-screen flows, where the plan leaves a choice

Date: 2026-10-22
Status: accepted

## Context

Milestone 4 Task 7 gives the sidebar its `Logs` and `Discard rules` entries
(with icons), binds ⌘1 to ⌘3 and ⌘, in the main window, keeps every screen's
state across switches and makes the trace view's back button return to where
the trace was opened from. The plan fixes the entries, the keys and the label
`Back`; the code keeps all of them. The choices below are the ones it does not
settle, or whose obvious reading fails, and that a later agent could undo by
mistake. Each names the test that pins it.

## Decision

- **The list and the funnel are not in GPUI Kit's default icon bundle, so
  `KitAssets` is now ours.** The plan says "Lucide from GPUI Kit's assets" for
  the map, the list and the funnel. `gpui_kit::assets::Assets` (what
  `KitAssets` was) bundles 106 icons that its components use, and `Map` is one
  of them but `List` and `Funnel` are not; `gpui_kit::component::IconName` has
  only those 106 variants. The full catalog is `gpui_kit::assets::IconName`
  (`AllAssets` embeds all of them, about 1,800 SVGs, for two). So
  `loctel_ui::KitAssets` is a unit struct (`src/assets.rs`) that answers
  `List` and `Funnel` from Kit's own `icon_assets!` and everything else from
  Kit's `Assets`, errors included. The apps register it exactly as before
  (`Application::with_assets(KitAssets)`) and `testing::headless_app` registers
  it too, so the screenshots draw the icons. `NavItem::icon` returns the full
  catalog's `IconName`. A new sidebar icon outside the default bundle goes in
  the `icon_assets!` list there. (`the_sidebar_icons_are_bundled`, the unit tests
  in `assets.rs`)
- **The shortcuts are GPUI actions in a key context of the shell's root, and
  the root takes the keyboard back whenever it is lost.** GPUI looks for a key
  binding's context from the focused element up, and from the root node alone
  when nothing is focused, so a binding scoped to `MainWindow` is silent unless
  the keyboard is somewhere in the shell. The shell therefore owns a focus handle:
  it takes the keyboard at creation, and `Context::on_focus_lost` gives it back
  after any frame in which nothing is focused (the element that had it is no
  longer drawn); a click on a bare part of the window focuses it too, because the
  root element tracks it. The first version only checked that the focused handle
  was gone, which the headless tests satisfied and the real window did not: a click
  in the home screen's empty table area focuses a handle that lives as long as the
  window, so after a switch to Logs it was alive but not in the frame, and every
  key after the first did nothing (found by driving the real window with
  `xdotool`; `shortcuts_work_after_a_click_in_a_part_of_a_screen_that_goes_away`
  reproduces it over an empty store). The bindings are `cmd-1`, `cmd-2`, `cmd-3`
  and `cmd-,` on every platform, as the plan writes them (the Super key on Linux).
  They are bound in `app::init`, once per app, in the context `MainWindow`, so the
  Settings window and the popover do not get them (the popover's own ⌘, is Task
  9's). (`keyboard_shortcuts_switch_screens`,
  `shortcuts_work_after_a_click_in_a_part_of_a_screen_that_goes_away`,
  `the_settings_shortcut_opens_one_settings_window`)
- **A trace stays under `Map & traces` in the sidebar and the toolbar title,
  even when it was opened from a log.** `Back` goes to the origin, but the lit
  entry and `Map & traces › <id>` do not follow it: the plan does not ask for
  that, `NavItem::is_active` and `toolbar_title` take a screen and nothing else,
  and a trace is a trace whichever way it was reached. If this is felt to be
  wrong on a Mac, lighting the origin's entry is a change to `is_active` plus the
  origin it would need, not a new mechanism. (`nav_items_are_lit_by_their_screens`)
- **`Back` returns to the origin *as it was left*, not to what opened the
  trace.** The shell notes the screen it leaves when a trace opens from another
  screen, and stores `NavItem::screen()` of it: `Logs { query: None }`, never the
  `query: Some(..)` that `Open in Logs` arrived with. A `Some` would put that
  query back into the bar on `Back`, over whatever the user typed since (the same
  rule `navigate` has for `Screen::Logs`). The origin is noted at each opening
  from another screen, so a trace opened from the table after one opened from Logs
  goes back to the table. Esc is the same as the button.
  (`trace_back_returns_to_logs_when_opened_from_logs`,
  `trace_back_returns_home_when_opened_from_home`)
- **Switching screens keeps state without new code.** The Logs and Rules models
  and views have lived as long as the window since Tasks 4 and 5 (they read only
  while their screen shows), and an entry's `screen()` is the "as left" form of
  each. The task's work is the entries and a test that pins it, including that an
  expanded log line is still open when the user returns, as the model keeps it.
  (`sidebar_switches_screens_and_keeps_state`)
- **The sidebar's entries are measured, like the other views' parts.** Each is a
  `relative` row with an invisible canvas that writes its bounds into a frame
  the shell clears every render; tests click `MainWindow::nav_bounds(item)`
  instead of computing the row from the layout constants, so a change of the
  spacing (`NAV_ITEM_GAP`) moves the test with it.
- **No shortcut hint is drawn on the entries.** The mockups are not reachable
  and the plan lists the keys, not a hint; the sidebar keeps the icon and the
  text, and the shortcuts are in the docs and the Mac checklist. Showing them
  later is a change to `nav_item` alone.

## Consequences

- Every reference with the sidebar changed (all `shell_*`, `home_*`, `trace_*`,
  `logs_*`, `rules_*`, `huge_trace_middle_dark` and `long_values_*`): the new
  entries, and for the trace screens `Back` where `Traces` was, which also moves
  the root name and the badge a few pixels left. Nothing else moved, which was
  checked pixel by pixel against the old references (outside the sidebar and the
  trace header, four pixels of antialiasing in `rules_file_error_dark` differ by
  one level).
- ⌘1 to ⌘3 and ⌘, from a real Mac keyboard are unverified until the M4 Mac check
  (`docs/areas/macos.md`).
