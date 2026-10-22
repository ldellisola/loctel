# 0032: The home toolbar's controls

Date: 2026-10-22
Status: accepted (unverified on a Mac: the clicks through the glass host and the
look of solid buttons on glass)

## Context

Design §9.2 puts four controls in the home screen's toolbar: a Live indicator,
the time range, Pause (which freezes the view while ingest continues) and Clear
all. The Milestone 3 plan had models for them (`HomeModel::set_range`,
`set_paused`, `TimeRange::label`, `UiBackend::clear_all`) and no task that drew
them, and decision 0031 first recorded that as a gap. The whole-branch review
sent it back: the roadmap says M3 is `per Home A`, which the user accepted, and
no later milestone owns it, so a fresh agent would never add it, and without it
the user is stuck on `Last 15 minutes` and cannot stop a moving table.

## Decision

- **The controls are in M3, over the existing calls and no new state.**
  `app::HomeControls` (`app/toolbar.rs`) is a small view the shell owns
  (`main.toolbar()`) and draws in the home screen's toolbar, right of the title.
  It observes the `HomeModel` and redraws when it changes; each control calls the
  model: `set_range`, `set_paused(!paused)` and `clear_all`. The only thing the
  view keeps is whether the range menu is open. The trace screen's toolbar stays
  the title alone (the open trace has its own refresh and no range).
- **`HomeModel::clear_all` is new, and tiny.** It calls `UiBackend::clear_all`
  and then `refresh_now`: the clear must show at the next frame even while the
  view is frozen (the user asked), and the model is where the sequencing lives
  and is tested (`tests/live.rs`). The filters, the range and the pause stay.
- **Live shows the view, not the engine.** A green dot and `Live` while the view
  runs; an amber dot and `Paused` while it is frozen. The engine's own pause
  (the macOS menu's `Pause receiving`) is the sidebar status box's, and the two
  can differ (a live view of a paused engine shows nothing arriving).
- **Pause becomes `Resume`**, filled with the accent while the view is frozen, so
  a frozen table cannot be mistaken for a quiet system. Resuming reads at once
  (`set_paused(false)` already did).
- **The range is a button with a menu, not four buttons.** At the smallest
  window the toolbar has 748 px, and the four labels the Global Constraints fix
  (`Last 5 minutes` and the others) as segments do not fit with the title and
  the other controls. The button shows the current label and a caret and opens
  Kit's `Popover` with the four ranges (`TimeRange::ALL`), the current one lit.
  The popover is controlled (`open` and `on_open_change`) so the test knows when
  it is open and the parts' places are forgotten when it closes; a choice closes
  it, a click outside closes it and changes nothing. The menu is drawn over the
  content (a deferred popup), not clipped by the toolbar's frame.
- **Clear all has no confirmation.** Design §5 says `Clear all` empties
  everything immediately; the macOS menu's `Clear all data…` asks because it is
  a menu item that is easy to hit by accident. The window's button is a
  developer's reset between runs of a service. If it turns out to be hit by
  accident, a confirmation is a later change in `HomeControls` only.
- **Buttons are solid.** They use Kit's default size (26 px, matching the Live
  pill) and its outline style, so on the glass toolbar they are opaque chips
  with a hairline. That is the design's "buttons are drawn by GPUI" (§9.3). How
  they look on real glass is on the Mac checklist (item 8).
- **Screenshots.** `toolbar_{dark,light}` (running), `toolbar_menu_*` (the menu
  open) and `toolbar_paused_*` are crops of the toolbar strip (and of what hangs
  under it), so a change of the controls shows in a small diff; the full-window
  `shell_*` and `home_*` references carry the controls too.
- **The decision log of the gap is updated**, not deleted: 0031 says the first
  gap is closed, and `ui.md`, the roadmap row of M3 and the Mac checklist
  preamble no longer say the controls are missing.

## Consequences

- `trace_charges_selected_dark.png` changed by one pixel (a value of 204 became
  203 in one channel of one antialiased glyph edge) because the glyph atlas now
  holds the toolbar's new text before the trace screen is drawn: not a visual
  change. It was regenerated and looked at.
- A later Logs or Discard rules screen adds its own toolbar parts the same way
  (`toolbar_contents` takes the screen); nothing here is shared with them except
  the title.
