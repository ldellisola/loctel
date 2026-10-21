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

A Cargo workspace (`crates/*` plus `xtask`). The core (model, store, ingest,
MCP, engine) has no UI or graphics dependency and builds and tests on Linux
alone. The GPUI layer is thin. macOS-only code lives in its own small crate.
Which crates exist today, what each owns and how data flows between them:
[`docs/architecture.md`](docs/architecture.md).

## Commands

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

Run the app headless (no window; prints one JSON line per listener and runs
until SIGINT or SIGTERM). Port 0 picks a free port, and the lines say which.
Point `--settings` at a scratch file so you never touch the real one:

```bash
cargo run -p loctel-app -- --headless --grpc-port 0 --http-port 0 --mcp-port 0 \
  --settings /tmp/loctel-settings.toml
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

See the app, and the screenshot tests. Without `--headless`, Linux opens the
status window and prints the same listener lines (with no display it says so
and runs headless), so the real window runs under Xvfb. The first command
writes `target/run-shot.png`; open it with the Read tool. The screenshot tests
run with the rest of `cargo test --workspace`; the second command rewrites
their reference PNGs, which you must view before committing (the repo skills
`run-and-screenshot` and `update-screenshots` have the steps, and
[`docs/areas/ui.md`](docs/areas/ui.md) has the theme and the harness):

```bash
cargo xtask run-shot
cargo xtask screenshots --update
```

The end-to-end tests (real OTLP in, a real MCP client out, driven by the same
fake mesh) run with the rest of `cargo test --workspace`, and alone with
`cargo test -p loctel-e2e` (a few seconds). Read the "End-to-end tests" section
of [`docs/areas/testing.md`](docs/areas/testing.md) before adding one.

Commits use the conventional style (`feat(store): ...`, `fix(mcp): ...`,
`docs: ...`, `chore: ...`).

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
