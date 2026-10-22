# 0035: Live port rebinding and the app backend

Date: 2026-10-22
Status: accepted

## Context

Milestone 4 Task 2 makes the ports editable in a Settings window: changing one
must move that listener at once, with no restart, and the window must read and
write everything it needs (rules, settings, pause, login item) through one
backend. The plan fixes the interfaces (`EngineHandle::rebind(Ports)`,
`EngineHandle::rules_generation()`, the new `UiBackend` methods, `LoginItem`,
`AppBackend`) and says what a rebind does ("for each listener whose port
changed, stop it and bind the new one; a bind that fails shows `Failed`; the
others stay"). The code it builds on does not allow that as it stood, and a few
details were left open.

## Decision

- **`loctel-ingest` starts one receiver at a time.** `start` bound both receivers
  under one stop signal, so one port could not move without stopping the other.
  `start_grpc` and `start_http` (each returning a `ListenerStatus` and a
  `ReceiverShutdown`) bind one receiver with a stop signal of its own, and
  `start` is the same code over a shared signal, so its behaviour and its tests
  are unchanged. The engine starts its receivers this way at launch too, so that
  there is one path. This is an addition to the ingest crate's API that the plan
  did not list (it names only the engine); it is the minimum that lets the plan's
  rebind exist, and the alternative (stopping and restarting both receivers) would
  drop in-flight requests of the one that did not move.
- **The engine keeps a stop handle per listener, and the statuses behind a
  lock.** `State::listeners` is a `parking_lot::Mutex` instead of two statuses and
  a `OnceLock`, so `EngineHandle::status()` and the MCP `get_status` still read
  one place and cannot disagree after a rebind. MCP's own status is `None` for
  the instant between the engine's start and its bind, as before.
- **"Changed" is compared with the port each listener was asked for**, not the
  one it bound. A listener started on port 0 has `requested` port 0, so
  `rebind(Ports { 0, 0, 0 })` changes nothing and `rebind` to a real port moves it.
  A listener that is `Failed` and asked for the same port again is **not** retried:
  it is retried when its port changes (the plan says "whose port changed"). Retry
  on the same port is a later feature if the Settings window wants an Apply that
  does it.
- **Stop first, then bind, and wait a moment for a port another listener just
  left.** A stopped server lets go of its socket a moment later (the shutdown is
  asynchronous), so a rebind that gives one listener the port another is leaving
  (a swap, or HTTP to MCP's old port while MCP moves on) would fail on the first
  try. A failed bind of a port that one of the moving listeners held is retried
  every 20 ms for up to a second (`RELEASE_WAIT`, `RELEASE_POLL`); any other
  failure stands at once, so a port that is really in use costs nothing extra. The
  test `rebind_can_take_the_port_another_listener_just_left` fails on most runs
  with the retry switched off.
- **`rebind` is a plain blocking call that works from any thread.** The binds run
  on the engine's runtime (`spawn`) and the caller waits on a channel, because
  `Handle::block_on` panics inside a runtime and the window may call it from
  anywhere (the same reasoning as decision 0004 for `shutdown`). It holds the
  engine's `running` lock for the whole time, so two rebinds, or a rebind and the
  shutdown, never interleave. After a shutdown it changes nothing and returns the
  last statuses.
- **The rules generation is a counter that goes up after the replace.** It is
  incremented by `set_rules`, `set_rule_enabled` and a successful reload of an
  edited file (not by a file that could not be used, which replaces nothing), and
  only once the new rules are in force, so whoever sees the new number reads the
  new rules. The editor (Task 5) compares it with the one it saw when it opened
  the rule.
- **`LoginItem` and `AppBackend` live in `loctel-ui`** (`app/app_backend.rs`).
  `AppBackend::new(engine, settings_path, login)` replaces `impl UiBackend for
  EngineHandle`, which is removed. `settings()` reads `settings.toml`, not the
  engine's own settings: a command-line `--http-port 0` changes what the engine
  listens on, not the file, and a form that showed `0` could not be applied.
  `apply_settings` saves first and only then rebinds, so a file that cannot be
  written moves nothing, and a port that cannot be bound is still saved.
- **`FixedBackend` keeps all of it in memory** (a `RuleSet`, the settings, the
  pause, `pub login`), and `status()` is its `status` field with those laid over
  it, so existing tests that read the field or compare the status keep working.
  The one earlier test that implemented `UiBackend` by hand
  (`the_status_box_follows_the_engine`) got the new methods, delegating to a
  `FixedBackend`; its assertions are unchanged.
- **`loctel-macos` builds the backend with the system's login item.**
  `window::open_or_focus` takes the settings path (`loctel-app` and the status
  item pass it) and constructs `AppBackend` with
  `login_item::SystemLoginItem`, a thin `LoginItem` over the existing
  `login_item::is_enabled` and `set_enabled`. Nothing of this has run on a Mac
  (`docs/areas/macos.md`).

## Consequences

- A later change that moves a listener goes through `EngineHandle::rebind`; do
  not restart the engine for a port change.
- Anything that implements `UiBackend` must implement the whole trait (it has no
  default methods on purpose, so a new backend cannot forget the rules or the
  settings); `FixedBackend` is the one to copy from.
- A failed listener stays failed until its port is changed. If Settings should
  retry on Apply with the same port, that is a change to `rebind` (retry
  `Failed` listeners too) and to its test `rebind_unchanged_is_noop`.
