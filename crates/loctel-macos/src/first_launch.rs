//! What the first launch does: turn on start at login, once (design §9.1,
//! "on by default").
//!
//! "Once" is `first_launch_done` in `settings.toml`. After that the login item
//! is the user's: the menu toggles it, and so does System Settings, so a later
//! launch must never turn it back on.

use std::path::Path;

use loctel_engine::Settings;

/// Runs the first-launch step against the settings file at `path` and returns
/// the problems to tell whoever reads stderr (none when all went well):
///
/// - On the first launch (`first_launch_done` is false, which a missing file
///   also means) it calls `enable_login_item`, sets `first_launch_done` and
///   saves the settings. A failure of either is a problem, not a stop: the
///   launch still counts as done, because asking again at every launch would
///   re-enable what the user may since have turned off.
/// - On every later launch it does nothing.
/// - If the file exists but cannot be read or parsed it does nothing and says
///   so: the file is not rewritten (it is the user's to fix) and the login
///   item is left alone, since this launch could never be recorded as done.
///
/// The settings it saves are the ones in the file. Command-line port flags are
/// not in them, so a run with `--grpc-port 0` never writes that port down.
pub(crate) fn first_launch(
    path: &Path,
    enable_login_item: impl FnOnce() -> anyhow::Result<()>,
) -> Vec<String> {
    let mut settings = match Settings::load(path) {
        Ok(settings) => settings,
        Err(error) => {
            return vec![format!("{error:#}; leaving start at login alone this time")];
        }
    };
    if settings.first_launch_done {
        return Vec::new();
    }

    let mut problems = Vec::new();
    if let Err(error) = enable_login_item() {
        problems.push(format!("cannot turn on start at login: {error:#}"));
    }
    settings.first_launch_done = true;
    if let Err(error) = settings.save(path) {
        problems.push(format!("cannot save {}: {error:#}", path.display()));
    }
    problems
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::fs;

    use loctel_engine::Settings;

    use super::*;

    fn enable_ok(calls: &Cell<u32>) -> impl FnOnce() -> anyhow::Result<()> + '_ {
        move || {
            calls.set(calls.get() + 1);
            Ok(())
        }
    }

    #[test]
    fn first_launch_enables_the_login_item_and_remembers_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("loctel").join("settings.toml");
        let calls = Cell::new(0);

        let problems = first_launch(&path, enable_ok(&calls));

        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(calls.get(), 1);
        assert!(Settings::load(&path).unwrap().first_launch_done);
    }

    #[test]
    fn later_launches_leave_the_login_item_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let calls = Cell::new(0);
        first_launch(&path, enable_ok(&calls));

        // The user turned it off in the menu since; it must stay off.
        let problems = first_launch(&path, enable_ok(&calls));

        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(calls.get(), 1, "the second launch must not enable it again");
    }

    #[test]
    fn a_login_item_error_is_reported_and_the_first_launch_is_still_done() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");

        let problems = first_launch(&path, || Err(anyhow::anyhow!("not signed")));

        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("not signed"), "{problems:?}");
        // Asking again at every launch would re-enable what the user may have
        // turned off, so the launch counts as done anyway.
        assert!(Settings::load(&path).unwrap().first_launch_done);
    }

    #[test]
    fn the_other_settings_survive() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        fs::write(&path, "otlp_grpc_port = 9001\nmcp_port = 9003\n").unwrap();

        first_launch(&path, || Ok(()));

        let settings = Settings::load(&path).unwrap();
        assert_eq!(settings.otlp_grpc_port, 9001);
        assert_eq!(settings.mcp_port, 9003);
        assert!(settings.first_launch_done);
    }

    #[test]
    fn a_broken_settings_file_is_not_rewritten_and_nothing_is_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        fs::write(&path, "this is = not [valid").unwrap();
        let calls = Cell::new(0);

        let problems = first_launch(&path, enable_ok(&calls));

        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("settings.toml"), "{problems:?}");
        assert_eq!(calls.get(), 0, "the login item must be left alone");
        assert_eq!(fs::read_to_string(&path).unwrap(), "this is = not [valid");
    }

    #[test]
    fn a_settings_path_that_is_a_directory_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        // Not a missing file (that is a first launch) but one that cannot be
        // read: reported, like a broken one.
        let path = dir.path().join("settings.toml");
        fs::create_dir(&path).unwrap();

        let calls = Cell::new(0);

        let problems = first_launch(&path, enable_ok(&calls));

        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(calls.get(), 0);
    }
}
