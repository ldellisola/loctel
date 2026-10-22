# loctel Milestone 3 (main window: Map & traces, Trace view): Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** loctel gets its main window. It has:

- a glass sidebar with the live status
- a toolbar
- a live **service map** above a **trace table** with TraceQL search (Home A)
- a **trace view**: waterfall, inspector, and the selected span's logs

The window opens from the menu bar. While it's open, loctel shows in the Dock
and ⌘-Tab. It follows the system's light or dark appearance. Agents build,
test and screenshot all of it on Linux.

**Architecture:**

- **Models:** pure Rust in `crates/loctel-ui/src/model/` (layout, rows,
  formatting, waterfall flattening, inspector). They have no GPUI types
  beyond colours, so they are unit-tested fast and plainly.
- **Views:** thin GPUI views in `src/app/` and `src/views/`, using
  **GPUI Kit** (`gpui-kit` =0.8.0) for the table, input, buttons, split
  panels, scroll areas and tooltips. The map is our own `canvas`; the
  waterfall uses `uniform_list`.
- **Data:** a `UiBackend` trait gives views the `Store` and the
  `EngineStatus`. Tests use a fixed backend with a `ManualClock` store
  loaded from meshgen, so screenshots are deterministic.
- **macOS-only parts** (in `loctel-macos`):
  - the "Open loctel" menu item
  - the window lifecycle and activation policy
  - Liquid Glass (`NSGlassEffectView` under GPUI's view)

**Tech Stack:**

- **Added:** `gpui-kit = "=0.8.0"`, which pins `gpui-pre =0.3.9`, matching
  ours.
- **objc2-app-kit features:** `NSGlassEffectView`, `NSView`, `NSResponder`,
  `NSColor`.
- **Unchanged:** everything else from M1 and M2.

**Spec:**

- `docs/superpowers/specs/2026-10-21-loctel-design.md`:
  - §9 (UI), mainly §9.1 lifecycle, §9.2 screens, §9.3 look and §9.4
    failure semantics
  - §3.1 the human flow
- The mockups (Home A, Trace): <https://claude.ai/artifact/QdYLABvppZc3rHGcdxpUJX>.
  The files are private to the user; agents can't open them, so their
  content is described in this plan.
- The existing plans for M1 and M2, for current interfaces.
- `AGENTS.md`.

**Spike evidence (2026-10-22):** a throwaway spike verified the following on
this container (code in the session scratchpad, not in the repo):

- **GPUI Kit:** 0.8.0 builds against our pins.
- **Headless rendering:**
  - Kit views render deterministically in the headless harness once
    `cx.set_reduce_motion(true)` is set. Without it, the scrollbar fade uses
    real time.
  - The test platform always reports the Light appearance, so tests set
    the theme directly.
- **Map drawing:** `canvas` with `PathBuilder` cubic béziers, filled
  arrowheads and `paint_quad` nodes renders.
- **Simulated input:** `window.dispatch_event(PlatformInput::MouseDown/Up/Move/ScrollWheel(..))`
  and `window.dispatch_keystroke(..)` work on `HeadlessAppContext`.
- **macOS facts** (read from source):
  - `NSGlassEffectView` is in objc2-app-kit 0.3.2.
  - GPUI's `Window` implements `HasWindowHandle`, giving an
    `AppKitWindowHandle` whose `ns_view` is GPUI's view, a subview of the
    content view.
  - `cx.set_activation_policy(ActivationPolicy::Regular|Accessory)` applies
    immediately.
  - `WindowBackgroundAppearance::Transparent` makes the window non-opaque.

## Global Constraints

Everything in the M1 and M2 Global Constraints still holds: the quality
gate, `script/check-macos.sh`, commit trailers, no macOS runner on PRs, docs
travelling with code, and never weakening tests. M3 adds:

- **Mockup values:**
  - **Window:** default size 1440×900, minimum 1000×640.
  - **Sidebar:** 220 px wide, floating, inset 10 px, corner radius 16 px.
    It holds the traffic-light space, the navigation, and the status box at
    the bottom.
  - **Corner radii:** content panels 16 px.
  - **Map nodes:** 176×64 px, radius 14 px. Columns are 96 px apart and rows
    32 px apart.
  - **Rows:** trace table rows 36 px, waterfall rows 28 px.
  - **Inspector:** 340 px wide, resizable from 280 to 520 px.
- **Colours** come only from `Theme`, never a hex literal in a view. The
  light theme exists and is tested.
- **Service colours** come from `ServiceColors`: the order of first
  appearance in the session, cycling through `Theme::service_palette`. A
  service keeps its colour everywhere for the whole run.
- **Failure display** (§9.4):
  - a failed trace has the red "Failed in", formatted
    `service · span name`; when the span is a client span with a peer, it
    reads `service · span name peer`, plus ` +N` when `more > 0`
  - errors inside get an amber "errors inside"
  - an exception event without error status gets a marker only
- **Formats** (exact strings, tested):

  | what | rule | examples |
  |---|---|---|
  | durations | ≥ 1 s uses two decimals; ≥ 1 ms uses whole ms; otherwise whole µs | `1.84 s`, `212 ms`, `850 µs` |
  | clock | `HH:MM:SS.mmm`, in the local zone (tests inject UTC) | `14:32:07.412` |
  | rates | ≥ 10 whole; otherwise one decimal | `84 req/s`, `0.3 req/s` |
  | error % | one decimal | `2.1% err`, `0% err` |
- **Refresh:**
  - home data (map and table) every **500 ms** while the window is visible
    and not paused
  - the open trace every **500 ms** until it is complete, then every 2 s
  - every store read runs on GPUI's background executor, never blocking a
    frame
- **Time ranges:** `Last 5 minutes`, `Last 15 minutes` (default),
  `Last hour`, `Everything`.
- **The trace table** shows at most **1,000** traces, the newest matching
  the filter. A footnote says `Showing the newest 1,000` when there are
  more.
- **Screenshots:**
  - every view gets reference PNGs in **dark and light**, at 1440×900
    logical
  - the harness sets reduce motion and forces the theme
  - keep each PNG under about 1.5 MB. If they're bigger, lower the capture
    scale to 1× in the harness and record that in a decision.
- **macOS:**
  - the window background is `Transparent` when glass is attached, and
    `Blurred` as the fallback (no `NSGlassEffectView` class, or attaching
    failed)
  - region frames are applied **outside** GPUI's frame (deferred), to avoid
    the AppKit-calls-back-into-GPUI deadlock others hit
  - all Mac behaviour stays "unverified" until the M3 checklist

## Review Focus

1. **A trace still arriving while it is open** (late spans, the root last).
   The waterfall must add rows and re-root without losing the selected span
   or the scroll position; selection follows the span ID. *Test:* Task 3
   `rebuild_keeps_selection_and_collapsed_state`, and Task 8
   `open_trace_fills_in_as_spans_arrive`.
2. **A huge trace (100,000 spans).** Building the model must stay fast and
   scrolling must work. *Tests:* Task 3 `waterfall_100k_builds_under_budget`
   (debug < 1.5 s) and Task 8 `huge_trace_scrolls_to_middle` (a screenshot
   at row 50,000).
3. **A map with cycles or a single node** (services calling each other, or
   only one service). Layout must terminate, place every node once, and stay
   stable as nodes appear. *Tests:* Task 2 `cycle_terminates_and_places_all`,
   `single_node`, and `new_node_does_not_move_existing_ones`.
4. **A TraceQL error typed in the query bar.** Expect a message under the
   input with its column, the last good results kept, and never a crash or
   an empty table that looks like "no traces". *Test:* Task 7
   `invalid_query_shows_error_and_keeps_rows`.
5. **Very long names and attribute values** (a 300-character span name, a
   20 KB attribute). Expect them truncated with an ellipsis in rows and
   wrapped or scrollable in the inspector, never widening the layout.
   *Tests:* Task 1 `truncate_middle_and_end`, and Task 8
   `long_values_do_not_break_layout` (a screenshot).

---

## File map (M3)

```
Cargo.toml                                  + gpui-kit =0.8.0; objc2-app-kit glass features
crates/loctel-ui/src/model/{mod.rs, format.rs, colors.rs, trace_table.rs, query_bar.rs,
                            map_layout.rs, waterfall.rs, inspector.rs}
crates/loctel-ui/src/app/{mod.rs, backend.rs, shell.rs, sidebar.rs, toolbar.rs, nav.rs,
                          data.rs, backdrop.rs, kit_theme.rs}
crates/loctel-ui/src/views/{map_view.rs, trace_table_view.rs, trace_view.rs,
                            waterfall_view.rs, inspector_view.rs, span_logs_view.rs}
crates/loctel-ui/src/testing.rs             + kit-aware render, forced theme, input helpers
crates/loctel-ui/tests/{model_format.rs, model_table.rs, model_map.rs, model_waterfall.rs,
                        screens.rs, interactions.rs, live.rs}
crates/loctel-ui/tests/snapshots/*.png      new references (dark + light)
crates/loctel-macos/src/{glass.rs, window.rs} + menu: "Open loctel"
crates/loctel-app/src/main.rs               Linux opens the main window; macOS wires window lifecycle
docs/areas/ui.md, docs/areas/macos.md, docs/milestones/{mac-checklist.md, roadmap.md}, AGENTS.md
.claude/skills/{run-and-screenshot, update-screenshots}/SKILL.md, + .claude/skills/upgrade-gpui/SKILL.md
```

---

### Task 1: Pure models: formatting, service colours, trace rows, query bar

**Files:**
- Create: `crates/loctel-ui/src/model/{mod.rs, format.rs, colors.rs, trace_table.rs, query_bar.rs}`
- Modify: `crates/loctel-ui/Cargo.toml` (deps `loctel-store`,
  `loctel-query`, `loctel-model`, `chrono` for local time)
- Test: `crates/loctel-ui/tests/{model_format.rs, model_table.rs}`

**Interfaces:**
- Produces, in `loctel_ui::model`:
  - **Formatting:**
    - `format::duration(nanos: u64) -> String`
    - `format::clock(unix_nano: u64, utc_offset_secs: i32) -> String`
    - `format::rate(per_sec: f64, unit: &str) -> String` (`"84 req/s"`)
    - `format::error_pct(errors: u64, total: u64) -> String` (`"2.1% err"`;
      `"0% err"` when `total == 0`)
    - `format::truncate_end(s: &str, max_chars: usize) -> String` (ends
      with `…`)
    - `format::truncate_middle(s, max_chars) -> String`
    - `format::local_utc_offset() -> i32`: the system zone's offset, used
      by the app; tests pass 0
  - **`colors::ServiceColors`:**
    - `new() -> ServiceColors`
    - `index(&mut self, service: &str) -> usize`: the first-seen order,
      stable; the palette slot is `index % 5`
    - `peek(&self, service: &str) -> Option<usize>`
  - **`trace_table::TraceRow`:**
    - `trace_id: TraceId`
    - `started: String`, `root: String` (the root name, or `(root not
      arrived)`)
    - `failed_in: Option<String>`, `errors_inside: bool`
    - `services: Vec<(String, usize)>` (name and colour index)
    - `span_count: usize`
    - `duration: String`, `duration_fraction: f32` (0..=1, relative to the
      longest row shown)
    - `complete: bool`
  - `trace_table::rows(summaries: &[TraceSummary], colors: &mut ServiceColors, utc_offset: i32) -> Vec<TraceRow>`
  - `trace_table::failed_in_text(f: &FailedIn) -> String`: §9.4 per Global
    Constraints, e.g. `payments-api · POST card-gateway +1`
  - `query_bar::QueryState`, with these variants:
    - `Empty`
    - `Valid(Arc<TraceQuery>)`
    - `Invalid { column: usize, message: String }` (1-based column)
  - `query_bar::validate(text: &str) -> QueryState`: an all-whitespace
    text is `Empty`
  - `query_bar::service_filter(service: &str) -> String`: gives
    `{ resource.service.name = "<escaped>" }`, with `"` and `\` escaped

- [ ] **Step 1: Write the failing tests.**
  - `tests/model_format.rs` holds `durations`, `clock_utc`, `rates`,
    `error_pct_cases` and `truncate_middle_and_end`. They cover the exact
    strings in Global Constraints, plus:
    - `duration(0)` gives `"0 µs"`
    - `duration(1_840_000_000)` gives `"1.84 s"`
    - `duration(999_999_999)` gives `"1000 ms"`. Rounding stays in ms below
      1 s: just state the rule and test it.
    - `truncate_end("abcdef", 4)` gives `"abc…"`
    - `truncate_middle("abcdefghij", 5)` gives `"ab…ij"`
  - `tests/model_table.rs`, using meshgen CheckoutFailure plus
    CheckoutSuccess loaded into a `Store` with a `ManualClock`:
    - `failed_trace_row`: `failed_in == Some("payments-api · POST card-gateway +1")`
      and `errors_inside == false`
    - `ok_trace_row_has_no_failure_text`
    - `services_in_first_seen_order_with_stable_colors`: `web-bff` is 0,
      `orders-api` 1, and so on, the same across two calls
    - `duration_fraction_relative_to_longest`
    - `query_bar_states`: `""` is `Empty`, `{ status = error }` is
      `Valid`, and `{ status = ` is `Invalid { column: 12, .. }`
    - `service_filter_escapes`
- [ ] **Step 2: Watch them fail.** Run `cargo test -p loctel-ui --test model_format --test model_table`.
  Expected: compile failure.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Watch them pass.** Expected: PASS.
- [ ] **Step 5: Docs.** Add a "Models" section to `docs/areas/ui.md` with
  the formats table.
- [ ] **Step 6: Commit.** Message: `feat(ui): formatting, service colours,
  trace rows and query bar models`.

### Task 2: Map layout model

**Files:**
- Create: `crates/loctel-ui/src/model/map_layout.rs`
- Test: `crates/loctel-ui/tests/model_map.rs`

**Interfaces:**
- Consumes: `loctel_store::{ServiceMap, NodeKind}` and `ServiceColors`.
- Produces:
  - `pub struct PlacedNode`:
    - `id: String`, `kind: NodeKind`, `color: Option<usize>` (`None` for
      non-service nodes)
    - `x: f32`, `y: f32`, `w: f32` (176), `h: f32` (64)
    - `subtitle: String`, with the exact formats:
      - service: `"84 req/s · 2.1% err"`
      - database: `"database · 140 q/s"`
      - queue: `"queue · 9 msg/s"`
      - external: `"external · 3 req/s"`
    - `failing: bool` (errors > 0)
  - `pub struct PlacedEdge`:
    - `from: String`, `to: String`
    - `calls: u64`, `errors: u64`, `failing: bool`
    - `label: Option<String>` (`"84 req/s"`, only on edges out of a
      layer-0 node)
  - `pub struct MapLayout { pub nodes: Vec<PlacedNode>, pub edges: Vec<PlacedEdge>, pub width: f32, pub height: f32 }`
  - `pub struct LayoutMemory` (positions remembered between refreshes):
    `default()`
  - `pub fn layout(map: &ServiceMap, window_secs: f64, colors: &mut ServiceColors, memory: &mut LayoutMemory) -> MapLayout`
- **Algorithm:**
  1. Layers come from the longest path from the sources (nodes with no
     incoming edge). Break cycles by ignoring back edges found in a DFS in
     id order.
  2. Order within a layer by the barycentre of predecessors, ties by id.
  3. `x = 40 + layer × (176 + 96)`, `y = 32 + slot × (64 + 32)`.
  4. Stability: a node already in `memory` keeps its layer and slot. New
     nodes go to the first free slot of their computed layer. A node
     missing from `map` is forgotten after 3 refreshes without it, so a
     brief gap doesn't make the map jump.
  5. Rates are `count / window_secs`.

- [ ] **Step 1: Write the failing tests:**
  - `checkout_topology_columns`: CheckoutFailure gives `web-bff` in layer
    0, `orders-api` in 1, `payments-api`, `inventory-api` and `rabbitmq` in
    2, and `postgresql`, `redis`, `card-gateway` and `notifications-worker`
    in 3. Each `x` follows the formula.
  - `failing_flags`: `payments-api` is failing, and so is the edge
    `orders-api→payments-api`.
  - `subtitles_formats`
  - `cycle_terminates_and_places_all`: a↔b plus c gives every node exactly
    once.
  - `single_node`
  - `new_node_does_not_move_existing_ones`
  - `missing_node_kept_for_three_refreshes`
  - `deterministic`: the same input gives an identical layout.
- [ ] **Step 2: Watch them fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Watch them pass.** Run `cargo test -p loctel-ui --test model_map`.
  Expected: PASS.
- [ ] **Step 5: Docs.** Add the layout rules to `docs/areas/ui.md`.
- [ ] **Step 6: Commit.** Message: `feat(ui): stable layered service map
  layout`.

### Task 3: Waterfall and inspector models

**Files:**
- Create: `crates/loctel-ui/src/model/{waterfall.rs, inspector.rs}`
- Test: `crates/loctel-ui/tests/model_waterfall.rs`

**Interfaces:**
- Consumes: `loctel_store::TraceSnapshot`, `ServiceColors`, `format`.
- Produces:
  - `pub struct WaterfallRow`:
    - `span_id: SpanId`, `depth: u16`
    - `has_children: bool`, `collapsed: bool`
    - `name: String`, `service: String`, `color: usize`
    - `peer_note: Option<String>`: `"postgresql"`, `"card-gateway"`, or
      `"missing parent"` for an orphan's placeholder row
    - `start_frac: f32` and `width_frac: f32`: relative to the trace,
      width at least 0.003
    - `duration: String`
    - `is_error: bool`
    - `exception_fracs: Vec<f32>`: offsets of exception events
  - `pub struct Waterfall`, with:
    - `new(snapshot: &TraceSnapshot, colors: &mut ServiceColors) -> Waterfall`:
      children are ordered by start then span ID. Orphans sit under a
      synthetic row `"missing parent"` at depth 0, after the root's subtree.
    - `rebuild(&mut self, snapshot: &TraceSnapshot, colors: &mut ServiceColors)`:
      keeps the collapsed set and the selection by span ID
    - `rows(&self) -> &[WaterfallRow]`: the visible rows only
    - `toggle(&mut self, id: SpanId)`, `collapse_all(&mut self)`,
      `expand_all(&mut self)`
    - `set_errors_path_only(&mut self, on: bool)`: shows only erroring
      spans and their ancestors
    - `find(&self, text: &str) -> Vec<usize>`: visible row indices whose
      name, service or any attribute value contains the text, ignoring
      case
    - `select(&mut self, id: SpanId)`: expands the ancestors so the span is
      visible
    - `selected(&self) -> Option<SpanId>`
    - `row_of(&self, id: SpanId) -> Option<usize>`
    - `ruler(&self) -> [String; 5]`: the 0, 25, 50, 75 and 100% labels,
      such as `["0", "460 ms", "920 ms", "1.38 s", "1.84 s"]`
  - `pub fn initial_selection(snapshot: &TraceSnapshot) -> Option<SpanId>`:
    the failed-in span if any, else the root
  - `pub struct SpanDetails`:
    - `name`, `service`, `kind: String`
    - `duration: String`, `offset: String` (`"starts at +200 ms"`)
    - `status: String`
    - `exception: Option<ExceptionInfo>`
    - `attributes: Vec<(String, String)>` and
      `resource: Vec<(String, String)>`, both sorted by key, with values
      kept whole (the view truncates)
  - `pub fn span_details(snapshot: &TraceSnapshot, id: SpanId) -> Option<SpanDetails>`

- [ ] **Step 1: Write the failing tests.** They use CheckoutFailure, plus
  HugeTrace from meshgen.
  - `rows_in_tree_order_with_depths`: the order and depths match the
    Task 8 table of the M1 plan.
  - `fractions_match_offsets`: `POST /charges` has
    `start_frac ≈ 200/1840` and `width_frac ≈ 1586/1840`, ±0.001.
  - `exception_marker_on_charges`
  - `initial_selection_is_failed_in`
  - `collapse_and_expand`
  - `errors_path_only_shows_errors_and_ancestors`
  - `find_matches_name_service_and_attribute_values`
  - `select_expands_ancestors`
  - `orphans_under_missing_parent_row`
  - `rebuild_keeps_selection_and_collapsed_state`
  - `ruler_labels`: gives `["0", "460 ms", "920 ms", "1.38 s", "1.84 s"]`
  - `span_details_of_charges`: the exception type, `starts at +200 ms`,
    and sorted attributes
  - `waterfall_100k_builds_under_budget`: HugeTrace{100000} builds in less
    than 1.5 s, measured with `Instant` in a debug test build
- [ ] **Step 2: Watch them fail.**
- [ ] **Step 3: Implement.** Use one pass to build child lists and an
  iterative DFS (no recursion).
- [ ] **Step 4: Watch them pass.** Run `cargo test -p loctel-ui --test model_waterfall`.
  Expected: PASS.
- [ ] **Step 5: Commit.** Message: `feat(ui): waterfall and inspector
  models`.

### Task 4: GPUI Kit, theme bridge, app shell, harness upgrade

**Files:**
- Modify: the root `Cargo.toml` (`gpui-kit = "=0.8.0"` in workspace deps)
  and `crates/loctel-ui/Cargo.toml`
- Create: `crates/loctel-ui/src/app/{mod.rs, backend.rs, shell.rs, sidebar.rs, toolbar.rs, nav.rs, kit_theme.rs}`
- Modify: `crates/loctel-ui/src/{lib.rs, theme.rs, testing.rs}`
- Test: `crates/loctel-ui/tests/screens.rs` (shell screenshots)
- Create: `docs/decisions/00NN-gpui-kit.md`
- Modify: `.claude/skills/update-screenshots/SKILL.md`. Also create
  `.claude/skills/upgrade-gpui/SKILL.md`; the spec asks for an upgrade
  guide.

**Interfaces:**
- Produces:
  - `pub trait UiBackend: 'static`:
    - `fn store(&self) -> Store`
    - `fn status(&self) -> EngineStatus`
    - `fn clear_all(&self)`
    - `fn utc_offset(&self) -> i32`

    `impl UiBackend for EngineHandle` uses `format::local_utc_offset()`.
    `testing::FixedBackend { store, status, utc_offset: 0 }` is for tests.
  - `pub enum Screen { Home, Trace(TraceId) }`
  - `pub struct MainWindow`, the root view, with:
    - `new(backend: Rc<dyn UiBackend>, backdrop: Option<Box<dyn Backdrop>>, window, cx)`
    - `screen(&self) -> &Screen`
    - `navigate(&mut self, s: Screen, cx)`
  - `pub fn open_main_window(backend: Rc<dyn UiBackend>, cx: &mut App, attach_backdrop: impl FnOnce(&mut Window) -> Option<Box<dyn Backdrop>>) -> anyhow::Result<WindowHandle<gpui_kit::base::Root>>`.
    It sets:
    - the 1440×900 default size and the 1000×640 minimum size
    - the title "loctel"
    - `cx.set_reduce_motion` left at the system value
  - `Theme::for_appearance(a: WindowAppearance) -> Theme` (dark for `Dark`
    and `VibrantDark`)
  - `kit_theme::apply(theme: &Theme, cx: &mut App)`: maps our tokens onto
    GPUI Kit's `ThemeColor` (background, foreground, border, accent, and the
    table, input and button colours) and sets the font family to `Inter`
  - `MainWindow` observes `window.observe_window_appearance` and re-applies
    the theme.
  - **Shell layout:**
    - the sidebar, with a single navigation item `Map & traces` (Logs and
      Discard rules arrive in M4) and the status box at the bottom
    - the status box shows `status_line`, `rules_line`, the OTLP ports and
      the MCP address, all from `StatusModel`
    - the content area, with a toolbar on top whose contents depend on the
      screen
  - `pub trait Backdrop: 'static { fn set_regions(&self, regions: &[GlassRegion]); }`,
    with `pub struct GlassRegion { pub bounds: Bounds<Pixels>, pub corner_radius: Pixels }`
    (see Task 10). In this task the shell only *reports* the sidebar and
    toolbar bounds, through `Backdrop::set_regions` and only when they
    change.
  - **Testing additions:**
    - `testing::render_window_png(size, theme: &Theme, build: impl FnOnce(&mut Window, &mut App) -> Entity<V>) -> RgbaImage`.
      It calls `gpui_kit::init`, wraps the view in `Root`, forces the
      theme, sets reduce motion, and runs until parked.
    - `testing::Driver`: a headless window that you drive, then capture.
      Methods: `click(point)`, `scroll(point, dy)`, `type_text(&str)`,
      `key(&str)`, `advance(Duration)`, `capture() -> RgbaImage`, and
      `update_root(|view, window, cx|)`.
    - `testing::RecordingBackdrop`, which records every
      `set_regions` call.

- [ ] **Step 1: Write the failing tests** in `tests/screens.rs`:
  - `shell_dark` and `shell_light`: an empty store and a status with 3
    listening endpoints and no rules. They compare against
    `snapshots/shell_dark.png` and `snapshots/shell_light.png`.
  - `shell_reports_glass_regions_once`: with a `RecordingBackdrop`, render
    two frames. There are exactly 2 regions (sidebar and toolbar), with
    non-zero sizes, recorded once (no repeat when unchanged).
  - `appearance_switch_changes_theme`: call `MainWindow`'s appearance
    handler with Dark, then Light, and check the Kit background colour
    changes to `Theme::dark().bg`, then `Theme::light().bg`.
- [ ] **Step 2: Watch them fail.**
- [ ] **Step 3: Implement.** Generate the references with
  `UPDATE_SNAPSHOTS=1`, then **open both PNGs with the Read tool** and check
  them against the Global Constraints. Check the PNG sizes.
- [ ] **Step 4: Run the full gate.** The existing `status_view`
  screenshots must still match. If adding GPUI Kit changes their pixels
  (fonts or shaders), regenerate them, view them, and say why in the
  commit.
- [ ] **Step 5: Docs.**
  - The decision file: why GPUI Kit, what it costs (a bigger lockfile and
    a rebuild), reduce motion, and the forced theme in tests.
  - `docs/areas/ui.md`: the shell and the backend.
  - The skills.
- [ ] **Step 6: Commit.** Message: `feat(ui): GPUI Kit, theme bridge, main
  window shell`.

### Task 5: Live data for the home screen

**Files:**
- Create: `crates/loctel-ui/src/app/data.rs`
- Test: `crates/loctel-ui/tests/live.rs`

**Interfaces:**
- Produces:
  - `pub enum TimeRange { Last5m, Last15m, LastHour, Everything }` with
    `Default = Last15m`, `label()` and `since(now) -> Option<u64>`
  - `pub struct HomeData`:
    - `map: MapLayout`
    - `rows: Vec<TraceRow>`, `truncated: bool`
    - `query: QueryState`
    - `status: StatusModel`
    - `refreshed_at: u64`
  - `pub struct HomeModel` (an `Entity`), with:
    - `new(backend, cx)`
    - `set_query(&mut self, text: &str, cx)`: validates at once; an invalid
      query keeps the previous rows
    - `set_errors_only(bool)`, `set_slow_only(bool)` (≥ 1 s),
      `set_range(TimeRange)`, `set_paused(bool)`
    - `select_service(Option<String>)`: writes `service_filter` into the
      query, or clears it
    - `data(&self) -> &HomeData`
    - `refresh_now(&mut self, cx)`

    The refresh timer runs every 500 ms while not paused, and does its
    reads on `cx.background_executor()`. The filter combines time range,
    errors only, slow only and the valid query, with a limit of 1,000.
- [ ] **Step 1: Write the failing tests** in `tests/live.rs`, using a
  `HeadlessAppContext`, a `FixedBackend` and a `ManualClock`:
  - `refresh_picks_up_new_traces`: ingest, advance 500 ms, and the rows
    grow.
  - `paused_does_not_refresh`
  - `range_filters_old_traces`
  - `invalid_query_keeps_previous_rows`
  - `select_service_writes_query`
  - `errors_only_and_slow_only_combine`
  - `truncation_flag_over_1000`
- [ ] **Step 2: Watch them fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Watch them pass.**
- [ ] **Step 5: Commit.** Message: `feat(ui): live home data with ranges,
  toggles and query`.

### Task 6: Service map view

**Files:**
- Create: `crates/loctel-ui/src/views/map_view.rs`
- Test: `crates/loctel-ui/tests/screens.rs` and `tests/interactions.rs`
  (both extended)

**Interfaces:**
- Produces `pub struct MapView` (an `Entity`):
  - `new(home: Entity<HomeModel>, cx)`
  - It draws `HomeData.map` on a `canvas`:
    - nodes are rounded quads, filled `panel`, with a coloured dot (a
      circle for services, a rounded square for the other kinds)
    - **failing nodes** have an `error` border and `error_text` subtitle;
      the **selected** node has a 3 px `accent` ring
    - edges are cubic béziers from the right-edge middle to the left-edge
      middle, `stroke 2`, with filled arrowheads; failing edges use the
      `error` colour
    - edge labels sit above the curve's midpoint
    - non-service nodes have a dashed border
  - **Pan:** drag. **Zoom:** ctrl or cmd plus scroll, or pinch, clamped
    from 0.5 to 2.0. A `Fit` button resets.
  - Clicking a service node calls `select_service(Some(id))`; clicking the
    selected one, or empty space, clears it.
  - When the map is empty it shows `Waiting for telemetry` and
    `Point OTEL_EXPORTER_OTLP_ENDPOINT at http://localhost:4317`.
- [ ] **Step 1: Write the failing tests.**
  - `home_map_dark` and `home_map_light`: CheckoutFailure, CheckoutSuccess
    and HealthChecks{1}, with the whole home screen visible. Their
    references are `home_dark.png` and `home_light.png`. The table is
    present but may be a placeholder until Task 7, which regenerates these
    references.
  - `empty_map_message`
  - In `tests/interactions.rs`:
    - `click_service_filters`: click `payments-api`'s node centre (from the
      layout) and the query text becomes
      `{ resource.service.name = "payments-api" }`. Then capture
      `home_payments_selected_dark.png`.
    - `drag_pans_and_fit_resets`
- [ ] **Step 2: Watch them fail.** **Step 3: Implement.** Generate the
  references and **view them**.
- [ ] **Step 4: Run the full gate.**
- [ ] **Step 5: Commit.** Message: `feat(ui): live service map view`.

### Task 7: Trace table view and query bar

**Files:**
- Create: `crates/loctel-ui/src/views/trace_table_view.rs`
- Modify: the home layout in `shell.rs`, so the map sits on top and the
  table panel fills the rest
- Test: `screens.rs` and `interactions.rs` (both extended)

**Interfaces:**
- Produces `pub struct TraceTableView`:
  - **Query row:**
    - a `TraceQL` label chip, a GPUI Kit `Input` and a `Run` button
    - Enter runs it too, and typing validates live
    - when invalid, an `error_text` line under the input reads
      `at column N: message`
  - **Toggle row:**
    - `Errors only` and `Slower than 1 s` (Kit `Toggle`s)
    - on the right, `N traces · newest first`, or
      `Showing the newest 1,000` when truncated
  - **Table:** a GPUI Kit `DataTable` with a delegate over
    `HomeData.rows`.
    - **Columns:**
      - `Started` (mono)
      - `Trace` (the root name, bold)
      - `Failed in` (`error_text`, or amber `errors inside`)
      - `Services` (8 px dots in service colours, with a tooltip of the
        names)
      - `Spans` (right-aligned)
      - `Duration` (the text plus a 4 px bar)
    - Rows are 36 px.
    - Clicking a row navigates to `Screen::Trace(id)`.
  - **Empty states:**
    - no data: `No traces yet`
    - a filter with no results: `No traces match`, and a `Clear filters`
      button
- [ ] **Step 1: Write the failing tests.**
  - Regenerate `home_dark.png` and `home_light.png` with the real table.
  - `home_errors_only_dark`
  - `invalid_query_shows_error_and_keeps_rows`: type `{ status = ` and a
    screenshot shows the error line `at column 12: …` and the previous rows
    (`home_query_error_dark.png`).
  - `click_row_opens_trace`: click the CheckoutFailure row, and the screen
    is `Trace(id)`.
  - `no_match_shows_clear_filters`
- [ ] **Step 2: Watch them fail.** **Step 3: Implement.** Regenerate and
  **view** the references.
- [ ] **Step 4: Run the full gate.**
- [ ] **Step 5: Commit.** Message: `feat(ui): trace table with TraceQL bar
  and toggles`.

### Task 8: Trace view (waterfall, inspector, span logs)

**Files:**
- Create: `crates/loctel-ui/src/views/{trace_view.rs, waterfall_view.rs, inspector_view.rs, span_logs_view.rs}`
- Test: `screens.rs`, `interactions.rs` and `live.rs` (all extended)

**Interfaces:**
- Produces `pub struct TraceView`:
  - `new(backend, trace_id, window, cx)`
  - **Header row:**
    - a `Traces` back button (Esc does the same)
    - the root span name
    - a red `Failed in …` badge, or an amber `Errors inside`
    - `1.84 s · 15 spans · 5 services · 14:32:07.412`
    - the trace ID (mono, middle-truncated)
    - a `Copy ID` button
    - the completeness indicator: `Receiving spans…` (accent) or
      `Complete` (ok)
  - **The body** is a horizontal Kit resizable split: the left side holds
    the waterfall and the span logs; the inspector is on the right,
    340 px.
  - **Waterfall toolbar:** a `Find` input (⌘F focuses it; Enter jumps to
    the next match, with a count `2 of 5`), an `Errors path only` toggle,
    and a `Collapse all` / `Expand all` button.
  - **Ruler row:** labels from `Waterfall::ruler`.
  - **Rows:** a `uniform_list` of 28 px rows, each holding:
    - indent `6 + depth×16`
    - a disclosure chevron (click toggles)
    - a 3 px service colour bar
    - an error icon (a red circle with `!`) when `is_error`
    - the name (ellipsis) and a tertiary service/peer note
    - the bar in the service colour, at 0.75 opacity unless erroring or
      selected
    - diamond markers at the exception fractions
    - the duration on the right

    The selected row has an `accent` tint at 18% alpha. Clicking a row
    selects it.
  - **Inspector** (in a Kit scroll area):
    - the title, service with its dot, kind, duration and offset
    - **Exception card:** the type in mono, the message, the stack trace in
      mono with pre-wrap, and a `Copy` button
    - **Attributes and Resource:** key/value grids. Values over 200
      characters are cut with `… (show all)`; clicking expands them.
    - **Buttons:** `Traces through this span name` (navigates home with
      the query `{ name = "<escaped>" }`) and `Copy span as JSON`
  - **Span logs** (under the waterfall, about 200 px):
    `Logs from <span name>`, with a count and rows of time, level badge and
    message, from `store.logs_for_span`.
  - **Live:** a refresh every 500 ms while not complete, then every 2 s.
    It calls `Waterfall::rebuild`.
  - **Opening:** the selection starts at `initial_selection` and the list
    scrolls to it.
- [ ] **Step 1: Write the failing tests.**
  - `trace_dark` and `trace_light`: CheckoutFailure opened. The failed-in
    span (`POST` card-gateway) is selected and the inspector shows it.
  - `trace_charges_selected_dark`: click the `POST /charges` row, and the
    inspector shows the exception card with the stack trace.
  - `trace_errors_path_only_dark`
  - `huge_trace_scrolls_to_middle`: HugeTrace{100000}, scroll to row
    50,000, then capture (`huge_trace_middle_dark.png`).
  - `long_values_do_not_break_layout`: a span with a 300-character name and
    a 20 KB attribute (`long_values_dark.png`).
  - `find_counts_and_jumps`
  - `back_returns_home_keeping_query`
  - `traces_through_span_name_sets_query`
  - In `live.rs`: `open_trace_fills_in_as_spans_arrive`. Ingest
    LateOutOfOrder's 3 batches one at a time, advancing the clock between
    them. Meshgen sends the 10 spans at depth 3 and deeper first, then the 3
    middle spans, then the root.
    - Real span rows grow 10 → 13 → 14.
    - The `missing parent` row is present until the root arrives.
    - Once the root arrives, the tree is rooted at `POST /api/checkout`.
    - A selection made after the first batch is kept throughout.
- [ ] **Step 2: Watch them fail.** **Step 3: Implement.** Regenerate and
  **view every new PNG**.
- [ ] **Step 4: Run the full gate.**
- [ ] **Step 5: Commit.** Message: `feat(ui): trace view with waterfall,
  inspector and span logs`.

### Task 9: Window lifecycle (Linux app and macOS)

**Files:**
- Modify: `crates/loctel-app/src/main.rs`. Linux without `--headless` opens
  `open_main_window` instead of the status window. `StatusView` and its
  test stay as a standalone view.
- Create: `crates/loctel-macos/src/window.rs`
- Modify: `crates/loctel-macos/src/{menu.rs, status_item.rs, lib.rs}`
- Test: the `loctel-macos` menu tests, plus `crates/loctel-app/tests/headless.rs`
  (unchanged and still passing)
- Modify: `docs/areas/macos.md`, `docs/areas/ui.md`

**Interfaces:**
- Produces, on macOS:
  - **The menu:** a new first item after the two header rows,
    `Open loctel` (⌘O, `Command::OpenWindow`). Its full order is:
    1. the status header and the rules header
    2. `Open loctel`
    3. a separator
    4. `Copy MCP address`, `Pause/Resume receiving`, `Edit discard rules…`
    5. a separator
    6. `Start at login`
    7. a separator
    8. `Quit loctel`
  - `window::open_or_focus(cx: &mut App, engine: EngineHandle)`:
    - if the main window exists, activate it
    - otherwise:
      1. call `cx.set_activation_policy(ActivationPolicy::Regular)`
      2. open it with `open_main_window(Rc::new(engine), cx, |w| glass::attach(w))`
         (Task 10; it returns `None` until then)
      3. call `cx.activate(true)`
  - When the main window closes: `cx.set_activation_policy(ActivationPolicy::Accessory)`.
    The app keeps running, because `QuitMode` is explicit.
  - Reopen: if the Dock icon is clicked while no window is open (GPUI's
    reopen event, `cx.on_reopen`), run `open_or_focus`.
  - The app no longer opens a window at launch; that is unchanged from M1.
- On Linux: closing the main window quits, as before.
- [ ] **Step 1: Write the failing tests.**
  - Menu plain-value tests: `rows_have_open_first_after_headers`, checking
    the exact order above.
  - A Linux smoke test in `crates/loctel-app/tests/window.rs`:
    `linux_window_opens_under_xvfb`. If `xvfb-run` exists, run
    `loctel --grpc-port 0 --http-port 0 --mcp-port 0 --settings <tmp> --rules <tmp>`
    under it for 3 s. Expect the listener lines printed, no panic on
    stderr, and a clean exit on SIGTERM. Mark the skip explicitly (print
    `skipped: no xvfb-run`) when it is absent. CI installs xvfb, so CI
    never skips.
- [ ] **Step 2: Watch them fail.** **Step 3: Implement.**
- [ ] **Step 4: Check it works.**
  - Run the full gate and `bash script/check-macos.sh --all-targets`.
  - Run `cargo xtask run-shot`. Update `xtask run-shot` so it captures the
    main window; it now waits about 2 s and sends meshgen CheckoutFailure
    first, so the shot has data.
  - **View `target/run-shot.png`.**
- [ ] **Step 5: Docs.** Update `docs/areas/macos.md` with the lifecycle,
  and add these to the "unverified" list: Open loctel, Dock and ⌘-Tab while
  open, Accessory after close, reopen from the Dock.
- [ ] **Step 6: Commit.** Message: `feat(app,macos): open the main window
  from the menu bar; Dock while open`.

### Task 10: Liquid Glass behind the sidebar and toolbar (macOS)

**Files:**
- Create: `crates/loctel-macos/src/glass.rs`
- Modify: the root `Cargo.toml` (objc2-app-kit features `NSGlassEffectView`,
  `NSView`, `NSResponder`, `NSColor`)
- Modify: `crates/loctel-macos/src/window.rs` (pass `glass::attach`)
- Modify: `crates/loctel-ui/src/app/shell.rs`. When a backdrop is
  attached, paint the sidebar and toolbar with a transparent fill (glass
  shows through), with a 1 px hairline border and an inner highlight.
  Without one, use the opaque translucent-look fill from the mockups.
- Test: a `loctel-ui` screenshot with `RecordingBackdrop` attached,
  `shell_glass_dark.png`: the sidebar and toolbar painted transparent over
  the window colour. Also `loctel-macos` unit tests for the frame
  conversion.
- Create: `docs/decisions/00NN-liquid-glass.md`

**Interfaces:**
- Produces, on macOS:
  - `glass::attach(window: &mut Window) -> Option<Box<dyn Backdrop>>`:
    - returns `None` if `NSGlassEffectView` isn't available (macOS
      earlier than 26) or the window handle isn't AppKit
    - otherwise: sets `WindowBackgroundAppearance::Transparent`, creates
      one non-hit-testing, clipping host `NSView` per region, below GPUI's
      view (`addSubview:positioned:NSWindowBelow relativeTo:<gpui view>`),
      each holding an `NSGlassEffectView` (style Regular, corner radius
      from the region) with an empty content view, and returns a backdrop
      whose `set_regions` stores the latest regions and applies the frames
      **deferred** to the next main-queue turn (never inside GPUI's frame)
  - The caller in `window.rs`: when `attach` returns `None`, set
    `WindowBackgroundAppearance::Blurred`. That is the fallback, and it is
    logged once.
  - `pub(crate) fn to_appkit_frame(region: Bounds<Pixels>, view_height: f64, flipped: bool) -> NSRect`
    is pure and unit-tested in plain Rust: it flips y when the view isn't
    flipped.
- [ ] **Step 1: Write the failing tests.**
  - `to_appkit_frame_flips_y_when_not_flipped`
  - `to_appkit_frame_keeps_y_when_flipped`
  - the `shell_glass_dark` screenshot
- [ ] **Step 2: Watch them fail.** **Step 3: Implement.** Keep every
  `unsafe` block in `glass.rs`, each with a `// SAFETY:` comment.
- [ ] **Step 4: Run the full gate and `script/check-macos.sh --all-targets`.**
  View the new PNG.
- [ ] **Step 5: Docs.** In the decision file: the approach, the deferred
  frames and why (the deadlock risk), the Blurred fallback, and that it is
  unverified until the M3 Mac check. Update `docs/areas/macos.md`.
- [ ] **Step 6: Commit.** Message: `feat(macos): Liquid Glass behind the
  sidebar and toolbar`.

### Task 11: Docs, skills, Mac checklist, final screenshots pass

**Files:**
- Modify: `docs/areas/ui.md`, `docs/architecture.md`, `AGENTS.md`,
  `docs/milestones/roadmap.md`, `docs/milestones/mac-checklist.md`
- Modify: `.claude/skills/run-and-screenshot/SKILL.md`, which now opens
  the main window
- Test: none new; this task is the docs and checklist pass. Run the full
  gate.

- [ ] **Step 1: Docs.**
  - `docs/areas/ui.md`:
    - the screen map
    - every view and the model behind it
    - the screenshot list (name, data, what it shows)
    - how to add a screen
  - `docs/architecture.md`: the UI layer and the data flow (engine, then
    `UiBackend`, then models, then views).
  - `AGENTS.md`: the skills table gains `upgrade-gpui`, and the commands
    gain the main-window run.
  - `roadmap.md`: link the M3 plan.
- [ ] **Step 2: The Mac checklist.** Add a **"Milestone 3"** section to
  `docs/milestones/mac-checklist.md`, keeping the M1 and M2 sections. Each
  item gives the steps and what "right" looks like:
  1. **Open loctel** (from the menu, or ⌘O while the menu is open). The
     window appears in under half a second. loctel now shows in the Dock
     and ⌘-Tab. Close it: the Dock icon goes, and loctel keeps running in
     the menu bar. Click the Dock icon while it's open, and it comes to
     the front.
  2. **Glass:** the sidebar and the toolbar show the desktop or windows
     behind them through Apple's glass (blurred and refracted, changing as
     you move the window over different backgrounds). The content areas
     are solid. Report whether it looks like Liquid Glass or like plain
     blur; plain blur is the fallback.
  3. **Light/dark:** switch the appearance in System Settings while the
     window is open, and it follows within a second.
  4. **Live map and table:** with meshgen sending checkout failures on
     repeat, the map shows web-bff → orders-api → payments-api with
     `payments-api` red, and new rows appear at the top of the table about
     twice a second without the scroll jumping.
  5. **Filtering:** click `payments-api` on the map; the query bar fills
     in and the table filters. Type a broken query; an error appears under
     the bar and the rows stay.
  6. **Trace view:** open a failed checkout. The failed-in span is selected
     and the inspector shows the exception. Pan and zoom the map with the
     trackpad: pinch to zoom, two-finger drag to pan.
  7. **Big trace:** send `meshgen --scenario huge-trace`, open it, and
     scroll fast from top to bottom. It stays smooth with no blank rows;
     report any stutter.
- [ ] **Step 3: Run the full gate** and `script/check-macos.sh`. View all
  the reference PNGs once more as a set, checking the dark and light pairs
  look consistent.
- [ ] **Step 4: Commit.** Message: `docs: M3 UI docs, skills and Mac
  checklist`.

---

## After the last task (M3 ship)

1. **Whole-branch review** against the spec (§9, §3.1) and this plan,
   including a look at every reference PNG. Fix what it finds.
2. **Open the PR** from `claude/m3-main-window`. Get the Linux CI green.
   Screenshot diffs on the CI runner are possible; the `update-screenshots`
   skill covers them.
3. **Squash-merge** with the title
   `feat: milestone 3, main window with service map, traces and trace view`.
   Watch the release until it's published with the `.dmg` and notes holding
   the M1, M2 and M3 checklist sections.
4. **Tell the user.**
