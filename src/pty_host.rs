use std::env;
use std::ffi::OsString;
use std::io::{self, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::style::{Color, ResetColor, SetBackgroundColor};
use crossterm::terminal;
use crossterm::{cursor, queue};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};

use crate::config::QuotaConfig;
use crate::hud::{HudSnapshot, LocalContext};
use crate::hud_collectors::HudCollectors;
use crate::pty::{
    launcher_env_entries, terminal_size_from_runtime_or_env, LauncherEnvironment, LauncherSurface,
};
use crate::surface::render_compact_ansi;

pub fn should_use_pty_host() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal() && io::stderr().is_terminal()
}

pub fn run_interactive_pty_host(
    real_codex: PathBuf,
    forwarded_args: Vec<OsString>,
    launcher_env: LauncherEnvironment,
    app_server_socket: PathBuf,
    hud_events: Option<Receiver<serde_json::Value>>,
    quota_config: QuotaConfig,
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
    let stdout = Arc::new(Mutex::new(io::stdout()));
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
    let hud_collectors = HudCollectors::spawn(
        real_codex,
        app_server_socket,
        hud_events,
        Arc::clone(&hud_snapshot),
        Arc::clone(&hud_stop),
        hud_dirty_tx,
        quota_config,
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
    let collector_result = hud_collectors.join();
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

fn draw_inline_hud(
    stdout: &mut impl Write,
    launcher_env: &LauncherEnvironment,
    cols: u16,
    rows: u16,
    snapshot: &HudSnapshot,
) -> io::Result<()> {
    if launcher_env.layout.bottom_rows == 0
        || !matches!(launcher_env.surface, LauncherSurface::Inline)
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

fn drain_hud_dirty(hud_dirty: &Receiver<()>) -> bool {
    let mut dirty = false;
    while hud_dirty.try_recv().is_ok() {
        dirty = true;
    }
    dirty
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hud::TokenUsage;
    use crate::pty::PtyLayout;
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
            token_usage: Some(TokenUsage {
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
}
