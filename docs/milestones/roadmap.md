# Roadmap

The whole build, by milestone. Each milestone gets its own detailed plan file
under `docs/superpowers/plans/` when it starts, written from the
[design](../superpowers/specs/2026-10-21-loctel-design.md) and this roadmap.
Each one ends with a release whose notes carry a "Check on your Mac" list
(design §10.4). Until a milestone check confirms them, Mac-only features count
as unverified.

**Status: all five milestones are delivered.** Everything on this table is built and
tested on Linux (the gate and the performance budgets), and a release's notes carry the
Mac checklist. What is left is the Mac side: [`mac-checklist.md`](mac-checklist.md) has one section per milestone, and
nothing in it has been confirmed on a Mac yet, so every Mac-only feature (the menu bar
icon and the popover, Liquid Glass, the login item, the Dock behaviour, and how Milestone
5's start, files and memory feel on Apple Silicon) is **unverified** until the user tries
the list and reports back in a new chat.

| # | Milestone | Delivers | Mac checklist highlights |
|---|---|---|---|
| **M1** | **Headless core, MCP, menu bar shell, releases** ([plan](../superpowers/plans/2026-10-21-loctel-m1-core.md)) | ingest, assembly, failure semantics, service map data, MCP tools, meshgen, e2e tests, screenshot harness, status item menu, login item, CI, `.dmg` releases | install via Open Anyway; menu bar icon, no Dock icon; Docker container and bare-metal service both reach loctel; Claude Code connects to MCP and answers "why did my last request fail"; start at login |
| **M2** | **Query languages and discard rules** ([plan](../superpowers/plans/2026-10-21-loctel-m2-query-rules.md)) | TraceQL and LogQL parsers and evaluators (§6); rules engine at ingest with persistence and preview (§7); MCP `query` arguments; rule counts in status | a health-check rule silences real health checks; a TraceQL structural query from Grafana works unchanged |
| **M3** | **Main window: Map & traces, Trace view** ([plan](../superpowers/plans/2026-10-22-loctel-m3-main-window.md)) | GPUI Kit UI per Home A and Trace mockups (§9.2): the live service map over the trace table with TraceQL search, the trace view (waterfall, inspector, span logs), glass sidebar and toolbar (§9.3), light and dark themes following the system, Open loctel from the menu bar with Dock/⌘-Tab switching while the window is open, the toolbar's Live indicator, time range, Pause and Clear all, screenshot tests in both themes | glass looks right; window opens fast; Dock/⌘-Tab behaviour; light/dark follows the system; live map and table; Pause freezes the table and range changes it; the big trace scrolls smoothly |
| **M4** | **Logs, Discard rules screen, popover, Settings** ([plan](../superpowers/plans/2026-10-22-loctel-m4-logs-rules-popover.md)) | the remaining screens (§9.2): Logs (LogQL search, level toggles, live tail, volume histogram, expandable lines that open their trace or the lines around them, "Discard lines like this…") and Discard rules (cards with switches and counts, an editor with live validation and the 5-minute preview); the sidebar gains both (⌘1 to ⌘3); the menu bar popover under the icon replaces the menu; a Settings window with the three ports (rebound live) and start at login | popover anchoring and glass, closing on click-away, ⌘Q and ⌘, in it; Logs with a live tail; Discard rules and their preview; Settings port changes take effect, and a port in use shows the failure; start at login agrees between the popover and Settings |
| **M5** | **Scale** ([plan](../superpowers/plans/2026-10-22-loctel-m5-scale.md)) | the store spills completed traces and sealed log chunks to temporary segment files and keeps them under a disk cap, 4 GB by default and set in Settings (§5, with the storage described end to end in [`areas/store.md`](../areas/store.md)); the performance budgets of §10.3 as a check, `cargo xtask perf`, that CI's `perf` job runs on every pull request; the Logs list scrolls through ten million lines; the popover's disk tile; both themes pass a contrast check | startup under half a second; the 100,000-span trace scrolls smoothly and opens as fast after it spilled; two million logs scroll without stalls and a LogQL query answers within a second; the disk tile and the cap in Settings, the run folder in `$TMPDIR` that goes on quit; memory stays bounded under a steady stream (it levels off, apart from about 15 bytes for every log kept, up to the 20 million lines the app holds); the light theme is readable everywhere |
