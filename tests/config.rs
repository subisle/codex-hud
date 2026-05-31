use codex_hud::config::{default_config_path_from_env, Config};
use tempfile::tempdir;

#[test]
fn default_config_path_prefers_xdg_config_home() {
    let xdg = tempdir().unwrap();
    let home = tempdir().unwrap();

    let path = default_config_path_from_env(Some(xdg.path()), Some(home.path()));

    assert_eq!(path, xdg.path().join("codex-hud").join("config.toml"));
}

#[test]
fn default_config_path_falls_back_to_home_config() {
    let home = tempdir().unwrap();

    let path = default_config_path_from_env(None, Some(home.path()));

    assert_eq!(
        path,
        home.path()
            .join(".config")
            .join("codex-hud")
            .join("config.toml")
    );
}

#[test]
fn missing_config_file_returns_builtin_defaults() {
    let missing = tempdir().unwrap().path().join("config.toml");

    let config = Config::load_from_path(&missing).unwrap();

    assert_eq!(config, Config::default());
    assert_eq!(config.launcher.bridge_listen, "ws://127.0.0.1:4500");
    assert_eq!(config.launcher.expanded_rows, 3);
    assert!(config.launcher.auto_show_hud);
}

#[test]
fn toml_config_overrides_explicit_fields_and_keeps_defaults_for_missing_ones() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    std::fs::write(
        &config_path,
        r#"
[launcher]
status_rows = 7
surface = "fallback"
fallback_surface = "split"

[display]
show_goal = false
"#,
    )
    .unwrap();

    let config = Config::load_from_path(&config_path).unwrap();

    assert_eq!(config.launcher.status_rows, 7);
    assert_eq!(config.launcher.surface, "fallback");
    assert_eq!(config.launcher.fallback_surface, "split");
    assert!(!config.display.show_goal);
    assert!(config.launcher.enabled);
    assert_eq!(config.daemon.socket, "/tmp/codex-hud/app-server.sock");
}
