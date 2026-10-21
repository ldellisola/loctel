# macOS shell (`loctel-macos`)

The menu bar item, the accessory-app behaviour and start at login. Design
references: §9.1 (app lifecycle), §9.2 (menu bar popover, which is Milestone 4;
Milestone 1 has the plain menu below), §10.3 (macOS checks), §10.4 (the Mac
checks), §11 (technology).

Status: written and type-checked, **never run**. Everything in "Unverified
until the M1 Mac check" below is a claim from reading the crates, not
something anyone has seen work.

## What is in the crate

The crate is empty off macOS, except for three plain-Rust modules that are
compiled for tests on every OS (see decision 0009): `menu` (the menu's rows as
values), `first_launch` (the once-only step) and `icon` (the two PNG
constants). Their tests run in the Linux gate. All `objc2` and `unsafe` code in
the project lives here.

| Item | What it is |
|---|---|
| `install(cx, engine, settings_path)` | `loctel-app` calls this once, inside `Application::run`. Runs the first-launch step, registers the actions, builds the status item and starts the 2 s refresh. Opens no window. If the item cannot be created it prints why and quits |
| `login_item::is_enabled()` | `SMAppService.mainAppService().status() == Enabled`. Asked every time; nothing is cached |
| `login_item::set_enabled(on)` | `registerAndReturnError` or `unregisterAndReturnError`; the system's `NSError` becomes an `anyhow` error |
| `menu::rows(&MenuState)` | the rows below, as values: tested on Linux |
| `first_launch::first_launch(path, enable)` | the once-only step, against a real settings file: tested on Linux |
| `icon::template_icon()` | the two PNGs as one template `NSImage` |

`loctel-app` without `--headless` on macOS (`run_menu_bar` in `main.rs`) starts
the engine, prints the listener lines like headless mode, sets
`QuitMode::Explicit` and, in `Application::run`, `ActivationPolicy::Accessory`,
calls `install` and opens no window. `--headless` is unchanged.

## The menu

Built with `gpui-tray`: each entry is a GPUI action, and the crate rebuilds the
menu after every click. The menu, in order:

| # | Row | Does |
|---|---|---|
| 1 | header: `Receiving · 412 spans/s · 1.2k logs/s`, `Paused` or `Port problem: otlp-http (address in use)` | nothing; greyed out. The text is `loctel_ui::menu_text::status_line`, the same function the Linux window uses, so the words cannot drift |
| 2 | `Copy MCP address` | puts `engine.mcp_url()` on the clipboard. Greyed out while the MCP server is not listening |
| 3 | `Pause receiving`, or `Resume receiving` while paused | `engine.set_paused(!paused)` |
| 4 | separator | |
| 5 | `Start at login` | checkmark from `login_item::is_enabled()`; choosing it flips the login item |
| 6 | separator | |
| 7 | `Quit loctel` | `engine.shutdown()`, removes the icon, quits |

Every 2 seconds the state behind the menu (header text, paused, login item, MCP
address) is read again; only if it differs from what the menu was built from is
the menu rebuilt. A rebuild also happens right after any click, so a toggled
row is correct at once.

**⌘Q is not in the menu.** `gpui-tray` 0.2.0 cannot set a key equivalent on its
items. The shortcut `cmd-q` is bound to the same Quit action, which works only
while a loctel window has the keyboard (the main window, Milestone 3). Decision
0009 says what changing that would take.

### The icon

The three-node glyph from the mockups (three rings joined by two lines),
black on transparent, `assets/status-icon.png` (18 by 18) and
`status-icon@2x.png` (36 by 36), both in one `NSImage` of 18 by 18 points and
marked as a template, so AppKit colours it for a light or dark menu bar and for
the open menu. `assets/status-icon.svg` is the source geometry, and
`script/gen-status-icon.py` (needs Pillow) draws the PNGs from it. Change the
glyph there, then run the script and the tests; `icon.rs` pins the sizes, that
the ink is black, and the three rings. The release's `.icns` (Milestone 1,
release task) comes from the same glyph.

## Activation policy and quitting

- loctel is an **accessory app** (design §9.1): no Dock icon, no menu bar of its
  own, and it is not in ⌘-Tab. `main.rs` sets `ActivationPolicy::Accessory`
  once the app has launched. The release bundle also sets `LSUIElement` (release
  task), which is what stops a Dock icon from flashing at launch.
- Showing in the Dock and ⌘-Tab while a window is open is the main window's job
  (Milestone 3: switch to `Regular` on open, back to `Accessory` on close).
- **`QuitMode::Explicit`**: closing the last window never quits, because the
  engine keeps receiving with no window open. Only `Quit loctel` does.

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
- Registering needs a code-signed app bundle. From `cargo run` (no bundle) it
  fails with the system's message; that is expected during development.

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

## Tests

- `cargo test -p loctel-macos` (runs on Linux): `menu` (the designed order, the
  Pause/Resume label, the checkmark, the greyed Copy, the header text),
  `first_launch` (once only, the login-item error, other settings survive, a
  broken file left alone) and `icon` (18 and 36 pixels, black template ink, three
  rings at the mockup's positions).
- `bash script/check-macos.sh --all-targets`: the macOS-only code type-checks
  and passes clippy.
- Nothing runs the AppKit parts. That is what the Mac check is for.
