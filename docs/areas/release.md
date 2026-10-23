# Release (`release.yml`, `script/bundle-mac.sh`, `xtask`)

How a merge to main becomes an installable `.dmg`: the version, the notes, the
bundle, the signature and the Gatekeeper steps for whoever installs it. Design
references: §10.3 (the release job, versioning, ad-hoc signing, the `.dmg`
layout, the install steps) and §10.4 (the "Check on your Mac" list).

Status: the pieces that run on Linux are tested (`cargo test -p xtask`, and
`cargo test -p loctel-e2e --test release_fixture` for the smoke-test input).
**The workflow itself has never run on `macos-26`**, and `bundle-mac.sh` has
never run on a Mac. See "Unverified until the first release run" below. The
agent that merges is the one who finds out, and fixes it (house rule).

## What happens on every merge to main

`.github/workflows/release.yml` triggers on `push` to `main` and nothing else.
It is the only workflow that uses the `macos-26` runner: pull requests never
get one (`ci.yml` checks the macOS code from Linux instead, see
[`macos.md`](macos.md)). It has `permissions: contents: write`, and
`concurrency: release` with `cancel-in-progress: false`, so two merges close
together release one after the other and never cancel each other.

| # | Step | Does |
|---|---|---|
| 1 | checkout, `fetch-depth: 0` | the whole history and every tag: the version and the notes come from the previous tag |
| 2 | `rustup toolchain install` | installs the toolchain, components and target of `rust-toolchain.toml` |
| 3 | `Swatinem/rust-cache@v2` | |
| 4 | Metal compiler check | GPUI's build script runs `xcrun metal`. Xcode 26 no longer ships it with the base install, so if `xcrun -sdk macosx metal --version` fails the step runs `xcodebuild -downloadComponent MetalToolchain`. A no-op when the image has it |
| 5 | version and notes | `VERSION=$(cargo xtask next-version)` goes to `$GITHUB_ENV`, `cargo xtask release-notes > notes.md`. Before the bundle (which needs the version) and before the new tag exists (so the notes cover everything since the previous release) |
| 6 | `cargo build --release -p loctel-app` | |
| 7 | `cargo install cargo-bundle --version 0.12.0 --locked` | |
| 8 | `script/bundle-mac.sh "$VERSION"` | `loctel.app` and `loctel-$VERSION.dmg` in the repository root (below) |
| 9 | smoke test | starts `loctel.app/Contents/MacOS/loctel --headless` on ports 24317, 24318 and 24319; posts `crates/loctel-e2e/fixtures/one-span.json` to `/v1/traces` (retrying for ten seconds while it starts); runs `cargo xtask mcp-smoke --url http://127.0.0.1:24319/mcp`; then `kill` and `wait`, which fails the step unless loctel exits 0 on SIGTERM |
| 10 | `gh release create "$VERSION" loctel-$VERSION.dmg --target "$GITHUB_SHA" --title "loctel $VERSION" --notes-file notes.md` | with `GH_TOKEN` from `github.token`. `--target` makes the new tag point at the commit that was built: without it `gh` tags the head of the default branch, which a later merge may already have moved |

A merge that only changes docs releases too: "every merge to main" is the
rule, and the version is cheap.

## Version

`cargo xtask next-version` prints it. The rule (`xtask/src/version.rs`): take
every tag that is exactly `vX.Y.Z`, find the highest one **numerically**
(`v0.1.10` beats `v0.1.9`), and add one to its patch. With no such tag the
first release is `v0.1.0`. Other tags (`v1.2`, `v1.2.3-rc1`, `latest`) are
ignored.

Merges bump the patch. To bump the minor or major version, push the tag by
hand (`git tag v0.2.0 <commit> && git push origin v0.2.0`) before the merge
that should continue from it: the next release is then `v0.2.1`.

## Notes

`cargo xtask release-notes` prints them (`xtask/src/notes.rs`):

1. **Changes**: the subject line of every commit since the previous version
   tag (all commits when there is none), newest first, one bullet each. With
   squash merges (the "how a change ships" loop in `AGENTS.md`) each subject is
   a pull request title ending in `(#N)`, so this is the list of PRs merged.
2. **Check on your Mac**: [`docs/milestones/mac-checklist.md`](../milestones/mac-checklist.md),
   inserted word for word (design §10.4). The file therefore has no title of
   its own. It is the current milestone's list: each milestone's plan has a
   task that writes it (Milestone 1: Task 13), and the roadmap's "Mac
   checklist highlights" column is its summary.
3. **Installing**: the steps below.

## The bundle

`script/bundle-mac.sh vX.Y.Z` (macOS only, `set -euo pipefail`, shellcheck
clean) does, from the repository root:

1. `cargo bundle --release --format osx -p loctel-app` with cargo-bundle 0.12.0.
   `--format osx` is the `.app` alone: on a Mac cargo-bundle also makes a `.dmg`
   by default. What goes into the bundle is
   `[package.metadata.bundle]` in `crates/loctel-app/Cargo.toml`. cargo-bundle
   0.12.0 takes the macOS-only keys in a `[package.metadata.bundle.macos]`
   table, without the `osx_` prefix of its older flat names (decision
   [0010](../decisions/0010-release-pipeline-choices.md)):
   - name `loctel` and identifier `io.github.ldellisola.loctel`;
   - `minimum_system_version = "26.0"` (`LSMinimumSystemVersion`);
   - `icon`, `crates/loctel-app/bundle/AppIcon.svg`: the three-node glyph of
     the menu bar item on a dark rounded square, in the UI's dark colours.
     cargo-bundle renders it to the `.icns` at every size;
   - `info_plist_exts`, `crates/loctel-app/bundle/info-plist-ext.xml`: bare
     Info.plist entries that cargo-bundle copies in as they are (so the file
     has no comments). It holds `LSUIElement = true`, which makes loctel an
     accessory app: no Dock icon and no flash of one at launch (the
     `ActivationPolicy::Accessory` in `main.rs` only applies once the app is
     running; while the main window is open the app switches itself to a regular
     one, see `macos.md`).
   cargo-bundle writes `CFBundleExecutable` (`loctel`, the `[[bin]]` name)
   itself, and puts the binary at `loctel.app/Contents/MacOS/loctel`. The
   result is copied from `target/release/bundle/osx/` to `./loctel.app`.
2. Sets `CFBundleShortVersionString` to the version without the `v`
   (`PlistBuddy`). `CFBundleVersion` stays the build number cargo-bundle
   writes (a timestamp). Then `plutil -lint`, and a check that `LSUIElement`
   is really `true` in the finished plist.
3. `codesign --force --deep --sign - loctel.app`, then `codesign --verify
   --deep --strict`. The signature is ad hoc, with no certificate, like Zed's
   builds: arm64 code must carry one to run at all, and a broken one shows up
   as "damaged".
4. Stages `loctel.app` plus a symlink `Applications -> /Applications` in a
   temporary folder and runs
   `hdiutil create -volname loctel -srcfolder <stage> -ov -format UDZO loctel-$VERSION.dmg`.

`loctel.app`, `loctel-v*.dmg` and `notes.md` are git-ignored.

## Installing (what the release notes tell the user)

The app is not notarized and its signature is ad hoc, so Gatekeeper stops the
first launch of **each version** (every download is a newly quarantined file).

1. Open the `.dmg` and drag `loctel` onto the `Applications` shortcut.
2. Open `loctel`. macOS says it cannot verify the app. Close the message.
3. System Settings, Privacy & Security, scroll to Security, find the message
   that `loctel` was blocked, click **Open Anyway** and confirm with a password
   or Touch ID. Open `loctel` again.
4. Repeat on the first launch of each new version.

Or, before the first launch:

```bash
xattr -dr com.apple.quarantine /Applications/loctel.app
```

## Checking it without a Mac

- `cargo test -p xtask`: the version rule, the notes (sections, order, the
  checklist carried over unchanged), the git glue against scratch repositories,
  and `mcp-smoke` against a real engine and against nothing listening.
- `cargo test -p loctel-e2e --test release_fixture`: `one-span.json` is a valid
  OTLP/JSON trace request that loctel stores. The job's `curl -f` only checks
  the status code and OTLP answers 200 even when it rejects the span
  (`partial_success`), so this test is what keeps the fixture honest.
- `actionlint .github/workflows/*.yml` and `shellcheck script/*.sh`, both
  silent. Use a current actionlint: 1.7.12 knows the `macos-26` label, 1.7.7
  does not.
- `cargo xtask next-version` and `cargo xtask release-notes` on any checkout
  with its tags: the same commands the job runs.
- `bash script/check-macos.sh --all-targets`: the macOS code type-checks and
  lints (see [`macos.md`](macos.md)).
- **A dry run of the bundle metadata.** cargo-bundle's `osx` format also runs
  on Linux, so after `cargo install cargo-bundle --version 0.12.0 --locked`
  (about 1 GB of build output), from the repository root,
  `cargo bundle --format osx -p loctel-app --binary-path target/debug/loctel`
  makes `target/debug/bundle/osx/loctel.app`: not a Mac executable, but a real
  `Info.plist` (check `LSUIElement`, `LSMinimumSystemVersion`,
  `CFBundleExecutable`) and a real `.icns` to look at. Change the metadata
  only with this check, since a wrong key name fails the whole release job.

## Unverified until the first release run

Written from the documentation of each tool and from running what Linux can
run. Nothing below has run on a Mac or on `macos-26`:

- the runner has Xcode 26 and an Apple Silicon CPU, `rustup` and `gh`, and the
  Metal toolchain check/download in step 4 works as written;
- `cargo bundle --release --format osx -p loctel-app` builds the binary
  itself and signs nothing (the metadata, the paths, the `Info.plist` it writes
  and the `.icns` were checked in the Linux dry run above; the build and the
  copy into `target/release/bundle/osx/` were not);
- `PlistBuddy`, `plutil`, `codesign`, `ditto` and `hdiutil` behave as the
  script assumes, and the ad-hoc signature survives the copy into the `.dmg`;
- the bundled loctel starts headless from the bundle and exits 0 on SIGTERM;
- (since Milestone 5) `zstd-sys` compiles its C sources with the runner's own clang
  (`script/check-macos.sh` only hands it a stand-in, so the C has never been built for
  the Mac before this job), and the smoke test's loctel makes and deletes its run
  directory under the runner's `$TMPDIR/loctel`;
- `GITHUB_TOKEN` is allowed to create releases and tags in this repository
  (the repository setting "Workflow permissions" must not force read-only: the
  workflow asks for `contents: write` itself, which an organisation policy can
  still override).

If the release job fails, read the log of the failing step first
([`investigate-ci`](../../.claude/skills/investigate-ci/SKILL.md)), reproduce
what Linux can, fix it in a pull request, and watch the next run. The notes
and version come out right for the retry as long as no tag was created: the
tag is made by the last step, so a job that failed earlier left none.
