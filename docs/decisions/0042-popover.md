# 0042: The popover is anchored from the mouse, and opens as a pop-up window

Date: 2026-10-22
Status: accepted

## Context

Milestone 4 Task 9 replaces the status item's menu with the popover
(`views::PopoverView`, decision 0041) and wires it up on a Mac: where it opens,
how it closes, what its actions do, and how Settings and the login item reach it.
Nobody can run any of it here: it is type-checked from Linux
(`script/check-macos.sh`) and unverified until the M4 Mac check.

The plan's evidence (reading `gpui-tray` 0.2.0): the activation callback
(`TrayBuilder::on_activate_with`) fires on a primary click **only when no menu is
attached**, and it does not report the item's position. So the popover cannot be
placed from the item.

## Decision

- **Anchor from the mouse.** The status item has no menu and its callback runs
  `popover::toggle`. That reads `NSEvent::mouseLocation()` (the click, within the
  moment it took the event to reach us), finds the `NSScreen` whose frame holds it,
  and places the window from that screen's `visibleFrame`. The numbers are a pure
  function, `anchor(click_x, screen_frame, visible_frame, size) -> (x, y)`, with
  tests that run in the Linux gate (`anchor_centres_under_click`,
  `anchor_clamps_at_right_edge`, `anchor_clamps_at_left_edge`,
  `anchor_below_menu_bar`, and others for a second screen, a Dock at a side and a
  visible frame narrower than the popover). The top is 6 points below the top of the
  visible frame, so under the menu bar whatever its height (a notch makes it
  taller); the popover is centred on the click and kept 8 points inside the visible
  frame. The reason it is the mouse and not the item: `gpui-tray` has no item frame.
  If a later `gpui-tray` reports one, replace `NSEvent::mouseLocation` with it; the
  rest stays.
- **`anchor` returns the window's origin on the display it opens on**, not in global
  coordinates. GPUI's macOS `open` places a window at `screen_frame.origin +
  (bounds.x, display_height - bounds.y)` for the display named by
  `WindowOptions::display_id` (the primary one without), so the popover opens on
  the clicked screen with `display_id` set to that screen's `NSScreenNumber` (a
  `CGDirectDisplayID`, which is what GPUI's `DisplayId` holds on a Mac) and the
  origin measured from that screen's top left. A click on a second display with its
  own menu bar therefore opens the popover on that display. If the screen under the
  mouse cannot be told, the first screen (the one with the menu bar) is used.
- **A `PopUp` window with no titlebar, over glass.** `WindowKind::PopUp` (a panel
  at pop-up level that does not activate the app by itself), `titlebar: None`, not
  movable, resizable or minimizable, 336 by `popover_height(true)` points,
  `WindowBackgroundAppearance::Transparent`, and the glass of `glass::attach` behind
  the panel (`PopoverView::with_backdrop`), or GPUI's `Blurred` background with an
  opaque panel and one line on stderr when there is none: the same `attach_glass`
  the main window uses. `attach` makes two glass hosts (the sidebar's and the
  toolbar's); the popover reports one region, so the second stays hidden.
  `loctel-macos` depends on `gpui-kit` for `Root`, which every loctel window sits
  in.
- **Closing.** The window closes when it is deactivated
  (`Context::observe_window_activation`: a click anywhere else), but only a
  deactivation that follows an activation counts, since the window starts inactive
  and a first report of "not active" must not close it. Esc and the actions that
  open a window close it through `PopoverActions::close`. A second click on the icon
  closes it too (`toggle`). `close` is idempotent, because the deactivation and the
  view's own `close()` can both arrive.
- **`PopoverActions` runs off the borrow of the app.** The view calls the actions
  from inside its event handlers, where GPUI's `App` is borrowed (decision 0041), so
  `MacActions` holds an `AsyncApp` and runs what needs the app (opening windows, the
  clipboard, quit, close) on the foreground executor, after the handler and in the
  order it was asked. Pause, clear and the login switch need no app and run at once,
  so the view reads their result right after it has asked.
- **The main window is shown through its handle.** The popover's `Open loctel` and
  rules line must open the main window *on a screen*. `loctel_macos::window` holds
  only the main window's `AnyWindowHandle`, and the downcast from it to `MainWindow`
  is UI knowledge, so it is `loctel_ui::app::show_screen(handle, screen, cx)`, which
  the Linux tests cover (`show_screen_*` in `tests/interactions.rs`). This is one
  public function the plan did not list, in a crate its Task 9 did not list.
- **Settings and the Dock icon.** `open_settings` sets `ActivationPolicy::Regular`
  and calls `open_settings_window`, so the window can take the keyboard (an
  accessory app's window cannot be typed into otherwise) and has a Dock icon while
  it is open. The policy after a window closes is no longer "the main window closed,
  so `Accessory`": it is set from the windows that are open, `Regular` while any
  loctel window other than the popover is, `Accessory` when none is. Otherwise
  closing the main window with Settings open would take the Dock icon from under
  Settings, and opening the popover would put loctel in the Dock for as long as it is
  open. The main window's own ⌘, calls `open_settings_window` directly and changes
  nothing: that window is open, so the app is already `Regular`.
- **The fallback never leaves loctel unreachable.** If the popover cannot be opened
  (not the main thread, no screen, the screen's display unknown, GPUI refusing the
  window) `toggle` opens the main window instead and logs the reason once.
- **The old menu goes with its code.** `menu.rs` (the rows as plain values, and
  their tests), the actions that only the menu used (copy, pause, edit rules, the
  login toggle), the 2 second refresh of the menu and `open_in_text_editor` are
  removed: the popover has the first three as its own rows and switch, nothing else
  describes a menu any more, and keeping plain values for a menu that no longer
  exists would test nothing real. The tests removed are `rows_are_the_designed_menu_in_order`,
  `rows_include_rules_header_and_edit_item`, `rows_have_open_first_after_headers`,
  `the_rules_header_is_whatever_the_rules_line_says`,
  `edit_rules_is_disabled_when_the_engine_has_no_rules_file`,
  `the_header_is_whatever_the_status_line_says`, `pause_reads_resume_while_paused`,
  `start_at_login_has_a_checkmark_only_while_enabled`,
  `copy_is_disabled_when_the_mcp_server_has_no_address` and
  `only_the_two_headers_are_not_actions`; what they pinned that still holds (the
  status and rules lines, the Pause/Resume words, the Copy button that is there only
  while MCP listens, the login switch) is pinned on the popover view by
  `tests/settings_popover.rs` (decision 0041). **`Edit discard rules…` is gone**:
  nothing in the plan's popover opens the rules file, the Discard rules screen
  replaces it, and `rules.toml` can still be edited by hand (the engine reloads it).
  ⌘Q and ⌘O stay bound for the main window and Settings (to `Quit` and `Open
  loctel`), where nothing else answers them without a menu.
- **`menu-state` stays on `gpui-tray`.** It forwards checkmarks and disabled items
  for a menu, which the status item no longer has; removing a feature flag the
  crate would simply ignore gains nothing and cannot be tried here.

## Consequences

- The popover's position, glass, closing on click-away, ⌘Q and ⌘,, the fallback and
  Settings' activation are **unverified** until the M4 Mac check; `docs/areas/macos.md`
  ("Unverified until the M4 Mac check") lists what to look at. The most likely
  surprises are named there: whether the mouse location at the callback is still the
  click's (the click reaches us through a channel), whether the panel counts as
  active after it opens, whether a no-titlebar panel shows traffic lights or a
  shadow, and whether a second click on the icon closes the popover and does not
  reopen it (if the deactivation arrives first, `toggle` sees it closed and opens it
  again; the fix is to ignore a click within a short time of a close).
- Anything that needs the popover's window from a place that holds the `App`
  borrowed must go through `MacActions::later`, not `AsyncApp::update`, which would
  panic on the borrow.
- A newer `gpui-tray` that reports the item's frame replaces the mouse in `place`.
