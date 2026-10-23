//! The window's backends over the engine and over a fixed store (M4 plan,
//! Task 2): `AppBackend` saves `settings.toml` and rebinds the engine's
//! listeners, passes the rules API and pause through, and holds the login item
//! it was given; `FixedBackend` answers the same calls in memory.

use std::cell::Cell;
use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use loctel_engine::{
    Engine, EngineConfig, EngineHandle, ListenerState, Preview, RuleId, RuleLanguage, RuleSpec,
    Settings,
};
use loctel_ingest::{logs_from_request, spans_from_request};
use loctel_meshgen::{Scenario, generate};
use loctel_rules::DropScope;
use loctel_store::{ManualClock, Store, StoreConfig};
use loctel_ui::app::{AppBackend, LoginItem, UiBackend};
use loctel_ui::testing::FixedBackend;

const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
const GIB: u64 = 1024 * 1024 * 1024;

/// 2027-01-15T08:00:00Z.
const BASE: u64 = 1_800_000_000_000_000_000;

/// `N` different ports nothing listens on right now (all held at once, so the
/// system cannot hand out the same one twice).
fn free_ports<const N: usize>() -> [u16; N] {
    let held: Vec<TcpListener> = (0..N)
        .map(|_| TcpListener::bind((LOOPBACK, 0)).unwrap())
        .collect();
    let mut ports = [0; N];
    for (port, listener) in ports.iter_mut().zip(&held) {
        *port = listener.local_addr().unwrap().port();
    }
    ports
}

/// An engine on port 0 everywhere, with its rules file in `dir`.
fn engine(dir: &std::path::Path) -> EngineHandle {
    Engine::start(EngineConfig {
        settings: Settings {
            otlp_grpc_port: 0,
            otlp_http_port: 0,
            mcp_port: 0,
            ..Settings::default()
        },
        rules_path: Some(dir.join("rules.toml")),
        ..EngineConfig::default()
    })
    .expect("the engine starts")
}

fn port_of(backend: &dyn UiBackend, name: &str) -> u16 {
    let listener = backend
        .status()
        .listeners
        .into_iter()
        .find(|l| l.name == name)
        .unwrap_or_else(|| panic!("no listener named {name}"));
    match listener.state {
        ListenerState::Listening(address) => address.port(),
        ListenerState::Failed(why) => panic!("{name} failed: {why}"),
    }
}

fn accepts(port: u16) -> bool {
    TcpStream::connect_timeout(&SocketAddr::new(LOOPBACK, port), Duration::from_secs(1)).is_ok()
}

/// A login item that is only a flag.
#[derive(Default)]
struct FakeLogin {
    on: Cell<bool>,
}

impl LoginItem for FakeLogin {
    fn is_enabled(&self) -> bool {
        self.on.get()
    }

    fn set_enabled(&self, on: bool) -> anyhow::Result<()> {
        self.on.set(on);
        Ok(())
    }
}

fn trace_rule(name: &str, query: &str) -> RuleSpec {
    RuleSpec {
        name: name.to_owned(),
        enabled: true,
        language: RuleLanguage::TraceQl,
        query: query.to_owned(),
        scope: DropScope::Trace,
    }
}

fn log_rule(name: &str, query: &str) -> RuleSpec {
    RuleSpec {
        language: RuleLanguage::LogQl,
        ..trace_rule(name, query)
    }
}

/// A store holding the failing checkout (15 spans, 7 logs, 3 of them errors),
/// dated now on a clock that stands still.
fn checkout_store() -> Store {
    let clock = Arc::new(ManualClock::new(BASE + 70_000_000_000));
    let store = Store::new(StoreConfig::default(), clock);
    for batch in &generate(&Scenario::CheckoutFailure, 7, BASE).batches {
        if let Some(request) = &batch.traces {
            store.ingest_spans(spans_from_request(request.clone()).items);
        }
        if let Some(request) = &batch.logs {
            store.ingest_logs(logs_from_request(request.clone()).items);
        }
    }
    store
}

#[test]
fn app_backend_saves_settings_and_rebinds() {
    let dir = tempfile::tempdir().unwrap();
    let settings_path = dir.path().join("settings.toml");
    let backend = AppBackend::new(engine(dir.path()), settings_path.clone(), None);

    // Nothing is saved yet: the settings are the defaults, except the ports,
    // which are the ones the listeners use (port 0 here, so the system's).
    let old_http = port_of(&backend, "otlp-http");
    assert_eq!(
        backend.settings(),
        Settings {
            otlp_grpc_port: port_of(&backend, "otlp-grpc"),
            otlp_http_port: old_http,
            mcp_port: port_of(&backend, "mcp"),
            ..Settings::default()
        }
    );

    // New ports are saved, whole (what the form does not touch is kept), and
    // the listeners move at once.
    let [grpc, http, mcp] = free_ports();
    let wanted = Settings {
        otlp_grpc_port: grpc,
        otlp_http_port: http,
        mcp_port: mcp,
        disk_cap_bytes: 8 * GIB,
        start_at_login: false,
        first_launch_done: true,
    };
    let statuses = backend.apply_settings(wanted.clone()).unwrap();

    assert_eq!(Settings::load(&settings_path).unwrap(), wanted);
    assert_eq!(backend.settings(), wanted);
    let names: Vec<_> = statuses.iter().map(|l| l.name).collect();
    assert_eq!(names, ["otlp-grpc", "otlp-http", "mcp"]);
    assert_eq!(backend.status().listeners, statuses);
    assert_eq!(port_of(&backend, "otlp-grpc"), grpc);
    assert_eq!(port_of(&backend, "otlp-http"), http);
    assert_eq!(port_of(&backend, "mcp"), mcp);
    assert_eq!(
        backend.mcp_url(),
        Some(format!("http://127.0.0.1:{mcp}/mcp"))
    );
    assert!(accepts(http));
    assert_ne!(old_http, http);

    // A port in use is saved all the same, and the listener says Failed.
    let busy = TcpListener::bind((LOOPBACK, 0)).unwrap();
    let busy_port = busy.local_addr().unwrap().port();
    let taken = Settings {
        otlp_http_port: busy_port,
        ..wanted.clone()
    };
    let statuses = backend.apply_settings(taken.clone()).unwrap();

    assert_eq!(backend.settings(), taken);
    assert_eq!(Settings::load(&settings_path).unwrap(), taken);
    let http_status = statuses.iter().find(|l| l.name == "otlp-http").unwrap();
    assert!(
        matches!(&http_status.state, ListenerState::Failed(why) if !why.is_empty()),
        "{http_status:?}"
    );
    // The others keep running.
    assert_eq!(port_of(&backend, "otlp-grpc"), grpc);
    assert_eq!(port_of(&backend, "mcp"), mcp);
    drop(busy);
}

#[test]
fn apply_settings_moves_the_disk_cap() {
    let dir = tempfile::tempdir().unwrap();
    let settings_path = dir.path().join("settings.toml");
    let backend = AppBackend::new(engine(dir.path()), settings_path.clone(), None);
    assert_eq!(backend.status().disk_cap_bytes, 4 * GIB, "the default cap");

    // Apply saves the new cap and the store keeps to it from then on.
    let wanted = Settings {
        disk_cap_bytes: 2 * GIB,
        ..backend.settings()
    };
    backend.apply_settings(wanted.clone()).unwrap();

    assert_eq!(
        Settings::load(&settings_path).unwrap().disk_cap_bytes,
        2 * GIB
    );
    assert_eq!(backend.status().disk_cap_bytes, 2 * GIB);
    assert_eq!(backend.store().disk_cap_bytes(), 2 * GIB);

    // A cap that cannot be saved is not applied.
    let blocker = dir.path().join("blocker");
    fs::write(&blocker, "not a directory").unwrap();
    let stuck = AppBackend::new(engine(dir.path()), blocker.join("settings.toml"), None);
    assert!(
        stuck
            .apply_settings(Settings {
                disk_cap_bytes: GIB,
                ..Settings::default()
            })
            .is_err()
    );
    assert_eq!(stuck.status().disk_cap_bytes, 4 * GIB);
}

#[test]
fn settings_show_the_ports_in_use_and_apply_moves_only_the_changed_one() {
    let dir = tempfile::tempdir().unwrap();
    let settings_path = dir.path().join("settings.toml");
    // The file says the default ports; the engine was started on others, as
    // the command-line port flags do.
    Settings::default().save(&settings_path).unwrap();
    let backend = AppBackend::new(engine(dir.path()), settings_path, None);
    let grpc = port_of(&backend, "otlp-grpc");
    let mcp = port_of(&backend, "mcp");

    let shown = backend.settings();
    assert_eq!(shown.otlp_grpc_port, grpc);
    assert_eq!(shown.otlp_http_port, port_of(&backend, "otlp-http"));
    assert_eq!(shown.mcp_port, mcp);

    // Changing only the HTTP port moves only the HTTP listener.
    let [http] = free_ports();
    backend
        .apply_settings(Settings {
            otlp_http_port: http,
            ..shown
        })
        .unwrap();
    assert_eq!(port_of(&backend, "otlp-http"), http);
    assert_eq!(port_of(&backend, "otlp-grpc"), grpc, "gRPC did not move");
    assert_eq!(port_of(&backend, "mcp"), mcp, "MCP did not move");
}

#[test]
fn app_backend_keeps_the_ports_when_the_settings_cannot_be_saved() {
    let dir = tempfile::tempdir().unwrap();
    // A file where the settings' directory should be: saving fails.
    let blocker = dir.path().join("blocker");
    fs::write(&blocker, "not a directory").unwrap();
    let backend = AppBackend::new(engine(dir.path()), blocker.join("settings.toml"), None);
    let before = backend.status().listeners;
    let [grpc, http, mcp] = free_ports();

    let result = backend.apply_settings(Settings {
        otlp_grpc_port: grpc,
        otlp_http_port: http,
        mcp_port: mcp,
        ..Settings::default()
    });

    assert!(result.is_err());
    // Nothing was saved, so nothing moved.
    assert_eq!(backend.status().listeners, before);
}

#[test]
fn app_backend_passes_the_rules_through() {
    let dir = tempfile::tempdir().unwrap();
    let backend = AppBackend::new(engine(dir.path()), dir.path().join("settings.toml"), None);
    assert!(backend.rules().is_empty());
    let first = backend.rules_generation();

    backend
        .set_rules(vec![
            trace_rule("Errors", "{ status = error }"),
            log_rule("Noise", r#"{service_name="x"} |= "noise""#),
        ])
        .unwrap();

    let rules = backend.rules();
    assert_eq!(rules.len(), 2);
    assert_eq!(rules[0].spec.name, "Errors");
    assert_eq!(rules[1].spec.language, RuleLanguage::LogQl);
    assert!(backend.rules_generation() > first);
    assert_eq!(backend.status().rules, rules);

    let after_set = backend.rules_generation();
    backend.set_rule_enabled(rules[1].id, false).unwrap();
    assert!(!backend.rules()[1].spec.enabled);
    assert!(backend.rules()[0].spec.enabled);
    assert!(backend.rules_generation() > after_set);
    assert!(backend.set_rule_enabled(RuleId(u64::MAX), true).is_err());

    // The preview reads the engine's store; an empty one drops nothing, and
    // a query that does not parse is the parser's error.
    assert_eq!(
        backend
            .preview_rule(&trace_rule("Errors", "{ status = error }"))
            .unwrap(),
        Preview::default()
    );
    let error = backend
        .preview_rule(&trace_rule("Typo", r#"{ nme = "x" }"#))
        .unwrap_err();
    assert!(error.message.contains("unknown intrinsic"), "{error}");
}

#[test]
fn app_backend_passes_pause_and_the_store_through() {
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(dir.path());
    let backend = AppBackend::new(engine.clone(), dir.path().join("settings.toml"), None);
    assert!(!backend.status().paused);

    backend.set_paused(true);
    assert!(engine.is_paused());
    assert!(backend.status().paused);
    backend.set_paused(false);
    assert!(!engine.is_paused());

    // The store is the engine's, and Clear all empties it.
    assert_eq!(backend.store().stats(), engine.store().stats());
    assert!(backend.mcp_url().is_some());
    backend.clear_all();
    assert_eq!(engine.store().stats().spans, 0);
}

#[test]
fn app_backend_holds_the_login_item_it_was_given() {
    let dir = tempfile::tempdir().unwrap();
    let none = AppBackend::new(engine(dir.path()), dir.path().join("settings.toml"), None);
    assert!(none.login_item().is_none(), "Linux has no login item");

    let login = Rc::new(FakeLogin::default());
    let some = AppBackend::new(
        engine(dir.path()),
        dir.path().join("settings.toml"),
        Some(login.clone()),
    );
    let item = some.login_item().expect("the item it was given");
    assert!(!item.is_enabled());
    item.set_enabled(true).unwrap();
    assert!(login.is_enabled(), "it is the same item");
    assert!(some.login_item().unwrap().is_enabled());
}

#[test]
fn fixed_backend_keeps_rules_in_memory() {
    let backend = FixedBackend::new(checkout_store());
    assert!(backend.rules().is_empty());
    let first = backend.rules_generation();

    backend
        .set_rules(vec![
            trace_rule("Errors", "{ status = error }"),
            log_rule("Noise", r#"{service_name="x"} |= "noise""#),
        ])
        .unwrap();

    let rules = backend.rules();
    assert_eq!(rules.len(), 2);
    assert_eq!(rules[0].spec.name, "Errors");
    assert!(rules[0].error.is_none());
    assert!(backend.rules_generation() > first);

    // A rule that is the same keeps its ID; a switch is a change too.
    let after_set = backend.rules_generation();
    backend.set_rule_enabled(rules[0].id, false).unwrap();
    let switched = backend.rules();
    assert_eq!(switched[0].id, rules[0].id);
    assert!(!switched[0].spec.enabled);
    assert!(switched[1].spec.enabled);
    assert!(backend.rules_generation() > after_set);
    assert!(backend.set_rule_enabled(RuleId(u64::MAX), true).is_err());

    // A draft that does not parse is kept, with its error.
    backend
        .set_rules(vec![trace_rule("Typo", r#"{ nme = "x" }"#)])
        .unwrap();
    assert!(backend.rules()[0].error.is_some());
}

#[test]
fn fixed_backend_previews_over_its_store() {
    let backend = FixedBackend::new(checkout_store());

    // The failing checkout, whole: 15 spans, 7 logs, 3 of them errors.
    assert_eq!(
        backend
            .preview_rule(&trace_rule("Errors", "{ status = error }"))
            .unwrap(),
        Preview {
            traces: 1,
            spans: 15,
            logs: 7,
            error_traces: 1,
            error_logs: 3,
        }
    );
    assert_eq!(
        backend
            .preview_rule(&log_rule("Errors", r#"{level="error"}"#))
            .unwrap(),
        Preview {
            logs: 3,
            error_logs: 3,
            ..Preview::default()
        }
    );
    let error = backend
        .preview_rule(&trace_rule("Typo", r#"{ nme = "x" }"#))
        .unwrap_err();
    assert!(error.message.contains("unknown intrinsic"), "{error}");
}

#[test]
fn fixed_backend_keeps_settings_pause_and_login_in_memory() {
    let backend = FixedBackend::new(checkout_store());
    assert_eq!(backend.settings(), Settings::default());
    assert!(backend.login_item().is_none());
    assert!(backend.mcp_url().is_some_and(|url| url.ends_with("/mcp")));

    // Applying settings keeps them, and the listeners follow the ports.
    let [grpc, http, mcp] = free_ports();
    let wanted = Settings {
        otlp_grpc_port: grpc,
        otlp_http_port: http,
        mcp_port: mcp,
        ..Settings::default()
    };
    let statuses = backend.apply_settings(wanted.clone()).unwrap();
    assert_eq!(backend.settings(), wanted);
    assert_eq!(statuses.len(), 3);
    assert_eq!(backend.status().listeners, statuses);
    assert_eq!(port_of(&backend, "otlp-http"), http);
    assert_eq!(
        backend.mcp_url(),
        Some(format!("http://127.0.0.1:{mcp}/mcp"))
    );

    // Pause shows in the status.
    assert!(!backend.status().paused);
    backend.set_paused(true);
    assert!(backend.status().paused);
    backend.set_paused(false);
    assert!(!backend.status().paused);
}
