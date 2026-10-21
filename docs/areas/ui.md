# UI (`loctel-ui`) and the screenshot workflow

The GPUI layer, kept thin on purpose: views draw what the engine reports and
hold no logic of their own. Design references: §9.2 (screens), §9.3 (look and
feel), §10.3 (screenshot tests) and §10.5 (gotchas from the spike).

Status: Milestones 1 and 2 have one view, the **status view**, and the Linux
window that shows it. Milestone 2 added the discard rules line to it and to the
macOS menu; the rules screen is Milestone 4. The main window (Map & traces, Trace view) is Milestone 3, the
remaining screens and the popover Milestone 4. The macOS menu bar item is in
`loctel-macos`; it only borrows the strings from `menu_text`.

GPUI comes as the weekly `gpui-pre` snapshot, `gpui-pre`, `gpui-pre-platform`
and `gpui-pre-wgpu`, all pinned to exactly `=0.3.9` (zed@93d7456). Upgrading is
its own change: the API moves, and the screenshots are what shows you what
moved.

## What is in the crate

| Item | What it is |
|---|---|
| `StatusModel`, `ListenerLine` | plain values for the view and the menu. `StatusModel::from_status(&EngineStatus)` builds one. A failed listener keeps the address it was asked for, and its error. The rules fields: `rules_on` (enabled rules, whether or not their queries parse), `rules_with_errors` (the enabled ones that do not parse; a switched-off draft with a bad query is not counted, so it never exceeds `rules_on`), `rules_file_error` (a flag: `rules.toml` could not be used; the text stays in `EngineStatus`) and `dropped_fraction` (0 to 1, since launch) |
| `StatusView` | the GPUI view: a headline pill, four numbers (spans/s, logs/s, traces held, logs held), the discard rules line under them (a dot and the words of `rules_line`; red dot and text when the file or a rule has an error, green when rules are on, grey when none is) and the endpoint list. `StatusView::new(model, theme)`, `set_model(model, cx)` (redraws only when the model changed) and `refresh_from(engine, cx)` (once a second, until the view is dropped) |
| `open_status_window(engine, cx)` | opens the 900 by 600 window on an `App`: dark theme, refreshed every second from `EngineHandle::status()`. This is what `loctel` (no `--headless`) shows on Linux |
| `menu_text::status_line(&StatusModel)` | the one-line status: `Receiving · 412 spans/s · 1.2k logs/s`, `Paused` or `Port problem: otlp-http (address in use)`. A failed listener wins over paused, paused over receiving. The reason is `address in use` when the OS error contains "in use", otherwise the raw error cut to 40 characters. The first failed listener is the one named |
| `menu_text::rules_line(&StatusModel)` | the discard rules in one line, the first that applies: `Discard rules file has an error`; `1 discard rule on · 1 has an error` (for more than one, `3 discard rules on · 2 have errors`); `No discard rules on`; `1 discard rule on · 31% dropped` or `3 discard rules on · 0% dropped`. The percentage is the share the rules dropped of what arrived since launch, rounded to a whole number; a value that is not a number reads `0%` and one outside 0 to 1 is cut to it. A file error wins over a rule error, a rule error over "no rules on" |
| `menu_text::format_rate(f64)` | `412` is `412`, `1234` is `1.2k`, `0.4` is `0`. One decimal under ten thousand, none above, never `.0` (`1k`, `10k`, `1.3M`); NaN, infinity and negatives read `0` |
| `Theme` | the colours below. `Theme::dark()` and `Theme::light()` |
| `register_fonts(cx)`, `FONT_FAMILY` | the bundled typeface (below) |
| `testing` (feature `screenshot`) | `render_png` and `assert_snapshot` |

The strings in `menu_text` are the contract: the macOS menu header and the
status view's pill both use `status_line`, the macOS menu's rules header and the
view's rules row both use `rules_line`, and `crates/loctel-ui/tests/menu_text.rs`
pins them.

## Theme tokens

The dark tokens come from the accepted mockups; a test pins them. The light
ones are the builder's reading of them (the same hues, darkened to hold their
contrast on a pale ground; a test checks that text stays at 4.5:1 or better on
both grounds in both themes). Views read `Theme` fields and never write a hex
literal.

| Token | Dark | Light | Use |
|---|---|---|---|
| `bg` | `#15161a` | `#f4f4f6` | window background |
| `panel` | `#1a1b20` | `#ffffff` | cards and lists |
| `border` | `#2a2b32` | `#dcdce2` | hairlines |
| `text` | `#ececf1` | `#1b1c20` | primary text |
| `text_secondary` | `#a3a3ad` | `#4d4d57` | addresses, state |
| `text_tertiary` | `#8b8b95` | `#62626c` | small labels |
| `accent` | `#4c9dff` | `#0a62d6` | focus, links |
| `error` | `#ff6b63` | `#d92d20` | error dots and fills |
| `error_text` | `#ff8a84` | `#b42318` | error text |
| `warn` | `#f5c26b` | `#9a5b00` | paused |
| `ok` | `#34c759` | `#1a7f37` | listening |
| `service_palette` | `#4c9dff #3cc8b4 #f0a04b #b18cff #f07fb0` | `#0a62d6 #0f7f70 #b45f06 #6f42c1 #c2306f` | one per service, in order, wrapped |

Errors are never marked by colour alone: a failed listener says `Failed` and
the error in words, the pill says `Port problem`, and the rules row says
`has an error` or `Discard rules file has an error`.

The Linux window always uses the dark theme. Following the system appearance
(§9.3) arrives with the main window in Milestone 3.

## The font

`FONT_FAMILY` is `Inter`, and the crate bundles `Inter-Regular.otf` and
`Inter-SemiBold.otf` (`assets/fonts/`, SIL OFL 1.1, licence next to them).
`register_fonts` adds them to a GPUI `App`; the window does it at launch and
the screenshot harness for every render.

Why bundle: a pixel-exact test must not depend on which fonts a machine
happens to have. On Linux the harness builds its text system with
`CosmicTextSystem::new_without_system_fonts`, so the bundled font is the only
one it can see, and a reference made in one container holds on CI. The design
says the product uses the system font (SF) on macOS; that switch belongs to the
Milestone 3 window, not to this view. See
[`decisions/0008-ui-and-screenshot-choices.md`](../decisions/0008-ui-and-screenshot-choices.md).

## Screenshot tests

`testing::render_png(size, build)` draws a view through GPUI's headless
renderer (`HeadlessAppContext` with `gpui_platform::current_headless_renderer`,
which is wgpu on the CPU through Mesa's lavapipe: no GPU and no display) and
returns an `image::RgbaImage`. `build` runs inside the window and returns an
`AnyView`:

```rust
let image = render_png((900, 600), |_window, cx| {
    cx.new(|_| StatusView::new(model, Theme::dark())).into()
});
assert_snapshot("status_view_dark", &image);
```

- **The image is 2x.** The headless platform draws at scale 2, like a Retina
  display, so `(900, 600)` comes back as 1800 by 1200 pixels.
- **`assert_snapshot(name, &img)`** compares with
  `crates/loctel-ui/tests/snapshots/{name}.png` pixel for pixel (decoded RGBA,
  so the PNG encoder does not matter).
  - `UPDATE_SNAPSHOTS=1` writes the reference instead of comparing.
  - On a mismatch, a size change or a missing reference it writes
    `target/screenshot-diffs/{name}.actual.png` and `{name}.diff.png` (red
    where pixels differ, the reference at a quarter brightness elsewhere) and
    panics with the number of differing pixels and the first one's place.
- **Time.** The first frame in a process takes about two seconds, later ones
  about 100 ms (§10.5), and every `render_png` starts a fresh context, so keep
  the number of screenshot tests reasonable. The three tests of the crate take
  about a second together.
- **Determinism** is a test of its own (`render_is_deterministic` renders
  twice and compares bytes, and also that the image is not one flat colour).

### The feature

`cargo test -p loctel-ui --features screenshot` is the spelled-out form. The
`screenshot` feature turns on GPUI's `test-support`, the platform crate's
`test-support` (which provides `current_headless_renderer`) and `gpui-pre-wgpu`
(the text system). It is off in the shipped app. `loctel-ui` lists itself as a
dev-dependency with the feature on, so every test, clippy and doc build in the
workspace has it, and `cargo test --workspace` (the gate, and what `ci.yml`
runs) runs the screenshot tests without any flag.

### Commands

```bash
cargo xtask screenshots            # the screenshot tests (and the rest of loctel-ui's)
cargo xtask screenshots --update   # rewrite the references (UPDATE_SNAPSHOTS=1), then list them
cargo xtask run-shot               # the real window under Xvfb, to target/run-shot.png
```

`cargo xtask` is an alias set in `.cargo/config.toml` for `cargo run -p xtask --`.

### Changing a reference

Only an intentional change to what a view draws may change a reference PNG,
and the same change must carry it. **View every new PNG before you commit it**
(the Read tool shows images) and check it shows what you meant, legibly. The
repo skill `update-screenshots` has the steps, and how to read a CI failure.

## The real window

`loctel` without `--headless` on Linux opens the status view in a window and
prints the same startup lines as headless mode (the listeners, then the rules
line), so a script that passed port 0 finds the ports. Closing the window quits. With no `DISPLAY` or
`WAYLAND_DISPLAY` it says so on stderr and runs headless: GPUI would otherwise
hand out windows that draw nothing.

`cargo xtask run-shot` builds `loctel` and runs it under `xvfb-run` (1280 by
800) with every port 0 and a scratch settings file. It waits for the window,
gives it two seconds (the first frame and one refresh), captures the whole
screen with `import -window root` and writes `target/run-shot.png`. A
capture of one flat colour means the window stayed black, the old X11
wake-up bug of crates.io GPUI (§10.5); the script then nudges the window with
`xdotool windowmove` and captures again, and fails if it is still blank. With
`gpui-pre` the window was not black on its first try, so the nudge is a safety
net. Open the PNG with the Read tool: the window sits in the black screen at
about (192, 100), 900 by 600.

The repo skill `run-and-screenshot` covers running with meshgen data.

## Unverified

- Everything here was checked on Linux with software rendering. How the status
  view looks and behaves on macOS (fonts, scale, window chrome) is unverified
  until a milestone check on a real Mac; the macOS app opens no window in
  Milestone 1 anyway.
- The references were made with Mesa lavapipe on Ubuntu 24.04. If the CI
  runner's Mesa renders anti-aliasing differently, `ci.yml` uploads the actual
  and diff images as the `screenshot-diffs` artifact; see the
  `update-screenshots` skill.
