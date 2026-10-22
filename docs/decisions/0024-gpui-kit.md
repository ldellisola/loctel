# 0024: GPUI Kit, the theme bridge, the shell and the harness upgrade

Date: 2026-10-22
Status: accepted

## Context

Milestone 3 Task 4 adds the main window's shell and fixes its interfaces
(`UiBackend`, `Screen`, `MainWindow`, `open_main_window`, `Theme::for_appearance`,
`kit_theme::apply`, `Backdrop`/`GlassRegion`, and the harness's
`render_window_png`, `Driver` and `RecordingBackdrop`). The design (section 11,
decision 0001) chose GPUI Kit for the table, the input, the toggles, the
buttons, the split panels, the scroll areas and the tooltips, so loctel does not
write them. This records why, what it costs, and the choices the plan leaves
open that a later agent might be tempted to undo.

## Decision

- **GPUI Kit `=0.8.0`, with its default features** (`component`, `assets`), as a
  workspace dependency of `loctel-ui`, pinned exactly like GPUI. Its own
  dependency is `gpui-pre =0.3.9`, the snapshot we already pin, so there is one
  GPUI and `gpui::Window` is the same type everywhere. Upgrading Kit and GPUI
  is one change (the `upgrade-gpui` skill). The `screenshot` feature turns on
  `gpui-kit/test-support`.
- **What it costs.**
  - A bigger lockfile: about 1,900 more lines. Kit's optional `gpui-fast`
    backend is not built, but Cargo locks it. That is also why `cocoa` is
    `0.26.0` in the lockfile instead of `0.26.1`: `gpui-fast-macos` requires
    exactly `0.26.0`, and `gpui-pre-macos` (which the macOS app does build)
    shares the one copy. `cargo update -p cocoa --precise 0.26.1` is refused.
    It is a patch version of a crate only the macOS platform layer uses, and
    it is unverified on a Mac like everything else there.
  - A rebuild and disk. Measured on a 4-CPU container with
    `CARGO_INCREMENTAL=0`: a clean `cargo test --workspace` build takes about
    4 minutes and `target/` is about 11 GB after the gate (clippy, the macOS
    check and the tests); Kit and its dependencies were the biggest part of
    the increase. Agents short of disk build with `CARGO_INCREMENTAL=0` and
    remove `target/debug/incremental`, or run `cargo clean`.
  - The bundled icon set (`gpui-kit-assets`, the Lucide icons) is embedded in
    the binary.
- **Our `Theme` is a GPUI global, and Kit's theme is derived from it.**
  `kit_theme::apply(&Theme, cx)` stores the `Theme` as a global (views read it
  with `cx.global::<Theme>()` while they render, so nothing holds a stale
  copy), switches Kit to the matching mode (`Theme` gained a `dark` flag for
  that), and lays our tokens over Kit's own set for the mode: window and text,
  border, the accent as Kit's `primary`, `ring` and `link`, the surfaces of the
  table, list, input and button, the scrollbar. Tokens we do not name keep
  Kit's values. The font family is `Inter`, the bundled one, so Kit's
  components and ours draw the same text. Kit's `accent` is a hover tint (text
  at 6%), not our accent; our accent at 18% is what marks a selected row.
- **The theme follows the window.** `MainWindow::new` applies
  `Theme::for_appearance(window.appearance())`, and the window's appearance
  observer calls `MainWindow::set_appearance`. The handler takes the appearance
  as an argument so a test can call it (the test platform never reports a
  change).
- **Tests force the theme after the view is built.** The test platform always
  reports the light appearance, so a view that reads it (the shell does) has
  set the light theme by the time `build` returns. `Driver::new` runs
  `kit_theme::apply` with the test's theme afterwards, which wins; because views
  read the global at render time, that is all that is needed.
- **Reduced motion in tests.** `Driver::new` calls `cx.set_reduce_motion(true)`.
  Without it Kit's scrollbar fade reads real time and the same render gives
  different bytes. `open_main_window` leaves the system value alone.
- **Kit is initialised once per app, by `app::init`.** `gpui_kit::init` binds
  keys and resets Kit's theme, so a second call (a second window opened from the
  menu) would undo the theme. `app::init` also registers the bundled font and
  marks itself done with a global. `open_main_window` and the harness both call
  it. An app registers the icons with `Application::with_assets(KitAssets)`
  (the harness does); `loctel-app` does that when Task 9 opens the main window.
- **Regions are measured, not computed.** The shell reports the sidebar's and
  the toolbar's bounds to the `Backdrop` from `canvas` prepaint callbacks laid
  over each frame's border box, and a last zero-size canvas, prepainted after
  both, makes the one `set_regions` call and only when the pair differs from
  the last call. So the numbers are the real layout, a resize gives one call
  (not a half-updated pair), and a test pins them against the plan's constants
  (sidebar 220 wide inset 10, 16 radius).
- **The shell's status box refreshes once a second** on the foreground executor
  (`UiBackend` is `Rc<dyn UiBackend>`, not `Send`, and the snapshot is cheap).
  The 500 ms home refresh and its background reads are Task 5's `HomeModel`.
- **The toolbar title on a trace** is `Map & traces › ` and the trace ID cut in
  the middle to 17 characters (`4bf92f35…0e0e4736`).
- **Harness details.**
  - `Driver::resize` calls `Window::bounds_changed` after `Window::resize`:
    the test platform changes its bounds without telling the window.
  - `Driver::scroll(at, dy)` takes a positive `dy` to show later content, and
    sends GPUI's wheel delta `-dy`.
  - `Driver::type_text` sends each character as a key down with its character,
    so it reaches a text input through the window's input handler. `key("enter")`
    carries its newline, as GPUI's simulated keyboard does.
  - The harness's asset source is Kit's `Assets`, so components that draw icons
    do so in screenshots.
- **Screenshot size.** The shell at 1440 by 900 is 2880 by 1800 pixels and about
  150 KB as a PNG, so the 1x fallback of the plan's constraints is not needed
  yet. Views with more text are checked again as they land.
- **The existing `status_view` references did not change**: `render_png` does
  not initialise Kit, and the status view uses no Kit component.

## Consequences

- Every Kit component is available to the later tasks, and its look follows the
  two themes through one function, `kit_colors`. A component that looks wrong
  is fixed there, not with a literal in a view.
- Adding Kit costs disk and build time on every machine; see above.
- A GPUI Kit or `gpui-pre` upgrade moves pixels. The screenshots show it; the
  `upgrade-gpui` skill has the steps.
- The macOS window options (a transparent title bar with the traffic lights at
  (20, 20) in the sidebar's top strip) are unverified until the M3 check on a
  Mac.
