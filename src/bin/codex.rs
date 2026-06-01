use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{self, IsTerminal, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, RecvTimeoutError, Sender},
    Arc, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};

use codex_hud::bridge::LocalWsBridgeHandle;
use codex_hud::config::Config;
use codex_hud::hud::{apply_app_server_message, HudSnapshot, LocalContext};
use codex_hud::protocol::{AppServerClient, ClientInfo};
use codex_hud::pty::{
    launcher_env_entries, launcher_environment, terminal_size_from_runtime_or_env,
    LauncherEnvironment,
};
use codex_hud::surface::render_compact_ansi;
use codex_hud::wrapper::{
    build_remote_launch_args, cached_unix_remote_support, classify_launch, find_real_codex_in_path,
    LaunchDisposition,
};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::style::{Color, ResetColor, SetBackgroundColor};
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

    let config = Config::load_from_env().unwrap_or_else(|_| Config::default());
    if !config.launcher.enabled {
        return exec_real_codex(real_codex, args, None);
    }

    let disposition = classify_launch(&args);
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
            let hud_events = prepared_launch
                .as_mut()
                .and_then(|prepared| prepared.hud_events.take());
            match run_interactive_pty_host(
                real_codex.clone(),
                forwarded_args.clone(),
                environment,
                daemon_socket_path(&config.daemon.socket),
                hud_events,
            ) {
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
    hud_events: Option<Receiver<serde_json::Value>>,
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

    if allow_ws_bridge && socket_path_is_socket(&socket_path) {
        let runtime = tokio::runtime::Runtime::new().map_err(io_error)?;
        let (hud_events_tx, hud_events_rx) = mpsc::channel();
        let bridge = runtime
            .block_on(codex_hud::bridge::spawn_local_ws_bridge_with_observer(
                &socket_path,
                Some(hud_events_tx),
            ))
            .map_err(io_error)?;
        let remote_url = OsString::from(bridge.local_url());

        return Ok(PreparedLaunch {
            forwarded_args: build_remote_launch_args(original_args, remote_url),
            _bridge: Some(bridge),
            _runtime: Some(runtime),
            hud_events: Some(hud_events_rx),
        });
    }

    if unix_remote_supported {
        let remote_url = OsString::from(format!("unix://{}", socket_path.display()));
        return Ok(PreparedLaunch {
            forwarded_args: build_remote_launch_args(original_args, remote_url),
            _bridge: None,
            _runtime: None,
            hud_events: None,
        });
    }

    Err(io::Error::other(
        "loopback ws bridge requires the wrapper process to remain active",
    ))
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

fn daemon_socket_path(configured_socket: &str) -> PathBuf {
    configured_socket
        .strip_prefix("unix://")
        .unwrap_or(configured_socket)
        .into()
}

fn ensure_app_server(real_codex: &Path, socket_path: &Path, config: &Config) -> io::Result<()> {
    if socket_path.exists() && !socket_path_is_socket(socket_path) {
        if config.daemon.auto_start {
            match fs::remove_file(socket_path) {
                Ok(()) => {}
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => return Err(err),
            }
        } else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
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
    app_server_socket: PathBuf,
    hud_events: Option<Receiver<serde_json::Value>>,
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
    let _raw_mode = RawModeGuard::enable()?;
    let stdout = Arc::new(Mutex::new(std::io::stdout()));
    let hud_snapshot = Arc::new(Mutex::new(launcher_hud_snapshot()));
    let output_stdout = Arc::clone(&stdout);
    let output_hud_snapshot = Arc::clone(&hud_snapshot);
    let output_launcher_env = launcher_env.clone();
    let output_thread = std::thread::spawn(move || {
        copy_pty_output(
            &mut reader,
            output_stdout,
            output_hud_snapshot,
            output_launcher_env,
        )
    });
    let hud_stop = Arc::new(AtomicBool::new(false));
    let (hud_dirty_tx, hud_dirty_rx) = mpsc::channel();
    let mut hud_collectors = spawn_hud_state_collectors(
        real_codex.clone(),
        app_server_socket,
        hud_events,
        Arc::clone(&hud_snapshot),
        Arc::clone(&hud_stop),
        hud_dirty_tx,
    );
    let result = (|| -> io::Result<i32> {
        let mut last_hud_draw = Instant::now();
        let snapshot = hud_snapshot
            .lock()
            .map_err(|_| io::Error::other("HUD snapshot lock poisoned"))?
            .clone();
        {
            let mut stdout = stdout
                .lock()
                .map_err(|_| io::Error::other("stdout lock poisoned"))?;
            draw_inline_hud(&mut *stdout, &launcher_env, cols, rows, &snapshot)?;
        }
        let mut last_hud_snapshot = Some(snapshot);

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
                        {
                            let snapshot = hud_snapshot
                                .lock()
                                .map_err(|_| io::Error::other("HUD snapshot lock poisoned"))?;
                            let snapshot = snapshot.clone();
                            let mut stdout = stdout
                                .lock()
                                .map_err(|_| io::Error::other("stdout lock poisoned"))?;
                            draw_inline_hud(&mut *stdout, &launcher_env, cols, rows, &snapshot)?;
                            last_hud_snapshot = Some(snapshot);
                        }
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

            let hud_dirty = drain_hud_dirty(&hud_dirty_rx);
            if hud_dirty || last_hud_draw.elapsed() >= Duration::from_secs(2) {
                let (cols, rows) = terminal_size_from_runtime_or_env();
                {
                    let snapshot = hud_snapshot
                        .lock()
                        .map_err(|_| io::Error::other("HUD snapshot lock poisoned"))?;
                    let snapshot = snapshot.clone();
                    if last_hud_snapshot.as_ref() != Some(&snapshot) {
                        let mut stdout = stdout
                            .lock()
                            .map_err(|_| io::Error::other("stdout lock poisoned"))?;
                        draw_inline_hud(&mut *stdout, &launcher_env, cols, rows, &snapshot)?;
                        last_hud_snapshot = Some(snapshot);
                    }
                }
                last_hud_draw = Instant::now();
            }
        };

        Ok(exit_code)
    })();

    if result.is_err() {
        let _ = child.kill();
    }
    drop(writer);
    hud_stop.store(true, Ordering::Relaxed);
    let collector_result = join_all_handles(&mut hud_collectors);
    let output_result = output_thread
        .join()
        .map_err(|_| io::Error::other("PTY output forwarding thread panicked"))?;

    let exit_code = result?;
    collector_result?;
    output_result?;
    Ok(exit_code)
}

fn launcher_hud_snapshot() -> HudSnapshot {
    HudSnapshot {
        thread_id: None,
        thread_name: Some("codex-hud".to_string()),
        model: None,
        model_provider: None,
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
        mcp_count: 0,
        skill_count: 0,
    }
}

fn spawn_hud_state_collectors(
    real_codex: PathBuf,
    socket_path: PathBuf,
    hud_events: Option<Receiver<serde_json::Value>>,
    snapshot: Arc<Mutex<HudSnapshot>>,
    stop: Arc<AtomicBool>,
    hud_dirty: Sender<()>,
) -> Vec<thread::JoinHandle<()>> {
    let mut handles = Vec::new();
    let consume_events = hud_events.is_none();

    if let Some(hud_events) = hud_events {
        let snapshot = Arc::clone(&snapshot);
        let stop = Arc::clone(&stop);
        let hud_dirty = hud_dirty.clone();
        handles.push(thread::spawn(move || {
            collect_bridge_events(hud_events, snapshot, stop, hud_dirty);
        }));
    }

    if socket_path.exists() {
        let snapshot = Arc::clone(&snapshot);
        let stop = Arc::clone(&stop);
        let hud_dirty = hud_dirty.clone();
        handles.push(thread::spawn(move || {
            let Ok(runtime) = tokio::runtime::Runtime::new() else {
                return;
            };

            runtime.block_on(async move {
                collect_hud_state(socket_path, snapshot, stop, consume_events, hud_dirty).await;
            });
        }));
    }

    {
        let snapshot = Arc::clone(&snapshot);
        let stop = Arc::clone(&stop);
        let hud_dirty = hud_dirty.clone();
        handles.push(thread::spawn(move || {
            collect_mcp_state(real_codex, snapshot, stop, hud_dirty);
        }));
    }

    handles
}

fn collect_mcp_state(
    real_codex: PathBuf,
    snapshot: Arc<Mutex<HudSnapshot>>,
    stop: Arc<AtomicBool>,
    hud_dirty: Sender<()>,
) {
    let mut last_count = None;

    while !stop.load(Ordering::Relaxed) {
        if let Ok(count) = read_enabled_mcp_count(&real_codex) {
            if last_count != Some(count) {
                if let Ok(mut guard) = snapshot.lock() {
                    if guard.mcp_count != count {
                        guard.mcp_count = count;
                        notify_hud_dirty(&hud_dirty);
                    }
                }
                last_count = Some(count);
            }
        }

        for _ in 0..20 {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            thread::sleep(Duration::from_millis(500));
        }
    }
}

fn read_enabled_mcp_count(real_codex: &Path) -> io::Result<u64> {
    let output = Command::new(real_codex)
        .args(["mcp", "list", "--json"])
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "codex mcp list --json exited with {status}",
            status = output.status
        )));
    }

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(io_error)?;
    Ok(count_enabled_mcp_servers(&value) as u64)
}

fn count_enabled_mcp_servers(value: &serde_json::Value) -> usize {
    match value {
        serde_json::Value::Array(items) => items
            .iter()
            .map(count_enabled_mcp_servers)
            .sum::<usize>(),
        serde_json::Value::Object(object) => {
            if let Some(servers) = object.get("servers").and_then(serde_json::Value::as_array) {
                return servers.iter().map(count_enabled_mcp_servers).sum();
            }

            if let Some(enabled) = object.get("enabled").and_then(serde_json::Value::as_bool) {
                return usize::from(enabled);
            }

            if object.contains_key("transport") || object.contains_key("command") || object.contains_key("url")
            {
                return 1;
            }

            0
        }
        _ => 0,
    }
}

fn collect_bridge_events(
    hud_events: Receiver<serde_json::Value>,
    snapshot: Arc<Mutex<HudSnapshot>>,
    stop: Arc<AtomicBool>,
    hud_dirty: Sender<()>,
) {
    while !stop.load(Ordering::Relaxed) {
        match hud_events.recv_timeout(Duration::from_millis(250)) {
            Ok(message) => {
                if let Ok(mut guard) = snapshot.lock() {
                    if apply_app_server_message(&mut guard, &message) {
                        notify_hud_dirty(&hud_dirty);
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

async fn collect_hud_state(
    socket_path: PathBuf,
    snapshot: Arc<Mutex<HudSnapshot>>,
    stop: Arc<AtomicBool>,
    consume_events: bool,
    hud_dirty: Sender<()>,
) {
    let client_info = ClientInfo {
        name: "codex-hud".to_string(),
        title: Some("Codex HUD".to_string()),
        version: env!("CARGO_PKG_VERSION").to_string(),
    };

    while !stop.load(Ordering::Relaxed) {
        let mut client = match AppServerClient::connect_unix(&socket_path).await {
            Ok(client) => client,
            Err(_) => {
                tokio::time::sleep(Duration::from_millis(250)).await;
                continue;
            }
        };

        let initialized = tokio::time::timeout(
            Duration::from_secs(1),
            client.initialize(client_info.clone()),
        )
        .await;
        if !matches!(initialized, Ok(Ok(_))) {
            tokio::time::sleep(Duration::from_millis(250)).await;
            continue;
        }
        let initialized =
            tokio::time::timeout(Duration::from_millis(500), client.initialized()).await;
        if !matches!(initialized, Ok(Ok(()))) {
            tokio::time::sleep(Duration::from_millis(250)).await;
            continue;
        }

        let rate_limits = tokio::time::timeout(
            Duration::from_millis(500),
            client.account_rate_limits_read(),
        )
        .await;
        if let Ok(Ok(rate_limits)) = rate_limits {
            let mut guard = match snapshot.lock() {
                Ok(guard) => guard,
                Err(_) => return,
            };
            if apply_app_server_message(&mut guard, &serde_json::json!({"result": rate_limits})) {
                notify_hud_dirty(&hud_dirty);
            }
        }

        if consume_events {
            loop {
                if stop.load(Ordering::Relaxed) {
                    return;
                }

                let message =
                    match tokio::time::timeout(Duration::from_millis(250), client.next_message())
                        .await
                    {
                        Ok(Ok(message)) => message,
                        Ok(Err(_)) => break,
                        Err(_) => continue,
                    };

                if let Ok(mut guard) = snapshot.lock() {
                    if apply_app_server_message(&mut guard, &message) {
                        notify_hud_dirty(&hud_dirty);
                    }
                }
            }
        } else {
            let mut known_thread_id = snapshot
                .lock()
                .ok()
                .and_then(|guard| guard.thread_id.clone());

            loop {
                if stop.load(Ordering::Relaxed) {
                    return;
                }

                let current_thread_id = snapshot
                    .lock()
                    .ok()
                    .and_then(|guard| guard.thread_id.clone());

                if current_thread_id.is_some() && current_thread_id != known_thread_id {
                    if let Some(thread_id) = current_thread_id.clone() {
                        let thread = tokio::time::timeout(
                            Duration::from_millis(500),
                            client.thread_read(&thread_id, false),
                        )
                        .await;
                        if let Ok(Ok(thread)) = thread {
                            if let Ok(mut guard) = snapshot.lock() {
                                if apply_app_server_message(
                                    &mut guard,
                                    &serde_json::json!({"result": thread}),
                                ) {
                                    notify_hud_dirty(&hud_dirty);
                                }
                            }
                        }
                        known_thread_id = current_thread_id;
                    }
                }

                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn join_all_handles(handles: &mut Vec<thread::JoinHandle<()>>) -> io::Result<()> {
    for handle in handles.drain(..) {
        handle
            .join()
            .map_err(|_| io::Error::other("HUD collector thread panicked"))?;
    }
    Ok(())
}

fn drain_hud_dirty(hud_dirty: &Receiver<()>) -> bool {
    let mut dirty = false;
    while hud_dirty.try_recv().is_ok() {
        dirty = true;
    }
    dirty
}

fn notify_hud_dirty(hud_dirty: &Sender<()>) {
    let _ = hud_dirty.send(());
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
    let mut lines = render_compact_ansi(snapshot, cols as usize);
    lines.truncate(bottom_rows as usize);
    let hud_background = Color::Rgb {
        r: 11,
        g: 16,
        b: 32,
    };

    queue!(stdout, cursor::SavePosition)?;
    for row_offset in 0..bottom_rows {
        let row = start_row + row_offset;
        queue!(
            stdout,
            cursor::MoveTo(0, row),
            SetBackgroundColor(hud_background),
            terminal::Clear(terminal::ClearType::CurrentLine)
        )?;
        queue!(stdout, cursor::MoveTo(0, row), ResetColor)?;
        if let Some(line) = lines.get(row_offset as usize) {
            write!(stdout, "{line}")?;
        }
    }
    queue!(stdout, ResetColor, cursor::RestorePosition)?;
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

fn copy_pty_output(
    reader: &mut dyn Read,
    stdout: Arc<Mutex<impl Write + Send + 'static>>,
    hud_snapshot: Arc<Mutex<HudSnapshot>>,
    launcher_env: LauncherEnvironment,
) -> io::Result<()> {
    let mut buffer = [0_u8; 8192];

    loop {
        let bytes_read = reader.read(&mut buffer)?;
        if bytes_read == 0 {
            return Ok(());
        }

        let snapshot = hud_snapshot
            .lock()
            .map_err(|_| io::Error::other("HUD snapshot lock poisoned"))?
            .clone();
        let (cols, rows) = terminal_size_from_runtime_or_env();
        let mut stdout = stdout
            .lock()
            .map_err(|_| io::Error::other("stdout lock poisoned"))?;
        stdout.write_all(&buffer[..bytes_read])?;
        draw_inline_hud(&mut *stdout, &launcher_env, cols, rows, &snapshot)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use codex_hud::pty::{LauncherSurface, PtyLayout};
    use std::io::Cursor;

    fn test_launcher_environment() -> LauncherEnvironment {
        LauncherEnvironment::new(
            LauncherSurface::Inline,
            "split",
            PtyLayout {
                total_rows: 8,
                main_rows: 6,
                bottom_rows: 2,
            },
        )
    }

    fn test_snapshot() -> HudSnapshot {
        HudSnapshot {
            thread_id: Some("thr_123".to_string()),
            thread_name: Some("build-agent".to_string()),
            model: Some("gpt-5.4".to_string()),
            model_provider: Some("openai".to_string()),
            turn_status: Some("running".to_string()),
            token_usage: Some(codex_hud::hud::TokenUsage {
                used: 9_216,
                limit: 12_800,
            }),
            rate_limit: None,
            local: LocalContext {
                cwd: Some("/Users/me/codex-hud".to_string()),
                git_branch: Some("main".to_string()),
                git_dirty: true,
            },
            goal: None,
            plan: None,
            mcp_summary: None,
            tool_summary: None,
            mcp_count: 0,
            skill_count: 3,
        }
    }

    #[test]
    fn draw_inline_hud_does_not_emit_full_width_blank_fill() {
        let mut output = Vec::new();
        let env = test_launcher_environment();

        draw_inline_hud(&mut output, &env, 12, 4, &test_snapshot()).unwrap();

        let rendered = String::from_utf8(output).unwrap();
        assert!(!rendered.contains(&" ".repeat(12)), "{rendered:?}");
    }

    #[test]
    fn copy_pty_output_redraws_hud_after_forwarding_output() {
        let mut reader = Cursor::new(b"hello from codex\n".to_vec());
        let stdout = Arc::new(Mutex::new(Vec::new()));
        let hud_snapshot = Arc::new(Mutex::new(test_snapshot()));

        copy_pty_output(
            &mut reader,
            Arc::clone(&stdout),
            hud_snapshot,
            test_launcher_environment(),
        )
        .unwrap();

        let rendered = String::from_utf8(stdout.lock().unwrap().clone()).unwrap();
        assert!(rendered.contains("hello from codex"), "{rendered:?}");
        assert!(rendered.contains("codex-hud"), "{rendered:?}");
        assert!(rendered.contains("来源"), "{rendered:?}");
        assert!(rendered.contains("openai"), "{rendered:?}");
    }

    #[test]
    fn counts_enabled_mcp_servers_from_json_payloads() {
        let value = serde_json::json!([
            { "name": "alpha", "enabled": true },
            { "name": "beta", "enabled": false },
            {
                "name": "gamma",
                "transport": { "type": "stdio" }
            }
        ]);

        assert_eq!(count_enabled_mcp_servers(&value), 2);
    }
}
