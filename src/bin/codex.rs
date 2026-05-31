use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use codex_hud::bridge::{spawn_local_ws_bridge, LocalWsBridgeHandle};
use codex_hud::config::Config;
use codex_hud::hud::{HudSnapshot, LocalContext};
use codex_hud::pty::{
    launcher_env_entries, launcher_environment, terminal_size_from_runtime_or_env,
    LauncherEnvironment,
};
use codex_hud::surface::render_compact;
use codex_hud::wrapper::{
    build_remote_launch_args, cached_unix_remote_support, classify_launch, find_real_codex_in_path,
    LaunchDisposition,
};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal;
use crossterm::{cursor, queue};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};

fn main() {
    if let Err(err) = run() {
        eprintln!("codex wrapper error: {err}");
        std::process::exit(1);
    }
}

fn run() -> io::Result<()> {
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    let current_exe = env::current_exe()?;
    let path_env = env::var_os("PATH").unwrap_or_default();
    let real_codex =
        find_real_codex_in_path(path_env.as_os_str(), &current_exe).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "unable to locate a real codex binary in PATH",
            )
        })?;

    let disposition = classify_launch(&args);
    let config = Config::load_from_env().unwrap_or_else(|_| Config::default());
    let launcher_env = if matches!(disposition, LaunchDisposition::Intercept) {
        let terminal_kind = env::var("TERM").ok();
        let (_, terminal_rows) = terminal_size_from_runtime_or_env();
        Some(launcher_environment(
            terminal_kind.as_deref(),
            terminal_rows,
            config.launcher.status_rows,
            Some(config.launcher.surface.as_str()),
            Some(config.launcher.fallback_surface.as_str()),
        ))
    } else {
        None
    };

    let use_pty_host = matches!(disposition, LaunchDisposition::Intercept) && should_use_pty_host();
    let mut prepared_launch = None;
    let forwarded_args = match disposition {
        LaunchDisposition::Intercept => {
            match prepare_remote_launch(&real_codex, &args, &config, use_pty_host) {
                Ok(prepared) => {
                    let forwarded_args = prepared.forwarded_args.clone();
                    prepared_launch = Some(prepared);
                    forwarded_args
                }
                Err(err) => {
                    eprintln!("codex HUD remote setup failed, falling back to plain Codex: {err}");
                    args.clone()
                }
            }
        }
        LaunchDisposition::Passthrough => args.clone(),
    };

    if use_pty_host {
        if let Some(environment) = launcher_env.clone() {
            match run_interactive_pty_host(real_codex.clone(), forwarded_args.clone(), environment)
            {
                Ok(exit_code) => std::process::exit(exit_code),
                Err(err) => {
                    eprintln!("codex HUD PTY host failed, falling back to plain Codex: {err}");
                }
            }
        }
    }

    let _prepared_launch = prepared_launch;
    exec_real_codex(real_codex, forwarded_args, launcher_env)
}

struct PreparedLaunch {
    forwarded_args: Vec<OsString>,
    _bridge: Option<LocalWsBridgeHandle>,
    _runtime: Option<tokio::runtime::Runtime>,
}

fn prepare_remote_launch(
    real_codex: &Path,
    original_args: &[OsString],
    config: &Config,
    allow_ws_bridge: bool,
) -> io::Result<PreparedLaunch> {
    let socket_path = daemon_socket_path(&config.daemon.socket);
    let unix_remote_supported = cached_unix_remote_support(real_codex);

    ensure_app_server(real_codex, &socket_path, config)?;

    if unix_remote_supported {
        let remote_url = OsString::from(format!("unix://{}", socket_path.display()));
        return Ok(PreparedLaunch {
            forwarded_args: build_remote_launch_args(original_args, remote_url),
            _bridge: None,
            _runtime: None,
        });
    }

    if !allow_ws_bridge {
        return Err(io::Error::other(
            "loopback ws bridge requires the wrapper process to remain active",
        ));
    }

    let runtime = tokio::runtime::Runtime::new().map_err(io_error)?;
    let bridge = runtime
        .block_on(spawn_local_ws_bridge(&socket_path))
        .map_err(io_error)?;
    let remote_url = OsString::from(bridge.local_url());

    Ok(PreparedLaunch {
        forwarded_args: build_remote_launch_args(original_args, remote_url),
        _bridge: Some(bridge),
        _runtime: Some(runtime),
    })
}

fn daemon_socket_path(configured_socket: &str) -> PathBuf {
    configured_socket
        .strip_prefix("unix://")
        .unwrap_or(configured_socket)
        .into()
}

fn ensure_app_server(real_codex: &Path, socket_path: &Path, config: &Config) -> io::Result<()> {
    if socket_path.exists() && config.daemon.reuse_shared_daemon {
        return Ok(());
    }

    if !config.daemon.auto_start {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
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
            return Err(io::Error::other(format!(
                "codex app-server exited before creating socket {listen}: {status}"
            )));
        }

        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("timed out waiting for codex app-server socket {listen}"),
            ));
        }

        std::thread::sleep(Duration::from_millis(25));
    }
}

fn should_use_pty_host() -> bool {
    std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal()
        && std::io::stderr().is_terminal()
}

fn run_interactive_pty_host(
    real_codex: PathBuf,
    forwarded_args: Vec<OsString>,
    launcher_env: LauncherEnvironment,
) -> io::Result<i32> {
    let (cols, rows) = terminal_size_from_runtime_or_env();
    let pty_rows = pty_rows_for_terminal(rows, launcher_env.layout.bottom_rows);
    let pty_size = PtySize {
        rows: pty_rows,
        cols: cols.max(1),
        pixel_width: 0,
        pixel_height: 0,
    };

    let pty_system = native_pty_system();
    let pair = pty_system.openpty(pty_size).map_err(io_error)?;
    let mut command = CommandBuilder::new(real_codex.as_os_str());
    command.args(forwarded_args);
    for (key, value) in launcher_env_entries(&launcher_env) {
        command.env(key, value);
    }

    let mut child = pair.slave.spawn_command(command).map_err(io_error)?;
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().map_err(io_error)?;
    let mut writer = pair.master.take_writer().map_err(io_error)?;
    let output_thread = std::thread::spawn(move || copy_pty_output(&mut reader));
    let _raw_mode = RawModeGuard::enable()?;
    let mut stdout = std::io::stdout();
    let hud_snapshot = launcher_hud_snapshot();
    let mut last_hud_draw = Instant::now();
    draw_inline_hud(&mut stdout, &launcher_env, cols, rows, &hud_snapshot)?;

    let exit_code = loop {
        if let Some(status) = child.try_wait()? {
            break status.exit_code() as i32;
        }

        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                Event::Resize(cols, rows) => {
                    let size = PtySize {
                        rows: pty_rows_for_terminal(rows, launcher_env.layout.bottom_rows),
                        cols: cols.max(1),
                        pixel_width: 0,
                        pixel_height: 0,
                    };
                    let _ = pair.master.resize(size);
                    draw_inline_hud(&mut stdout, &launcher_env, cols, rows, &hud_snapshot)?;
                    last_hud_draw = Instant::now();
                }
                Event::Key(key) => {
                    if let Some(bytes) = encode_key_event(key) {
                        writer.write_all(&bytes)?;
                        writer.flush()?;
                    }
                }
                Event::Paste(text) => {
                    writer.write_all(text.as_bytes())?;
                    writer.flush()?;
                }
                Event::FocusGained | Event::FocusLost | Event::Mouse(_) => {}
            }
        }

        if last_hud_draw.elapsed() >= Duration::from_millis(500) {
            let (cols, rows) = terminal_size_from_runtime_or_env();
            draw_inline_hud(&mut stdout, &launcher_env, cols, rows, &hud_snapshot)?;
            last_hud_draw = Instant::now();
        }
    };

    drop(writer);
    let _ = output_thread
        .join()
        .map_err(|_| io::Error::other("PTY output forwarding thread panicked"))?;

    Ok(exit_code)
}

fn launcher_hud_snapshot() -> HudSnapshot {
    HudSnapshot {
        thread_id: None,
        thread_name: Some("Codex HUD".to_string()),
        model: None,
        turn_status: Some("launcher active".to_string()),
        token_usage: None,
        rate_limit: None,
        local: LocalContext {
            cwd: env::current_dir()
                .ok()
                .map(|path| path.display().to_string()),
            git_branch: current_git_branch(),
            git_dirty: git_worktree_is_dirty(),
        },
        goal: None,
        plan: None,
        mcp_summary: None,
        tool_summary: None,
    }
}

fn draw_inline_hud(
    stdout: &mut impl Write,
    launcher_env: &LauncherEnvironment,
    cols: u16,
    rows: u16,
    snapshot: &HudSnapshot,
) -> io::Result<()> {
    if launcher_env.layout.bottom_rows == 0
        || !matches!(
            launcher_env.surface,
            codex_hud::pty::LauncherSurface::Inline
        )
    {
        return Ok(());
    }

    let bottom_rows = launcher_env.layout.bottom_rows.min(rows);
    let start_row = rows.saturating_sub(bottom_rows);
    let mut lines = render_compact(snapshot, cols as usize);
    lines.truncate(bottom_rows as usize);

    queue!(stdout, cursor::SavePosition)?;
    for row_offset in 0..bottom_rows {
        queue!(
            stdout,
            cursor::MoveTo(0, start_row + row_offset),
            terminal::Clear(terminal::ClearType::CurrentLine)
        )?;
        if let Some(line) = lines.get(row_offset as usize) {
            write!(stdout, "{line}")?;
        }
    }
    queue!(stdout, cursor::RestorePosition)?;
    stdout.flush()
}

fn current_git_branch() -> Option<String> {
    let output = Command::new("git")
        .args(["branch", "--show-current"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if branch.is_empty() {
        None
    } else {
        Some(branch)
    }
}

fn git_worktree_is_dirty() -> bool {
    let Ok(output) = Command::new("git").args(["status", "--porcelain"]).output() else {
        return false;
    };

    output.status.success() && !output.stdout.is_empty()
}

fn copy_pty_output(reader: &mut dyn Read) -> io::Result<()> {
    let mut stdout = std::io::stdout();
    let mut buffer = [0_u8; 8192];

    loop {
        let bytes_read = reader.read(&mut buffer)?;
        if bytes_read == 0 {
            return Ok(());
        }

        stdout.write_all(&buffer[..bytes_read])?;
        stdout.flush()?;
    }
}

fn pty_rows_for_terminal(total_rows: u16, bottom_rows: u16) -> u16 {
    total_rows.saturating_sub(bottom_rows).max(1)
}

fn encode_key_event(key: KeyEvent) -> Option<Vec<u8>> {
    if matches!(key.kind, KeyEventKind::Release) {
        return None;
    }

    let bytes: &[u8] = match key.code {
        KeyCode::Backspace => b"\x7f",
        KeyCode::Enter => b"\r",
        KeyCode::Left => b"\x1b[D",
        KeyCode::Right => b"\x1b[C",
        KeyCode::Up => b"\x1b[A",
        KeyCode::Down => b"\x1b[B",
        KeyCode::Home => b"\x1b[H",
        KeyCode::End => b"\x1b[F",
        KeyCode::PageUp => b"\x1b[5~",
        KeyCode::PageDown => b"\x1b[6~",
        KeyCode::Tab => b"\t",
        KeyCode::BackTab => b"\x1b[Z",
        KeyCode::Delete => b"\x1b[3~",
        KeyCode::Insert => b"\x1b[2~",
        KeyCode::Esc => b"\x1b",
        KeyCode::F(1) => b"\x1bOP",
        KeyCode::F(2) => b"\x1bOQ",
        KeyCode::F(3) => b"\x1bOR",
        KeyCode::F(4) => b"\x1bOS",
        KeyCode::F(5) => b"\x1b[15~",
        KeyCode::F(6) => b"\x1b[17~",
        KeyCode::F(7) => b"\x1b[18~",
        KeyCode::F(8) => b"\x1b[19~",
        KeyCode::F(9) => b"\x1b[20~",
        KeyCode::F(10) => b"\x1b[21~",
        KeyCode::F(11) => b"\x1b[23~",
        KeyCode::F(12) => b"\x1b[24~",
        KeyCode::Char(ch) => return encode_char_key(ch, key.modifiers),
        KeyCode::Null
        | KeyCode::CapsLock
        | KeyCode::ScrollLock
        | KeyCode::NumLock
        | KeyCode::PrintScreen
        | KeyCode::Pause
        | KeyCode::Menu
        | KeyCode::KeypadBegin
        | KeyCode::Media(_)
        | KeyCode::Modifier(_) => return None,
        KeyCode::F(_) => return None,
    };

    if key.modifiers.contains(KeyModifiers::ALT) {
        let mut prefixed = Vec::with_capacity(bytes.len() + 1);
        prefixed.push(0x1b);
        prefixed.extend_from_slice(bytes);
        Some(prefixed)
    } else {
        Some(bytes.to_vec())
    }
}

fn encode_char_key(ch: char, modifiers: KeyModifiers) -> Option<Vec<u8>> {
    if modifiers.contains(KeyModifiers::CONTROL) {
        return control_char(ch).map(|byte| {
            if modifiers.contains(KeyModifiers::ALT) {
                vec![0x1b, byte]
            } else {
                vec![byte]
            }
        });
    }

    let mut bytes = Vec::new();
    if modifiers.contains(KeyModifiers::ALT) {
        bytes.push(0x1b);
    }

    let mut encoded = [0_u8; 4];
    bytes.extend_from_slice(ch.encode_utf8(&mut encoded).as_bytes());
    Some(bytes)
}

fn control_char(ch: char) -> Option<u8> {
    match ch {
        '@' => Some(0x00),
        'a'..='z' | 'A'..='Z' => Some(ch.to_ascii_lowercase() as u8 - b'a' + 1),
        '[' => Some(0x1b),
        '\\' => Some(0x1c),
        ']' => Some(0x1d),
        '^' => Some(0x1e),
        '_' => Some(0x1f),
        '?' => Some(0x7f),
        _ => None,
    }
}

fn io_error(err: impl std::fmt::Display) -> io::Error {
    io::Error::other(err.to_string())
}

struct RawModeGuard;

impl RawModeGuard {
    fn enable() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
    }
}

#[cfg(unix)]
fn exec_real_codex(
    real_codex: std::path::PathBuf,
    forwarded_args: Vec<OsString>,
    launcher_env: Option<LauncherEnvironment>,
) -> io::Result<()> {
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
fn exec_real_codex(
    real_codex: std::path::PathBuf,
    forwarded_args: Vec<OsString>,
    launcher_env: Option<LauncherEnvironment>,
) -> io::Result<()> {
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
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!("codex exited with status {status}"),
        ))
    }
}
