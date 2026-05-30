use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchDisposition {
    Intercept,
    Passthrough,
}

const PASSTHROUGH_COMMANDS: &[&str] = &[
    "exec",
    "review",
    "login",
    "logout",
    "mcp",
    "plugin",
    "mcp-server",
    "app-server",
    "remote-control",
    "completion",
    "update",
    "doctor",
    "sandbox",
    "debug",
    "apply",
    "cloud",
    "exec-server",
    "help",
    "app",
    "features",
];

const VALUE_OPTIONS: &[&str] = &[
    "-c",
    "--config",
    "--enable",
    "--disable",
    "--remote",
    "--remote-auth-token-env",
    "-i",
    "--image",
    "-m",
    "--model",
    "--local-provider",
    "-p",
    "--profile",
    "-s",
    "--sandbox",
    "-C",
    "--cd",
    "--add-dir",
    "-a",
    "--ask-for-approval",
];

const PASSTHROUGH_FLAGS: &[&str] = &[
    "-h",
    "--help",
    "-V",
    "--version",
    "--strict-config",
    "--dangerously-bypass-approvals-and-sandbox",
    "--dangerously-bypass-hook-trust",
    "--oss",
    "--search",
    "--no-alt-screen",
];

static UNIX_REMOTE_SUPPORT: OnceLock<bool> = OnceLock::new();

pub fn classify_launch(args: &[OsString]) -> LaunchDisposition {
    if args.iter().any(|arg| {
        matches!(
            arg.as_os_str().to_str(),
            Some("-h") | Some("--help") | Some("-V") | Some("--version")
        )
    }) {
        return LaunchDisposition::Passthrough;
    }

    let mut skip_next_value = false;
    let mut positional_only = false;
    for arg in args {
        let Some(token) = arg.to_str() else {
            continue;
        };

        if skip_next_value {
            skip_next_value = false;
            continue;
        }

        if positional_only {
            return if PASSTHROUGH_COMMANDS.contains(&token) {
                LaunchDisposition::Passthrough
            } else {
                LaunchDisposition::Intercept
            };
        }

        if token == "--" {
            positional_only = true;
            continue;
        }

        if token.starts_with('-') {
            if token.contains('=') {
                continue;
            }

            if PASSTHROUGH_FLAGS.contains(&token) {
                continue;
            }

            if VALUE_OPTIONS.contains(&token) {
                skip_next_value = true;
            }
            continue;
        }

        return if PASSTHROUGH_COMMANDS.contains(&token) {
            LaunchDisposition::Passthrough
        } else {
            LaunchDisposition::Intercept
        };
    }

    LaunchDisposition::Intercept
}

pub fn find_real_codex_in_path(path_env: &OsStr, current_exe: &Path) -> Option<PathBuf> {
    let current_canonical = fs::canonicalize(current_exe).ok();

    for dir in std::env::split_paths(path_env) {
        let candidate = dir.join("codex");
        if !is_executable(&candidate) {
            continue;
        }

        if same_binary(&candidate, current_exe, current_canonical.as_deref()) {
            continue;
        }

        return Some(candidate);
    }

    None
}

pub fn supports_unix_remote_help(help_text: &str) -> bool {
    help_text.contains("unix://")
}

pub fn cached_unix_remote_support(codex_path: &Path) -> bool {
    *UNIX_REMOTE_SUPPORT.get_or_init(|| probe_unix_remote_support(codex_path))
}

pub fn probe_unix_remote_support(codex_path: &Path) -> bool {
    let Ok(output) = Command::new(codex_path).arg("--help").output() else {
        return false;
    };

    let mut help_text = String::new();
    help_text.push_str(&String::from_utf8_lossy(&output.stdout));
    help_text.push_str(&String::from_utf8_lossy(&output.stderr));
    supports_unix_remote_help(&help_text)
}

fn same_binary(candidate: &Path, current_exe: &Path, current_canonical: Option<&Path>) -> bool {
    if candidate == current_exe {
        return true;
    }

    let candidate_canonical = fs::canonicalize(candidate).ok();
    match (candidate_canonical.as_deref(), current_canonical) {
        (Some(candidate), Some(current)) => candidate == current,
        _ => false,
    }
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => metadata.permissions().mode() & 0o111 != 0,
        _ => false,
    }
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    match fs::metadata(path) {
        Ok(metadata) => metadata.is_file(),
        _ => false,
    }
}
