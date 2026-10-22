# 0031: What Milestone 3 does not do that the design asks for

Date: 2026-10-22
Status: accepted (as a record; each gap is open until someone closes it)

## Context

The last task of Milestone 3 is the docs pass: the screen map, the view and
model tables, the screenshot list and the Mac checklist. Writing them meant
checking every claim against the code and the references, and three places
where the design (the spec, §9.2 and §9.3) asks for something the Milestone 3
plan has no task for, and the window therefore does not do. They are written
down here so that no later agent mistakes them for finished work, and so that the
Mac checklist can tell the person trying the release not to report them.

## The gaps

- **The toolbar has only its title.** Design §9.2 puts four controls in the
  home screen's toolbar: a Live indicator, the time range, Pause (which freezes
  the view while ingest continues) and Clear all. The models for all of them
  exist and are tested (`HomeModel::set_range`, `set_paused`, `TimeRange::label`,
  `UiBackend::clear_all`, Milestone 3 Task 5), and the plan's Global Constraints
  list the four ranges, but no task draws a control, so nothing in the window
  calls them: the window always shows `Last 15 minutes` and refreshes every
  500 ms. `docs/areas/ui.md` ("Screens and views at a glance") says so, the
  roadmap lists it under what Milestone 3 does not deliver, and the Mac
  checklist says not to report it. The controls belong with a screenshot of the
  toolbar in both themes, and they must write into the same `HomeModel` calls
  rather than add new state. (The macOS menu's `Pause receiving` is a different
  thing: it stops ingest in the engine.)
- **The font is Inter on every platform.** Design §9.3 says the system font (SF)
  and the system monospace. The harness cannot see system fonts (a pixel-exact
  reference must not depend on the machine), and Milestone 3 never made the
  switch: `kit_theme::apply` and the shell use the bundled `Inter`. Moving a Mac
  build to SF would change nothing in a Linux reference. The Mac checklist asks
  whether Inter looks out of place.
- **The trace table does not hold the user's place.** New traces arrive at the
  top while the table is open. With the table at the top this is what the plan
  asks for (no jump, `home_dark` and the live tests). Scrolled down, the scroll
  position stays in pixels, so the rows being read drift down as traces arrive
  above. Holding the place would mean correcting the scroll offset across a
  refresh by the height of the rows added above the viewport, and nothing in
  the table view does that today (the tests only look at the table at the top).
  Once Pause has a control (the first gap), it is a way to read a table that is
  moving. The checklist asks whether the drift is tolerable.

## Consequences

- The three gaps are in the docs (`ui.md`, the roadmap, the Mac checklist), so
  a reader of any of them is told. `ui.md` also has the list of what is
  unverified on a Mac.
- Whoever closes one updates this record's status line, the roadmap row of
  Milestone 3, and the sentence in `ui.md`.
