# AGENTS.md

Read this first. loctel is built and maintained only by AI agents. Every
feature or fix starts as a fresh chat with no memory, so this repository is
the memory: the docs, the decision log and the tests say what is true.

## What loctel is

loctel is a native macOS menu bar app that receives OpenTelemetry traces and
logs from services on the developer's machine, shows them in a fast window
(service map, traces, logs), and exposes the same data to AI agents over MCP.
The moment it exists for: a request fails somewhere in the mesh, and you need
to know which service broke and what the exception was.

The full design is in
[`docs/superpowers/specs/2026-10-21-loctel-design.md`](docs/superpowers/specs/2026-10-21-loctel-design.md).
Read the sections that touch your task before you change anything. The current
milestone plan lives in `docs/superpowers/plans/`, and the milestone table is
in [`docs/milestones/roadmap.md`](docs/milestones/roadmap.md).

## Crate map

A Cargo workspace (`crates/*` plus `xtask`). The core (model, store, query,
rules, ingest, MCP, engine) has no UI or graphics dependency and builds and
tests on Linux alone. `loctel-query` holds the TraceQL and LogQL parsers and
evaluators and `loctel-rules` the discard rules that use them at ingest. The
GPUI layer is thin. macOS-only code lives in its own small crate. Which crates
exist today, what each owns and how data flows between them:
[`docs/architecture.md`](docs/architecture.md).

## Commands

All from the repository root. The sections below say what each one is for.

| To | Run |
|---|---|
| set up a Linux machine (once; idempotent) | `bash script/setup-linux.sh` |
| run the quality gate | `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` |
| run loctel with no window | `cargo run -p loctel-app -- --headless --grpc-port 0 --http-port 0 --mcp-port 0 --settings /tmp/loctel-settings.toml --rules /tmp/loctel-rules.toml` |
| send it fake traffic (meshgen) | `cargo run -p loctel-meshgen -- --scenario checkout-failure --transport http-json --endpoint http://127.0.0.1:4318` |
| see the main window, with a failing checkout in it | `cargo xtask run-shot`, then open `target/run-shot.png` |
| run the main window on a virtual display (to drive it yourself) | `xvfb-run -a -s "-screen 0 1440x900x24" cargo run -p loctel-app -- --grpc-port 0 --http-port 0 --mcp-port 0 --settings /tmp/loctel-settings.toml --rules /tmp/loctel-rules.toml` |
| rewrite the screenshot references | `cargo xtask screenshots --update` (view every changed PNG) |
| check the macOS code from Linux | `bash script/check-macos.sh --all-targets` |
| run only the end-to-end tests | `cargo test -p loctel-e2e` |
| see the next release's version | `cargo xtask next-version` |
| see the next release's notes | `cargo xtask release-notes` |
| check a running loctel's MCP server | `cargo xtask mcp-smoke --url http://127.0.0.1:4319/mcp` |
| lint the workflows and scripts | `actionlint .github/workflows/*.yml` and `shellcheck script/*.sh` (both silent when clean; the setup script installs shellcheck, and actionlint 1.7.12 or newer comes from `go install github.com/rhysd/actionlint/cmd/actionlint@latest`) |

`cargo xtask <command>` is `cargo run -p xtask -- <command>` (an alias in
`.cargo/config.toml`); `cargo xtask` with no command prints the list of them.

Setup (Linux, idempotent; also runs from the Claude Code cloud SessionStart
hook). It installs the apt packages, the toolchain pinned in
`rust-toolchain.toml` and the `aarch64-apple-darwin` target:

```bash
bash script/setup-linux.sh
```

The quality gate, which is the only quality gate. All three must pass before
anything merges:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Run the app headless (no window; prints one JSON line per listener and one for
the discard rules, and runs until SIGINT or SIGTERM). Port 0 picks a free port,
and the lines say which. Point `--settings` and `--rules` at scratch files so you
never touch the real ones (`--rules` defaults to `rules.toml` beside the settings
file, and a new rules file holds no rules: nothing is discarded unless you write
a rule, see [`docs/areas/rules.md`](docs/areas/rules.md)):

```bash
cargo run -p loctel-app -- --headless --grpc-port 0 --http-port 0 --mcp-port 0 \
  --settings /tmp/loctel-settings.toml --rules /tmp/loctel-rules.toml
```

The run also serves MCP (the `mcp` line it prints has the address; the default
is `http://127.0.0.1:4319/mcp`). To point Claude Code at it, and for the tools
and their limits, see [`docs/areas/mcp.md`](docs/areas/mcp.md):

```bash
claude mcp add --transport http loctel http://127.0.0.1:4319/mcp
```

Send fake traffic (the fake microservice mesh; see
[`docs/areas/testing.md`](docs/areas/testing.md) for the scenarios). With the
app running headless on its default ports (the command above without the port
flags), this sends the failing checkout trace and prints
`sent 15 spans and 7 logs (trace ...)`:

```bash
cargo run -p loctel-meshgen -- --scenario checkout-failure \
  --transport http-json --endpoint http://127.0.0.1:4318
```

`--scenario` is one of `checkout-failure`, `checkout-success`, `health-checks`,
`late-out-of-order`, `huge-trace` and `log-flood`; `--transport` is `grpc`,
`http-protobuf` or `http-json` (`--endpoint` is the gRPC address, or the HTTP
base URL); `--repeat N --interval-ms M` sends it again and again with fresh IDs
(a steady stream), `--seed` fixes the IDs and `--size` scales the big
scenarios. `cargo run -p loctel-meshgen -- --help` has them all.

See the app, and the screenshot tests. Without `--headless`, Linux opens the
main window and prints the same startup lines (with no display it says so
and runs headless), so the real window runs under Xvfb. The first command
sends it meshgen's failing checkout trace and writes `target/run-shot.png`;
open it with the Read tool. The screenshot tests
run with the rest of `cargo test --workspace`; the second command rewrites
their reference PNGs, which you must view before committing (the repo skills
`run-and-screenshot` and `update-screenshots` have the steps, and
[`docs/areas/ui.md`](docs/areas/ui.md) has the theme and the harness):

```bash
cargo xtask run-shot
cargo xtask screenshots --update
```

To drive the main window yourself (another scenario, a steady stream, a
capture at a moment you choose), start it on a virtual display with the
command in the table. It prints the listener lines like headless mode, so the
ports are in them, and it runs until SIGTERM (or until the window closes). Send
meshgen to the OTLP HTTP address, wait about 2 seconds (the window reads the
store every 500 ms) and capture the screen with `import -window root out.png`;
the `run-and-screenshot` skill has the whole script. The main window has two
screens: the home screen (the service map over the trace table) and the trace
view, which a click on a table row opens. [`docs/areas/ui.md`](docs/areas/ui.md)
has the map of them, and how to add a screen.

The macOS code (`crates/loctel-macos`, and the macOS half of `crates/loctel-app`)
is compiled out of the Linux build, so the gate never sees it. Type-check and
lint it for the Mac from Linux with the command below (`ci.yml` runs it too).
Nothing can run it here: what it does on a Mac stays "unverified" until the
milestone check, and [`docs/areas/macos.md`](docs/areas/macos.md) lists what:

```bash
bash script/check-macos.sh --all-targets
```

The end-to-end tests (real OTLP in, a real MCP client out, driven by the same
fake mesh) run with the rest of `cargo test --workspace`, and alone with
`cargo test -p loctel-e2e` (a few seconds). Read the "End-to-end tests" section
of [`docs/areas/testing.md`](docs/areas/testing.md) before adding one.

The release pipeline (what the three `xtask` release commands are for, and the
bundle script that only runs on a Mac) is in
[`docs/areas/release.md`](docs/areas/release.md). Run `cargo test -p xtask` after
touching `xtask/`, and `actionlint` and `shellcheck` after touching a workflow
or a script.

Commits use the conventional style (`feat(store): ...`, `fix(mcp): ...`,
`docs: ...`, `chore: ...`).

## Skills

Step-by-step guides for the jobs that recur, in `.claude/skills/`. Read the one
that matches before you start:

| Skill | Use it when |
|---|---|
| `run-and-screenshot` | you need to see loctel run: headless with meshgen data, or the real window under Xvfb with a PNG |
| `update-screenshots` | a view's pixels change on purpose, you add a screenshot test, or one fails locally or in CI |
| `add-mcp-tool` | an agent-facing capability changes: a new tool, argument or result field |
| `add-query-feature` | TraceQL or LogQL gains syntax or semantics, a feature leaves the "not supported yet" list, or a query pasted from Grafana behaves differently from Grafana |
| `upgrade-gpui` | the pinned GPUI snapshot (`gpui-pre`) or GPUI Kit has to move: one upgrade per pull request, with the screenshots as the guide |
| `investigate-ci` | CI is red (the gate on a PR, a screenshot diff, or the release job after a merge): find the failing step, reproduce it, fix the cause |

## How a change ships

Every change, however small, goes the same way (design §10.3). Main is
protected, so only a green pull request merges, and no human approval is
needed.

1. **Branch** from `main`. Never commit to `main`.
2. **Change it** test first, with the docs and the decision log in the same
   change (house rules below). Run the quality gate locally, and
   `bash script/check-macos.sh --all-targets` if you touched macOS code.
3. **Open a pull request.** Wait for `ci.yml` (the Linux gate) to go green. If
   it is red, use the `investigate-ci` skill, fix the cause and push again.
   Never merge on red, and never loosen a test to get there.
4. **Squash-merge** it. The squash commit's subject is the pull request title
   and becomes a line in the release notes, so make it a good conventional
   commit message.
5. **Watch the release job.** The merge starts `release.yml` on `macos-26`: it
   builds the `.app` and the unsigned `.dmg`, smoke-tests it and publishes a
   GitHub release. It is slow. You wait for it, because nobody else is
   watching: if it fails, fix it in a follow-up pull request (steps 1 to 5
   again) until a release exists.

A milestone's last release also carries the "Check on your Mac" list
(`docs/milestones/mac-checklist.md`). The user tries it and reports back in a
new chat; until then everything Mac-only counts as unverified.

## House rules

- Never merge on red.
- Never skip, disable or loosen a test to get green.
- Update docs and the decision log in the same PR as the change.
- The merging agent watches the release job. If it fails, fix it before
  finishing, because nobody else is watching.
- Mac-only features stay "unverified" until a milestone check on a real Mac
  confirms them. Say so in the docs; do not treat them as proven.

Also keep these in mind:

- Decisions and their reasons go in `docs/decisions/` as numbered files
  (`NNNN-short-name.md`), so later agents do not undo choices they do not
  understand.
- A change that adds a crate also updates `docs/architecture.md`.
