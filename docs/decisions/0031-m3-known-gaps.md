# 0031: What Milestone 3 does not do that the design asks for

Date: 2026-10-22
Status: accepted (as a record; the first gap is closed, the other two are open until someone closes them)

## Context

The last task of Milestone 3 is the docs pass: the screen map, the view and
model tables, the screenshot list and the Mac checklist. Writing them meant
checking every claim against the code and the references, and three places
where the design (the spec, §9.2 and §9.3) asks for something the Milestone 3
plan has no task for, and the window therefore does not do. They are written
down here so that no later agent mistakes them for finished work, and so that the
Mac checklist can tell the person trying the release not to report them.

## The gaps

- **The toolbar has only its title.** *Closed (decision 0032).* Design §9.2 puts
  four controls in the home screen's toolbar: a Live indicator, the time range,
  Pause (which freezes the view while ingest continues) and Clear all. The
  models for all of them existed and were tested (`HomeModel::set_range`,
  `set_paused`, `TimeRange::label`, `UiBackend::clear_all`, Milestone 3 Task 5),
  and the plan's Global Constraints list the four ranges, but no task drew a
  control, so nothing in the window called them: the window always showed `Last
  15 minutes` and refreshed every 500 ms. The whole-branch review found this
  while the roadmap row of Milestone 3 says `per Home A`, which the user
  accepted, and no later milestone owns it, so it was built in Milestone 3: the
  four controls are in the home toolbar over the same `HomeModel` calls, with
  the toolbar in both themes in the screenshots. (The macOS menu's `Pause
  receiving` is a different thing: it stops ingest in the engine.)
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
  Pause has a control now (the first gap, closed by decision 0032), and it is a
  way to read a table that is moving; the drift is what a user who scrolled down
  without pausing sees. The checklist asks whether the drift is tolerable.

## Consequences

- The two open gaps are in the docs (`ui.md`, the roadmap, the Mac checklist),
  so a reader of any of them is told. `ui.md` also has the list of what is
  unverified on a Mac.
- Whoever closes one updates this record's status line, the roadmap row of
  Milestone 3, and the sentence in `ui.md`.
