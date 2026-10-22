# macOS shell (`loctel-macos`)

The menu bar item and its popover (Milestone 4; Milestones 1 and 2 had a menu),
the lifecycle of the main window and of the Settings window (opening them, the
Dock while one is open), the Liquid Glass behind the main window's sidebar and
toolbar and behind the popover, the accessory-app behaviour and start at login.
Design references: §9.1 (app lifecycle), §9.2 (menu bar popover and Settings),
§10.3 (macOS checks), §10.4 (the Mac checks), §11 (technology).

Status: written and type-checked, **never run**. Everything in "Unverified
until the M1 Mac check", "Unverified until the M2 Mac check", "Unverified until
the M3 Mac check" and "Unverified until the M4 Mac check" below is a claim from
reading the crates, not something anyone has seen work.

## What is in the crate

The crate is empty off macOS, except for plain-Rust parts that are compiled
for tests on every OS (see decision 0009): `popover` (`anchor` and
`containing_screen`, the numbers of where the popover opens),
`first_launch` (the once-only step), `icon` (the two PNG constants) and, in
`glass`, `appkit_frame` (the y flip of the glass frames). Their tests run in
the Linux gate. All `objc2` and `unsafe` code in the project lives here.

| Item | What it is |
|---|---|
| `install(cx, engine, settings_path)` | `loctel-app` calls this once, inside `Application::run`. Runs the first-launch step, binds ⌘Q and ⌘O for the main window, gives the popover its engine and settings path and builds the status item (no menu; a click opens the popover). Opens no window. If the item cannot be created it prints why and quits |
| `popover::toggle(cx)` | what a click on the status item runs: closes the popover if it is open, otherwise opens it under the click. See "The popover" below |
| `popover::anchor(click_x, screen_frame, visible_frame, size)` | where the popover's top left goes, as plain numbers: tested on Linux |
| `window::open_or_focus(cx, engine, settings_path)` (also `loctel_macos::open_or_focus`) | shows the main window: brings the open one to the front, or opens it with the Dock icon on, over a `loctel_ui::app::AppBackend` (the engine, `settings_path` and `login_item::SystemLoginItem`). GPUI's reopen event calls it, and so does `show_screen`. See "The main window's lifecycle" below |
| `window::show_screen(cx, engine, settings_path, screen)` | `open_or_focus`, then `loctel_ui::app::show_screen`: the main window on a `Screen`. The popover's `Open loctel` (home) and its rules line (Discard rules) |
| `window::open_settings(cx, engine, settings_path)` | the Settings window (`loctel_ui::app::open_settings_window`) with the Dock icon on while it is open. The popover's `Settings…` |
| `glass::attach(window)` | puts Liquid Glass behind the main window's sidebar and toolbar and returns the `Backdrop` the shell reports their places to; `None` without `NSGlassEffectView`. `open_or_focus` passes it to `open_main_window`. See "Liquid Glass" below |
| `login_item::is_enabled()` | `SMAppService.mainAppService().status() == Enabled`. Asked every time; nothing is cached |
| `login_item::set_enabled(on)` | `registerAndReturnError` or `unregisterAndReturnError`; the system's `NSError` becomes an `anyhow` error |
| `login_item::SystemLoginItem` | the two above as `loctel_ui::app::LoginItem`, the hook every window's `AppBackend` is given (the main window, Settings and the popover: Milestone 4). It adds no behaviour of its own |
| `first_launch::first_launch(path, enable)` | the once-only step, against a real settings file: tested on Linux |
| `icon::template_icon()` | the two PNGs as one template `NSImage` |

`loctel-app` without `--headless` on macOS (`run_menu_bar` in `main.rs`) starts
the engine, prints the startup lines (listeners, then the rules line) like
headless mode, registers `KitAssets` (the icons of GPUI Kit's components, which
the main window draws), sets `QuitMode::Explicit`, registers the reopen handler
(`Application::on_reopen`, which runs `open_or_focus`) and, in
`Application::run`, `ActivationPolicy::Accessory`, calls `install` and opens no
window at launch. `--headless` is unchanged.

## The popover

Milestone 4 Task 9; decision [0042](../decisions/0042-popover.md) has the reasons.
It replaces the menu of Milestones 1 and 2 (the rows, the 2 s refresh and the
`Edit discard rules…` item are gone; `rules.toml` can still be edited by hand, and
the Discard rules screen shows the change). What it shows and does is
`loctel_ui::views::PopoverView`, built and screenshotted on Linux (see
[`ui.md`](ui.md), "The popover"): the title and status pill, four stats, the
endpoints with `Copy` on MCP, the rules line (a click opens the main window on
Discard rules), then `Open loctel` ⌘O, `Pause receiving` / `Resume receiving`,
`Clear all data…` (asks first), `Start at login`, `Settings…` ⌘, and `Quit loctel`
⌘Q. This section is about the part that is macOS's: where it opens, how it closes,
and what its actions do.

- **The status item has no menu.** `Tray::builder().icon(..).tooltip("loctel")
  .on_activate_with(|_, cx| popover::toggle(cx))`. `gpui-tray` 0.2 calls the
  activation callback on a primary click **only while no menu is attached**, and it
  reports no frame for the item.
- **`popover::toggle(cx)`**: if the popover is open it closes it. Otherwise it
  opens it:
  1. reads `NSEvent::mouseLocation()` and finds the `NSScreen` whose frame holds it
     (`containing_screen`, with `NSMouseInRect`'s edges: the top edge counts, since
     the mouse pushed against the menu bar reports it; the first screen, which has
     the menu bar, if none does);
  2. computes the window's origin with `anchor(click_x, screen_frame,
     visible_frame, size)`;
  3. opens a GPUI window with `WindowKind::PopUp`, no titlebar, not movable,
     resizable or minimizable, 336 by `popover_height(true)` points, on that
     screen's display (`display_id` is the screen's `NSScreenNumber`, which is
     GPUI's `DisplayId` on a Mac), with `WindowBackgroundAppearance::Transparent`
     and the glass of `glass::attach` behind the panel, or GPUI's `Blurred`
     background and one stderr line when there is none (`window::attach_glass`,
     the same as the main window's); the view is `PopoverView` in GPUI Kit's
     `Root`, with the glass backdrop given to it (`with_backdrop`);
  4. activates the app and the window.
- **`anchor`** is a pure function (`popover.rs`, tested on Linux): the popover's
  top sits 6 points under the top of `visible_frame` (the screen less the menu bar
  and Dock, so 6 points under the menu bar, notch or not); it is centred on the
  click and clamped so its whole width is inside `visible_frame` with 8 points to
  spare at each side (a Dock at a side moves that edge). It returns the distance
  from the left and the top of the **screen** it opens on, which is what GPUI's
  window bounds are relative to when a `display_id` is given. The height does not
  move it.
- **Closing.** The popover closes when its window is deactivated
  (`observe_window_activation`: a click anywhere else; only a deactivation that
  follows an activation counts, since the window starts inactive), on Esc and after
  an action that opens a window (`PopoverActions::close`), and by a second click on
  the icon (`toggle`). A call to close a popover that is already closing does
  nothing.
- **The actions** (`MacActions`, `PopoverActions`): the view calls them from inside
  its event handlers, with GPUI's `App` borrowed, so those that need the app run
  later, on the foreground executor, in the order they were asked (an `AsyncApp`
  held by the actions; decision 0041). `open_main` is `window::show_screen`;
  `open_settings` is `window::open_settings`; `copy_mcp` writes
  `engine.mcp_url()` to the clipboard; `quit` is `engine.shutdown()`, the icon out
  of the menu bar and `cx.quit()` (the same as ⌘Q in the main window); `close` is
  the close above. `toggle_pause` (`engine.set_paused`), `clear_all` (`store.clear()`)
  and `set_start_at_login` (`login_item::set_enabled`, an error to stderr) run at
  once, so that the view reads the result as soon as it has asked.
- **Shortcuts.** ⌘Q and ⌘, work while the popover has the keyboard: they are the
  view's own key bindings in its key context (`LoctelPopover`, bound by
  `loctel_ui::app::init`), so they need no menu, and the popover takes the keyboard
  when it opens. ⌘Q and ⌘O are also bound for the main window and Settings (no
  context) to `Quit` and `Open loctel`.
- **Fallback.** If the popover's window cannot be opened (no main thread, no screen,
  the display of the screen unknown, GPUI refusing the window) the main window opens
  instead (`open_or_focus`) and the reason is printed once. loctel never becomes
  unreachable.
- **Settings** opens in a window of its own (520 by 360, one at a time) with the
  activation policy `Regular` while it is open, so that it gets the keyboard and a
  Dock icon; see the next section for when the icon goes.

### The icon

The three-node glyph from the mockups (three rings joined by two lines),
black on transparent, `assets/status-icon.png` (18 by 18) and
`status-icon@2x.png` (36 by 36), both in one `NSImage` of 18 by 18 points and
marked as a template, so AppKit colours it for a light or dark menu bar and for
the open menu. `assets/status-icon.svg` is the source geometry, and
`script/gen-status-icon.py` (needs Pillow) draws the PNGs from it. Change the
glyph there, then run the script and the tests; `icon.rs` pins the sizes, that
the ink is black, and the three rings. The app's own icon (Finder, the `.dmg`,
Login Items) is `crates/loctel-app/bundle/AppIcon.svg`: the same geometry in the
accent colour on a dark rounded square, which cargo-bundle renders into the
bundle's `.icns` (see [`release.md`](release.md)). Change the glyph in both.

## Activation policy and quitting

- loctel is an **accessory app** (design §9.1) while no window is open: no Dock
  icon, no menu bar of its own, and it is not in ⌘-Tab. `main.rs` sets
  `ActivationPolicy::Accessory` once the app has launched. The release bundle
  also sets `LSUIElement` (`crates/loctel-app/bundle/info-plist-ext.xml`, which
  cargo-bundle copies into `Info.plist`, and `script/bundle-mac.sh` checks that
  it landed), which is what stops a Dock icon from flashing at launch.
- While the main window or the Settings window is open loctel is a **regular
  app**: in the Dock and in ⌘-Tab. See the next section. The popover does not
  count: it opens and closes with the app staying an accessory.
- **`QuitMode::Explicit`**: closing the last window never quits, because the
  engine keeps receiving with no window open. Only `Quit loctel` (the popover's
  row, or ⌘Q) does.

## The main window's lifecycle

`window.rs` (Milestone 3, Task 9; the Settings window and the popover's way in,
Milestone 4 Task 9). The app opens no window at launch (as in Milestone 1); the
popover's `Open loctel` is the way in, and so is the Dock icon.

- **`open_or_focus(cx, engine, settings_path)`**, which `show_screen` (the
  popover's `Open loctel` and rules line), the ⌘O action and the reopen handler
  call (`loctel-app` hands the reopen handler the settings path; the window's
  `AppBackend` reads and writes the settings there, Milestone 4):
  - If the main window exists (`MainWindowState`, a GPUI global holding its
    handle, checked against `cx.windows()`): `window.activate_window()` and
    `cx.activate(true)`. A second `Open loctel` never opens a second window.
  - Otherwise, in this order:
    1. `cx.set_activation_policy(ActivationPolicy::Regular)`: the Dock icon and
       the ⌘-Tab entry appear.
    2. `loctel_ui::app::open_main_window(Rc::new(engine), cx, attach_glass)`:
       `attach_glass` is `glass::attach`, and when that returns `None` it sets
       the `Blurred` window background and logs the fallback once (see "Liquid
       Glass").
    3. `cx.activate(true)`.
  - If opening fails the reason goes to stderr and the policy is set again from
    the windows that are open (`Accessory` if none is).
- **`show_screen(cx, engine, settings_path, screen)`** is `open_or_focus` and then
  `loctel_ui::app::show_screen(handle, screen, cx)`, which navigates the window's
  `MainWindow` from its handle alone (tested on Linux). The popover's `Open loctel`
  shows `Screen::Home` and its rules line `Screen::Rules { draft: None }`; asking
  again for the screen that is showing changes nothing.
- **`open_settings(cx, engine, settings_path)`** sets `Regular` and calls
  `loctel_ui::app::open_settings_window` (which opens the one window of Settings, or
  brings the open one forward, and activates the app). If it fails the reason goes
  to stderr and the policy is set again from the windows that are open. The main
  window's own ⌘, calls `open_settings_window` directly, and needs no policy change
  (the main window is open, so the app is already `Regular`).
- **Closing a window** (`cx.on_window_closed`, registered once, the first time a
  main or Settings window is opened): if it is the main window its handle is
  forgotten, and in every case the policy follows the windows that are left:
  `Regular` while a loctel window other than the popover is open, `Accessory`
  when none is (the Dock icon and the ⌘-Tab entry go). So closing the main window
  while Settings is open keeps the Dock icon until Settings closes too. loctel keeps
  running in the menu bar. The next `Open loctel` opens a new window.
- **Reopen**: a click on the Dock icon reaches GPUI as
  `applicationShouldHandleReopen:hasVisibleWindows:`, and GPUI calls its reopen
  handler only when there are no visible windows. `Application::on_reopen` (it
  exists only on `Application`, so `main.rs` registers it before `run`) runs
  `open_or_focus`: it opens the window when there is none, and brings the
  existing one forward when it is there but not visible (minimised or hidden).
- **Linux** has none of this: closing the main window quits (see
  [`ui.md`](ui.md), "The real window").

## Liquid Glass

`glass.rs` (Milestone 3, Task 10; decision
[0030](../decisions/0030-liquid-glass.md) has the reasons). GPUI draws the whole
window into one `NSView`, so the glass is native views *under* it, in a
transparent window.

- **`glass::attach(window: &mut Window) -> Option<Box<dyn Backdrop>>`** runs
  inside the new window, before the shell is built:
  - It returns `None`, having changed nothing, when the `NSGlassEffectView`
    class is not there (macOS before 26), when it is not on the main thread, or
    when the window's handle is not an AppKit one.
  - Otherwise it sets `WindowBackgroundAppearance::Transparent` and makes two
    regions (the sidebar first, then the toolbar). Each is a host `NSView`
    (`LoctelGlassHost`: clips to its bounds, `hitTest:` returns `nil`) holding an
    `NSGlassEffectView` (style Regular, an empty content view) that fills it.
    The hosts are inserted into GPUI's view's superview, below GPUI's view, and
    are hidden until the first regions are applied.
- **The backdrop** stores the latest regions in `set_regions` and applies them
  from the main dispatch queue, never inside GPUI's frame:
  - For each region: the host's frame (`to_appkit_frame`), the glass's frame
    (the host's bounds) and the glass's corner radius (the region's, 16), then
    the host is shown.
  - Several reports before the queue's turn apply once, with the last.
  - Regions the shell no longer reports hide their hosts.
- **`to_appkit_frame(region, view_height, flipped) -> NSRect`**: GPUI's regions
  are measured from the top left of GPUI's view. For a parent that is not
  flipped (AppKit's default) y becomes `view_height - y - height`; for a flipped
  one it stays. The frames are also offset by GPUI's view's own origin in its
  parent, which is zero when it fills it. The y flip is `appkit_frame`, plain
  Rust with tests in the Linux gate.
- **The fallback**: when `attach` returns `None`, `window.rs` sets
  `WindowBackgroundAppearance::Blurred` and prints one line to stderr (the first
  time only). The shell then has no backdrop and paints its opaque panels.
- **What the shell does for the glass** (in `loctel-ui`, tested on Linux): with a
  backdrop, the sidebar and toolbar have no fill, only a hairline and an inner
  highlight; the window colour is painted round them, not under them; and Kit's
  `Root` background is cleared, so nothing above the glass hides it. See
  [`ui.md`](ui.md), "Glass regions".

## Start at login

- `SMAppService.mainAppService`, macOS 13 and later (loctel needs 26). The
  system is the source of truth: the user may switch the item off in System
  Settings > General > Login Items, so the popover's switch and the Settings
  window's (`SystemLoginItem`) read it from the system every time, and
  `Settings::start_at_login` is not read or written by the shell.
- **First launch** (`first_launch_done` is false, which a missing
  `settings.toml` also means): turn the login item on, set `first_launch_done =
  true`, save. An error from either is printed to stderr, and the launch still
  counts as done: asking again at every launch would switch back on what the
  user turned off. Later launches never touch the login item.
- The settings written are the ones read from the file: port flags such as
  `--grpc-port 0` are not in them, so they are never written down.
- A `settings.toml` that exists but does not parse is not rewritten, and the
  login item is left alone that launch (the file's error is printed).
- Registering needs a code-signed app bundle. The release bundle is signed ad
  hoc (`script/bundle-mac.sh`, see [`release.md`](release.md)); from `cargo run`
  (no bundle) it fails with the system's message, which is expected during
  development.

The popover's rules line moves with the data like its rates (the view reads the
engine again every second). It is read from the same `EngineStatus` as the Linux
window (`menu_text::rules_line`), never from the file, so the two cannot disagree.

## Checking the macOS code from Linux

```bash
bash script/check-macos.sh --all-targets
```

Runs `cargo clippy --target aarch64-apple-darwin -p loctel-macos -p loctel-app
-- -D warnings`. The Linux gate compiles none of the macOS-only code, so run
this whenever you touch `crates/loctel-macos/` or the macOS parts of
`crates/loctel-app/` (`ci.yml` runs it too). It type-checks against the real
`objc2`, `gpui-pre` and `gpui-tray` APIs and lints the result; it does not link
or run anything. A cold run takes about 50 s. Why it needs a wrapper script:
[`decisions/0009-macos-check-from-linux.md`](../decisions/0009-macos-check-from-linux.md).
Since Milestone 5 it also hands cc-rs a stand-in C compiler for the Mac target
(`script/stub-cc.sh`, which writes empty object files): `zstd-sys`, under the
store's segment files, compiles C, and a Linux machine has no Apple SDK for it
([`decisions/0044-segment-file-format.md`](../decisions/0044-segment-file-format.md)).
A new dependency whose build script compiles C is covered the same way; one that
needs more than a C compiler from the SDK (a header search, a linker probe) is
not, and the check will say so.

The Linux gate does run the tests of the plain-Rust modules (`cargo test -p
loctel-macos`): the popover's anchoring, the first-launch rules and the icon files.

## Unverified until the M1 Mac check

Nobody has run any of this. The Mac checklist for Milestone 1 should confirm,
and a failure goes in `docs/decisions/` or an issue.

*Since Milestone 4 the menu these items describe is the popover (see "The
popover"): a build that has it has no menu to open, so its tester finds the same
things in the popover (the status pill and the four stats instead of the header,
`Copy` beside MCP instead of `Copy MCP address`, `Pause receiving`, `Start at
login` as a switch, `Quit loctel`), and the M4 list below says what else to check
there. What is about the icon, the Dock and the bundle is unchanged.*

- **The menu bar icon** appears, is crisp on a Retina display, and is dark on a
  light menu bar and light on a dark one (the template image).
- **No Dock icon** and no ⌘-Tab entry while running, and none flashing at
  launch from the bundle (which needs `LSUIElement`). Running the bare binary
  from a terminal may differ.
- **The menu actions**:
  - the header reads as in the table and is greyed out (it relies on
    `NSMenuItem.setEnabled:`, which `NSMenu.autoenablesItems` can override), and
    shows a port problem when a port is taken;
  - `Copy MCP address` puts `http://127.0.0.1:4319/mcp` on the clipboard;
  - `Pause receiving` flips to `Resume receiving`, the header says `Paused`, and
    new telemetry is dropped until resumed;
  - `Quit loctel` ends the process and frees the ports.
- **The header keeps up.** It is rebuilt every 2 s while its text changes
  (rates move all the time); check that this does not close or flicker an open
  menu, and whether an open menu updates before it is reopened. If rebuilding
  disturbs an open menu, the refresh needs another mechanism (updating the
  item's title from an `NSMenuDelegate`).
- **Start at login**:
  - a first launch turns it on, which needs the signed bundle, and macOS may show
    its "background item added" notice or need approval in System Settings (a
    registered item awaiting approval reads as off here);
  - the checkmark follows System Settings both ways;
  - choosing it toggles the login item, and a launch after turning it off does
    not turn it back on;
  - after logging out and in, loctel is running.
- **⌘Q** is absent from the menu (see above); decide whether that matters.
- **The bundle** (built by the release job, see [`release.md`](release.md)): it
  opens after **Open Anyway** (the ad-hoc signature is accepted and macOS does
  not call the app damaged), its icon looks right in Finder, in the `.dmg` window
  and in Login Items, and `LSUIElement` really keeps the Dock icon away.

## Unverified until the M2 Mac check

Milestone 2 added the rules line and the edit item. Nobody has run them either;
the Milestone 2 section of the Mac checklist (`docs/milestones/mac-checklist.md`,
items 1 to 4) should confirm.

*Since Milestone 4 the menu rows below are the popover's rules line (a click opens
the Discard rules screen) and the edit item is gone, replaced by that screen; the
list stays as the record of what the menu claimed.*

- **`Edit discard rules…`** opens `rules.toml` in a text editor: through
  `NSWorkspace` `openURL` when the Mac has an app for `.toml` (which app is the
  system's choice, and it might be an unexpected one), and through the
  `open -t` fallback when it has none. Check both if you can. In the app it is never
  greyed out (the path is always set), so a greyed item is a bug.
- **The rules header** is greyed out like the status header, shows
  `No discard rules on` with the default file, and follows an edit of the file
  within about 5 seconds (up to 2 s for the engine to reload the file, up to 2 s
  for the menu to notice): `1 discard rule on · 0% dropped`, then a non-zero
  percentage once matching traffic arrives, `1 discard rule on · 1 has an error`
  for a query that does not parse, and `Discard rules file has an error` for
  broken TOML. Nothing crashes.
- **Two disabled headers** (both rely on `NSMenuItem.setEnabled:` as the status
  header does) look right together, and an open menu is not disturbed by the
  refresh while the percentage moves (the same open question as for the rates).

## Unverified until the M3 Mac check

Milestone 3 Task 9 added the main window's lifecycle. Nobody has run it. The
Milestone 3 section of the Mac checklist (`docs/milestones/mac-checklist.md`)
should confirm:

- **Open loctel** is the first item after the two headers, opens the window in
  under half a second, and choosing it again brings the same window to the
  front instead of opening another. Its shortcut, ⌘O, only works with a loctel
  window focused: the menu cannot answer it.
- **Dock and ⌘-Tab while the window is open**: loctel's icon appears in the Dock
  and in ⌘-Tab when the window opens (`ActivationPolicy::Regular` takes effect at
  once), and the window comes to the front over the app that was active.
- **Accessory after close**: closing the window removes the Dock icon and the
  ⌘-Tab entry, and loctel keeps receiving and keeps its menu bar item.
- **Reopen from the Dock**: clicking the Dock icon while the window is open
  brings it to the front, and while the window is minimised shows it again (no
  window can be closed with the icon still there, since closing removes the
  icon). Report what happens in each case.
- **No leftovers**: opening and closing the window ten times in a row leaves one
  window each time and no stray Dock icon.

## Unverified until the M3 Mac check (Liquid Glass)

Milestone 3 Task 10 added the glass. Nobody has run it either; the Milestone 3
section of the Mac checklist (item 2) should confirm, and report in words what
it sees:

- **The glass appears** behind the sidebar and the toolbar, and looks like
  Liquid Glass (blurred and refracted, changing as the window moves over
  different backgrounds), not plain blur. The content panels are solid, and the
  window colour is solid in the gaps and the margin.
- **No seam**: AppKit rounds the glass and GPUI rounds the hole in the window
  colour round it; the 1 px hairline is meant to cover the edge. Check the
  four corners of each glass part.
- **Opening, resizing and closing the window never hangs.** The frames are
  applied from the main queue to avoid the AppKit-calls-GPUI deadlock; this is
  where that is tested for the first time. Resize the window quickly and move
  the sidebar's edge over busy backgrounds.
- **How far the glass trails a live resize.** It follows each layout one
  main-queue turn late; report whether it is visible.
- **Clicks reach GPUI** through the glass hosts (the nav entry, the toolbar's
  `Pause`, `Clear all` and range button, the traffic lights), and the hosts never take a click or the keyboard.
- **The transparent window**: its shadow, the traffic lights over the sidebar,
  and the title bar. The window colour must not show a see-through window
  anywhere (a gap that shows the desktop is a bug).
- **Light and dark**: the glass follows the system appearance, and the text on
  it stays readable in both. The hairline and the highlight are pinned only
  against a flat stand-in colour on Linux.
- **The fallback** (only if you can force it, for instance on a Mac without
  `NSGlassEffectView`): the window gets the blurred background, the panels are
  opaque, and one line about it is printed to stderr.

## Unverified until the M4 Mac check

Milestone 4 Task 2 changed what the window is built over. Nobody has run it on a
Mac. The Milestone 4 section of the Mac checklist (`docs/milestones/mac-checklist.md`) asks the tester to confirm:

- **The window still opens and works** from the popover's `Open loctel` and from
  the Dock icon after the change of `open_or_focus` (it takes the settings path now
  and builds an `AppBackend` with `login_item::SystemLoginItem`).
- **`SystemLoginItem`** asks `SMAppService` every time (`is_enabled`) and
  registers or unregisters on `set_enabled`, exactly as the two functions it wraps
  do: the "Start at login" item of the M1 list still stands.

- **The main window's shortcuts** (M4 Task 7): ⌘1, ⌘2 and ⌘3 show Map & traces,
  Logs and Discard rules, and ⌘, opens the Settings window, from anywhere in the
  main window: with the focus in an input, on the trace screen, after a click on
  nothing. They are GPUI key bindings in the window's own key context, and the
  tests send them through GPUI's dispatch, not from a keyboard. Whether macOS or
  GPUI takes ⌘, or the digit chords first (the app has no menu bar) is what a Mac
  must say. They do nothing in the Settings window.
- **The sidebar's icons** (a map, a list and a funnel, from Kit's Lucide icons)
  draw in the real window: the list and the funnel come from `KitAssets`'s own
  list on top of Kit's default bundle (decision 0040).

Milestone 4 Task 9 added the popover and the Settings wiring (decision 0042).
Nobody has run it. The Milestone 4 section of the Mac checklist should confirm,
and report in words what it sees:

- **The popover's position and glass.** A click on the icon opens a panel 336
  points wide just under the menu bar (6 points below it, notch or not), centred
  on the click, and kept 8 points inside the screen edge when the icon is near it.
  It is Liquid Glass with rounded corners of 18 points, or (without
  `NSGlassEffectView`) the blurred background with the opaque panel and one line on
  stderr. Check it from an icon near the right edge, on a second display with its
  own menu bar, with the Dock at a side, and in light and dark: the position comes
  from `NSEvent.mouseLocation` and the clicked screen's `visibleFrame`
  (`popover::anchor`), `display_id` from `NSScreenNumber`, and GPUI's own
  conversion of the origin (from the top left of that display). Whether the click's
  location is still the mouse's when the callback runs (it arrives through a
  channel from `gpui-tray`), and whether a `NonactivatingPanel` window with no
  titlebar shows no traffic lights, no shadow it should not have and no square
  corner behind the round panel, are what a Mac must say.
- **Closing on click-away.** A click anywhere outside the panel (another app, the
  desktop, another menu bar item) closes it; Esc closes it; a second click on the
  icon closes it and does **not** open it again; `Open loctel`, `Settings…` and the
  rules line close it after they act. The panel must not close while the pointer is
  merely moving or while `Clear all data…` asks. Whether the window counts as
  active after it opens (so that the click-away observer can fire; it only reacts to
  a deactivation that follows an activation) is the main thing to see. If it stays
  open and cannot be dismissed but by clicking the icon, say so.
- **⌘Q and ⌘,.** With the panel open and nothing clicked, ⌘Q quits loctel (the ports
  are freed, the icon leaves the menu bar) and ⌘, opens Settings and closes the
  panel; they come from the view's key bindings, and the panel has to have the
  keyboard on its own (a `NonactivatingPanel` that the app activated). Esc, ⌘O
  likewise.
- **The fallback.** Only if you can force it (for instance by denying the window): the
  main window opens instead, with one line on stderr, and loctel stays reachable.
- **Settings window activation.** `Settings…` opens the window 520 by 360 at the
  front and focused, with the Dock icon on, even when no other window was open (the
  policy goes `Regular` before the window opens and back to `Accessory` when the last
  loctel window closes; the popover's own window never counts). Asking again brings
  the same window forward. Closing the main window while Settings is open keeps the
  Dock icon until Settings closes. Typing in a port field works (the app is activated
  after the switch to `Regular`, as for the main window).
- **The popover's actions** are the plain ones: `Pause receiving` flips at once and
  the pill says `Paused`; `Copy` puts `http://127.0.0.1:4319/mcp` on the clipboard;
  `Clear all data…` asks, then empties the store; the `Start at login` switch agrees
  with System Settings (and with Settings) and a refusal leaves it where it was;
  `Open loctel` and the rules line open the main window on Map & traces and on
  Discard rules, and bring an open window forward on that screen.
- **`Edit discard rules…` is gone** from the macOS shell with the menu; the rules
  are edited in the Discard rules screen, or in `rules.toml` by hand.

## Tests

- `cargo test -p loctel-macos` (runs on Linux): `glass` (the y flip of the
  frames: kept for a flipped parent, counted from the bottom otherwise), `popover`
  (`anchor_centres_under_click`, `anchor_clamps_at_right_edge`,
  `anchor_clamps_at_left_edge` and `anchor_below_menu_bar` for the placement, and
  tests for a Dock at a side, a second screen, a visible frame narrower than the
  popover, the height not mattering, and which screen holds the mouse including the
  top edge), `first_launch` (once only, the login-item error, other settings survive, a
  broken file left alone) and `icon` (18 and 36 pixels, black template ink, three
  rings at the mockup's positions).
- `bash script/check-macos.sh --all-targets`: the macOS-only code type-checks
  and passes clippy. That includes the two tests of `to_appkit_frame` that
  need an `NSRect` (`to_appkit_frame_flips_y_when_not_flipped` and
  `to_appkit_frame_keeps_y_when_flipped`): they are compiled there and run
  nowhere until someone runs `cargo test -p loctel-macos` on a Mac.
- `cargo test -p loctel-ui --test screens`: the shell's side of the glass (the
  see-through parts, the hairline and the highlight, and the `shell_glass_dark`
  and `shell_glass_light` references).
- `cargo test -p loctel-ui --test interactions`: `show_screen` (what the popover's
  `open_main` relies on: a screen shown in a window that `open_main_window` opened,
  from its handle, and an error for the Settings window or a closed one). The popover
  view itself is `tests/settings_popover.rs` (see [`ui.md`](ui.md), "The popover").
- Nothing runs the AppKit parts. That is what the Mac check is for.
