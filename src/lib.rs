pub mod bridge;
pub mod config;
pub mod hud;
pub mod hud_collectors;
pub mod launcher;
pub mod protocol;
pub mod pty;
pub mod pty_host;
pub mod surface;
pub mod wrapper;

pub fn app_name() -> &'static str {
    "codex-hud"
}
