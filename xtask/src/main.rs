//! Repo automation (`cargo xtask ...`, an alias set in `.cargo/config.toml`).
//!
//! - `screenshots [--update]` runs the screenshot tests of `loctel-ui`.
//!   With `--update` it regenerates the reference PNGs instead of comparing
//!   (and reminds you to look at them before committing).
//! - `run-shot` builds `loctel`, runs it under a virtual X display, captures
//!   the screen and writes `target/run-shot.png`.
//!
//! The release subcommands (next-version, release-notes, mcp-smoke) arrive
//! with the tasks that need them. This crate uses only the standard library,
//! so `cargo xtask` starts fast and has nothing to upgrade.

use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const USAGE: &str = "\
usage: cargo xtask <command>

commands:
  screenshots [--update]  run the screenshot tests; --update rewrites the
                          reference PNGs (view them before committing)
  run-shot                run `loctel` under xvfb-run and write target/run-shot.png
";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let command = match parse(&args) {
        Ok(command) => command,
        Err(message) => {
            eprintln!("xtask: {message}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("xtask: {message}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum XtaskCommand {
    Screenshots { update: bool },
    RunShot,
}

fn parse(args: &[String]) -> Result<XtaskCommand, String> {
    let Some((command, rest)) = args.split_first() else {
        return Err("no command given".to_owned());
    };
    match (command.as_str(), rest) {
        ("screenshots", []) => Ok(XtaskCommand::Screenshots { update: false }),
        ("screenshots", [flag]) if flag == "--update" => {
            Ok(XtaskCommand::Screenshots { update: true })
        }
        ("screenshots", [other, ..]) => Err(format!("screenshots: unknown argument `{other}`")),
        ("run-shot", []) => Ok(XtaskCommand::RunShot),
        ("run-shot", [other, ..]) => Err(format!("run-shot: unknown argument `{other}`")),
        (other, _) => Err(format!("unknown command `{other}`")),
    }
}

fn run(command: XtaskCommand) -> Result<(), String> {
    match command {
        XtaskCommand::Screenshots { update } => screenshots(update),
        XtaskCommand::RunShot => run_shot(),
    }
}

/// The workspace root: this crate sits one level below it.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives inside the workspace")
        .to_path_buf()
}

/// Cargo's target directory, as the build will use it.
fn target_dir(root: &Path) -> PathBuf {
    match env::var_os("CARGO_TARGET_DIR") {
        // Joining an absolute path replaces the base, so both forms work.
        Some(dir) => root.join(dir),
        None => root.join("target"),
    }
}

fn cargo() -> OsString {
    env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"))
}

fn screenshots(update: bool) -> Result<(), String> {
    let root = workspace_root();
    let mut cargo = Command::new(cargo());
    cargo
        .current_dir(&root)
        .args(["test", "-p", "loctel-ui", "--features", "screenshot"]);
    if update {
        cargo.env("UPDATE_SNAPSHOTS", "1");
    }
    succeeded(cargo.status(), "cargo test -p loctel-ui")?;

    if update {
        let snapshots = root.join("crates/loctel-ui/tests/snapshots");
        println!("\nReference screenshots are in {}:", snapshots.display());
        let mut names: Vec<_> = std::fs::read_dir(&snapshots)
            .map_err(|error| format!("cannot read {}: {error}", snapshots.display()))?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "png"))
            .collect();
        names.sort();
        for path in names {
            println!("  {}", path.display());
        }
        println!("Open every PNG that changed (`git status`) and check it before you commit it.");
    }
    Ok(())
}

/// Runs inside `xvfb-run`. Arguments: the `loctel` binary, the output PNG.
///
/// It starts `loctel` with every port 0 and a scratch settings file, waits
/// for its window, lets the first frames land, and captures the whole screen
/// with ImageMagick's `import`. A capture of a single colour means the window
/// stayed black (the old GPUI X11 wake-up bug, design 10.5): one `xdotool
/// windowmove` nudge and a second capture. Still blank, it fails.
const RUN_SHOT_SCRIPT: &str = r#"
set -u
bin=$1
out=$2
scratch=$(mktemp -d)
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-$scratch}"
app=
cleanup() {
  [ -n "$app" ] && kill "$app" 2>/dev/null
  [ -n "$app" ] && wait "$app" 2>/dev/null
  rm -rf "$scratch"
}
trap cleanup EXIT

"$bin" --grpc-port 0 --http-port 0 --mcp-port 0 --settings "$scratch/settings.toml" &
app=$!

win=
for _ in $(seq 1 200); do
  win=$(xdotool search --onlyvisible --pid "$app" 2>/dev/null | head -n 1)
  [ -n "$win" ] && break
  if ! kill -0 "$app" 2>/dev/null; then
    echo "xtask: loctel exited before it opened a window" >&2
    exit 1
  fi
  sleep 0.1
done
if [ -z "$win" ]; then
  echo "xtask: no loctel window appeared within 20 s" >&2
  exit 1
fi

# The first frame, and one refresh from the engine.
sleep 2
import -window root "$out" || exit 1

colors=$(convert "$out" -format '%k' info:)
if [ "$colors" -lt 2 ]; then
  echo "xtask: the capture is one flat colour; nudging the window and retrying" >&2
  xdotool mousemove 1 1 windowmove "$win" 0 0
  sleep 2
  import -window root "$out" || exit 1
  colors=$(convert "$out" -format '%k' info:)
  if [ "$colors" -lt 2 ]; then
    echo "xtask: the window stayed blank" >&2
    exit 1
  fi
fi
"#;

fn run_shot() -> Result<(), String> {
    let root = workspace_root();
    for tool in ["xvfb-run", "xdotool", "import", "convert"] {
        if !on_path(tool) {
            return Err(format!(
                "`{tool}` is not installed; run `bash script/setup-linux.sh`"
            ));
        }
    }

    let mut build = Command::new(cargo());
    build.current_dir(&root).args(["build", "-p", "loctel-app"]);
    succeeded(build.status(), "cargo build -p loctel-app")?;

    let target = target_dir(&root);
    let binary = target.join("debug/loctel");
    let output = target.join("run-shot.png");
    std::fs::create_dir_all(&target)
        .map_err(|error| format!("cannot create {}: {error}", target.display()))?;
    // A stale image must not pass for this run's.
    let _ = std::fs::remove_file(&output);

    let mut shot = Command::new("xvfb-run");
    shot.current_dir(&root)
        .args([
            "-a",
            "-s",
            "-screen 0 1280x800x24",
            "bash",
            "-c",
            RUN_SHOT_SCRIPT,
        ])
        .arg("run-shot")
        .arg(&binary)
        .arg(&output);
    succeeded(shot.status(), "xvfb-run")?;

    println!("wrote {}", output.display());
    Ok(())
}

fn on_path(program: &str) -> bool {
    env::var_os("PATH")
        .is_some_and(|paths| env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

fn succeeded(status: std::io::Result<std::process::ExitStatus>, what: &str) -> Result<(), String> {
    match status {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!("{what} failed ({status})")),
        Err(error) => Err(format!("cannot run {what}: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(args: &[&str]) -> Result<XtaskCommand, String> {
        parse(&args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>())
    }

    #[test]
    fn parses_the_commands() {
        assert_eq!(
            parsed(&["screenshots"]),
            Ok(XtaskCommand::Screenshots { update: false })
        );
        assert_eq!(
            parsed(&["screenshots", "--update"]),
            Ok(XtaskCommand::Screenshots { update: true })
        );
        assert_eq!(parsed(&["run-shot"]), Ok(XtaskCommand::RunShot));
    }

    #[test]
    fn rejects_what_it_does_not_know() {
        assert!(parsed(&[]).is_err());
        assert!(parsed(&["bogus"]).is_err());
        assert!(
            parsed(&["screenshots", "--bogus"])
                .unwrap_err()
                .contains("--bogus")
        );
        assert!(parsed(&["run-shot", "extra"]).is_err());
    }

    #[test]
    fn the_workspace_root_has_the_workspace_manifest() {
        assert!(workspace_root().join("Cargo.toml").is_file());
        assert!(workspace_root().join("xtask").is_dir());
    }
}
