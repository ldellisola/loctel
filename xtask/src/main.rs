//! Repo automation (`cargo xtask ...`, an alias set in `.cargo/config.toml`).
//!
//! - `screenshots [--update]` runs the screenshot tests of `loctel-ui`.
//!   With `--update` it regenerates the reference PNGs instead of comparing
//!   (and reminds you to look at them before committing).
//! - `run-shot` builds `loctel` and `meshgen`, runs `loctel` under a virtual X
//!   display, sends it the failing checkout trace, captures the screen with the
//!   main window on it and writes `target/run-shot.png`.
//! - `next-version` prints the version the next release gets (`version.rs`).
//! - `release-notes` prints the notes of the next release: the commit subjects
//!   since the previous version tag, the Mac checklist and the install steps
//!   (`notes.rs`).
//! - `mcp-smoke --url URL` checks that the MCP server at URL lists
//!   `recent_failures` (`smoke.rs`).
//!
//! The last three are what `.github/workflows/release.yml` runs. Only
//! `mcp-smoke` needs more than the standard library (rmcp's client), which is
//! why `cargo xtask` has a few dependencies at all.

mod notes;
mod smoke;
mod version;

use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const USAGE: &str = "\
usage: cargo xtask <command>

commands:
  screenshots [--update]  run the screenshot tests; --update rewrites the
                          reference PNGs (view them before committing)
  run-shot                run `loctel` under xvfb-run, send it the failing checkout
                          trace and write the main window to target/run-shot.png
  next-version            print the next release's version (the highest vX.Y.Z
                          tag with its patch plus one; v0.1.0 with no tags)
  release-notes           print the next release's notes: the commit subjects
                          since the previous version tag, the Mac checklist and
                          the install steps
  mcp-smoke --url URL     exit 0 only if the MCP server at URL lists the tool
                          `recent_failures` (the release job's smoke test)
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
    NextVersion,
    ReleaseNotes,
    McpSmoke { url: String },
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
        ("next-version", []) => Ok(XtaskCommand::NextVersion),
        ("next-version", [other, ..]) => Err(format!("next-version: unknown argument `{other}`")),
        ("release-notes", []) => Ok(XtaskCommand::ReleaseNotes),
        ("release-notes", [other, ..]) => Err(format!("release-notes: unknown argument `{other}`")),
        ("mcp-smoke", [flag, url]) if flag == "--url" => {
            Ok(XtaskCommand::McpSmoke { url: url.clone() })
        }
        ("mcp-smoke", []) => Err("mcp-smoke: `--url URL` is required".to_owned()),
        ("mcp-smoke", [flag]) if flag == "--url" => {
            Err("mcp-smoke: `--url` needs a value".to_owned())
        }
        ("mcp-smoke", [other, ..]) => Err(format!("mcp-smoke: unknown argument `{other}`")),
        (other, _) => Err(format!("unknown command `{other}`")),
    }
}

fn run(command: XtaskCommand) -> Result<(), String> {
    match command {
        XtaskCommand::Screenshots { update } => screenshots(update),
        XtaskCommand::RunShot => run_shot(),
        XtaskCommand::NextVersion => next_version_command(),
        XtaskCommand::ReleaseNotes => release_notes_command(),
        XtaskCommand::McpSmoke { url } => smoke::mcp_smoke(&url),
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

/// Runs inside `xvfb-run`. Arguments: the `loctel` binary, the `meshgen`
/// binary, the output PNG.
///
/// It starts `loctel` with every port 0 and a scratch settings file, waits
/// for its main window and lets the first frames land (about 2 s). Then it
/// reads the OTLP HTTP address from the listener lines `loctel` printed, sends
/// the failing checkout trace (`meshgen --scenario checkout-failure`), waits
/// for the window's next refresh (it reads the store every 500 ms) and captures
/// the whole screen with ImageMagick's `import`. The screen is exactly the
/// window's default size, so the capture is the window and nothing else. A
/// capture of a single colour means the window stayed black (the old GPUI X11
/// wake-up bug, design 10.5): one `xdotool windowmove` nudge and a second
/// capture. Still blank, it fails.
const RUN_SHOT_SCRIPT: &str = r#"
set -u
bin=$1
meshgen=$2
out=$3
scratch=$(mktemp -d)
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-$scratch}"
app=
cleanup() {
  [ -n "$app" ] && kill "$app" 2>/dev/null
  [ -n "$app" ] && wait "$app" 2>/dev/null
  rm -rf "$scratch"
}
trap cleanup EXIT

"$bin" --grpc-port 0 --http-port 0 --mcp-port 0 --settings "$scratch/settings.toml" \
  >"$scratch/stdout" 2>"$scratch/stderr" &
app=$!

win=
for _ in $(seq 1 200); do
  win=$(xdotool search --onlyvisible --pid "$app" 2>/dev/null | head -n 1)
  [ -n "$win" ] && break
  if ! kill -0 "$app" 2>/dev/null; then
    echo "xtask: loctel exited before it opened a window" >&2
    cat "$scratch/stderr" >&2
    exit 1
  fi
  sleep 0.1
done
if [ -z "$win" ]; then
  echo "xtask: no loctel window appeared within 20 s" >&2
  exit 1
fi

# The first frames.
sleep 2

# The traffic: the listener lines say where the HTTP receiver is (port 0).
addr=$(sed -n 's/.*"name":"otlp-http".*"address":"\([^"]*\)".*/\1/p' "$scratch/stdout" | head -n 1)
if [ -z "$addr" ]; then
  echo "xtask: loctel printed no otlp-http address" >&2
  cat "$scratch/stdout" >&2
  exit 1
fi
"$meshgen" --scenario checkout-failure --transport http-json --endpoint "http://$addr" >&2 || exit 1

# One refresh of the window's data (every 500 ms) and a frame to draw it.
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

/// The virtual screen of `run-shot`: the main window's default size
/// (`loctel_ui::app::DEFAULT_SIZE`, 1440 by 900), so the window fills it.
const SHOT_SCREEN: &str = "-screen 0 1440x900x24";

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
    build
        .current_dir(&root)
        .args(["build", "-p", "loctel-app", "-p", "loctel-meshgen"]);
    succeeded(
        build.status(),
        "cargo build -p loctel-app -p loctel-meshgen",
    )?;

    let target = target_dir(&root);
    let binary = target.join("debug/loctel");
    let meshgen = target.join("debug/meshgen");
    let output = target.join("run-shot.png");
    std::fs::create_dir_all(&target)
        .map_err(|error| format!("cannot create {}: {error}", target.display()))?;
    // A stale image must not pass for this run's.
    let _ = std::fs::remove_file(&output);

    let mut shot = Command::new("xvfb-run");
    shot.current_dir(&root)
        .args(["-a", "-s", SHOT_SCREEN, "bash", "-c", RUN_SHOT_SCRIPT])
        .arg("run-shot")
        .arg(&binary)
        .arg(&meshgen)
        .arg(&output);
    succeeded(shot.status(), "xvfb-run")?;

    println!("wrote {}", output.display());
    Ok(())
}

/// The Mac checklist that goes into every release's notes, relative to the
/// workspace root.
const CHECKLIST: &str = "docs/milestones/mac-checklist.md";

/// The lines `git <args>` prints, run in `root`.
fn git_lines(root: &Path, args: &[&str]) -> Result<Vec<String>, String> {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .map_err(|error| format!("cannot run git: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "`git {}` failed ({}): {}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_owned)
        .collect())
}

/// Every tag that looks like a release (`v...`) in the repository at `root`.
/// A shallow checkout has none, so the release job fetches the full history.
fn release_tags(root: &Path) -> Result<Vec<String>, String> {
    git_lines(root, &["tag", "--list", "v*"])
}

/// The subject line of every commit since the previous release (the highest
/// `vX.Y.Z` tag), newest first; all commits when there is no release yet.
fn subjects_since_last_release(root: &Path) -> Result<Vec<String>, String> {
    let range = match version::latest_tag(&release_tags(root)?) {
        Some(tag) => format!("{tag}..HEAD"),
        None => "HEAD".to_owned(),
    };
    git_lines(root, &["log", "--format=%s", &range])
}

fn next_version_command() -> Result<(), String> {
    let tags = release_tags(&workspace_root())?;
    println!("{}", version::next_version(&tags));
    Ok(())
}

fn release_notes_command() -> Result<(), String> {
    let root = workspace_root();
    let subjects = subjects_since_last_release(&root)?;
    let checklist_path = root.join(CHECKLIST);
    let checklist = std::fs::read_to_string(&checklist_path)
        .map_err(|error| format!("cannot read {}: {error}", checklist_path.display()))?;
    print!("{}", notes::release_notes(&subjects, &checklist));
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
        assert_eq!(parsed(&["next-version"]), Ok(XtaskCommand::NextVersion));
        assert_eq!(parsed(&["release-notes"]), Ok(XtaskCommand::ReleaseNotes));
        assert_eq!(
            parsed(&["mcp-smoke", "--url", "http://127.0.0.1:4319/mcp"]),
            Ok(XtaskCommand::McpSmoke {
                url: "http://127.0.0.1:4319/mcp".to_owned()
            })
        );
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
        assert!(parsed(&["next-version", "extra"]).is_err());
        assert!(parsed(&["release-notes", "extra"]).is_err());
    }

    #[test]
    fn mcp_smoke_needs_its_url() {
        assert!(parsed(&["mcp-smoke"]).unwrap_err().contains("--url"));
        assert!(
            parsed(&["mcp-smoke", "--url"])
                .unwrap_err()
                .contains("value")
        );
        assert!(
            parsed(&["mcp-smoke", "http://x/mcp"])
                .unwrap_err()
                .contains("http://x/mcp")
        );
        assert!(parsed(&["mcp-smoke", "--url", "a", "b"]).is_err());
    }

    #[test]
    fn the_workspace_root_has_the_workspace_manifest() {
        assert!(workspace_root().join("Cargo.toml").is_file());
        assert!(workspace_root().join("xtask").is_dir());
    }

    #[test]
    fn the_release_job_has_a_checklist_to_put_in_the_notes() {
        // The job runs `release-notes` on every merge: a renamed or emptied
        // checklist file would fail the release, so this fails first.
        let checklist = std::fs::read_to_string(workspace_root().join(CHECKLIST))
            .expect("docs/milestones/mac-checklist.md exists");

        assert!(
            checklist.contains("\n1. "),
            "the checklist is a numbered list"
        );
        // It goes in under the notes' own "Check on your Mac" heading.
        assert!(!checklist.starts_with('#'), "the checklist has no title");
        let notes = notes::release_notes(&["feat: x".to_owned()], &checklist);
        assert!(notes.contains(&checklist));
    }

    /// A scratch repository whose commits and tags the tests write by hand.
    struct Repo(tempfile::TempDir);

    impl Repo {
        fn new() -> Repo {
            let repo = Repo(tempfile::tempdir().unwrap());
            repo.git(&["init", "--quiet"]);
            repo
        }

        fn path(&self) -> &Path {
            self.0.path()
        }

        /// Runs git with an identity and without signing, whatever the
        /// machine's own configuration says.
        fn git(&self, args: &[&str]) {
            let status = Command::new("git")
                .current_dir(self.path())
                .args([
                    "-c",
                    "user.name=test",
                    "-c",
                    "user.email=test@example.com",
                    "-c",
                    "commit.gpgsign=false",
                    "-c",
                    "tag.gpgsign=false",
                ])
                .args(args)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} failed");
        }

        fn commit(&self, subject: &str) {
            self.git(&["commit", "--quiet", "--allow-empty", "-m", subject]);
        }

        fn tag(&self, name: &str) {
            self.git(&["tag", name]);
        }
    }

    #[test]
    fn with_no_release_tag_every_commit_is_in_the_notes_and_the_version_is_the_first() {
        let repo = Repo::new();
        repo.commit("feat: first");
        repo.commit("fix: second");
        repo.tag("not-a-release");

        assert_eq!(
            subjects_since_last_release(repo.path()).unwrap(),
            ["fix: second", "feat: first"]
        );
        assert_eq!(
            version::next_version(&release_tags(repo.path()).unwrap()),
            "v0.1.0"
        );
    }

    #[test]
    fn the_notes_start_after_the_highest_version_tag() {
        let repo = Repo::new();
        repo.commit("feat: old");
        repo.tag("v0.1.9");
        repo.commit("feat: between");
        repo.tag("v0.1.10");
        repo.commit("fix: new (#7)");
        repo.commit("docs: newer (#8)");

        assert_eq!(
            subjects_since_last_release(repo.path()).unwrap(),
            ["docs: newer (#8)", "fix: new (#7)"]
        );
        assert_eq!(
            version::next_version(&release_tags(repo.path()).unwrap()),
            "v0.1.11"
        );
    }

    #[test]
    fn git_failures_are_reported_with_the_command() {
        let not_a_repo = tempfile::tempdir().unwrap();

        let error = release_tags(not_a_repo.path()).unwrap_err();

        assert!(error.contains("git tag --list"), "{error}");
    }
}
