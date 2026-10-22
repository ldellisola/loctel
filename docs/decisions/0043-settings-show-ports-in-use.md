# 0043: The Settings window shows the ports in use

Date: 2026-10-22
Status: accepted (supersedes one point of decision 0039)

## Context

`AppBackend::settings()` first read only `settings.toml`, and
`EngineHandle::rebind` compared the wanted ports with the ports the engine was
*asked* for. With the command-line port flags that agents use (the docs' Xvfb
runs pass `--grpc-port`, `--http-port` and `--mcp-port`), the Settings window
showed the file's 4317, 4318 and 4319 as `listening` while the listeners were
somewhere else, and changing one port and pressing Apply moved all three. The
M4 whole-branch review found it. On a Mac the app starts without those flags, so
users never saw it, but it misled the agents who drive the window.

## Decision

- `AppBackend::settings()` lays the ports the listeners use over the file: the
  bound port of a listening one, the requested port of a failed one.
- `EngineHandle::rebind` leaves a listener alone when it already answers on the
  wanted port: either it was asked for that port, or it was asked for port 0 and
  the system gave it that one (`holds` in `crates/loctel-engine/src/lib.rs`).

## Consequences

- The form shows where loctel really listens, and Apply moves only the port
  that was edited.
- Apply saves the shown ports to `settings.toml`, so a run with port flags that
  presses Apply writes those ports to the file it was given. The docs' runs
  point `--settings` at a scratch file, so the real one is never touched.
- Tests: `settings_show_the_ports_in_use_and_apply_moves_only_the_changed_one`
  (`crates/loctel-ui/tests/app_backend.rs`) and
  `rebind_to_the_ports_a_port_zero_engine_was_given_is_noop`
  (`crates/loctel-engine/tests/rebind.rs`).
