use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub daemon: DaemonConfig,
    pub launcher: LauncherConfig,
    pub display: DisplayConfig,
    pub keymap: KeymapConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DaemonConfig {
    pub socket: String,
    pub auto_start: bool,
    pub reuse_shared_daemon: bool,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            socket: "/tmp/codex-hud/app-server.sock".to_string(),
            auto_start: true,
            reuse_shared_daemon: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LauncherConfig {
    pub enabled: bool,
    pub auto_show_hud: bool,
    pub surface: String,
    pub fallback_surface: String,
    pub bridge_listen: String,
    pub status_rows: u16,
    pub expanded_rows: u16,
}

impl Default for LauncherConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_show_hud: true,
            surface: "inline-statusbar".to_string(),
            fallback_surface: "split".to_string(),
            bridge_listen: "ws://127.0.0.1:4500".to_string(),
            status_rows: 2,
            expanded_rows: 3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplayConfig {
    pub mode: String,
    pub default_preset: String,
    pub visible_sections: Vec<String>,
    pub show_account: bool,
    pub show_goal: bool,
    pub show_compaction: bool,
    pub show_mcp_calls: bool,
    pub settings_enabled: bool,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            mode: "compact".to_string(),
            default_preset: "operator".to_string(),
            visible_sections: vec![
                "model".to_string(),
                "cwd".to_string(),
                "git_project".to_string(),
                "git".to_string(),
                "thread".to_string(),
                "turn".to_string(),
                "context".to_string(),
                "rate".to_string(),
            ],
            show_account: false,
            show_goal: true,
            show_compaction: true,
            show_mcp_calls: true,
            settings_enabled: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeymapConfig {
    pub toggle_mode: String,
    pub toggle_git: String,
    pub toggle_usage: String,
    pub toggle_context: String,
    pub toggle_plan: String,
    pub toggle_mcp: String,
    pub toggle_debug: String,
    pub open_settings: String,
}

impl Default for KeymapConfig {
    fn default() -> Self {
        Self {
            toggle_mode: "tab".to_string(),
            toggle_git: "3".to_string(),
            toggle_usage: "2".to_string(),
            toggle_context: "2".to_string(),
            toggle_plan: "5".to_string(),
            toggle_mcp: "6".to_string(),
            toggle_debug: "7".to_string(),
            open_settings: "s".to_string(),
        }
    }
}

impl Config {
    pub fn load_from_path(path: &Path) -> io::Result<Self> {
        match fs::read_to_string(path) {
            Ok(contents) => toml::from_str(&contents)
                .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err)),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(err),
        }
    }

    pub fn load_from_env() -> io::Result<Self> {
        let xdg_config_home = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
        let home_dir = std::env::var_os("HOME").map(PathBuf::from);
        let path = default_config_path_from_env(xdg_config_home.as_deref(), home_dir.as_deref());
        Self::load_from_path(&path)
    }
}

pub fn default_config_path_from_env(
    xdg_config_home: Option<&Path>,
    home_dir: Option<&Path>,
) -> PathBuf {
    if let Some(xdg_config_home) = xdg_config_home {
        return xdg_config_home.join("codex-hud").join("config.toml");
    }

    if let Some(home_dir) = home_dir {
        return home_dir
            .join(".config")
            .join("codex-hud")
            .join("config.toml");
    }

    PathBuf::from(".config")
        .join("codex-hud")
        .join("config.toml")
}
