# 0004: Engine lifecycle, settings failures and the headless contract

Date: 2026-10-21
Status: accepted

## Context

The Milestone 1 plan (Task 7) fixes the engine's interfaces (`Settings`,
`EngineConfig`, `Engine::start`, `EngineHandle`, `EngineStatus`, `RateMeter`)
and the headless command line, and leaves the failure behaviour to the
builder. These are the choices that other code will lean on, and the ones a
later agent might be tempted to "simplify":

- `Engine::start` is a plain function that blocks on its own runtime. A GUI
  callback, a test harness or an async `main` could call it inside a tokio
  runtime, where `Runtime::block_on` panics.
- A runtime cannot be dropped, or shut down with a wait, inside another
  runtime. `shutdown` and the last handle's drop can happen anywhere.
- The settings file can be missing (first launch), unreadable or broken (hand
  edited). The plan gives `load_or_default`, which hides the difference.
- The headless binary is driven by scripts that read its stdout the moment the
  first line appears, and signal it right after.

## Decision

- **`Engine::start` refuses to run inside a tokio runtime** with an `Err` that
  says why. It never panics for that.
- **`shutdown` and drop never block inside a runtime.** `shutdown` is
  idempotent. From a thread with no runtime it waits up to 2 seconds for the
  engine's threads (so the sockets are closed when it returns). Inside a
  runtime it only asks them to stop. Dropping the last handle never waits.
  Requests in flight at shutdown may be cut off; exporters retry.
- **Settings:** `Settings::load` is strict (missing file is the defaults, an
  unreadable or invalid file is an `Err` naming the file) and
  `load_or_default`, the plan's function, is `load` with the defaults on any
  error. Neither rewrites a file it could not read: overwriting a
  hand-edited file with defaults would destroy the user's work. The
  headless binary calls `load` so it can say on stderr that it fell back. The
  `load` function is an addition to the plan's interface.
- **Unknown keys in `settings.toml` are ignored and every key is optional**, so
  a file written by a newer loctel still loads in an older one (and the other
  way round).
- **`Settings::default_path()` falls back to the temporary directory** when
  the machine has no config directory (`dirs::config_dir()` is `None`). loctel
  must still start; the settings just do not survive. On macOS this does not
  happen.
- **`Settings::save` writes whole then renames**, so a crash cannot leave half
  a file.
- **The rate meter is a ring of 64 buckets of 100 ms**, with the window being
  the 5 seconds before "now". Constant memory and time per record, safe for
  records arriving slightly out of order, and a clock that is set back
  replaces stale buckets instead of being ignored. The cost is a window edge
  blurred by up to 100 ms, which a status display does not show.
- **Headless installs its signal handlers before it prints the listener
  lines.** Those lines are the "ready" signal that tests and scripts wait for;
  a SIGTERM that arrived before the handlers existed would kill the process
  with a signal status instead of exiting 0.
- **Headless only reads settings.** It never writes `settings.toml`. First
  launch handling (`first_launch_done`) belongs to the macOS shell.
- **Stdout is for the listener lines only.** Warnings, the settings fallback
  message and library logs go to stderr, so a script can parse stdout line by
  line.
- **Small dependency additions to the plan's lists:** `loctel-model` for the
  engine (the `Sink` trait is spelled in model types), and `serde`,
  `serde_json` and `anyhow` for the app (listener lines are serialized with
  the field order the plan shows, and the error text must be escaped
  properly). `tokio` in the app adds the `signal` feature, and
  `tracing-subscriber` the `env-filter` feature so `RUST_LOG` works.

## Consequences

- A later task that wires MCP into `Engine::start` must keep the rules above:
  no blocking inside a runtime in `shutdown`, `mcp_url()` as the one place that
  knows the MCP address, and the `mcp` listener in `EngineStatus::listeners`.
- The headless line format is a contract for the release smoke test (and for
  any script that starts loctel with ports 0). Changing a key name breaks them;
  the unit tests in `crates/loctel-app/src/main.rs` pin the exact bytes.
  `loctel-e2e` does not read these lines: it starts the engine in-process (see
  `0007-e2e-harness-choices.md`).
- Pause drops data for good. If a "resume and replay" is ever wanted, it is a
  new feature with a buffer, not a bug in the sink.
