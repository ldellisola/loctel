//! `settings.toml`: defaults, partial files, round trips and bad input.

use std::fs;

use loctel_engine::Settings;

const GIB: u64 = 1024 * 1024 * 1024;

#[test]
fn settings_defaults_when_missing() {
    let dir = tempfile::tempdir().unwrap();
    let settings = Settings::load_or_default(&dir.path().join("settings.toml"));

    assert_eq!(settings, Settings::default());
    assert_eq!(settings.otlp_grpc_port, 4317);
    assert_eq!(settings.otlp_http_port, 4318);
    assert_eq!(settings.mcp_port, 4319);
    assert_eq!(settings.disk_cap_bytes, 4 * GIB);
    assert!(settings.start_at_login);
    assert!(!settings.first_launch_done);
}

#[test]
fn settings_partial_toml_fills_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    fs::write(&path, "otlp_http_port = 9999\nstart_at_login = false\n").unwrap();

    let settings = Settings::load_or_default(&path);

    assert_eq!(settings.otlp_http_port, 9999);
    assert!(!settings.start_at_login);
    // Everything the file does not mention keeps its default.
    assert_eq!(
        settings,
        Settings {
            otlp_http_port: 9999,
            start_at_login: false,
            ..Settings::default()
        }
    );
}

#[test]
fn settings_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    // The directories are created as needed.
    let path = dir
        .path()
        .join("nested")
        .join("loctel")
        .join("settings.toml");
    let settings = Settings {
        otlp_grpc_port: 14317,
        otlp_http_port: 14318,
        mcp_port: 14319,
        disk_cap_bytes: 8 * GIB,
        start_at_login: false,
        first_launch_done: true,
    };

    settings.save(&path).unwrap();

    assert_eq!(Settings::load_or_default(&path), settings);
    assert_eq!(Settings::load(&path).unwrap(), settings);
}

#[test]
fn save_replaces_an_existing_file_and_leaves_nothing_else_behind() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    Settings::default().save(&path).unwrap();
    let changed = Settings {
        mcp_port: 5000,
        ..Settings::default()
    };

    changed.save(&path).unwrap();

    assert_eq!(Settings::load_or_default(&path), changed);
    let names: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(
        names,
        ["settings.toml"],
        "no temporary file may stay behind"
    );
}

#[test]
fn unknown_keys_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    // A key from a newer loctel, next to a known one.
    fs::write(&path, "theme = \"light\"\nmcp_port = 5000\n").unwrap();

    let settings = Settings::load(&path).unwrap();

    assert_eq!(settings.mcp_port, 5000);
}

#[test]
fn invalid_file_falls_back_to_defaults_but_load_reports_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    for bad in [
        "this is not toml",
        "otlp_grpc_port = \"four\"",
        "otlp_grpc_port = 70000",
    ] {
        fs::write(&path, bad).unwrap();

        assert_eq!(
            Settings::load_or_default(&path),
            Settings::default(),
            "{bad}"
        );
        let error = Settings::load(&path).unwrap_err();
        assert!(
            format!("{error:#}").contains("settings.toml"),
            "the error names the file: {error:#}"
        );
        // Looking at a bad file never rewrites it.
        assert_eq!(fs::read_to_string(&path).unwrap(), bad);
    }
}

#[test]
fn a_directory_in_place_of_the_file_falls_back_to_defaults() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(Settings::load_or_default(dir.path()), Settings::default());
    assert!(Settings::load(dir.path()).is_err());
}

#[test]
fn load_of_a_missing_file_is_the_defaults_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let settings = Settings::load(&dir.path().join("absent.toml")).unwrap();
    assert_eq!(settings, Settings::default());
}

#[test]
fn default_path_is_loctel_settings_toml() {
    let path = Settings::default_path();
    assert!(path.is_absolute(), "{}", path.display());
    assert!(path.ends_with("loctel/settings.toml"), "{}", path.display());
}
