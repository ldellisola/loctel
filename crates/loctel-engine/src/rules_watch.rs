//! The discard rules inside the running engine: the rule list and its file,
//! kept in step.
//!
//! [`Rules`] holds the [`RuleSet`] the sink filters with and, when the engine
//! was given a path, `rules.toml`. It reads the file at start, saves it when the
//! handle changes the rules, and (through [`watch`]) reloads it when somebody
//! edits it by hand. A broken file never takes the rules down: the previous
//! rules keep applying and the file's error is reported, until a good file
//! arrives (`docs/areas/rules.md`, "In the engine").

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use anyhow::bail;
use loctel_rules::{RuleId, RuleSet, RuleSpec, RulesFile};
use loctel_store::Clock;
use parking_lot::Mutex;
use tokio::time::MissedTickBehavior;

/// How often the engine looks at the rules file for an edit.
pub const RULES_POLL: Duration = Duration::from_secs(2);

/// What the file looked like when the engine last read or wrote it. A change
/// of either part means somebody else wrote it. The length is there because a
/// coarse file system clock can give two writes made close together the same
/// modification time, and a rule switched on or off changes the length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
}

/// The file's stamp, or `None` when there is no file to stat.
fn stamp(path: &Path) -> Option<Stamp> {
    let metadata = fs::metadata(path).ok()?;
    Some(Stamp {
        modified: metadata.modified().ok(),
        len: metadata.len(),
    })
}

/// The file side of the rules, behind one lock so that a reload, a save from
/// the handle and the stamp they keep cannot interleave.
struct FileState {
    /// What the engine last read or wrote. Its own writes update it, so they
    /// do not look like an edit at the next poll.
    seen: Option<Stamp>,
    /// Why the file could not be used the last time it was read, or `None`.
    error: Option<String>,
}

/// The rule list, and the file it lives in.
pub(crate) struct Rules {
    set: Arc<RuleSet>,
    /// `None`: the rules live in memory only (tests).
    path: Option<PathBuf>,
    file: Mutex<FileState>,
    /// How many times the rules have been replaced since the engine started
    /// (see [`Rules::generation`]).
    generation: AtomicU64,
}

impl Rules {
    /// Opens the rules the way the engine starts them:
    ///
    /// - no path: no rules, kept in memory only
    /// - no file: one is written holding no rules (loctel never decides what to
    ///   discard), and there are no rules
    /// - a file: its rules
    /// - a file that cannot be read or is not valid rules (or a place where
    ///   the empty file cannot be written): no rules and the error, and the
    ///   file is left as it was
    pub(crate) fn open(path: Option<PathBuf>, clock: Arc<dyn Clock>) -> Rules {
        let mut specs = Vec::new();
        let mut file = FileState {
            seen: None,
            error: None,
        };
        if let Some(path) = &path {
            // The stamp comes first: an edit between the stamp and the read is
            // then read twice (harmless), never missed.
            file.seen = stamp(path);
            match RulesFile::load(path) {
                Ok(Some(rules)) => specs = rules,
                Ok(None) => match RulesFile::write_empty(path) {
                    Ok(()) => file.seen = stamp(path),
                    Err(error) => file.error = Some(format!("{error:#}")),
                },
                Err(error) => file.error = Some(format!("{error:#}")),
            }
        }
        Rules {
            set: Arc::new(RuleSet::new(specs, clock)),
            path,
            file: Mutex::new(file),
            generation: AtomicU64::new(0),
        }
    }

    /// The rules, for the sink to filter with.
    pub(crate) fn set(&self) -> &Arc<RuleSet> {
        &self.set
    }

    pub(crate) fn path(&self) -> Option<PathBuf> {
        self.path.clone()
    }

    /// How many times the rules have been replaced: a reload of an edited file
    /// and every save through the handle count, a file that could not be used
    /// does not. It goes up after the new rules are in force, so whoever sees a
    /// new number reads the new rules.
    pub(crate) fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// The error of the rules file, as of the last time it was read.
    pub(crate) fn error(&self) -> Option<String> {
        self.file.lock().error.clone()
    }

    /// Looks at the file and, if it changed since the engine last read or wrote
    /// it, loads it. Returns whether it read the file.
    ///
    /// - On success the rules are replaced (a rule that did not change keeps its
    ///   counters) and the error is cleared.
    /// - On a failure (unreadable, bad TOML, a wrong key) the current rules stay
    ///   and the error is set. The file is read again only when it changes
    ///   again.
    /// - A file that is gone changes nothing: an editor that saves by
    ///   renaming can leave none for a moment, and the engine must not write
    ///   over what it is about to put there. The rules in memory stay, and the
    ///   file is written again by the next change made through the handle.
    pub(crate) fn reload_if_changed(&self) -> bool {
        let Some(path) = &self.path else {
            return false;
        };
        let mut file = self.file.lock();
        let now = stamp(path);
        if now == file.seen {
            return false;
        }
        file.seen = now;
        if now.is_none() {
            return false;
        }
        match RulesFile::load(path) {
            Ok(Some(specs)) => {
                self.set.replace(specs);
                self.generation.fetch_add(1, Ordering::Release);
                file.error = None;
            }
            // Gone again since the stat: the next poll sees that.
            Ok(None) => return false,
            Err(error) => file.error = Some(format!("{error:#}")),
        }
        true
    }

    /// Makes `specs` the rules and, with a path, saves the file first: if it
    /// cannot be written nothing changes and the error comes back, so the rules
    /// in memory are never ones the file does not have. Whatever was wrong with
    /// the file is gone, since it has just been written over.
    pub(crate) fn set_rules(&self, specs: Vec<RuleSpec>) -> anyhow::Result<()> {
        let mut file = self.file.lock();
        self.apply(&mut file, specs)
    }

    /// Turns one rule on or off, keeping the order and everything else, and
    /// saves like [`set_rules`](Self::set_rules).
    pub(crate) fn set_rule_enabled(&self, id: RuleId, enabled: bool) -> anyhow::Result<()> {
        // The lock is held from the read of the list to the write, so a reload
        // cannot change the rules in between.
        let mut file = self.file.lock();
        let mut found = false;
        let specs: Vec<RuleSpec> = self
            .set
            .statuses()
            .into_iter()
            .map(|status| {
                let mut spec = status.spec;
                if status.id == id {
                    found = true;
                    spec.enabled = enabled;
                }
                spec
            })
            .collect();
        if !found {
            bail!("there is no rule with id {}", id.0);
        }
        self.apply(&mut file, specs)
    }

    fn apply(&self, file: &mut FileState, specs: Vec<RuleSpec>) -> anyhow::Result<()> {
        if let Some(path) = &self.path {
            RulesFile::save(path, &specs)?;
            file.seen = stamp(path);
            file.error = None;
        }
        self.set.replace(specs);
        self.generation.fetch_add(1, Ordering::Release);
        Ok(())
    }
}

/// Polls the rules file every [`RULES_POLL`] until the runtime stops. The file
/// is read on the blocking pool: the file may be on a slow disk, and the
/// engine's async threads are the receivers'.
pub(crate) async fn watch(rules: Arc<Rules>) {
    let mut ticker = tokio::time::interval(RULES_POLL);
    // A poll that ran late is not made up for with a burst of them.
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    // The first tick is now, and the file was just read.
    ticker.tick().await;
    loop {
        ticker.tick().await;
        let rules = Arc::clone(&rules);
        // A panic in the reload ends that poll, not the watching.
        let _ = tokio::task::spawn_blocking(move || rules.reload_if_changed()).await;
    }
}

#[cfg(test)]
mod tests {
    use loctel_rules::{DropScope, RuleLanguage};
    use loctel_store::SystemClock;

    use super::*;

    fn rules_at(path: &Path) -> Rules {
        Rules::open(Some(path.to_owned()), Arc::new(SystemClock))
    }

    fn spec(name: &str) -> RuleSpec {
        RuleSpec {
            name: name.to_owned(),
            enabled: true,
            language: RuleLanguage::TraceQl,
            query: r#"{ name = "x" }"#.to_owned(),
            scope: DropScope::Trace,
        }
    }

    #[test]
    fn own_writes_do_not_look_like_an_edit() {
        let dir = tempfile::tempdir().unwrap();
        let rules = rules_at(&dir.path().join("rules.toml"));
        assert!(!rules.reload_if_changed(), "the empty file just written");

        rules.set_rules(vec![spec("a")]).unwrap();
        assert!(!rules.reload_if_changed(), "a save through the handle");

        let id = rules.set().statuses()[0].id;
        rules.set_rule_enabled(id, false).unwrap();
        assert!(!rules.reload_if_changed(), "a switch through the handle");
    }

    #[test]
    fn an_outside_edit_is_read_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.toml");
        let rules = rules_at(&path);
        rules.set_rules(vec![spec("a")]).unwrap();

        RulesFile::save(&path, &[spec("a"), spec("b")]).unwrap();

        assert!(rules.reload_if_changed());
        assert_eq!(rules.set().statuses().len(), 2);
        assert!(!rules.reload_if_changed(), "nothing changed since");
    }

    #[test]
    fn a_bad_file_is_read_once_and_keeps_the_rules() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.toml");
        let rules = rules_at(&path);
        rules.set_rules(vec![spec("a")]).unwrap();

        fs::write(&path, "not [ toml").unwrap();

        assert!(rules.reload_if_changed());
        assert!(rules.error().is_some_and(|e| e.contains("rules.toml")));
        assert_eq!(rules.set().statuses().len(), 1);
        assert!(!rules.reload_if_changed(), "a bad file is not retried");

        // A save through the handle writes over it: no error any more.
        rules.set_rules(vec![spec("a")]).unwrap();
        assert_eq!(rules.error(), None);
    }

    #[test]
    fn a_deleted_file_changes_nothing_and_is_not_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.toml");
        let rules = rules_at(&path);
        rules.set_rules(vec![spec("a")]).unwrap();

        fs::remove_file(&path).unwrap();

        assert!(!rules.reload_if_changed());
        assert_eq!(rules.set().statuses().len(), 1, "the rules stay");
        assert_eq!(rules.error(), None);
        assert!(!path.exists(), "the engine does not write over a gap");

        // When the file comes back, it is read.
        RulesFile::save(&path, &[]).unwrap();
        assert!(rules.reload_if_changed());
        assert!(rules.set().statuses().is_empty());
    }

    #[test]
    fn without_a_path_there_is_nothing_to_reload() {
        let rules = Rules::open(None, Arc::new(SystemClock));
        rules.set_rules(vec![spec("a")]).unwrap();

        assert!(!rules.reload_if_changed());
        assert_eq!(rules.path(), None);
        assert_eq!(rules.set().statuses().len(), 1);
    }
}
