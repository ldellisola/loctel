# 0039: The Settings window, where the plan leaves a choice

Date: 2026-10-22
Status: accepted

## Context

Milestone 4 Task 6 builds the Settings window (`SettingsView`,
`open_settings_window`). The plan fixes the interfaces, the layout, the exact
strings and what Apply does ("calls `apply_settings`, then shows the resulting
listener states"), and the code keeps all of them. The choices below are the
ones it does not settle, or settles in a way that two readings of it would
undo, and that a later agent could undo by mistake. Each names the test that
pins it.

## Decision

- **The login switch takes effect on Apply, not when it is clicked.** The plan's
  `Apply` is "enabled when the form differs and is valid", and the form
  (`settings::Form`) has the switch in it (`start_at_login`), so a click on the
  switch makes the form differ and lights `Apply`. The popover's switch (Task 8)
  is different on purpose: it has no Apply and acts at once. Settings then calls
  `LoginItem::set_enabled` after the ports are saved, only if the switch changed,
  and afterwards shows what `LoginItem::is_enabled` says, whatever was asked (the
  system may have refused). The switch is **not** written to `settings.toml`
  (`Settings::start_at_login` stays as it is on disk): the system is the source
  of truth and nothing mirrors it (`login_item.rs`, decision 0036).
  (`the_login_switch_takes_effect_on_apply`,
  `the_login_switch_is_not_called_when_it_did_not_change`,
  `a_login_item_the_system_refuses_is_reported_and_the_switch_follows_the_system`)
- **A switch nobody has touched follows the system.** The window asks the login
  item again once a second while the switch is as it was loaded, so a change made
  in System Settings shows. A switch the user has clicked is theirs until Apply:
  a refresh does not take it back. (`the_switch_follows_the_system_while_it_is_not_being_edited`)
- **A port that cannot be bound is saved, and Apply counts as done.** The plan
  says the listener shows `Failed`, the setting is still saved and "the window
  says `otlp-http: address in use`". The window says it twice, for two
  readers: the endpoint's own row (`failed: address in use`, in the row of the
  field, red, always while the listener is failed) and a line beside the buttons
  (`otlp-http: address in use`, one per failed listener) that reports what
  *this Apply* did and goes as soon as the form is edited again. The form then
  equals what was saved, so `Apply` goes off; the input keeps the new port. The
  other listeners are untouched (plan Review Focus 4).
  (`apply_rebinds_and_shows_failure`,
  `the_line_under_the_endpoints_goes_when_the_form_changes_again`)
- **A settings file that cannot be written is the one real failure of Apply.**
  `apply_settings` returning an error means nothing moved (decision 0035), so the
  window shows `Could not save the settings: <reason>`, keeps the form and the
  switch exactly as they were with `Apply` still on, and does not touch the login
  item (it is applied together with the ports or not at all).
  (`a_settings_file_that_cannot_be_written_is_reported_and_the_form_is_kept`)
- **The row's state is the engine's, not the form's.** `listening` and `failed:
  <reason>` come from `UiBackend::status().listeners`, read when the window
  opens, after an Apply and once a second. While a port is edited and not applied,
  the row still says what is true of the port the listener is bound to. A refresh
  never rewrites what is typed. The reason is `menu_text`'s (`address in use`
  for any OS wording of "in use", otherwise the error cut to 40 characters), made
  `pub(crate)` so the menu's status line and the window cannot say it differently.
  (`the_statuses_follow_the_engine_while_the_window_is_open`,
  `an_unrecognised_failure_is_shown_as_the_engine_gave_it_cut_to_forty_characters`)
- **After an Apply the inputs show the ports as saved.** `" 4500 "` and `04500`
  are accepted by the form (it parses numbers), and the input is rewritten to
  `4500`, so what is on the screen is what the file says.
  (`apply_saves_the_ports_and_shows_the_listeners_it_got`)
- **The form opens on the saved settings, not on what the engine listens on.**
  `UiBackend::settings()` reads `settings.toml` (decision 0035), so a command-line
  port flag (`--http-port 0`) shows in the row (`listening`), not in the input.
- **The view applies the theme, not `open_settings_window`.** `SettingsView::new`
  takes the theme from the window's appearance and follows it, as `MainWindow::new`
  does, because the first frame is drawn while the window opens and in the test
  harness the forced theme only arrives after it.
- **Opening it twice focuses the open one.** The handle lives in a GPUI global
  (`SettingsWindowState`) and is trusted only while `cx.windows()` still contains
  it, so a closed window is forgotten and the next call opens a new one. The
  window options are the ones the plan names (520 by 360, centred, titled
  `Settings`, `is_resizable: false`) and nothing more: no minimum size and the
  minimise button stays, as in a Mac's own settings windows.
  (`the_settings_window_opens_once_at_520_by_360_and_is_focused_when_asked_again`,
  the unit test of `options` in `settings_window.rs`)
- **A compact layout, so the worst case fits.** The window cannot be resized, so
  the form is laid out to hold all three validation messages and the `Startup`
  panel inside the 360 px (rows 32 px, the title 18 px). It still scrolls, as a
  safety, but `settings_invalid_dark` shows the worst case without a scroll.
- **Three references beyond the plan's two.** `settings_dark` and `settings_light`
  are the plan's. `settings_edited_dark` (Apply lit), `settings_failed_dark` (the
  failure in the row and beside the buttons) and `settings_invalid_dark` (the
  messages) show the states the pair cannot.
- **Test support.** `FixedBackend` gains `fail_bind` (a listener `apply_settings`
  reports `Failed`, as the engine does for a port in use; the plan asks for "a
  `FixedBackend` whose `apply_settings` reports `otlp-http` failed") and
  `fail_save` (`apply_settings` is an error). `Driver::is_open` and a `draw` that
  skips a closed window let a test click `Close`.

## Consequences

Task 7 (⌘,) and Task 9 (the popover's `Settings…`) only call
`open_settings_window`. The popover's `Start at login` switch and Settings agree
because both ask the system and neither mirrors it; the Mac check (Task 10, item
7) is the only thing that can show that they do on a real login item.
