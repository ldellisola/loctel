# 0019: The rules line and the Edit discard rules item

Date: 2026-10-21
Status: accepted

## Context

The Milestone 2 plan (Task 8) gives the exact strings of the rules line, in
priority order, and the new macOS menu. It leaves open what "a rule is on" and
"a rule has an error" count when a rule is switched off, how the line reads for
more than one error, what happens to the percentage when it is not a number, and
how the existing menu tests change when the menu gains a second greyed-out line.
None of these changes the plan's strings or interfaces; they are the choices a
later agent might undo without knowing why.

## Decision

- **`rules_on` counts enabled rules, valid or not, and `rules_with_errors`
  counts only enabled rules whose query does not parse.** The engine parses
  every rule, so a draft saved with `enabled = false` still carries an error in
  `RuleStatus` (decision 0017). The line is about what you expect to be working:
  a rule you switched off is not expected to work, so it is neither "on" nor "has
  an error" here (MCP `get_status` still lists the draft's error). It also keeps
  the numbers coherent: `rules_with_errors <= rules_on`, so the line can never
  read "0 discard rules on · 1 has an error". A bad draft that is switched off
  therefore reads `No discard rules on`, which is true.
- **The plural of the error count is `N have errors`.** The plan gives only the
  singular, `1 discard rule on · 1 has an error`. For two or more the line reads
  `3 discard rules on · 2 have errors`, and the rule count says `rule` for one and
  `rules` for any other number, as in the plan's `3 discard rules on`.
- **The percentage is clamped and never shows a non-number.** The line rounds
  `dropped_fraction * 100` to a whole number. A value outside 0 to 1 is cut to
  it, and NaN or infinity reads `0%`, the same rule `format_rate` has for a value
  that is not a rate, so a menu never shows `NaN%` or `250%`. A fraction under half
  a percent reads `0%` even when something was dropped, which is what "rounded to
  an integer" means.
- **A file error is a flag in `StatusModel`, not text.** The plan's
  `rules_file_error: bool` is all the line needs: its words are fixed. The error
  text stays in `EngineStatus` and MCP, where it has room.
- **The rules line is a second `Row::Header`, not a new row kind.** The plan calls
  it "a disabled rules header", which is what `Row::Header` already is (a greyed
  out `NoAction` item). The existing menu tests changed only where the menu
  changed: `rows_are_the_designed_menu_in_order` has the two headers and the new
  item in its expected order, and `only_the_header_is_not_an_action` became
  `only_the_two_headers_are_not_actions`, which asserts the same things for two
  headers (both at the top, the last row still Quit). The plan's test
  `rows_include_rules_header_and_edit_item` pins the order separately, label by
  label.
- **`Edit discard rules…` is enabled when `engine.rules_path()` is `Some`.**
  `MenuState` carries a `has_rules_file` flag for it, as it carries
  `has_mcp_address` for Copy. The menu is refreshed when it changes, like the
  rest of the state.
- **Opening the file: `NSWorkspace` first, `open -t` second.** `openURL` with the
  file URL is what Finder does and uses the system's choice of app for `.toml`;
  it returns false when there is none (and the code also takes this route when the
  path is not valid UTF-8, because the URL is built from text). `open -t <path>`
  asks for the default text editor outright. Both are in `status_item.rs` and are
  unverified until the M2 Mac check (`docs/areas/macos.md`). The cost is two small
  features of the objc2 crates (`NSWorkspace` in `objc2-app-kit`, `NSURL` in
  `objc2-foundation`) in the workspace `Cargo.toml`.
- **The Linux status view shows the rules line as a dot and a line of text under
  the four numbers,** not a card: red (dot and text) for a file or rule error,
  green when rules are on, grey when none is. The words carry the meaning, the
  colour only backs them up.

## Consequences

- A change to the line's strings is a change to `menu_text::rules_line`, its
  test file and `docs/areas/ui.md` together; the menu and the window cannot
  differ.
- If the rules screen (M4) ever wants disabled drafts flagged in the menu, it is
  a one-line change in `StatusModel::from_status` and the test
  `from_status_counts_the_rules_that_are_on_and_their_errors`.
