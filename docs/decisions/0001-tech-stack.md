# 0001: Technology stack

Date: 2026-10-21
Status: accepted

## Context

loctel is a native macOS menu bar app, built and maintained only by AI agents.
The design (spec §11) fixed the technology up front so later sessions do not
relitigate it. Versions are as of 2026-10-21, and newer patch versions are
fine. This record summarises that section and the reasons the rest of the
repository depends on.

## Decision

| Concern | Choice |
|---|---|
| Language | Rust, toolchain pinned to 1.98.1 in `rust-toolchain.toml` (what the chosen GPUI snapshot needs) |
| UI framework | GPUI (Zed's framework) through the weekly `gpui-pre` snapshot, pinned to an exact version (`gpui-pre`, `gpui-pre-platform`, `gpui-pre-wgpu` `=0.3.9`, which matches zed@93d7456 and includes `HeadlessAppContext::capture_screenshot`). Fallback: a pinned Zed git revision |
| UI components | GPUI Kit (`longbridge/gpui-kit` 0.8.0): virtualized DataTable and VirtualList, Tree, Input, Tabs, Resizable, Sidebar. It pins `gpui-pre`, so the two upgrade together |
| Liquid Glass | `NSGlassEffectView` via `objc2`, under GPUI's view in a transparent window, frames synced to the sidebar and toolbar regions, click-through. Fallback if glass proves unworkable: GPUI's `Blurred` window background |
| Menu bar | `gpui-tray` (NSStatusItem), GPUI `ActivationPolicy::Accessory` (Regular while the window is open), `QuitMode` explicit |
| Start at login | `SMAppService.mainApp` via `objc2-service-management` |
| OTLP | `opentelemetry-proto` 0.33.1 (`gen-tonic`, `trace`, `logs`, `with-serde`), `tonic` 0.14 (server, gzip), `axum` 0.8 plus `tower-http` decompression |
| MCP | `rmcp` 3.6, `StreamableHttpService` in axum, spec 2026-07-28, read-only |
| Query languages | Own parsers and evaluators from Tempo's and Loki's grammars (Milestone 2) |
| Storage | Custom segment store; in memory first (design §5) |
| Packaging | `cargo-bundle` or `cargo-packager` with `LSUIElement`, then `hdiutil` for the DMG |
| Minimum OS | macOS 26 on Apple Silicon. Linux x86_64 for development and tests only |

CI runs on `ubuntu-24.04` for checks. The `macos-26` runner is used only by
the release job on pushes to main, never on pull requests (design §10.3).

## Consequences

- The core (model, store, ingest, MCP, engine) has no UI or graphics
  dependency and builds and tests on Linux alone. The UI is a thin layer, and
  macOS-only code stays small and isolated in `loctel-macos`.
- GPUI is pre-1.0 and its API breaks often, and `gpui-pre` is published by a
  third party. Upgrading the pinned snapshot is deliberate, one upgrade per
  PR, guided by the screenshot tests. If `gpui-pre` stops, switch to a pinned
  Zed git revision.
- Glass integration is fragile (no upstream support, a known startup deadlock
  when AppKit calls back into GPUI under a window lock). It stays isolated
  with the `Blurred` fallback ready.
- Mac-only code is invisible to Linux tests, so it counts as unverified until
  a milestone check on a real Mac (design §10.4).
- Later decisions that change anything above get their own numbered file in
  this directory, not an edit to this one.
