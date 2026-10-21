# 0008: UI crate and screenshot harness choices

Date: 2026-10-21
Status: accepted

## Context

The Milestone 1 plan (Task 11) fixes the interfaces of `loctel-ui` (`Theme`,
`StatusModel`, `StatusView`, `menu_text`, `testing::render_png` and
`assert_snapshot`), asks for pixel-exact screenshot tests and a Linux window,
and says the workspace test must run them. It leaves open how the headless
renderer gets its fonts, how the feature reaches the workspace test, how the
real window starts, and a few smaller things. These are the choices a later
agent might be tempted to undo, and the facts about `gpui-pre` 0.3.9 that they
rest on.

## Decision

- **The screenshot harness sees only a bundled font.** On Linux `render_png`
  builds `gpui_wgpu::CosmicTextSystem::new_without_system_fonts("Inter")` and
  `register_fonts` adds `Inter-Regular.otf` and `Inter-SemiBold.otf`, which
  live in `crates/loctel-ui/assets/fonts/` with their SIL OFL licence (the
  files are unmodified from Ubuntu's `fonts-inter` 4.0). The plan's
  `HeadlessAppContext::with_platform(...)` takes any text system; the obvious
  one (`gpui_platform::current_platform(true).text_system()`) loads whatever
  fonts the machine has, so a reference made here would not match a CI runner
  that lacks Inter, and every agent's container could differ from the last.
  Bundling costs 1.2 MB and makes the references machine-independent. The
  window uses the same bundled font, so what the test shows is what the window
  draws. On macOS the platform text system is used with the bundled fonts
  added; the design's system font (SF) arrives with the Milestone 3 window.
- **The `screenshot` feature is turned on by a dev-dependency on the crate
  itself**, one of the two ways the plan allows. `loctel-ui` lists
  `loctel-ui = { path = ".", features = ["screenshot"] }` under
  `[dev-dependencies]`, so every test, clippy and doc build has it and
  `ci.yml` needs no change: `cargo test --workspace` and
  `cargo clippy --workspace --all-targets` cover the screenshots. The shipped
  binary is built without it (Cargo resolves dev-dependency features only
  when building tests), so `test-support` code is not in the app. The feature
  is `gpui-pre/test-support` (it gates `HeadlessAppContext`),
  `gpui-pre-platform/test-support` (it gates `current_headless_renderer`) and
  `gpui-pre-wgpu/test-support`.
- **`gpui-pre-wgpu` is an optional dependency, enabled by `screenshot`.** The
  plan lists it as a dependency; only the harness uses it (for the text
  system), and an optional one keeps wgpu out of the macOS build of the shipped
  app. The tests always have it.
- **`gpui-pre-platform` gets both `x11` and `wayland`.** Its defaults enable
  neither, and without one the Linux window cannot open. Both compile with the
  packages `script/setup-linux.sh` installs. `loctel-ui` and `loctel-app` take
  the crates from `[workspace.dependencies]`.
- **Screenshots are 2x.** The headless platform draws at scale 2, so the
  900 by 600 window of the plan is a 1800 by 1200 image. `render_png`'s `size`
  is the logical window size. Retina-size references are also easier to read
  when you view them.
- **The window prints the listener lines.** `loctel` without `--headless` on
  Linux prints the same JSON lines as headless mode before it opens the
  window, so a script that started it with ports 0 can read the ports and send
  meshgen data (the `run-and-screenshot` skill does). The plan does not say it
  should; it costs nothing and removes a guess.
- **No display means headless.** With neither `DISPLAY` nor `WAYLAND_DISPLAY`
  set, GPUI's Linux platform quietly falls back to a headless client whose
  windows draw nothing, and the app would sit there looking dead. `loctel`
  checks first, says so on stderr and runs the headless path (which also
  shuts down cleanly on SIGINT and SIGTERM).
- **The window is dark, always.** The design wants the system appearance
  followed (§9.3); that belongs to the main window of Milestone 3. `light()`
  exists, is contrast-tested, and has a screenshot test, so it is ready.
- **Extra tests beyond the plan's list**, all additions: the theme tokens and
  contrast, `StatusModel::from_status`, the 40-character cut and the other
  `status_line` rules, `format_rate` edge cases, the snapshot comparison itself
  (a changed pixel, a size change, a missing reference, update mode), the
  `xtask` argument parsing, and a light-theme screenshot.
- **`assert_snapshot` compares decoded pixels**, not PNG bytes, so an image
  encoder upgrade cannot fail a test. It writes the diff images to
  `target/screenshot-diffs/` of the workspace (honouring `CARGO_TARGET_DIR`),
  where `ci.yml` already looks.

## Consequences

- A GPUI snapshot upgrade is its own change, and its first run tells you what
  moved: expect the screenshots to change and read the diffs.
- The references were made with Mesa lavapipe on Ubuntu 24.04 in the agent
  container. If the GitHub runner's Mesa anti-aliases differently, the
  `screenshot-diffs` artifact shows it; the `update-screenshots` skill says
  what to do (no tolerance; the CI render becomes the reference). Not yet
  observed: CI has not run this change.
- Adding a font or changing the size of the bundled ones changes every
  reference.

## Facts about `gpui-pre` 0.3.9 that the above rests on

- The library is named `gpui` (`gpui-pre` is only the package), the platform
  crate `gpui_platform` and the wgpu crate `gpui_wgpu`.
- `HeadlessAppContext` exists only with `gpui/test-support`.
  `gpui_platform::current_headless_renderer` exists only with
  `gpui-pre-platform/test-support` and, on Linux, creates a wgpu renderer on
  the CPU adapter (lavapipe); with no Vulkan or EGL it fails with "No usable
  headless GPU adapter found".
- `capture_screenshot` returns an `image::RgbaImage` of the `image` 0.25 line,
  at scale 2 on the headless platform.
- `gpui_platform::application()` is the windowed `Application`;
  `Application::run` blocks until quit. Under Xvfb the window opened and drew
  on the first try, with no X event "nudge" (the crates.io 0.2.2 bug of
  design §10.5 is gone), but `cargo xtask run-shot` keeps a one-shot nudge
  as a safety net.
