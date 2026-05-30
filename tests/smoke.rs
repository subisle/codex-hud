use codex_hud::app_name;

#[test]
fn crate_loads_and_exposes_app_name() {
    assert_eq!(app_name(), "codex-hud");
}
