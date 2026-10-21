//! The built `loctel` binary in headless mode: what it prints, and how it
//! stops. Every run gets its own `--settings` path so it never reads or
//! writes the developer's real settings.

use std::io::{BufRead, BufReader};
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

const TIMEOUT: Duration = Duration::from_secs(30);

struct Loctel {
    child: Child,
    lines: Receiver<String>,
}

impl Loctel {
    /// Starts `loctel --headless` with `args`. The settings file is the one
    /// given, which may not exist. Unless `args` has a `--rules` of its own,
    /// the rules file is `rules.toml` beside the settings file, so a run never
    /// reads or writes the developer's real rules.
    fn spawn(settings: &Path, args: &[&str]) -> Loctel {
        if args.contains(&"--rules") {
            return Loctel::spawn_exactly(settings, args);
        }
        let rules = rules_path_beside(settings);
        let mut all = vec!["--rules", rules.to_str().unwrap()];
        all.extend_from_slice(args);
        Loctel::spawn_exactly(settings, &all)
    }

    /// [`spawn`](Self::spawn) with `args` as given: no `--rules` is added.
    fn spawn_exactly(settings: &Path, args: &[&str]) -> Loctel {
        let mut child = Command::new(env!("CARGO_BIN_EXE_loctel"))
            .arg("--headless")
            .arg("--settings")
            .arg(settings)
            .args(args)
            .stdout(Stdio::piped())
            .spawn()
            .expect("the loctel binary starts");
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Loctel { child, lines }
    }

    /// The next line on stdout, parsed as JSON.
    fn next_json(&self) -> Value {
        let line = self
            .lines
            .recv_timeout(TIMEOUT)
            .expect("a line on stdout within the timeout");
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("not JSON ({e}): {line:?}"))
    }

    /// The three listener lines, as printed: `otlp-grpc`, `otlp-http`, `mcp`.
    fn listener_lines(&self) -> [Value; 3] {
        [self.next_json(), self.next_json(), self.next_json()]
    }

    fn signal(&self, name: &str) {
        let status = Command::new("kill")
            .arg(format!("-{name}"))
            .arg(self.child.id().to_string())
            .status()
            .expect("kill runs");
        assert!(status.success(), "kill -{name} failed");
    }

    /// Waits for the process to exit.
    fn wait(&mut self) -> ExitStatus {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "loctel did not exit in time");
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Loctel {
    fn drop(&mut self) {
        // A test that failed must not leave a server behind.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn settings_path(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("settings.toml")
}

/// `rules.toml` in the directory of `settings`.
fn rules_path_beside(settings: &Path) -> PathBuf {
    settings.with_file_name("rules.toml")
}

const PORTS_ZERO: [&str; 6] = ["--grpc-port", "0", "--http-port", "0", "--mcp-port", "0"];

fn assert_listening(line: &Value, name: &str) -> SocketAddr {
    assert_eq!(line["event"], "listener", "{line}");
    assert_eq!(line["name"], name, "{line}");
    assert_eq!(line["state"], "listening", "{line}");
    assert!(line.get("error").is_none(), "{line}");
    let address: SocketAddr = line["address"]
        .as_str()
        .unwrap_or_else(|| panic!("no address in {line}"))
        .parse()
        .unwrap();
    assert!(address.ip().is_loopback(), "{line}");
    assert_ne!(address.port(), 0, "the real port, not the requested one");
    address
}

#[test]
fn headless_binary_starts_and_stops() {
    let dir = tempfile::tempdir().unwrap();
    let mut loctel = Loctel::spawn(&settings_path(&dir), &PORTS_ZERO);

    let [grpc, http, mcp] = loctel.listener_lines();
    assert_listening(&grpc, "otlp-grpc");
    assert_listening(&http, "otlp-http");
    assert_listening(&mcp, "mcp");

    loctel.signal("TERM");

    let status = loctel.wait();
    assert_eq!(status.code(), Some(0), "{status:?}");
}

#[test]
fn headless_binary_stops_cleanly_on_sigint() {
    let dir = tempfile::tempdir().unwrap();
    let mut loctel = Loctel::spawn(&settings_path(&dir), &PORTS_ZERO);
    loctel.listener_lines();

    loctel.signal("INT");

    assert_eq!(loctel.wait().code(), Some(0));
}

#[test]
fn headless_binary_serves_the_ports_it_printed() {
    let dir = tempfile::tempdir().unwrap();
    let loctel = Loctel::spawn(&settings_path(&dir), &PORTS_ZERO);
    let [grpc, http, mcp] = loctel.listener_lines();

    for line in [&grpc, &http, &mcp] {
        let name = line["name"].as_str().unwrap();
        let address = assert_listening(line, name);
        assert!(
            std::net::TcpStream::connect(address).is_ok(),
            "{name} does not accept connections on {address}"
        );
    }
}

#[test]
fn headless_binary_reports_a_failed_listener_and_keeps_running() {
    let taken = TcpListener::bind("127.0.0.1:0").unwrap();
    let taken_port = taken.local_addr().unwrap().port().to_string();
    let dir = tempfile::tempdir().unwrap();
    let mut loctel = Loctel::spawn(
        &settings_path(&dir),
        &[
            "--grpc-port",
            "0",
            "--http-port",
            &taken_port,
            "--mcp-port",
            "0",
        ],
    );

    let [grpc, http, mcp] = loctel.listener_lines();

    assert_listening(&grpc, "otlp-grpc");
    assert_listening(&mcp, "mcp");
    assert_eq!(http["event"], "listener", "{http}");
    assert_eq!(http["name"], "otlp-http", "{http}");
    assert_eq!(http["state"], "failed", "{http}");
    assert!(
        http["error"].as_str().is_some_and(|e| !e.is_empty()),
        "the error says why: {http}"
    );
    assert!(http.get("address").is_none(), "{http}");

    // A taken port is not fatal: the process runs until it is told to stop.
    assert!(loctel.child.try_wait().unwrap().is_none());
    loctel.signal("TERM");
    assert_eq!(loctel.wait().code(), Some(0));
}

#[test]
fn headless_binary_reads_ports_from_the_settings_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = settings_path(&dir);
    // Ports 0 in the file, no flags: the real ports printed are not the
    // defaults, so the file was read.
    std::fs::write(
        &path,
        "otlp_grpc_port = 0\notlp_http_port = 0\nmcp_port = 0\n",
    )
    .unwrap();
    let loctel = Loctel::spawn(&path, &[]);

    let [grpc, http, mcp] = loctel.listener_lines();

    assert_ne!(assert_listening(&grpc, "otlp-grpc").port(), 4317);
    assert_ne!(assert_listening(&http, "otlp-http").port(), 4318);
    assert_ne!(assert_listening(&mcp, "mcp").port(), 4319);
}

#[test]
fn headless_binary_prefers_flags_over_the_settings_file() {
    let taken = TcpListener::bind("127.0.0.1:0").unwrap();
    let taken_port = taken.local_addr().unwrap().port();
    let dir = tempfile::tempdir().unwrap();
    let path = settings_path(&dir);
    // The file names a port that is taken; the flag moves off it.
    std::fs::write(
        &path,
        format!("otlp_grpc_port = 0\notlp_http_port = {taken_port}\nmcp_port = 0\n"),
    )
    .unwrap();
    let loctel = Loctel::spawn(&path, &["--http-port", "0"]);

    let [grpc, http, mcp] = loctel.listener_lines();

    assert_listening(&grpc, "otlp-grpc");
    assert_ne!(assert_listening(&http, "otlp-http").port(), taken_port);
    assert_listening(&mcp, "mcp");
}

#[test]
fn headless_binary_survives_an_invalid_settings_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = settings_path(&dir);
    std::fs::write(&path, "this is not toml").unwrap();
    let mut loctel = Loctel::spawn(&path, &PORTS_ZERO);

    // The flags still apply, and the bad file is left as it was.
    let [grpc, http, mcp] = loctel.listener_lines();
    assert_listening(&grpc, "otlp-grpc");
    assert_listening(&http, "otlp-http");
    assert_listening(&mcp, "mcp");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "this is not toml");

    loctel.signal("TERM");
    assert_eq!(loctel.wait().code(), Some(0));
}

#[test]
fn headless_binary_reports_a_taken_mcp_port_and_keeps_running() {
    let taken = TcpListener::bind("127.0.0.1:0").unwrap();
    let taken_port = taken.local_addr().unwrap().port().to_string();
    let dir = tempfile::tempdir().unwrap();
    let mut loctel = Loctel::spawn(
        &settings_path(&dir),
        &[
            "--grpc-port",
            "0",
            "--http-port",
            "0",
            "--mcp-port",
            &taken_port,
        ],
    );

    let [grpc, http, mcp] = loctel.listener_lines();

    assert_listening(&grpc, "otlp-grpc");
    assert_listening(&http, "otlp-http");
    assert_eq!(
        mcp,
        serde_json::json!({
            "event": "listener",
            "name": "mcp",
            "state": "failed",
            "error": mcp["error"],
        })
    );
    assert!(
        mcp["error"].as_str().is_some_and(|e| !e.is_empty()),
        "{mcp}"
    );
    assert!(loctel.child.try_wait().unwrap().is_none());
    loctel.signal("TERM");
    assert_eq!(loctel.wait().code(), Some(0));
}

/// A POST to `address`, answered in full (the request closes the connection).
fn post(address: SocketAddr, path: &str, extra_headers: &str, body: &str) -> String {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(TIMEOUT)).unwrap();
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nHost: {address}\r\n{extra_headers}\
         Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    reply
}

#[test]
fn headless_binary_answers_mcp_on_the_printed_address() {
    let dir = tempfile::tempdir().unwrap();
    let loctel = Loctel::spawn(&settings_path(&dir), &PORTS_ZERO);
    let [_, _, mcp] = loctel.listener_lines();
    let address = assert_listening(&mcp, "mcp");
    let initialize = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}"#;

    let reply = post(address, "/mcp", "", initialize);

    assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
    assert!(reply.contains("\"serverInfo\""), "{reply}");
    assert!(reply.contains("\"name\":\"loctel\""), "{reply}");

    // A page on another site is refused.
    let refused = post(
        address,
        "/mcp",
        "Origin: https://evil.example\r\n",
        initialize,
    );
    assert!(refused.starts_with("HTTP/1.1 403"), "{refused}");
}

/// The line after the three listener lines.
fn rules_line(loctel: &Loctel) -> Value {
    for _ in 0..3 {
        loctel.next_json();
    }
    loctel.next_json()
}

#[test]
fn headless_prints_rules_line() {
    let dir = tempfile::tempdir().unwrap();
    let rules = dir.path().join("rules.toml");
    let rules_arg = rules.to_str().unwrap();
    let mut args = vec!["--rules", rules_arg];
    args.extend_from_slice(&PORTS_ZERO);
    let mut loctel = Loctel::spawn(&settings_path(&dir), &args);

    let line = rules_line(&loctel);

    // A first start writes a file with no rules, and says where it is.
    assert_eq!(
        line,
        serde_json::json!({
            "event": "rules",
            "path": rules_arg,
            "count": 0,
            "error": null,
        })
    );
    let text = std::fs::read_to_string(&rules).expect("the rules file was written");
    assert!(text.contains("# [[rule]]"), "{text}");
    assert!(!text.lines().any(|l| l.starts_with("[[rule]]")), "{text}");

    loctel.signal("TERM");
    assert_eq!(loctel.wait().code(), Some(0));
}

#[test]
fn headless_rules_line_counts_the_rules_of_an_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let rules = dir.path().join("rules.toml");
    std::fs::write(
        &rules,
        r#"[[rule]]
name = "Health"
enabled = true
language = "traceql"
query = '{ span.url.path = "/health" }'

[[rule]]
name = "Noise"
enabled = false
language = "logql"
query = '{service_name="web"} |= "noise"'
"#,
    )
    .unwrap();
    let before = std::fs::read_to_string(&rules).unwrap();
    let loctel = Loctel::spawn(&settings_path(&dir), &PORTS_ZERO);

    let line = rules_line(&loctel);

    assert_eq!(line["event"], "rules", "{line}");
    assert_eq!(
        line["count"], 2,
        "every rule counts, enabled or not: {line}"
    );
    assert_eq!(line["error"], serde_json::Value::Null, "{line}");
    assert_eq!(std::fs::read_to_string(&rules).unwrap(), before);
}

#[test]
fn headless_rules_line_reports_a_broken_file_and_keeps_running() {
    let dir = tempfile::tempdir().unwrap();
    let rules = dir.path().join("rules.toml");
    std::fs::write(&rules, "[[rule]\nname = ").unwrap();
    let mut loctel = Loctel::spawn(&settings_path(&dir), &PORTS_ZERO);

    let line = rules_line(&loctel);

    assert_eq!(line["event"], "rules", "{line}");
    assert_eq!(line["count"], 0, "{line}");
    let error = line["error"]
        .as_str()
        .expect("the error says what is wrong");
    assert!(error.contains("rules.toml"), "{error}");
    // The bad file is left as it was, and loctel runs.
    assert_eq!(std::fs::read_to_string(&rules).unwrap(), "[[rule]\nname = ");
    assert!(loctel.child.try_wait().unwrap().is_none());
    loctel.signal("TERM");
    assert_eq!(loctel.wait().code(), Some(0));
}

#[test]
fn headless_rules_file_defaults_to_beside_the_settings_file() {
    let dir = tempfile::tempdir().unwrap();
    let settings = settings_path(&dir);
    // No `--rules`: the file is next to the settings file given, so a run
    // with a scratch settings path never touches the real rules.
    let loctel = Loctel::spawn_exactly(&settings, &PORTS_ZERO);

    let line = rules_line(&loctel);

    assert_eq!(line["path"], rules_path_beside(&settings).to_str().unwrap());
    assert!(rules_path_beside(&settings).exists());
}

#[test]
fn an_unknown_flag_is_a_usage_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_loctel"))
        .arg("--no-such-flag")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(
        output.stdout.is_empty(),
        "nothing on stdout but listener lines"
    );
}
