//! `settings.toml`, the only thing loctel writes to disk besides discard
//! rules (design §5: telemetry is never persisted).

use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

/// What the user can configure. Every key is optional in the file: a missing
/// one takes its default, and a key this version does not know is ignored, so
/// a file written by a newer loctel still loads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// OTLP gRPC port (design §4).
    pub otlp_grpc_port: u16,
    /// OTLP HTTP port.
    pub otlp_http_port: u16,
    /// MCP port. MCP always listens on loopback.
    pub mcp_port: u16,
    /// How much telemetry may be spilled to disk before the oldest goes
    /// (design §5): the engine gives it to the store at start
    /// ([`Engine::start`](crate::Engine::start)), and
    /// [`EngineHandle::set_disk_cap`](crate::EngineHandle::set_disk_cap) applies a
    /// new one while loctel runs. 4 GiB by default; the Settings window takes 1 to
    /// 100 GB.
    pub disk_cap_bytes: u64,
    /// Start loctel when the user logs in (design §9.1).
    pub start_at_login: bool,
    /// Whether the first-launch welcome has been shown.
    pub first_launch_done: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            otlp_grpc_port: 4317,
            otlp_http_port: 4318,
            mcp_port: 4319,
            disk_cap_bytes: 4 * 1024 * 1024 * 1024,
            start_at_login: true,
            first_launch_done: false,
        }
    }
}

impl Settings {
    /// `<config dir>/loctel/settings.toml`. On macOS that is
    /// `~/Library/Application Support/loctel/settings.toml`.
    ///
    /// A machine with no home directory has no config directory; the
    /// system's temporary directory stands in, so that loctel still runs
    /// (and a read-only home cannot stop it from starting).
    pub fn default_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("loctel")
            .join("settings.toml")
    }

    /// Reads the settings file. A file that does not exist is not an error:
    /// it gives the defaults, which is what a first launch looks like. A
    /// file that cannot be read or is not valid settings is an error that
    /// names the file, and is never rewritten by this call.
    pub fn load(path: &Path) -> anyhow::Result<Settings> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Settings::default()),
            Err(e) => {
                return Err(e).with_context(|| format!("cannot read {}", path.display()));
            }
        };
        toml::from_str(&text).with_context(|| format!("invalid settings in {}", path.display()))
    }

    /// [`Settings::load`], with the defaults for a file that cannot be read
    /// or parsed. A broken settings file must not stop the app from
    /// starting; callers that want to tell the user use `load`.
    pub fn load_or_default(path: &Path) -> Settings {
        Settings::load(path).unwrap_or_default()
    }

    /// Writes the settings, creating the directories as needed. The file is
    /// written whole beside the target and then renamed over it, so a crash
    /// leaves the old file or the new one, never half of one.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let text = toml::to_string_pretty(self).context("cannot serialize the settings")?;
        let dir = path
            .parent()
            .filter(|dir| !dir.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;

        let name = path
            .file_name()
            .with_context(|| format!("{} does not name a file", path.display()))?;
        let mut temporary = name.to_os_string();
        temporary.push(".tmp");
        let temporary = dir.join(temporary);

        let written = fs::File::create(&temporary).and_then(|mut file| {
            file.write_all(text.as_bytes())?;
            file.sync_all()
        });
        written
            .and_then(|()| fs::rename(&temporary, path))
            .inspect_err(|_| {
                let _ = fs::remove_file(&temporary);
            })
            .with_context(|| format!("cannot write {}", path.display()))
    }
}
