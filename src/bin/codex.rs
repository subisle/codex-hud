use std::env;
use std::ffi::OsString;
use std::io;

use codex_hud::config::Config;
use codex_hud::launcher::{daemon_socket_path, exec_real_codex, prepare_remote_launch};
use codex_hud::pty::{launcher_environment, terminal_size_from_runtime_or_env};
use codex_hud::pty_host::{run_interactive_pty_host, should_use_pty_host};
use codex_hud::wrapper::{classify_launch, find_real_codex_in_path, LaunchDisposition};

fn main() {
    if let Err(err) = run() {
        eprintln!("codex wrapper error: {err}");
        std::process::exit(1);
    }
}

fn run() -> io::Result<()> {
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    let current_exe = env::current_exe()?;
    let path_env = codex_hud::wrapper::prepare_real_codex_environment(&current_exe);
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
                config.quota.clone(),
            ) {
                Ok(exit_code) => std::process::exit(exit_code),
                Err(err) => {
                    eprintln!("codex HUD PTY host failed, falling back to plain Codex: {err}");
                    return exec_real_codex(real_codex, args, None);
                }
            }
        }
    }

    let _prepared_launch = prepared_launch;
    exec_real_codex(real_codex, forwarded_args, launcher_env)
}
