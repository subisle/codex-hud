use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use codex_hud::wrapper::{
    build_remote_launch_args, cached_unix_remote_support, classify_launch, find_real_codex_in_path,
    LaunchDisposition,
};
use tempfile::tempdir;

fn os(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

#[test]
fn classifies_interactive_launches_as_intercepted() {
    assert_eq!(classify_launch(&os(&[])), LaunchDisposition::Intercept);
    assert_eq!(
        classify_launch(&os(&["resume"])),
        LaunchDisposition::Intercept
    );
    assert_eq!(
        classify_launch(&os(&["fork"])),
        LaunchDisposition::Intercept
    );
    assert_eq!(
        classify_launch(&os(&["--model", "o3", "hello"])),
        LaunchDisposition::Intercept
    );
}

#[test]
fn classifies_noninteractive_subcommands_as_passthrough() {
    assert_eq!(
        classify_launch(&os(&["exec", "hello"])),
        LaunchDisposition::Passthrough
    );
    assert_eq!(
        classify_launch(&os(&["review", "hello"])),
        LaunchDisposition::Passthrough
    );
    assert_eq!(
        classify_launch(&os(&["--help"])),
        LaunchDisposition::Passthrough
    );
    assert_eq!(
        classify_launch(&os(&["-V"])),
        LaunchDisposition::Passthrough
    );
}

#[test]
fn finds_the_later_real_codex_binary_and_skips_the_wrapper_itself() {
    let wrapper_dir = tempdir().unwrap();
    let real_dir = tempdir().unwrap();

    let wrapper_path = wrapper_dir.path().join("codex");
    let real_path = real_dir.path().join("codex");

    write_executable(&wrapper_path);
    write_executable(&real_path);

    let path_env = std::env::join_paths([wrapper_dir.path(), real_dir.path()]).unwrap();

    let found = find_real_codex_in_path(&path_env, &wrapper_path);
    assert_eq!(found, Some(real_path));
}

#[test]
fn detects_unix_remote_support_from_help_text() {
    let current_help = r#"
--remote <ADDR>
Accepted forms: `ws://host:port`, `wss://host:port`, `unix://`, or `unix://PATH`.
"#;
    let old_help = r#"
--remote <ADDR>
Accepted forms: `ws://host:port`, `wss://host:port`.
"#;

    assert!(codex_hud::wrapper::supports_unix_remote_help(current_help));
    assert!(!codex_hud::wrapper::supports_unix_remote_help(old_help));
}

#[test]
fn prepends_remote_args_without_reordering_the_original_arguments() {
    let forwarded = build_remote_launch_args(
        &os(&["resume", "--model", "o3", "hello"]),
        "ws://127.0.0.1:4500",
    );

    assert_eq!(
        forwarded,
        os(&[
            "--remote",
            "ws://127.0.0.1:4500",
            "resume",
            "--model",
            "o3",
            "hello",
        ])
    );
}

#[test]
fn wrapper_remote_overrides_user_remote_without_reordering_other_arguments() {
    let forwarded = build_remote_launch_args(
        &os(&[
            "resume",
            "--remote",
            "ws://user.example:1234",
            "--model",
            "o3",
            "--remote=ws://inline.example:5678",
            "hello",
        ]),
        "unix:///tmp/codex-hud/app-server.sock",
    );

    assert_eq!(
        forwarded,
        os(&[
            "--remote",
            "unix:///tmp/codex-hud/app-server.sock",
            "resume",
            "--model",
            "o3",
            "hello",
        ])
    );
}

#[test]
fn caches_unix_remote_support_per_binary_path() {
    let unix_dir = tempdir().unwrap();
    let ws_dir = tempdir().unwrap();

    let unix_codex = unix_dir.path().join("codex");
    let ws_codex = ws_dir.path().join("codex");

    write_help_script(
        &unix_codex,
        r#"#!/bin/sh
cat <<'EOF'
--remote <ADDR>
Accepted forms: `ws://host:port`, `wss://host:port`, `unix://`, or `unix://PATH`.
EOF
"#,
    );
    write_help_script(
        &ws_codex,
        r#"#!/bin/sh
cat <<'EOF'
--remote <ADDR>
Accepted forms: `ws://host:port`, `wss://host:port`.
EOF
"#,
    );

    assert!(cached_unix_remote_support(&unix_codex));
    assert!(!cached_unix_remote_support(&ws_codex));
}

fn write_executable(path: &PathBuf) {
    fs::write(path, b"#!/bin/sh\nexit 0\n").unwrap();
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

fn write_help_script(path: &PathBuf, script: &str) {
    fs::write(path, script.as_bytes()).unwrap();
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}
