//! The built `loctel` binary without `--headless`, on Linux: it opens the main
//! window on a display, prints the same startup lines as headless mode and
//! stops cleanly on SIGTERM.
//!
//! The display is a virtual one (`xvfb-run`). Where `xvfb-run` is missing the
//! test says so and passes without running anything; CI installs it
//! (`script/setup-linux.sh`), so CI never skips.

#![cfg(target_os = "linux")]

use std::path::Path;
use std::process::Command;

use serde_json::Value;

/// Runs inside `xvfb-run`. Arguments: the `loctel` binary, a scratch
/// directory.
///
/// It starts `loctel` with every port 0 and settings and rules files in the
/// scratch directory (so a run never touches the developer's real ones),
/// leaves it running for 3 s, says whether it is still alive, sends SIGTERM and
/// waits up to 10 s for it to go. Its own stdout is the verdict, one word per
/// line: `alive` or `gone`, then `exit=N` (the status `loctel` ended with) or
/// `hung` (it ignored SIGTERM and was killed). What `loctel` printed is in
/// `stdout` and `stderr` of the scratch directory.
const SCRIPT: &str = r#"
set -u
bin=$1
dir=$2
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-$dir}"
"$bin" --grpc-port 0 --http-port 0 --mcp-port 0 \
  --settings "$dir/settings.toml" --rules "$dir/rules.toml" \
  >"$dir/stdout" 2>"$dir/stderr" &
app=$!
sleep 3
if kill -0 "$app" 2>/dev/null; then echo alive; else echo gone; fi
kill -TERM "$app" 2>/dev/null
for _ in $(seq 1 100); do
  kill -0 "$app" 2>/dev/null || break
  sleep 0.1
done
if kill -0 "$app" 2>/dev/null; then
  kill -KILL "$app"
  wait "$app" 2>/dev/null
  echo hung
  exit 0
fi
wait "$app"
echo "exit=$?"
"#;

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[test]
fn linux_window_opens_under_xvfb() {
    if !on_path("xvfb-run") {
        println!("skipped: no xvfb-run");
        return;
    }
    let dir = tempfile::tempdir().expect("a scratch directory");

    let run = Command::new("xvfb-run")
        .args(["-a", "-s", "-screen 0 1280x800x24", "bash", "-c", SCRIPT])
        .arg("window-smoke")
        .arg(env!("CARGO_BIN_EXE_loctel"))
        .arg(dir.path())
        .output()
        .expect("xvfb-run starts");
    let verdict = String::from_utf8_lossy(&run.stdout).into_owned();
    let stderr = read(&dir.path().join("stderr"));
    let stdout = read(&dir.path().join("stdout"));
    let context = format!(
        "verdict: {verdict:?}\nxvfb-run stderr: {}\nloctel stderr: {stderr}",
        String::from_utf8_lossy(&run.stderr)
    );

    // The window opened and stayed open: a window that could not be opened
    // ends the process with an error before the 3 s are up.
    let words: Vec<&str> = verdict.lines().collect();
    assert_eq!(words.first(), Some(&"alive"), "{context}");

    // The startup lines are the headless ones: the three listeners, then the
    // discard rules.
    let lines: Vec<Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {line:?}")))
        .collect();
    assert_eq!(lines.len(), 4, "stdout: {stdout:?}\n{context}");
    for (line, name) in lines.iter().zip(["otlp-grpc", "otlp-http", "mcp"]) {
        assert_eq!(line["event"], "listener", "{line}");
        assert_eq!(line["name"], name, "{line}");
        assert_eq!(line["state"], "listening", "{line}");
    }
    assert_eq!(lines[3]["event"], "rules", "{}", lines[3]);

    // No panic, and no "cannot open the window" (what a display GPUI refuses
    // prints). Driver noise (libEGL, Vulkan) is allowed.
    assert!(!stderr.contains("panicked"), "{context}");
    assert!(!stderr.contains("cannot open the window"), "{context}");

    // SIGTERM stops it with status 0, not by killing it or by hanging.
    assert_eq!(words.get(1), Some(&"exit=0"), "{context}");
}
