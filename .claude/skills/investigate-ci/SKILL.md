---
name: investigate-ci
description: "Find out why a loctel CI run failed and fix it. Use whenever the Linux gate (ci.yml) is red on a pull request, a screenshot test fails in CI but not locally, or the release job (release.yml, macos-26) fails after a merge to main. Covers finding the failing job and step, reproducing it locally with the same command, reading the screenshot-diffs artifact, and what can and cannot be reproduced for the macOS release job."
---

# Investigate a CI failure

Never merge on red and never loosen, skip or `#[ignore]` a test to get green.
Find the cause, fix the cause. There are two workflows:

- `.github/workflows/ci.yml`, job `gate` on `ubuntu-24.04`, for every pull
  request and for main. It is the quality gate.
- `.github/workflows/release.yml`, job `release` on `macos-26`, only on pushes
  to main. After you merge, **you** watch it: nobody else does.

## 1. Find the failing job and step

```bash
gh run list --branch <branch> --limit 5          # the runs; take the failed one's id
gh run view <run-id>                             # jobs and steps, the red one marked
gh run view <run-id> --log-failed | tail -100    # only the failed steps' logs
```

Without `gh`, the GitHub MCP tools do the same (`actions_list`, `actions_get`,
`get_job_logs`). Read the log from the **first** error up,
not the last line: later errors are usually fallout. Note which step failed;
it tells you which command to run.

## 2. Reproduce with the same command

The `gate` job is `script/setup-linux.sh` and then these steps, so run the one
that failed, from the repository root:

| Failed step | Run locally |
|---|---|
| Format | `cargo fmt --check` (fix with `cargo fmt`) |
| Clippy | `cargo clippy --workspace --all-targets -- -D warnings` |
| Type-check and lint the macOS code | `bash script/check-macos.sh --all-targets` |
| Test | `cargo test --workspace`, or `cargo test -p <crate> <test name>` for one |
| Set up Linux build and screenshot dependencies | `bash script/setup-linux.sh` |

If it passes locally and fails in CI, look for what differs: a test that
depends on time, ports, ordering or the machine (CI has 4 CPUs and no GPU,
like your container), a file that is not committed (`git status`), a toolchain
or lockfile difference (`rust-toolchain.toml`, `Cargo.lock`), or the macOS
check, which only CI and `check-macos.sh` run. Run the failing test in a loop
(`for i in $(seq 20); do cargo test -p <crate> <name> || break; done`) before
deciding it is not flaky, and fix flakiness at the cause (never a retry or a
longer sleep without a reason).

## 3. A screenshot test fails in CI

The gate uploads `target/screenshot-diffs/**` as the **screenshot-diffs**
artifact when it fails.

```bash
gh run download <run-id> -n screenshot-diffs -D /tmp/screenshot-diffs
```

Open `{name}.actual.png` and `{name}.diff.png` (red where pixels differ) with
the Read tool, and the reference in `crates/loctel-ui/tests/snapshots/`. Then
follow the `update-screenshots` skill: it says when a reference may change,
what to do when only anti-aliasing differs between your machine and CI, and
what a font or missing-GPU failure looks like.

## 4. The release job fails (after a merge to main)

Most of it cannot run on Linux, so read the log carefully; start from the
failing step of `release.yml` (the steps are listed in
[`docs/areas/release.md`](../../../docs/areas/release.md)).

| Failed step | What to check |
|---|---|
| Metal compiler check | the runner image changed; `xcodebuild -downloadComponent MetalToolchain` is the fix GPUI's build script needs |
| `cargo xtask next-version` / `release-notes` | `cargo test -p xtask`; both run on Linux with the same tags (`git fetch --tags`). The checkout must be `fetch-depth: 0` |
| `cargo build --release -p loctel-app` | a macOS-only compile error: `bash script/check-macos.sh --all-targets` reproduces type errors and lints from Linux. Link or build-script errors do not reproduce there |
| `script/bundle-mac.sh` | the log names the failing command (`cargo bundle`, `PlistBuddy`, `codesign`, `hdiutil`). `shellcheck script/*.sh` still runs on Linux; read `[package.metadata.bundle]` in `crates/loctel-app/Cargo.toml` and the two files in `crates/loctel-app/bundle/` |
| Smoke test | "exited before it answered": the bundled binary crashed on launch; "did not accept the trace": the fixture or the ports (`cargo test -p loctel-e2e --test release_fixture`); `mcp-smoke` fails: run `cargo xtask mcp-smoke --url ...` against a headless loctel on Linux; the step also fails if loctel does not exit 0 on SIGTERM |
| Publish the release | `GITHUB_TOKEN` permissions (`contents: write`), or the tag already exists (a manual tag with the same name) |

Fix it in a pull request like any change (the `gate` must be green; the fix
itself only shows on the next release run), merge, and watch the run until a
release exists. The tag is created by the last step, so a failure before it
leaves nothing to clean up. Record anything that was surprising in
`docs/decisions/` and update `docs/areas/release.md`.

## 5. Before you push the fix

Run the whole gate locally (`cargo fmt --check`, `cargo clippy --workspace
--all-targets -- -D warnings`, `cargo test --workspace`, and
`bash script/check-macos.sh --all-targets` if you touched macOS code), and for
a workflow change `actionlint .github/workflows/*.yml` (a current actionlint)
and `shellcheck script/*.sh`.
