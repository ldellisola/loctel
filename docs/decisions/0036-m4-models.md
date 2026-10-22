# 0036: The Milestone 4 models, where the plan leaves a choice

Date: 2026-10-22
Status: accepted

## Context

Milestone 4 Task 3 adds the pure models behind the Logs screen, the Discard
rules screen, the Settings window and the menu bar popover (`model::logs`,
`histogram`, `rules`, `settings`, `popover`). The plan fixes their names,
signatures and exact strings, and the models keep all of them. A few cases it
does not cover, or covers in two ways that cannot both hold, needed an answer
that a later agent could undo by mistake. They are listed here, with the test
that pins each one.

## Decision

- **A log row's `message` is the whole body on one line.** The plan says "the
  first line only, newlines become ` ⏎ `". A first line has no newline to
  replace, and its own test list says "a multi-line body shown on one line", so
  the model flattens the body: `\n`, `\r\n` and a lone `\r` each become one
  ` ⏎ `, and breaks at the very end are dropped. The row stays one line because
  the view cuts it to the column; the details grid has the body whole.
  (`a_multi_line_body_shows_on_one_line`, `a_windows_line_break_is_one_mark_...`)
- **A rule card that is off says `Off` even if its query does not parse.** The
  plan lists "when off" and "when invalid" without an order. The menu's rules
  line already counts an error only for a rule that is on, so a card agrees with
  it, and `RuleRow::error` is set exactly when `counts` shows the error (so a view
  that paints red when `error` is set never paints `Off` red). Switch the rule on
  and its error shows. (`a_switched_off_rule_with_a_bad_query_is_off_not_an_error`)
- **Rule counts are singular for one**, as every other count in the window is
  (`1 span dropped in 1 trace`, `1 line dropped`), and a trace-scope rule that has
  dropped no whole trace says only `N spans dropped`, never `in 0 traces`.
- **`dropped_line` never calls a small share nothing.** A share that rounds to 0%
  but is more than 0 reads `<1% of incoming data dropped`; `Nothing dropped yet`
  is for 0 and for what is not a share (NaN, infinity, negatives), as
  `menu_text::rules_line` reads them. The two lines can differ for the same
  value (`0% dropped` in the menu, `<1%` here): the menu line is fixed-width
  words, this one has room.
- **"Discard lines like this…" works from the body's first line that has text.**
  The name quotes its first 40 characters (then `…`), the filter its first 60 cut
  back to the last whitespace when the cut falls inside a word. A stack trace
  after the message would otherwise be in a filter no other line matches, and a
  line break does not belong in a rule's name. A body with no text at all gives
  `{service_name="<service>"}` alone rather than `|= ""`, which would match every
  line of the service. `"` and `\` are escaped in the filter and the service name,
  not in the name. Every log of the failed checkout is matched by its own
  suggestion (`every_log_of_the_failed_checkout_is_matched_by_its_own_suggestion`).
- **`Draft::to_spec` trims nothing.** The rules crate keeps a rule's identity (its
  counters, its ID) by name, language, query and scope, so opening a rule and
  saving it unchanged must hand back the same spec. Only `name_ok` (and so
  `can_save`) looks at the name with blanks trimmed. `validate` sends the query to
  the parser as it is, as the rules engine does, so an empty query is an error and
  its column counts from 1 like the query bars'.
- **The Settings form reads the login switch from the system**, not from the
  settings file (`Form::from(settings, login)`: the system is the source of truth
  for a login item, see `UiBackend::login_item`). A port is digits only, so `+80`,
  `4,318` and non-ASCII digits are refused; duplicates are compared as numbers and
  reported on every field that shares a port; a field that is not a port shares
  nothing.
- **The popover's endpoints** are `OTLP gRPC` and `OTLP HTTP` with `host:port` and
  `MCP` with the URL an agent connects to (what its `Copy` button copies). A failed
  listener shows the address it was asked for and `ok` false, with no URL for MCP.
  The reason is in the pill's `Port problem` and the Settings window, not in the
  row.
- **The histogram's labels are `format::clock` times** (`08:05:00.000`), the
  window's only clock format, cut to a millisecond; the bars' error part is on
  the tallest bar's scale, so the stack never exceeds its bar.

## Consequences

- The Logs, rules, Settings and popover views (Tasks 4, 5, 6 and 8) lay out these
  strings and add none of their own; a view that needs another one adds it to the
  model with a test first.
- Undoing one of these choices means changing the test named above on purpose.
