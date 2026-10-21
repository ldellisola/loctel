# 0018: The engine's side of the discard rules

Date: 2026-10-21
Status: accepted

## Context

The Milestone 2 plan (Task 6) has the engine run `loctel-rules`: the sink
filters every batch, `rules.toml` is created, loaded, saved and reloaded, the
status carries the counts, the handle edits the rules and the app gets
`--rules PATH`. The plan fixes the interfaces. It leaves open where the rules
file is when `--settings` points at a scratch file, what a reload does with a
file that is gone or was written in the same instant, what a failed save leaves
behind, and which calls the sink makes. These are the choices a later agent might
be tempted to undo.

## Decision

- **`--rules` defaults to `rules.toml` in the directory of the settings file.**
  The plan says "defaulting to `RulesFile::default_path()`", and the design says
  rules persist "next to `settings.toml`". Without `--settings` the two are the
  same file (`RulesFile::default_path()` is the directory of
  `Settings::default_path()` plus `rules.toml`). With `--settings /tmp/x.toml` and
  no `--rules` the file is `/tmp/rules.toml` and not the user's real one:
  AGENTS.md tells agents to point `--settings` at a scratch file "so you never
  touch the real one", and unlike the settings (which headless mode only reads)
  the rules file is created and rewritten, so a default that ignored `--settings`
  would let every test run and every agent session write the real file, or read a
  real rule that drops the data the run is looking for. The headless tests pass
  `--rules` beside their scratch settings for the same reason.
- **The rules line is printed in every mode, not only headless.** The plan has
  headless mode print `{"event":"rules","path":...,"count":N,"error":null}`. The
  window on Linux and the menu bar app on macOS already print "the same lines as
  headless mode"; keeping that true is simpler than a second list of lines. The
  line comes after the three listener lines, and nothing reads stdout to the end.
  `count` is the number of rules in the file, enabled or not; `path` is `null`
  only for an engine with no path (never the app).
- **A rules file that cannot be used at start is "no rules and an error",
  and it is left alone.** That covers a file that cannot be read, one that does not
  parse, and a place where the empty file cannot be written (a folder that is
  read-only). The engine starts anyway and says so in `rules_file_error`. It never
  writes over a file it could not read: that is the user's rules, with a typo.
- **Reload compares the modification time and the length, not the time alone.**
  The plan polls the modification time. A coarse file system clock gives two
  writes made within a few milliseconds the same time (a test that saves through
  the handle and then edits by hand does exactly that), and a rule switched on or
  off changes the length. An edit that keeps both the same within one clock tick
  is still missed; the cost of closing that, a read of the file every 2 s, is not
  worth it for a file that people edit by hand. The engine's own writes record the
  stamp of what they wrote, so they never look like an edit.
- **A file that is gone is not a change.** The rules in memory stay, the error
  (if any) stays, and nothing is written. Writing the empty file on a gap would
  race with an editor that saves by renaming over the file (it leaves none for a
  moment) and could destroy what the editor is about to put there. The next change
  through the handle writes the file again, and a file that reappears is read. The
  cost is that deleting the file does not stop the rules that are running until
  the engine restarts; switching rules off is what the file and the handle are
  for.
- **The file is read on the blocking pool,** every `RULES_POLL`, by a task on the
  engine's runtime. The receivers' async threads are the ones that must not wait
  for a slow disk. A poll that panics ends that poll, not the watching.
- **`set_rules` and `set_rule_enabled` save first and then apply.** If the file
  cannot be written the call is an `Err` and the rules in memory do not change, so
  the rules in force are never ones the file does not have (a restart would
  silently drop them). A successful save clears the file error, since the file has
  just been written over. Both run under the lock the reload uses, so a reload
  cannot change the list between `set_rule_enabled` reading it and writing it.
  An ID that is not in the list is an `Err`, not a no-op.
- **The sink calls `RuleSet::remove_dropped`,** not `remove_trace` or
  `remove_spans` plus `record_removed`. The plan words the engine's step that way,
  and decision 0017 explains why `remove_dropped` is the same step with the rule
  found in the memory (the verdict does not name it) and with the spans before the
  traces. The whole batch (`filter_spans`, `remove_dropped`, `ingest_spans`, or
  `filter_logs` and `ingest_logs`) runs under one `parking_lot::Mutex`, logs
  included: a log batch that was judged before a span batch dropped its trace
  would otherwise be stored after that trace's logs were removed. The pause check
  comes before the lock, so a paused engine never touches the rules.
- **"Accepted" is what the rules let through.** The rate meters and the total
  behind `dropped_fraction` count the batch's `keep`, after the rules. The total
  does not shrink when a later drop removes what was accepted (the stored spans of
  a trace whose root arrives last), so such a span is on both sides of the
  fraction, once as accepted and once as dropped (decision 0017 says the same of
  `totals()`). For a ratio shown as a whole percent that is not worth the
  bookkeeping.
- **`EngineStatus` is a plain snapshot,** with `rules` as the `RuleStatus` list
  and the totals read from the `RuleSet` at the time of the call. The two reads
  are not one atomic step, so a batch in between can make `dropped_*` and the rule
  counters disagree for a moment.
- **The engine re-exports** `RuleSpec`, `RuleStatus`, `RuleId`, `RuleLanguage`,
  `DropScope`, `RulesFile`, `Preview`, `PREVIEW_WINDOW`, `QueryError` and
  `RULES_POLL`, so the app, the UI and the macOS shell depend on the engine alone,
  as they do for the settings.

## Consequences

- A test or an agent that starts `loctel` with a scratch `--settings` gets a
  scratch rules file next to it, and `--rules` overrides it. The release smoke
  test starts the bundled binary with no `--settings` at all, so it uses (and
  creates) the per-user rules file on the runner, which holds no rules.
- Editing `rules.toml` while loctel runs is safe: a typo keeps the old rules
  and shows the error, within about two seconds.
- Saving through the handle drops comments a person added to the file. The header
  stays. Menu and rules-screen code must not offer "edit in the app" and "edit by
  hand" as if both were lossless.
- The Mac pieces (Task 8) get the rules from `EngineHandle::rules()` and
  `rules_path()`, and never read the file themselves.
