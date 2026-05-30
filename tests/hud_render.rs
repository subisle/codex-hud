use codex_hud::hud::{HudSnapshot, LocalContext, RateLimitSummary, TokenUsage};
use codex_hud::surface::{render_compact, render_expanded};

fn snapshot() -> HudSnapshot {
    HudSnapshot {
        thread_id: Some("thr_123".to_string()),
        thread_name: Some("build-agent".to_string()),
        model: Some("gpt-5.4".to_string()),
        turn_status: Some("running".to_string()),
        token_usage: Some(TokenUsage {
            used: 9_200,
            limit: 12_800,
        }),
        rate_limit: Some(RateLimitSummary {
            used_percent: 42,
            limit_label: Some("codex".to_string()),
        }),
        local: LocalContext {
            cwd: Some("/Users/me/codex-hud".to_string()),
            git_branch: Some("main".to_string()),
            git_dirty: true,
        },
        goal: Some("ship transport layer".to_string()),
        plan: Some("finish task 4".to_string()),
        mcp_summary: Some("2 servers".to_string()),
        tool_summary: Some("1 active tool".to_string()),
    }
}

#[test]
fn compact_rendering_stays_dense_and_contains_core_fields() {
    let lines = render_compact(&snapshot(), 80);
    let rendered = lines.join("\n");

    assert!(lines.len() <= 2);
    assert!(lines.iter().all(|line| line.len() <= 80));
    assert!(rendered.contains("build-agent"));
    assert!(rendered.contains("gpt-5.4"));
    assert!(rendered.contains("42%"));
    assert!(rendered.contains("main"));
}

#[test]
fn compact_rendering_truncates_instead_of_expanding() {
    let lines = render_compact(&snapshot(), 48);

    assert!(lines.len() <= 2);
    assert!(lines.iter().all(|line| line.len() <= 48));
}

#[test]
fn expanded_rendering_surfaces_goal_plan_mcp_and_tools() {
    let lines = render_expanded(&snapshot(), 100);
    let rendered = lines.join("\n");

    assert!(rendered.contains("plan"));
    assert!(rendered.contains("goal"));
    assert!(rendered.contains("MCP"));
    assert!(rendered.contains("tool"));
    assert!(rendered.contains("rate"));
}
