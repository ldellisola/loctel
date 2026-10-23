# 0052: The disk cap in Settings, the disk tile, and the dark hairline

Date: 2026-10-23
Status: accepted

Milestone 5, Task 8. The plan fixes the interfaces (`settings::Form::disk_cap_gb`, valid
from 1 to 100 with `Enter a size from 1 to 100 GB`; a `Storage` section in the Settings
window with the input and `Using 1.3 GB of 4 GB` under it; the popover's fourth tile
`1.3 GB` over `on disk, cap 4 GB`, or `Disk full` in `error_text` with a `spill_error`;
`format::bytes`; `contrast::ratio` and `tests/contrast.rs`). This file records what it left
open, and the three places where the plan and the code that exists meet.

## Decisions

### A GB is 2^30 bytes, and `bytes` drops a `.0`

The plan's strings are `on disk, cap 4 GB` and `Using 1.3 GB of 4 GB`, and the default cap
is `4 * 1024 * 1024 * 1024` (4 GiB). With decimal units that cap would read `4.3 GB`, so
the units are binary everywhere: `format::bytes` steps by 1024 (`B`, `KB`, `MB`, `GB`, `TB`),
the field takes whole binary GB (`BYTES_PER_GB`), and 1.3 billion bytes reads `1.2 GB`.

"One decimal under 10" would also make the default cap `4.0 GB`. The plan's `4 GB` wins:
a size that rounds to a whole at one decimal has no decimal (`4 GB`, `1 GB`, and 1.04 GB
reads `1 GB`), and one that rounds to 10 reads `10 GB`. As for `duration`, the unit is
chosen from the exact value and the number rounded in it, so it never rolls over (a hair
under 1 MB is `1024 KB`). `TB` is the largest unit; past it the number has more digits.

### The cap is not a `Field`, and an unedited cap is saved as it was

`model::settings::Field` is the three ports and the listeners they move (`Field::ALL`,
`Field::listener`), and `Form::validate` returns the `Ports` or problems per `Field`; the
tests of Milestone 4 pin both. The cap does not fit either, so it has its own check:
`Form::validate_disk_cap() -> Result<u64, &'static str>` (bytes, or the message) and
`Form::is_valid()` for both. `validate()` does not look at the cap, so a bad cap and a bad
port are each reported by their own field and neither hides the other, and `Apply` needs
`is_valid`.

The field holds whole GB and `settings.toml` holds bytes, so a file edited by hand can hold
a cap the window cannot write (1.5 GB). The form shows the next whole GB up
(`disk_cap_bytes.div_ceil(BYTES_PER_GB)`, never less than the file's), and **Apply writes the
cap only when the field's number differs from the one the form was loaded with**; a change
of port does not round the file's 1.5 GB to 2. `Using ... of 1.5 GB` says the cap in force
as it is. A cap the window cannot show and the user wants to change is changed by typing a
different number first (`a_cap_that_was_not_edited_is_saved_as_it_was`).

The line under the input says the cap **in force** (`EngineStatus::disk_cap_bytes`, read once
a second like the endpoint lines), not the one being typed: while an edit is not applied it
is still true of what the engine is doing, as the endpoint rows are.

### The window is 520 by 480 now

A `Storage` section with a message under it is about 90 px more form, and the Milestone 4
window was 360 px tall with 17 px to spare in its worst case (`settings_invalid_dark`: every
field with a message). The plan gives the section and not a new size. Keeping 360 would
leave a form that scrolls in its default state, and the fields-scroll rule was only the
safety net of a form that otherwise fits. So the window grew to the height that fits the
same worst case with the same 17 px to spare: `SETTINGS_SIZE` is (520, 480). That touched
three assertions of Milestone 4, all of them the size itself and nothing about what is in
the window: the unit test in `settings_window.rs`, `the_settings_window_opens_once_at_...`
and the `SIZE` of `settings_popover.rs`. The new test
`everything_fits_the_window_with_every_message_showing` states what the old comment said:
with a message under every field (four, the cap included) every part is above the buttons.
The window's chrome on a Mac is unverified as before; the Mac checklist (Milestone 4 item
6) says 480.

### The popover's fourth stat has a label of its own

`Popover::stats` was `[(String, &'static str); 4]`. The disk label has the cap in it
(`on disk, cap 4 GB`), so it is `[(String, String); 4]` (the first three labels are still
`STAT_LABELS`, now three of them, and `popover::disk_label(cap)` makes the fourth). The tile's
red `Disk full` is `Popover::disk_full`, which the view reads to paint the value in
`error_text`. The label stays `on disk, cap 4 GB` while the disk is full: the cap is still
the cap, and the size is the one thing the tile cannot say then. The pill is untouched: it
says `Port problem` for a listener that failed and nothing else, so a full disk and a port
problem show in two places and neither is mistaken for the other
(`a_port_problem_and_a_full_disk_are_each_shown_where_they_belong`). Any `spill_error`
shows `Disk full` (the plan's word), including a run directory that could not be made.

`popover_disk_full_dark.png` is a new reference, so that the red value and the label under
it are looked at once; the other seven popover references changed by the tile.

### The dark hairline was too faint, and nothing else was

`tests/contrast.rs` holds both looks to the plan's table (text 4.5:1 on `bg` and `panel`,
`error_text`, `warn` and `accent` 4.5:1 on `panel`, `border` 1.3:1 against `panel`) with the
WCAG formula in `loctel_ui::contrast::ratio`. Only one pair failed: the dark `border`,
`#2a2b32` on `#1a1b20`, is 1.22:1. The plan says to fix the failing tokens without changing
their hue family, so it is `#303139` (1.33:1: the same blue-grey, 6 to 7 steps lighter in each
channel). Every light pair already passed, so the light look is unchanged.

A hairline is on nearly every dark screen, so every dark reference with a panel, a table
line or a card changed (41 references changed in this task, with the Settings and popover
ones, which changed for their own reasons as well). Each was looked at, and the ones that
changed only for the hairline were compared with the ones they replaced pixel by pixel:
the pixels that changed are the old hairline colour that is now the new one, and the
blended pixels at its rounded corners and edges (no more than 8 of 255 in any channel);
no glyph, fill or position moved.
The theme's own unit test `dark_tokens_are_the_mockup_values` does not pin the hairline,
and now says so in `theme.rs`. A `border` change to a token that is not on the plan's list
(`bg`, `panel` and the like) would still be a mockup change and needs a decision of its own.

## Consequences

- A later change to a token has a test that names the pair it broke and its ratio.
- `format::bytes` is the one way a size is written; the Mac checklist's Milestone 5 section
  (Task 9) says `Using ... of ...` and the tile as they are here.
- A cap of 100 GB is the most the window takes, as the plan says; `settings.toml` can hold
  more and the engine will keep it (the field then shows it as an error until it is changed).
