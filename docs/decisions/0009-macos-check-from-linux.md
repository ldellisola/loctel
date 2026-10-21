# 0009: Checking the macOS shell from Linux, and the shell's choices

Date: 2026-10-21
Status: accepted

The Milestone 1 plan (Task 12) names this file `0002-macos-check-from-linux.md`.
Number 0002 was already taken by the log-storage decision when the task ran, so
this is the next free number.

## Context

Agents work on Linux, and nobody can run the macOS code until the Milestone 1
check on a real Mac (design §10.4). Design §10.3 asks us to keep macOS CI to
releases, to guard the macOS code on Linux "by type-checking that crate for
`aarch64-apple-darwin`, if its build scripts allow it", and only otherwise to
run a macOS job on PRs that touch macOS paths. Task 12 step 1 says to try the
cross-check, record exactly what worked, and add it to `ci.yml` if it works.

## What was tried, exactly

`cargo check --target aarch64-apple-darwin -p loctel-macos -p loctel-app`
(Rust 1.98.1, the target installed by `rust-toolchain.toml`) type-checks every
crate in the graph (`objc2*`, `gpui-tray`, `gpui-pre-macos`, `loctel-ui`, ...)
except one, and fails there:

```
error: couldn't read `.../out/shaders.metallib`: No such file or directory
  --> gpui-pre-apple-0.3.9/src/metal_renderer.rs:32:33
```

The cause is in `gpui-pre-apple`'s `build.rs`. Its body is
`#[cfg(any(target_os = "macos", target_os = "ios"))] apple_build::run();`, and
in a build script `cfg(target_os)` is the **build machine's** OS, not the
target's. On a Linux host the script does nothing, while `metal_renderer.rs`
`include_bytes!`s the file it was supposed to write. So the plan's example
suspect, a missing `xcrun`, is not the problem: the script never gets as far as
calling it.

The plan's suggested fix, GPUI's `runtime_shaders` feature
(`--features gpui-pre-platform/runtime_shaders`), does not help for the same
reason. It switches the source to `include_str!` of
`stitched_shaders.metal`, which the same skipped function would have written:

```
error: couldn't read `.../out/stitched_shaders.metal`: No such file or directory
```

## Decision

**The cross-check works, with placeholder files, and it runs in the Linux job
of `ci.yml`. There is no `macos-check` job.**

- `script/check-macos.sh` sets itself as `RUSTC_WRAPPER`. Whenever cargo is
  about to compile the one crate `gpui-pre-apple`, the wrapper first creates
  empty `shaders.metallib` and `stitched_shaders.metal` files in that crate's
  `OUT_DIR`. Then it runs rustc as usual. `cargo
  check` and `clippy` produce metadata only and never link or run anything, so
  empty files are enough. No other crate needed help.
- The script runs `cargo clippy --target aarch64-apple-darwin -p loctel-macos
  -p loctel-app -- -D warnings`, not plain `cargo check`: clippy type-checks
  too, and the macOS code is otherwise outside the `clippy --workspace` gate,
  so this is the only lint it gets. `ci.yml` passes `--all-targets` so the tests
  compile as well. Cold it takes about 50 s and 300 MB of `target/`.
- If `gpui-pre` is upgraded, re-read its `build.rs`: the day it stops skipping
  the shaders on a Linux host (or changes the file names), the wrapper is dead
  weight or wrong, and the check will say which (it fails on a missing file).

**What this check can and cannot prove.** It proves the code type-checks against
the real `objc2`, `gpui-pre` and `gpui-tray` APIs and passes clippy. It does not
link, run, sign or draw anything, and does not compile the Metal shaders, so
every runtime behaviour of the shell stays on the "Unverified until the M1 Mac
check" list in [`areas/macos.md`](../areas/macos.md).

## The shell's own choices

- **`gpui-tray` features are `gpui-kit` and `menu-state`.** The plan says
  "the `gpui-pre` backend feature"; `gpui-tray` 0.2.0 calls it `gpui-kit`
  (it builds against `gpui-pre` 0.3.x, so its `Tray` and `MenuItem` types are
  the same ones `loctel-ui` uses; `--all-features` is not supported there).
  `menu-state` is not in the plan, but without it the crate ignores
  `MenuItem::checked` and `disabled`, and the menu needs both (the checkmark on
  Start at login, the greyed header).
- **The parts that hold decisions are plain Rust, compiled for tests on every
  OS.** The plan makes every dependency macOS-only and the crate empty
  elsewhere, which would leave the menu's order, the Pause/Resume label, the
  first-launch rules and the icon files tested by nothing the gate runs. So
  `menu.rs` (the rows as values), `first_launch.rs` (the once-only step, against
  a real settings file) and `icon.rs` (the two PNG constants) are compiled when
  `cfg(any(target_os = "macos", test))`; their tests run in `cargo test
  --workspace` on Linux. For that, `loctel-engine`, `anyhow`, `image` and
  `tempfile` are `[dev-dependencies]` of the crate; the plan's rule that
  dependencies are macOS-only still holds for everything shipped. In a normal
  Linux build the crate is empty.
- **The icon is one `NSImage` with both PNGs.** 18 by 18 and 36 by 36, black on
  transparent, the second added as a Retina representation of the same 18 by 18
  points, marked as a template. (`gpui-tray`'s own `Icon::from_rgba` takes one
  bitmap, so this uses its `Icon::from_ns_image`.) The glyph is the mockup's
  (`assets/status-icon.svg`); `script/gen-status-icon.py` draws the PNGs from
  the same geometry, and a test pins their size, colour and the three rings.
- **⌘Q is not in the menu.** `gpui-tray` 0.2.0 builds its `NSMenuItem`s with an
  empty key equivalent and gives no way to set one (the menu object is private),
  so the status item's menu can neither show nor answer ⌘Q. `Quit loctel` is
  there and works; `cmd-q` is also bound to the same action, which takes effect
  while one of loctel's own windows has the keyboard (Milestone 3). If the Mac
  check wants the shortcut in the menu, that is a change to `gpui-tray` or a
  hand-written `NSStatusItem` in this crate.
- **The system owns "start at login".** `login_item::is_enabled` asks
  `SMAppService` every time (the user can switch the item off in System
  Settings), and the menu toggle changes it there. `settings.toml` is not
  mirrored: `Settings::start_at_login` stays as the engine left it and is not
  read. The first launch turns the item on once (`first_launch_done`); a later
  launch never does, so a user's "off" sticks.
- **A broken `settings.toml` is not touched.** If the file exists but does not
  parse, the first-launch step does nothing (no login item, no rewrite) and
  says so on stderr. Otherwise this launch could never be recorded as done and
  every launch would switch the login item back on.
- **Copy MCP address is greyed out while the MCP server is not listening**
  (`engine.mcp_url()` is `None`), instead of doing nothing when chosen.
- **If the status item cannot be created, loctel quits** after printing why. An
  accessory app with no window and no icon would run unseen, and there is
  nothing to click to stop it.
- **The menu is rebuilt only when it changed.** Every 2 s the state behind the
  menu (header text, paused, login item, MCP address) is compared with what the
  menu was last built from, and `gpui-tray` is asked to refresh only if it
  differs. A refresh replaces the whole `NSMenu`, so skipping the no-change
  case keeps an open menu from being swapped for an identical one. (Whether a
  rebuild disturbs an open menu is unverified.)

## Facts about the crates that the above rests on

- `gpui-tray` 0.2.0: features `gpui` (default), `gpui-kit` (= `gpui-pre`),
  `gpui-ce`, `menu-state`; exactly one backend. `Tray::builder().icon().menu(..)
  .build(cx)`; menu entries are `MenuItem`s whose actions are dispatched as
  GPUI actions, after which it rebuilds the menu; `Tray::refresh_menu` and
  `Tray::close`; `Tray` is not `Send`, and dropping the last clone removes the
  item.
- `gpui-pre` 0.3.9: `Application::with_quit_mode(QuitMode::Explicit)`,
  `App::set_activation_policy(ActivationPolicy::Accessory)`, and
  `App::dispatch_action`, which goes to the global handlers when no window is
  active (the case for a menu bar app).
- `objc2-service-management` 0.3.2: `SMAppService::mainAppService()`,
  `status()`, `registerAndReturnError()` and `unregisterAndReturnError()`, all
  `unsafe` in the bindings; it needs its default features.
