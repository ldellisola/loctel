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
