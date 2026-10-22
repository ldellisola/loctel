# 0029: Window lifecycle choices

Date: 2026-10-22
Status: accepted

## Context

Milestone 3 Task 9 makes the main window reachable: on Linux `loctel` opens it
instead of the status window; on macOS the menu's new `Open loctel` opens it,
loctel shows in the Dock and Cmd-Tab while it is open, and goes back to a menu
bar only app when it closes. The plan fixes the behaviour and a few interfaces;
this records what it left open, and the three places where the GPUI API or the
existing code differs from what the plan assumed.

## Decision

- **The window's handle is a GPUI global, and the close observer is registered
  once.** `window::open_or_focus(cx, engine)` is called from two places that
  share nothing (the menu's action and the Dock's reopen handler), so what they
  must agree on (is there a main window, which one) lives in a
  `MainWindowState` global holding an `AnyWindowHandle` (not
  `WindowHandle<Root>`: `loctel-macos` does not depend on `gpui-kit`). "The
  window exists" is the global *and* `cx.windows()` containing the handle, so a
  stale handle opens a new window instead of failing. The `on_window_closed`
  observer is registered the first time a window is opened (guarded by a flag
  in the same global) and compares the closed `WindowId` with the main window's;
  it is detached rather than dropped from inside its own callback, and it
  ignores every other window.
- **Reopen is registered on `Application`, in `main.rs`.** The plan says
  `cx.on_reopen`; in `gpui-pre` 0.3.9 `on_reopen` exists only on `Application`
  (before `run`), not on `App`. `run_menu_bar` calls
  `app.on_reopen(move |cx| loctel_macos::open_or_focus(cx, engine.clone()))`.
  GPUI calls it only when AppKit says there are no visible windows.
- **The backdrop closure is `|_| None` in Task 9.** The plan writes
  `|w| glass::attach(w)`, but `glass.rs` is Task 10's file. Task 10 changes this
  one closure (and adds the `Blurred` fallback). Done: `window::attach_glass`,
  decision 0030.
- **⌘O is bound, but cannot be in the menu.** `gpui-tray` 0.2.0 builds its
  `NSMenuItem`s with an empty key equivalent (decision 0009 says the same of
  ⌘Q), so the menu cannot show or answer ⌘O. `cmd-o` is bound to the
  `OpenWindow` action next to `cmd-q`, which works while a loctel window has the
  keyboard (it brings the window to the front). The plan's wording for the
  Mac checklist, "⌘O while the menu is open", therefore describes something that
  cannot work; Task 11's checklist item must test `Open loctel` by clicking it.
- **On Linux, SIGINT and SIGTERM quit the window cleanly.** The plan's smoke
  test wants "a clean exit on SIGTERM", but the status window the app had
  until now died of the signal (status 143, the engine never shut down), which
  `run-and-screenshot` even documented. `run_window` now installs the same
  `ShutdownSignals` as headless mode before printing anything, waits for them on
  a thread, and passes the news to GPUI's thread through a
  `tokio::sync::oneshot` that a `cx.spawn` future awaits (only GPUI's thread may
  call `cx.quit()`; a waker from another thread schedules the task through
  GPUI's dispatcher). `Application::run` then returns, `engine.shutdown()` runs
  and the exit status is 0. The test asserts `exit=0`.
- **The smoke test signals `loctel`, not `xvfb-run`.** A SIGTERM sent to
  `xvfb-run` (a shell script) would not reach the app. The test runs a small
  bash script under `xvfb-run` that starts `loctel` in the background, sleeps
  3 s, checks it is alive, sends it SIGTERM, waits up to 10 s (SIGKILL and a
  `hung` verdict after that) and prints the exit status; the test reads that,
  `loctel`'s stdout (the four startup lines) and its stderr (no `panicked`, no
  `cannot open the window`). Without `xvfb-run` it prints `skipped: no
  xvfb-run` and passes.
- **`KitAssets` is registered at `Application` level in both apps.** GPUI Kit's
  components draw their icons from its assets, and only `Application::with_assets`
  can set them. `loctel-ui` therefore became a macOS dependency of `loctel-app`
  too (it was Linux-only); the window itself is still opened by
  `loctel-macos`.
- **`xtask run-shot` uses a 1440 by 900 screen.** The window's default size is
  1440 by 900, larger than the old 1280 by 800 screen, so the window would have
  been cut off. With a screen of exactly that size the capture is the window and
  nothing else (checked: `target/run-shot.png` is 1440 by 900, no border). The
  script waits 2 s for the first frame, reads the OTLP HTTP address from the
  listener lines (the ports are 0), sends `meshgen --scenario checkout-failure
  --transport http-json`, waits 2 s and captures. `xtask` builds `loctel-meshgen`
  too.
- **`StatusView` and `open_status_window` stay**, as the plan says, as a
  standalone view with its screenshot tests; nothing in the app calls the
  function any more.

## Consequences

- Everything Mac-only here is unverified until the M3 check: `Open loctel`, the
  Dock and Cmd-Tab while the window is open, Accessory after close, a reopen from
  the Dock (`docs/areas/macos.md`, "Unverified until the M3 Mac check").
- A later change that needs to open the window from somewhere else calls
  `open_or_focus`; it must not call `open_main_window` itself, or two windows
  can exist and the Dock icon will go with the first one that closes.
