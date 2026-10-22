# loctel Milestone 4 (Logs, Discard rules, popover, Settings): Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the remaining screens of the design (§9.2):

- **Logs:**
  - LogQL search, level toggles, live tail and a volume histogram
  - expandable lines that link to their trace and to the lines around them
  - "Discard lines like this…"
- **Discard rules:** the list with on/off switches and counts, plus an
  editor with live validation and the 5-minute preview.
- **The menu bar popover:** it replaces the menu.
- **A Settings window:** ports and start at login.

The sidebar gains `Logs` and `Discard rules`. Everything except hosting the
popover and the login item is built, tested and screenshotted on Linux.

**Architecture:** The same layers as M3.

- **Store additions:** stable log IDs, level and time-window filters, a
  histogram, and the lines around a log.
- **Engine addition:** rebinding the ports live.
- **UI backend:** the `UiBackend` trait grows the rules and settings APIs,
  implemented by an `AppBackend` in `loctel-ui` (engine plus settings path
  plus an optional login-item hook).
- **Views:** pure models in `src/model/`, then thin views.
- **macOS:** `loctel-macos` swaps the `gpui-tray` menu for an activation
  callback that opens a GPUI pop-up window (the popover) under the
  click.

**Tech Stack:** unchanged from M3 (GPUI `gpui-pre` =0.3.9, `gpui-kit`
=0.8.0, `gpui-tray` 0.2, objc2).

**Spec:**

- `docs/superpowers/specs/2026-10-21-loctel-design.md`: §9.2 (Logs,
  Discard rules, Menu bar popover, Settings), §7 (rules), §6.2 (LogQL),
  §9.1 (lifecycle)
- `docs/areas/ui.md` and `docs/areas/rules.md`
- the M1–M3 plans, for existing interfaces
- `AGENTS.md`

**Evidence for the popover (2026-10-22, from reading `gpui-tray` 0.2.0):**

- `TrayBuilder::on_activate_with(callback)` fires on a primary click
  **only when no menu is attached** (`src/tray.rs:179-199`).
- It doesn't report the item's position. So the popover is placed from
  `NSEvent::mouseLocation()` at the click, under the menu bar, using the
  click's screen's `visibleFrame`.

## Global Constraints

Everything in the M1–M3 Global Constraints still holds: the gate,
`script/check-macos.sh`, the trailers, no macOS runner on PRs, docs with
code, reference PNGs viewed before committing, colours only from `Theme`,
`ServiceColors`, and "unverified until the Mac check". M4 adds:

- **Mockup values:**
  - **Logs columns:** time 96 px (mono), level 44 px (a badge), service
    112 px (dot plus name), message (mono, flexible, one line with an
    ellipsis), trace 96 px (mono, an 8-character link). Rows are 30 px.
  - **Histogram:** **48** buckets, 64 px high, with a 2 px gap between bars.
    Errors are stacked at the bottom of each bar in `error`, the rest in
    `border`. Four time labels sit under it.
  - **Level toggles:**

    | label | includes | colour |
    |---|---|---|
    | `ERR` | error, fatal | `error_text` |
    | `WRN` | warn | `warn` |
    | `INF` | info, unspecified | `text_secondary` |
    | `DBG` | debug, trace | `text_tertiary` |

    All four are on by default. A toggle that is off has no fill.
  - **Rules screen:** cards in a list (a switch, name, a `TRACEQL` or
    `LOGQL` chip, the query in mono, the count text on the right) and an
    editor panel 380 px wide (glass-styled).
  - **Popover:** 336 px wide, radius 18 px, glass.
  - **Settings window:** 520×360, not resizable.
- **Exact strings** (tested). Rule card counts:
  - a trace rule: `48,210 spans dropped`
  - a trace rule with scope trace and dropped traces: `48,210 spans dropped
    in 602 traces`
  - a log rule: `96,433 lines dropped`
  - when off: `Off`
  - when invalid: `Error: <message>` in `error_text`
  - Numbers use a thousands separator `,`.
- **Logs list paging:**
  - pages of **500**, newest first, loaded as the scroll nears the end
    (within 50 rows)
  - at most **20,000** rows in memory; past that a footer reads
    `Showing the newest 20,000 matching lines` (the 10M-line scroll is M5)
- **Live tail:** on by default.
  - **While the list is at the top:** the newest page is re-read every
    500 ms and new lines are prepended.
  - **Scrolling away from the top** pauses the tail, and a pill
    `New lines · Jump to newest` appears. Clicking it, or scrolling back to
    the top, resumes.
  - The histogram is recomputed every 2 s.
- **Discard rule suggestion** ("Discard lines like this…"):
  - name: `Lines like "<first 40 chars of body>…"` (no `…` when the body
    fits)
  - language: `logql`
  - query: `{service_name="<service>"} |= "<body text>"`, where the body
    text is the first 60 characters, cut back to the last word boundary,
    with `"` and `\` escaped
  - it opens in the rules editor **unsaved**
- **Ports:** 1–65535, all three distinct. Changing one rebinds that
  listener at once. If the new port can't be bound, the listener shows
  `Failed` and the setting is still saved. The window says
  `otlp-http: address in use`.
- **Disk cap:** not shown in Settings until M5 (it isn't enforced yet).
- **The popover replaces the M1–M2 menu.** Its rows, top to bottom:
  1. the status pill (`Receiving`, `Paused` or `Port problem`) and the
     title `loctel`
  2. a 2×2 stats grid: spans/s, log lines/s, traces held, log lines held.
     Disk use is M5; the mockup's disk tile is replaced by "log lines held".
  3. the endpoints: OTLP gRPC, OTLP HTTP, and MCP with a `Copy` button
  4. the rules line (`rules_line`). Clicking it opens the main window on
     Discard rules.
  5. a divider
  6. `Open loctel` ⌘O, `Pause receiving` / `Resume receiving`, and
     `Clear all data…` (asks first)
  7. the `Start at login` switch (macOS only)
  8. `Settings…` ⌘,
  9. a divider
  10. `Quit loctel` ⌘Q

  The popover closes when it loses focus, on Esc, or after any action that
  opens a window.

## Review Focus

1. **Live tail while lines arrive faster than the refresh** (a log flood).
   No line may appear twice, and the scroll position must not jump while
   the tail is paused. *Test:* Task 4 `tail_dedupes_and_paused_tail_keeps_position`.
2. **A rule edited in the file while the editor has an unsaved draft of
   it.** The draft stays and a banner says the file changed. Saving
   overwrites the file only after confirming. *Test:* Task 5
   `external_change_during_edit_warns`.
3. **A rule with an invalid query saved from the editor** (allowed as a
   draft, per M2). It shows `Error: …` on its card and drops nothing, and
   the preview shows the parse error instead of numbers. *Test:* Task 5
   `invalid_rule_saved_shows_error_and_no_preview`.
4. **A port change to a port in use, or to the same port as another
   listener.** Same-port is rejected in the form. A port in use is saved
   but shows Failed with the reason, and the other listeners keep running.
   *Tests:* Task 2 `rebind_to_busy_port_reports_failed_keeps_others`, and
   Task 6 `duplicate_ports_rejected`.
5. **"Show lines around" on a log whose neighbours were evicted, or that
   was itself evicted.** It shows what remains, with the target marked if
   present, else the message `This line is no longer held`. *Test:* Task 1
   `logs_around_evicted_target`.

---

## File map (M4)

```
crates/loctel-store/src/{logs.rs, store.rs, page.rs}    LogId, keyed search, levels/until, histogram, around
crates/loctel-engine/src/lib.rs                         rebind_ports
crates/loctel-ui/src/app/{backend.rs, app_backend.rs, nav.rs, shell.rs, settings_window.rs}
crates/loctel-ui/src/model/{logs.rs, histogram.rs, rules.rs, settings.rs, popover.rs}
crates/loctel-ui/src/views/{logs_view.rs, histogram_view.rs, rules_view.rs, rule_editor_view.rs,
                            settings_view.rs, popover_view.rs}
crates/loctel-ui/tests/{model_logs.rs, model_rules.rs, model_misc.rs, logs_screen.rs,
                        rules_screen.rs, settings_popover.rs}   (+ snapshots)
crates/loctel-macos/src/{popover.rs, status_item.rs, menu.rs(removed or reduced), window.rs, login_item.rs}
crates/loctel-app/src/main.rs                           AppBackend wiring
docs/areas/{ui.md, macos.md, store.md}, docs/milestones/{mac-checklist.md, roadmap.md}, AGENTS.md
.claude/skills/add-screen/SKILL.md (new)
```

---

### Task 1: Store: log IDs, level and window filters, histogram, lines around

**Files:**
- Modify: `crates/loctel-store/src/{logs.rs, store.rs, lib.rs}`
- Test: `crates/loctel-store/tests/logs_m4.rs`
- Modify: `docs/areas/store.md`

**Interfaces:**
- Produces:
  - `#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)] pub struct LogId { pub time_unix_nano: u64, pub seq: u64 }`:
    the existing internal key, made public. It is stable while the log is
    held.
  - `Store::search_logs_keyed(&self, f: &LogFilter, p: &PageRequest) -> Page<(LogId, LogRecord)>`:
    the same order and cursor as `search_logs`. `search_logs` stays, and is
    implemented on top of it.
  - `Store::log(&self, id: LogId) -> Option<LogRecord>`
  - `LogFilter` gains two fields:
    - `pub until_unix_nano: Option<u64>`: exclusive
    - `pub levels: Option<LevelSet>`: `None` means every level
  - `#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)] pub struct LevelSet(u8)`,
    with:
    - `LevelSet::ALL`
    - `with(self, Severity) -> Self`
    - `contains(self, Severity) -> bool`
    - `from_groups(err: bool, wrn: bool, inf: bool, dbg: bool) -> LevelSet`:
      uses the groups in Global Constraints
  - `#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)] pub struct HistogramBucket { pub total: u64, pub errors: u64 }`,
    where errors means error or fatal.
  - `Store::log_histogram(&self, f: &LogFilter, since: u64, until: u64, buckets: usize) -> Vec<HistogramBucket>`:
    buckets of equal width over `[since, until)`, exactly `buckets` long,
    counting logs that match `f`. `f`'s own time fields are ignored.
  - `Store::logs_around(&self, id: LogId, before: usize, after: usize) -> Around`,
    where `pub struct Around { pub lines: Vec<(LogId, LogRecord)>, pub target: Option<usize> }`.
    It is unfiltered and oldest first. `target` is the index of `id` if it
    is still held. If it isn't, the result is the lines nearest to its time.
- [ ] **Step 1: Write the failing tests:**
  - `keyed_search_matches_unkeyed`
  - `log_by_id`
  - `until_is_exclusive`
  - `level_groups`: `from_groups(true, false, false, false)` holds error
    and fatal, not warn. `inf` includes unspecified.
  - `histogram_buckets_and_errors`: 48 buckets over 15 minutes. CheckoutFailure's
    7 logs land in the last bucket, with errors equal to 3.
  - `histogram_respects_filter`
  - `logs_around_target_in_middle`: 3 before and 3 after, with `target ==
    Some(3)`
  - `logs_around_evicted_target`
- [ ] **Step 2: Watch them fail. Step 3: Implement. Step 4: Watch them
  pass.** Run `cargo test -p loctel-store`. The existing tests stay green.
- [ ] **Step 5: Docs, then commit.** Message: `feat(store): log IDs, level
  and window filters, histogram, lines around`.

### Task 2: Engine port rebinding and the app backend

**Files:**
- Modify: `crates/loctel-engine/src/lib.rs` (plus tests in
  `tests/rebind.rs`)
- Create: `crates/loctel-ui/src/app/app_backend.rs`
- Modify: `crates/loctel-ui/Cargo.toml` (add a dependency on
  `loctel-rules`, for `RuleSpec`, `RuleStatus`, `RuleId`, `Preview` and
  `preview`)
- Modify: `crates/loctel-ui/src/app/backend.rs` (and `testing::FixedBackend`)
- Modify: `crates/loctel-app/src/main.rs`, which uses `AppBackend` for
  the main window
- Modify: `crates/loctel-macos/src/window.rs`, which constructs `AppBackend`
  with a macOS login hook

**Interfaces:**
- Produces:
  - `EngineHandle::rebind(&self, ports: Ports) -> Vec<ListenerStatus>`,
    where `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub struct Ports { pub otlp_grpc: u16, pub otlp_http: u16, pub mcp: u16 }`.
    For each listener whose port changed, it stops it and binds the new
    one. A listener whose bind fails shows `Failed`; the others stay. It
    returns the new statuses in the usual order.
  - `UiBackend` gains these methods:
    - `fn rules(&self) -> Vec<RuleStatus>`
    - `fn set_rules(&self, specs: Vec<RuleSpec>) -> anyhow::Result<()>`
    - `fn set_rule_enabled(&self, id: RuleId, on: bool) -> anyhow::Result<()>`
    - `fn preview_rule(&self, spec: &RuleSpec) -> Result<Preview, QueryError>`
    - `fn rules_generation(&self) -> u64` (see below)
    - `fn settings(&self) -> Settings`
    - `fn apply_settings(&self, s: Settings) -> anyhow::Result<Vec<ListenerStatus>>`:
      saves `settings.toml` and rebinds
    - `fn login_item(&self) -> Option<Rc<dyn LoginItem>>`
    - `fn set_paused(&self, on: bool)`
    - `fn mcp_url(&self) -> Option<String>`
  - **Rules generation:** `EngineHandle` gains `rules_generation() -> u64`,
    which increments on every replace (a file reload or `set_rules`). The
    editor compares generations to detect external changes.
  - `pub trait LoginItem { fn is_enabled(&self) -> bool; fn set_enabled(&self, on: bool) -> anyhow::Result<()>; }`
  - `pub struct AppBackend { engine: EngineHandle, settings_path: PathBuf, login: Option<Rc<dyn LoginItem>> }`
    with `new(..)`, implementing `UiBackend`. Remove the
    `impl UiBackend for EngineHandle`; every caller moves to `AppBackend`.
  - `testing::FixedBackend` implements every new method in memory, using
    `loctel_rules::{RuleSet, preview}` over its store.
- [ ] **Step 1: Write the failing tests.**
  - Engine:
    - `rebind_moves_http_listener`
    - `rebind_to_busy_port_reports_failed_keeps_others`
    - `rebind_unchanged_is_noop`
    - `rules_generation_increments_on_reload_and_set`
  - UI: `app_backend_saves_settings_and_rebinds` (temp dir, ports 0, then
    new ports from free ones).
- [ ] **Step 2: Watch them fail.** **Step 3: Implement.** **Step 4: Run the
  full gate** and `script/check-macos.sh`.
- [ ] **Step 5: Commit.** Message: `feat(engine,ui): live port rebinding and
  the app backend`.

### Task 3: Pure models: logs, histogram, rules, suggestion, settings, popover

**Files:**
- Create: `crates/loctel-ui/src/model/{logs.rs, histogram.rs, rules.rs, settings.rs, popover.rs}`
- Test: `crates/loctel-ui/tests/{model_logs.rs, model_rules.rs, model_misc.rs}`

**Interfaces:**
- Produces:
  - `logs::LogRow`:
    - `id: LogId`
    - `time: String` (`format::clock`)
    - `level: LevelBadge` (`Err|Wrn|Inf|Dbg`), with `label()` giving `ERR`,
      `WRN`, `INF` or `DBG`
    - `service: String`, `color: usize`
    - `message: String`: the first line only, newlines become ` ⏎ `
    - `trace_short: Option<String>`: the first 8 hex digits
    - `trace_id: Option<TraceId>`, `span_id: Option<SpanId>`
  - `logs::rows(records: &[(LogId, LogRecord)], colors, utc_offset) -> Vec<LogRow>`
  - `logs::details(record) -> Vec<(String, String)>`: the label set from
    `loctel_query::logql::labels`, plus `body` (the whole body) first
  - `logs::LogQueryState { Empty, Valid(Arc<LogQuery>), Invalid { column, message } }`
    and `logs::validate(text) -> LogQueryState`
  - `logs::span_logs_query(service: &str, span: SpanId) -> String`: gives
    `{service_name="<svc>"} | span_id = "<hex>"`, used by the trace view's
    `Open in Logs`
  - `histogram::Bars`:
    - `bars: Vec<(f32 total_frac, f32 error_frac)>`: the fractions of the
      tallest bar
    - `labels: [String; 4]`: clock times at 0, ⅓, ⅔ and the end
  - `histogram::bars(buckets: &[HistogramBucket], since: u64, until: u64, utc_offset) -> Bars`
  - `rules::RuleRow`:
    - `id: RuleId`
    - `enabled: bool`
    - `name: String`
    - `chip: &'static str`: `TRACEQL` or `LOGQL`
    - `query: String`
    - `counts: String`: the exact strings in Global Constraints
    - `error: Option<String>`
  - `rules::rows(statuses: &[RuleStatus]) -> Vec<RuleRow>`
  - `rules::dropped_line(fraction: f64) -> String`: gives
    `"31% of incoming data dropped"`, or `"Nothing dropped yet"` at 0
  - `rules::Draft`:
    - fields: `name: String`, `language: RuleLanguage`, `query: String`,
      `scope: DropScope`, `enabled: bool`
    - `from_spec`, `to_spec`
    - `validate(&self) -> DraftCheck`, where `DraftCheck { name_ok: bool, query: Result<(), (usize, String)> }`
    - `can_save(&self) -> bool`: true for any non-empty name. Invalid queries
      may be saved, per M2.
  - `rules::suggest_from_log(record: &LogRecord) -> Draft`: per Global
    Constraints
  - `settings::Form`:
    - fields: `grpc: String`, `http: String`, `mcp: String`,
      `start_at_login: Option<bool>`
    - `from(settings, login)`
    - `validate(&self) -> Result<Ports, Vec<(Field, String)>>`. The messages
      are exact: `Enter a port from 1 to 65535` and
      `Each endpoint needs its own port`.
  - `popover::Popover`:
    - `pill: Pill { text, tone: Ok|Warn|Error }`
    - `stats: [(String value, &'static str label); 4]`: labels are
      `spans per second`, `log lines per second`, `traces held` and
      `log lines held`. Values use `menu_text::format_rate` for rates and
      thousands separators for counts.
    - `endpoints: Vec<(&'static str, String, bool ok)>`
    - `rules_line: String`
    - `paused: bool`
    - `start_at_login: Option<bool>`
  - `popover::from_status(status: &EngineStatus, login: Option<bool>) -> Popover`
- [ ] **Step 1: Write the failing tests.** Every exact string above gets an
  assertion, plus:
  - rows from CheckoutFailure logs
  - a multi-line body shown on one line
  - histogram fractions and labels
  - the rule rows for every count, off and error case
  - `suggest_from_log` with a body containing quotes, longer than 60
    characters, cut at a word boundary
  - the settings validation cases, including duplicate ports
  - the popover pill priorities (a failed listener over paused over
    receiving)
- [ ] **Step 2: Watch them fail. Step 3: Implement. Step 4: Watch them
  pass.**
- [ ] **Step 5: Docs.** Add the models table to `docs/areas/ui.md`.
  **Commit.** Message: `feat(ui): logs, histogram, rules, settings and
  popover models`.

### Task 4: The Logs screen

**Files:**
- Create: `crates/loctel-ui/src/views/{logs_view.rs, histogram_view.rs}`,
  and `crates/loctel-ui/src/app/logs_data.rs` (`LogsModel`)
- Modify: `crates/loctel-ui/src/views/trace_view.rs`: add the `Open in Logs`
  button in the span-logs header
- Test: `crates/loctel-ui/tests/logs_screen.rs`, plus snapshots

**Interfaces:**
- Produces:
  - `pub struct LogsModel` (an `Entity`), with:
    - `new(backend, cx)`
    - `set_query(&str)`, `set_levels(LevelSet)`, `set_range(TimeRange)`
    - `set_tail(bool)`
    - `load_more(cx)`
    - `expand(Option<LogId>)`
    - `show_around(LogId)` and `back_to_search()`
    - `rows()`, `histogram()`, `state()`
  - Live tail and paging follow Global Constraints. Every read runs on the
    background executor.
  - **`LogsView`, top to bottom:**
    1. the toolbar: title `Logs`, a `Live tail` toggle pill, the range
       dropdown (shared with home) and `Clear all`
    2. the search panel: `LogQL` chip, input, `Run`, with the error line
       under them
    3. the level toggles and, on the right,
       `N lines · newest first` (or `20,000+ lines`)
    4. the histogram
    5. the list
  - **The list:** clicking a row expands it inline. The expanded part
    shows:
    - a details grid: the body whole (pre-wrap), then the labels
    - buttons: `Open trace` (navigates to `Screen::Trace(id)` and selects
      the log's span), `Show lines around this one`, and `Discard lines
      like this…` (navigates to `Screen::Rules` with the editor open on
      `suggest_from_log`, unsaved)
  - **Context mode** ("lines around"): a banner
    `Lines around 14:32:09.194 · Back to search`, with the target row
    highlighted. If the target was evicted, it reads
    `This line is no longer held`.
  - `Screen` gains `Logs { query: Option<String> }` and
    `Rules { draft: Option<Draft> }`. Task 7 wires the sidebar; this task
    only adds the variants and the view.
  - The trace view's `Open in Logs` navigates to
    `Screen::Logs { query: Some(span_logs_query(service, span)) }`.
- [ ] **Step 1: Write the failing tests.**
  - `logs_dark` and `logs_light`: CheckoutFailure plus LogFlood{200}, with
    a `ManualClock`.
  - `logs_expanded_dark`: the ERR "Charge failed…" line expanded.
  - `logs_query_error_dark`
  - `logs_context_dark`
  - `level_toggles_filter`
  - `paging_loads_more_near_end`: LogFlood{3000} gives 500 rows, then a
    scroll to the end gives 1000.
  - `tail_dedupes_and_paused_tail_keeps_position`
  - `open_trace_from_log_selects_span`
  - `discard_like_this_opens_editor_with_suggestion`
  - `open_in_logs_from_trace_view_sets_query`
- [ ] **Step 2: Watch them fail. Step 3: Implement.** Generate the
  references and **view them**. **Step 4: Run the full gate.**
- [ ] **Step 5: Docs, then commit.** Message: `feat(ui): Logs screen with
  LogQL, levels, live tail and histogram`.

### Task 5: The Discard rules screen

**Files:**
- Create: `crates/loctel-ui/src/views/{rules_view.rs, rule_editor_view.rs}`,
  and `crates/loctel-ui/src/app/rules_data.rs`
- Test: `crates/loctel-ui/tests/rules_screen.rs`, plus snapshots

**Interfaces:**
- Produces `RulesView`:
  - **Header:** `Discard rules`, a subtitle (`Anything a rule matches is
    dropped as it arrives and never stored. Rules are kept when loctel
    restarts.`), `rules::dropped_line` on the right, and a `New rule`
    button.
  - **Banners:**
    - when `rules_file_error` is set: `The rules file has an error: <msg>.
      loctel keeps using the rules it had.` in `error_text`
    - when there are no rules: `No discard rules. Nothing is dropped
      unless you add one.`
  - **Cards** (`RuleRow`):
    - the switch calls `set_rule_enabled`
    - clicking the card opens it in the editor
  - **Editor panel:**
    - fields: `Name` (input), `Applies to` (a segmented control
      `Traces (TraceQL)` / `Logs (LogQL)`), `Query` (a multi-line input
      with the error line under it), and, for traces only, a radio
      `When a span matches, drop` with `the whole trace it belongs to` /
      `only the matching span and its children`
    - **Preview box** `Tried on the last 5 minutes`, debounced 300 ms:
      - valid: `Would have dropped 1,204 spans in 602 traces. None of those
        traces had errors.` (or `3 of those traces had errors.`), or for
        logs `Would have dropped 96 lines. 2 were errors.`
      - invalid: `Fix the query to see what it would drop.`
    - buttons: `Save`, `Cancel`, `Delete` (with a confirmation)
  - **Saving** builds the new full list and calls `set_rules`.
  - **External change while editing:** if `rules_generation` moved since
    the editor opened, a banner reads `The rules file changed while you
    were editing.` with `Keep my draft` and `Discard my draft`. Saving
    after `Keep my draft` overwrites the file.
  - The rules list refreshes every 1 s, to show reloads and counts.
- [ ] **Step 1: Write the failing tests.**
  - `rules_dark` and `rules_light`: three rules (a trace rule on with
    counts, a log rule on, a trace rule off), with the editor closed.
  - `rules_editor_dark`: the first rule open, with its preview filled.
  - `rules_empty_dark`
  - `rules_file_error_dark`
  - `toggle_switch_disables_rule`
  - `new_rule_save_appears_in_list`
  - `invalid_rule_saved_shows_error_and_no_preview`
  - `external_change_during_edit_warns`
  - `delete_asks_then_removes`
- [ ] **Step 2: Watch them fail. Step 3: Implement.** Generate and **view**
  the references. **Step 4: Run the full gate.**
- [ ] **Step 5: Docs, then commit.** Message: `feat(ui): Discard rules
  screen with editor and preview`.

### Task 6: The Settings window

**Files:**
- Create: `crates/loctel-ui/src/views/settings_view.rs` and
  `crates/loctel-ui/src/app/settings_window.rs`
- Test: `crates/loctel-ui/tests/settings_popover.rs` (the settings part),
  plus snapshots

**Interfaces:**
- Produces:
  - `pub fn open_settings_window(backend: Rc<dyn UiBackend>, cx: &mut App) -> anyhow::Result<WindowHandle<Root>>`:
    520×360 and not resizable. If one is already open, it focuses that
    one.
  - **`SettingsView` layout:**
    - title `Settings`
    - a section `Endpoints`, with three labelled inputs (`OTLP gRPC port`,
      `OTLP HTTP port`, `MCP port`), each showing its current status
      (`listening` in `ok`, or `failed: <reason>` in `error_text`)
    - on macOS (when `login_item()` is `Some`): a section `Startup`,
      holding a `Start loctel at login` switch
    - buttons: `Apply` (enabled when the form differs and is valid) and
      `Close`
    - validation messages under the fields
  - **Apply** calls `apply_settings`, then shows the resulting listener
    states.
- [ ] **Step 1: Write the failing tests.**
  - `settings_dark` and `settings_light`
  - `duplicate_ports_rejected`
  - `apply_rebinds_and_shows_failure`: uses a `FixedBackend` whose
    `apply_settings` reports `otlp-http` failed
  - `login_switch_hidden_without_login_item`
- [ ] **Step 2: Watch them fail. Step 3: Implement.** Generate and **view**
  the references. **Step 4: Run the full gate.**
- [ ] **Step 5: Commit.** Message: `feat(ui): Settings window with live
  port changes`.

### Task 7: Sidebar navigation and cross-screen flows

**Files:**
- Modify: `crates/loctel-ui/src/app/{nav.rs, shell.rs, sidebar.rs}`
- Test: `crates/loctel-ui/tests/interactions.rs` (extended), plus
  regenerated shell and home references (the sidebar changed)

**Interfaces:**
- Produces:
  - **Sidebar items:** `Map & traces` (⌘1), `Logs` (⌘2), `Discard rules`
    (⌘3), each with its icon (map, list, funnel; Lucide from GPUI Kit's
    assets).
  - **Settings shortcut:** ⌘, opens Settings from the main window.
  - **Screen state:** switching screens keeps each screen's state for the
    session: the home query, the logs query and tail, and the editor
    draft.
  - **Back behaviour:** the trace view's `Traces` back button returns to
    the screen it was opened from (home or logs). Its label becomes
    `Back`.
- [ ] **Step 1: Write the failing tests.**
  - `sidebar_switches_screens_and_keeps_state`
  - `keyboard_shortcuts_switch_screens`
  - `trace_back_returns_to_logs_when_opened_from_logs`
  - Regenerate `shell_*`, `home_*` and `trace_*`, plus any reference with
    the sidebar, and **view every changed PNG**.
- [ ] **Step 2: Watch them fail. Step 3: Implement. Step 4: Run the full
  gate.**
- [ ] **Step 5: Commit.** Message: `feat(ui): sidebar navigation across
  Map & traces, Logs and Discard rules`.

### Task 8: The popover view (cross-platform)

**Files:**
- Create: `crates/loctel-ui/src/views/popover_view.rs`
- Test: `crates/loctel-ui/tests/settings_popover.rs` (the popover part),
  plus snapshots

**Interfaces:**
- Produces:
  - `pub struct PopoverView`:
    - `new(backend: Rc<dyn UiBackend>, actions: Rc<dyn PopoverActions>, cx)`
    - it renders `popover::from_status` per Global Constraints and
      refreshes every 1 s
  - `pub trait PopoverActions`, with: `open_main(Screen)`, `open_settings()`,
    `toggle_pause()`, `clear_all()` (the view asks for confirmation inline
    first: `Clear all traces and logs?` with `Clear` / `Cancel`),
    `copy_mcp()`, `set_start_at_login(bool)`, `quit()`, and `close()`.
  - The rules line calls `open_main(Screen::Rules { draft: None })`.
  - `PopoverView` reports one `GlassRegion` covering itself through an
    optional `Backdrop`.
- [ ] **Step 1: Write the failing tests.**
  - `popover_dark` and `popover_light`: receiving, with rates
  - `popover_paused_dark`
  - `popover_port_problem_dark`
  - `popover_clear_confirms`: uses a recording `PopoverActions`
  - `popover_actions_dispatch`
- [ ] **Step 2: Watch them fail. Step 3: Implement.** Generate and **view**
  the references. **Step 4: Run the full gate.**
- [ ] **Step 5: Commit.** Message: `feat(ui): menu bar popover view`.

### Task 9: macOS: the popover replaces the menu; Settings and login wiring

**Files:**
- Create: `crates/loctel-macos/src/popover.rs`
- Modify: `crates/loctel-macos/src/{status_item.rs, menu.rs, window.rs, lib.rs, login_item.rs}`
- Modify: `crates/loctel-app/src/main.rs` (macOS path)
- Test: plain-Rust unit tests for the anchoring arithmetic, which run in the
  Linux gate if the function is `cfg`-free. Then `script/check-macos.sh
  --all-targets`.
- Modify: `docs/areas/macos.md` and `docs/decisions/00NN-popover.md`

**Interfaces:**
- Produces:
  - **Status item:** built with `Tray::builder().icon(..).on_activate_with(|_, cx| popover::toggle(cx))`
    and **no menu**.
  - `popover::toggle(cx)`:
    - if the popover is open, close it
    - otherwise:
      1. read `NSEvent::mouseLocation()` and the `NSScreen` containing it
      2. compute the origin with `anchor(click_x, screen_frame, visible_frame, size) -> (x, y)`
      3. open a GPUI window with `WindowKind::PopUp`, no titlebar, bounds
         at that origin, `WindowBackgroundAppearance::Transparent`, and
         glass via `glass::attach`, falling back to `Blurred`
      4. activate it
  - **Closing:** the popover closes when its window is deactivated
    (`observe_window_activation`), on Esc, or on any action that opens a
    window.
  - **`anchor` arithmetic:** a pure function, unit-tested on Linux.
    - The popover's top sits at the top of `visible_frame` minus 6 px.
    - It is horizontally centred on the click.
    - It is clamped so its whole 336 px width stays inside `visible_frame`,
      with an 8 px margin.
  - **`PopoverActions` implementation:**
    - `open_main`: `window::open_or_focus`, then navigate
    - `open_settings`: `open_settings_window`, with the activation policy
      Regular while it's open
    - `toggle_pause`: the engine
    - `clear_all`: the store
    - `copy_mcp`: the clipboard
    - `set_start_at_login`: `login_item::set_enabled`
    - `quit`: shut down the engine, then quit
  - **Shortcuts:** ⌘Q works while the popover has focus, and ⌘, opens
    Settings.
  - **Fallback:** if opening the popover window fails, open the main
    window instead and log once. loctel never becomes unreachable.
  - **Old menu code:** `menu.rs` loses the old rows. Keep its plain-value
    tests only if they still describe something; otherwise remove them
    together with the code, and say why in the commit. Removing tests for
    deleted behaviour is allowed; weakening tests for kept behaviour is not.
- [ ] **Step 1: Write the failing tests:**
  - `anchor_centres_under_click`
  - `anchor_clamps_at_right_edge`
  - `anchor_clamps_at_left_edge`
  - `anchor_below_menu_bar`
- [ ] **Step 2: Watch them fail. Step 3: Implement.** **Step 4: Run the full
  gate** and `script/check-macos.sh --all-targets`.
- [ ] **Step 5: Docs.** Add these to the "unverified" list in
  `docs/areas/macos.md`:
  - the popover's position and glass
  - closing on click-away
  - ⌘Q and ⌘,
  - the fallback
  - Settings window activation

  Write the decision file: anchoring from the mouse location, because
  `gpui-tray` has no item frame.
  **Commit.** Message: `feat(macos): menu bar popover replaces the menu;
  Settings and login wiring`.

### Task 10: Docs, skills, Mac checklist, final pass

**Files:**
- Modify: `docs/areas/ui.md`, `docs/architecture.md`, `AGENTS.md`,
  `docs/milestones/{roadmap.md, mac-checklist.md}`
- Create: `.claude/skills/add-screen/SKILL.md`, covering:
  - the steps to add a screen: a `Screen` variant, a sidebar item and
    shortcut, a model in `src/model` with tests, a data entity with
    background reads, a thin view, dark and light references (viewed),
    and docs
- Modify: `.claude/skills/run-and-screenshot/SKILL.md`, which gains how
  to reach Logs and Discard rules under Xvfb (⌘2 and ⌘3 through `xdotool`)

- [ ] **Step 1: Docs.** Update the `ui.md` screen map and screenshot list,
  the `architecture.md` UI section, the AGENTS.md skills table
  (`add-screen`), and link the plan from `roadmap.md`.
- [ ] **Step 2: The Mac checklist.** Add a **Milestone 4** section, keeping
  the sections before it. In Milestone 1's text, add a note that the
  M1–M2 menu is now the popover, so a tester on v0.1.3 or later uses the
  popover for those items. Then these items:
  1. **The popover.** Click the menu bar icon. A glass panel opens just
     under it, with status, rates, endpoints, the rules line and the
     actions. Clicking elsewhere closes it. Esc closes it. ⌘Q in it quits.
  2. **The popover actions.** Check `Pause receiving` / `Resume receiving`,
     `Copy` beside MCP, and `Clear all data…`, which asks first. `Open
     loctel` opens the window. `Settings…` opens Settings.
  3. **Logs.** With meshgen `log-flood --repeat`, open Logs (⌘2). New lines
     appear at the top. Scroll down and the `Jump to newest` pill shows.
     Type `{service_name="payments-api"} | level = "error"`. Expand a line,
     then `Open trace`.
  4. **Discard rules** (⌘3). Create a rule with `New rule` and watch the
     preview numbers. Save it, toggle it off and on, and watch the counts.
     Edit `rules.toml` in a text editor while the editor is open, and the
     "file changed" banner appears.
  5. **"Discard lines like this…"** from a log opens the editor prefilled.
  6. **Settings.** Change the OTLP HTTP port to a free one and Apply. The
     status shows listening on it, and a service pointed at the new port
     arrives. Change it to a port that is in use, and see the failure
     message. Change it back.
  7. **Start at login** switches from the popover and from Settings agree.
- [ ] **Step 3: Run the full gate** and `script/check-macos.sh`. View every
  reference PNG once, as a set.
- [ ] **Step 4: Commit.** Message: `docs: M4 UI docs, add-screen skill and
  Mac checklist`.

---

## After the last task (M4 ship)

1. **Whole-branch review** against the spec (§9.2, §7, §6.2, §9.1) and this
   plan, including every reference PNG. Fix what it finds.
2. **Open the PR** from `claude/m4-logs-rules-popover`. Get the Linux CI
   green.
3. **Squash-merge** with the title
   `feat: milestone 4, Logs and Discard rules screens, menu bar popover, Settings`.
   Watch the release until it's published with the `.dmg` and the M1–M4
   checklist sections.
4. **Tell the user.**
