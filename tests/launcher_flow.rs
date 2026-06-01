use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use codex_hud::config::Config;
use codex_hud::launcher::prepare_remote_launch;
use tempfile::tempdir;

#[test]
fn interactive_launch_injects_remote_and_launcher_environment() {
    let temp = tempdir().unwrap();
    let fake_codex = temp.path().join("codex");
    write_unix_remote_fake_codex(&fake_codex);
    let socket_path = temp.path().join("app-server.sock");
    write_config(
        temp.path(),
        &format!(
            r#"
[daemon]
socket = "{}"

[launcher]
surface = "fallback"
fallback_surface = "split"
status_rows = 3
"#,
            socket_path.display()
        ),
    );

    let output = run_codex(
        &temp,
        &[],
        Some(("TERM", "xterm-256color")),
        Some(("LINES", "10")),
    );

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains(&format!("ARGS:--remote unix://{}", socket_path.display())),
        "{stdout}"
    );
    assert!(socket_path.exists());
    assert!(stdout.contains("SURFACE:fallback"));
    assert!(stdout.contains("LAYOUT_TOTAL_ROWS:10"));
    assert!(stdout.contains("LAYOUT_MAIN_ROWS:7"));
    assert!(stdout.contains("LAYOUT_BOTTOM_ROWS:3"));
    assert!(stdout.contains("FALLBACK:split"));
}

#[test]
fn default_daemon_socket_is_scoped_to_the_launch_working_directory() {
    let temp = tempdir().unwrap();
    let fake_codex = temp.path().join("codex");
    write_unix_remote_fake_codex(&fake_codex);
    write_config(
        temp.path(),
        r#"
[launcher]
surface = "fallback"
fallback_surface = "split"
status_rows = 3
"#,
    );
    let first_cwd = temp.path().join("first");
    let second_cwd = temp.path().join("second");
    fs::create_dir_all(&first_cwd).unwrap();
    fs::create_dir_all(&second_cwd).unwrap();

    let first = run_codex_from(
        &temp,
        &first_cwd,
        &[],
        Some(("TERM", "xterm-256color")),
        None,
    );
    let second = run_codex_from(
        &temp,
        &second_cwd,
        &[],
        Some(("TERM", "xterm-256color")),
        None,
    );

    assert!(first.status.success());
    assert!(second.status.success());
    let first_stdout = String::from_utf8(first.stdout).unwrap();
    let second_stdout = String::from_utf8(second.stdout).unwrap();
    let first_remote = remote_arg_from_stdout(&first_stdout);
    let second_remote = remote_arg_from_stdout(&second_stdout);

    assert_ne!(first_remote, second_remote);
    assert!(first_remote.starts_with("unix:///tmp/codex-hud/app-server-"));
    assert!(second_remote.starts_with("unix:///tmp/codex-hud/app-server-"));
}

#[test]
fn interactive_launch_falls_back_for_unsupported_terminals_without_blocking_codex() {
    let temp = tempdir().unwrap();
    let fake_codex = temp.path().join("codex");
    write_unix_remote_fake_codex(&fake_codex);
    let socket_path = temp.path().join("app-server.sock");
    write_config(
        temp.path(),
        &format!(
            r#"
[daemon]
socket = "{}"

[launcher]
surface = "inline-statusbar"
fallback_surface = "split"
status_rows = 3
"#,
            socket_path.display()
        ),
    );

    let output = run_codex(&temp, &[], Some(("TERM", "dumb")), Some(("LINES", "10")));

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains(&format!("ARGS:--remote unix://{}", socket_path.display())),
        "{stdout}"
    );
    assert!(stdout.contains("SURFACE:fallback"));
    assert!(stdout.contains("FALLBACK:split"));
}

#[test]
fn bridge_launch_does_not_wait_for_app_server_socket() {
    let temp = tempdir().unwrap();
    let fake_codex = temp.path().join("codex");
    write_delayed_app_server_codex(&fake_codex);
    let socket_path = temp.path().join("app-server.sock");
    let config = Config {
        daemon: codex_hud::config::DaemonConfig {
            socket: socket_path.display().to_string(),
            auto_start: true,
            reuse_shared_daemon: true,
        },
        ..Config::default()
    };

    let started = Instant::now();
    let launch = prepare_remote_launch(&fake_codex, &[] as &[OsString], &config, true).unwrap();
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_millis(250),
        "remote launch waited for app-server socket: {elapsed:?}"
    );
    assert!(
        launch.forwarded_args[0].to_string_lossy() == "--remote",
        "{:?}",
        launch.forwarded_args
    );
    assert!(
        launch.forwarded_args[1]
            .to_string_lossy()
            .starts_with("ws://127.0.0.1:"),
        "{:?}",
        launch.forwarded_args
    );
    assert!(
        !socket_path.exists(),
        "socket should still be delayed when launch returns"
    );
}

#[test]
fn interactive_launch_recovers_from_stale_non_socket_daemon_paths() {
    let temp = tempdir().unwrap();
    let fake_codex = temp.path().join("codex");
    write_unix_remote_fake_codex(&fake_codex);
    let socket_path = temp.path().join("app-server.sock");
    let marker_path = temp.path().join("app-server.started");
    fs::write(&socket_path, b"stale").unwrap();
    write_config(
        temp.path(),
        &format!(
            r#"
[daemon]
socket = "{}"

[launcher]
surface = "fallback"
fallback_surface = "split"
status_rows = 3
"#,
            socket_path.display()
        ),
    );

    let output = run_codex(&temp, &["resume"], Some(("TERM", "xterm-256color")), None);

    assert!(output.status.success());
    assert!(marker_path.exists(), "app-server was not restarted");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains(&format!("ARGS:--remote unix://{}", socket_path.display())),
        "{stdout}"
    );
}

#[test]
fn interactive_launch_falls_back_to_plain_codex_when_remote_setup_fails() {
    let temp = tempdir().unwrap();
    let fake_codex = temp.path().join("codex");
    write_failing_app_server_codex(&fake_codex);
    write_config(
        temp.path(),
        r#"
[daemon]
socket = "/tmp/codex-hud-test-missing.sock"

[launcher]
surface = "fallback"
fallback_surface = "split"
status_rows = 3
"#,
    );

    let output = run_codex(&temp, &["resume"], Some(("TERM", "xterm-256color")), None);

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("ARGS:resume"));
    assert!(!stdout.contains("--remote"));
}

#[test]
fn launcher_disabled_in_config_bypasses_wrapper_behaviour() {
    let temp = tempdir().unwrap();
    let fake_codex = temp.path().join("codex");
    write_unix_remote_fake_codex(&fake_codex);
    write_config(
        temp.path(),
        r#"
[launcher]
enabled = false
"#,
    );

    let output = run_codex(&temp, &["resume"], Some(("TERM", "xterm-256color")), None);

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("ARGS:resume"));
    assert!(!stdout.contains("--remote"));
    assert!(stdout.contains("SURFACE:<unset>"));
}

#[test]
fn noninteractive_launch_keeps_original_arguments_and_skips_launcher_env() {
    let temp = tempdir().unwrap();
    let fake_codex = temp.path().join("codex");
    write_unix_remote_fake_codex(&fake_codex);
    write_config(
        temp.path(),
        r#"
[launcher]
surface = "fallback"
fallback_surface = "split"
status_rows = 3
"#,
    );

    let output = run_codex(&temp, &["exec", "hello"], None, None);

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("ARGS:exec hello"));
    assert!(stdout.contains("SURFACE:<unset>"));
    assert!(stdout.contains("LAYOUT_TOTAL_ROWS:<unset>"));
    assert!(stdout.contains("LAYOUT_MAIN_ROWS:<unset>"));
    assert!(stdout.contains("LAYOUT_BOTTOM_ROWS:<unset>"));
    assert!(stdout.contains("FALLBACK:<unset>"));
    assert!(!stdout.contains("--remote"));
}

#[test]
fn noninteractive_help_and_plugin_passthrough() {
    let temp = tempdir().unwrap();
    let fake_codex = temp.path().join("codex");
    write_passthrough_codex(&fake_codex);

    let help_output = run_codex(&temp, &["--help"], None, None);
    let plugin_output = run_codex(&temp, &["plugin", "list"], None, None);

    assert!(help_output.status.success());
    assert!(plugin_output.status.success());

    let help_stdout = String::from_utf8(help_output.stdout).unwrap();
    let plugin_stdout = String::from_utf8(plugin_output.stdout).unwrap();

    assert!(help_stdout.contains("HELP:--help"));
    assert!(plugin_stdout.contains("ARGS:plugin list"));
    assert!(plugin_stdout.contains("PLUGIN:plugin list"));
    assert!(!help_stdout.contains("--remote"));
    assert!(!plugin_stdout.contains("--remote"));
}

#[test]
fn launcher_preserves_the_real_codex_exit_code() {
    let temp = tempdir().unwrap();
    let fake_codex = temp.path().join("codex");
    write_exiting_fake_codex(&fake_codex);

    let output = run_codex(&temp, &[], None, Some(("LINES", "4")));

    assert_eq!(output.status.code(), Some(17));
}

fn run_codex(
    path_dir: &tempfile::TempDir,
    args: &[&str],
    term: Option<(&str, &str)>,
    lines: Option<(&str, &str)>,
) -> std::process::Output {
    run_codex_from(path_dir, std::path::Path::new("."), args, term, lines)
}

fn run_codex_from(
    path_dir: &tempfile::TempDir,
    cwd: &std::path::Path,
    args: &[&str],
    term: Option<(&str, &str)>,
    lines: Option<(&str, &str)>,
) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_codex"));
    command.env("PATH", path_dir.path());
    command.env("XDG_CONFIG_HOME", path_dir.path());
    command.current_dir(cwd);
    if let Some((key, value)) = term {
        command.env(key, value);
    }
    if let Some((key, value)) = lines {
        command.env(key, value);
    }
    command.args(args);

    command.output().unwrap()
}

fn remote_arg_from_stdout(stdout: &str) -> &str {
    stdout
        .lines()
        .find_map(|line| {
            line.strip_prefix("ARGS:--remote ")
                .and_then(|rest| rest.split_whitespace().next())
        })
        .unwrap_or_else(|| panic!("missing remote arg in stdout: {stdout}"))
}

fn write_config(temp_dir: &std::path::Path, contents: &str) {
    let config_dir = temp_dir.join("codex-hud");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(config_dir.join("config.toml"), contents).unwrap();
}

fn write_unix_remote_fake_codex(path: &PathBuf) {
    fs::write(
        path,
        r#"#!/bin/sh
if [ "${1:-}" = "--help" ]; then
  printf '%s\n' '--remote <ADDR>'
  printf '%s\n' 'Accepted forms: `ws://host:port`, `wss://host:port`, `unix://`, or `unix://PATH`.'
  exit 0
fi

if [ "${1:-}" = "app-server" ]; then
  listen="${3:-}"
  socket="${listen#unix://}"
  marker="${socket%.sock}.started"
  printf 'started\n' > "$marker"
  : > "$socket"
  exit 0
fi

printf 'ARGS:%s\n' "$*"
printf 'SURFACE:%s\n' "${CODEX_HUD_LAUNCHER_SURFACE:-<unset>}"
printf 'LAYOUT_TOTAL_ROWS:%s\n' "${CODEX_HUD_LAYOUT_TOTAL_ROWS:-<unset>}"
printf 'LAYOUT_MAIN_ROWS:%s\n' "${CODEX_HUD_LAYOUT_MAIN_ROWS:-<unset>}"
printf 'LAYOUT_BOTTOM_ROWS:%s\n' "${CODEX_HUD_LAYOUT_BOTTOM_ROWS:-<unset>}"
printf 'FALLBACK:%s\n' "${CODEX_HUD_FALLBACK_SURFACE:-<unset>}"
"#,
    )
    .unwrap();
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

fn write_failing_app_server_codex(path: &PathBuf) {
    fs::write(
        path,
        r#"#!/bin/sh
if [ "${1:-}" = "--help" ]; then
  printf '%s\n' '--remote <ADDR>'
  printf '%s\n' 'Accepted forms: `ws://host:port`, `wss://host:port`.'
  exit 0
fi

if [ "${1:-}" = "app-server" ]; then
  exit 42
fi

printf 'ARGS:%s\n' "$*"
printf 'SURFACE:%s\n' "${CODEX_HUD_LAUNCHER_SURFACE:-<unset>}"
"#,
    )
    .unwrap();
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

fn write_delayed_app_server_codex(path: &PathBuf) {
    fs::write(
        path,
        r#"#!/bin/sh
if [ "${1:-}" = "app-server" ]; then
  listen="${3:-}"
  socket="${listen#unix://}"
  sleep 1
  : > "$socket"
  exit 0
fi

printf 'ARGS:%s\n' "$*"
"#,
    )
    .unwrap();
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

fn write_exiting_fake_codex(path: &PathBuf) {
    fs::write(
        path,
        r#"#!/bin/sh
if [ "${1:-}" = "--help" ]; then
  printf '%s\n' '--remote <ADDR>'
  printf '%s\n' 'Accepted forms: `ws://host:port`, `wss://host:port`.'
  exit 0
fi

exit 17
"#,
    )
    .unwrap();
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

fn write_passthrough_codex(path: &PathBuf) {
    fs::write(
        path,
        r#"#!/bin/sh
printf 'ARGS:%s\n' "$*"
if [ "${1:-}" = "--help" ]; then
  printf 'HELP:%s\n' "$1"
  exit 0
fi

if [ "${1:-}" = "plugin" ]; then
  printf 'PLUGIN:%s %s\n' "$1" "${2:-}"
  exit 0
fi

exit 17
"#,
    )
    .unwrap();
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}
