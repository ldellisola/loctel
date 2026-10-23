//! The command line, and how it combines with `settings.toml`.

use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use loctel_engine::{EngineConfig, RulesFile, Settings, default_spill_base};

/// loctel: receive OpenTelemetry traces and logs from services on this
/// machine, show which service failed, and answer AI agents over MCP.
#[derive(Debug, Parser)]
#[command(name = "loctel", version)]
pub struct Cli {
    /// Run without a window or menu bar item: serve OTLP and print one JSON
    /// line per listener to stdout, then run until SIGINT or SIGTERM.
    #[arg(long)]
    pub headless: bool,

    /// OTLP gRPC port, instead of the one in settings (default 4317). Port 0
    /// picks a free port.
    #[arg(long, value_name = "N")]
    pub grpc_port: Option<u16>,

    /// OTLP HTTP port, instead of the one in settings (default 4318).
    #[arg(long, value_name = "N")]
    pub http_port: Option<u16>,

    /// MCP port, instead of the one in settings (default 4319).
    #[arg(long, value_name = "N")]
    pub mcp_port: Option<u16>,

    /// Read settings from this file instead of the per-user one. The file is
    /// optional; flags win over what it says.
    #[arg(long, value_name = "PATH")]
    pub settings: Option<PathBuf>,

    /// Read and write the discard rules in this file instead of the default
    /// one: `rules.toml` next to the settings file (so the per-user
    /// `~/Library/Application Support/loctel/rules.toml` on a Mac, unless
    /// `--settings` names another file). A missing file is created holding no
    /// rules.
    #[arg(long, value_name = "PATH")]
    pub rules: Option<PathBuf>,

    /// How long a trace must go without a new span to count as complete, in
    /// milliseconds (default 10000). For tests.
    #[arg(long, value_name = "N")]
    pub completeness_ms: Option<u64>,

    /// Keep everything in memory: do not move traces and logs to temporary
    /// segment files. By default loctel spills to a `run-<pid>-<time>` directory
    /// in `loctel/` of the system's temporary directory, up to the disk cap of
    /// the settings, and deletes it when it quits.
    #[arg(long)]
    pub no_spill: bool,
}

impl Cli {
    /// The settings file to read: `--settings`, else the per-user one.
    pub fn settings_path(&self) -> PathBuf {
        self.settings.clone().unwrap_or_else(Settings::default_path)
    }

    /// The rules file: `--rules`, else `rules.toml` in the directory of the
    /// settings file (the per-user one unless `--settings` names another, so a
    /// run on a scratch settings file never touches the real rules).
    pub fn rules_path(&self) -> PathBuf {
        if let Some(path) = &self.rules {
            return path.clone();
        }
        match &self.settings {
            Some(settings) => settings.with_file_name("rules.toml"),
            None => RulesFile::default_path(),
        }
    }

    /// The engine's configuration: `settings` with the port flags applied on
    /// top, the rules file, the completeness flag on the store, and where the
    /// store spills (the run directory under the system's temporary directory,
    /// unless `--no-spill`).
    pub fn engine_config(&self, mut settings: Settings) -> EngineConfig {
        if let Some(port) = self.grpc_port {
            settings.otlp_grpc_port = port;
        }
        if let Some(port) = self.http_port {
            settings.otlp_http_port = port;
        }
        if let Some(port) = self.mcp_port {
            settings.mcp_port = port;
        }
        let mut config = EngineConfig {
            settings,
            rules_path: Some(self.rules_path()),
            spill_base: (!self.no_spill).then(default_spill_base),
            ..EngineConfig::default()
        };
        if let Some(millis) = self.completeness_ms {
            config.store.completeness_quiet = Duration::from_millis(millis);
        }
        config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("loctel").chain(args.iter().copied())).unwrap()
    }

    #[test]
    fn no_flags_leave_settings_and_store_defaults_alone() {
        let cli = parse(&[]);
        assert!(!cli.headless);

        let config = cli.engine_config(Settings::default());

        assert_eq!(config.settings, Settings::default());
        assert_eq!(config.store, EngineConfig::default().store);
        assert!(config.bind_ip.is_loopback());
    }

    #[test]
    fn spilling_is_the_default_and_no_spill_keeps_everything_in_memory() {
        // The app spills into a run directory under the system's temporary
        // directory (Milestone 5) ...
        let cli = parse(&[]);
        assert!(!cli.no_spill);
        assert_eq!(
            cli.engine_config(Settings::default()).spill_base,
            Some(std::env::temp_dir().join("loctel"))
        );

        // ... unless told not to, and neither changes the store's own limits.
        let cli = parse(&["--no-spill"]);
        assert!(cli.no_spill);
        let config = cli.engine_config(Settings::default());
        assert_eq!(config.spill_base, None);
        assert_eq!(config.store, EngineConfig::default().store);
    }

    #[test]
    fn the_disk_cap_comes_from_the_settings() {
        let settings = Settings {
            disk_cap_bytes: 7 * 1024 * 1024 * 1024,
            ..Settings::default()
        };
        let config = parse(&[]).engine_config(settings);
        assert_eq!(config.settings.disk_cap_bytes, 7 * 1024 * 1024 * 1024);
    }

    #[test]
    fn flags_win_over_settings() {
        let cli = parse(&[
            "--headless",
            "--grpc-port",
            "1",
            "--http-port",
            "2",
            "--mcp-port",
            "3",
            "--completeness-ms",
            "500",
        ]);
        assert!(cli.headless);
        let from_file = Settings {
            otlp_grpc_port: 9001,
            otlp_http_port: 9002,
            mcp_port: 9003,
            start_at_login: false,
            ..Settings::default()
        };

        let config = cli.engine_config(from_file);

        assert_eq!(config.settings.otlp_grpc_port, 1);
        assert_eq!(config.settings.otlp_http_port, 2);
        assert_eq!(config.settings.mcp_port, 3);
        // What no flag names comes from the file.
        assert!(!config.settings.start_at_login);
        assert_eq!(config.store.completeness_quiet, Duration::from_millis(500));
    }

    #[test]
    fn a_flag_names_only_its_own_port() {
        let cli = parse(&["--http-port", "0"]);
        let from_file = Settings {
            otlp_grpc_port: 9001,
            otlp_http_port: 9002,
            ..Settings::default()
        };

        let config = cli.engine_config(from_file);

        assert_eq!(config.settings.otlp_grpc_port, 9001);
        assert_eq!(config.settings.otlp_http_port, 0);
        assert_eq!(config.settings.mcp_port, Settings::default().mcp_port);
    }

    #[test]
    fn settings_flag_names_the_file() {
        assert_eq!(
            parse(&["--settings", "/tmp/x.toml"]).settings_path(),
            PathBuf::from("/tmp/x.toml")
        );
        assert_eq!(parse(&[]).settings_path(), Settings::default_path());
    }

    #[test]
    fn rules_flag_names_the_file() {
        let cli = parse(&["--rules", "/tmp/r/rules.toml"]);

        assert_eq!(cli.rules_path(), PathBuf::from("/tmp/r/rules.toml"));
        assert_eq!(
            cli.engine_config(Settings::default()).rules_path,
            Some(PathBuf::from("/tmp/r/rules.toml"))
        );
    }

    #[test]
    fn rules_file_defaults_to_the_per_user_one() {
        assert_eq!(parse(&[]).rules_path(), RulesFile::default_path());
        // It is next to the per-user settings file, as the design says.
        assert_eq!(
            RulesFile::default_path().parent(),
            Settings::default_path().parent()
        );
        assert_eq!(
            parse(&[]).engine_config(Settings::default()).rules_path,
            Some(RulesFile::default_path())
        );
    }

    #[test]
    fn rules_file_defaults_to_beside_a_named_settings_file() {
        // A scratch `--settings` must not send the rules to the real file.
        let cli = parse(&["--settings", "/tmp/scratch/settings.toml"]);

        assert_eq!(cli.rules_path(), PathBuf::from("/tmp/scratch/rules.toml"));
        // And `--rules` still wins.
        let cli = parse(&[
            "--settings",
            "/tmp/scratch/settings.toml",
            "--rules",
            "/tmp/elsewhere.toml",
        ]);
        assert_eq!(cli.rules_path(), PathBuf::from("/tmp/elsewhere.toml"));
    }

    #[test]
    fn ports_must_fit_in_sixteen_bits() {
        assert!(Cli::try_parse_from(["loctel", "--grpc-port", "70000"]).is_err());
        assert!(Cli::try_parse_from(["loctel", "--grpc-port", "-1"]).is_err());
        assert!(Cli::try_parse_from(["loctel", "--grpc-port", "four"]).is_err());
    }
}
