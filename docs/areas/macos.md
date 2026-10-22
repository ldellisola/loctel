# macOS shell (`loctel-macos`)

The menu bar item, the main window's lifecycle (opening it, the Dock while it
is open), the Liquid Glass behind its sidebar and toolbar, the accessory-app
behaviour and start at login. Design
references: §9.1 (app lifecycle), §9.2 (menu bar popover, which is Milestone 4;
Milestone 1 has the plain menu below), §10.3 (macOS checks), §10.4 (the Mac
checks), §11 (technology).

Status: written and type-checked, **never run**. Everything in "Unverified
until the M1 Mac check", "Unverified until the M2 Mac check" and "Unverified
until the M3 Mac check" below is a claim from reading the crates, not something
anyone has seen work.

## What is in the crate

The crate is empty off macOS, except for plain-Rust parts that are compiled
for tests on every OS (see decision 0009): `menu` (the menu's rows as values),
`first_launch` (the once-only step), `icon` (the two PNG constants) and, in
`glass`, `appkit_frame` (the y flip of the glass frames). Their tests run in
the Linux gate. All `objc2` and `unsafe` code in the project lives here.

| Item | What it is |
|---|---|
| `install(cx, engine, settings_path)` | `loctel-app` calls this once, inside `Application::run`. Runs the first-launch step, registers the actions, builds the status item and starts the 2 s refresh. Opens no window. If the item cannot be created it prints why and quits |
| `window::open_or_focus(cx, engine, settings_path)` (also `loctel_macos::open_or_focus`) | shows the main window: brings the open one to the front, or opens it with the Dock icon on, over a `loctel_ui::app::AppBackend` (the engine, `settings_path` and `login_item::SystemLoginItem`). The menu's `Open loctel` and GPUI's reopen event call it. See "The main window's lifecycle" below |
| `glass::attach(window)` | puts Liquid Glass behind the main window's sidebar and toolbar and returns the `Backdrop` the shell reports their places to; `None` without `NSGlassEffectView`. `open_or_focus` passes it to `open_main_window`. See "Liquid Glass" below |
| `login_item::is_enabled()` | `SMAppService.mainAppService().status() == Enabled`. Asked every time; nothing is cached |
| `login_item::set_enabled(on)` | `registerAndReturnError` or `unregisterAndReturnError`; the system's `NSError` becomes an `anyhow` error |
| `login_item::SystemLoginItem` | the two above as `loctel_ui::app::LoginItem`, the hook `open_or_focus` gives the window's `AppBackend` (Milestone 4). It adds no behaviour of its own |
| `menu::rows(&MenuState)` | the rows below, as values: tested on Linux |
| `first_launch::first_launch(path, enable)` | the once-only step, against a real settings file: tested on Linux |
| `icon::template_icon()` | the two PNGs as one template `NSImage` |

`loctel-app` without `--headless` on macOS (`run_menu_bar` in `main.rs`) starts
the engine, prints the startup lines (listeners, then the rules line) like
headless mode, registers `KitAssets` (the icons of GPUI Kit's components, which
the main window draws), sets `QuitMode::Explicit`, registers the reopen handler
(`Application::on_reopen`, which runs `open_or_focus`) and, in
`Application::run`, `ActivationPolicy::Accessory`, calls `install` and opens no
window at launch. `--headless` is unchanged.

## The menu

Built with `gpui-tray`: each entry is a GPUI action, and the crate rebuilds the
menu after every click. The menu, in order:

| # | Row | Does |
|---|---|---|
| 1 | header: `Receiving · 412 spans/s · 1.2k logs/s`, `Paused` or `Port problem: otlp-http (address in use)` | nothing; greyed out. The text is `loctel_ui::menu_text::status_line`, the same function the Linux window uses, so the words cannot drift |
| 2 | rules header: `No discard rules on`, `1 discard rule on · 31% dropped`, `1 discard rule on · 1 has an error` or `Discard rules file has an error` | nothing; greyed out. The text is `loctel_ui::menu_text::rules_line` (the exact strings and their priority are in [`ui.md`](ui.md)), the same function the Linux window uses |
| 3 | `Open loctel` | `window::open_or_focus`: opens the main window and shows loctel in the Dock, or brings the open window to the front. Never greyed out |
| 4 | separator | |
| 5 | `Copy MCP address` | puts `engine.mcp_url()` on the clipboard. Greyed out while the MCP server is not listening |
| 6 | `Pause receiving`, or `Resume receiving` while paused | `engine.set_paused(!paused)` |
| 7 | `Edit discard rules…` | opens `engine.rules_path()` (`rules.toml`) in the default text editor: `NSWorkspace` `openURL` with the file URL, and if no app handles `.toml`, `open -t <path>`. A failure is printed to stderr. Greyed out when `rules_path()` is `None` (the rules live in memory only; the app always passes a path). The file exists because the engine writes the defaults (no rules, a comment header) at start; saving it in the editor is picked up within about 2 seconds, with no restart |
| 8 | separator | |
| 9 | `Start at login` | checkmark from `login_item::is_enabled()`; choosing it flips the login item |
| 10 | separator | |
| 11 | `Quit loctel` | `engine.shutdown()`, removes the icon, quits |

Every 2 seconds the state behind the menu (header text, rules line, paused,
login item, MCP address, whether there is a rules file) is read again; only if it differs from what the menu was built from is
the menu rebuilt. A rebuild also happens right after any click, so a toggled
row is correct at once.

**⌘Q and ⌘O are not in the menu.** `gpui-tray` 0.2.0 cannot set a key equivalent
on its items. The shortcuts `cmd-q` and `cmd-o` are bound to the same Quit and
`Open loctel` actions, which work only while a loctel window has the keyboard
(the main window, Milestone 3; there ⌘O just brings the window to the front).
So "⌘O while the menu is open" does nothing: the menu shows no shortcut and
cannot answer one. Decision 0009 says what changing that would take.

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
- While the main window is open loctel is a **regular app**: in the Dock and in
  ⌘-Tab. See the next section.
- **`QuitMode::Explicit`**: closing the last window never quits, because the
  engine keeps receiving with no window open. Only `Quit loctel` does.

## The main window's lifecycle

`window.rs` (Milestone 3, Task 9). The app opens no window at launch (as in
Milestone 1); the menu's `Open loctel` is the way in.

- **`open_or_focus(cx, engine, settings_path)`**, which the `Open loctel` action and the
  reopen handler call (`loctel-app` hands both the settings path; the window's
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
  - If opening fails the reason goes to stderr and the policy goes back to
    `Accessory`.
- **Closing the window** (`cx.on_window_closed`, registered once, the first time
  a window is opened; it ignores every window but the main one): the handle is
  forgotten and `cx.set_activation_policy(ActivationPolicy::Accessory)` takes
  the Dock icon away. loctel keeps running in the menu bar. The next
  `Open loctel` opens a new window.
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
  Settings > General > Login Items, so the checkmark is read from the system
  every time, and `Settings::start_at_login` is not read or written by the
  shell.
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

The rules line is rebuilt with the rest: its percentage moves while data
arrives, so an open menu may be replaced about every 2 s like the rates (see
"The header keeps up" below). The rules header is read from the same
`EngineStatus` as the Linux window, never from the file, so the two cannot
disagree.

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

The Linux gate does run the tests of the plain-Rust modules (`cargo test -p
loctel-macos`): the menu's rows, the first-launch rules and the icon files.

## Unverified until the M1 Mac check

Nobody has run any of this. The Mac checklist for Milestone 1 should confirm,
and a failure goes in `docs/decisions/` or an issue:

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
items 1 to 4) should confirm:

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
Mac. The Milestone 4 section of the Mac checklist should confirm, once it exists:

- **The window still opens and works** from `Open loctel` and from the Dock icon
  after the change of `open_or_focus` (it takes the settings path now and builds
  an `AppBackend` with `login_item::SystemLoginItem`). Nothing a user sees
  changed yet; the settings and the popover that use the backend come later in
  the milestone.
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

## Tests

- `cargo test -p loctel-macos` (runs on Linux): `glass` (the y flip of the
  frames: kept for a flipped parent, counted from the bottom otherwise), `menu` (the designed order with
  `Open loctel` first after the headers, the rules header and the Edit item,
  the Pause/Resume label, the checkmark, the greyed Copy and the greyed Edit,
  the header text),
  `first_launch` (once only, the login-item error, other settings survive, a
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
- Nothing runs the AppKit parts. That is what the Mac check is for.
