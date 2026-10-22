# 0021: Main window model choices (formats, colours, rows)

Date: 2026-10-22
Status: accepted

## Context

Milestone 3 Task 1 fixes the interfaces of `loctel_ui::model` (`format`,
`colors`, `trace_table`, `query_bar`) and the exact strings in its Global
Constraints, and leaves open the corners the examples do not reach: where a
number rounds, what a zero reads, which rows carry the failure text, how a
service index maps to a colour. These are choices a later agent might be tempted
to "fix", so they are written down with the reason.

## Decision

- **Durations: the unit comes from the exact value, then the number is rounded
  in that unit.** 999,999,999 ns is `1000 ms` and 999,500 ns is `1000 µs`; the
  plan states the first and the second follows the same rule. Rolling over to
  the next unit after rounding would make `1000 ms` become `1.00 s`, which is
  a second rule that every caller would have to know about, and the two forms
  are never ambiguous.
- **All rounding is half up, on integers** (`u128` nanoseconds, tenths of a
  percent). A binary float prints `1.845` as `1.84` because the nearest double
  is just under; the table of cases includes `1.845 s` = `1.85 s` so the rule
  stays one thing on every machine.
- **`rate` shows zero as `0.0`.** The plan's rule is "10 or more whole, below
  that one decimal", and a zero rate is below 10. A value that rounds up to
  10.0 reads `10`, never `10.0`. A NaN, infinite or negative value reads as
  zero, as `menu_text::format_rate` does.
- **`error_pct` reads `0% err` only for exactly no errors** (or no total, as the
  plan says). Errors that round to under 0.1% read `0.0% err`, so a map node
  with one error in 100,000 requests is not shown as clean. More errors than
  total reads `100.0% err`.
- **`clock` cuts milliseconds and does not round them**, so the clock never
  shows a time later than the instant; and it is plain integer arithmetic, so no
  timestamp can make it fail. `local_utc_offset()` reads the system zone's
  offset now (chrono with its `clock` feature) and applies it to every row, so
  a trace from before a daylight saving change shows an hour off against
  its own date. That is what a developer comparing it with the wall clock
  expects, and the tests pass 0.
- **Truncation counts characters (Unicode scalar values).** It never cuts
  inside one and does not try to keep a grapheme cluster whole: that needs a
  segmentation crate for a corner that does not matter in service names and
  span names. `truncate_middle` gives the extra character to the start.
- **`ServiceColors::index` is the first-seen number and is never reduced;** the
  palette slot is `index % 5` and the view takes it. A `TraceRow` carries the
  unreduced index so a row's colours do not depend on the palette size.
- **A row has `failed_in` only for a failed trace, and `errors_inside` only for
  a trace with errors inside.** The store names a lowest erroring span for
  both, but design 9.4 shows the second as the amber `errors inside` and keeps
  the red `Failed in` for failures, so the row decides once and the view does
  not re-derive it. `failed_in_text` appends ` +N` whenever `more > 0`, with or
  without a peer.
- **The query bar parses the text as typed.** `validate` trims only to decide
  `Empty`; the text given to the parser is the original, so an error's column
  counts the blanks the user typed. The message is the parser's own, without
  the `at column N:` prefix, because the column is a field of its own.
- **`service_filter` escapes only `"` and `\`.** Those are the only escapes
  a name needs in a TraceQL string (a raw newline or tab inside quotes is fine),
  and a test runs odd names through the real parser and a store search.

## Consequences

- Any of these is a one-line change in `model/`, and a failing row in
  `tests/model_format.rs` or `tests/model_table.rs` names the rule.
- `loctel-ui` now depends on `loctel-store`, `loctel-query`, `loctel-model` and
  `chrono` (the `clock` feature, for the zone). The core still depends on no UI
  crate, and the models still have no GPUI types.
