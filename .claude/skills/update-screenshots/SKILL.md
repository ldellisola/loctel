---
name: update-screenshots
description: "Change, add or review the reference screenshots of loctel-ui (crates/loctel-ui/tests/snapshots). Use whenever a view's pixels change on purpose, whenever you add a screenshot test, and whenever a screenshot test fails locally or in CI (the screenshot-diffs artifact)."
---

# Update the reference screenshots

The screenshot tests render each view headlessly and compare it pixel for pixel
with a PNG in `crates/loctel-ui/tests/snapshots/`. `docs/areas/ui.md` explains
the harness (`render_png`, `render_window_png`, `Driver`, `assert_snapshot`);
this is the procedure.

## When a reference may change

Only when you changed what a view draws **on purpose**, and the new PNG goes in
the same commit as the change. A reference that changes while you were doing
something else (a refactor, a dependency or GPUI upgrade, a font or theme
token) is a regression until you can say why it moved. Never regenerate a
reference to make a red test green without looking at it, and never loosen the
comparison (no tolerance, no skipping the test).

## Steps

1. **Run the tests and read the failure.**
   `cargo test -p loctel-ui` (the `screenshot` feature is on in tests
   automatically; add `--test screens` for the main window's, or a test name). A mismatch says how many pixels differ, the first one's
   place, and writes `target/screenshot-diffs/{name}.actual.png` and
   `{name}.diff.png` (red where pixels differ). Open both with the Read tool.
   If the diff is not the change you made, fix the code, not the reference.
2. **Regenerate** only the intended ones:
   `cargo xtask screenshots --update` (it is
   `UPDATE_SNAPSHOTS=1 cargo test -p loctel-ui --features screenshot`). It
   rewrites every reference, so afterwards `git status` shows which PNGs
   actually changed; `git checkout` any you did not mean to change (a
   byte-identical render leaves the file unchanged).
3. **View every changed PNG with the Read tool before committing.** Check that
   it shows what you meant and that the text is legible: the listener lines,
   the numbers, the error text. A reference nobody looked at is worthless,
   and the plan requires this. The images are 2x (a 900 by 600 window is 1800
   by 1200 pixels, the main window's 1440 by 900 is 2880 by 1800). Check the
   colours too (the theme you forced, not the other one), and that nothing is
   clipped or overlapping; a PNG over about 1.5 MB means the capture should be
   1x (record it in a decision).
4. **Run them again** without `UPDATE_SNAPSHOTS` (twice if you like): they
   must pass, and `render_is_deterministic` guards against flaky output.
5. **Commit** the PNGs with the code, and mention the visual change in the
   message.

## Adding a screenshot test

A bare view (the status view): put it in `crates/loctel-ui/tests/screenshots.rs`,
build a fixed model (no clocks, no random data; meshgen scenarios are fine
because `generate` is deterministic), render with `render_png`, name the
snapshot after the view and the theme (`status_view_dark`).

A screen of the main window: put it in `crates/loctel-ui/tests/screens.rs`
(`interactions.rs` when it clicks, types or scrolls first, `live.rs` when time
passes; a model with no window, like `HomeModel`, is tested there with
`testing::headless_app()`). Build a `FixedBackend` over a store loaded from meshgen on a
`ManualClock`, and render with `render_window_png((1440, 900), &Theme::dark(),
|window, cx| ...)`, or keep the window with `Driver::new(...)` to click,
scroll, type, `advance` the clock and `capture()`. Every screen gets a dark and
a light reference at 1440 by 900 (`home_dark.png`, `home_light.png`). The
harness forces the theme and reduces motion; do not try to get them from the
window (the test platform is always light). Then generate the reference with
step 2 and view it (step 3).

Keep the count reasonable: each render starts a fresh headless context (about
two seconds for the first frame in a process, then about 100 ms).

## A failure in CI

`ci.yml` uploads `target/screenshot-diffs/**` as the **screenshot-diffs**
artifact when the gate fails. Download it from the run's page (or
`gh run download <run-id> -n screenshot-diffs`), then open
`{name}.actual.png` and `{name}.diff.png` with the Read tool and compare with
the reference in the repo.

- **The diff is the change you made:** regenerate locally (step 2) and
  commit. If CI still differs from your machine, see the next point.
- **Only anti-aliasing at glyph or corner edges differs** and you changed
  nothing visual: the runner's Mesa (lavapipe) version renders slightly
  differently from the one the references were made with. Do not add a
  tolerance. Record it in `docs/decisions/` and regenerate the references with
  the CI image as the source of truth (download `{name}.actual.png` from the
  artifact, view it, and commit it as the reference), because CI is the gate.
- **Everything differs, or the text looks different:** a font problem. The
  views use the bundled Inter (`crates/loctel-ui/assets/fonts`) and the
  harness sees no system fonts; check that `register_fonts` still runs before
  the first frame.
- **"No usable headless GPU adapter found":** Vulkan or EGL is missing from
  the machine; `bash script/setup-linux.sh` installs them.
- **A missing reference** (`no usable reference ...`): you added a test and
  forgot step 2.
