# 0038: The Discard rules screen, where the plan leaves a choice

Date: 2026-10-22
Status: accepted

## Context

Milestone 4 Task 5 builds the Discard rules screen (`RulesModel`, `RulesView`,
`RuleEditorView`). The plan fixes the interfaces, the exact strings and the
behaviour of the banners, and the code keeps all of them. The choices below are
the ones it does not settle, or settles in a way that two readings of it would
undo, and that a later agent could undo by mistake. Each names the test that
pins it.

## Decision

- **The preview runs `loctel_rules::preview` over a clone of the store on the
  background executor, not `UiBackend::preview_rule`.** The preview is a pass
  over five minutes of stored data, and the M3 constraint that every store read
  runs on GPUI's background executor, never blocking a frame, still holds. A
  `UiBackend` is an `Rc<dyn UiBackend>` on the foreground thread and cannot be
  sent to the executor; its `store()` (a cheap clone) can. So the model computes
  `since` from `store.now_unix_nano() - PREVIEW_WINDOW` and calls the rules
  crate itself, which is exactly what both `AppBackend::preview_rule` (the
  engine's clock is the store's clock) and `FixedBackend::preview_rule` do. The
  trait method stays, for callers that are not a frame. (`the_preview_waits_for_the_typing_to_stop`,
  `rules_editor_dark`)
- **The preview waits 300 ms after a keystroke, and not for anything else.**
  Opening a rule, and a click on the language or on the scope, run at once (they
  are one decision, not typing). A query that does not parse says `Fix the query
  to see what it would drop.` at once, with no read and no wait. A blank query
  has no preview (the box is empty and keeps its height), and no error line: a
  rule that has just been started is not an error yet. A result that is there
  stays until the next one replaces it, so typing does not flicker the box; the
  one that was an `Invalid` goes as soon as the text parses. (`an_edit_during_the_wait_restarts_it`,
  `switching_the_language_changes_the_parser_the_scope_and_the_preview`,
  `invalid_rule_saved_shows_error_and_no_preview`)
- **The wording of the preview where the plan gives only the plural.** `1 span in
  1 trace. That trace had no errors.` / `That trace had errors.` for one trace,
  `Would have dropped nothing.` for none, `1 was an error.`, `It was an error.`,
  `It was not an error.` and `None were errors.` for lines. The sentences of the
  plan (`Would have dropped 1,204 spans in 602 traces. None of those traces had
  errors.`, `3 of those traces had errors.`, `Would have dropped 96 lines. 2 were
  errors.`) are unchanged. The logs a trace rule takes with its traces are not
  part of its sentence. (`tests/model_rules.rs`)
- **`Save` writes the rules as the editor saw them, not as they are now.** The
  editor keeps a snapshot of the rules (IDs and specs) from when it opened.
  `Save` and `Delete` build the new list from that snapshot, the draft in the
  place of its rule (at the end for a new one), and hand it to `set_rules`. When
  the rules did not change behind the editor's back the snapshot is the list, and
  this is just "replace the rule where it stands". When they did, it is what makes
  "Saving after `Keep my draft` overwrites the file" true and deterministic: the
  rule being edited may have been changed or removed in the file, so there may be
  nowhere to put the draft in the file's list, and the plan's word is
  "overwrites". The banner's two buttons are the user's decision, and until it is
  made `Save` and `Delete` do nothing (plan Review Focus 2: "Saving overwrites the
  file only after confirming"). `Discard my draft` closes the editor and leaves
  the file as it is. (`external_change_during_edit_warns`,
  `discarding_the_draft_closes_the_editor_and_keeps_the_file`)
- **The window's own writes are not external changes.** Every `set_rule_enabled`
  and `set_rules` moves `rules_generation`, so a switch on a card while an editor
  is open would flag its own number. The model re-takes the generation and the
  snapshot after its own write (unless a banner is already up: then what the user
  saw stays what they saw), and the draft of the open rule takes the new state of
  its switch, or saving it would turn the rule back on. A write that fails leaves
  the editor open with `Could not save the rules: <reason>` above the buttons.
  (`a_change_made_here_is_not_an_external_one`,
  `a_rule_turned_off_while_open_stays_off_when_saved`)
- **`Delete` asks inside the editor.** The plan says "`Delete` (with a
  confirmation)" and fixes no words. The buttons give way to `Delete this rule?`
  with `Delete rule` (the danger button) and `Keep rule`; nothing is removed until
  the first. A rule that is not saved yet (`New rule`, a suggestion) has no
  `Delete`: `Cancel` is that. (`delete_asks_then_removes`,
  `a_rule_that_is_not_saved_yet_has_nothing_to_delete`)
- **The model and the view live for the window, and the model reads only while
  its screen shows**, as the Logs screen's do, so Task 7's "the editor draft is
  kept across screens" needs nothing more. Coming back reads at once.
  `navigate(Screen::Rules { draft: Some(d) })` opens the editor on `d` every time,
  replacing a draft that was open: it is what the user just asked for, and the
  Logs screen's button says so. `draft: None` shows the screen as it was left.
  (`a_hidden_rules_screen_catches_up_when_it_shows`,
  `a_new_suggestion_replaces_the_draft_in_the_editor`)
- **The rules list is read on the foreground.** `UiBackend::rules` and `status`
  are in-memory reads of the engine's rules (no store pass), like the status
  refresh of the shell, and they are not `Send`; only the preview touches the
  store. (`the_list_refreshes_every_second`)
- **The title is on the screen twice.** The toolbar's strip says `Discard rules`
  as on every screen (`toolbar_title`), and the plan's header of the screen has its
  own title, subtitle, share dropped and `New rule`. Both are kept: the toolbar
  is the window's, the header the screen's.
- **The editor is a `RuleEditorView` of its own, made with the screen.** Its two
  inputs own their text, so the view copies the draft into them when a different
  editor session opens (and focuses `Name` for a rule that has none), and their
  changes into the model as they are typed; `set_value` does not emit a change,
  so there is no echo. It is drawn only while an editor is open, so the screen
  reports its parts as absent then (`RulesView::bounds_of` asks it only while one
  is open).
- **`Applies to` is built from divs, not Kit's `TabBar`.** The plan says "a
  segmented control". Two cells in a rounded track give a test the middle of each
  side (`RulesPart::LanguageTraces`, `LanguageLogs`), which a `TabBar`'s tabs do
  not, and look like the Logs screen's level toggles (the accent tint for the
  chosen side). The scope is Kit's `Radio`.
- **The editor panel's "glass" is the shell's look without a backdrop.** The
  panel fill, the translucent `glass_hairline` round it and the `glass_highlight`
  ring just inside: what a glass part of the shell shows on Linux. The panel sits
  over opaque content, so there is nothing for a backdrop to show.
- **`FixedBackend::rule_set()`** is new, so a screenshot test can send data
  through the rules (`filter_spans`, `filter_logs`) and have the cards count it
  the way the engine's sink does, instead of the test inventing counters.
- **Two references beyond the plan's five.** `rules_editor_invalid_dark` and
  `rules_editor_changed_dark` show the states the Review Focus is about (the
  error line and the preview's request to fix, the banner over a kept draft),
  which no plan reference shows.

## Consequences

- Task 7 puts the sidebar entry and the ⌘3 shortcut on top of what is here and
  regenerates the `rules_*` references with the sidebar's new look.
- A rule saved from the window rewrites `rules.toml` and drops hand-written
  comments (decision 0018 has the header it keeps), as the plan accepts.
- The editor's snapshot means a `Save` after `Keep my draft` loses what the file
  got meanwhile; the banner says the file changed, and `Keep my draft` is the
  user's word that their draft matters more.
