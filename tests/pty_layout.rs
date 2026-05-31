use codex_hud::pty::{choose_launcher_surface, reserve_bottom_rows, LauncherSurface, PtyLayout};
use codex_hud::pty::{launcher_env_entries, launcher_environment};

#[test]
fn reserves_at_least_one_row_for_the_main_view() {
    assert_eq!(
        reserve_bottom_rows(2, 4),
        PtyLayout {
            total_rows: 2,
            main_rows: 1,
            bottom_rows: 1,
        }
    );
    assert_eq!(
        reserve_bottom_rows(1, 4),
        PtyLayout {
            total_rows: 1,
            main_rows: 1,
            bottom_rows: 0,
        }
    );
}

#[test]
fn avoids_underflow_when_requested_bottom_rows_are_smaller() {
    assert_eq!(
        reserve_bottom_rows(8, 2),
        PtyLayout {
            total_rows: 8,
            main_rows: 6,
            bottom_rows: 2,
        }
    );
}

#[test]
fn chooses_inline_only_when_terminal_and_capacity_allow_it() {
    assert_eq!(
        choose_launcher_surface(Some("xterm-256color"), 8, 2),
        LauncherSurface::Inline
    );
    assert_eq!(
        choose_launcher_surface(Some("dumb"), 8, 2),
        LauncherSurface::Fallback
    );
    assert_eq!(
        choose_launcher_surface(Some("xterm-256color"), 1, 2),
        LauncherSurface::Fallback
    );
}

#[test]
fn launcher_environment_honors_configured_fallback_surface() {
    let environment = launcher_environment(
        Some("xterm-256color"),
        8,
        2,
        Some("fallback"),
        Some("split"),
    );

    assert_eq!(environment.surface, LauncherSurface::Fallback);
    assert_eq!(environment.layout.bottom_rows, 2);
    assert_eq!(environment.fallback_surface, "split");
}

#[test]
fn launcher_environment_uses_inline_statusbar_when_supported_terminal_has_capacity() {
    let environment = launcher_environment(
        Some("xterm-256color"),
        8,
        2,
        Some("inline-statusbar"),
        Some("split"),
    );

    assert_eq!(environment.surface, LauncherSurface::Inline);
    assert_eq!(environment.layout.main_rows, 6);
    assert_eq!(environment.layout.bottom_rows, 2);
}

#[test]
fn launcher_environment_falls_back_for_dumb_terminals_even_when_inline_is_requested() {
    let environment =
        launcher_environment(Some("dumb"), 8, 2, Some("inline-statusbar"), Some("split"));

    assert_eq!(environment.surface, LauncherSurface::Fallback);
    assert_eq!(environment.fallback_surface, "split");
}

#[test]
fn launcher_environment_exports_expected_env_pairs() {
    let environment =
        launcher_environment(Some("xterm-256color"), 8, 2, Some("inline"), Some("split"));
    let entries = launcher_env_entries(&environment);

    assert_eq!(
        entries[0],
        ("CODEX_HUD_LAUNCHER_SURFACE", "inline".to_string())
    );
    assert_eq!(entries[1], ("CODEX_HUD_LAYOUT_TOTAL_ROWS", "8".to_string()));
    assert_eq!(entries[2], ("CODEX_HUD_LAYOUT_MAIN_ROWS", "6".to_string()));
    assert_eq!(
        entries[3],
        ("CODEX_HUD_LAYOUT_BOTTOM_ROWS", "2".to_string())
    );
    assert_eq!(
        entries[4],
        ("CODEX_HUD_FALLBACK_SURFACE", "split".to_string())
    );
}
