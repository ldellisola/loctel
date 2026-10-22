# Roadmap

The whole build, by milestone. Each milestone gets its own detailed plan file
under `docs/superpowers/plans/` when it starts, written from the
[design](../superpowers/specs/2026-10-21-loctel-design.md) and this roadmap.
Each one ends with a release whose notes carry a "Check on your Mac" list
(design §10.4). Until a milestone check confirms them, Mac-only features count
as unverified.

| # | Milestone | Delivers | Mac checklist highlights |
|---|---|---|---|
| **M1** | **Headless core, MCP, menu bar shell, releases** ([plan](../superpowers/plans/2026-10-21-loctel-m1-core.md)) | ingest, assembly, failure semantics, service map data, MCP tools, meshgen, e2e tests, screenshot harness, status item menu, login item, CI, `.dmg` releases | install via Open Anyway; menu bar icon, no Dock icon; Docker container and bare-metal service both reach loctel; Claude Code connects to MCP and answers "why did my last request fail"; start at login |
| **M2** | **Query languages and discard rules** ([plan](../superpowers/plans/2026-10-21-loctel-m2-query-rules.md)) | TraceQL and LogQL parsers and evaluators (§6); rules engine at ingest with persistence and preview (§7); MCP `query` arguments; rule counts in status | a health-check rule silences real health checks; a TraceQL structural query from Grafana works unchanged |
| **M3** | **Main window: Map & traces, Trace view** ([plan](../superpowers/plans/2026-10-22-loctel-m3-main-window.md)) | GPUI Kit UI per Home A and Trace mockups (§9.2): the live service map over the trace table with TraceQL search, the trace view (waterfall, inspector, span logs), glass sidebar and toolbar (§9.3), light and dark themes following the system, Open loctel from the menu bar with Dock/⌘-Tab switching while the window is open, the toolbar's Live indicator, time range, Pause and Clear all, screenshot tests in both themes | glass looks right; window opens fast; Dock/⌘-Tab behaviour; light/dark follows the system; live map and table; Pause freezes the table and range changes it; the big trace scrolls smoothly |
| M4 | Logs, Discard rules screen, popover, Settings | remaining screens (§9.2); popover under the icon; settings UI | popover anchoring; settings changes take effect |
| M5 | Scale | spill-to-disk segment store and 4 GB cap (§5); performance budgets as CI tests (§10.3) | smooth scrolling of the 100k-span trace and 10M logs; startup times |
