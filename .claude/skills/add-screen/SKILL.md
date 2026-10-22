---
name: add-screen
description: "Add a screen to loctel's main window (crates/loctel-ui): a Screen variant, a sidebar item with its shortcut, a pure model with tests, a data entity that reads in the background, a thin view, dark and light screenshot references that you have looked at, and the docs. Use whenever the main window gains a screen (as Logs and Discard rules did in Milestone 4), or a screen gains a way to be reached."
---

# Add a screen to the main window

The main window shows one `Screen` at a time inside a shell (the glass sidebar
and toolbar). The four it has (`Home`, `Trace`, `Logs`, `Rules`) are the
examples: **Logs** is the one that reads the store over time (copy it for a
data-heavy screen), **Rules** is the one that edits state and takes a draft from
another screen (copy it for a form), and **Trace** is a screen that is reached
from another one and is not a sidebar entry. `docs/areas/ui.md` has the map
("Screens and views at a glance"), the models, the theme tokens and the
screenshot harness; read "Adding a screen" there too, which is the same list
without the commands. The whole job is one pull request, test first, with the
docs in it.

## Steps

1. **Decide what the screen is.** Its name, its sidebar label and icon (or why
   it is not an entry), how a user gets there and back, what it reads, and what
   it changes. Write down the strings that matter (headings, counts, empty
   states, errors): they go into the model and its tests, not into the view. If
   the choice could be undone by a later agent who does not see the reason (a
   limit, a refresh period, a place in the sidebar), it gets a decision file
   (`docs/decisions/`, next free number, short).
2. **The model, test first.** Everything that is not a pixel goes in
   `crates/loctel-ui/src/model/<name>.rs` as plain Rust over the store's types:
   the rows, the strings, the numbers, the limits as `pub const`, colours as
   indices into `ServiceColors` and never as values. Add `pub mod <name>;` to
   `src/model/mod.rs` and pin it with plain assertions in
   `crates/loctel-ui/tests/model_<name>.rs` (or `model_misc.rs` for a small
   one). Write the test, run it (`cargo test -p loctel-ui --test model_<name>`)
   and **see it fail**, then implement. Use `model::format` for every number,
   time and truncation. No GPUI type appears in a model.
3. **The data entity, when the screen reads over time.** A GPUI entity in
   `src/app/<name>_data.rs`, built over `Rc<dyn UiBackend>` (never the engine),
   in the style of `LogsModel` and `RulesModel`:
   - it reads the store on `cx.background_executor()` and applies the result on
     the foreground; **a frame never reads the store**;
   - it refreshes on a timer that stops when the entity is dropped, and calls
     `cx.notify()` only when something changed;
   - it reads **only while its screen shows** (`set_active(bool, cx)`, which the
     shell calls from `navigate`), so a screen nobody is looking at costs nothing;
   - a read for an old query or an old state that finishes late is dropped
     (carry a generation number), so a fast typist never sees stale rows.
   Test it with `testing::headless_app()` (`tests/live.rs`) or, when it lives in
   the window, with `Driver` in the screen's own test file (`logs_screen.rs`), over
   a `FixedBackend` on a store with a `ManualClock`; move the clock (`advance`,
   `advance_clock`) to fire the timer. Extend
   `UiBackend` (`app/backend.rs`) only if the screen needs something the backend
   does not give yet; then give `FixedBackend` and `AppBackend` the method too.
4. **The view.** `src/views/<name>_view.rs`: a thin GPUI view that lays the
   model out with GPUI Kit's components and writes what the user does back into
   the entity (or emits an event the shell turns into a `navigate`). Colours come
   from `cx.global::<Theme>()` and nothing else. Cut long text with the
   `format::truncate_*` helpers and make no layout depend on the width of a
   string. Give it `for tests` accessors (`bounds_of`, `center_of`, as the
   existing views have) so a test clicks what it means to click and never guesses
   pixels. Add it to `src/views/mod.rs`.
5. **The shell.** In `src/app/`:
   - **`Screen` variant** in `shell.rs` (carry data only when another screen can
     hand it one, as `Logs { query }` and `Rules { draft }` do), a field in
     `MainWindow` for the view and the entity, an arm in `MainWindow::render`, and
     a call to `set_active` in `navigate`.
   - **A title** for the toolbar: an arm in `toolbar_title` (`app/toolbar.rs`),
     and `toolbar_contents` if the screen has controls of its own (`LogsControls`
     is the example).
   - **A sidebar entry** (skip this when the screen is reached from another one
     and stays lit under it, as `Trace` does through `is_active`): a `NavItem` in
     `app/nav.rs` (`ALL`, `label`, `icon` from Kit's Lucide catalog
     `gpui_kit::assets::IconName`, `shortcut`, `screen`, `is_active`), an action in
     the `actions!` list, its `KeyBinding` in `bind_keys`, and its `on_action`
     handler on the shell's root element. The shortcut is `cmd-N` in GPUI's syntax
     (the Super key on Linux, ⌘ on a Mac); ⌘1 to ⌘3 and ⌘, are taken. An icon that
     Kit's default bundle lacks goes into `src/assets.rs` too (the list and the
     funnel did).
   - **Leaving and returning.** Decide whether the screen's views live as long as
     the window (like Logs and Rules: the user finds them as they left them) or
     are made on arrival and dropped on leaving (like Trace). Say which, in
     `ui.md`.
   Pin the shell's side in `tests/` with `Driver`: the sidebar entry is lit, the
   shortcut and the click show the screen, the title is right, and going away and
   back keeps what it should.
6. **The screenshots, test first.** In a test file of its own
   (`tests/<name>_screen.rs`, like `logs_screen.rs` and `rules_screen.rs`) render
   the screen with `Driver` over a `FixedBackend` and a store loaded from meshgen
   on a `ManualClock`, at `WINDOW` (1440 by 900), in `Theme::dark()` **and**
   `Theme::light()`. Then one dark reference for each other state that matters:
   empty, an error, an open editor or detail, a loading or capped list. Write
   `assert_snapshot("<name>_dark", &image)` calls first, run them to see the
   missing reference fail, then `cargo xtask screenshots --update`. **Open every
   PNG with the Read tool** and check it shows what its test says: legible text,
   the right colours in both themes, nothing clipped except what scrolls, a light
   twin that differs from its dark one only in colour. A PNG you have not looked
   at is not a reference. The `update-screenshots` skill has the details.
7. **Run it for real.** The headless harness and the real window share the code,
   but only the second one has an event loop and real input. Follow
   `run-and-screenshot`: start the window under Xvfb, send meshgen data, press the
   screen's shortcut with `xdotool key super+N`, wait two seconds and capture. Look
   at the picture.
8. **The docs, in the same change.**
   - `docs/areas/ui.md`: a section for the screen (the view, the model, what the
     user can do, the screenshots, what is unverified on a Mac), a row in "Views
     and the models behind them", the screen map and the table under it in
     "Screens and views at a glance", a row per reference in "The references", and
     the shortcut where "Navigation" lists them.
   - `docs/architecture.md`: the UI section (the module list, the entities that
     read the store, the data-flow diagram) if a module or an entity was added.
   - `AGENTS.md`: the line about the screens, if its count changed.
   - the milestone's plan (the file map) and `docs/milestones/mac-checklist.md` (an
     item that says what to try on a Mac and what is right), with
     `docs/milestones/roadmap.md` if a milestone changes.
   - a decision file for any choice a later agent could undo by mistake.
9. **The gate.** `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D
   warnings`, `cargo test --workspace`, and `bash script/check-macos.sh
   --all-targets` when macOS code changed. Never skip, loosen or `#[ignore]` a
   test to get green: a screenshot that moved without a reason is a regression.

## Things that bite

- **A screen that is not showing must not read.** Forgetting `set_active` makes
  every screen refresh at once, and the screenshots stay green while the app gets
  slow. Add a test that moves the clock while another screen shows and asserts no
  read happened (`a_hidden_logs_screen_reads_nothing` in `tests/logs_screen.rs`).
- **The view must never name the engine.** If a view needs something, the
  backend (`UiBackend`) gets a method and `FixedBackend` gets the in-memory
  version, so the screenshot tests keep working.
- **Lists that scroll need GPUI's `list` or `uniform_list`**, not a `div` of
  children; a screen that can hold 20,000 rows must draw only the ones in view.
  A `uniform_list` cannot hold rows of different heights (an expanded one): use
  `list` and keep its state in step with the model, as `LogsView` does.
- **The test platform always reports the light appearance.** `render_window_png`
  and `Driver` force the theme you pass; a view that reads the window's
  appearance has already chosen light by then.
- **Time in a test is the test dispatcher's**, not the store's. A store on a
  `ManualClock` moves only when the test advances that clock as well.
- **A screenshot with text in it is only worth keeping if you read the text.**
  Check the strings against the model's tests.
