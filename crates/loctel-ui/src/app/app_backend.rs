//! The real [`UiBackend`]: the engine, the settings file and, where the
//! platform has one, the login item.

use std::path::PathBuf;
use std::rc::Rc;

use loctel_engine::{EngineHandle, EngineStatus, ListenerState, ListenerStatus, Ports, Settings};
use loctel_query::QueryError;
use loctel_rules::{Preview, RuleId, RuleSpec, RuleStatus};
use loctel_store::Store;

use super::backend::UiBackend;
use crate::model::format;

/// Start at login, as the platform does it. macOS implements it over
/// `SMAppService` (`loctel_macos::login_item::SystemLoginItem`); there is none
/// on Linux.
///
/// The system is the source of truth: the user can switch the item off in
/// System Settings, so [`is_enabled`](LoginItem::is_enabled) asks every time and
/// nothing mirrors it.
pub trait LoginItem {
    /// Whether the app is registered to start at login and allowed to.
    fn is_enabled(&self) -> bool;
    /// Registers (`true`) or unregisters (`false`) the app as a login item. The
    /// error says why the system refused.
    fn set_enabled(&self, on: bool) -> anyhow::Result<()>;
}

/// [`UiBackend`] over the running engine. It lives on GPUI's foreground thread
/// (the login item is an `Rc`), and holds:
///
/// - the engine, for the store, the status, pause, the rules and the listeners
/// - the path of `settings.toml`, which [`settings`](UiBackend::settings) reads
///   and [`apply_settings`](UiBackend::apply_settings) writes. It reads the
///   file, not the engine's own settings: a command-line port flag (`--http-port
///   0`) changes what the engine listens on, not the file
/// - the login item, `None` on platforms that have none
pub struct AppBackend {
    engine: EngineHandle,
    settings_path: PathBuf,
    login: Option<Rc<dyn LoginItem>>,
}

impl AppBackend {
    pub fn new(
        engine: EngineHandle,
        settings_path: PathBuf,
        login: Option<Rc<dyn LoginItem>>,
    ) -> AppBackend {
        AppBackend {
            engine,
            settings_path,
            login,
        }
    }
}

impl UiBackend for AppBackend {
    fn store(&self) -> Store {
        self.engine.store().clone()
    }

    fn status(&self) -> EngineStatus {
        self.engine.status()
    }

    fn clear_all(&self) {
        self.engine.store().clear();
    }

    fn utc_offset(&self) -> i32 {
        format::local_utc_offset()
    }

    fn rules(&self) -> Vec<RuleStatus> {
        self.engine.rules()
    }

    fn set_rules(&self, specs: Vec<RuleSpec>) -> anyhow::Result<()> {
        self.engine.set_rules(specs)
    }

    fn set_rule_enabled(&self, id: RuleId, on: bool) -> anyhow::Result<()> {
        self.engine.set_rule_enabled(id, on)
    }

    fn preview_rule(&self, spec: &RuleSpec) -> Result<Preview, QueryError> {
        self.engine.preview_rule(spec)
    }

    fn rules_generation(&self) -> u64 {
        self.engine.rules_generation()
    }

    /// `settings.toml`, with the ports the listeners use now. Those are the
    /// file's, except when the command line overrode them for this run
    /// (`--grpc-port` and the others, as the docs' Xvfb runs do): then the
    /// Settings window shows where loctel really listens, and Apply moves only
    /// the port that was changed (`EngineHandle::rebind` leaves a listener
    /// that already answers on its port alone).
    fn settings(&self) -> Settings {
        let mut settings = Settings::load_or_default(&self.settings_path);
        for listener in self.engine.status().listeners {
            let port = match listener.state {
                ListenerState::Listening(address) => address.port(),
                ListenerState::Failed(_) => listener.requested.port(),
            };
            match listener.name {
                "otlp-grpc" => settings.otlp_grpc_port = port,
                "otlp-http" => settings.otlp_http_port = port,
                "mcp" => settings.mcp_port = port,
                _ => {}
            }
        }
        settings
    }

    fn apply_settings(&self, s: Settings) -> anyhow::Result<Vec<ListenerStatus>> {
        // Saved first: a port that cannot be bound is still the user's choice,
        // and a file that cannot be written must not move anything.
        s.save(&self.settings_path)?;
        // The disk cap applies at once: a lower one deletes the oldest data now.
        self.engine.set_disk_cap(s.disk_cap_bytes);
        Ok(self.engine.rebind(Ports::from(&s)))
    }

    fn login_item(&self) -> Option<Rc<dyn LoginItem>> {
        self.login.clone()
    }

    fn set_paused(&self, on: bool) {
        self.engine.set_paused(on);
    }

    fn mcp_url(&self) -> Option<String> {
        self.engine.mcp_url()
    }
}
