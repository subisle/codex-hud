use std::ffi::OsString;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use crate::bridge::LocalWsBridgeHandle;
use crate::config::{default_daemon_socket, Config};
use crate::pty::{launcher_env_entries, LauncherEnvironment};
use crate::wrapper::{build_remote_launch_args, cached_unix_remote_support};

pub struct PreparedLaunch {
    pub forwarded_args: Vec<OsString>,
    pub hud_events: Option<Receiver<serde_json::Value>>,
    _bridge: Option<LocalWsBridgeHandle>,
    _runtime: Option<tokio::runtime::Runtime>,
}

pub fn prepare_remote_launch(
    real_codex: &Path,
    original_args: &[OsString],
    config: &Config,
    allow_ws_bridge: bool,
) -> std::io::Result<PreparedLaunch> {
    let socket_path = daemon_socket_path(&config.daemon.socket);

    if allow_ws_bridge {
        start_app_server_without_waiting(real_codex, &socket_path, config)?;
        let runtime = tokio::runtime::Runtime::new().map_err(io_error)?;
        let (hud_events_tx, hud_events_rx) = mpsc::channel();
        let bridge = runtime
            .block_on(crate::bridge::spawn_local_ws_bridge_with_observer(
                &socket_path,
                Some(hud_events_tx),
            ))
            .map_err(io_error)?;
        let remote_url = OsString::from(bridge.local_url());

        return Ok(PreparedLaunch {
            forwarded_args: build_remote_launch_args(original_args, remote_url),
            hud_events: Some(hud_events_rx),
            _bridge: Some(bridge),
            _runtime: Some(runtime),
        });
    }

    let unix_remote_supported = cached_unix_remote_support(real_codex);
    ensure_app_server(real_codex, &socket_path, config)?;

    if unix_remote_supported {
        let remote_url = OsString::from(format!("unix://{}", socket_path.display()));
        return Ok(PreparedLaunch {
            forwarded_args: build_remote_launch_args(original_args, remote_url),
            hud_events: None,
            _bridge: None,
            _runtime: None,
        });
    }

    Err(std::io::Error::other(
        "loopback ws bridge requires the wrapper process to remain active",
    ))
}

fn start_app_server_without_waiting(
    real_codex: &Path,
    socket_path: &Path,
    config: &Config,
) -> std::io::Result<()> {
    if socket_path.exists() && !socket_path_is_socket(socket_path) {
        if config.daemon.auto_start {
            match fs::remove_file(socket_path) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(err),
            }
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!(
                    "app-server socket path exists but is not a socket: {}",
                    socket_path.display()
                ),
            ));
        }
    }

    if socket_path.exists()
        && socket_path_is_socket(socket_path)
        && config.daemon.reuse_shared_daemon
    {
        return Ok(());
    }

    if !config.daemon.auto_start {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!(
                "app-server socket does not exist: {}",
                socket_path.display()
            ),
        ));
    }

    if let Some(parent) = socket_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let listen = format!("unix://{}", socket_path.display());
    Command::new(real_codex)
        .arg("app-server")
        .arg("--listen")
        .arg(&listen)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    Ok(())
}

pub fn daemon_socket_path(configured_socket: &str) -> PathBuf {
    let path: PathBuf = configured_socket
        .strip_prefix("unix://")
        .unwrap_or(configured_socket)
        .into();

    if configured_socket == default_daemon_socket() || configured_socket == unix_default_socket() {
        return workspace_daemon_socket_path().unwrap_or(path);
    }

    path
}

fn unix_default_socket() -> &'static str {
    "unix:///tmp/codex-hud/app-server.sock"
}

fn workspace_daemon_socket_path() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let hash = stable_path_hash(&cwd);
    Some(PathBuf::from(format!(
        "/tmp/codex-hud/app-server-{hash:016x}.sock"
    )))
}

fn stable_path_hash(path: &Path) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in path.as_os_str().to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(unix)]
fn socket_path_is_socket(path: &Path) -> bool {
    fs::metadata(path)
        .map(|metadata| metadata.file_type().is_socket())
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn socket_path_is_socket(_path: &Path) -> bool {
    false
}

fn ensure_app_server(
    real_codex: &Path,
    socket_path: &Path,
    config: &Config,
) -> std::io::Result<()> {
    if socket_path.exists() && !socket_path_is_socket(socket_path) {
        if config.daemon.auto_start {
            match fs::remove_file(socket_path) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(err),
            }
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!(
                    "app-server socket path exists but is not a socket: {}",
                    socket_path.display()
                ),
            ));
        }
    }

    if socket_path.exists()
        && socket_path_is_socket(socket_path)
        && config.daemon.reuse_shared_daemon
    {
        return Ok(());
    }

    if !config.daemon.auto_start {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!(
                "app-server socket does not exist: {}",
                socket_path.display()
            ),
        ));
    }

    if let Some(parent) = socket_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let listen = format!("unix://{}", socket_path.display());
    let mut child = Command::new(real_codex)
        .arg("app-server")
        .arg("--listen")
        .arg(&listen)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    let deadline = Instant::now() + Duration::from_millis(750);
    loop {
        if socket_path.exists() {
            return Ok(());
        }

        if let Some(status) = child.try_wait()? {
            return Err(std::io::Error::other(format!(
                "codex app-server exited before creating socket {listen}: {status}"
            )));
        }

        if Instant::now() >= deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("timed out waiting for codex app-server socket {listen}"),
            ));
        }

        std::thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(unix)]
pub fn exec_real_codex(
    real_codex: PathBuf,
    forwarded_args: Vec<OsString>,
    launcher_env: Option<LauncherEnvironment>,
) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;

    let mut command = Command::new(real_codex);
    command.args(forwarded_args);
    if let Some(environment) = launcher_env {
        for (key, value) in launcher_env_entries(&environment) {
            command.env(key, value);
        }
    }

    let err = command.exec();
    Err(err)
}

#[cfg(not(unix))]
pub fn exec_real_codex(
    real_codex: PathBuf,
    forwarded_args: Vec<OsString>,
    launcher_env: Option<LauncherEnvironment>,
) -> std::io::Result<()> {
    let mut command = Command::new(real_codex);
    command.args(forwarded_args);
    if let Some(environment) = launcher_env {
        for (key, value) in launcher_env_entries(&environment) {
            command.env(key, value);
        }
    }

    let status = command.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("codex exited with status {status}"),
        ))
    }
}

fn io_error(err: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(err.to_string())
}
