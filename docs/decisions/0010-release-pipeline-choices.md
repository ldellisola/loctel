# 0010: Release pipeline choices

Date: 2026-10-21
Status: accepted

## Context

The Milestone 1 plan (Task 13) fixes the release workflow's shape, the three
`xtask` commands and the contents of the `.dmg`, and was written without running
any of the tools it names. Building it from Linux (where only part of it can
run) turned up one wrong third-party name and a handful of places where the
plan's text leaves a gap that would fail or mislead on the first real run.
These are the choices a later agent might be tempted to undo, and the facts
they rest on.

## Decision

- **cargo-bundle 0.12.0 takes the macOS keys in a `macos` table, with no
  `osx_` prefix.** The plan says `osx_minimum_system_version` and
  `osx_info_plist_exts`. Those are the old flat names; 0.12.0 parses
  `[package.metadata.bundle]` with `deny_unknown_fields` and fails with
  "unknown field `osx_info_plist_exts`". The crate's `OsxSettings` has
  `minimum_system_version` and `info_plist_exts`, in a table named `osx` with
  the alias `macos`, so `crates/loctel-app/Cargo.toml` has
  `[package.metadata.bundle.macos]`. The plan's own names for what is set
  (minimum system version 26.0, `LSUIElement = true`, the identifier, the name,
  the icon) are unchanged. Found by running cargo-bundle's `osx` format on
  Linux, which works because that bundler has no platform gate (see "Checking
  it without a Mac" in `docs/areas/release.md`).
- **`info_plist_exts` is a file of bare `<key>`/`<string>` pairs** that
  cargo-bundle pastes into the top-level dict of `Info.plist`
  (`crates/loctel-app/bundle/info-plist-ext.xml`, which holds `LSUIElement`).
  It has no XML comment, because the comment would be pasted into the plist
  too and nothing here can check that every Apple tool accepts one.
- **The icon is an SVG**, `crates/loctel-app/bundle/AppIcon.svg`, not a set of
  PNGs. cargo-bundle renders an SVG with resvg to every size of the `.icns`
  (16 to 512 at 1x and 2x), so no binary files are committed. It uses the same
  geometry as the menu bar glyph (`status-icon.svg`) in the UI's accent colour
  on a dark rounded square (824 px in a 1024 px canvas, the margin macOS icons
  keep).
- **cargo-bundle runs from the repository root with `-p loctel-app` and
  `--format osx`.** It resolves the `icon` and `info_plist_exts` paths against
  the working directory, hence the root-relative paths in `Cargo.toml`. On a
  Mac its default is an `.app` **and** a `.dmg`; `--format osx` asks for the
  `.app` alone, because `hdiutil` makes ours (the plan's layout, with the
  `Applications` symlink).
- **`script/bundle-mac.sh` also checks its own output**: `plutil -lint` on the
  edited plist, that `LSUIElement` is `true`, and `codesign --verify --deep
  --strict` after signing. A missing `LSUIElement` would otherwise only show as
  a Dock icon flashing on someone's Mac, and a broken signature as "damaged".
  It leaves `loctel.app` and `loctel-vX.Y.Z.dmg` in the repository root (both
  git-ignored) because the workflow's smoke test runs `loctel.app/...` by that
  relative path. `CFBundleVersion` stays cargo-bundle's build number; the
  plan sets only `CFBundleShortVersionString`.
- **`gh release create` gets `--target "$GITHUB_SHA"`.** Without it `gh` tags
  the head of the default branch when the tag does not exist. Two merges can
  be in flight (the release job is slow and queues), so the head can already
  be a later commit than the one that was built, and the tag would name code
  that is not in the `.dmg`.
- **The smoke test retries its first request** for ten seconds, and the job
  asks loctel to exit on SIGTERM and fails unless it exits 0. The plan's text
  posts once; starting a process and posting to it in the next line is a race.
  The shutdown check costs nothing and covers the "quit" half of the design's
  "launch, check the endpoints and MCP answer, quit".
- **The workflow has a step that fetches the Metal shader compiler when the
  runner lacks it** (`xcodebuild -downloadComponent MetalToolchain`). Xcode 26
  stopped shipping it with the base install, GPUI's build script
  (`gpui-pre-apple`) runs `xcrun metal` on a Mac, and this is the first time
  anything builds that crate on a Mac. It does nothing when the image has the
  compiler. Whether it is needed is unverified (`docs/areas/release.md`).
- **`xtask` is no longer standard-library only.** `mcp-smoke` needs a real MCP
  client, so `xtask` depends on rmcp's client, reqwest (no proxy, like the
  e2e harness) and tokio, with `loctel-engine` and `tempfile` for its tests.
  `next-version` and `release-notes` still use only `git` and the standard
  library. The cost is a longer first `cargo xtask` build.
- **The version rule ignores tags that are not exactly `vX.Y.Z`** (no
  pre-release suffix, no leading zeros, parts that fit in a `u32`), and the
  "previous release" for the notes is the highest of those, not the nearest
  tag in history. A hand-pushed `v0.2.0` therefore moves both the next version
  (`v0.2.1`) and the start of the notes.
- **`docs/milestones/mac-checklist.md` has no title.** `release_notes` inserts
  it word for word under its own "Check on your Mac" heading, so a heading in
  the file would appear twice. It starts with a paragraph that tells the
  reader what to do with the list.
- **`one-span.json` has its own test** (`loctel-e2e/tests/release_fixture.rs`).
  The smoke test posts it with `curl -f`, which only sees the status code, and
  OTLP answers 200 with a `partial_success` when it rejects the span.
  Verified by breaking the trace ID: the test fails with loctel's reason.

## Not decided here

- Whether the `macos-26` runner has the Metal compiler, `gh` and a recent
  rustup, and whether `GITHUB_TOKEN` may create releases here, are facts about
  GitHub that nobody can check from Linux. The first release run answers
  them. `docs/areas/release.md` lists them as unverified, and the agent that
  merges Milestone 1 fixes whatever the run finds.
- Notarization. The design accepts Open Anyway; a Developer ID certificate
  would replace the ad-hoc signature and the install steps.
