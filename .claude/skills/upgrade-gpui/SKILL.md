---
name: upgrade-gpui
description: "Upgrade the pinned GPUI snapshot (gpui-pre) and GPUI Kit (gpui-kit) of loctel. Use whenever a new gpui-pre or gpui-kit version is wanted, a security or build fix needs one, or the pins have to move for any reason. One upgrade per pull request: the API moves, and the screenshot tests show what else moved."
---

# Upgrade GPUI and GPUI Kit

GPUI is pre-1.0 and ships as the weekly `gpui-pre` snapshot, published by a third
party (GPUI Kit's maintainer). loctel pins it, and GPUI Kit, **exactly**
(`=`), because the API breaks often and the screenshots are pixel-exact.
Upgrading is its own pull request, never a side effect of another change
(`docs/decisions/0001-tech-stack.md`, `0008`, `0024`; design section 13). If
`gpui-pre` stops being published, the fallback is a pinned Zed git revision;
that is a decision of its own.

## What is pinned, and where

- Root `Cargo.toml`, `[workspace.dependencies]`: `gpui-pre`,
  `gpui-pre-platform`, `gpui-pre-wgpu` (all `=0.3.9` at the time of writing) and
  `gpui-kit` (`=0.8.0`). Kit depends on one exact `gpui-pre`; **the two versions
  must agree**, so check Kit's `Cargo.toml` (`~/.cargo/registry/src/*/gpui-kit-*/`
  after a fetch, or docs.rs) for the `gpui-pre` it wants and pick the Kit
  release that matches the snapshot you want, or the other way round.
- `gpui-tray` (the status item) is generic over the GPUI it is built for; its
  `gpui-kit` feature selects `gpui-pre` types. Check it still builds.
- `rust-toolchain.toml`: the snapshot may need a newer Rust (its `rust-version`).
- `docs/areas/ui.md` and `docs/decisions/0001`, `0008`, `0024`
  mention the version; update the numbers.

## Steps

1. **Branch from `main`**, one upgrade per branch. Never mix it with a feature.
2. **Read what changed.** Look at the crates' changelogs or diff the two source
   trees in `~/.cargo/registry/src/` (`diff -r gpui-pre-A gpui-pre-B | less`);
   look at the parts loctel uses: `HeadlessAppContext` and `capture_screenshot`
   (`docs/decisions/0008`), `Window`/`Context` APIs, `canvas`, `uniform_list`,
   `observe_window_appearance`, `set_activation_policy`, `PlatformInput`, and
   in Kit `Root`, `Theme`/`ThemeColor` (`app/kit_theme.rs`), `DataTable`,
   `Input`, `Toggle`, the resizable panels.
3. **Bump the pins** in the root `Cargo.toml`, then `cargo update -p gpui-pre -p
   gpui-kit` (plus the other `gpui-pre-*` crates) and look at what else moved
   in `Cargo.lock`; a transitive pin that downgrades something loctel uses (see
   the `cocoa` note in decision 0024) goes in the commit message.
4. **Compile and fix**, smallest changes first:
   `cargo clippy --workspace --all-targets -- -D warnings`, then
   `bash script/check-macos.sh --all-targets` (the macOS half of `loctel-app`,
   `loctel-macos` and the shell's `cfg(target_os = "macos")` code are only
   type-checked here, so this is the only way to see them break).
5. **Run the tests**, `cargo test --workspace`. Screenshot tests that fail show
   what moved: open `target/screenshot-diffs/{name}.actual.png` and
   `{name}.diff.png` with the Read tool. A text-rendering or shader change moves
   **every** reference by a few pixels; a layout change moves some. Decide for
   each whether the new look is acceptable; if something is wrong, fix the code
   or pin back, do not regenerate over it.
6. **Regenerate** (`cargo xtask screenshots --update`) only when every diff is
   explained, **view every changed PNG** (the `update-screenshots` skill), and
   say in the commit message why they moved ("gpui-pre 0.3.10: glyph
   positioning changed by a subpixel").
7. **Re-check the tricks that depend on GPUI internals**:
   - `Driver::resize` needs `Window::bounds_changed` after `Window::resize`;
     `render_window_png` needs `cx.set_reduce_motion(true)` and the forced theme
     (`docs/decisions/0024`).
   - The Linux headless text system (`CosmicTextSystem::new_without_system_fonts`)
     and the bundled font.
   - `script/check-macos.sh` and its placeholder shader files
     (`docs/decisions/0009`): a new `gpui-pre-apple` may change what its build
     script writes.
8. **Run the app**: `run-and-screenshot` skill. A real window under Xvfb can break
   when the headless harness does not (windowing, X11/Wayland features).
9. **Docs and the decision log in the same PR**: the versions in `ui.md`, and a
   new `docs/decisions/NNNN-gpui-upgrade-X.md` when anything non-obvious
   changed (an API replaced, a workaround added or removed).
10. **Ship it** the usual way (AGENTS.md "How a change ships"). Mac-only
    behaviour that the upgrade could change stays unverified until the next
    milestone check on a Mac: say so in the PR.
